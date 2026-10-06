//! Parser must never panic (and never stack-overflow via depth limits)
//! on arbitrary input.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(s) = std::str::from_utf8(data) {
        if let Ok(p) = brish_core::parser::parse(s) {
            // Exercise AST Debug (span/shape assumptions) too.
            let _ = format!("{p:?}");
        }
    }
});
