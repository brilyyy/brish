//! Startup configuration and the `.config/brish` path family.
//!
//! `~/.config/brish/config.toml` — project rule: literal `.config`
//! under `$HOME` on every platform (not the platform-native config
//! dir). A broken or unknown config never crashes: warn once, use
//! defaults (plan P3).

use crate::builtin;
use std::collections::BTreeMap;
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
    pub highlight: Option<HighlightSection>,
    pub errors: Option<ErrorsSection>,
    pub not_found: Option<NotFoundSection>,
    pub hooks: Option<HooksSection>,
    pub engine: Option<EngineSection>,
    pub completion: Option<CompletionSection>,
    pub update: Option<UpdateSection>,
    pub startup: Option<StartupSection>,
}

/// `[completion]` — how Tab matches candidates.
#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct CompletionSection {
    /// `prefix` (default), `substring`, or `fuzzy` (subsequence).
    pub algorithm: Option<String>,
    /// Sort candidates alphabetically before display (default true).
    pub sort: Option<bool>,
    /// Also match the typed text against each candidate's description.
    pub match_description: Option<bool>,
}

/// `[update]` — auto-update check on startup (oh-my-zsh style).
#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct UpdateSection {
    /// Enable automatic update check (default true).
    pub enabled: Option<bool>,
    /// Check interval in days (default 13, oh-my-zsh default).
    pub interval_days: Option<u64>,
}

/// `[startup]` — first-run and startup banner behavior.
#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct StartupSection {
    /// Show banner on very first run (default true).
    pub banner_first_init: Option<bool>,
    /// Show banner when version check finds an update (default true).
    pub banner_on_update: Option<bool>,
}

/// `[hooks]` — shell-level hooks the engine runs.
#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct HooksSection {
    /// Command line run with the missing name as its only argument
    /// when a command is not found (interactive only). Its stdout is
    /// the answer (nushell's `command_not_found` hook, Arch/NixOS
    /// `command-not-found` style).
    pub command_not_found: Option<String>,
}

/// `[engine]` — engine behavior knobs.
#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct EngineSection {
    /// Command duration mode: `wall` (default) or `cpu`.
    /// Affects the `{cmd_duration}` token.
    pub cmd_duration_mode: Option<String>,
}

/// `[errors]` — error report verbosity.
#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct ErrorsSection {
    /// `fancy` (source excerpt + caret), `short`, or `plain`. Absent =
    /// mode default (fancy on a tty, short in batch).
    pub style: Option<String>,
}

/// `[not_found]` — interactive command-not-found report.
#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct NotFoundSection {
    /// `fancy` (colored report + `did you mean` hint), `short`, or
    /// `plain`. Absent = mode default (fancy on a tty, short in batch).
    pub style: Option<String>,
    /// `did you mean` hint (default true). `false` also skips the
    /// edit-distance scan.
    pub suggest: Option<bool>,
}

/// `[highlight]` — zsh-patina-style dynamic highlighting knobs.
#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct HighlightSection {
    /// Invalid commands red + existing paths underlined (default on).
    pub dynamic: Option<bool>,
}

#[derive(Default, serde::Deserialize)]
#[serde(default)]
pub struct ThemeSection {
    pub name: Option<String>,
    /// Custom prompt template (`{arrow} {cwd} {segments} {fg:…}`,
    /// `\n` allowed). Overrides the named-theme render when set;
    /// `PS1` env still wins.
    pub prompt: Option<String>,
    /// Right-hand prompt template (same tokens as `prompt`). Empty /
    /// absent = no right prompt.
    pub prompt_right: Option<String>,
    /// Transient prompt (nushell-style): shown after a command runs,
    /// so a long prompt does not scroll away with the output. Absent
    /// or empty = no transient prompt (behaviour unchanged).
    pub prompt_transient: Option<String>,
    /// Per-segment palette for powerline themes: segment_name = "bg_color".
    /// Color is a 256-color index (0-255) or named color.
    /// Example: { git = "11", venv = "5", aws = "208" }
    pub palette: Option<BTreeMap<String, String>>,
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
pub struct Config {
    /// `[theme] name`, if any (priority: `--theme` > env > this).
    pub theme: Option<String>,
    /// `[theme] prompt` template, if any (overrides named theme).
    pub theme_prompt: Option<String>,
    /// `[theme] prompt_right` template, if any.
    pub theme_prompt_right: Option<String>,
    /// `[theme] prompt_transient` template, if any.
    pub theme_prompt_transient: Option<String>,
    /// `[theme] palette` map: segment_name -> bg_color (256-color index or name).
    pub theme_palette: Option<BTreeMap<String, String>>,
    /// Prompt chrome with defaults applied.
    pub prompt: PromptChrome,
    /// `[highlight] dynamic` (zsh-patina-style): default true.
    pub dynamic_highlight: bool,
    /// `[errors] style` override, if any.
    pub error_style: Option<String>,
    /// `[not_found] style` override, if any.
    pub not_found_style: Option<String>,
    /// `[not_found] suggest` (default true).
    pub not_found_suggest: bool,
    /// `[hooks] command_not_found` line, if any.
    pub command_not_found: Option<String>,
    /// `[completion] algorithm`, if any.
    pub completion_algorithm: Option<String>,
    /// `[completion] sort`, if any.
    pub completion_sort: Option<bool>,
    /// `[completion] match_description`, if any.
    pub completion_match_description: Option<bool>,
    /// `[engine] cmd_duration_mode`: "wall" (default) or "cpu".
    pub cmd_duration_mode: CmdDurationMode,
    /// `[update] enabled`: check for updates on startup (default true).
    pub update_enabled: bool,
    /// `[update] interval_days`: check interval (default 13).
    pub update_interval_days: u64,
    /// `[startup] banner_first_init`: show banner on first run (default true).
    pub banner_first_init: bool,
    /// `[startup] banner_on_update`: show banner when update found (default true).
    pub banner_on_update: bool,
    enabled: Option<Vec<String>>,
    disabled: Option<Vec<String>>,
}

use brish_plugin_api::CmdDurationMode;

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: None,
            theme_prompt: None,
            theme_prompt_right: None,
            theme_prompt_transient: None,
            theme_palette: None,
            prompt: PromptChrome::default(),
            dynamic_highlight: true,
            error_style: None,
            not_found_style: None,
            not_found_suggest: true,
            command_not_found: None,
            completion_algorithm: None,
            completion_sort: None,
            completion_match_description: None,
            cmd_duration_mode: CmdDurationMode::default(),
            update_enabled: true,
            update_interval_days: 13,
            banner_first_init: true,
            banner_on_update: true,
            enabled: None,
            disabled: None,
        }
    }
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
    let (theme_prompt, theme_prompt_right, theme_prompt_transient, theme_palette) = match file.theme
    {
        Some(t) => (t.prompt, t.prompt_right, t.prompt_transient, t.palette),
        None => (None, None, None, None),
    };
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
    let dynamic_highlight = file.highlight.and_then(|h| h.dynamic).unwrap_or(true);
    let (not_found_style, not_found_suggest) = match file.not_found {
        Some(n) => (n.style, n.suggest.unwrap_or(true)),
        None => (None, true),
    };
    let cmd_duration_mode = file
        .engine
        .as_ref()
        .and_then(|e| e.cmd_duration_mode.as_deref())
        .map(CmdDurationMode::from_str)
        .unwrap_or_default();
    let cfg = Config {
        theme,
        theme_prompt,
        theme_prompt_right,
        theme_prompt_transient,
        theme_palette,
        prompt,
        dynamic_highlight,
        error_style: file.errors.and_then(|e| e.style),
        not_found_style,
        not_found_suggest,
        command_not_found: file
            .hooks
            .and_then(|h| h.command_not_found)
            .filter(|h| !h.trim().is_empty()),
        completion_algorithm: file.completion.as_ref().and_then(|c| c.algorithm.clone()),
        completion_sort: file.completion.as_ref().and_then(|c| c.sort),
        completion_match_description: file.completion.as_ref().and_then(|c| c.match_description),
        cmd_duration_mode,
        update_enabled: file.update.as_ref().and_then(|u| u.enabled).unwrap_or(true),
        update_interval_days: file
            .update
            .as_ref()
            .and_then(|u| u.interval_days)
            .unwrap_or(13),
        banner_first_init: file
            .startup
            .as_ref()
            .and_then(|s| s.banner_first_init)
            .unwrap_or(true),
        banner_on_update: file
            .startup
            .as_ref()
            .and_then(|s| s.banner_on_update)
            .unwrap_or(true),
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
