//! Platform traits: pure interfaces for the engine.
//!
//! Types are intentionally small; `brish-platform::unix` implements them.

use std::convert::Infallible;
use std::path::{Path, PathBuf};

/// Signed OS process id.
pub type Pid = i32;

/// Process-group id (always positive; sign handled internally).
pub type Pgid = i32;

/// Raw OS file descriptor number.
pub type RawFd = i32;

/// Wait options requested by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WaitOpts {
    /// Do not block (WNOHANG).
    pub no_hang: bool,
    /// Report stopped children (WUNTRACED).
    pub untraced: bool,
}

/// Result of a wait call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WaitResult {
    pub pid: Pid,
    pub status: i32,
    pub exited: bool,
    pub stopped: bool,
    pub continued: bool,
}

/// A terminal signal disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SigAction {
    /// Keep the current disposition.
    #[default]
    Keep,
    /// Reset to default.
    Default,
    /// Ignore the signal.
    Ignore,
    /// Use the given handler address (Unix only).
    Handler(usize),
}

/// Command specification for spawn/exec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    /// Binary to run (PATH looked up by the platform).
    pub program: String,
    /// Arguments (argv excluding program).
    pub args: Vec<String>,
    /// Working directory.
    pub cwd: Option<PathBuf>,
    /// Environment overrides (replaces caller env).
    pub env: Vec<(String, String)>,
    /// stdin fd (None = inherit / closed).
    pub stdin: Option<RawFd>,
    /// stdout fd.
    pub stdout: Option<RawFd>,
    /// stderr fd.
    pub stderr: Option<RawFd>,
}

impl CommandSpec {
    /// Create a minimal command spec.
    pub fn new(program: String, args: Vec<String>) -> Self {
        CommandSpec {
            program,
            args,
            cwd: None,
            env: Vec::new(),
            stdin: None,
            stdout: None,
            stderr: None,
        }
    }
}

/// Handle to a spawned child process.
pub trait ChildHandle: Send {
    /// Block until the child exits (honors WaitOpts where supported).
    fn wait(&mut self, opts: WaitOpts) -> Result<(), crate::PlatformError>;
    /// PID of the child.
    fn pid(&self) -> Pid;
    /// Send a signal to the child.
    fn kill(&mut self, sig: i32) -> Result<(), crate::PlatformError>;
}

/// Process management.
pub trait Process: Send + Sync {
    /// Spawn a child; returns a handle.
    fn spawn(&self, cmd: &CommandSpec) -> Result<Box<dyn ChildHandle>, crate::PlatformError>;
    /// Replace the current process image (exec).
    ///
    /// Returns `Err` only when exec itself fails; on success the process
    /// image is replaced and this never returns.
    fn exec_replace(&self, cmd: &CommandSpec) -> Result<Infallible, crate::PlatformError>;
    /// Wait for a specific pid.
    fn wait(&self, pid: Pid, opts: WaitOpts) -> Result<WaitResult, crate::PlatformError>;
    /// Send a signal to a process group.
    fn kill_group(&self, pgid: Pgid, sig: i32) -> Result<(), crate::PlatformError>;
}

/// Signal management.
pub trait Signals: Send + Sync {
    /// Set/get signal disposition.
    fn sigaction(&self, sig: i32, act: SigAction) -> Result<SigAction, crate::PlatformError>;
    /// Block signals; returns the previous set.
    fn block(&self, _mask: &[i32]) -> Result<Vec<i32>, crate::PlatformError>;
    /// Restore the previous mask.
    fn restore(&self, _mask: &[i32]) -> Result<(), crate::PlatformError>;
    /// Send a signal to self.
    fn raise(&self, _sig: i32) -> Result<(), crate::PlatformError>;
}

/// Terminal / TTY management.
pub trait Terminal: Send + Sync {
    /// Is fd a tty?
    fn isatty(&self, fd: RawFd) -> bool;
    /// Put fd into raw mode; returns opaque state for restore.
    fn make_raw(&self, fd: RawFd) -> Result<TermiosState, crate::PlatformError>;
    /// Restore terminal state on the given fd.
    fn restore(&self, fd: RawFd, state: &TermiosState) -> Result<(), crate::PlatformError>;
    /// Window size (rows, cols).
    fn get_size(&self, fd: RawFd) -> Result<(u16, u16), crate::PlatformError>;
}

/// Filesystem helpers.
pub trait Filesystem: Send + Sync {
    fn exists(&self, path: &Path) -> bool;
    fn metadata(&self, path: &Path) -> Result<std::fs::Metadata, crate::PlatformError>;
    fn create_dir(&self, path: &Path) -> Result<(), crate::PlatformError>;
    fn remove_file(&self, path: &Path) -> Result<(), crate::PlatformError>;
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, crate::PlatformError>;
}

/// Job / process-group control.
pub trait JobControl: Send + Sync {
    fn set_foreground(&self, pgid: Pgid) -> Result<(), crate::PlatformError>;
    fn set_background(&self, _pgid: Pgid) -> Result<(), crate::PlatformError>;
    fn tcsetpgrp(&self, fd: RawFd, pgid: Pgid) -> Result<(), crate::PlatformError>;
    fn current_pgrp(&self) -> Pgid;
}

/// Process environment access.
pub trait EnvApi: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
    fn set(&self, name: &str, value: &str);
    fn remove(&self, name: &str);
    fn all(&self) -> Vec<(String, String)>;
}

/// Aggregate platform interface: one object exposes all capabilities.
pub trait Platform: Send + Sync {
    fn processes(&self) -> &dyn Process;
    fn signals(&self) -> &dyn Signals;
    fn terminal(&self) -> &dyn Terminal;
    fn fs(&self) -> &dyn Filesystem;
    fn jobs(&self) -> &dyn JobControl;
    fn env(&self) -> &dyn EnvApi;
}

/// Opaque terminal state (restored verbatim).
///
/// On Unix it wraps `nix::sys::termios::Termios`; other
/// platforms (and the mock) use `Empty`, for which restore
/// is a no-op.
pub enum TermiosState {
    /// Unix captured termios.
    Unix(nix::sys::termios::Termios),
    /// No captured state; restore is a no-op.
    Empty,
}

impl std::fmt::Debug for TermiosState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TermiosState").finish_non_exhaustive()
    }
}

impl TermiosState {
    /// Build a no-op state (mock + non-Unix restore fallback).
    pub fn empty() -> Self {
        TermiosState::Empty
    }
}
