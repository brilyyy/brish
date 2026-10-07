//! brish-words: the pure word-processing layer of briSH.
//!
//! POSIX arithmetic evaluation, `test`/`[` predicate evaluation, IFS field
//! splitting and pathname globbing. No third-party dependencies: the only
//! crate dependency is the in-tree `brish-platform`, and only for the
//! `access(2)`/`isatty` calls that `test -r/-w/-x` and `-t` cannot answer
//! by inspecting mode bits. `unsafe` stays confined to that crate.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used)]

pub mod arith;
pub mod glob;
pub mod pattern;
pub mod split;
pub mod test;

pub use arith::{ArithEnv, ArithError, eval as eval_arith};
pub use glob::{GlobError, expand};
pub use pattern::{pattern_matches, trim};
pub use split::fields;
pub use test::{TestError, eval as eval_test};
