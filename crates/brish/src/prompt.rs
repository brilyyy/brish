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

use brish_plugin::{PromptSegment, Registry, color_enabled};
use reedline::{
    Prompt, PromptEditMode, PromptHelixMode, PromptHistorySearch, PromptHistorySearchStatus,
    PromptViMode,
};
use std::borrow::Cow;
use std::path::Path;

use crate::config::PromptChrome;

/// Render the theme named `theme` with the registry's segments —
/// or the `[theme] prompt` template when present (NOTES.md 10).
/// Pure — env reads happen in [`BrishPrompt::new`].
fn theme_text(
    status: i32,
    cwd: &Path,
    registry: &Registry,
    theme: &str,
    template: Option<&str>,
) -> String {
    let segs: Vec<&dyn PromptSegment> = registry
        .prompt_segments
        .iter()
        .map(|s| s.as_ref())
        .collect();
    if let Some(tmpl) = template {
        return brish_builtin::store::expand_template(tmpl, status, cwd, &segs, color_enabled());
    }
    registry
        .themes
        .iter()
        .find(|t| t.name() == theme)
        .map(|t| t.render(status, cwd, &segs))
        .unwrap_or_else(|| "$ ".to_string())
}

/// Continuation > PS1 (literal) > template > theme rendering.
#[allow(clippy::too_many_arguments)]
fn left_text(
    status: i32,
    cwd: &Path,
    ps1: Option<&str>,
    ps2: Option<&str>,
    continuation: bool,
    registry: &Registry,
    theme: &str,
    template: Option<&str>,
    chrome: &PromptChrome,
) -> String {
    if continuation {
        return ps2
            .map(str::to_string)
            .unwrap_or_else(|| chrome.multiline.clone());
    }
    if let Some(ps1) = ps1 {
        return ps1.to_string();
    }
    theme_text(status, cwd, registry, theme, template)
}

/// Active-theme prompt for one `read_line` call.
pub struct BrishPrompt {
    left: String,
    chrome: PromptChrome,
}

impl BrishPrompt {
    /// `continuation` selects PS2 (a multi-line buffer is pending).
    pub fn new(
        status: i32,
        continuation: bool,
        registry: &Registry,
        theme: &str,
        chrome: &PromptChrome,
        template: Option<&str>,
    ) -> Self {
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
                template,
                chrome,
            ),
            chrome: chrome.clone(),
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
            PromptEditMode::Default | PromptEditMode::Emacs => {
                Cow::Borrowed(self.chrome.indicator.as_str())
            }
            PromptEditMode::Helix(PromptHelixMode::Insert)
            | PromptEditMode::Vi(PromptViMode::Insert) => {
                Cow::Borrowed(self.chrome.indicator.as_str())
            }
            PromptEditMode::Helix(PromptHelixMode::Normal)
            | PromptEditMode::Vi(PromptViMode::Normal) => {
                Cow::Borrowed(self.chrome.vi_normal.as_str())
            }
            PromptEditMode::Helix(PromptHelixMode::Select)
            | PromptEditMode::Vi(PromptViMode::Visual) => {
                Cow::Borrowed(self.chrome.vi_visual.as_str())
            }
            PromptEditMode::Custom(s) => Cow::Owned(format!("({s})")),
        }
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.chrome.multiline.as_str())
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
        assert_eq!(theme_text(0, cwd, &reg, "t1", None), "t1 ");
        assert_eq!(theme_text(1, cwd, &reg, "t1", None), "t1 ");
    }

    #[test]
    fn unknown_theme_falls_back_to_core_prompt() {
        let reg = registry_with("t1");
        assert_eq!(theme_text(0, Path::new("/x"), &reg, "nope", None), "$ ");
        assert_eq!(
            theme_text(0, Path::new("/x"), &Registry::default(), "t1", None),
            "$ "
        );
    }

    #[test]
    fn ps1_and_ps2_beat_theme_and_continuation_uses_chrome() {
        let reg = registry_with("t1");
        let cwd = Path::new("/x");
        let chrome = PromptChrome::default();
        // theme when nothing overrides
        assert_eq!(
            left_text(0, cwd, None, None, false, &reg, "t1", None, &chrome),
            "t1 "
        );
        // PS1 literal wins
        assert_eq!(
            left_text(0, cwd, Some("$ "), None, false, &reg, "t1", None, &chrome),
            "$ "
        );
        // continuation: PS2 or chrome.multiline, theme/PS1 irrelevant
        assert_eq!(
            left_text(0, cwd, Some("$ "), None, true, &reg, "t1", None, &chrome),
            "::: "
        );
        assert_eq!(
            left_text(
                0,
                cwd,
                Some("$ "),
                Some(".. "),
                true,
                &reg,
                "t1",
                None,
                &chrome
            ),
            ".. "
        );
        // custom multiline indicator from config
        let c = PromptChrome {
            multiline: ".. ".into(),
            ..Default::default()
        };
        assert_eq!(
            left_text(0, cwd, None, None, true, &reg, "t1", None, &c),
            ".. "
        );
    }

    #[test]
    fn template_overrides_theme_and_is_multiline() {
        let reg = registry_with("t1");
        let cwd = Path::new("/x");
        let chrome = PromptChrome::default();
        // template wins over named theme
        assert_eq!(
            left_text(
                0,
                cwd,
                None,
                None,
                false,
                &reg,
                "t1",
                Some("{cwd}\n> "),
                &chrome
            ),
            "x\n> "
        );
        // PS1 still beats template
        assert_eq!(
            left_text(
                0,
                cwd,
                Some("$ "),
                None,
                false,
                &reg,
                "t1",
                Some("{cwd}"),
                &chrome
            ),
            "$ "
        );
    }
}
