//! Tab completion: a thin **router** over the registry's
//! [`CompletionProvider`]s (plan 6.6).
//!
//! Context is heuristic (whitespace + `;`/`|`/`&`/`&&`/`||` segmentation)
//! — quotes are not honoured yet. The three default providers
//! (`default-completion` plugin) reproduce the lite behaviour: command
//! names at command position, `$VARS` after `$`, files elsewhere.
//! Providers early-return when the context is not theirs, so the router
//! stays a plain loop with merge + dedupe + cap.

use brish_builtin::BuiltIn;
use brish_plugin::{Completion, CompletionCtx, Plugin, Registry};
use reedline::{Completer, CompletionResult, Span, Suggestion};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Catalog name of the built-in completion plugin.
pub const DEFAULT_COMPLETION: &str = "default-completion";

/// Commands the engine intercepts itself (exec.rs) — not in `BuiltIn`.
const ENGINE_COMMANDS: &[&str] = &["eval", "wait", "jobs", "kill", "fg", "bg", "true", "false"];

const MAX_SUGGESTIONS: usize = 100;

/// Word under the cursor and its byte start.
fn word_at(line: &str, pos: usize) -> (usize, &str) {
    let start = line[..pos].rfind(char::is_whitespace).map_or(0, |i| i + 1);
    (start, &line[start..pos])
}

/// True when the cursor sits at the start of a command (prefix contains
/// no word after the last `;`/`|`/`&`/`&&`/`||` separator).
fn command_position(prefix: &str) -> bool {
    let mut cut = 0;
    for op in ["&&", "||", ";", "|", "&"] {
        if let Some(i) = prefix.rfind(op) {
            cut = cut.max(i + op.len());
        }
    }
    prefix[cut..].trim().is_empty()
}

fn sug(value: String, span: &Span, desc: Option<&str>, keep_typing: bool) -> Suggestion {
    Suggestion {
        value,
        display_override: None,
        description: desc.map(str::to_string),
        style: None,
        extra: None,
        span: *span,
        append_whitespace: !keep_typing,
        match_indices: None,
    }
}

// ---- default-completion providers ----

/// Builtins + engine commands + `$PATH` executables.
#[derive(Default)]
pub struct CommandsProvider {
    /// PATH executables, scanned once per session (names only — the
    /// executable-bit filter is not worth a stat storm).
    path_cmds: OnceLock<Vec<String>>,
}

impl CommandsProvider {
    fn path_commands(&self) -> &[String] {
        self.path_cmds.get_or_init(|| {
            let mut set = HashSet::new();
            if let Some(paths) = std::env::var_os("PATH") {
                for dir in std::env::split_paths(&paths) {
                    if let Ok(rd) = std::fs::read_dir(&dir) {
                        for e in rd.flatten() {
                            let name = e.file_name();
                            if let Some(n) = name.to_str() {
                                set.insert(n.to_string());
                            }
                        }
                    }
                }
            }
            let mut v: Vec<String> = set.into_iter().collect();
            v.sort();
            v
        })
    }
}

impl brish_plugin::CompletionProvider for CommandsProvider {
    fn complete(&self, ctx: &CompletionCtx<'_>) -> Vec<Completion> {
        if !ctx.is_command {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for c in BuiltIn::names().iter().chain(ENGINE_COMMANDS) {
            if c.starts_with(ctx.word) && seen.insert(c) {
                out.push(Completion {
                    value: (*c).to_string(),
                    description: Some("builtin".to_string()),
                    keep_typing: false,
                });
            }
        }
        for c in self.path_commands() {
            if c.starts_with(ctx.word) && seen.insert(c.as_str()) && out.len() < MAX_SUGGESTIONS {
                out.push(Completion {
                    value: c.clone(),
                    description: None,
                    keep_typing: false,
                });
            }
        }
        out
    }
}

/// `$NAME` completion from the REPL's variable-name snapshot.
pub struct VarsProvider {
    names: Arc<Mutex<Vec<String>>>,
}

impl VarsProvider {
    pub fn new(names: Arc<Mutex<Vec<String>>>) -> Self {
        Self { names }
    }
}

impl brish_plugin::CompletionProvider for VarsProvider {
    fn complete(&self, ctx: &CompletionCtx<'_>) -> Vec<Completion> {
        if !ctx.after_dollar {
            return Vec::new();
        }
        let names = self.names.lock().unwrap_or_else(|e| e.into_inner());
        names
            .iter()
            .filter(|v| v.starts_with(ctx.word))
            .take(MAX_SUGGESTIONS)
            .map(|v| Completion {
                value: format!("${v}"),
                description: None,
                keep_typing: true, // `$FOO` may be followed by more
            })
            .collect()
    }
}

/// File/dir suggestions relative to `ctx.cwd` (`~` honoured).
pub struct FilesProvider;

fn completions_for(word: &str, base: &Path) -> Vec<Completion> {
    let (dir_part, name_part) = match word.rfind('/') {
        Some(i) => (&word[..=i], &word[i + 1..]),
        None => ("", word),
    };
    let dir: PathBuf = if dir_part == "~" || dir_part.starts_with("~/") {
        match dirs::home_dir() {
            Some(home) => home.join(dir_part.trim_start_matches('~').trim_start_matches('/')),
            None => return Vec::new(),
        }
    } else if dir_part.is_empty() {
        base.to_path_buf()
    } else {
        base.join(dir_part)
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut entries: Vec<(String, bool)> = rd
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_str()?.to_string();
            n.starts_with(name_part)
                .then(|| (n, e.file_type().is_ok_and(|t| t.is_dir())))
        })
        .collect();
    entries.sort();
    entries
        .into_iter()
        .take(MAX_SUGGESTIONS)
        .map(|(n, is_dir)| {
            let value = if is_dir {
                format!("{dir_part}{n}/")
            } else {
                format!("{dir_part}{n}")
            };
            Completion {
                value,
                description: if is_dir { Some("dir".into()) } else { None },
                keep_typing: is_dir, // dirs stay open, no trailing space
            }
        })
        .collect()
}

impl brish_plugin::CompletionProvider for FilesProvider {
    fn complete(&self, ctx: &CompletionCtx<'_>) -> Vec<Completion> {
        if ctx.is_command || ctx.after_dollar {
            return Vec::new();
        }
        completions_for(ctx.word, ctx.cwd)
    }
}

/// Catalog plugin `default-completion` (on by default).
pub struct DefaultCompletion {
    pub vars: Arc<Mutex<Vec<String>>>,
}

impl Plugin for DefaultCompletion {
    fn name(&self) -> &str {
        DEFAULT_COMPLETION
    }

    fn install(&self, reg: &mut Registry) {
        reg.completion_providers
            .push(Box::new(CommandsProvider::default()));
        reg.completion_providers
            .push(Box::new(VarsProvider::new(Arc::clone(&self.vars))));
        reg.completion_providers.push(Box::new(FilesProvider));
    }
}

// ---- router ----

/// Completer over a registry snapshot. Context detection stays here;
/// providers answer only for their slice.
pub struct BrishCompleter {
    providers: Arc<Registry>,
}

impl BrishCompleter {
    pub fn new(providers: Arc<Registry>) -> Self {
        Self { providers }
    }

    fn suggestions(&self, line: &str, pos: usize) -> Vec<Suggestion> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.suggestions_in(line, pos, &cwd)
    }

    fn suggestions_in(&self, line: &str, pos: usize, cwd: &Path) -> Vec<Suggestion> {
        let (start, word) = word_at(line, pos);
        let span = Span { start, end: pos };
        let after_dollar = word.starts_with('$');
        let w = word.strip_prefix('$').unwrap_or(word);
        let is_command = !after_dollar && command_position(&line[..start]);
        let ctx = CompletionCtx {
            word: w,
            is_command,
            after_dollar,
            cwd,
            line_before: &line[..start],
        };

        let mut merged: Vec<Completion> = Vec::new();
        for p in &self.providers.completion_providers {
            merged.extend(p.complete(&ctx));
        }
        let mut seen = HashSet::new();
        merged
            .into_iter()
            .filter(|c| seen.insert(c.value.clone()))
            .take(MAX_SUGGESTIONS)
            .map(|c| sug(c.value, &span, c.description.as_deref(), c.keep_typing))
            .collect()
    }
}

impl Completer for BrishCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        CompletionResult::fresh(self.suggestions(line, pos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brish_plugin::CompletionProvider as _;

    #[test]
    fn word_at_splits_on_whitespace() {
        assert_eq!(word_at("echo he", 7), (5, "he"));
        assert_eq!(word_at("", 0), (0, ""));
        assert_eq!(word_at("cat ", 4), (4, ""));
        assert_eq!(word_at("a b c", 5), (4, "c"));
    }

    #[test]
    fn command_position_follows_separators() {
        assert!(command_position(""));
        assert!(!command_position("echo "), "second word is an arg position");
        assert!(command_position("echo hi; "));
        assert!(command_position("a && "));
        assert!(command_position("a || "));
        assert!(command_position("a | "));
        assert!(command_position("a & "));
        assert!(!command_position("echo"));
        assert!(!command_position("echo hi; cat "));
    }

    fn ctx<'a>(
        word: &'a str,
        is_command: bool,
        after_dollar: bool,
        cwd: &'a Path,
    ) -> CompletionCtx<'a> {
        CompletionCtx {
            word,
            is_command,
            after_dollar,
            cwd,
            line_before: "",
        }
    }

    #[test]
    fn commands_provider_completes_builtin_prefixes() {
        let p = CommandsProvider::default();
        // inject a fake PATH so the scan is deterministic
        p.path_cmds.set(vec!["zztool".into()]).ok();
        let dir = Path::new(".");
        let out = p.complete(&ctx("ec", true, false, dir));
        assert!(out.iter().any(|c| c.value == "echo"), "{out:?}");
        assert!(!out.iter().any(|c| c.value == "cd"));
        assert!(
            p.complete(&ctx("zz", true, false, dir))
                .iter()
                .any(|c| c.value == "zztool")
        );
        // not a command position → silent
        assert!(p.complete(&ctx("ec", false, false, dir)).is_empty());
    }

    #[test]
    fn vars_provider_reads_snapshot_only_after_dollar() {
        let names = Arc::new(Mutex::new(vec!["PATH".to_string(), "PS1".to_string()]));
        let p = VarsProvider::new(Arc::clone(&names));
        let dir = Path::new(".");
        let out = p.complete(&ctx("P", false, true, dir));
        let vals: Vec<&str> = out.iter().map(|c| c.value.as_str()).collect();
        assert_eq!(vals, vec!["$PATH", "$PS1"]);
        assert!(out.iter().all(|c| c.keep_typing));
        assert!(p.complete(&ctx("P", false, false, dir)).is_empty());
    }

    #[test]
    fn files_provider_completes_files_and_dirs_relative_to_cwd() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();

        let out = FilesProvider.complete(&ctx("al", false, false, dir.path()));
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].value, "alpha.txt");
        assert!(!out[0].keep_typing, "file accepts trailing space");

        let out = FilesProvider.complete(&ctx("su", false, false, dir.path()));
        assert_eq!(out[0].value, "sub/");
        assert!(out[0].keep_typing, "dir keeps typing open");

        // context guards: never for commands or $-words
        assert!(
            FilesProvider
                .complete(&ctx("al", true, false, dir.path()))
                .is_empty()
        );
        assert!(
            FilesProvider
                .complete(&ctx("al", false, true, dir.path()))
                .is_empty()
        );
    }

    struct Dup(&'static str);
    impl brish_plugin::CompletionProvider for Dup {
        fn complete(&self, _ctx: &CompletionCtx<'_>) -> Vec<Completion> {
            vec![
                Completion {
                    value: "same".into(),
                    description: Some(self.0.into()),
                    keep_typing: false,
                },
                Completion {
                    value: self.0.into(),
                    description: None,
                    keep_typing: false,
                },
            ]
        }
    }

    #[test]
    fn router_merges_dedupes_and_caps() {
        let mut reg = Registry::default();
        reg.completion_providers.push(Box::new(Dup("one")));
        reg.completion_providers.push(Box::new(Dup("two")));
        let c = BrishCompleter::new(Arc::new(reg));
        let out = c.suggestions("x", 1);
        let vals: Vec<&str> = out.iter().map(|s| s.value.as_str()).collect();
        // "same" appears once (first provider wins), one/two both kept
        assert_eq!(vals.iter().filter(|v| **v == "same").count(), 1);
        assert!(vals.contains(&"one"));
        assert!(vals.contains(&"two"));
        assert_eq!(out[0].description.as_deref(), Some("one"));
    }

    #[test]
    fn router_routes_context_to_right_provider() {
        let names = Arc::new(Mutex::new(vec!["HOME".to_string()]));
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("file.txt"), "x").unwrap();

        let mut reg = Registry::default();
        reg.completion_providers
            .push(Box::new(CommandsProvider::default()));
        reg.completion_providers
            .push(Box::new(VarsProvider::new(names)));
        reg.completion_providers.push(Box::new(FilesProvider));
        let c = BrishCompleter::new(Arc::new(reg));

        // command position: builtins, not files
        let out = c.suggestions_in("ec", 2, dir.path());
        assert!(out.iter().any(|s| s.value == "echo"));
        assert!(!out.iter().any(|s| s.value == "file.txt"));

        // $-word: vars only
        let out = c.suggestions_in("$HO", 3, dir.path());
        assert!(out.iter().any(|s| s.value == "$HOME"));
        assert!(!out.iter().any(|s| s.value == "echo"));

        // arg position: files only
        let out = c.suggestions_in("cat fi", 6, dir.path());
        assert!(out.iter().any(|s| s.value == "file.txt"));
        assert!(!out.iter().any(|s| s.value == "echo"));
    }
}
