#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Property tests (plan 6.4): the front half of the shell must never
//! panic on any input — fuzz-shaped strings go through lexer, parser and
//! expander; the expander's contract with `Env` round-trips.

use brish_core::env::Env;
use brish_core::expand;
use brish_core::lexer;
use brish_core::parser;
use proptest::prelude::*;

/// Stubs for the two substitution callbacks: neither forks, so the
/// properties stay hermetic.
fn cmd_subst_stub(_s: &str) -> Result<String, brish_core::error::Error> {
    Ok(String::new())
}

fn proc_subst_stub(s: &str, _out: bool) -> Result<String, brish_core::error::Error> {
    Ok(format!("/dev/fd/{s}"))
}

proptest! {
    #[test]
    fn lex_never_panics(src in ".*") {
        let _ = lexer::lex(&src);
    }

    #[test]
    fn parse_never_panics(src in ".*") {
        let _ = parser::parse(&src);
    }

    #[test]
    fn expand_text_never_panics(src in ".*", value in ".*") {
        let mut env = Env::new();
        env.set("X", value).unwrap();
        let mut cs = cmd_subst_stub;
        let mut ps = proc_subst_stub;
        let _ = expand::expand_text(&mut env, &src, &mut cs, &mut ps);
    }

    #[test]
    fn env_round_trip(
        name in proptest::string::string_regex("[A-Za-z_][A-Za-z_0-9]{0,15}").unwrap(),
        value in ".*",
    ) {
        let mut env = Env::new();
        env.set(&name, value.clone()).unwrap();
        prop_assert_eq!(env.get(&name).map(str::to_string), Some(value));
    }

    /// Quote-removal on plain (unquoted, expansion-free) text is the
    /// identity: gaps between tokens are copied verbatim.
    #[test]
    fn expand_plain_text_is_identity(src in "[a-z0-9]+( [a-z0-9]+)*") {
        let mut env = Env::new();
        let mut cs = cmd_subst_stub;
        let mut ps = proc_subst_stub;
        prop_assert_eq!(
            expand::expand_text(&mut env, &src, &mut cs, &mut ps).unwrap(),
            src
        );
    }
}
