//! Interactive input handling: completeness check + default prompts.
//!
//! The REPL appends lines until [`is_complete`] reports the buffer parses
//! (hard errors count as complete — they are reported, not continued).

use crate::error::Error;
use crate::parser;

/// `true` when source is fully parseable or has a hard syntax error
/// (only `Error::Incomplete` means "read more input").
pub fn is_complete(src: &str) -> bool {
    !matches!(parser::parse(src), Err(Error::Incomplete))
}

/// Default primary prompt.
pub fn default_ps1() -> String {
    "$ ".to_string()
}

/// Default continuation prompt.
pub fn default_ps2() -> String {
    "> ".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_commands() {
        assert!(is_complete("echo hi"));
        assert!(is_complete(""));
        assert!(is_complete("if true; then echo; fi"));
        assert!(is_complete("case x in a) ;; esac"));
    }

    #[test]
    fn incomplete_commands() {
        assert!(!is_complete("if true; then"));
        assert!(!is_complete("echo 'unterminated"));
        assert!(!is_complete("echo hi |"));
        assert!(!is_complete("while true; do"));
        assert!(!is_complete("{ echo;"));
    }

    #[test]
    fn hard_errors_are_complete() {
        assert!(is_complete("echo hi )"));
        assert!(is_complete("then"));
    }

    #[test]
    fn prompts() {
        assert_eq!(default_ps1(), "$ ");
        assert_eq!(default_ps2(), "> ");
    }
}
