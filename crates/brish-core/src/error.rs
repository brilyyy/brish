//! Core shell error type.

use crate::lexer::Span;

/// Errors produced by the briSH engine.
///
/// Every fallible engine operation returns `Result<_, Error>`; no panics
/// on user input (zero-panic policy, plan §5).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Shell grammar / syntax error. `span` locates the offending token
    /// when the lexer/parser knows it (interactive diagnostics render a
    /// caret there; `None` just prints the message).
    #[error("parse error: {msg}")]
    Parse { msg: String, span: Option<Span> },

    /// Input is a prefix of a valid construct; caller should read more
    /// lines (PS2 continuation in the REPL).
    #[error("incomplete input")]
    Incomplete,

    /// Word expansion error (bad substitution, nesting too deep, ...).
    /// `span` is reserved for callers that can locate the word; the
    /// expander does not track it yet.
    #[error("expansion error: {msg}")]
    Expand { msg: String, span: Option<Span> },

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
        Error::Parse {
            msg: msg.into(),
            span: None,
        }
    }

    /// Parse error at a known source span.
    pub fn parse_at(msg: impl Into<String>, span: Span) -> Self {
        Error::Parse {
            msg: msg.into(),
            span: Some(span),
        }
    }

    /// Convenience constructor for expansion errors.
    pub fn expand(msg: impl Into<String>) -> Self {
        Error::Expand {
            msg: msg.into(),
            span: None,
        }
    }

    /// Where the error happened, when the caller tracked it.
    pub fn span(&self) -> Option<Span> {
        match self {
            Error::Parse { span, .. } | Error::Expand { span, .. } => *span,
            _ => None,
        }
    }
}
