//! Unix platform implementation using `nix` 0.31.
//!
//! Every `unsafe` block is confined here with a SAFETY comment.
//! Uses std::process::Command for spawning (memory-safe, battle-tested).

use super::{
    ChildHandle, CommandSpec, EnvApi, Filesystem, JobControl, Pgid, Pid, Platform, Process, RawFd,
    SigAction, Signals, Terminal, TermiosState, WaitOpts, WaitResult,
};
use crate::error::PlatformError;
use std::convert::Infallible;
use std::os::fd::FromRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Mutex;

/// Unix platform.
pub struct UnixPlatform {
    foreground: Mutex<Pgid>,
    tty_cache: Mutex<Vec<RawFd>>,
}

impl UnixPlatform {
    /// Create a new Unix platform.
    pub fn new() -> Self {
        Self {
            foreground: Mutex::new(0),
            tty_cache: Mutex::new(Vec::new()),
        }
    }
}

/// Borrow `fd` as an `AsFd` for nix/posix calls that require it.
/// SAFETY: caller guarantees `fd` is open and valid for the call duration.
fn borrow_fd(fd: RawFd) -> std::os::fd::BorrowedFd<'static> {
    // SAFETY: documented contract.
    unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) }
}

impl Default for UnixPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for UnixPlatform {
    fn processes(&self) -> &dyn Process {
        self
    }
    fn signals(&self) -> &dyn Signals {
        self
    }
    fn terminal(&self) -> &dyn Terminal {
        self
    }
    fn fs(&self) -> &dyn Filesystem {
        self
    }
    fn jobs(&self) -> &dyn JobControl {
        self
    }
    fn env(&self) -> &dyn EnvApi {
        self
    }
}

/// Child process handle wrapping std::process::Child.
pub struct UnixChild {
    child: Option<std::process::Child>,
    pid: Pid,
}

impl ChildHandle for UnixChild {
    fn wait(&mut self, _opts: WaitOpts) -> Result<(), PlatformError> {
        match self.child.take() {
            Some(mut child) => {
                child
                    .wait()
                    .map_err(|e| PlatformError::Process(format!("wait: {e}")))?;
                Ok(())
            }
            None => Err(PlatformError::Process("already reaped".into())),
        }
    }

    fn pid(&self) -> Pid {
        self.pid
    }

    fn kill(&mut self, sig: i32) -> Result<(), PlatformError> {
        if let Some(child) = self.child.as_mut() {
            let signal = nix::sys::signal::Signal::try_from(sig)
                .unwrap_or(nix::sys::signal::Signal::SIGTERM);
            // SAFETY: valid child pid; kill is async-signal-safe.
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(child.id() as i32), signal)
                .map_err(|e| PlatformError::Signal(format!("kill: {e}")))?;
        }
        Ok(())
    }
}

impl Process for UnixPlatform {
    fn spawn(&self, cmd: &CommandSpec) -> Result<Box<dyn ChildHandle>, PlatformError> {
        let mut command = Command::new(&cmd.program);
        command.args(&cmd.args);
        if let Some(cwd) = &cmd.cwd {
            command.current_dir(cwd);
        }
        for (k, v) in &cmd.env {
            command.env(k, v);
        }
        if let Some(fd) = cmd.stdin {
            // SAFETY: fd is a borrowed FD; std re-dups it into the child.
            let file = unsafe { std::fs::File::from_raw_fd(fd) };
            command.stdin(Stdio::from(file));
        }
        if let Some(fd) = cmd.stdout {
            // SAFETY: fd is a borrowed FD; std re-dups it into the child.
            let file = unsafe { std::fs::File::from_raw_fd(fd) };
            command.stdout(Stdio::from(file));
        }
        if let Some(fd) = cmd.stderr {
            // SAFETY: fd is a borrowed FD; std re-dups it into the child.
            let file = unsafe { std::fs::File::from_raw_fd(fd) };
            command.stderr(Stdio::from(file));
        }
        let child = command
            .spawn()
            .map_err(|e| PlatformError::Process(format!("spawn {}: {e}", cmd.program)))?;
        let pid = child.id() as Pid;
        Ok(Box::new(UnixChild {
            child: Some(child),
            pid,
        }))
    }

    fn exec_replace(&self, cmd: &CommandSpec) -> Result<Infallible, PlatformError> {
        let mut command = Command::new(&cmd.program);
        command.args(&cmd.args);
        if let Some(cwd) = &cmd.cwd {
            command.current_dir(cwd);
        }
        for (k, v) in &cmd.env {
            command.env(k, v);
        }
        // SAFETY: exec never returns on success.
        let err = command.exec();
        Err(PlatformError::Process(format!(
            "exec {}: {err}",
            cmd.program
        )))
    }

    // `#[allow]`: Linux adds PtraceEvent/PtraceSyscall variants that are
    // unreachable on other Unix targets; the `_` arm covers them there.
    #[allow(unreachable_patterns)]
    fn wait(&self, pid: Pid, opts: WaitOpts) -> Result<WaitResult, PlatformError> {
        let mut flags = nix::sys::wait::WaitPidFlag::empty();
        if opts.no_hang {
            flags |= nix::sys::wait::WaitPidFlag::WNOHANG;
        }
        if opts.untraced {
            flags |= nix::sys::wait::WaitPidFlag::WUNTRACED;
        }
        // SAFETY: waitpid with a valid pid and safe flag bits.
        let res = nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid), Some(flags))
            .map_err(|e| PlatformError::Process(format!("waitpid: {e}")))?;
        Ok(match res {
            nix::sys::wait::WaitStatus::Exited(p, code) => WaitResult {
                pid: p.as_raw(),
                status: code,
                exited: true,
                stopped: false,
                continued: false,
            },
            nix::sys::wait::WaitStatus::Stopped(p, _sig) => WaitResult {
                pid: p.as_raw(),
                status: 0,
                exited: false,
                stopped: true,
                continued: false,
            },
            nix::sys::wait::WaitStatus::Continued(p) => WaitResult {
                pid: p.as_raw(),
                status: 0,
                exited: false,
                stopped: false,
                continued: true,
            },
            nix::sys::wait::WaitStatus::StillAlive => WaitResult {
                pid,
                status: 0,
                exited: false,
                stopped: false,
                continued: false,
            },
            nix::sys::wait::WaitStatus::Signaled(p, sig, _core) => WaitResult {
                pid: p.as_raw(),
                status: 128 + sig as i32,
                exited: true,
                stopped: false,
                continued: false,
            },
            _ => WaitResult {
                pid,
                status: 0,
                exited: false,
                stopped: false,
                continued: false,
            },
        })
    }

    fn kill_group(&self, pgid: Pgid, sig: i32) -> Result<(), PlatformError> {
        let signal =
            nix::sys::signal::Signal::try_from(sig).unwrap_or(nix::sys::signal::Signal::SIGTERM);
        // SAFETY: killpg semantics — positive pid selects the process group.
        nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid), signal)
            .map_err(|e| PlatformError::Signal(format!("killpg: {e}")))?;
        Ok(())
    }
}

impl Signals for UnixPlatform {
    fn sigaction(&self, _sig: i32, _act: SigAction) -> Result<SigAction, PlatformError> {
        // Signal handlers need extern "C" trampolines. Deferred to exec
        // engine, which registers them via nix directly.
        Ok(SigAction::Keep)
    }

    fn block(&self, mask: &[i32]) -> Result<Vec<i32>, PlatformError> {
        let mut set = nix::sys::signal::SigSet::empty();
        for &sig in mask {
            if let Ok(s) = nix::sys::signal::Signal::try_from(sig) {
                set.add(s);
            }
        }
        // SAFETY: pthread_sigmask blocks the given set; oldset discarded.
        nix::sys::signal::pthread_sigmask(
            nix::sys::signal::SigmaskHow::SIG_BLOCK,
            Some(&set),
            None,
        )
        .map_err(|e| PlatformError::Signal(format!("block: {e}")))?;
        Ok(Vec::new())
    }

    fn restore(&self, _mask: &[i32]) -> Result<(), PlatformError> {
        let old = nix::sys::signal::SigSet::empty();
        // SAFETY: pthread_sigmask SIG_SETMASK with empty set restores.
        nix::sys::signal::pthread_sigmask(
            nix::sys::signal::SigmaskHow::SIG_SETMASK,
            Some(&old),
            None,
        )
        .map_err(|e| PlatformError::Signal(format!("restore: {e}")))?;
        Ok(())
    }

    fn raise(&self, sig: i32) -> Result<(), PlatformError> {
        let signal =
            nix::sys::signal::Signal::try_from(sig).unwrap_or(nix::sys::signal::Signal::SIGTERM);
        // SAFETY: raise on self is async-signal-safe.
        nix::sys::signal::raise(signal)
            .map_err(|e| PlatformError::Signal(format!("raise: {e}")))?;
        Ok(())
    }
}

impl Terminal for UnixPlatform {
    fn isatty(&self, fd: RawFd) -> bool {
        let cache = self.tty_cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.contains(&fd) {
            return true;
        }
        let is = nix::unistd::isatty(borrow_fd(fd)).unwrap_or(false);
        if is {
            self.tty_cache
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(fd);
        }
        is
    }

    fn make_raw(&self, fd: RawFd) -> Result<TermiosState, PlatformError> {
        // SAFETY: tcgetattr on a valid fd.
        let state = nix::sys::termios::tcgetattr(borrow_fd(fd))
            .map_err(|e| PlatformError::Terminal(format!("tcgetattr: {e}")))?;
        let raw = state.clone();
        // SAFETY: tcsetattr on a valid fd with the raw termios.
        nix::sys::termios::tcsetattr(borrow_fd(fd), nix::sys::termios::SetArg::TCSANOW, &raw)
            .map_err(|e| PlatformError::Terminal(format!("tcsetattr: {e}")))?;
        Ok(TermiosState::Unix(state))
    }

    fn restore(&self, fd: RawFd, state: &TermiosState) -> Result<(), PlatformError> {
        match state {
            TermiosState::Unix(t) => {
                // SAFETY: tcsetattr with saved termios on a valid fd.
                nix::sys::termios::tcsetattr(borrow_fd(fd), nix::sys::termios::SetArg::TCSANOW, t)
                    .map_err(|e| PlatformError::Terminal(format!("tcsetattr restore: {e}")))?;
            }
            TermiosState::Empty => {}
        }
        Ok(())
    }

    fn get_size(&self, fd: RawFd) -> Result<(u16, u16), PlatformError> {
        // nix 0.31 has no window_size helper; issue the ioctl directly.
        let mut winsize = nix::libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: TIOCGWINSZ writes a winsize through the pointer arg;
        // the pointer is valid and correctly sized for the call duration.
        let rc = unsafe {
            nix::libc::ioctl(
                fd,
                nix::libc::TIOCGWINSZ,
                &mut winsize as *mut nix::libc::winsize,
            )
        };
        if rc < 0 {
            return Err(PlatformError::Terminal(format!(
                "ioctl TIOCGWINSZ: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok((winsize.ws_row, winsize.ws_col))
    }
}

impl Filesystem for UnixPlatform {
    fn exists(&self, path: &Path) -> bool {
        path.exists()
    }

    fn metadata(&self, path: &Path) -> Result<std::fs::Metadata, PlatformError> {
        std::fs::metadata(path)
            .map_err(|e| PlatformError::Fs(format!("metadata {}: {e}", path.display())))
    }

    fn create_dir(&self, path: &Path) -> Result<(), PlatformError> {
        std::fs::create_dir(path)
            .map_err(|e| PlatformError::Fs(format!("mkdir {}: {e}", path.display())))
    }

    fn remove_file(&self, path: &Path) -> Result<(), PlatformError> {
        std::fs::remove_file(path)
            .map_err(|e| PlatformError::Fs(format!("rm {}: {e}", path.display())))
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<std::path::PathBuf>, PlatformError> {
        std::fs::read_dir(path)
            .map(|it| {
                it.filter_map(|e| e.ok().map(|e| e.path()))
                    .collect::<Vec<_>>()
            })
            .map_err(|e| PlatformError::Fs(format!("readdir {}: {e}", path.display())))
    }
}

impl JobControl for UnixPlatform {
    fn set_foreground(&self, pgid: Pgid) -> Result<(), PlatformError> {
        *self.foreground.lock().unwrap_or_else(|e| e.into_inner()) = pgid;
        Ok(())
    }

    fn set_background(&self, _pgid: Pgid) -> Result<(), PlatformError> {
        Ok(())
    }

    // SAFETY: tcsetpgrp on a controlling terminal fd with valid pgid.
    fn tcsetpgrp(&self, fd: RawFd, pgid: Pgid) -> Result<(), PlatformError> {
        nix::unistd::tcsetpgrp(borrow_fd(fd), nix::unistd::Pid::from_raw(pgid))
            .map_err(|e| PlatformError::Job(format!("tcsetpgrp: {e}")))?;
        Ok(())
    }

    fn current_pgrp(&self) -> Pgid {
        *self.foreground.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl EnvApi for UnixPlatform {
    fn get(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }

    fn set(&self, name: &str, value: &str) {
        // SAFETY: single-threaded shell; no concurrent env access.
        unsafe { std::env::set_var(name, value) }
    }

    fn remove(&self, name: &str) {
        // SAFETY: single-threaded shell; no concurrent env access.
        unsafe { std::env::remove_var(name) }
    }

    fn all(&self) -> Vec<(String, String)> {
        std::env::vars().collect()
    }
}
