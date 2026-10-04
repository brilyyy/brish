//! The literal `~/.config/brish` path family (project rule: same path
//! on every platform, `dirs::home_dir()` based). Central home so the
//! engine's `plugin` builtin and the binary agree on one root.

use std::path::PathBuf;

/// `~/.config/brish`.
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/brish")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn rc_path() -> PathBuf {
    config_dir().join(".brishrc")
}

pub fn history_path() -> PathBuf {
    config_dir().join(".brish_history")
}

/// `~/.config/brish/z` — frecency database for `z` builtin.
pub fn z_path() -> PathBuf {
    config_dir().join("z")
}

/// `~/.config/brish/plugins` — one directory per installed store plugin.
pub fn plugins_dir() -> PathBuf {
    config_dir().join("plugins")
}

/// `~/.config/brish/index` — cached clone of the plugin index repo.
pub fn index_cache_dir() -> PathBuf {
    config_dir().join("index")
}
