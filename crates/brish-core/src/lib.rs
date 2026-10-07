//! Core shell functionality: lexer, parser, AST, expander, environment.
//!
//! Zero dependencies by design — everything else in the workspace is
//! allowed to depend on this crate, never the reverse.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]

pub mod ast;
pub mod env;
pub mod error;
pub mod expand;
pub mod lexer;
pub mod mode;
pub mod parser;
pub mod path;
pub mod reader;

/// True when the space-separated `BRISH_DEBUG` tag list contains `tag`.
/// Tags: `lexer`, `parser`, `expand`, `exec`, `jobs` (plan 6.8).
pub fn debug_on(tag: &str) -> bool {
    std::env::var("BRISH_DEBUG").is_ok_and(|v| v.split_whitespace().any(|t| t == tag))
}

// Re-exports for convenience
pub use error::Error;
pub use mode::ModePolicy;
