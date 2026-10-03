//! Default interactive prompt: robbyrussell-style arrow + cwd basename.
//!
//! ```text
//! ➜  brish        # last status 0  → green arrow
//! ➜  brish        # status ≠ 0     → red arrow
//! ```
//!
//! `PS1`/`PS2` set in the environment override the default as literal
//! strings (no escape processing — plan: prompt sequences arrive with the
//! theme/plugin work). `NO_COLOR` disables ANSI.

use reedline::{
    Prompt, PromptEditMode, PromptHelixMode, PromptHistorySearch, PromptHistorySearchStatus,
    PromptViMode,
};
use std::borrow::Cow;

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const CYAN: &str = "\x1b[36m";
const RESET: &str = "\x1b[0m";

fn color_enabled() -> bool {
    std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
}

/// Left-prompt text for one `read_line` call. Pure — env reads happen in
/// [`BrishPrompt::new`] so tests can drive the rules directly.
fn left_text(
    status: i32,
    cwd: &str,
    ps1: Option<&str>,
    ps2: Option<&str>,
    continuation: bool,
) -> String {
    if continuation {
        return ps2.map(str::to_string).unwrap_or_else(|| "> ".to_string());
    }
    if let Some(ps1) = ps1 {
        return ps1.to_string();
    }
    let dir = basename(cwd);
    if color_enabled() {
        let arrow = if status == 0 { GREEN } else { RED };
        format!("{arrow}➜ {RESET}{CYAN}{dir}{RESET} ")
    } else {
        format!("➜ {dir} ")
    }
}

fn basename(cwd: &str) -> &str {
    let p = std::path::Path::new(cwd);
    match p.file_name() {
        Some(name) => name.to_str().unwrap_or(cwd),
        None => cwd, // root or trailing-slash-only
    }
}

/// robbyrussell-style prompt for one `read_line` call.
pub struct BrishPrompt {
    left: String,
}

impl BrishPrompt {
    /// `continuation` selects PS2 (a multi-line buffer is pending).
    pub fn new(status: i32, continuation: bool) -> Self {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| ".".to_string());
        let ps1 = std::env::var("PS1").ok();
        let ps2 = std::env::var("PS2").ok();
        Self {
            left: left_text(status, &cwd, ps1.as_deref(), ps2.as_deref(), continuation),
        }
    }
}

impl Prompt for BrishPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.left)
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, edit_mode: PromptEditMode) -> Cow<'_, str> {
        match edit_mode {
            PromptEditMode::Default | PromptEditMode::Emacs => Cow::Borrowed("> "),
            PromptEditMode::Helix(PromptHelixMode::Insert)
            | PromptEditMode::Vi(PromptViMode::Insert) => Cow::Borrowed("> "),
            PromptEditMode::Helix(PromptHelixMode::Normal)
            | PromptEditMode::Vi(PromptViMode::Normal) => Cow::Borrowed(": "),
            PromptEditMode::Helix(PromptHelixMode::Select)
            | PromptEditMode::Vi(PromptViMode::Visual) => Cow::Borrowed("+ "),
            PromptEditMode::Custom(s) => Cow::Owned(format!("({s})")),
        }
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("::: ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "failing ",
        };
        Cow::Owned(format!(
            "({prefix}reverse-search: {}) ",
            history_search.term
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_color_follows_status() {
        fn plain(s: &str) -> String {
            s.replace(GREEN, "")
                .replace(RED, "")
                .replace(CYAN, "")
                .replace(RESET, "")
        }
        let ok = left_text(0, "/home/u/brish", None, None, false);
        let bad = left_text(1, "/home/u/brish", None, None, false);
        assert_eq!(plain(&ok), "➜ brish ");
        assert_eq!(plain(&bad), "➜ brish ");
        if color_enabled() {
            assert!(ok.starts_with(GREEN), "green arrow for status 0: {ok:?}");
            assert!(bad.starts_with(RED), "red arrow for status != 0: {bad:?}");
            assert!(ok.contains(CYAN), "cwd is cyan: {ok:?}");
        }
    }

    #[test]
    fn basename_is_last_path_component() {
        let t = left_text(0, "/home/u/dev", None, None, false);
        assert!(t.contains("dev"), "{t:?}");
        assert!(!t.contains("/home"), "{t:?}");
        let root = left_text(0, "/", None, None, false);
        assert!(root.contains('/'), "{root:?}");
    }

    #[test]
    fn ps1_and_ps2_override_defaults() {
        assert_eq!(left_text(0, "/x", Some("$ "), None, false), "$ ");
        assert_eq!(left_text(0, "/x", None, None, true), "> ");
        assert_eq!(left_text(0, "/x", Some("$ "), Some(".. "), true), ".. ");
    }
}
