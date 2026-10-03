//! Mock platform for deterministic engine tests (plan §8.2).
//!
//! Records commands instead of touching the OS. Uses `Arc<Mutex<T>>`
//! for all shared state to satisfy `Send + Sync`.

use crate::{
    ChildHandle, CommandSpec, EnvApi, Filesystem, JobControl, Pgid, Pid, Platform, PlatformError,
    Process, RawFd, SigAction, Signals, Terminal, TermiosState, WaitOpts, WaitResult,
};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A recorded process event.
#[derive(Debug, Clone)]
pub enum MockEvent {
    Spawn(String, Vec<String>),
    Exec(String, Vec<String>),
    KillGroup(Pgid, i32),
    Wait(Pid, WaitOpts),
    Sigaction(i32, SigAction),
    Tcsetpgrp(RawFd, Pgid),
}

/// Shared mock state behind `Arc` so the platform is `Clone + Send + Sync`.
#[derive(Debug, Default)]
pub struct MockState {
    pub events: Mutex<Vec<MockEvent>>,
    pub env: Mutex<Vec<(String, String)>>,
    pub files: Mutex<Vec<PathBuf>>,
    pub dirs: Mutex<Vec<PathBuf>>,
    pub next_pid: Mutex<Pid>,
    pub tty_fds: Mutex<Vec<RawFd>>,
    pub foreground: Mutex<Pgid>,
}

impl MockState {
    /// Create empty mock state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an event.
    pub fn record(&self, ev: MockEvent) {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(ev);
    }
}

/// Mock platform implementing all traits over `MockState`.
#[derive(Clone)]
pub struct MockPlatform {
    pub state: Arc<MockState>,
}

impl MockPlatform {
    /// Create a new mock platform.
    pub fn new() -> Self {
        Self {
            state: Arc::new(MockState::new()),
        }
    }
}

impl Default for MockPlatform {
    fn default() -> Self {
        Self::new()
    }
}

impl Platform for MockPlatform {
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

/// Mock child handle: returns the recorded pid.
pub struct MockChild {
    pid: Pid,
}

impl ChildHandle for MockChild {
    fn wait(&mut self, _opts: WaitOpts) -> Result<(), PlatformError> {
        Ok(())
    }

    fn pid(&self) -> Pid {
        self.pid
    }

    fn kill(&mut self, _sig: i32) -> Result<(), PlatformError> {
        Ok(())
    }
}

impl Process for MockPlatform {
    fn spawn(&self, cmd: &CommandSpec) -> Result<Box<dyn ChildHandle>, PlatformError> {
        self.state
            .record(MockEvent::Spawn(cmd.program.clone(), cmd.args.clone()));
        let pid = {
            let mut guard = self
                .state
                .next_pid
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            *guard += 1;
            *guard
        };
        Ok(Box::new(MockChild { pid }))
    }

    fn exec_replace(&self, cmd: &CommandSpec) -> Result<Infallible, PlatformError> {
        self.state
            .record(MockEvent::Exec(cmd.program.clone(), cmd.args.clone()));
        Err(PlatformError::Process("mock exec".into()))
    }

    fn wait(&self, pid: Pid, opts: WaitOpts) -> Result<WaitResult, PlatformError> {
        self.state.record(MockEvent::Wait(pid, opts));
        Ok(WaitResult {
            pid,
            status: 0,
            exited: true,
            stopped: false,
            continued: false,
        })
    }

    fn kill_group(&self, pgid: Pgid, sig: i32) -> Result<(), PlatformError> {
        self.state.record(MockEvent::KillGroup(pgid, sig));
        Ok(())
    }
}

impl Signals for MockPlatform {
    fn sigaction(&self, sig: i32, act: SigAction) -> Result<SigAction, PlatformError> {
        self.state.record(MockEvent::Sigaction(sig, act));
        Ok(SigAction::Keep)
    }

    fn block(&self, _mask: &[i32]) -> Result<Vec<i32>, PlatformError> {
        Ok(Vec::new())
    }

    fn restore(&self, _mask: &[i32]) -> Result<(), PlatformError> {
        Ok(())
    }

    fn raise(&self, _sig: i32) -> Result<(), PlatformError> {
        Ok(())
    }
}

impl Terminal for MockPlatform {
    fn isatty(&self, fd: RawFd) -> bool {
        self.state
            .tty_fds
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&fd)
    }

    fn make_raw(&self, _fd: RawFd) -> Result<TermiosState, PlatformError> {
        Ok(TermiosState::Empty)
    }

    fn restore(&self, _fd: RawFd, _state: &TermiosState) -> Result<(), PlatformError> {
        Ok(())
    }

    fn get_size(&self, _fd: RawFd) -> Result<(u16, u16), PlatformError> {
        Ok((24, 80))
    }
}

impl Filesystem for MockPlatform {
    fn exists(&self, path: &Path) -> bool {
        let p = path.to_path_buf();
        let files = self.state.files.lock().unwrap_or_else(|e| e.into_inner());
        let dirs = self.state.dirs.lock().unwrap_or_else(|e| e.into_inner());
        files.contains(&p) || dirs.contains(&p)
    }

    fn metadata(&self, _path: &Path) -> Result<std::fs::Metadata, PlatformError> {
        Err(PlatformError::Fs("mock metadata unavailable".into()))
    }

    fn create_dir(&self, path: &Path) -> Result<(), PlatformError> {
        self.state
            .dirs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(path.to_path_buf());
        Ok(())
    }

    fn remove_file(&self, path: &Path) -> Result<(), PlatformError> {
        let mut files = self.state.files.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(i) = files.iter().position(|p| p == path) {
            files.remove(i);
            Ok(())
        } else {
            Err(PlatformError::Fs(format!(
                "no such file: {}",
                path.display()
            )))
        }
    }

    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>, PlatformError> {
        let files = self.state.files.lock().unwrap_or_else(|e| e.into_inner());
        Ok(files
            .iter()
            .filter(|p| p.parent() == Some(path))
            .cloned()
            .collect())
    }
}

impl JobControl for MockPlatform {
    fn set_foreground(&self, pgid: Pgid) -> Result<(), PlatformError> {
        *self
            .state
            .foreground
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = pgid;
        Ok(())
    }

    fn set_background(&self, _pgid: Pgid) -> Result<(), PlatformError> {
        Ok(())
    }

    fn tcsetpgrp(&self, fd: RawFd, pgid: Pgid) -> Result<(), PlatformError> {
        self.state.record(MockEvent::Tcsetpgrp(fd, pgid));
        Ok(())
    }

    fn current_pgrp(&self) -> Pgid {
        *self
            .state
            .foreground
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
}

impl EnvApi for MockPlatform {
    fn get(&self, name: &str) -> Option<String> {
        self.state
            .env
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    }

    fn set(&self, name: &str, value: &str) {
        let mut env = self.state.env.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(slot) = env.iter_mut().find(|(k, _)| k == name) {
            slot.1 = value.to_string();
        } else {
            env.push((name.to_string(), value.to_string()));
        }
    }

    fn remove(&self, name: &str) {
        self.state
            .env
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(k, _)| k != name);
    }

    fn all(&self) -> Vec<(String, String)> {
        self.state
            .env
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}
