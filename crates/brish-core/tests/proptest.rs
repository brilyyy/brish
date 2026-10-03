#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Property tests (plan 6.4): the front half of the shell must never
//! panic on any input — fuzz-shaped strings go through lexer, parser and
//! expander; the expander's contract with `Env` round-trips.

use brish_core::env::Env;
use brish_core::expand;
use brish_core::lexer;
use brish_core::parser;
use proptest::prelude::*;

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
        let mut cs = |_s: &str| -> Result<String, brish_core::error::Error> { Ok(String::new()) };
        let _ = expand::expand_text(&mut env, &src, &mut cs);
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
        let mut cs = |_s: &str| -> Result<String, brish_core::error::Error> { Ok(String::new()) };
        prop_assert_eq!(expand::expand_text(&mut env, &src, &mut cs).unwrap(), src);
    }
}
