//! Built-in shell commands.
//!
//! Dispatch table first; individual built-in implementations land with
//! the plan's phase 3 (`docs/briSH-technical-plan.md`).

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]

// Re-exports
pub use builtins::BuiltIn;
pub use run::{Flow, run};

mod builtins;
pub mod exec;
mod run;
