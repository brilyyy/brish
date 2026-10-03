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
pub mod helper;
pub mod paths;
mod run;
pub mod store;

/// Shared test locks (process-wide state like cwd must not race).
#[cfg(test)]
pub(crate) mod test_util {
    use std::sync::Mutex;
    pub(crate) static CWD_LOCK: Mutex<()> = Mutex::new(());
}
