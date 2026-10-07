//! briSH plugins: every bundled implementation.
//!
//! Layered above `brish-engine` (NOTES.md 1) so plugins that need
//! engine types live here rather than in the binary. The trait and
//! registry surface is `brish-plugin-api`, which stays engine-free and
//! is what a third-party Rust plugin compiles against; this crate
//! depends on it and on the engine.
//!
//! The binary (`brish`) owns `config.rs` -> `build_registry()` wiring
//! and the REPL, and filters this catalog through
//! `~/.config/brish/config.toml`.

pub mod builtin;
pub mod completion;
pub mod config;
pub mod edit_mode;
pub mod highlight;
pub mod hinter;
pub mod hist;
pub mod keymap;
pub mod packs;
pub mod prompt;
pub mod report;

use brish_plugin_api::Plugin;

/// REPL-facing plugins wired in `build_registry`, as
/// `(name, plugin, default_enabled)`.
///
/// `brish-vi` is opt-in — `enabled = ["brish-vi"]` (or `disabled`
/// minus emacs) selects it; `Registry::edit_mode_factory` takes the
/// first. `brish-syntax-highlight` is NOT here: it carries runtime
/// state (aliases + dynamic flag) and is built in `build_registry`.
pub fn engine_plugins() -> [(&'static str, &'static dyn Plugin, bool); 6] {
    [
        ("brish-autosuggest", &hinter::AutosuggestPlugin, true),
        ("brish-emacs", &edit_mode::EmacsModePlugin, true),
        ("brish-vi", &edit_mode::ViModePlugin, false),
        ("brish-menus", &edit_mode::DefaultMenusPlugin, true),
        (
            "brish-history-search",
            &edit_mode::HistorySearchPlugin,
            true,
        ),
        ("brish-validator", &edit_mode::ValidatorPlugin, true),
    ]
}
