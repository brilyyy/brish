//! Core shell error type.

/// Errors produced by the briSH engine.
///
/// Every fallible engine operation returns `Result<_, Error>`; no panics
/// on user input (zero-panic policy, plan §5).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Shell grammar / syntax error.
    #[error("parse error: {0}")]
    Parse(String),

    /// Input is a prefix of a valid construct; caller should read more
    /// lines (PS2 continuation in the REPL).
    #[error("incomplete input")]
    Incomplete,

    /// Word expansion error (bad substitution, nesting too deep, ...).
    #[error("expansion error: {0}")]
    Expand(String),

    /// Execution error (spawn, redirection, job control, ...).
    #[error("execution error: {0}")]
    Exec(String),

    /// Configuration / rc-file error.
    #[error("configuration error: {0}")]
    Config(String),

    /// Underlying I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl Error {
    /// Convenience constructor for parse errors.
    pub fn parse(msg: impl Into<String>) -> Self {
        Error::Parse(msg.into())
    }

    /// Convenience constructor for expansion errors.
    pub fn expand(msg: impl Into<String>) -> Self {
        Error::Expand(msg.into())
    }
}
