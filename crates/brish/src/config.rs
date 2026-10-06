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
pub use brish_builtin::paths::{config_path, history_path, rc_path};

/// Parsed `config.toml` (all sections optional, unknown keys ignored).
#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct ConfigFile {
    pub theme: Option<ThemeSection>,
    pub plugins: Option<PluginsSection>,
    pub prompt: Option<PromptSection>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct ThemeSection {
    pub name: Option<String>,
    /// Custom prompt template (`{arrow} {cwd} {segments} {fg:…}`,
    /// `\n` allowed). Overrides the named-theme render when set;
    /// `PS1` env still wins.
    pub prompt: Option<String>,
}

/// Prompt chrome (indicators + completion description style).
/// Absent values keep the built-in defaults.
#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct PromptSection {
    /// Edit-mode indicator (emacs/default).
    pub indicator: Option<String>,
    /// Vi normal-mode indicator.
    pub vi_normal: Option<String>,
    /// Vi visual-mode indicator.
    pub vi_visual: Option<String>,
    /// Multi-line (continuation) indicator.
    pub multiline: Option<String>,
    /// Completion menu description color: named color, `#rrggbb`, or
    /// `off`. Absent = reedline default (muted gray).
    pub completion_description: Option<String>,
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
    /// `[theme] prompt` template, if any (overrides named theme).
    pub theme_prompt: Option<String>,
    /// Prompt chrome with defaults applied.
    pub prompt: PromptChrome,
    enabled: Option<Vec<String>>,
    disabled: Option<Vec<String>>,
}

/// Resolved indicator strings. Empty = toggle off (renders nothing).
#[derive(Debug, Clone)]
pub struct PromptChrome {
    pub indicator: String,
    pub vi_normal: String,
    pub vi_visual: String,
    pub multiline: String,
    pub completion_description: Option<String>,
}

impl Default for PromptChrome {
    fn default() -> Self {
        Self {
            indicator: "> ".into(),
            vi_normal: ": ".into(),
            vi_visual: "+ ".into(),
            multiline: "::: ".into(),
            completion_description: None,
        }
    }
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

/// Load the user config (`~/.config/brish/config.toml`) with extra
/// known plugin names (`extra` = store plugins found on disk) so
/// disabling an installed plugin does not warn as unknown.
pub fn load_with_known(extra: &[&str]) -> Config {
    let theme_cat = brish_theme::catalog();
    let plugin_cat = builtin::catalog();
    let mut known: Vec<&str> = theme_cat.iter().map(|e| e.plugin.name()).collect();
    known.extend(plugin_cat.iter().map(|e| e.plugin.name()));
    known.extend_from_slice(extra);
    known.push(crate::completion::DEFAULT_COMPLETION);
    known.extend(crate::engine_plugins().map(|(n, _, _)| n));
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
    let theme = file.theme.as_ref().and_then(|t| t.name.clone());
    let theme_prompt = file.theme.and_then(|t| t.prompt);
    let mut prompt = PromptChrome::default();
    if let Some(p) = file.prompt {
        if let Some(v) = p.indicator {
            prompt.indicator = v;
        }
        if let Some(v) = p.vi_normal {
            prompt.vi_normal = v;
        }
        if let Some(v) = p.vi_visual {
            prompt.vi_visual = v;
        }
        if let Some(v) = p.multiline {
            prompt.multiline = v;
        }
        prompt.completion_description = p.completion_description;
    }
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
        theme_prompt,
        prompt,
        enabled,
        disabled,
    };
    cfg.warn_unknown_plugins(known);
    cfg
}

/// `[prompt] completion_description` → style override for the
/// completion menu descriptions (NOTES.md 8). `None` = caller default.
/// `off` yields a plain (colorless) style so descriptions go muted.
pub fn completion_desc_style() -> Option<nu_ansi_term::Style> {
    let text = std::fs::read_to_string(config_path()).ok()?;
    let file: ConfigFile = toml::from_str(&text).ok()?;
    let spec = file.prompt?.completion_description?;
    parse_style(&spec)
}

/// Color spec: named (`black`…`white`, optional `bright-` prefix),
/// `#rrggbb`, or `off` (plain). Unknown → None.
fn parse_style(spec: &str) -> Option<nu_ansi_term::Style> {
    use nu_ansi_term::{Color, Style};
    let s = spec.trim();
    if s.eq_ignore_ascii_case("off") {
        return Some(Style::new());
    }
    if let Some(hex) = s.strip_prefix('#')
        && hex.len() == 6
        && let Ok(n) = u32::from_str_radix(hex, 16)
    {
        return Some(Style::new().fg(Color::Rgb(
            ((n >> 16) & 0xff) as u8,
            ((n >> 8) & 0xff) as u8,
            (n & 0xff) as u8,
        )));
    }
    let (bright, name) = s
        .strip_prefix("bright-")
        .map(|r| (true, r))
        .unwrap_or((false, s));
    let base = match name.to_ascii_lowercase().as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "gray" | "grey" => Color::DarkGray,
        _ => return None,
    };
    let color = match (bright, base) {
        (true, Color::Black) => Color::Fixed(8),
        (true, Color::Red) => Color::Fixed(9),
        (true, Color::Green) => Color::Fixed(10),
        (true, Color::Yellow) => Color::Fixed(11),
        (true, Color::Blue) => Color::Fixed(12),
        (true, Color::Magenta) => Color::Fixed(13),
        (true, Color::Cyan) => Color::Fixed(14),
        (true, Color::White) => Color::Fixed(15),
        (_, c) => c,
    };
    Some(Style::new().fg(color))
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

    #[test]
    fn parses_theme_prompt_and_chrome() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let p = write(
            dir.path(),
            "[theme]\nname = \"plain\"\nprompt = \"{arrow} {cwd}\\n> \"\n\
             [prompt]\nindicator = \"$ \"\nvi_normal = \"n \"\nvi_visual = \"v \"\n\
             multiline = \".. \"\ncompletion_description = \"off\"\n",
        );
        let cfg = load_from(&p, &[]);
        assert_eq!(cfg.theme.as_deref(), Some("plain"));
        assert_eq!(cfg.theme_prompt.as_deref(), Some("{arrow} {cwd}\n> "));
        assert_eq!(cfg.prompt.indicator, "$ ");
        assert_eq!(cfg.prompt.vi_normal, "n ");
        assert_eq!(cfg.prompt.vi_visual, "v ");
        assert_eq!(cfg.prompt.multiline, ".. ");
        assert_eq!(cfg.prompt.completion_description.as_deref(), Some("off"));
    }

    #[test]
    fn chrome_defaults_when_absent() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let p = write(dir.path(), "[theme]\nname = \"briiish\"\n");
        let cfg = load_from(&p, &[]);
        assert_eq!(cfg.prompt.indicator, "> ");
        assert_eq!(cfg.prompt.vi_normal, ": ");
        assert_eq!(cfg.prompt.vi_visual, "+ ");
        assert_eq!(cfg.prompt.multiline, "::: ");
        assert!(cfg.prompt.completion_description.is_none());
        assert!(cfg.theme_prompt.is_none());
    }

    #[test]
    fn parses_color_specs() {
        use nu_ansi_term::{Color, Style};
        assert_eq!(parse_style("off"), Some(Style::new()));
        assert_eq!(parse_style("red"), Some(Style::new().fg(Color::Red)));
        assert_eq!(
            parse_style("bright-blue"),
            Some(Style::new().fg(Color::Fixed(12)))
        );
        assert_eq!(
            parse_style("#ff8000"),
            Some(Style::new().fg(Color::Rgb(255, 128, 0)))
        );
        assert_eq!(parse_style("nope"), None);
    }
}
