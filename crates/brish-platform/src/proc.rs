//! Process primitives for the exec engine: fork, fd juggling, pre-exec ops.
//!
//! SAFETY: every `unsafe` block here is a thin wrapper around one POSIX
//! call. The shell is single-threaded, so `fork` inherits a consistent
//! world; children run a closure and leave through `_exit` (no atexit,
//! no duplicated stdio flush). Callers of this module never write
//! `unsafe` themselves.

use crate::RawFd;
use crate::error::PlatformError;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

/// EBADF, without dragging nix into portable code paths.
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

fn sys_dup2(from: RawFd, to: RawFd) -> std::io::Result<()> {
    // SAFETY: both are plain ints; the kernel validates liveness.
    let rc = unsafe { nix::libc::dup2(from, to) };
    if rc < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

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
pub fn send_signal(pid: i32, sig: i32) -> Result<(), PlatformError> {
    // SAFETY: kill(2) is async-signal-safe; args validated by caller.
    let r = unsafe { nix::libc::kill(pid, sig) };
    if r == 0 {
        Ok(())
    } else {
        Err(io_err("kill", std::io::Error::last_os_error()))
    }
}

/// POSIX: children must see SIGPIPE at disposition default. Rust's std
/// flips it to SIG_IGN process-wide at startup, and ignored dispositions
/// survive exec — restore it once, before any spawn (plan 4.9).
pub fn reset_sigpipe() {
    // SAFETY: one-time process-global call before threads exist.
    unsafe { nix::libc::signal(nix::libc::SIGPIPE, nix::libc::SIG_DFL) };
}

/// Write to fd 1 by raw syscall — bypasses `std::io::stdout` (libtest
/// capture, buffering) so FdScope redirects and command-substitution
/// pipes always receive the bytes. Interrupted writes retry.
pub fn write_stdout(bytes: &[u8]) {
    let mut off = 0usize;
    while off < bytes.len() {
        // SAFETY: write(2) to fd 1; pointer is valid for len-off bytes.
        let n = unsafe {
            nix::libc::write(
                1,
                bytes[off..].as_ptr().cast::<nix::libc::c_void>(),
                bytes.len() - off,
            )
        };
        if n < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return; // EPIPE etc: shell builtins die quietly like echo
        }
        off += n as usize;
    }
}

// ---- trap signal flags ----

use std::sync::atomic::{AtomicU32, Ordering};

/// Bit per trappable signal (`trap` builtin). EXIT is virtual — no
/// bit, fired from the engine at process exit.
pub const TRAP_BIT_INT: u32 = 1 << 0;
pub const TRAP_BIT_TERM: u32 = 1 << 1;
pub const TRAP_BIT_HUP: u32 = 1 << 2;
pub const TRAP_BIT_QUIT: u32 = 1 << 3;

static PENDING_TRAPS: AtomicU32 = AtomicU32::new(0);

/// Swap out pending signal-trap flags (engine drains at command
/// boundaries / before each prompt). Async-signal-safe.
pub fn take_pending_traps() -> u32 {
    PENDING_TRAPS.swap(0, Ordering::Relaxed)
}

/// Observe pending trap flags without consuming them (wait loops use
/// this on EINTR to decide "drain + retry" vs "retry silently").
pub fn peek_pending_traps() -> u32 {
    PENDING_TRAPS.load(Ordering::Relaxed)
}

/// Clear only `mask` bits (leave other engines' flags pending).
pub fn clear_pending_traps(mask: u32) {
    PENDING_TRAPS.fetch_and(!mask, Ordering::Relaxed);
}

/// Test hook: mark a trap bit pending without a real signal delivery
/// (real kills race with parallel tests restoring `SIG_DFL`).
pub fn inject_pending_trap(bit: u32) {
    PENDING_TRAPS.fetch_or(bit, Ordering::Relaxed);
}

fn trap_bit(sig: i32) -> u32 {
    {
        match sig {
            nix::libc::SIGINT => TRAP_BIT_INT,
            nix::libc::SIGTERM => TRAP_BIT_TERM,
            nix::libc::SIGHUP => TRAP_BIT_HUP,
            nix::libc::SIGQUIT => TRAP_BIT_QUIT,
            _ => 0,
        }
    }
}

extern "C" fn trap_handler(sig: i32) {
    let bit = trap_bit(sig);
    if bit != 0 {
        // Only async-signal-safe work: set a flag.
        PENDING_TRAPS.fetch_or(bit, Ordering::Relaxed);
    }
}

/// Install the flag-setting handler for `sig` (`trap 'cmd' SIG`).
pub fn trap_on(sig: i32) -> Result<(), PlatformError> {
    use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, sigaction};
    if trap_bit(sig) == 0 {
        return Err(PlatformError::Process(format!(
            "signal {sig} not trappable"
        )));
    }
    let action = SigAction::new(
        SigHandler::Handler(trap_handler),
        SaFlags::empty(),
        SigSet::empty(),
    );
    // SAFETY: sigaction with our handler; old action discarded (the
    // shell owns these signals once a trap is set).
    unsafe {
        sigaction(
            nix::sys::signal::Signal::try_from(sig)
                .map_err(|_| PlatformError::Process("bad signal".into()))?,
            &action,
        )
    }
    .map_err(|e| PlatformError::Process(e.to_string()))?;
    Ok(())
}

/// Restore default disposition (`trap - SIG`).
pub fn trap_off(sig: i32) -> Result<(), PlatformError> {
    use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, sigaction};
    let action = SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty());
    // SAFETY: restore SIG_DFL for a signal we installed earlier.
    unsafe {
        sigaction(
            nix::sys::signal::Signal::try_from(sig)
                .map_err(|_| PlatformError::Process("bad signal".into()))?,
            &action,
        )
    }
    .map_err(|e| PlatformError::Process(e.to_string()))?;
    Ok(())
}

/// `umask`: `None` = query (get-and-restore), `Some(m)` = set.
/// Returns the previous mask either way.
pub fn umask(mode: Option<u16>) -> u16 {
    use nix::sys::stat::Mode;
    // ponytail: `as _` because mode_t is u32 on Linux, u16 on macOS.
    let prev = nix::sys::stat::umask(Mode::from_bits_truncate(
        u32::from(mode.unwrap_or(0o777)) as _
    ));
    if mode.is_none() {
        // Query only: put the old mask back (single-threaded at call
        // site — engine builtins run before/after spawns, not during).
        nix::sys::stat::umask(prev);
    }
    prev.bits() as u16
}

/// `times(3)`: seconds `[shell_user, shell_sys, child_user, child_sys]`.
pub fn times_secs() -> [f64; 4] {
    // SAFETY: tms is a plain struct; times() only writes into it.
    let mut t: nix::libc::tms = unsafe { std::mem::zeroed() };
    // SAFETY: POSIX times(&mut tms).
    if unsafe { nix::libc::times(&mut t) } == (-1i64) as nix::libc::clock_t {
        return [0.0; 4];
    }
    let tick = nix::unistd::sysconf(nix::unistd::SysconfVar::CLK_TCK)
        .ok()
        .flatten()
        .unwrap_or(100) as f64;
    [
        t.tms_utime as f64 / tick,
        t.tms_stime as f64 / tick,
        t.tms_cutime as f64 / tick,
        t.tms_cstime as f64 / tick,
    ]
}

/// Fresh `dup` copy of `fd` (for `>&N` stdio wiring in the engine).
pub fn dup_fd(fd: RawFd) -> std::io::Result<std::fs::File> {
    match sys_dup(fd)? {
        Some(f) => Ok(std::fs::File::from(f)),
        None => Err(std::io::Error::from_raw_os_error(EBADF)),
    }
}

/// RAII fd redirection: originals saved on apply, restored on drop
/// (including during unwind — a panicking builtin still gets its shell
/// stdio back).
pub struct FdScope {
    saved: Vec<(RawFd, Option<OwnedFd>)>,
}

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

/// Apply setups with no save/restore bookkeeping — for forked children
/// that are about to `_exit`, where restoring "closed" fds would defeat
/// the close (and the save-dups would scribble over low fd numbers).
pub fn apply_bare(setups: Vec<(RawFd, FdSetup)>) -> Result<(), PlatformError> {
    for (target, setup) in setups {
        if let Err(e) = apply_one(target, setup) {
            return Err(io_err("redirect", e));
        }
    }
    Ok(())
}

fn restore(saved: &mut Vec<(RawFd, Option<OwnedFd>)>) {
    while let Some((target, old)) = saved.pop() {
        let _ = match old {
            Some(fd) => sys_dup2(fd.as_raw_fd(), target),
            None => sys_close(target),
        };
    }
}

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
            // POSIX: subshells reset caught traps to their default
            // actions (only SIG_IGN survives). Without this a forked
            // background subshell keeps the parent's TERM/INT handlers
            // and *survives* `kill %n`, racing whatever it spawns next.
            reset_trappable_dispositions();
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
pub fn wait_pid(pid: i32) -> Result<i32, PlatformError> {
    loop {
        match nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid), None) {
            Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => return Ok(code),
            Ok(nix::sys::wait::WaitStatus::Signaled(_, sig, _)) => return Ok(128 + sig as i32),
            Ok(_) => continue,
            Err(nix::errno::Errno::EINTR) => {
                if peek_pending_traps() != 0 {
                    // Trap handlers fire (no SA_RESTART): let the engine
                    // drain mid-wait instead of silently retrying.
                    return Err(PlatformError::Interrupted);
                }
                continue;
            }
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
pub fn fork_run(
    setups: Vec<(RawFd, FdSetup)>,
    f: impl FnOnce() -> i32,
) -> Result<i32, PlatformError> {
    let pid = fork_spawn(setups, false, f)?;
    wait_pid(pid)
}

/// Restore SIG_DFL for the trappable shell signals — called in fork
/// children (POSIX subshell trap reset).
fn reset_trappable_dispositions() {
    use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, sigaction};
    for sig in [
        nix::libc::SIGINT,
        nix::libc::SIGTERM,
        nix::libc::SIGHUP,
        nix::libc::SIGQUIT,
    ] {
        let Ok(s) = nix::sys::signal::Signal::try_from(sig) else {
            continue;
        };
        let action = SigAction::new(SigHandler::SigDfl, SaFlags::empty(), SigSet::empty());
        // SAFETY: child right after fork, single-threaded; restoring
        // default dispositions is async-signal-safe.
        let _ = unsafe { sigaction(s, &action) };
    }
}

/// Job-control signal numbers, platform-correct (macOS: TSTP=18,
/// CONT=19; Linux: TSTP=20, CONT=18 — never hardcode these).
pub const SIGCONT: i32 = nix::sys::signal::Signal::SIGCONT as i32;
pub const SIGTSTP: i32 = nix::sys::signal::Signal::SIGTSTP as i32;
pub const SIGSTOP: i32 = nix::sys::signal::Signal::SIGSTOP as i32;

/// Signal number by common POSIX name (platform-correct), for
/// `kill -SIGNAME`. Unknown name → `None`.
pub fn signal_by_name(name: &str) -> Option<i32> {
    use nix::sys::signal::Signal;
    let s = match name {
        "HUP" => Signal::SIGHUP,
        "INT" => Signal::SIGINT,
        "QUIT" => Signal::SIGQUIT,
        "KILL" => Signal::SIGKILL,
        "USR1" => Signal::SIGUSR1,
        "USR2" => Signal::SIGUSR2,
        "TERM" => Signal::SIGTERM,
        "CONT" => Signal::SIGCONT,
        "TSTP" => Signal::SIGTSTP,
        "STOP" => Signal::SIGSTOP,
        "TTIN" => Signal::SIGTTIN,
        "TTOU" => Signal::SIGTTOU,
        "CHLD" => Signal::SIGCHLD,
        "ALRM" => Signal::SIGALRM,
        "PIPE" => Signal::SIGPIPE,
        _ => return None,
    };
    Some(s as i32)
}

/// Child state seen by job-control waits: exited (status or
/// 128+signal) or stopped by a job-control signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildState {
    Exited(i32),
    Stopped,
}

/// Non-blocking wait that also reports stops (`WNOHANG | WUNTRACED`);
/// `None` = still running (or stopped-but-already-reported).
pub fn poll_pid(pid: i32) -> Result<Option<ChildState>, PlatformError> {
    use nix::sys::wait::WaitPidFlag;
    let flags = WaitPidFlag::WNOHANG | WaitPidFlag::WUNTRACED;
    // SAFETY: waitpid with a valid pid and safe flag bits.
    match nix::sys::wait::waitpid(nix::unistd::Pid::from_raw(pid), Some(flags)) {
        Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => Ok(Some(ChildState::Exited(code))),
        Ok(nix::sys::wait::WaitStatus::Signaled(_, sig, _)) => {
            Ok(Some(ChildState::Exited(128 + sig as i32)))
        }
        Ok(nix::sys::wait::WaitStatus::Stopped(_, _)) => Ok(Some(ChildState::Stopped)),
        Ok(_) => Ok(None),
        Err(nix::errno::Errno::ECHILD) => Ok(Some(ChildState::Exited(0))),
        Err(nix::errno::Errno::EINTR) => Ok(None),
        Err(e) => Err(io_err(
            "waitpid",
            std::io::Error::from_raw_os_error(e as i32),
        )),
    }
}

/// Blocking wait that reports stops (`WUNTRACED`): returns when `pid`
/// exits or is stopped. Never returns `Running`.
pub fn wait_untraced(pid: i32) -> Result<ChildState, PlatformError> {
    loop {
        // SAFETY: waitpid with a valid pid and safe flag bits.
        match nix::sys::wait::waitpid(
            nix::unistd::Pid::from_raw(pid),
            Some(nix::sys::wait::WaitPidFlag::WUNTRACED),
        ) {
            Ok(nix::sys::wait::WaitStatus::Exited(_, code)) => return Ok(ChildState::Exited(code)),
            Ok(nix::sys::wait::WaitStatus::Signaled(_, sig, _)) => {
                return Ok(ChildState::Exited(128 + sig as i32));
            }
            Ok(nix::sys::wait::WaitStatus::Stopped(_, _)) => return Ok(ChildState::Stopped),
            Ok(_) => continue, // Continued/StillAlive: keep waiting
            Err(nix::errno::Errno::EINTR) => {
                if peek_pending_traps() != 0 {
                    return Err(PlatformError::Interrupted);
                }
                continue;
            }
            Err(nix::errno::Errno::ECHILD) => return Ok(ChildState::Exited(0)),
            Err(e) => {
                return Err(io_err(
                    "waitpid",
                    std::io::Error::from_raw_os_error(e as i32),
                ));
            }
        }
    }
}

/// Parent half of the group-leader race: `&` jobs become group
/// leaders from the parent too, so a `kill %1` immediately after the
/// `&` never sees a not-yet-created group. (The child also calls
/// setpgid before exec; whichever lands first wins.)
pub fn set_group_leader(pid: i32) {
    // SAFETY: setpgid on our own just-forked child; no-op if the
    // child already led the group.
    let _ = nix::unistd::setpgid(
        nix::unistd::Pid::from_raw(pid),
        nix::unistd::Pid::from_raw(pid),
    );
}

/// Signal a whole process group (bg jobs lead their own group).
pub fn kill_group(pgid: i32, sig: i32) -> Result<(), PlatformError> {
    // SAFETY: killpg semantics — positive pid selects the group; sig
    // validated by the caller (parse_signal) or is a fixed job-control
    // signal.
    let r = unsafe { nix::libc::killpg(pgid, sig) };
    if r == 0 {
        Ok(())
    } else {
        Err(io_err("killpg", std::io::Error::last_os_error()))
    }
}

/// The shell's own process group id.
pub fn shell_pgrp() -> i32 {
    nix::unistd::getpgrp().as_raw()
}

/// The controlling terminal (`/dev/tty`); drop to close.
pub fn open_tty() -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(nix::libc::O_NOCTTY | nix::libc::O_CLOEXEC)
        .open("/dev/tty")
}

/// Give the terminal to `pgid` (job control handoff).
pub fn tcsetpgrp_fd(fd: std::os::fd::RawFd, pgid: i32) -> Result<(), PlatformError> {
    use std::os::fd::BorrowedFd;
    // SAFETY: caller passes an open tty fd for the duration of this
    // call; pgid is a real process group id.
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    nix::unistd::tcsetpgrp(borrowed, nix::unistd::Pid::from_raw(pgid))
        .map_err(|e| PlatformError::Job(format!("tcsetpgrp: {e}")))
}

/// Interactive startup: stop letting line-discipline signals stop the
/// shell itself (children still honor them) — bash does the same.
pub fn ignore_jobctl_signals() {
    use nix::sys::signal::{self, SigHandler, Signal};
    for sig in [Signal::SIGTSTP, Signal::SIGTTIN, Signal::SIGTTOU] {
        // SAFETY: signal(2) with a constant handler; process-wide but
        // only ever called from single-threaded shell startup.
        unsafe {
            let _ = signal::signal(sig, SigHandler::SigIgn);
        };
    }
}

/// Take over the terminal: lead our own process group (when we are
/// not one already), become the foreground group, ignore job-control
/// signals. Call only when stdin is a tty.
pub fn claim_terminal() -> Result<(), PlatformError> {
    let pid = nix::unistd::getpid();
    if nix::unistd::getpgrp() != pid {
        // SAFETY: setpgid(0,0) puts ourselves in a new group we lead;
        // fails only if we are already a session leader (then the
        // group is already ours).
        let _ = nix::unistd::setpgid(nix::unistd::Pid::from_raw(0), nix::unistd::Pid::from_raw(0));
    }
    let tty = open_tty().map_err(|e| PlatformError::Job(format!("open /dev/tty: {e}")))?;
    tcsetpgrp_fd(tty.as_raw_fd(), nix::unistd::getpgrp().as_raw())?;
    ignore_jobctl_signals();
    Ok(())
}

/// Suspend the shell (Ctrl-Z at the prompt): briefly restore the
/// default disposition, raise SIGTSTP on ourselves, re-ignore on
/// resume (SIGCONT). Only for interactive shells that ignored it.
pub fn suspend_self() {
    use nix::sys::signal::{self, SigHandler, Signal};
    // SAFETY: flip our own SIGTSTP disposition, stop, restore —
    // single-threaded shell, no concurrent signal users.
    unsafe {
        let _ = signal::signal(Signal::SIGTSTP, SigHandler::SigDfl);
        let _ = signal::raise(Signal::SIGTSTP);
        let _ = signal::signal(Signal::SIGTSTP, SigHandler::SigIgn);
    }
}

/// Reap `pid` if it already exited (background jobs); `None` = still running.
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

    #[test]
    fn poll_and_wait_untraced_see_stops() {
        let pid = fork_spawn(vec![], false, || {
            // Stop ourselves as soon as we run; CONT resumes to 0.
            let _ = send_signal(std::process::id() as i32, SIGSTOP);
            0
        })
        .unwrap();
        let mut saw_stop = false;
        for _ in 0..400 {
            match poll_pid(pid) {
                Ok(Some(ChildState::Stopped)) => {
                    saw_stop = true;
                    break;
                }
                Ok(Some(ChildState::Exited(c))) => panic!("exited before stop: {c}"),
                _ => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
        assert!(saw_stop, "poll_pid must report WUNTRACED stops");
        send_signal(pid, SIGCONT).unwrap();
        match wait_untraced(pid).unwrap() {
            ChildState::Exited(c) => assert_eq!(c, 0),
            ChildState::Stopped => panic!("stopped again after CONT"),
        }
    }

    #[test]
    fn set_group_leader_prevents_killpg_race() {
        let pid = fork_spawn(vec![], true, || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            0
        })
        .unwrap();
        set_group_leader(pid);
        // Group exists immediately: killpg to it must not be ESRCH.
        kill_group(pid, 0).expect("group must exist right after fork");
        let _ = wait_pid(pid);
    }
}
