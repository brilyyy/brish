//! The four built-in prompt themes: `briiish-*` only (NOTES follow-up).
//!
//! A theme owns the whole left-prompt layout; registered
//! [`PromptSegment`](brish_plugin_api::PromptSegment)s are appended wherever the
//! theme decides (arrow themes: after the cwd, minimal/plain: ignored).

use brish_plugin_api::{PromptSegment, Theme, color_enabled};

const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const CYAN: &str = "\x1b[36m";
const RESET: &str = "\x1b[0m";

fn basename(cwd: &std::path::Path) -> &str {
    match cwd.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => cwd
            .to_str()
            .unwrap_or(cwd.as_os_str().to_str().unwrap_or(".")),
    }
}

/// Non-empty segment texts, space-joined.
fn seg_text(status: i32, cwd: &std::path::Path, segments: &[&dyn PromptSegment]) -> String {
    let mut out = String::new();
    for s in segments {
        if let Some(t) = s.render(status, cwd)
            && !t.is_empty()
        {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(&t);
        }
    }
    out
}

/// Shared arrow layout: `{color}{glyph} {RESET}{CYAN}{dir}{RESET}{segs} `.
fn arrow_prompt(
    status: i32,
    cwd: &std::path::Path,
    segments: &[&dyn PromptSegment],
    glyph: &str,
    dir_prefix: &str,
) -> String {
    let dir = basename(cwd);
    let segs = seg_text(status, cwd, segments);
    let mut out = String::new();
    if color_enabled() {
        let arrow = if status == 0 { GREEN } else { RED };
        out.push_str(arrow);
        out.push_str(glyph);
        out.push(' ');
        out.push_str(RESET);
        out.push_str(CYAN);
        out.push_str(dir_prefix);
        out.push_str(dir);
        out.push_str(RESET);
    } else {
        out.push_str(glyph);
        out.push(' ');
        out.push_str(dir_prefix);
        out.push_str(dir);
    }
    if !segs.is_empty() {
        out.push(' ');
        out.push_str(&segs);
    }
    out.push(' ');
    out
}

/// `{basename} ` — directory only, no color. Default theme: no font
/// requirements.
pub struct BriiishMinimal;

impl Theme for BriiishMinimal {
    fn name(&self) -> &str {
        "briiish-minimal"
    }

    fn render(
        &self,
        _status: i32,
        cwd: &std::path::Path,
        _segments: &[&dyn PromptSegment],
    ) -> String {
        format!("{} ", basename(cwd))
    }
}

/// `$ ` — POSIX-style.
pub struct BriiishPlain;

impl Theme for BriiishPlain {
    fn name(&self) -> &str {
        "briiish-plain"
    }

    fn render(
        &self,
        _status: i32,
        _cwd: &std::path::Path,
        _segments: &[&dyn PromptSegment],
    ) -> String {
        "$ ".to_string()
    }
}

/// Nerd-font: `❯  dir [segments] ` — folder glyph before cwd.
pub struct BriiishNerdFont;

impl Theme for BriiishNerdFont {
    fn name(&self) -> &str {
        "briiish-nerd-font"
    }

    fn render(
        &self,
        status: i32,
        cwd: &std::path::Path,
        segments: &[&dyn PromptSegment],
    ) -> String {
        // U+F07B nf-fa-folder — requires a Nerd Font.
        arrow_prompt(status, cwd, segments, "❯", "\u{f07b} ")
    }
}

/// Emoji: `✅ dir [segments] ` — status emoji instead of an arrow.
pub struct BriiishEmoji;

impl Theme for BriiishEmoji {
    fn name(&self) -> &str {
        "briiish-emoji"
    }

    fn render(
        &self,
        status: i32,
        cwd: &std::path::Path,
        segments: &[&dyn PromptSegment],
    ) -> String {
        let glyph = if status == 0 { "\u{2705}" } else { "\u{274c}" };
        arrow_prompt(status, cwd, segments, glyph, "")
    }
}

/// Registers all themes (catalog plugin `brish-themes`).
pub struct BrishThemes;

impl brish_plugin_api::Plugin for BrishThemes {
    fn name(&self) -> &str {
        "brish-themes"
    }

    fn install(&self, reg: &mut brish_plugin_api::Registry) {
        reg.themes.push(Box::new(BriiishMinimal));
        reg.themes.push(Box::new(BriiishPlain));
        reg.themes.push(Box::new(BriiishNerdFont));
        reg.themes.push(Box::new(BriiishEmoji));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Seg(&'static str);
    impl PromptSegment for Seg {
        fn render(&self, _status: i32, _cwd: &std::path::Path) -> Option<String> {
            Some(self.0.to_string())
        }
    }

    #[test]
    fn arrow_themes_show_dir_and_segments() {
        let cwd = std::path::Path::new("/home/u/brish");
        let seg = Seg("git:(main)");
        let themes: [&dyn Theme; 2] = [&BriiishNerdFont, &BriiishEmoji];
        for t in themes {
            let out = t.render(0, cwd, &[&seg]);
            assert!(out.contains("brish"), "{}: {out:?}", t.name());
            assert!(out.contains("git:(main)"), "{}: {out:?}", t.name());
            assert!(out.ends_with(' '), "{}: {out:?}", t.name());
        }
        let bad = BriiishNerdFont.render(1, cwd, &[]);
        assert!(!bad.contains("git:(main)"), "no segments when none given");
        if color_enabled() {
            let out = BriiishNerdFont.render(0, cwd, &[]);
            assert!(out.contains(GREEN), "status 0: {out:?}");
            let bad = BriiishNerdFont.render(1, cwd, &[]);
            assert!(bad.contains(RED), "status 1: {bad:?}");
        }
    }

    #[test]
    fn emoji_glyph_follows_status() {
        let cwd = std::path::Path::new("/x/y");
        let ok = BriiishEmoji.render(0, cwd, &[]);
        let bad = BriiishEmoji.render(1, cwd, &[]);
        assert!(ok.contains("\u{2705}"), "{ok:?}");
        assert!(bad.contains("\u{274c}"), "{bad:?}");
        assert!(!ok.contains("\u{274c}"), "{ok:?}");
    }

    #[test]
    fn minimal_and_plain_ignore_segments() {
        let cwd = std::path::Path::new("/x/y");
        let seg = Seg("ignored");
        assert_eq!(BriiishMinimal.render(0, cwd, &[&seg]), "y ");
        assert_eq!(BriiishPlain.render(0, cwd, &[&seg]), "$ ");
    }

    #[test]
    fn brish_themes_plugin_registers_in_order() {
        let mut reg = brish_plugin_api::Registry::default();
        let p = BrishThemes;
        reg.install(&p);
        let names: Vec<&str> = reg.themes.iter().map(|t| t.name()).collect();
        assert_eq!(
            names,
            vec![
                "briiish-minimal",
                "briiish-plain",
                "briiish-nerd-font",
                "briiish-emoji"
            ]
        );
        assert_eq!(reg.installed(), &[("brish-themes".to_string(), true)]);
    }

    #[test]
    fn empty_segment_is_skipped() {
        struct Empty;
        impl PromptSegment for Empty {
            fn render(&self, _status: i32, _cwd: &std::path::Path) -> Option<String> {
                None
            }
        }
        let e = Empty;
        let s = Seg("x");
        let cwd = std::path::Path::new("/d");
        assert_eq!(seg_text(0, cwd, &[&e, &s]), "x");
        assert_eq!(seg_text(0, cwd, &[&e]), "");
    }
}
