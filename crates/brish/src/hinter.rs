//! Fish-style history autosuggestion, wired by the config-gated
//! `autosuggest` plugin.

use nu_ansi_term::{Color, Style};
use reedline::{Hinter, History, SearchQuery};

/// Suggests the most recent history entry that starts with the typed
/// line. The hint only appears with the cursor at end-of-line —
/// mid-line, the remainder on screen isn't the hint's business.
#[derive(Default)]
pub struct BrishHinter {
    current: String,
}

impl Hinter for BrishHinter {
    fn handle(
        &mut self,
        line: &str,
        pos: usize,
        history: &dyn History,
        use_ansi_coloring: bool,
        _cwd: &str,
    ) -> String {
        self.current.clear();
        if pos == line.len()
            && !line.is_empty()
            && let Ok(items) = history.search(SearchQuery::last_with_prefix(
                line.to_string(),
                history.session(),
            ))
            && let Some(entry) = items.first()
            && let Some(rest) = entry.command_line.get(line.len()..)
            && !rest.is_empty()
        {
            self.current = rest.to_string();
        }
        if use_ansi_coloring && !self.current.is_empty() {
            Style::new()
                .fg(Color::LightGray)
                .paint(&self.current)
                .to_string()
        } else {
            self.current.clone()
        }
    }

    fn complete_hint(&self) -> String {
        self.current.clone()
    }

    fn next_hint_token(&self) -> String {
        self.current
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

/// Plugin that installs the built-in history hinter into the registry.
pub struct AutosuggestPlugin;

impl brish_plugin::Plugin for AutosuggestPlugin {
    fn name(&self) -> &str {
        "brish-autosuggest"
    }

    fn install(&self, reg: &mut brish_plugin::Registry) {
        struct F;
        impl brish_plugin::HinterFactory for F {
            fn create(&self) -> Box<dyn reedline::Hinter> {
                Box::new(BrishHinter::default())
            }
        }
        reg.hinter_factories.push(Box::new(F));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reedline::{FileBackedHistory, HistoryItem};

    fn history_with(entries: &[&str]) -> FileBackedHistory {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut hist = FileBackedHistory::with_file(100, dir.path().join("hist")).expect("history");
        for e in entries {
            hist.save(HistoryItem::from_command_line((*e).to_string()))
                .expect("save");
        }
        // Keep the tempdir alive for the lifetime of the history by
        // leaking it (tests only).
        std::mem::forget(dir);
        hist
    }

    #[test]
    fn suggests_at_line_end_only() {
        let h = history_with(&["git status", "git push --force"]);
        let mut hinter = BrishHinter::default();
        assert_eq!(hinter.handle("git ", 4, &h, false, "/"), "push --force");
        assert_eq!(hinter.complete_hint(), "push --force");
        // mid-line: no hint, and the stale one is cleared
        assert_eq!(hinter.handle("git ", 2, &h, false, "/"), "");
        assert_eq!(hinter.complete_hint(), "");
        // no match
        assert_eq!(hinter.handle("zzz", 3, &h, false, "/"), "");
    }

    #[test]
    fn next_hint_token_is_first_word() {
        let h = history_with(&["git push origin main"]);
        let mut hinter = BrishHinter::default();
        hinter.handle("git ", 4, &h, false, "/");
        assert_eq!(hinter.next_hint_token(), "push");
    }

    #[test]
    fn empty_line_never_hints() {
        let h = history_with(&["git status"]);
        let mut hinter = BrishHinter::default();
        assert_eq!(hinter.handle("", 0, &h, false, "/"), "");
    }
}
