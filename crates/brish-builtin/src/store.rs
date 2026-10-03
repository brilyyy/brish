//! Store plugin manifests and discovery (`docs/PLUGINS.md`, store
//! stages). A plugin is a directory under `plugins/` containing a
//! `plugin.toml`; the manifest declares which seams it contributes
//! (declarative data and/or helper subprocess commands).

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Manifest file name inside each plugin directory.
pub const MANIFEST: &str = "plugin.toml";
/// Internal install metadata (origin + pinned commit), written by `plugin add`.
pub const META: &str = ".brish-store.toml";

/// `plugins/<name>/plugin.toml`. All sections optional — a manifest may
/// contribute any subset of the seams.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Declarative prompt theme (template string).
    #[serde(default)]
    pub theme: Option<ThemeDecl>,
    /// Declarative keybindings `key = "event"`.
    #[serde(default)]
    pub keymap: BTreeMap<String, String>,
    /// Declarative completion wordlists.
    #[serde(default)]
    pub completion: Option<CompletionDecl>,
    /// Prompt segment: runs `cmd`, caches its output.
    #[serde(default)]
    pub segment: Option<SegmentDecl>,
    /// Hook commands (argv, PATH-resolved), by event.
    #[serde(default)]
    pub hooks: Option<HooksDecl>,
    /// Helper executable for completion/keymap seams.
    #[serde(default)]
    pub helper: Option<HelperDecl>,
}

#[derive(Debug, Deserialize)]
pub struct ThemeDecl {
    pub name: String,
    pub prompt: String,
}

#[derive(Debug, Deserialize)]
pub struct CompletionDecl {
    /// Suggestions at any argument position.
    #[serde(default)]
    pub words: Vec<String>,
    /// Extra candidates at command position.
    #[serde(default)]
    pub commands: Vec<String>,
    /// `args.<command>` — suggestions when the word before the cursor
    /// starts with `<command>` (e.g. `args.git = ["checkout", "rebase"]`).
    #[serde(default)]
    pub args: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct SegmentDecl {
    pub name: String,
    pub cmd: Vec<String>,
    /// Cache TTL in seconds (0 = re-run on every prompt render).
    #[serde(default = "default_cache_secs")]
    pub cache_secs: u64,
}

fn default_cache_secs() -> u64 {
    1
}

#[derive(Debug, Deserialize)]
pub struct HooksDecl {
    #[serde(default)]
    pub pre_exec: Vec<Vec<String>>,
    #[serde(default)]
    pub post_exec: Vec<Vec<String>>,
    #[serde(default)]
    pub chdir: Vec<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct HelperDecl {
    /// Path to the helper executable, relative to the plugin directory.
    pub path: PathBuf,
    /// Per-call deadline for every helper invocation.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    250
}

/// `.brish-store.toml` written by `plugin add` — where the plugin came
/// from, so `update`/`info` can find it again.
#[derive(Debug, Deserialize)]
pub struct StoreMeta {
    pub source: String,
    /// Pinned commit for index installs.
    #[serde(default)]
    pub commit: Option<String>,
    /// True when installed through the index (commit must verify).
    #[serde(default)]
    pub index: bool,
}

/// One discovered plugin: manifest + its directory.
pub struct Stored {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

/// Discover `plugins/<name>/plugin.toml` entries. Bad manifests warn and
/// skip; a directory without a manifest is not a plugin and is skipped
/// silently. Never fails: missing dir = empty list.
pub fn scan(plugins_dir: &Path) -> (Vec<Stored>, Vec<String>) {
    let mut out = Vec::new();
    let mut warnings = Vec::new();
    let entries = match std::fs::read_dir(plugins_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (out, warnings),
        Err(e) => {
            warnings.push(format!("plugin store: {}: {e}", plugins_dir.display()));
            return (out, warnings);
        }
    };
    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let mpath = dir.join(MANIFEST);
        if !mpath.is_file() {
            continue;
        }
        let text = match std::fs::read_to_string(&mpath) {
            Ok(t) => t,
            Err(e) => {
                warnings.push(format!("plugin store: {}: {e}", mpath.display()));
                continue;
            }
        };
        let manifest: Manifest = match toml::from_str(&text) {
            Ok(m) => m,
            Err(e) => {
                warnings.push(format!("plugin store: {}: {e}", mpath.display()));
                continue;
            }
        };
        let dirname = dir.file_name().and_then(|s| s.to_str()).unwrap_or_default();
        if manifest.name.is_empty() {
            warnings.push(format!("plugin store: {}: empty `name`", mpath.display()));
            continue;
        }
        if manifest.name != dirname {
            warnings.push(format!(
                "plugin store: {}: name `{}` does not match directory `{}`",
                mpath.display(),
                manifest.name,
                dirname
            ));
            continue;
        }
        out.push(Stored { dir, manifest });
    }
    out.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
    (out, warnings)
}

/// Read `.brish-store.toml` from an installed plugin dir.
pub fn read_meta(dir: &Path) -> Option<StoreMeta> {
    let text = std::fs::read_to_string(dir.join(META)).ok()?;
    toml::from_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: &str = r#"
name = "fancy"
version = "1.0.0"
description = "Fancy things"

[theme]
name = "fancy"
prompt = "{arrow} {cwd} {segments}"

[keymap]
"ctrl-g" = "menu-next"

[completion]
words = ["--verbose"]
commands = ["fancy"]
[completion.args]
git = ["checkout", "rebase"]

[segment]
name = "uptime"
cmd = ["uptime", "-p"]
cache_secs = 2

[hooks]
pre_exec = [["guard", "check"]]

[helper]
path = "bin/fancy-brish"
timeout_ms = 400
"#;

    fn write_plugin(root: &Path, dir_name: &str, manifest: &str) -> PathBuf {
        let dir = root.join(dir_name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MANIFEST), manifest).unwrap();
        dir
    }

    #[test]
    fn parses_full_manifest() {
        let (plugins, warnings) = {
            let tmp = tempfile::tempdir().unwrap();
            write_plugin(tmp.path(), "fancy", FULL);
            scan(tmp.path())
        };
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(plugins.len(), 1);
        let m = &plugins[0].manifest;
        assert_eq!(m.name, "fancy");
        assert_eq!(m.version, "1.0.0");
        let theme = m.theme.as_ref().unwrap();
        assert_eq!(theme.name, "fancy");
        assert_eq!(
            m.keymap.get("ctrl-g").map(String::as_str),
            Some("menu-next")
        );
        let comp = m.completion.as_ref().unwrap();
        assert_eq!(comp.words, vec!["--verbose"]);
        assert_eq!(
            comp.args.get("git").map(|v| v.as_slice()),
            Some(["checkout".to_string(), "rebase".to_string()].as_slice())
        );
        let seg = m.segment.as_ref().unwrap();
        assert_eq!(seg.cache_secs, 2);
        let hooks = m.hooks.as_ref().unwrap();
        assert_eq!(
            hooks.pre_exec,
            vec![vec!["guard".to_string(), "check".to_string()]]
        );
        let helper = m.helper.as_ref().unwrap();
        assert_eq!(helper.timeout_ms, 400);
    }

    #[test]
    fn defaults_for_minimal_manifest() {
        let (plugins, warnings) = {
            let tmp = tempfile::tempdir().unwrap();
            write_plugin(tmp.path(), "tiny", "name = \"tiny\"\n");
            scan(tmp.path())
        };
        assert!(warnings.is_empty(), "{warnings:?}");
        let m = &plugins[0].manifest;
        assert!(m.theme.is_none());
        assert!(m.segment.is_none());
        assert!(m.helper.is_none());
        let seg = SegmentDecl {
            name: "s".into(),
            cmd: vec!["echo".into()],
            cache_secs: default_cache_secs(),
        };
        assert_eq!(seg.cache_secs, 1);
        let h = HelperDecl {
            path: PathBuf::from("h"),
            timeout_ms: default_timeout_ms(),
        };
        assert_eq!(h.timeout_ms, 250);
    }

    #[test]
    fn bad_toml_warns_and_skips() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(tmp.path(), "broken", "name = [not toml");
        let (plugins, warnings) = scan(tmp.path());
        assert!(plugins.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("broken"));
    }

    #[test]
    fn missing_name_warns_and_skips() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(tmp.path(), "nameless", "version = \"1\"\n");
        let (plugins, warnings) = scan(tmp.path());
        assert!(plugins.is_empty());
        assert!(warnings[0].contains("nameless"));
    }

    #[test]
    fn name_dir_mismatch_warns_and_skips() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(tmp.path(), "wrongdir", "name = \"other\"\n");
        let (plugins, warnings) = scan(tmp.path());
        assert!(plugins.is_empty());
        assert!(warnings[0].contains("does not match directory"));
    }

    #[test]
    fn empty_name_warns_and_skips() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(tmp.path(), "anon", "name = \"\"\n");
        let (plugins, warnings) = scan(tmp.path());
        assert!(plugins.is_empty());
        assert!(warnings[0].contains("empty"));
    }

    #[test]
    fn non_manifest_dirs_silently_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        std::fs::write(tmp.path().join("readme.txt"), "hi").unwrap();
        let (plugins, warnings) = scan(tmp.path());
        assert!(plugins.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn missing_dir_is_empty_not_error() {
        let tmp = tempfile::tempdir().unwrap();
        let (plugins, warnings) = scan(&tmp.path().join("nope"));
        assert!(plugins.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn scan_sorts_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(tmp.path(), "zeta", "name = \"zeta\"\n");
        write_plugin(tmp.path(), "alpha", "name = \"alpha\"\n");
        let (plugins, _) = scan(tmp.path());
        let names: Vec<_> = plugins.iter().map(|p| p.manifest.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
    }

    #[test]
    fn meta_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(tmp.path(), "m", "name = \"m\"\n");
        std::fs::write(
            tmp.path().join("m").join(META),
            "source = \"https://x/y\"\ncommit = \"abc\"\nindex = true\n",
        )
        .unwrap();
        let meta = read_meta(&tmp.path().join("m")).unwrap();
        assert_eq!(meta.source, "https://x/y");
        assert_eq!(meta.commit.as_deref(), Some("abc"));
        assert!(meta.index);
    }
}
