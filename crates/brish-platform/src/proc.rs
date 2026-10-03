//! Process primitives for the exec engine: fork, fd juggling, pre-exec ops.
//!
//! SAFETY: every `unsafe` block here is a thin wrapper around one POSIX
//! call. The shell is single-threaded, so `fork` inherits a consistent
//! world; children run a closure and leave through `_exit` (no atexit,
//! no duplicated stdio flush). Callers of this module never write
//! `unsafe` themselves.

use crate::RawFd;
use crate::error::PlatformError;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// EBADF, without dragging nix into portable code paths.
#[cfg(unix)]
const EBADF: i32 = 9;

/// What to install on a `target` fd (in-process scope or forked child).
pub enum FdSetup {
    /// `dup2(from, target)`
    Dup(RawFd),
    /// `dup2(file, target)`; the file is closed here right after the dup.
    File(std::fs::File),
    /// `close(target)`
    Close,
}

/// Extra fd surgery for a spawned external, run after std's stdio setup.
pub enum FdOp {
    Dup { from: RawFd, to: RawFd },
    Close(RawFd),
}

fn io_err(op: &str, e: std::io::Error) -> PlatformError {
    PlatformError::Process(format!("{op}: {e}"))
}

#[cfg(unix)]
fn sys_dup2(from: RawFd, to: RawFd) -> std::io::Result<()> {
    // SAFETY: both are plain ints; the kernel validates liveness.
    let rc = unsafe { nix::libc::dup2(from, to) };
    if rc < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(unix)]
fn sys_close(fd: RawFd) -> std::io::Result<()> {
    // SAFETY: kernel validates; EBADF is tolerated by callers that treat
    // "already closed" as success.
    let rc = unsafe { nix::libc::close(fd) };
    if rc < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Duplicate `fd` (save a copy); `Ok(None)` when `fd` is not open.
#[cfg(unix)]
fn sys_dup(fd: RawFd) -> std::io::Result<Option<OwnedFd>> {
    // SAFETY: kernel validates `fd`; a fresh fd number is returned.
    let rc = unsafe { nix::libc::dup(fd) };
    if rc >= 0 {
        // SAFETY: `rc` is a freshly dup'd descriptor we now own.
        Ok(Some(unsafe { OwnedFd::from_raw_fd(rc) }))
    } else {
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() == Some(nix::libc::EBADF) {
            Ok(None)
        } else {
            Err(e)
        }
    }
}

#[cfg(unix)]
fn apply_one(target: RawFd, setup: FdSetup) -> std::io::Result<()> {
    match setup {
        FdSetup::Dup(from) => sys_dup2(from, target),
        FdSetup::File(f) => sys_dup2(f.as_raw_fd(), target),
        FdSetup::Close => sys_close(target).or_else(|e| {
            if e.raw_os_error() == Some(EBADF) {
                Ok(())
            } else {
                Err(e)
            }
        }),
    }
}

/// Deliver `sig` (POSIX signal number) to `pid` (plan 4.10 `kill`).
#[cfg(unix)]
pub fn send_signal(pid: i32, sig: i32) -> Result<(), PlatformError> {
    // SAFETY: kill(2) is async-signal-safe; args validated by caller.
    let r = unsafe { nix::libc::kill(pid, sig) };
    if r == 0 {
        Ok(())
    } else {
        Err(io_err("kill", std::io::Error::last_os_error()))
    }
}

#[cfg(not(unix))]
pub fn send_signal(_pid: i32, _sig: i32) -> Result<(), PlatformError> {
    Err(PlatformError::Process(
        "signals unsupported on this platform".into(),
    ))
}

/// POSIX: children must see SIGPIPE at disposition default. Rust's std
/// flips it to SIG_IGN process-wide at startup, and ignored dispositions
/// survive exec — restore it once, before any spawn (plan 4.9).
#[cfg(unix)]
pub fn reset_sigpipe() {
    // SAFETY: one-time process-global call before threads exist.
    unsafe { nix::libc::signal(nix::libc::SIGPIPE, nix::libc::SIG_DFL) };
}

#[cfg(not(unix))]
pub fn reset_sigpipe() {}

/// Fresh `dup` copy of `fd` (for `>&N` stdio wiring in the engine).
#[cfg(unix)]
pub fn dup_fd(fd: RawFd) -> std::io::Result<std::fs::File> {
    match sys_dup(fd)? {
        Some(f) => Ok(std::fs::File::from(f)),
        None => Err(std::io::Error::from_raw_os_error(EBADF)),
    }
}

#[cfg(not(unix))]
pub fn dup_fd(_fd: RawFd) -> std::io::Result<std::fs::File> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

/// RAII fd redirection: originals saved on apply, restored on drop
/// (including during unwind — a panicking builtin still gets its shell
/// stdio back).
#[cfg(unix)]
pub struct FdScope {
    saved: Vec<(RawFd, Option<OwnedFd>)>,
}

#[cfg(not(unix))]
pub struct FdScope;

#[cfg(unix)]
impl FdScope {
    pub fn apply(setups: Vec<(RawFd, FdSetup)>) -> Result<Self, PlatformError> {
        let mut saved: Vec<(RawFd, Option<OwnedFd>)> = Vec::new();
        for (target, setup) in setups {
            match sys_dup(target) {
                Ok(old) => saved.push((target, old)),
                Err(e) => {
                    restore(&mut saved);
                    return Err(io_err("dup", e));
                }
            }
            if let Err(e) = apply_one(target, setup) {
                restore(&mut saved);
                return Err(io_err("redirect", e));
            }
        }
        Ok(Self { saved })
    }
}

/// Non-Unix: only the empty plan (no redirections) can run; everything
/// else needs dup2, which the Windows stub doesn't have yet.
#[cfg(not(unix))]
impl FdScope {
    pub fn apply(setups: Vec<(RawFd, FdSetup)>) -> Result<Self, PlatformError> {
        if setups.is_empty() {
            Ok(Self)
        } else {
            Err(PlatformError::Process(
                "fd redirection unsupported on this platform".into(),
            ))
        }
    }
}

/// Apply setups with no save/restore bookkeeping — for forked children
/// that are about to `_exit`, where restoring "closed" fds would defeat
/// the close (and the save-dups would scribble over low fd numbers).
#[cfg(unix)]
pub fn apply_bare(setups: Vec<(RawFd, FdSetup)>) -> Result<(), PlatformError> {
    for (target, setup) in setups {
        if let Err(e) = apply_one(target, setup) {
            return Err(io_err("redirect", e));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn apply_bare(setups: Vec<(RawFd, FdSetup)>) -> Result<(), PlatformError> {
    if setups.is_empty() {
        Ok(())
    } else {
        Err(PlatformError::Process(
            "fd redirection unsupported on this platform".into(),
        ))
    }
}

#[cfg(unix)]
fn restore(saved: &mut Vec<(RawFd, Option<OwnedFd>)>) {
    while let Some((target, old)) = saved.pop() {
        let _ = match old {
            Some(fd) => sys_dup2(fd.as_raw_fd(), target),
            None => sys_close(target),
        };
    }
}

#[cfg(unix)]
impl Drop for FdScope {
    fn drop(&mut self) {
        restore(&mut self.saved);
    }
}

/// Run `f` with fds redirected; restore on return or panic.
pub fn with_fds<T>(
    setups: Vec<(RawFd, FdSetup)>,
    f: impl FnOnce() -> T,
) -> Result<T, PlatformError> {
    let scope = FdScope::apply(setups)?;
    let out = f();
    drop(scope);
    Ok(out)
}

/// Fork a child that applies `setups`, runs `f`, and `_exit`s with its
/// status. Returns the child pid; call [`wait_pid`]. `new_pgroup` puts
/// the child in its own process group (background jobs, plan 4.10).
#[cfg(unix)]
pub fn fork_spawn(
    setups: Vec<(RawFd, FdSetup)>,
    new_pgroup: bool,
    f: impl FnOnce() -> i32,
) -> Result<i32, PlatformError> {
    // Buffered stdout must not be duplicated into the child.
    let _ = std::io::Write::flush(&mut std::io::stdout());
    match unsafe { nix::unistd::fork() } {
        Ok(nix::unistd::ForkResult::Child) => {
            if new_pgroup {
                let _ = nix::unistd::setpgid(
                    nix::unistd::Pid::from_raw(0),
                    nix::unistd::Pid::from_raw(0),
                );
            }
            let code = match apply_bare(setups) {
                Ok(()) => f(),
                Err(_) => 125,
            };
            // Output written by f() (builtins print in the child) lives
            // in this process's buffers; push it out before _exit, which
            // skips Rust's destructors.
            let _ = std::io::Write::flush(&mut std::io::stdout());
            let _ = std::io::Write::flush(&mut std::io::stderr());
            // SAFETY: immediate exit; no atexit side effects, no
            // duplicated buffers (the pre-fork flush above emptied the
            // parent's, and we flushed the child's just above).
            unsafe { nix::libc::_exit(code) }
        }
        Ok(nix::unistd::ForkResult::Parent { child }) => Ok(child.as_raw()),
        Err(e) => Err(io_err("fork", std::io::Error::from_raw_os_error(e as i32))),
    }
}

/// Wait for one child; exit code, or 128+signal for signal deaths.
#[cfg(unix)]
pub fn wait_pid(pid: i32) -> Result<i32, PlatformError> {
    loop {
        match nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid), None) {
            Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => return Ok(code),
            Ok(nix::sys::wait::WaitStatus::Signaled(_, sig, _)) => return Ok(128 + sig as i32),
            Ok(_) => continue,
            Err(nix::errno::Errno::EINTR) => continue,
            Err(e) => {
                return Err(io_err(
                    "waitpid",
                    std::io::Error::from_raw_os_error(e as i32),
                ));
            }
        }
    }
}

/// Fork + wait in one call.
#[cfg(unix)]
pub fn fork_run(
    setups: Vec<(RawFd, FdSetup)>,
    f: impl FnOnce() -> i32,
) -> Result<i32, PlatformError> {
    let pid = fork_spawn(setups, false, f)?;
    wait_pid(pid)
}

/// Reap `pid` if it already exited (background jobs); `None` = still running.
#[cfg(unix)]
pub fn try_wait(pid: i32) -> Result<Option<i32>, PlatformError> {
    use nix::sys::wait::WaitPidFlag;
    match nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid), Some(WaitPidFlag::WNOHANG)) {
        Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => Ok(Some(code)),
        Ok(nix::sys::wait::WaitStatus::Signaled(_, sig, _)) => Ok(Some(128 + sig as i32)),
        Ok(_) => Ok(None),
        Err(nix::errno::Errno::ECHILD) => Ok(Some(0)),
        Err(nix::errno::Errno::EINTR) => Ok(None),
        Err(e) => Err(io_err(
            "waitpid",
            std::io::Error::from_raw_os_error(e as i32),
        )),
    }
}

/// Queue fd surgery for a spawned external: std sets up stdio first, then
/// these run between `fork` and `exec`.
#[cfg(unix)]
pub fn preexec_fd_ops(cmd: &mut std::process::Command, ops: Vec<FdOp>) {
    if ops.is_empty() {
        return;
    }
    use std::os::unix::process::CommandExt;
    // SAFETY: each op is one dup2/close with kernel-validated fds; errors
    // abort exec, which is the safe failure mode.
    unsafe {
        cmd.pre_exec(move || {
            for op in &ops {
                let rc = match op {
                    FdOp::Dup { from, to } => nix::libc::dup2(*from, *to),
                    FdOp::Close(fd) => nix::libc::close(*fd),
                };
                if rc < 0 {
                    let e = std::io::Error::last_os_error();
                    // Closing an fd that is already gone is fine.
                    if matches!(op, FdOp::Close(_)) && e.raw_os_error() == Some(nix::libc::EBADF) {
                        continue;
                    }
                    return Err(e);
                }
            }
            Ok(())
        });
    }
}

// ---- non-unix stubs (Phase 4.13 hardens these) ----

#[cfg(not(unix))]
pub fn fork_spawn(
    _setups: Vec<(RawFd, FdSetup)>,
    _new_pgroup: bool,
    _f: impl FnOnce() -> i32,
) -> Result<i32, PlatformError> {
    Err(PlatformError::Process(
        "fork unsupported on this platform".into(),
    ))
}

#[cfg(not(unix))]
pub fn wait_pid(_pid: i32) -> Result<i32, PlatformError> {
    Err(PlatformError::Process(
        "wait unsupported on this platform".into(),
    ))
}

#[cfg(not(unix))]
pub fn fork_run(
    setups: Vec<(RawFd, FdSetup)>,
    _f: impl FnOnce() -> i32,
) -> Result<i32, PlatformError> {
    let _ = setups;
    Err(PlatformError::Process(
        "fork unsupported on this platform".into(),
    ))
}

#[cfg(not(unix))]
pub fn try_wait(_pid: i32) -> Result<Option<i32>, PlatformError> {
    Ok(None)
}

#[cfg(not(unix))]
pub fn preexec_fd_ops(_cmd: &mut std::process::Command, _ops: Vec<FdOp>) {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    // Every test here touches fd 1; parallel threads would race the
    // dup2/restore dance. Serialize them.
    static FD_LOCK: Mutex<()> = Mutex::new(());

    fn fd_guard() -> MutexGuard<'static, ()> {
        FD_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    use std::io::{Read, Write};

    #[test]
    fn fork_runs_closure_and_reports_status() {
        let _g = fd_guard();
        let code = fork_run(Vec::new(), || 42).expect("fork_run");
        assert_eq!(code, 42);
    }

    #[test]
    fn fork_redirects_stdout_to_pipe() {
        let _g = fd_guard();
        let (r, w) = std::io::pipe().expect("pipe");
        let f = std::fs::File::from(std::os::fd::OwnedFd::from(w));
        let code = fork_run(vec![(1, FdSetup::File(f))], || {
            // Raw write: the test harness captures `println!` above the fd
            // level, which would bypass the redirection entirely.
            let msg = b"hello\n";
            // SAFETY: constant bytes to fd 1 in the forked child.
            unsafe { nix::libc::write(1, msg.as_ptr().cast(), msg.len()) };
            7
        })
        .expect("fork_run");
        assert_eq!(code, 7);
        let mut out = String::new();
        let mut r = r;
        r.read_to_string(&mut out).expect("read");
        assert_eq!(out, "hello\n");
    }

    #[test]
    fn with_fds_restores_stdout() {
        let _g = fd_guard();
        use std::os::unix::fs::MetadataExt;
        fn fd1_inode() -> Option<u64> {
            std::fs::metadata("/dev/fd/1").ok().map(|m| m.ino())
        }
        let before = fd1_inode();
        let tmp = tempfile::NamedTempFile::new().expect("tmp");
        let f = tmp.reopen().expect("reopen");
        let setups = vec![(1, FdSetup::File(f))];
        with_fds(setups, || {
            // See fork test: harness-level capture bypasses fd 1.
            let msg = b"to-file\n";
            // SAFETY: constant bytes to fd 1 while the scope is active.
            unsafe { nix::libc::write(1, msg.as_ptr().cast(), msg.len()) };
        })
        .expect("with_fds");
        let mut s = String::new();
        std::fs::File::open(tmp.path())
            .expect("open")
            .read_to_string(&mut s)
            .expect("read");
        assert_eq!(s, "to-file\n");
        assert_eq!(fd1_inode(), before, "stdout must be restored");
    }

    #[test]
    fn try_wait_reaps_finished_child() {
        let _g = fd_guard();
        let pid = fork_spawn(Vec::new(), false, || 3).expect("spawn");
        let code = wait_pid(pid).expect("wait");
        assert_eq!(code, 3);
        // Already reaped: ECHILD → treated as status 0.
        assert_eq!(try_wait(pid).expect("try_wait"), Some(0));
    }

    #[test]
    fn stderr_survives_a_forked_child() {
        let _g = fd_guard();
        let mut err = tempfile::NamedTempFile::new().expect("tmp");
        // sanity: the platform error formatting is stable
        let e = PlatformError::Process("boom".into());
        assert_eq!(e.to_string(), "process error: boom");
        let _ = err.write_all(b"");
    }
}
