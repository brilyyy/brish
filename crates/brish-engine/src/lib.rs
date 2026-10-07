//! Execution engine: AST evaluation, pipelines, redirections, job
//! control and plugin hook dispatch.
//!
//! Split out of `brish-builtin` (NOTES.md 1) so plugins needing engine
//! types can live in `brish-plugin` above this crate without a cycle.
//! `Engine` walks a `brish-plugin-api` `Registry` read-only; the
//! registry is built by the binary and is immutable after startup.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]

pub use exec::{Engine, Outcome, Reload, Stop};

mod exec;

/// Shared test locks (process-wide state like cwd must not race).
///
/// Duplicated from `brish-builtin` on purpose: cargo runs each crate's
/// unit tests in a separate process, and these serialize *threads*
/// within one test binary. They are not a cross-process lock.
#[cfg(test)]
pub(crate) mod test_util {
    use std::sync::Mutex;
    pub(crate) static CWD_LOCK: Mutex<()> = Mutex::new(());
    /// Serializes tests that install/reset process-global signal
    /// handlers (`trap` tests): one test's `trap - SIG` restoring
    /// SIG_DFL would turn another's real signal into a suite-killer.
    pub(crate) static TRAP_LOCK: Mutex<()> = Mutex::new(());
}
