//! Expander: lex → expand every word with a dummy command-substitution
//! callback (returns empty). Never panics; depth limits are the
//! expander's own contract under fuzz.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else { return };
    let Ok(lexed) = brish_core::lexer::lex(s) else { return };
    let mut env = brish_core::env::Env::new();
    let mut cs = |_src: &str| Ok::<String, brish_core::error::Error>(String::new());
    for t in &lexed.tokens {
        if let brish_core::lexer::Tok::Word(w) = &t.tok {
            let _ = brish_core::expand::expand_words(&mut env, std::slice::from_ref(w), &mut cs);
        }
    }
});
