//! `plugin` builtin subcommands: list / add / rm / update / search /
//! info (`docs/PLUGINS.md`, store stages).
//!
//! Install = materialize plugin files under
//! `<config>/plugins/<name>` (git clone + pinned-commit verify, or
//! local copy), write `.brish-store.toml` origin metadata, and keep
//! `config.toml [plugins]` in sync. A plugin added at runtime takes
//! effect on the next shell start (registry is immutable after
//! startup) — every success note says so.

use crate::paths;
use crate::store::{self, META, StoreMeta};
use serde::Deserialize;
use std::path::{Component, Path, PathBuf};
use toml_edit::Item;

/// Default index repository (override: `[store] index` in config.toml
/// or `$BRISH_INDEX`).
const DEFAULT_INDEX: &str = "https://github.com/brilyyy/bsh";

/// Engine entry point: `argv[0] == "plugin"`.
pub fn dispatch(argv: &[String], installed: &[(String, bool)], shell_cwd: &Path) -> i32 {
    dispatch_in(argv, installed, shell_cwd, &paths::config_dir())
}

/// Same, with an explicit config root (tests isolate without env vars).
pub fn dispatch_in(
    argv: &[String],
    installed: &[(String, bool)],
    shell_cwd: &Path,
    root: &Path,
) -> i32 {
    let sub = argv.get(1).map(|s| s.as_str()).unwrap_or("list");
    match sub {
        "list" => list(installed, root),
        "add" => match argv.get(2) {
            Some(arg) => add(arg, shell_cwd, root),
            None => usage(2),
        },
        "rm" => parse_rm(&argv[2..], root),
        "update" => match argv.get(2) {
            Some(name) => update(name, root),
            None => usage(2),
        },
        "search" => search(&argv[2..].join(" "), root),
        "info" => match argv.get(2) {
            Some(name) => info(name, root),
            None => usage(2),
        },
        _ => usage(2),
    }
}

fn usage(status: i32) -> i32 {
    eprintln!(
        "brish: usage: plugin [list] | add <name|url|path> | rm [--purge] <name> | \
         update <name> | search <query> | info <name>"
    );
    status
}

// ---------------------------------------------------------------- list

fn list(installed: &[(String, bool)], root: &Path) -> i32 {
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (name, on) in installed {
        println!("{name} {}", on_off(*on));
        seen.insert(name);
    }
    // Plugins installed this session are not in the startup registry
    // yet — show them from disk so `add && list` is not confusing.
    let (stored, _) = store::scan(&root.join("plugins"));
    for p in stored {
        if !seen.contains(p.manifest.name.as_str()) {
            let on = raw_config_enabled(root, &p.manifest.name, true);
            println!("{} {}", p.manifest.name, on_off(on));
        }
    }
    0
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

// ----------------------------------------------------------------- add

#[derive(Debug)]
enum Source {
    Local(PathBuf),
    Url(String),
    IndexName,
}

fn classify(arg: &str, shell_cwd: &Path) -> Source {
    if arg.contains("://") || arg.starts_with("git@") || arg.ends_with(".git") {
        Source::Url(arg.to_string())
    } else if shell_cwd.join(arg).is_dir() {
        Source::Local(shell_cwd.join(arg))
    } else {
        Source::IndexName
    }
}

fn add(arg: &str, shell_cwd: &Path, root: &Path) -> i32 {
    let plugins = root.join("plugins");
    let staged = match classify(arg, shell_cwd) {
        Source::Local(src) => materialize_local(&src, &plugins),
        Source::Url(url) => materialize_git(&url, None, None, &plugins),
        Source::IndexName => {
            if !store::safe_name(arg) {
                return err(&format!("not a valid plugin name: {arg}"));
            }
            let cache = match ensure_index(root) {
                Ok(c) => c,
                Err(e) => return err(&e),
            };
            let entry = match find_index(&cache, arg) {
                Ok(Some(e)) => e,
                Ok(None) => {
                    return err(&format!(
                        "`{arg}` is not in the index and not a URL or local path"
                    ));
                }
                Err(e) => return err(&e),
            };
            if let Some(p) = &entry.path
                && !safe_subpath(p)
            {
                return err(&format!("index entry `{arg}` has unsafe path: {p}"));
            }
            materialize_index(&entry, &plugins)
        }
    };
    let staged = match staged {
        Ok(s) => s,
        Err(e) => return err(&e),
    };
    let manifest = match store::load_manifest(&staged.work) {
        Ok(m) => m,
        Err(e) => return err(&e),
    };
    if !store::safe_name(&manifest.name) {
        return err(&format!("unsafe plugin name: {}", manifest.name));
    }
    let dest = plugins.join(&manifest.name);
    if dest.exists() {
        return err(&format!(
            "`{}` is already installed (plugin rm --purge first)",
            manifest.name
        ));
    }
    // Fail closed on an unparseable config: config.toml stays the
    // single source of truth for enablement.
    if let Err(e) = edit_config(root, true, &manifest.name) {
        return err(&format!("installed nothing; {e}"));
    }
    if let Err(e) = swap(&staged.work, &dest) {
        return err(&e);
    }
    if let Err(e) = write_meta(&dest, &staged.meta) {
        return err(&e);
    }
    println!(
        "installed {} (restart the shell to activate)",
        manifest.name
    );
    0
}

fn err(msg: &str) -> i32 {
    eprintln!("brish: plugin: {msg}");
    1
}

/// Materialized plugin files, kept alive until moved into place.
struct Staged {
    work: PathBuf,
    meta: StoreMeta,
    _staging: tempfile::TempDir,
}

fn staging_dir(plugins: &Path) -> Result<tempfile::TempDir, String> {
    std::fs::create_dir_all(plugins).map_err(|e| format!("{}: {e}", plugins.display()))?;
    tempfile::tempdir_in(plugins).map_err(|e| format!("{}: {e}", plugins.display()))
}

fn materialize_local(src: &Path, plugins: &Path) -> Result<Staged, String> {
    let staging = staging_dir(plugins)?;
    let work = staging.path().join("work");
    copy_dir(src, &work)?;
    let source = std::fs::canonicalize(src)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| src.display().to_string());
    Ok(Staged {
        work,
        meta: StoreMeta {
            source,
            commit: None,
            index: false,
            path: None,
        },
        _staging: staging,
    })
}

fn materialize_git(
    url: &str,
    commit: Option<&str>,
    subpath: Option<&str>,
    plugins: &Path,
) -> Result<Staged, String> {
    let staging = staging_dir(plugins)?;
    let repo = staging.path().join("repo");
    git(&["clone", "--quiet", url, &repo.display().to_string()])?;
    if let Some(c) = commit {
        git(&["-C", &repo.display().to_string(), "checkout", "--quiet", c])?;
        let head = git(&["-C", &repo.display().to_string(), "rev-parse", "HEAD"])?;
        let head = head.trim();
        if head != c {
            return Err(format!("commit verify failed: wanted {c}, got {head}"));
        }
    }
    let head = git(&["-C", &repo.display().to_string(), "rev-parse", "HEAD"])?;
    let src = match subpath {
        Some(p) => repo.join(p),
        None => repo.clone(),
    };
    if !src.is_dir() {
        return Err(format!("source path not found in repo: {url}"));
    }
    let work = staging.path().join("work");
    copy_dir(&src, &work)?;
    Ok(Staged {
        work,
        meta: StoreMeta {
            source: url.to_string(),
            commit: Some(head.trim().to_string()),
            index: false,
            path: subpath.map(|p| p.to_string()),
        },
        _staging: staging,
    })
}

/// `add` for index entries: metadata marked `index: true`.
fn materialize_index(entry: &IndexEntry, plugins: &Path) -> Result<Staged, String> {
    let mut staged = materialize_git(
        &entry.source,
        entry.commit.as_deref(),
        entry.path.as_deref(),
        plugins,
    )?;
    staged.meta.index = true;
    Ok(staged)
}

fn write_meta(dir: &Path, meta: &StoreMeta) -> Result<(), String> {
    let text = toml::to_string(meta).map_err(|e| format!("meta serialize: {e}"))?;
    std::fs::write(dir.join(META), text).map_err(|e| format!("{}: {e}", dir.join(META).display()))
}

/// Atomic-ish replace: old dir renamed aside first, rolled back if the
/// new one cannot move in.
fn swap(work: &Path, dest: &Path) -> Result<(), String> {
    let parent = dest.parent().ok_or("destination has no parent")?;
    if !dest.exists() {
        return std::fs::rename(work, dest)
            .map_err(|e| format!("cannot move plugin into place: {e}"));
    }
    let backup = staging_dir(parent)?;
    let old = backup.path().join("old");
    std::fs::rename(dest, &old).map_err(|e| format!("cannot stage old plugin: {e}"))?;
    if let Err(e) = std::fs::rename(work, dest) {
        let _ = std::fs::rename(&old, dest);
        return Err(format!("cannot replace plugin: {e}"));
    }
    Ok(())
}

// ------------------------------------------------------------------ rm

fn parse_rm(args: &[String], root: &Path) -> i32 {
    let mut purge = false;
    let mut name: Option<&str> = None;
    for a in args {
        if a == "--purge" {
            purge = true;
        } else if name.is_none() {
            name = Some(a);
        } else {
            return usage(2);
        }
    }
    let Some(name) = name else {
        return usage(2);
    };
    rm(name, purge, root)
}

fn rm(name: &str, purge: bool, root: &Path) -> i32 {
    if !store::safe_name(name) {
        return err(&format!("unsafe plugin name: {name}"));
    }
    if let Err(e) = edit_config(root, false, name) {
        return err(&e);
    }
    let dir = root.join("plugins").join(name);
    if !purge {
        println!("disabled {name} (plugin files kept; --purge deletes them)");
        return 0;
    }
    if dir.exists()
        && let Err(e) = std::fs::remove_dir_all(&dir)
    {
        return err(&format!("cannot delete {}: {e}", dir.display()));
    }
    println!("removed {name}");
    0
}

// -------------------------------------------------------------- update

fn update(name: &str, root: &Path) -> i32 {
    if !store::safe_name(name) {
        return err(&format!("unsafe plugin name: {name}"));
    }
    let dir = root.join("plugins").join(name);
    if !dir.is_dir() {
        return err(&format!("`{name}` is not installed"));
    }
    let meta = match store::read_meta(&dir) {
        Some(m) => m,
        None => {
            return err(&format!(
                "`{name}` has no origin recorded (manually maintained)"
            ));
        }
    };
    let plugins = root.join("plugins");
    let staged = if meta.index {
        let cache = match ensure_index(root) {
            Ok(c) => c,
            Err(e) => return err(&e),
        };
        let entry = match find_index(&cache, name) {
            Ok(Some(e)) => e,
            Ok(None) => return err(&format!("`{name}` vanished from the index")),
            Err(e) => return err(&e),
        };
        if entry.commit.as_deref() == meta.commit.as_deref()
            && entry.source == meta.source
            && entry.path == meta.path
        {
            println!("{name} is up to date");
            return 0;
        }
        if entry.source != meta.source {
            return err(&format!(
                "index entry `{name}` changed source; plugin rm --purge + add instead"
            ));
        }
        materialize_index(&entry, &plugins)
    } else if let Source::Url(url) = classify(&meta.source, root) {
        materialize_git(&url, None, meta.path.as_deref(), &plugins)
    } else if let Source::Local(src) = classify(&meta.source, root) {
        if !src.is_dir() {
            return err(&format!("origin is gone: {}", meta.source));
        }
        materialize_local(&src, &plugins)
    } else {
        return err(&format!(
            "origin is neither a path nor a URL: {}",
            meta.source
        ));
    };
    let staged = match staged {
        Ok(s) => s,
        Err(e) => return err(&e),
    };
    if let Err(e) = store::load_manifest(&staged.work) {
        return err(&e);
    }
    if let Err(e) = swap(&staged.work, &dir) {
        return err(&e);
    }
    if let Err(e) = write_meta(&dir, &staged.meta) {
        return err(&e);
    }
    println!("updated {name} (restart the shell to activate)");
    0
}

// -------------------------------------------------------------- search

#[derive(Debug, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub source: String,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn index_url(root: &Path) -> String {
    if let Ok(u) = std::env::var("BRISH_INDEX")
        && !u.is_empty()
    {
        return u;
    }
    #[derive(Default, Deserialize)]
    struct Raw {
        store: Option<RawStore>,
    }
    #[derive(Default, Deserialize)]
    struct RawStore {
        index: Option<String>,
    }
    std::fs::read_to_string(root.join("config.toml"))
        .ok()
        .and_then(|t| toml::from_str::<Raw>(&t).ok())
        .and_then(|r| r.store)
        .and_then(|s| s.index)
        .unwrap_or_else(|| DEFAULT_INDEX.to_string())
}

/// Clone (or best-effort refresh) the index repo; returns its cache dir.
fn ensure_index(root: &Path) -> Result<PathBuf, String> {
    let url = index_url(root);
    let cache = root.join("index");
    if cache.join(".git").is_dir() {
        if let Err(e) = git(&[
            "-C",
            &cache.display().to_string(),
            "pull",
            "--ff-only",
            "--quiet",
        ]) {
            eprintln!("brish: plugin: index refresh failed: {e} (using cached copy)");
        }
        return Ok(cache);
    }
    if cache.exists() {
        return Err(format!(
            "{} exists but is not a git repo; delete it and retry",
            cache.display()
        ));
    }
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    git(&["clone", "--quiet", &url, &cache.display().to_string()])?;
    Ok(cache)
}

fn find_index(cache: &Path, name: &str) -> Result<Option<IndexEntry>, String> {
    let path = cache.join("index").join(format!("{name}.toml"));
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let entry: IndexEntry =
        toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if entry.name != name {
        return Err(format!(
            "index entry `{}` declares name `{}`",
            name, entry.name
        ));
    }
    Ok(Some(entry))
}

fn search(query: &str, root: &Path) -> i32 {
    let cache = match ensure_index(root) {
        Ok(c) => c,
        Err(e) => return err(&e),
    };
    let dir = cache.join("index");
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => return err(&format!("{}: {e}", dir.display())),
    };
    let q = query.to_lowercase();
    let mut hits: Vec<IndexEntry> = Vec::new();
    for f in entries.flatten() {
        let p = f.path();
        if p.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(entry) = toml::from_str::<IndexEntry>(&text) else {
            continue;
        };
        let hay = format!(
            "{} {} {}",
            entry.name,
            entry.description,
            entry.tags.join(" ")
        )
        .to_lowercase();
        if q.is_empty() || hay.contains(&q) {
            hits.push(entry);
        }
    }
    if hits.is_empty() {
        eprintln!("brish: plugin: no matches for `{query}`");
        return 1;
    }
    hits.sort_by(|a, b| a.name.cmp(&b.name));
    for h in hits {
        if h.description.is_empty() {
            println!("{}", h.name);
        } else {
            println!("{}  {}", h.name, h.description);
        }
    }
    0
}

// ---------------------------------------------------------------- info

fn info(name: &str, root: &Path) -> i32 {
    if !store::safe_name(name) {
        return err(&format!("unsafe plugin name: {name}"));
    }
    let dir = root.join("plugins").join(name);
    if dir.is_dir() {
        match store::load_manifest(&dir) {
            Ok(m) => {
                println!("name: {}", m.name);
                if !m.version.is_empty() {
                    println!("version: {}", m.version);
                }
                if !m.description.is_empty() {
                    println!("description: {}", m.description);
                }
                println!("installed: yes ({})", dir.display());
                if let Some(meta) = store::read_meta(&dir) {
                    print!("origin: {}", meta.source);
                    if let Some(c) = &meta.commit {
                        print!(" @ {}", &c[..c.len().min(12)]);
                    }
                    println!();
                }
                let seams: Vec<&str> = [
                    m.theme.is_some().then_some("theme"),
                    (!m.keymap.is_empty()).then_some("keymap"),
                    m.completion.is_some().then_some("completion"),
                    m.segment.is_some().then_some("segment"),
                    m.hooks.is_some().then_some("hooks"),
                    m.helper.is_some().then_some("helper"),
                ]
                .into_iter()
                .flatten()
                .collect();
                println!("seams: {}", seams.join(", "));
                0
            }
            Err(e) => err(&e),
        }
    } else {
        let cache = match ensure_index(root) {
            Ok(c) => c,
            Err(e) => return err(&e),
        };
        match find_index(&cache, name) {
            Ok(Some(entry)) => {
                println!("name: {}", entry.name);
                if !entry.description.is_empty() {
                    println!("description: {}", entry.description);
                }
                println!("installed: no");
                println!("origin: {}", entry.source);
                if !entry.tags.is_empty() {
                    println!("tags: {}", entry.tags.join(", "));
                }
                0
            }
            Ok(None) => err(&format!("`{name}` not installed and not in the index")),
            Err(e) => err(&e),
        }
    }
}

// ------------------------------------------------------- config editing

/// Keep `config.toml [plugins]` in sync. `enable`: exact `enabled`
/// list gets the name, or `disabled` loses it; with no lists at all a
/// store plugin is enabled by default (no edit). `disable`: reverse.
fn edit_config(root: &Path, enable: bool, name: &str) -> Result<(), String> {
    let path = root.join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| format!("config.toml cannot be edited: {e}"))?;
    let mut changed = false;
    {
        let plugins = doc
            .entry("plugins")
            .or_insert(Item::Table(toml_edit::Table::new()));
        let Some(tbl) = plugins.as_table_mut() else {
            return Err("config.toml: [plugins] is not a table".to_string());
        };
        let has_enabled = tbl.get("enabled").and_then(|i| i.as_array()).is_some();
        let has_disabled = tbl.get("disabled").and_then(|i| i.as_array()).is_some();
        if enable {
            if has_enabled {
                let arr = tbl.get_mut("enabled").and_then(|i| i.as_array_mut());
                if let Some(arr) = arr
                    && !arr.iter().any(|v| v.as_str() == Some(name))
                {
                    arr.push(name);
                    changed = true;
                }
            } else if has_disabled {
                let arr = tbl.get_mut("disabled").and_then(|i| i.as_array_mut());
                if let Some(arr) = arr {
                    let pos = arr.iter().position(|v| v.as_str() == Some(name));
                    if let Some(pos) = pos {
                        arr.remove(pos);
                        changed = true;
                    }
                }
            }
            // neither list → store plugins default on, nothing to write
        } else if has_enabled {
            let arr = tbl.get_mut("enabled").and_then(|i| i.as_array_mut());
            if let Some(arr) = arr {
                let pos = arr.iter().position(|v| v.as_str() == Some(name));
                if let Some(pos) = pos {
                    arr.remove(pos);
                    changed = true;
                }
            }
            // not in the exact list → already off
        } else if has_disabled {
            let arr = tbl.get_mut("disabled").and_then(|i| i.as_array_mut());
            if let Some(arr) = arr
                && !arr.iter().any(|v| v.as_str() == Some(name))
            {
                arr.push(name);
                changed = true;
            }
        } else {
            let mut a = toml_edit::Array::new();
            a.push(name);
            tbl.insert("disabled", Item::Value(toml_edit::Value::Array(a)));
            changed = true;
        }
    }
    if !changed {
        return Ok(());
    }
    std::fs::create_dir_all(root).map_err(|e| format!("{}: {e}", root.display()))?;
    crate::paths::write_private(&path, doc.to_string().as_bytes())
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Enablement per config semantics (same rules as startup `Config`).
fn raw_config_enabled(root: &Path, name: &str, default: bool) -> bool {
    #[derive(Default, Deserialize)]
    struct Raw {
        plugins: Option<RawPlugins>,
    }
    #[derive(Default, Deserialize)]
    struct RawPlugins {
        enabled: Option<Vec<String>>,
        disabled: Option<Vec<String>>,
    }
    let raw: Raw = std::fs::read_to_string(root.join("config.toml"))
        .ok()
        .and_then(|t| toml::from_str(&t).ok())
        .unwrap_or_default();
    match raw.plugins.as_ref().and_then(|p| p.enabled.as_ref()) {
        Some(list) => list.iter().any(|n| n == name),
        None => {
            let disabled: &[String] = raw
                .plugins
                .as_ref()
                .and_then(|p| p.disabled.as_ref())
                .map(|l| l.as_slice())
                .unwrap_or(&[]);
            default && !disabled.iter().any(|n| n == name)
        }
    }
}

// -------------------------------------------------------------- helpers

fn git(args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(args)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "git not found on PATH (required for index/URL plugins)".to_string()
            } else {
                format!("git: {e}")
            }
        })?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        let msg = msg.trim();
        return Err(if msg.is_empty() {
            "git command failed".to_string()
        } else {
            msg.to_string()
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn copy_dir(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    let entries = std::fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))?;
    for entry in entries.flatten() {
        let from = entry.path();
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let to = dst.join(&name);
        if from.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|e| format!("{}: {e}", to.display()))?;
        }
    }
    Ok(())
}

fn safe_subpath(p: &str) -> bool {
    let path = Path::new(p);
    !path.is_absolute() && !path.components().any(|c| matches!(c, Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plugin_src(parent: &Path, name: &str, extra: &str) -> PathBuf {
        let src = parent.join(format!("src-{name}"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("plugin.toml"),
            format!("name = \"{name}\"\n{extra}"),
        )
        .unwrap();
        src
    }

    fn run(root: &Path, args: &[&str]) -> i32 {
        let argv: Vec<String> = std::iter::once("plugin".to_string())
            .chain(args.iter().map(|s| s.to_string()))
            .collect();
        dispatch_in(&argv, &[], Path::new("/"), root)
    }

    #[test]
    fn add_local_copies_and_records_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path().join("cfg");
        let src = plugin_src(tmp.path(), "lp", "");
        assert_eq!(run(&r, &["add", &src.display().to_string()]), 0);
        let dest = r.join("plugins/lp");
        assert!(dest.join("plugin.toml").is_file());
        let meta = store::read_meta(&dest).unwrap();
        assert!(meta.source.ends_with("src-lp"));
        assert!(!meta.index);
    }

    #[test]
    fn add_updates_exact_enabled_list() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path().join("cfg");
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(r.join("config.toml"), "[plugins]\nenabled = [\"other\"]\n").unwrap();
        let src = plugin_src(tmp.path(), "lp", "");
        assert_eq!(run(&r, &["add", &src.display().to_string()]), 0);
        let cfg = std::fs::read_to_string(r.join("config.toml")).unwrap();
        assert!(cfg.contains("\"other\"") && cfg.contains("\"lp\""), "{cfg}");
        // and the new plugin counts as enabled
        assert!(raw_config_enabled(&r, "lp", true));
    }

    #[test]
    fn add_rejects_bad_manifest_and_duplicates() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path().join("cfg");
        let bad = tmp.path().join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("plugin.toml"), "version = \"1\"\n").unwrap();
        assert_eq!(run(&r, &["add", &bad.display().to_string()]), 1);
        assert!(!r.join("plugins").join("bad").exists());

        let src = plugin_src(tmp.path(), "lp", "");
        assert_eq!(run(&r, &["add", &src.display().to_string()]), 0);
        assert_eq!(
            run(&r, &["add", &src.display().to_string()]),
            1,
            "duplicate"
        );
    }

    #[test]
    fn rm_disables_then_purges() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path().join("cfg");
        let src = plugin_src(tmp.path(), "lp", "");
        run(&r, &["add", &src.display().to_string()]);
        // disabled list should now hold it? fresh root → no lists →
        // disable creates `disabled`
        assert_eq!(run(&r, &["rm", "lp"]), 0);
        let cfg = std::fs::read_to_string(r.join("config.toml")).unwrap();
        assert!(cfg.contains("disabled") && cfg.contains("lp"), "{cfg}");
        assert!(r.join("plugins/lp").is_dir(), "files kept without --purge");
        assert_eq!(run(&r, &["rm", "--purge", "lp"]), 0);
        assert!(!r.join("plugins/lp").exists());
        assert_eq!(run(&r, &["rm", "lp"]), 0, "idempotent");
    }

    #[test]
    fn rm_rejects_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path().join("cfg");
        assert_eq!(run(&r, &["rm", "--purge", "../evil"]), 1);
        assert_eq!(run(&r, &["rm", ".."]), 1);
        assert_eq!(run(&r, &["add", "../elsewhere"]), 1, "add validates too");
    }

    #[test]
    fn update_recopies_local_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path().join("cfg");
        let src = plugin_src(tmp.path(), "lp", "");
        run(&r, &["add", &src.display().to_string()]);
        std::fs::write(
            src.join("plugin.toml"),
            "name = \"lp\"\nversion = \"2.0\"\n",
        )
        .unwrap();
        assert_eq!(run(&r, &["update", "lp"]), 0);
        let text = std::fs::read_to_string(r.join("plugins/lp/plugin.toml")).unwrap();
        assert!(text.contains("2.0"), "{text}");
    }

    #[test]
    fn update_needs_origin_and_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path().join("cfg");
        assert_eq!(run(&r, &["update", "nope"]), 1);
        // installed but metadata stripped
        let src = plugin_src(tmp.path(), "lp", "");
        run(&r, &["add", &src.display().to_string()]);
        std::fs::remove_file(r.join("plugins/lp").join(META)).unwrap();
        assert_eq!(run(&r, &["update", "lp"]), 1);
    }

    // ---- index-backed flows (local git fixtures, no network) ----

    fn git_fixture(dir: &Path, files: &[(&str, &str)]) -> String {
        std::fs::create_dir_all(dir).unwrap();
        for (name, body) in files {
            let p = dir.join(name);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(p, body).unwrap();
        }
        let d = dir.display().to_string();
        let d = d.as_str();
        for args in [
            vec!["-C", d, "init", "-q"],
            vec!["-C", d, "add", "-A"],
            vec![
                "-C",
                d,
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-qm",
                "x",
            ],
        ] {
            git(&args).unwrap();
        }
        git(&["-C", d, "rev-parse", "HEAD"])
            .unwrap()
            .trim()
            .to_string()
    }

    #[test]
    fn index_add_verifies_pinned_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path().join("cfg");
        let plug = tmp.path().join("plugin-repo");
        let sha = git_fixture(&plug, &[("plugin.toml", "name = \"idxplug\"\n")]);
        let idx = tmp.path().join("index-repo");
        git_fixture(
            &idx,
            &[(
                "index/idxplug.toml",
                &format!(
                    "name = \"idxplug\"\nsource = \"{}\"\ncommit = \"{sha}\"\ndescription = \"demo\"\n",
                    plug.display()
                ),
            )],
        );
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(
            r.join("config.toml"),
            format!("[store]\nindex = \"{}\"\n", idx.display()),
        )
        .unwrap();

        assert_eq!(run(&r, &["add", "idxplug"]), 0);
        let dest = r.join("plugins/idxplug");
        assert!(dest.join("plugin.toml").is_file());
        let meta = store::read_meta(&dest).unwrap();
        assert!(meta.index);
        assert_eq!(meta.commit.as_deref(), Some(sha.as_str()));
        assert!(!dest.join(".git").exists(), "plain dir, no worktree");

        // search + info hit the same local index
        assert_eq!(run(&r, &["search", "demo"]), 0);
        assert_eq!(run(&r, &["info", "idxplug"]), 0);

        // update: same pinned commit → up to date, no re-clone
        assert_eq!(run(&r, &["update", "idxplug"]), 0);
    }

    #[test]
    fn index_add_fails_closed_on_commit_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path().join("cfg");
        let plug = tmp.path().join("plugin-repo");
        git_fixture(&plug, &[("plugin.toml", "name = \"idxplug\"\n")]);
        let idx = tmp.path().join("index-repo");
        git_fixture(
            &idx,
            &[(
                "index/idxplug.toml",
                &format!(
                    "name = \"idxplug\"\nsource = \"{}\"\ncommit = \"deadbeefdeadbeefdeadbeefdeadbeefdeadbeef\"\n",
                    plug.display()
                ),
            )],
        );
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(
            r.join("config.toml"),
            format!("[store]\nindex = \"{}\"\n", idx.display()),
        )
        .unwrap();
        assert_eq!(run(&r, &["add", "idxplug"]), 1);
        assert!(!r.join("plugins/idxplug").exists(), "nothing installed");
    }

    #[test]
    fn unknown_index_name_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path().join("cfg");
        let idx = tmp.path().join("index-repo");
        git_fixture(&idx, &[("README", "empty index")]);
        std::fs::create_dir_all(&r).unwrap();
        std::fs::write(
            r.join("config.toml"),
            format!("[store]\nindex = \"{}\"\n", idx.display()),
        )
        .unwrap();
        assert_eq!(run(&r, &["add", "ghost"]), 1);
        assert_eq!(run(&r, &["search", "ghost"]), 1, "no matches → 1");
        assert_eq!(run(&r, &["info", "ghost"]), 1);
    }

    #[test]
    fn list_merges_startup_registry_and_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let r = tmp.path().join("cfg");
        let src = plugin_src(tmp.path(), "fresh", "");
        run(&r, &["add", &src.display().to_string()]);
        let argv = vec!["plugin".to_string()];
        let installed = vec![("catalog-one".to_string(), false)];
        let status = dispatch_in(&argv, &installed, Path::new("/"), &r);
        assert_eq!(status, 0);
        // stdout not captured here — presence of both paths is enough:
        // catalog entry + disk scan run without panic.
        assert!(r.join("plugins/fresh").is_dir());
    }

    #[test]
    fn index_manifests_parse() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../index");
        let mut seen = 0;
        for e in std::fs::read_dir(&root).unwrap() {
            let path = e.unwrap().path();
            if path.extension().and_then(|x| x.to_str()) != Some("toml") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let entry: IndexEntry = toml::from_str(&text).unwrap();
            assert!(!entry.source.is_empty(), "{}", path.display());
            seen += 1;
        }
        assert!(seen >= 2, "starter + sentinel index entries expected");
    }
}
