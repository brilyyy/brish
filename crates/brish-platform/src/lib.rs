//! Platform abstraction layer for briSH.
//!
//! Isolates Unix-specific syscalls (processes, signals, terminals,
//! job control) behind traits so the engine stays platform-agnostic.
//! Windows support is secondary (Phase 4.13); DOS exploratory only.
//!
//! SAFETY: this crate is the ONLY crate allowed `unsafe` (confined to
//! thin wrappers around nix calls, each with a `// SAFETY:` comment).

#![allow(unsafe_code)] // confined to this crate; see module docs
#![deny(clippy::unwrap_used, clippy::expect_used)]

pub mod error;
#[cfg(unix)]
pub mod guard;
pub mod mock;
pub mod proc;
pub mod traits;
#[cfg(unix)]
pub mod unix;
pub mod user;

pub use error::PlatformError;
#[cfg(unix)]
pub use guard::{FdGuard, RawModeGuard};
pub use mock::{MockEvent, MockPlatform, MockState};
pub use proc::{
    ChildState, FdOp, FdScope, FdSetup, SIGCONT, SIGSTOP, SIGTSTP, TRAP_BIT_HUP, TRAP_BIT_INT,
    TRAP_BIT_QUIT, TRAP_BIT_TERM, claim_terminal, fork_run, fork_spawn, ignore_jobctl_signals,
    inject_pending_trap, kill_group, open_tty, poll_pid, preexec_fd_ops, reset_sigpipe,
    send_signal, set_group_leader, shell_pgrp, signal_by_name, suspend_self, take_pending_traps,
    tcsetpgrp_fd, times_secs, trap_off, trap_on, try_wait, umask, wait_pid, wait_untraced,
    with_fds, write_stdout,
};
pub use traits::{
    ChildHandle, CommandSpec, EnvApi, Filesystem, JobControl, Pgid, Pid, Platform, Process, RawFd,
    SigAction, Signals, Terminal, TermiosState, WaitOpts, WaitResult,
};
#[cfg(unix)]
pub use unix::UnixPlatform;
