//! Store plugin manifests and discovery (`docs/PLUGINS.md`, store
//! stages). A plugin is a directory under `plugins/` containing a
//! `plugin.toml`; the manifest declares which seams it contributes
//! (declarative data and/or helper subprocess commands).

use crate::helper;
use brish_plugin_api::{
    Completion, CompletionCtx, CompletionProvider, KeymapProvider, Plugin, PromptSegment, Registry,
    Theme,
};
use serde::{Deserialize, Serialize};
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
    /// WASM prompt segment. Interpreted, default-deny: no host imports,
    /// no fs/net/spawn. Needs a `brish` built with `--features wasm`.
    #[serde(default)]
    pub wasm: Option<WasmDecl>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WasmDecl {
    /// Module path relative to the plugin directory.
    pub path: PathBuf,
    /// Fuel per render; the guest is cut off when it runs out.
    #[serde(default = "default_wasm_fuel")]
    pub fuel: u64,
    /// Hard cap on the returned string, in bytes.
    #[serde(default = "default_wasm_max_output")]
    pub max_output: usize,
}

fn default_wasm_fuel() -> u64 {
    1_000_000
}

fn default_wasm_max_output() -> usize {
    4096
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThemeDecl {
    pub name: String,
    pub prompt: String,
    /// Right-hand prompt template (same tokens as `prompt`).
    #[serde(default)]
    pub prompt_right: Option<String>,
    /// Transient prompt template (same tokens as `prompt`).
    #[serde(default)]
    pub prompt_transient: Option<String>,
    /// Per-segment palette for powerline themes: segment_name = "bg_color".
    #[serde(default)]
    pub palette: Option<BTreeMap<String, String>>,
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

#[derive(Debug, Clone, Deserialize)]
pub struct SegmentDecl {
    pub name: String,
    pub cmd: Vec<String>,
    /// Cache TTL in seconds (0 = re-run on every prompt render).
    #[serde(default = "default_cache_secs")]
    pub cache_secs: u64,
    /// Per-run deadline in milliseconds.
    #[serde(default = "default_segment_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_cache_secs() -> u64 {
    1
}

fn default_segment_timeout_ms() -> u64 {
    500
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
#[derive(Debug, Serialize, Deserialize)]
pub struct StoreMeta {
    pub source: String,
    /// Pinned commit for git installs (head at install time).
    #[serde(default)]
    pub commit: Option<String>,
    /// True when installed through the index (commit must verify).
    #[serde(default)]
    pub index: bool,
    /// Subdirectory of the source repo holding the plugin root.
    #[serde(default)]
    pub path: Option<String>,
}

/// One discovered plugin: manifest + its directory.
pub struct Stored {
    pub dir: PathBuf,
    pub manifest: Manifest,
}

/// Plugin names become directory names — reject anything that could
/// escape `plugins/` or hide.
pub fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && !name.contains('/')
        && !name.contains('\\')
}

/// Read + validate a single `plugin.toml` (no directory-name rule —
/// install sources may use a different directory until placed).
pub fn load_manifest(dir: &Path) -> Result<Manifest, String> {
    let mpath = dir.join(MANIFEST);
    let text = std::fs::read_to_string(&mpath).map_err(|e| format!("{}: {e}", mpath.display()))?;
    let manifest: Manifest =
        toml::from_str(&text).map_err(|e| format!("{}: {e}", mpath.display()))?;
    if manifest.name.is_empty() {
        return Err(format!("{}: empty `name`", mpath.display()));
    }
    Ok(manifest)
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
        let manifest = match load_manifest(&dir) {
            Ok(m) => m,
            Err(e) => {
                warnings.push(format!("plugin store: {e}"));
                continue;
            }
        };
        let dirname = dir.file_name().and_then(|s| s.to_str()).unwrap_or_default();
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
                prompt_right: t.prompt_right.clone(),
                prompt_transient: t.prompt_transient.clone(),
                palette: t.palette.clone(),
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
    pub prompt_right: Option<String>,
    pub prompt_transient: Option<String>,
    pub palette: Option<BTreeMap<String, String>>,
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
            brish_plugin_api::color_enabled(),
            self.palette.as_ref(),
            0, // duration not available at theme render time
            brish_plugin_api::CmdDurationMode::default(),
        )
    }

    fn right_template(&self) -> Option<&str> {
        self.prompt_right.as_deref()
    }

    fn transient_template(&self) -> Option<&str> {
        self.prompt_transient.as_deref()
    }

    fn palette(&self) -> Option<&BTreeMap<String, String>> {
        self.palette.as_ref()
    }
}

// --- Helper functions for expand_template (must be defined before the closure) ---

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

/// Full cwd with ~ substitution.
fn cwd_full(cwd: &Path) -> String {
    let home = std::env::var("HOME").ok();
    let cwd_str = cwd.to_string_lossy();
    if let Some(h) = home
        && let Some(stripped) = cwd_str.strip_prefix(&h)
    {
        format!("~{stripped}")
    } else {
        cwd_str.to_string()
    }
}

/// Shortened cwd: last 2 components full, earlier → first char.
/// e.g. ~/a/b/c/d → ~/a/b/c/d (≤3 deep), ~/a/b/c/d/e → ~/a/b/c.../e
fn cwd_short(cwd: &Path) -> String {
    let full = cwd_full(cwd);
    let parts: Vec<&str> = full.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() <= 3 {
        return full;
    }
    // First (len - 2) components collapse to their first character; the
    // last two stay whole — enough to place the directory, short enough
    // to leave room for the segments.
    let lead = if full.starts_with('~') { "~" } else { "/" };
    let keep_full = parts.len() - 2;
    let mut out = String::from(lead);
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push('/');
        }
        if i < keep_full {
            out.push(part.chars().next().unwrap_or('?'));
        } else {
            out.push_str(part);
        }
    }
    out
}

/// Local time HH:MM:SS via chrono (brish-platform).
fn localtime_hms() -> Option<String> {
    brish_platform::time::localtime_hms().map(|(h, m, s)| format!("{h:02}:{m:02}:{s:02}"))
}

/// Local time HH:MM via chrono (brish-platform).
fn localtime_hm() -> Option<String> {
    brish_platform::time::localtime_hm().map(|(h, m)| format!("{h:02}:{m:02}"))
}

/// Format duration: wall="2.3s", cpu="1.8s cpu".
fn format_duration(ms: u128, _mode: brish_plugin_api::CmdDurationMode) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let mins = ms / 60_000;
        let secs = (ms % 60_000) / 1000;
        format!("{mins}m{secs}s")
    }
}

/// Powerline separator: U+E0B0 with fg=last_bg, bg=next_bg (or default).
fn separator(color: bool, last_bg: Option<String>, next_bg: Option<&str>) -> String {
    if !color {
        return String::new();
    }
    let sep = "\u{e0b0}"; // 
    let fg = last_bg.unwrap_or_else(|| "0".to_string()); // default fg = black
    let bg = next_bg.unwrap_or("0"); // default bg = black
    format!("\x1b[38;5;{fg}m\x1b[48;5;{bg}m{sep}\x1b[0m")
}

/// ANSI code for `red`/`bright-blue`/`#rrggbb`; `None` = unknown name.
fn color_code(spec: &str, fg: bool) -> Option<String> {
    // 256-color index: `{bg:11}` / `{fg:208}`. Needed for powerline
    // palettes, where a segment's exact background is the design.
    if let Ok(n) = spec.parse::<u16>()
        && (0..=255).contains(&n)
    {
        return Some(format!("\x1b[{};5;{n}m", if fg { 38 } else { 48 }));
    }
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

/// Tokens: `{arrow}` `{cwd}` `{path}` `{path:short}` `{home}` `{user}` `{host}`
/// `{context}` `{time}` `{time:short}` `{status}` `{status:all}` `{version}`
/// `{segments}` `{segments:plain}` `{segment:name}` `{segmentc:name}`
/// `{cmd_duration}` `{reset}` `{fg:…}` `{bg:…}` `{sep}` `{sep:NEXT}`
/// Conditionals: `{if:TOKEN}...{endif}` `{if:TOKEN}...{else}...{endif}`
/// (named 8+bright or `#rrggbb`). Unknown tokens and unmatched braces
/// pass through literally.
#[allow(clippy::too_many_arguments)]
pub fn expand_template(
    tmpl: &str,
    status: i32,
    cwd: &Path,
    segments: &[&dyn PromptSegment],
    color: bool,
    palette: Option<&BTreeMap<String, String>>,
    duration_ms: u128,
    duration_mode: brish_plugin_api::CmdDurationMode,
) -> String {
    let mut out = String::new();
    let mut rest = tmpl;

    // Separator state: tracks the last {bg:SPEC} for {sep} rendering.
    let mut last_bg: Option<String> = None;
    // Conditional stack: one frame per open `{if:}`.
    struct IfFrame {
        condition_met: bool,
        in_else: bool,
    }
    let mut if_stack: Vec<IfFrame> = Vec::new();
    const MAX_IF_DEPTH: usize = 8;

    // Pre-compute segment outputs for {segment:name} and {segmentc:name}
    let segment_plain: Vec<(&str, String)> = segments
        .iter()
        .filter_map(|s| s.render_plain(status, cwd).map(|v| (s.name(), v)))
        .collect();
    let segment_colored: Vec<(&str, String)> = segments
        .iter()
        .filter_map(|s| s.render_colored(status, cwd).map(|v| (s.name(), v)))
        .collect();

    // Helper to check if a token would produce non-empty output (for conditionals)
    let token_truthy = |token: &str| -> bool {
        match token {
            "context" => {
                std::env::var_os("SSH_CONNECTION").is_some()
                    || std::env::var_os("SSH_TTY").is_some()
            }
            "status" => status != 0,
            "status:all" => true,
            "time" | "time:short" => true,
            "user" => std::env::var_os("USER").is_some(),
            "host" => std::env::var_os("HOSTNAME").is_some(),
            "path" | "path:short" | "home" | "cwd" => true,
            "arrow" => true,
            "version" => true,
            "cmd_duration" => true,
            "segments" | "segments:plain" => {
                !segment_plain.is_empty() || !segment_colored.is_empty()
            }
            t if t.starts_with("segment:") => {
                let name = &t["segment:".len()..];
                segment_plain
                    .iter()
                    .any(|(n, v)| *n == name && !v.is_empty())
            }
            t if t.starts_with("segmentc:") => {
                let name = &t["segmentc:".len()..];
                segment_colored
                    .iter()
                    .any(|(n, v)| *n == name && !v.is_empty())
            }
            _ => false,
        }
    };

    // Helper to expand a single token to its string value
    let expand_token = |token: &str, color: bool, last_bg: &mut Option<String>| -> String {
        // Check for palette lookup first: {palette:name}
        if let Some(palette) = palette
            && let Some(name) = token.strip_prefix("palette:")
        {
            return palette.get(name).cloned().unwrap_or_default();
        }
        match token {
            "arrow" => arrow(status, color),
            "cwd" => cwd_base(cwd),
            "path" => cwd_full(cwd),
            "path:short" => cwd_short(cwd),
            "home" => std::env::var("HOME").unwrap_or_default(),
            "user" => std::env::var("USER").unwrap_or_else(|_| "user".into()),
            "host" => std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".into()),
            "context" => {
                if std::env::var_os("SSH_CONNECTION").is_some()
                    || std::env::var_os("SSH_TTY").is_some()
                {
                    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
                    let host = std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".into());
                    format!("{user}@{host}")
                } else {
                    String::new()
                }
            }
            "time" => localtime_hms().unwrap_or_else(|| "??:??:??".into()),
            "time:short" => localtime_hm().unwrap_or_else(|| "??:??".into()),
            "status" => {
                if status != 0 {
                    status.to_string()
                } else {
                    String::new()
                }
            }
            "status:all" => status.to_string(),
            "version" => env!("CARGO_PKG_VERSION").to_string(),
            "segments" => segment_colored
                .iter()
                .map(|(_, v)| v.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            "segments:plain" => segment_plain
                .iter()
                .map(|(_, v)| v.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            "cmd_duration" => format_duration(duration_ms, duration_mode),
            "reset" => {
                *last_bg = None;
                if color {
                    "\x1b[0m".to_string()
                } else {
                    String::new()
                }
            }
            "sep" => separator(color, last_bg.clone(), None),
            t if t.starts_with("sepp:") => {
                // `{sepp:NAME}` — separator into a palette entry. `{sep:…}`
                // takes a colour spec, which a palette name is not.
                let key = &t["sepp:".len()..];
                let next = palette.and_then(|p| p.get(key)).map(String::as_str);
                separator(color, last_bg.clone(), next)
            }
            t if t.starts_with("sep:") => {
                let next_bg = &t["sep:".len()..];
                separator(color, last_bg.clone(), Some(next_bg))
            }
            t if t.starts_with("segment:") => {
                let name = &t["segment:".len()..];
                segment_plain
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            }
            t if t.starts_with("segmentc:") => {
                let name = &t["segmentc:".len()..];
                segment_colored
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            }
            // `{bgp:NAME}` / `{fgp:NAME}` — palette lookup. Braces do not
            // nest, so a colour cannot be interpolated into `{bg:…}`; these
            // are the form a palette-driven theme actually needs.
            t if t.starts_with("bgp:") => {
                let key = &t["bgp:".len()..];
                match palette.and_then(|p| p.get(key)) {
                    Some(spec) => {
                        // Track it like `{bg:}` so a trailing `{sep}` chains.
                        *last_bg = Some(spec.clone());
                        styled(spec, false, color).unwrap_or_default()
                    }
                    None => format!("{{{t}}}"),
                }
            }
            t if t.starts_with("fgp:") => palette
                .and_then(|p| p.get(&t["fgp:".len()..]))
                .and_then(|spec| styled(spec, true, color))
                .unwrap_or_else(|| format!("{{{t}}}")),
            t => t
                .strip_prefix("fg:")
                .and_then(|spec| styled(spec, true, color))
                .or_else(|| {
                    t.strip_prefix("bg:").and_then(|spec| {
                        let code = styled(spec, false, color);
                        if code.is_some() {
                            // Track background for separator state
                            *last_bg = Some(spec.to_string());
                        }
                        code
                    })
                })
                .unwrap_or_else(|| {
                    // Unknown token: pass through literally
                    format!("{{{token}}}")
                }),
        }
    };

    // Is every enclosing `{if:}` branch currently active? Literal text is
    // gated too, not just tokens — otherwise a skipped branch still leaks
    // its static characters.
    let rendering = |stack: &[IfFrame]| {
        stack.iter().all(|f| {
            if f.in_else {
                !f.condition_met
            } else {
                f.condition_met
            }
        })
    };

    while let Some(open) = rest.find('{') {
        if rendering(&if_stack) {
            out.push_str(&rest[..open]);
        }
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            if rendering(&if_stack) {
                out.push_str(&rest[open..]);
            }
            return out;
        };
        let token = &after[..close];

        // Handle conditionals
        if let Some(cond) = token.strip_prefix("if:") {
            if if_stack.len() >= MAX_IF_DEPTH {
                // Depth exceeded: treat as literal
                out.push('{');
                out.push_str(token);
                out.push('}');
            } else {
                let met = token_truthy(cond);
                if_stack.push(IfFrame {
                    condition_met: met,
                    in_else: false,
                });
            }
        } else if token == "else" {
            match if_stack.last_mut() {
                Some(frame) => frame.in_else = true,
                None => {
                    // Unmatched else: literal
                    out.push('{');
                    out.push_str(token);
                    out.push('}');
                }
            }
        } else if token == "endif" {
            if_stack.pop();
        } else if rendering(&if_stack) {
            out.push_str(&expand_token(token, color, &mut last_bg));
        }

        rest = &after[close + 1..];
    }
    if rendering(&if_stack) {
        out.push_str(rest);
    }
    out
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
            timeout_ms: default_segment_timeout_ms(),
        };
        assert_eq!(seg.cache_secs, 1);
        assert_eq!(seg.timeout_ms, 500);
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
        expand_template(
            s,
            status,
            cwd,
            &[&Dash],
            color,
            None,
            0,
            brish_plugin_api::CmdDurationMode::default(),
        )
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
    fn path_and_status_tokens() {
        let cwd = Path::new("/home/u/deep/nest/proj");
        let out = expand_template(
            "{path}|{path:short}",
            0,
            cwd,
            &[],
            false,
            None,
            0,
            brish_plugin_api::CmdDurationMode::default(),
        );
        assert_eq!(out, "/home/u/deep/nest/proj|/h/u/d/nest/proj", "{out:?}");
        // shallow paths stay whole
        let shallow = Path::new("/home/u/proj");
        assert_eq!(
            expand_template(
                "{path:short}",
                0,
                shallow,
                &[],
                false,
                None,
                0,
                brish_plugin_api::CmdDurationMode::default(),
            ),
            "/home/u/proj"
        );
        // status: empty on success, code on failure
        assert_eq!(tmpl("[{status}]", 0, false), "[]");
        assert_eq!(tmpl("[{status}]", 7, false), "[7]");
        assert_eq!(tmpl("[{status:all}]", 0, false), "[0]");
    }

    #[test]
    fn conditionals_select_and_skip() {
        // {if:status} is false on success, true on failure.
        assert_eq!(tmpl("{if:status}FAIL{endif}OK", 0, false), "OK");
        assert_eq!(tmpl("{if:status}FAIL{endif}OK", 1, false), "FAILOK");
        // else branch
        assert_eq!(tmpl("{if:status}F{else}OK{endif}", 0, false), "OK");
        assert_eq!(tmpl("{if:status}F{else}OK{endif}", 1, false), "F");
        // unknown condition is false, not a crash
        assert_eq!(tmpl("{if:bogus}x{endif}y", 0, false), "y");
        // unmatched else stays literal
        assert_eq!(tmpl("a{else}", 0, false), "a{else}");
        // a stray {endif} opens nothing and renders nothing
        assert_eq!(tmpl("a{endif}", 0, false), "a");
    }

    #[test]
    fn conditional_depth_is_capped() {
        // 9 nested ifs exceeds MAX_IF_DEPTH (8): the 9th stays literal.
        let deep = format!("{}{}{}", "{if:status}".repeat(9), "x", "{endif}".repeat(9));
        let out = tmpl(&deep, 1, false);
        assert!(out.ends_with("x"), "{out:?}");
        assert!(out.contains("{if:status}"), "ninth if is literal: {out:?}");
    }

    #[test]
    fn separator_tracks_the_last_bg() {
        // {sep} after {bg:11}: fg=11, bg=default(0).
        let out = tmpl("{bg:11}x{sep}", 0, true);
        assert_eq!(
            out, "\x1b[48;5;11mx\x1b[38;5;11m\x1b[48;5;0m\u{e0b0}\x1b[0m",
            "{out:?}"
        );
        // {sep:NEXT} sets the next background too.
        let out = tmpl("{bg:11}x{sep:5}", 0, true);
        assert_eq!(
            out, "\x1b[48;5;11mx\x1b[38;5;11m\x1b[48;5;5m\u{e0b0}\x1b[0m",
            "{out:?}"
        );
        // {reset} clears the state, so {sep} falls back to fg=0.
        let out = tmpl("{bg:11}x{reset}{sep}", 0, true);
        assert!(
            out.ends_with("\x1b[38;5;0m\x1b[48;5;0m\u{e0b0}\x1b[0m"),
            "{out:?}"
        );
        // NO_COLOR collapses separators to nothing.
        assert_eq!(tmpl("{bg:11}x{sep}", 0, false), "x");
    }

    #[test]
    fn named_segments_and_palette_lookup() {
        struct Named(&'static str);
        impl PromptSegment for Named {
            fn name(&self) -> &str {
                self.0
            }
            fn render_colored(&self, _s: i32, _c: &Path) -> Option<String> {
                Some("\x1b[33mgit:(main)\x1b[0m".into())
            }
        }
        let seg = Named("git");
        let mut palette = BTreeMap::new();
        palette.insert("git".to_string(), "11".to_string());
        let cwd = Path::new("/x");
        let expand = |t: &str, color: bool| {
            expand_template(
                t,
                0,
                cwd,
                &[&seg],
                color,
                Some(&palette),
                0,
                brish_plugin_api::CmdDurationMode::default(),
            )
        };
        let named = expand("{segment:git}|{segmentc:git}|{segment:missing}", false);
        assert_eq!(named, "git:(main)|\x1b[33mgit:(main)\x1b[0m|", "{named:?}");
        // `{bgp:}` resolves the palette and feeds the separator state, so a
        // trailing `{sep}` chains off it.
        let out = expand("{bgp:git}x{sep}", true);
        assert_eq!(
            out, "\x1b[48;5;11mx\x1b[38;5;11m\x1b[48;5;0m\u{e0b0}\x1b[0m",
            "{out:?}"
        );
        // `{sepp:}` names the next block by palette key.
        let out = expand("{bgp:git}x{sepp:missing}", true);
        assert!(
            out.contains("\x1b[38;5;11m\x1b[48;5;0m"),
            "unknown key degrades to default bg: {out:?}"
        );
        // NO_COLOR: palette colours collapse like every other colour token.
        assert_eq!(expand("{bgp:git}x", false), "x");
        // Unknown palette keys stay literal, never silently blank.
        assert_eq!(expand("{bgp:nope}", false), "{bgp:nope}");
    }

    #[test]
    fn duration_formatting() {
        let f = |ms| {
            expand_template(
                "{cmd_duration}",
                0,
                Path::new("/x"),
                &[],
                false,
                None,
                ms,
                brish_plugin_api::CmdDurationMode::default(),
            )
        };
        assert_eq!(f(0), "0ms");
        assert_eq!(f(999), "999ms");
        assert_eq!(f(1_500), "1.5s");
        assert_eq!(f(90_000), "1m30s");
    }

    #[test]
    fn time_tokens_have_hh_mm_shape() {
        for (t, want_len) in [("{time}", 8), ("{time:short}", 5)] {
            let body = tmpl(t, 0, false);
            assert_eq!(body.len(), want_len, "{t}: {body:?}");
            let digits: Vec<char> = body.chars().filter(|c| *c != ':').collect();
            assert!(digits.iter().all(|c| c.is_ascii_digit()), "{t}: {body:?}");
            assert!(body.contains(':'), "{t}: {body:?}");
        }
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
            algorithm: brish_plugin_api::Algorithm::Prefix,
            match_description: false,
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
            algorithm: brish_plugin_api::Algorithm::Prefix,
            match_description: false,
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

    #[test]
    fn example_plugins_parse() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins");
        let mut seen = 0;
        for e in std::fs::read_dir(&root).unwrap() {
            let dir = e.unwrap().path();
            if !dir.join("plugin.toml").exists() {
                continue;
            }
            let m = load_manifest(&dir).unwrap();
            assert_eq!(m.name, dir.file_name().unwrap().to_str().unwrap());
            seen += 1;
        }
        assert!(seen >= 2, "starter + sentinel examples expected");
    }

    #[test]
    fn powerline_example_expands_to_a_two_line_prompt() {
        // The p10k-style example is the only shipped theme that leans on
        // conditionals, separators and per-segment blocks at once, so a
        // silent regression in any of the three shows up here.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/powerline");
        let m = load_manifest(&dir).expect("powerline example manifest");
        let t = m.theme.expect("powerline declares a theme");
        assert_eq!(t.name, "powerline");
        assert_eq!(t.prompt_right.as_deref(), Some("{fg:bright-black}{time} "));
        assert_eq!(t.prompt_transient.as_deref(), Some("{arrow} "));
        let pal = t.palette.as_ref().expect("powerline declares a palette");
        for seg in ["git", "venv", "aws", "kubectx", "docker"] {
            assert!(pal.contains_key(seg), "palette missing {seg}");
        }

        struct Git;
        impl PromptSegment for Git {
            fn name(&self) -> &str {
                "git"
            }
            fn render_colored(&self, _s: i32, _c: &Path) -> Option<String> {
                Some("\x1b[33mgit:(main)\x1b[0m".into())
            }
        }
        let cwd = Path::new("/home/u/deep/nest/proj");
        let plain = expand_template(
            &t.prompt,
            0,
            cwd,
            &[&Git],
            false,
            t.palette.as_ref(),
            0,
            brish_plugin_api::CmdDurationMode::default(),
        );
        let lines: Vec<&str> = plain.lines().collect();
        assert_eq!(lines.len(), 2, "two-line prompt: {plain:?}");
        // cwd shortened, git block present, arrow on the second line
        assert!(lines[0].contains("/h/u/d/nest/proj"), "{plain:?}");
        assert!(lines[0].contains("git:(main)"), "{plain:?}");
        assert!(
            !lines[0].contains('\u{1b}'),
            "plain text under NO_COLOR: {plain:?}"
        );
        assert_eq!(lines[1], "\u{279c} ");

        // Colors on: blocks and separators are real SGR sequences.
        let colored = expand_template(
            &t.prompt,
            0,
            cwd,
            &[&Git],
            true,
            t.palette.as_ref(),
            0,
            brish_plugin_api::CmdDurationMode::default(),
        );
        // Colours come from the palette, not hardcoded in the template.
        let p = t.palette.as_ref().unwrap();
        assert!(
            colored.contains(&format!("\x1b[48;5;{}m", p["cwd"])),
            "cwd block: {colored:?}"
        );
        assert!(
            colored.contains(&format!("\x1b[48;5;{}m", p["git"])),
            "git block: {colored:?}"
        );
        assert!(colored.contains('\u{e0b0}'), "separators: {colored:?}");

        // Transient prompt collapses to the arrow alone.
        let transient = t.prompt_transient.unwrap_or_default();
        assert_eq!(tmpl(&transient, 0, false), "\u{279c} ");
    }
}
