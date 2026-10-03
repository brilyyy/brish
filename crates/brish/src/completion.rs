//! Lite tab completion (plan 5.3): command names at command position,
//! `$VARS` after `$`, file paths elsewhere.
//!
//! Completion context is heuristic (whitespace + `;`/`|`/`&`/`&&`/`||`
//! segmentation) — quotes are not honoured yet; that arrives with the
//! plugin `CompletionProvider` work (`docs/PLUGIN-PLAN.md`).

use brish_builtin::BuiltIn;
use reedline::{Completer, CompletionResult, Span, Suggestion};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// Commands the engine intercepts itself (exec.rs) — not in `BuiltIn`.
const ENGINE_COMMANDS: &[&str] = &["eval", "wait", "jobs", "kill", "true", "false"];

const MAX_SUGGESTIONS: usize = 100;

pub struct BrishCompleter {
    path_cmds: OnceLock<Vec<String>>,
    /// Shell variable names, refreshed before each prompt by the REPL.
    vars: Arc<Mutex<Vec<String>>>,
}

impl BrishCompleter {
    pub fn new(vars: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            path_cmds: OnceLock::new(),
            vars,
        }
    }

    /// PATH executables, scanned once per session (names only — the
    /// executable-bit filter is not worth a stat storm for a prompt cache).
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

fn sug(value: String, span: &Span, desc: Option<&str>, append_space: bool) -> Suggestion {
    Suggestion {
        value,
        display_override: None,
        description: desc.map(str::to_string),
        style: None,
        extra: None,
        span: *span,
        append_whitespace: append_space,
        match_indices: None,
    }
}

fn commands(word: &str, span: &Span, path: &[String], out: &mut Vec<Suggestion>) {
    let mut seen: HashSet<&str> = HashSet::new();
    for c in BuiltIn::names().iter().chain(ENGINE_COMMANDS) {
        if c.starts_with(word) && seen.insert(c) {
            out.push(sug((*c).to_string(), span, Some("builtin"), true));
        }
    }
    for c in path {
        if c.starts_with(word) && seen.insert(c.as_str()) && out.len() < MAX_SUGGESTIONS {
            out.push(sug(c.clone(), span, None, true));
        }
    }
}

fn vars(word: &str, span: &Span, names: &Mutex<Vec<String>>, out: &mut Vec<Suggestion>) {
    let names = names.lock().unwrap_or_else(|e| e.into_inner());
    for v in names.iter() {
        if v.starts_with(word) && out.len() < MAX_SUGGESTIONS {
            out.push(sug(format!("${v}"), span, None, false));
        }
    }
}

/// File/dir suggestions for `word`, relative to `base`.
fn files(word: &str, span: &Span, base: &Path, out: &mut Vec<Suggestion>) {
    let (dir_part, name_part) = match word.rfind('/') {
        Some(i) => (&word[..=i], &word[i + 1..]),
        None => ("", word),
    };
    let dir: PathBuf = if dir_part == "~" || dir_part.starts_with("~/") {
        match dirs::home_dir() {
            Some(home) => home.join(dir_part.trim_start_matches('~').trim_start_matches('/')),
            None => return,
        }
    } else if dir_part.is_empty() {
        base.to_path_buf()
    } else {
        base.join(dir_part)
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
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
    for (n, is_dir) in entries.into_iter().take(MAX_SUGGESTIONS) {
        let value = if is_dir {
            format!("{dir_part}{n}/")
        } else {
            format!("{dir_part}{n}")
        };
        out.push(sug(
            value,
            span,
            if is_dir { Some("dir") } else { None },
            !is_dir,
        ));
    }
}

impl Completer for BrishCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> CompletionResult {
        let (start, word) = word_at(line, pos);
        let span = Span { start, end: pos };
        let mut out = Vec::new();
        if let Some(v) = word.strip_prefix('$') {
            vars(v, &span, &self.vars, &mut out);
        } else if command_position(&line[..start]) {
            commands(word, &span, self.path_commands(), &mut out);
        } else {
            let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            files(word, &span, &base, &mut out);
        }
        CompletionResult::fresh(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn completes_builtin_prefixes() {
        let mut out = Vec::new();
        let span = Span { start: 0, end: 2 };
        commands("ec", &span, &[], &mut out);
        assert!(out.iter().any(|s| s.value == "echo"), "{out:?}");
        assert!(!out.iter().any(|s| s.value == "cd"));
    }

    #[test]
    fn completes_vars_from_snapshot() {
        let names = Mutex::new(vec!["PATH".to_string(), "PS1".to_string()]);
        let span = Span { start: 0, end: 1 };
        let mut out = Vec::new();
        vars("P", &span, &names, &mut out);
        let vals: Vec<_> = out.iter().map(|s| s.value.as_str()).collect();
        assert_eq!(vals, vec!["$PATH", "$PS1"]);
    }

    #[test]
    fn completes_files_and_dirs_relative_to_base() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("alpha.txt"), "x").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();

        let span = Span { start: 0, end: 2 };
        let mut out = Vec::new();
        files("al", &span, dir.path(), &mut out);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].value, "alpha.txt");

        let mut out = Vec::new();
        files("su", &span, dir.path(), &mut out);
        assert_eq!(out[0].value, "sub/");
        assert!(!out[0].append_whitespace, "dir keeps typing open");
    }
}
