//! Built-in shell commands and the plugin store.
//!
//! POSIX builtins (`BuiltIn` + `run`), the `plugin` store
//! (`store`/`store_cmd`) and the shared `~/.config/brish` path family.
//! Execution lives in `brish-engine`, which depends on this crate.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]

// Re-exports
pub use builtins::BuiltIn;
pub use run::{Flow, run};

mod builtins;
pub mod helper;
pub mod paths;
mod run;
pub mod store;
pub mod store_cmd;
pub mod table;
pub mod ucfg;
mod z;

/// Shared test locks (process-wide state like cwd must not race).
#[cfg(test)]
pub(crate) mod test_util {
    use std::sync::Mutex;
    pub(crate) static CWD_LOCK: Mutex<()> = Mutex::new(());
    /// Serializes tests that install/reset process-global signal
    /// handlers (`trap` tests): one test's `trap - SIG` restoring
    /// SIG_DFL would turn another's real signal into a suite-killer.
    pub(crate) static TRAP_LOCK: Mutex<()> = Mutex::new(());
}
