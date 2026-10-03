//! The three built-in prompt themes.
//!
//! A theme owns the whole left-prompt layout; registered
//! [`PromptSegment`](crate::PromptSegment)s are appended wherever the
//! theme decides (robbyrussell: after the cwd, others: ignored).

use crate::{PromptSegment, Theme, color_enabled};

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

/// robbyrussell-style: `➜ dir [segments] ` — arrow green/red by status,
/// cwd cyan.
pub struct Robbyrussell;

impl Theme for Robbyrussell {
    fn name(&self) -> &str {
        "robbyrussell"
    }

    fn render(
        &self,
        status: i32,
        cwd: &std::path::Path,
        segments: &[&dyn PromptSegment],
    ) -> String {
        let dir = basename(cwd);
        let segs = seg_text(status, cwd, segments);
        let mut out = String::from("➜ ");
        if color_enabled() {
            let arrow = if status == 0 { GREEN } else { RED };
            out.clear();
            out.push_str(arrow);
            out.push_str("➜ ");
            out.push_str(RESET);
            out.push_str(CYAN);
            out.push_str(dir);
            out.push_str(RESET);
        } else {
            out.push_str(dir);
        }
        if !segs.is_empty() {
            out.push(' ');
            out.push_str(&segs);
        }
        out.push(' ');
        out
    }
}

/// `{basename} ` — directory only, no color.
pub struct Minimal;

impl Theme for Minimal {
    fn name(&self) -> &str {
        "minimal"
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
pub struct Plain;

impl Theme for Plain {
    fn name(&self) -> &str {
        "plain"
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

/// Registers all three (catalog plugin `default-themes`).
pub struct DefaultThemes;

impl crate::Plugin for DefaultThemes {
    fn name(&self) -> &str {
        "default-themes"
    }

    fn install(&self, reg: &mut crate::Registry) {
        reg.themes.push(Box::new(Robbyrussell));
        reg.themes.push(Box::new(Minimal));
        reg.themes.push(Box::new(Plain));
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
    fn robbyrussell_shows_dir_and_segments() {
        let cwd = std::path::Path::new("/home/u/brish");
        let seg = Seg("git:(main)");
        let out = Robbyrussell.render(0, cwd, &[&seg]);
        assert!(out.contains("brish"), "{out:?}");
        assert!(out.contains("git:(main)"), "{out:?}");
        assert!(out.ends_with(' '), "{out:?}");
        if color_enabled() {
            assert!(out.starts_with(GREEN), "status 0 arrow: {out:?}");
            assert!(out.contains(CYAN), "cwd cyan: {out:?}");
        }
        let bad = Robbyrussell.render(1, cwd, &[]);
        if color_enabled() {
            assert!(bad.starts_with(RED), "status 1 arrow: {bad:?}");
        }
        assert!(!bad.contains("git:(main)"), "no segments when none given");
    }

    #[test]
    fn minimal_and_plain_ignore_segments() {
        let cwd = std::path::Path::new("/x/y");
        let seg = Seg("ignored");
        assert_eq!(Minimal.render(0, cwd, &[&seg]), "y ");
        assert_eq!(Plain.render(0, cwd, &[&seg]), "$ ");
    }

    #[test]
    fn default_themes_plugin_registers_in_order() {
        let mut reg = crate::Registry::default();
        let p = DefaultThemes;
        reg.install(&p);
        let names: Vec<&str> = reg.themes.iter().map(|t| t.name()).collect();
        assert_eq!(names, vec!["robbyrussell", "minimal", "plain"]);
        assert_eq!(reg.installed(), &[("default-themes".to_string(), true)]);
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
