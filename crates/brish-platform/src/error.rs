//! Platform error type.

use std::path::PathBuf;

/// Errors from the platform abstraction layer.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// A process / exec error.
    #[error("process error: {0}")]
    Process(String),

    /// A signal error.
    #[error("signal error: {0}")]
    Signal(String),

    /// A terminal error.
    #[error("terminal error: {0}")]
    Terminal(String),

    /// A filesystem error.
    #[error("fs error: {0}")]
    Fs(String),

    /// A job-control error.
    #[error("job error: {0}")]
    Job(String),

    /// An IO error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Path is not absolute when one was required.
    #[error("path not absolute: {0}")]
    NotAbsolute(PathBuf),

    /// Missing executable.
    #[error("no such executable: {0}")]
    NoExecutable(String),

    /// Unknown / unsupported operation.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// Interrupted syscall; retry.
    #[error("interrupted")]
    Interrupted,
}
