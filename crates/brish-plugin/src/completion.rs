//! Tab completion: a thin **router** over the registry's
//! [`CompletionProvider`]s (plan 6.6).
//!
//! Context is heuristic (whitespace + `;`/`|`/`&`/`&&`/`||` segmentation)
//! — words are quote-aware (unclosed quotes keep spaces). The three default providers
//! (`default-completion` plugin) reproduce the lite behaviour: command
//! names at command position, `$VARS` after `$`, files elsewhere.
//! Providers early-return when the context is not theirs, so the router
//! stays a plain loop with merge + dedupe + cap.

use brish_builtin::BuiltIn;
use brish_plugin_api::{Algorithm, Completion, CompletionCtx, Plugin, Registry};
use reedline::{Completer, CompletionResult, Span, Suggestion};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Catalog name of the built-in completion plugin.
pub const DEFAULT_COMPLETION: &str = "brish-completion";

/// Commands the engine intercepts itself (exec.rs) — not in `BuiltIn`.
pub(crate) const ENGINE_COMMANDS: &[&str] =
    &["eval", "wait", "jobs", "kill", "fg", "bg", "true", "false"];

/// Every built-in shell command: in-enum ones (cd/echo/…) plus the
/// engine-intercepted commands (eval/true/…).
pub(crate) fn is_builtin(name: &str) -> bool {
    ENGINE_COMMANDS.contains(&name) || brish_builtin::BuiltIn::names().contains(&name)
}

const MAX_SUGGESTIONS: usize = 100;

/// Word under the cursor and its byte start. Quote-aware: whitespace
/// inside an unclosed `'…`/`"…` does not end the word, so
/// `cat "foo b<Tab>` completes the path behind the quote.
fn word_at(line: &str, pos: usize) -> (usize, &str) {
    let pos = pos.min(line.len());
    let mut start = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    for (i, c) in line[..pos].char_indices() {
        if c == '\'' && !in_double {
            in_single = !in_single;
        } else if c == '"' && !in_single {
            in_double = !in_double;
        } else if c.is_whitespace() && !in_single && !in_double {
            start = i + c.len_utf8();
        }
    }
    (start, &line[start..pos])
}

/// Peel one leading `'`/`"` for matching (the span still covers it,
/// and the suggestion value gets it back — see `suggestions_in`).
fn strip_open_quote(word: &str) -> (Option<char>, &str) {
    match word.chars().next() {
        Some(q @ ('"' | '\'')) => (Some(q), &word[q.len_utf8()..]),
        _ => (None, word),
    }
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

impl brish_plugin_api::CompletionProvider for CommandsProvider {
    fn complete(&self, ctx: &CompletionCtx<'_>) -> Vec<Completion> {
        if !ctx.is_command {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for c in BuiltIn::names().iter().chain(ENGINE_COMMANDS) {
            if ctx.matches(c) && seen.insert(c) {
                out.push(Completion {
                    value: (*c).to_string(),
                    description: Some("builtin".to_string()),
                    keep_typing: false,
                });
            }
        }
        for c in self.path_commands() {
            if ctx.matches(c) && seen.insert(c.as_str()) && out.len() < MAX_SUGGESTIONS {
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

impl brish_plugin_api::CompletionProvider for VarsProvider {
    fn complete(&self, ctx: &CompletionCtx<'_>) -> Vec<Completion> {
        if !ctx.after_dollar {
            return Vec::new();
        }
        let names = self.names.lock().unwrap_or_else(|e| e.into_inner());
        names
            .iter()
            .filter(|v| ctx.matches(v))
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

fn completions_for(ctx: &CompletionCtx<'_>, base: &Path) -> Vec<Completion> {
    let word = ctx.word;
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
            // Match on the filename part only — `ctx.word` still carries the
            // `dir_part` prefix, so `src/fo` would never match `foo`.
            ctx.algorithm
                .matches(name_part, &n)
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

impl brish_plugin_api::CompletionProvider for FilesProvider {
    fn complete(&self, ctx: &CompletionCtx<'_>) -> Vec<Completion> {
        if ctx.is_command || ctx.after_dollar {
            return Vec::new();
        }
        completions_for(ctx, ctx.cwd)
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

/// `[completion]` knobs, applied by the router after the providers.
#[derive(Debug, Clone, Copy, Default)]
pub struct MatchOpts {
    pub algorithm: Algorithm,
    /// Sort candidates by value before handing them to the menu.
    pub sort: bool,
    /// Keep a candidate whose *description* matches even when the
    /// value does not (nushell's `match_description`).
    pub match_description: bool,
    /// Max candidates shown before truncating (prevents scroll). Default 24.
    pub max_candidates: usize,
    /// Max width per candidate value (ellipsis truncated). 0 = no limit.
    pub truncate_width: usize,
}

impl MatchOpts {
    /// Build from config; defaults are prefix matching, sorted.
    pub fn from_config(cfg: &crate::config::Config) -> Self {
        Self {
            algorithm: cfg
                .completion_algorithm
                .as_deref()
                .and_then(Algorithm::from_name)
                .unwrap_or_default(),
            sort: cfg.completion_sort.unwrap_or(true),
            match_description: cfg.completion_match_description.unwrap_or(false),
            max_candidates: cfg.completion_max_candidates.unwrap_or(24),
            truncate_width: cfg.completion_truncate_width.unwrap_or(0),
        }
    }
}

/// Completer over a registry snapshot. Context detection stays here;
/// providers answer only for their slice.
pub struct BrishCompleter {
    providers: Arc<Registry>,
    opts: MatchOpts,
}

impl BrishCompleter {
    /// With `[completion]` knobs from config.
    pub fn new(providers: Arc<Registry>, opts: MatchOpts) -> Self {
        Self { providers, opts }
    }

    fn suggestions(&self, line: &str, pos: usize) -> Vec<Suggestion> {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        self.suggestions_in(line, pos, &cwd)
    }

    fn suggestions_in(&self, line: &str, pos: usize, cwd: &Path) -> Vec<Suggestion> {
        let (start, raw_word) = word_at(line, pos);
        let span = Span { start, end: pos };
        // Match without the opening quote; put it back on the way out
        // so the span replacement keeps the line's quoting intact.
        let (quote, word) = strip_open_quote(raw_word);
        let after_dollar = word.starts_with('$');
        let w = word.strip_prefix('$').unwrap_or(word);
        // A word containing `/` or starting with `.` is a path, never a
        // command name — `./t`, `../x`, `/usr/bi`, `.foo`, `..` must route
        // to the file provider even at the start of the line. POSIX command
        // names cannot begin with `.`.
        let looks_like_path = w.contains('/') || w.starts_with('.');
        let is_command = !after_dollar && !looks_like_path && command_position(&line[..start]);
        let ctx = CompletionCtx {
            word: w,
            is_command,
            after_dollar,
            cwd,
            line_before: &line[..start],
            algorithm: self.opts.algorithm,
            match_description: self.opts.match_description,
        };

        let mut merged: Vec<Completion> = Vec::new();
        for p in &self.providers.completion_providers {
            merged.extend(p.complete(&ctx));
        }
        // Providers already matched (value, and description when
        // `match_description` is on); the router only dedupes, sorts
        // and caps.
        let opts = self.opts;
        let mut seen = HashSet::new();
        let mut out: Vec<Completion> = merged
            .into_iter()
            .filter(|c| seen.insert(c.value.clone()))
            .take(MAX_SUGGESTIONS)
            .collect();
        if opts.sort {
            out.sort_by_key(|c| c.value.to_lowercase());
        }
        out.into_iter()
            .take(opts.max_candidates)
            .map(|c| {
                let value = match quote {
                    Some(q) => format!("{q}{}", c.value),
                    None => c.value,
                };
                let value =
                    if opts.truncate_width > 0 && value.chars().count() > opts.truncate_width {
                        let truncated: String = value
                            .chars()
                            .take(opts.truncate_width.saturating_sub(1))
                            .collect();
                        format!("{truncated}…")
                    } else {
                        value
                    };
                sug(value, &span, c.description.as_deref(), c.keep_typing)
            })
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
    use brish_plugin_api::CompletionProvider as _;

    #[test]
    fn word_at_splits_on_whitespace() {
        assert_eq!(word_at("echo he", 7), (5, "he"));
        assert_eq!(word_at("", 0), (0, ""));
        assert_eq!(word_at("cat ", 4), (4, ""));
        assert_eq!(word_at("a b c", 5), (4, "c"));
    }

    #[test]
    fn word_at_is_quote_aware() {
        // whitespace inside an open double quote keeps the word whole
        assert_eq!(word_at("cat \"foo b", 10), (4, "\"foo b"));
        // closed quote then space → boundary after the closing quote
        assert_eq!(word_at("echo \"a b\" c", 13), (11, "c"));
        // single quotes
        assert_eq!(word_at("cat 'a b", 8), (4, "'a b"));
        // quote-only word
        assert_eq!(word_at("x \"", 3), (2, "\""));
    }

    #[test]
    fn open_quote_is_stripped_for_match_and_restored_in_value() {
        let (q, w) = strip_open_quote("\"./fi");
        assert_eq!((q, w), (Some('"'), "./fi"));
        let (q, w) = strip_open_quote("./fi");
        assert_eq!((q, w), (None, "./fi"));
        let (q, w) = strip_open_quote("'x");
        assert_eq!((q, w), (Some('\''), "x"));

        // end-to-end: file behind an open quote completes, quote kept
        let dir = tempfile::tempdir().expect("tmpdir");
        std::fs::write(dir.path().join("script.sh"), "#!/bin/sh\n").expect("w");
        let line = "cat \"./scr";
        let pos = line.len();
        let (start, raw) = word_at(line, pos);
        assert_eq!(raw, "\"./scr");
        assert_eq!(&line[start..], "\"./scr");
        let (quote, stripped) = strip_open_quote(raw);
        assert_eq!(stripped, "./scr");
        assert_eq!(quote, Some('"'));
        // suggestion value restores the quote (splice covers the span)
        let value = format!("{}{}", quote.unwrap(), "./script.sh");
        assert_eq!(value, "\"./script.sh");
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
            algorithm: Algorithm::Prefix,
            match_description: false,
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

    #[test]
    fn files_provider_matches_on_filename_part_not_whole_word() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/alpha.txt"), "x").unwrap();

        // `src/al`: the dir prefix must not be part of the match
        let out = FilesProvider.complete(&ctx("src/al", false, false, dir.path()));
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].value, "src/alpha.txt");

        // trailing slash: empty name part matches every entry
        let out = FilesProvider.complete(&ctx("src/", false, false, dir.path()));
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].value, "src/alpha.txt");
    }

    #[test]
    fn path_word_at_command_position_completes_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("test-script.sh"), "x").unwrap();

        let mut reg = Registry::default();
        reg.completion_providers.push(Box::new(FilesProvider));
        let c = BrishCompleter::new(
            Arc::new(reg),
            MatchOpts {
                max_candidates: 100,
                truncate_width: 0,
                ..Default::default()
            },
        );

        // `./t` is the first word of the line (a command position)
        // but contains `/`, so it is a path, not a command name.
        for line in ["./t", "./test-script.sh"] {
            let out = c.suggestions_in(line, line.len(), dir.path());
            assert!(
                out.iter().any(|s| s.value == "./test-script.sh"),
                "{line} → {out:?}"
            );
        }
    }

    #[test]
    fn dot_word_completes_hidden_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".hidden"), "x").unwrap();
        std::fs::create_dir(dir.path().join(".hiddendir")).unwrap();
        std::fs::write(dir.path().join("visible"), "x").unwrap();

        let mut reg = Registry::default();
        reg.completion_providers.push(Box::new(FilesProvider));
        let c = BrishCompleter::new(
            Arc::new(reg),
            MatchOpts {
                max_candidates: 100,
                truncate_width: 0,
                ..Default::default()
            },
        );

        // `.` at command position → file completion, not command lookup
        let out = c.suggestions_in(".", 1, dir.path());
        let vals: Vec<&str> = out.iter().map(|s| s.value.as_str()).collect();
        assert!(vals.contains(&".hidden"), "{out:?}");
        assert!(vals.contains(&".hiddendir/"), "{out:?}");
        assert!(
            !vals.contains(&"visible"),
            "non-hidden entries must not match"
        );
        assert!(!out.iter().any(|s| s.value == "echo"));
    }

    struct Dup(&'static str);
    impl brish_plugin_api::CompletionProvider for Dup {
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
    fn algorithms_match_prefix_substring_and_fuzzy() {
        use brish_plugin_api::Algorithm;
        let (prefix, substring, fuzzy) =
            (Algorithm::Prefix, Algorithm::Substring, Algorithm::Fuzzy);
        assert!(prefix.matches("gi", "git"));
        assert!(!prefix.matches("gt", "git"));
        assert!(substring.matches("it", "git"));
        assert!(!substring.matches("gt", "git"), "g..t is not contiguous");
        // subsequence, case-insensitive
        assert!(fuzzy.matches("gt", "git"));
        assert!(fuzzy.matches("GIT", "git"));
        assert!(!fuzzy.matches("tg", "git"));
        // empty word matches everything (bare Tab)
        assert!(prefix.matches("", "anything"));
        assert_eq!(Algorithm::from_name("FUZZY"), Some(fuzzy));
        assert_eq!(Algorithm::from_name("nope"), None);
    }

    #[test]
    fn router_applies_algorithm_sort_and_description_match() {
        // provider matches with the ctx algorithm; values deliberately
        // unsorted so `sort` has something to do
        struct P;
        impl brish_plugin_api::CompletionProvider for P {
            fn complete(&self, ctx: &CompletionCtx<'_>) -> Vec<Completion> {
                ["zeta", "alpha", "beta"]
                    .iter()
                    .filter(|v| {
                        ctx.matches_with(v, if **v == "zeta" { Some("tailing") } else { None })
                    })
                    .map(|v| Completion {
                        value: (*v).to_string(),
                        description: match *v {
                            "zeta" => Some("tailing".to_string()),
                            _ => None,
                        },
                        keep_typing: false,
                    })
                    .collect()
            }
        }
        let dir = Path::new(".");
        let with_p = |opts: MatchOpts| {
            let mut reg = Registry::default();
            reg.completion_providers.push(Box::new(P));
            BrishCompleter::new(Arc::new(reg), opts)
        };

        let c = with_p(MatchOpts {
            algorithm: brish_plugin_api::Algorithm::Prefix,
            sort: true,
            match_description: false,
            max_candidates: 100,
            truncate_width: 0,
        });
        let got: Vec<String> = c
            .suggestions_in("al", 2, dir)
            .iter()
            .map(|s| s.value.clone())
            .collect();
        assert_eq!(got, vec!["alpha"], "prefix match, sorted");

        // substring matches inside a value
        let c = with_p(MatchOpts {
            algorithm: brish_plugin_api::Algorithm::Substring,
            sort: false,
            match_description: false,
            max_candidates: 100,
            truncate_width: 0,
        });
        let got: Vec<String> = c
            .suggestions_in("et", 2, dir)
            .iter()
            .map(|s| s.value.clone())
            .collect();
        assert_eq!(got, vec!["zeta", "beta"], "substring keeps provider order");

        // match_description: word matches the description, not the value
        let c = with_p(MatchOpts {
            algorithm: brish_plugin_api::Algorithm::Substring,
            sort: true,
            match_description: true,
            max_candidates: 100,
            truncate_width: 0,
        });
        let got: Vec<String> = c
            .suggestions_in("ailin", 5, dir)
            .iter()
            .map(|s| s.value.clone())
            .collect();
        assert_eq!(got, vec!["zeta"], "description match keeps zeta");
    }

    #[test]
    fn router_merges_dedupes_and_caps() {
        let mut reg = Registry::default();
        reg.completion_providers.push(Box::new(Dup("one")));
        reg.completion_providers.push(Box::new(Dup("two")));
        let c = BrishCompleter::new(
            Arc::new(reg),
            MatchOpts {
                max_candidates: 100,
                truncate_width: 0,
                ..Default::default()
            },
        );
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
        let c = BrishCompleter::new(
            Arc::new(reg),
            MatchOpts {
                max_candidates: 100,
                truncate_width: 0,
                ..Default::default()
            },
        );

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
