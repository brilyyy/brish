//! Execution mode policy: batch (POSIX script) vs interactive (REPL).
//!
//! One grammar and one AST serve both modes; every pass consults
//! `ModePolicy` instead of forking the parser (plan §4.2).

/// How strictly this parse/exec session follows POSIX.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModePolicy {
    /// Reject non-POSIX constructs (batch scripts, `--posix`).
    pub strict_posix: bool,
    /// Interactive-only behavior: prompts, job control, aliases.
    pub interactive: bool,
}

impl ModePolicy {
    /// Batch mode: strict POSIX, no interactive behavior.
    pub fn batch() -> Self {
        Self {
            strict_posix: true,
            interactive: false,
        }
    }

    /// Interactive mode: POSIX base plus shell UX extensions.
    pub fn interactive() -> Self {
        Self {
            strict_posix: false,
            interactive: true,
        }
    }
}

impl Default for ModePolicy {
    fn default() -> Self {
        Self::batch()
    }
}
