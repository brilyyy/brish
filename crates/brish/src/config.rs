//! Startup configuration and the `.config/brish` path family.
//!
//! `~/.config/brish/config.toml` — project rule: literal `.config`
//! under `$HOME` on every platform (not the platform-native config
//! dir). A broken or unknown config never crashes: warn once, use
//! defaults (plan P3).

use brish_plugin::builtin;
use std::io::ErrorKind;
use std::path::Path;

// The `~/.config/brish` path family lives with the engine so the
// `plugin` builtin and the binary share one root.
pub use brish_builtin::paths::{config_dir, config_path, history_path, rc_path};

/// Parsed `config.toml` (all sections optional, unknown keys ignored).
#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct ConfigFile {
    pub theme: Option<ThemeSection>,
    pub plugins: Option<PluginsSection>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct ThemeSection {
    pub name: Option<String>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct PluginsSection {
    pub enabled: Option<Vec<String>>,
    pub disabled: Option<Vec<String>>,
}

/// Effective configuration after parsing + validation.
#[derive(Default)]
pub struct Config {
    /// `[theme] name`, if any (priority: `--theme` > env > this).
    pub theme: Option<String>,
    enabled: Option<Vec<String>>,
    disabled: Option<Vec<String>>,
}

impl Config {
    /// Does plugin `name` run? An explicit `enabled` list replaces the
    /// catalog default; otherwise `disabled` subtracts from it.
    pub fn plugin_enabled(&self, name: &str, default_enabled: bool) -> bool {
        match &self.enabled {
            Some(list) => list.iter().any(|n| n == name),
            None => {
                !self
                    .disabled
                    .as_ref()
                    .is_some_and(|l| l.iter().any(|n| n == name))
                    && default_enabled
            }
        }
    }

    /// Warn about names the catalog does not know (never fatal).
    pub fn warn_unknown_plugins(&self, known: &[&str]) {
        for name in self
            .enabled
            .iter()
            .flatten()
            .chain(self.disabled.iter().flatten())
        {
            if !known.contains(&name.as_str()) {
                eprintln!("brish: unknown plugin: {name}");
            }
        }
    }
}

/// Load the user config (`~/.config/brish/config.toml`), warning about
/// unknown plugin names against the in-tree catalog.
pub fn load() -> Config {
    let catalog = builtin::catalog();
    let mut known: Vec<&str> = catalog.iter().map(|e| e.plugin.name()).collect();
    known.push(crate::completion::DEFAULT_COMPLETION);
    load_from(&config_path(), &known)
}

/// Load a specific file (tests). Missing file → defaults, silent;
/// unreadable/broken file → warning + defaults.
pub fn load_from(path: &Path, known: &[&str]) -> Config {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == ErrorKind::NotFound => return Config::default(),
        Err(e) => {
            eprintln!("brish: config: {e}");
            return Config::default();
        }
    };
    let file: ConfigFile = match toml::from_str(&text) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("brish: config: {e}");
            return Config::default();
        }
    };
    let theme = file.theme.and_then(|t| t.name);
    let (enabled, mut disabled) = match file.plugins {
        Some(p) => (p.enabled, p.disabled),
        None => (None, None),
    };
    if enabled.is_some() && disabled.is_some() {
        eprintln!("brish: config: [plugins] enabled overrides disabled");
        disabled = None;
    }
    let cfg = Config {
        theme,
        enabled,
        disabled,
    };
    cfg.warn_unknown_plugins(known);
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write(dir: &Path, text: &str) -> PathBuf {
        let p = dir.join("config.toml");
        std::fs::write(&p, text).expect("write");
        p
    }

    #[test]
    fn missing_file_is_defaults() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let cfg = load_from(&dir.path().join("nope.toml"), &["a"]);
        assert!(cfg.theme.is_none());
        assert!(cfg.plugin_enabled("a", true));
        assert!(!cfg.plugin_enabled("a", false));
    }

    #[test]
    fn parses_theme_and_enabled_list() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let p = write(
            dir.path(),
            "[theme]\nname = \"minimal\"\n[plugins]\nenabled = [\"a\"]\n",
        );
        let cfg = load_from(&p, &["a", "b"]);
        assert_eq!(cfg.theme.as_deref(), Some("minimal"));
        assert!(cfg.plugin_enabled("a", false), "enabled list includes a");
        assert!(!cfg.plugin_enabled("b", true), "enabled list excludes b");
    }

    #[test]
    fn disabled_subtracts_from_defaults() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let p = write(dir.path(), "[plugins]\ndisabled = [\"a\"]\n");
        let cfg = load_from(&p, &["a", "b"]);
        assert!(!cfg.plugin_enabled("a", true));
        assert!(cfg.plugin_enabled("b", true));
        assert!(!cfg.plugin_enabled("a", false));
    }

    #[test]
    fn bad_toml_falls_back_to_defaults() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let p = write(dir.path(), "this is [not toml");
        let cfg = load_from(&p, &["a"]);
        assert!(cfg.theme.is_none());
        assert!(cfg.plugin_enabled("a", true));
    }

    #[test]
    fn unknown_plugin_names_do_not_crash() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let p = write(dir.path(), "[plugins]\nenabled = [\"ghost\"]\n");
        let cfg = load_from(&p, &["a"]);
        cfg.warn_unknown_plugins(&["a"]); // warn path: exercised, no panic
        assert!(!cfg.plugin_enabled("a", true));
    }

    #[test]
    fn path_fns_live_under_config_dir() {
        assert!(config_path().ends_with(".config/brish/config.toml"));
        assert!(rc_path().ends_with(".config/brish/.brishrc"));
        assert!(history_path().ends_with(".config/brish/.brish_history"));
    }
}
