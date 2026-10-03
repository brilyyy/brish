//! Store plugin manifests and discovery (`docs/PLUGINS.md`, store
//! stages). A plugin is a directory under `plugins/` containing a
//! `plugin.toml`; the manifest declares which seams it contributes
//! (declarative data and/or helper subprocess commands).

use crate::helper;
use brish_plugin::{
    Completion, CompletionCtx, CompletionProvider, KeymapProvider, Plugin, PromptSegment, Registry,
    Theme,
};
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

#[derive(Debug, Clone, Deserialize)]
pub struct ThemeDecl {
    pub name: String,
    pub prompt: String,
}

#[derive(Debug, Clone, Deserialize)]
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

/// A discovered plugin packaged for `Registry::install`.
pub struct StorePlugin {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

impl Stored {
    pub fn into_plugin(self) -> StorePlugin {
        StorePlugin {
            dir: self.dir,
            manifest: self.manifest,
        }
    }
}

impl Plugin for StorePlugin {
    fn name(&self) -> &str {
        &self.manifest.name
    }

    fn install(&self, reg: &mut Registry) {
        if let Some(t) = &self.manifest.theme {
            reg.themes.push(Box::new(TemplateTheme {
                name: t.name.clone(),
                prompt: t.prompt.clone(),
            }));
        }
        if !self.manifest.keymap.is_empty() {
            reg.keymaps
                .push(Box::new(ManifestKeymap(self.manifest.keymap.clone())));
        }
        if let Some(c) = &self.manifest.completion {
            reg.completion_providers.push(Box::new(WordsProvider {
                plugin: self.manifest.name.clone(),
                decl: c.clone(),
            }));
        }
        if let Some(seg) = &self.manifest.segment {
            reg.prompt_segments
                .push(Box::new(helper::HelperSegment::new(
                    &self.manifest.name,
                    self.dir.clone(),
                    seg,
                )));
        }
        if let Some(hooks) = &self.manifest.hooks {
            for argv in &hooks.pre_exec {
                if !argv.is_empty() {
                    reg.pre_exec.push(Box::new(helper::HelperHook::new(
                        &self.manifest.name,
                        self.dir.clone(),
                        argv.clone(),
                        helper::HookEvent::Pre,
                    )));
                }
            }
            for argv in &hooks.post_exec {
                if !argv.is_empty() {
                    reg.post_exec.push(Box::new(helper::HelperHook::new(
                        &self.manifest.name,
                        self.dir.clone(),
                        argv.clone(),
                        helper::HookEvent::Post,
                    )));
                }
            }
            for argv in &hooks.chdir {
                if !argv.is_empty() {
                    reg.on_chdir.push(Box::new(helper::HelperHook::new(
                        &self.manifest.name,
                        self.dir.clone(),
                        argv.clone(),
                        helper::HookEvent::Chdir,
                    )));
                }
            }
        }
        if let Some(hlp) = &self.manifest.helper {
            let cmd = vec![self.dir.join(&hlp.path).display().to_string()];
            reg.completion_providers
                .push(Box::new(helper::HelperCompletion::new(
                    &self.manifest.name,
                    cmd.clone(),
                    self.dir.clone(),
                    hlp.timeout_ms,
                )));
            let pairs = helper::keymap_pairs(&cmd, &self.dir, hlp.timeout_ms);
            if !pairs.is_empty() {
                reg.keymaps
                    .push(Box::new(ManifestKeymap(pairs.into_iter().collect())));
            }
        }
    }
}

/// Declarative theme: template string expanded per render.
pub struct TemplateTheme {
    pub name: String,
    pub prompt: String,
}

impl Theme for TemplateTheme {
    fn name(&self) -> &str {
        &self.name
    }

    fn render(&self, status: i32, cwd: &Path, segments: &[&dyn PromptSegment]) -> String {
        expand_template(
            &self.prompt,
            status,
            cwd,
            segments,
            brish_plugin::color_enabled(),
        )
    }
}

/// Tokens: `{arrow}` `{cwd}` `{segments}` `{reset}` `{fg:…}` `{bg:…}`
/// (named 8+bright or `#rrggbb`). Unknown tokens and unmatched braces
/// pass through literally.
pub fn expand_template(
    tmpl: &str,
    status: i32,
    cwd: &Path,
    segments: &[&dyn PromptSegment],
    color: bool,
) -> String {
    let mut out = String::new();
    let mut rest = tmpl;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let token = &after[..close];
        let replacement = match token {
            "arrow" => Some(arrow(status, color)),
            "cwd" => Some(cwd_base(cwd)),
            "segments" => Some(
                segments
                    .iter()
                    .filter_map(|s| s.render(status, cwd))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            "reset" => Some(if color {
                "\x1b[0m".to_string()
            } else {
                String::new()
            }),
            t => t
                .strip_prefix("fg:")
                .and_then(|spec| styled(spec, true, color))
                .or_else(|| {
                    t.strip_prefix("bg:")
                        .and_then(|spec| styled(spec, false, color))
                }),
        };
        match replacement {
            Some(r) => out.push_str(&r),
            None => {
                out.push('{');
                out.push_str(token);
                out.push('}');
            }
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

fn arrow(status: i32, color: bool) -> String {
    if !color {
        return "\u{279c}".to_string();
    }
    let code = if status == 0 { 32 } else { 31 };
    format!("\x1b[{code}m\u{279c}\x1b[0m")
}

fn cwd_base(cwd: &Path) -> String {
    cwd.file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.display().to_string())
}

/// ANSI code for `red`/`bright-blue`/`#rrggbb`; `None` = unknown name.
fn color_code(spec: &str, fg: bool) -> Option<String> {
    let (bright, name) = match spec.strip_prefix("bright-") {
        Some(n) => (true, n),
        None => (false, spec),
    };
    let idx = match name {
        "black" => 0,
        "red" => 1,
        "green" => 2,
        "yellow" => 3,
        "blue" => 4,
        "magenta" => 5,
        "cyan" => 6,
        "white" => 7,
        _ => return hex_code(spec, fg),
    };
    let base = if fg { 30 } else { 40 };
    let n = if bright { base + 60 + idx } else { base + idx };
    Some(format!("\x1b[{n}m"))
}

fn hex_code(spec: &str, fg: bool) -> Option<String> {
    let h = spec.strip_prefix('#')?;
    if h.len() != 6 || !h.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some(format!("\x1b[{};2;{r};{g};{b}m", if fg { 38 } else { 48 }))
}

fn styled(spec: &str, fg: bool, color: bool) -> Option<String> {
    // Validate the name first so unknown colors fall back to literal.
    color_code(spec, fg).map(|code| if color { code } else { String::new() })
}

/// `[keymap]` table → keymap provider (parsed by the binary's `keymap`
/// module at REPL startup; unknown pairs warn there).
struct ManifestKeymap(BTreeMap<String, String>);

impl KeymapProvider for ManifestKeymap {
    fn bindings(&self) -> Vec<(String, String)> {
        self.0.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
}

/// `[completion]` wordlists, tagged with the plugin name in the menu
/// description so users see where a suggestion came from.
struct WordsProvider {
    plugin: String,
    decl: CompletionDecl,
}

impl CompletionProvider for WordsProvider {
    fn complete(&self, ctx: &CompletionCtx<'_>) -> Vec<Completion> {
        if ctx.after_dollar {
            return Vec::new();
        }
        let mut pool: Vec<&String> = Vec::new();
        if ctx.is_command {
            pool.extend(&self.decl.commands);
        } else {
            pool.extend(&self.decl.words);
            if let Some(parent) = ctx.line_before.split_whitespace().next()
                && let Some(extra) = self.decl.args.get(parent)
            {
                pool.extend(extra);
            }
        }
        pool.into_iter()
            .filter(|c| c.starts_with(ctx.word))
            .map(|c| Completion {
                value: c.clone(),
                description: Some(self.plugin.clone()),
                keep_typing: false,
            })
            .collect()
    }
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

    struct Dash;
    impl PromptSegment for Dash {
        fn render(&self, status: i32, _cwd: &Path) -> Option<String> {
            if status == 0 {
                Some("git:(main)".into())
            } else {
                None
            }
        }
    }

    fn tmpl(s: &str, status: i32, color: bool) -> String {
        let cwd = Path::new("/home/u/proj");
        expand_template(s, status, cwd, &[&Dash], color)
    }

    #[test]
    fn template_expands_tokens() {
        let out = tmpl("{arrow} {cwd} {segments}", 0, false);
        assert_eq!(out, "\u{279c} proj git:(main)", "{out:?}");
        // failing status: no segment (Dash returns None)
        let out = tmpl("{arrow} {cwd}{segments}", 3, false);
        assert_eq!(out, "\u{279c} proj", "{out:?}");
    }

    #[test]
    fn template_colors_only_when_enabled() {
        let on = tmpl("{arrow}", 0, true);
        assert_eq!(on, "\x1b[32m\u{279c}\x1b[0m");
        let off = tmpl("{fg:red}x{reset}", 0, false);
        assert_eq!(off, "x", "colors collapse to empty when disabled");
        let on = tmpl("{fg:red}x{reset}", 0, true);
        assert_eq!(on, "\x1b[31mx\x1b[0m");
        let hex = tmpl("{fg:#0a1b2c}", 0, true);
        assert_eq!(hex, "\x1b[38;2;10;27;44m");
        let bg = tmpl("{bg:bright-white}", 0, true);
        assert_eq!(bg, "\x1b[107m");
    }

    #[test]
    fn template_unknown_tokens_stay_literal() {
        assert_eq!(tmpl("{nope} {cwd", 0, false), "{nope} {cwd");
        assert_eq!(tmpl("{fg:chartreuse}", 0, true), "{fg:chartreuse}");
        assert_eq!(tmpl("plain", 0, true), "plain");
    }

    #[test]
    fn keymap_provider_returns_pairs() {
        let mut m = BTreeMap::new();
        m.insert("ctrl-g".to_string(), "menu-next".to_string());
        let bindings = ManifestKeymap(m).bindings();
        assert_eq!(
            bindings,
            vec![("ctrl-g".to_string(), "menu-next".to_string())]
        );
    }

    fn wctx<'a>(word: &'a str, is_command: bool, before: &'a str) -> CompletionCtx<'a> {
        CompletionCtx {
            word,
            is_command,
            after_dollar: false,
            cwd: Path::new("."),
            line_before: before,
        }
    }

    #[test]
    fn words_provider_routes_by_position() {
        let decl = CompletionDecl {
            words: vec!["--verbose".into()],
            commands: vec!["mytool".into()],
            args: BTreeMap::from([(
                "git".to_string(),
                vec!["checkout".to_string(), "rebase".to_string()],
            )]),
        };
        let p = WordsProvider {
            plugin: "demo".into(),
            decl,
        };
        let out = p.complete(&wctx("my", true, ""));
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].value, "mytool");
        assert_eq!(out[0].description.as_deref(), Some("demo"));

        let out = p.complete(&wctx("--v", false, "mytool "));
        assert_eq!(out.len(), 1, "generic words at any arg position");
        assert_eq!(out[0].value, "--verbose");

        let out = p.complete(&wctx("che", false, "git "));
        assert_eq!(out.len(), 1, "args.git matched on parent word");
        assert_eq!(out[0].value, "checkout");

        let out = p.complete(&wctx("che", false, "hg "));
        assert!(out.is_empty(), "args.git not offered under hg");

        let out = p.complete(&wctx("x", true, ""));
        assert!(out.is_empty(), "prefix filter");
    }

    #[test]
    fn words_provider_ignores_dollar_words() {
        let p = WordsProvider {
            plugin: "demo".into(),
            decl: CompletionDecl {
                words: vec!["--x".into()],
                commands: vec![],
                args: BTreeMap::new(),
            },
        };
        let ctx = CompletionCtx {
            word: "HOM",
            is_command: false,
            after_dollar: true,
            cwd: Path::new("."),
            line_before: "",
        };
        assert!(p.complete(&ctx).is_empty());
    }

    #[test]
    fn store_plugin_installs_helper_seams() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("b").join("bin")).unwrap();
        std::fs::write(
            tmp.path().join("b").join(MANIFEST),
            "name = \"b\"\n             [segment]\nname = \"up\"\ncmd = [\"echo\", \"up\"]\n             [hooks]\npre_exec = [[\"true\"]]\n             [helper]\npath = \"bin/h\"\n",
        )
        .unwrap();
        let (plugins, _) = scan(tmp.path());
        let plugin = plugins.into_iter().next().unwrap().into_plugin();
        let mut reg = Registry::default();
        reg.install(&plugin);
        assert_eq!(reg.prompt_segments.len(), 1);
        assert_eq!(reg.pre_exec.len(), 1);
        assert_eq!(reg.completion_providers.len(), 1, "helper completion");
        // helper `keymap` missing → empty pairs, no keymap provider
        assert_eq!(reg.keymaps.len(), 0);
    }

    #[test]
    fn store_plugin_skips_empty_hook_commands() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(tmp.path(), "e", "name = \"e\"\n[hooks]\npre_exec = [[]]\n");
        let (plugins, _) = scan(tmp.path());
        let plugin = plugins.into_iter().next().unwrap().into_plugin();
        let mut reg = Registry::default();
        reg.install(&plugin);
        assert_eq!(reg.pre_exec.len(), 0, "empty argv never installed");
    }

    #[test]
    fn store_plugin_installs_all_seams() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(
            tmp.path(),
            "fancy",
            r#"
name = "fancy"
[theme]
name = "fancy"
prompt = "{arrow} {cwd}"
[keymap]
"ctrl-g" = "menu-next"
[completion]
commands = ["fancy"]
"#,
        );
        let (plugins, _) = scan(tmp.path());
        let plugin = plugins.into_iter().next().unwrap().into_plugin();
        let mut reg = Registry::default();
        reg.install(&plugin);
        assert_eq!(reg.themes.len(), 1);
        assert_eq!(reg.themes[0].name(), "fancy");
        assert_eq!(reg.keymaps.len(), 1);
        assert_eq!(reg.completion_providers.len(), 1);
        assert_eq!(reg.installed(), vec![("fancy".to_string(), true)]);
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
