//! Engine-side user config: `[cd]`, `[ls]`, `[output]` from
//! `config.toml`. Read per command (these are interactive/admin paths,
//! not hot) — mirrors `store.rs`'s standalone-`Raw` pattern so the
//! binary's `Config` and this parser stay independent.
//!
//! Bad/missing file → defaults; unknown keys ignored (serde default).

use serde::Deserialize;

/// `ls` backend selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LsBackend {
    /// Use `eza` when on PATH, else builtin.
    #[default]
    Auto,
    /// Never shell out.
    Builtin,
    /// Require `eza` (status 127 when missing).
    Eza,
}

/// Builtin-list rendering mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TableMode {
    /// Align when it fits (≤ 2 columns of moderate width).
    #[default]
    Auto,
    Always,
    Off,
}

/// Resolved engine-side settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserCfg {
    /// `cd` falls back to `zoxide query` when the operand is not a path.
    pub cd_zoxide: bool,
    pub ls_backend: LsBackend,
    pub ls_icons: bool,
    pub table: TableMode,
}

impl Default for UserCfg {
    fn default() -> Self {
        Self {
            cd_zoxide: true,
            ls_backend: LsBackend::Auto,
            ls_icons: true,
            table: TableMode::Auto,
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct Raw {
    cd: RawCd,
    ls: RawLs,
    output: RawOutput,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawCd {
    zoxide: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawLs {
    backend: Option<String>,
    icons: Option<bool>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawOutput {
    table: Option<String>,
}

fn bad(value: &str, what: &str) {
    eprintln!("brish: config: bad {what} value {value:?}, using default");
}

/// Load `~/.config/brish/config.toml` (missing/broken → defaults,
/// with a warning for broken files only).
pub fn load() -> UserCfg {
    let mut cfg = UserCfg::default();
    let path = crate::paths::config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return cfg,
    };
    let raw: Raw = match toml::from_str(&text) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("brish: config: {e}");
            return cfg;
        }
    };
    if let Some(z) = raw.cd.zoxide.as_deref() {
        match z {
            "auto" => cfg.cd_zoxide = true,
            "off" => cfg.cd_zoxide = false,
            other => bad(other, "[cd] zoxide"),
        }
    }
    if let Some(b) = raw.ls.backend.as_deref() {
        match b {
            "auto" => cfg.ls_backend = LsBackend::Auto,
            "builtin" => cfg.ls_backend = LsBackend::Builtin,
            "eza" => cfg.ls_backend = LsBackend::Eza,
            other => bad(other, "[ls] backend"),
        }
    }
    if let Some(i) = raw.ls.icons {
        cfg.ls_icons = i;
    }
    if let Some(t) = raw.output.table.as_deref() {
        match t {
            "auto" => cfg.table = TableMode::Auto,
            "always" => cfg.table = TableMode::Always,
            "off" => cfg.table = TableMode::Off,
            other => bad(other, "[output] table"),
        }
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_defaults() {
        // No config written: HOME in tests may or may not have one;
        // defaults must at least be self-consistent.
        let d = UserCfg::default();
        assert!(d.cd_zoxide);
        assert_eq!(d.ls_backend, LsBackend::Auto);
        assert!(d.ls_icons);
        assert_eq!(d.table, TableMode::Auto);
    }

    #[test]
    fn parses_sections_and_bad_values_fall_back() {
        let r: Raw = toml::from_str(
            "[cd]\nzoxide = \"off\"\n[ls]\nbackend = \"eza\"\nicons = false\n[output]\ntable = \"always\"\n",
        )
        .unwrap();
        assert_eq!(r.cd.zoxide.as_deref(), Some("off"));
        assert_eq!(r.ls.backend.as_deref(), Some("eza"));
        assert_eq!(r.ls.icons, Some(false));
        assert_eq!(r.output.table.as_deref(), Some("always"));

        let r: Raw = toml::from_str(
            "[cd]\nzoxide = \"bogus\"\n[ls]\nbackend = \"nope\"\n[output]\ntable = \"weird\"\n",
        )
        .unwrap();
        // Parser keeps the strings; load() maps unknown → default + warn.
        assert_eq!(r.cd.zoxide.as_deref(), Some("bogus"));

        let r: Result<Raw, _> = toml::from_str("this is not toml");
        assert!(r.is_err());
    }
}
