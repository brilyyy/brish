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

pub use error::PlatformError;
#[cfg(unix)]
pub use guard::{FdGuard, RawModeGuard};
pub use mock::{MockEvent, MockPlatform, MockState};
pub use proc::{
    FdOp, FdScope, FdSetup, fork_run, fork_spawn, preexec_fd_ops, reset_sigpipe, send_signal,
    try_wait, wait_pid, with_fds,
};
pub use traits::{
    ChildHandle, CommandSpec, EnvApi, Filesystem, JobControl, Pgid, Pid, Platform, Process, RawFd,
    SigAction, Signals, Terminal, TermiosState, WaitOpts, WaitResult,
};
#[cfg(unix)]
pub use unix::UnixPlatform;
