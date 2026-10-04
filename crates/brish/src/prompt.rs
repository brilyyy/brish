//! Interactive prompt assembly.
//!
//! Precedence (highest first): `PS2` while a continuation is pending,
//! `PS1` env (literal), else the **active theme** — looked up by name
//! in the plugin registry (`--theme` / `BRISH_THEME` / config /
//! `theme` builtin decide the name). Unknown theme → core `$ ` fallback.
//!
//! ```text
//! ❯  brish git:(main)     # briiish + segments (registry)
//! y                       # minimal
//! $                       # plain / fallback
//! ```

use brish_plugin::{PromptSegment, Registry};
use reedline::{
    Prompt, PromptEditMode, PromptHelixMode, PromptHistorySearch, PromptHistorySearchStatus,
    PromptViMode,
};
use std::borrow::Cow;
use std::path::Path;

/// Render the theme named `theme` with the registry's segments.
/// Pure — env reads happen in [`BrishPrompt::new`].
fn theme_text(status: i32, cwd: &Path, registry: &Registry, theme: &str) -> String {
    let segs: Vec<&dyn PromptSegment> = registry
        .prompt_segments
        .iter()
        .map(|s| s.as_ref())
        .collect();
    registry
        .themes
        .iter()
        .find(|t| t.name() == theme)
        .map(|t| t.render(status, cwd, &segs))
        .unwrap_or_else(|| "$ ".to_string())
}

/// Continuation > PS1 (literal) > theme rendering.
fn left_text(
    status: i32,
    cwd: &Path,
    ps1: Option<&str>,
    ps2: Option<&str>,
    continuation: bool,
    registry: &Registry,
    theme: &str,
) -> String {
    if continuation {
        return ps2.map(str::to_string).unwrap_or_else(|| "> ".to_string());
    }
    if let Some(ps1) = ps1 {
        return ps1.to_string();
    }
    theme_text(status, cwd, registry, theme)
}

/// Active-theme prompt for one `read_line` call.
pub struct BrishPrompt {
    left: String,
}

impl BrishPrompt {
    /// `continuation` selects PS2 (a multi-line buffer is pending).
    pub fn new(status: i32, continuation: bool, registry: &Registry, theme: &str) -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
        let ps1 = std::env::var("PS1").ok();
        let ps2 = std::env::var("PS2").ok();
        Self {
            left: left_text(
                status,
                &cwd,
                ps1.as_deref(),
                ps2.as_deref(),
                continuation,
                registry,
                theme,
            ),
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
    use brish_plugin::{Plugin, Registry, Theme};

    struct Dummy(&'static str);
    impl Theme for Dummy {
        fn name(&self) -> &str {
            self.0
        }
        fn render(&self, _s: i32, _c: &Path, _seg: &[&dyn PromptSegment]) -> String {
            format!("{} ", self.0)
        }
    }

    fn registry_with(name: &'static str) -> Registry {
        struct P(&'static str);
        impl Plugin for P {
            fn name(&self) -> &str {
                "dummy"
            }
            fn install(&self, reg: &mut Registry) {
                reg.themes.push(Box::new(Dummy(self.0)));
            }
        }
        let mut reg = Registry::default();
        reg.install(&P(name));
        reg
    }

    #[test]
    fn active_theme_renders_from_registry() {
        let reg = registry_with("t1");
        let cwd = Path::new("/x");
        assert_eq!(theme_text(0, cwd, &reg, "t1"), "t1 ");
        assert_eq!(theme_text(1, cwd, &reg, "t1"), "t1 ");
    }

    #[test]
    fn unknown_theme_falls_back_to_core_prompt() {
        let reg = registry_with("t1");
        assert_eq!(theme_text(0, Path::new("/x"), &reg, "nope"), "$ ");
        assert_eq!(
            theme_text(0, Path::new("/x"), &Registry::default(), "t1"),
            "$ "
        );
    }

    #[test]
    fn ps1_and_ps2_beat_theme_and_continuation_uses_ps2() {
        let reg = registry_with("t1");
        let cwd = Path::new("/x");
        // theme when nothing overrides
        assert_eq!(left_text(0, cwd, None, None, false, &reg, "t1"), "t1 ");
        // PS1 literal wins
        assert_eq!(left_text(0, cwd, Some("$ "), None, false, &reg, "t1"), "$ ");
        // continuation: PS2 or "> ", theme/PS1 irrelevant
        assert_eq!(left_text(0, cwd, Some("$ "), None, true, &reg, "t1"), "> ");
        assert_eq!(
            left_text(0, cwd, Some("$ "), Some(".. "), true, &reg, "t1"),
            ".. "
        );
    }
}
