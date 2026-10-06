//! `HISTCONTROL` support for the interactive history backend.
//!
//! reedline's `FileBackedHistory` already suppresses *consecutive*
//! duplicates (bash `ignoredups` semantics, always on). This wrapper
//! adds `erasedups` / `ignoreboth`: when enabled, saving a line that
//! appears earlier rebuilds the store without the old copies (bash
//! keeps only the newest occurrence, at the end).
//!
//! ponytail: rebuild uses `clear()` (drops the file) + re-save — a
//! crash between those loses history that session. Ceiling: in-place
//! splice if that ever bites.

use reedline::{
    FileBackedHistory, History, HistoryItem, HistoryItemId, HistorySessionId, Result,
    SearchDirection, SearchQuery,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Shared snapshot of `HISTCONTROL`, refreshed by the REPL each prompt
/// (same pattern as the highlighter's alias snapshot).
#[derive(Default)]
pub struct HistControl {
    pub erasedups: AtomicBool,
}

impl HistControl {
    /// Parse a `HISTCONTROL` value: `ignoredups`, `erasedups`,
    /// `ignoreboth`, comma/colon separated. Ignoredups is free
    /// (consecutive dedup is built in) — we only flip `erasedups`.
    pub fn set_from_value(&self, v: Option<&str>) {
        let on = v.is_some_and(|s| {
            s.split([',', ':'])
                .any(|p| p == "erasedups" || p == "ignoreboth")
        });
        self.erasedups.store(on, Ordering::Relaxed);
    }

    pub fn erasedups(&self) -> bool {
        self.erasedups.load(Ordering::Relaxed)
    }
}

/// `FileBackedHistory` + `HISTCONTROL=erasedups`.
pub struct BrishHistory {
    inner: FileBackedHistory,
    ctl: Arc<HistControl>,
}

impl BrishHistory {
    pub fn new(inner: FileBackedHistory, ctl: Arc<HistControl>) -> Self {
        Self { inner, ctl }
    }
}

impl History for BrishHistory {
    fn save(&mut self, h: HistoryItem) -> Result<HistoryItem> {
        if self.ctl.erasedups() && !h.command_line.is_empty() {
            let all = self
                .inner
                .search(SearchQuery::everything(SearchDirection::Forward, None))?;
            if all.iter().any(|i| i.command_line == h.command_line) {
                let keep: Vec<String> = all
                    .iter()
                    .filter(|i| i.command_line != h.command_line)
                    .map(|i| i.command_line.clone())
                    .collect();
                // Rebuild without any prior copy of this line; the new
                // save below lands it at the end (bash order).
                self.inner.clear()?;
                for line in keep {
                    self.inner.save(HistoryItem::from_command_line(line))?;
                }
                return self.inner.save(h);
            }
        }
        self.inner.save(h)
    }

    fn load(&self, id: HistoryItemId) -> Result<HistoryItem> {
        self.inner.load(id)
    }

    fn count(&self, query: SearchQuery) -> Result<i64> {
        self.inner.count(query)
    }

    fn search(&self, query: SearchQuery) -> Result<Vec<HistoryItem>> {
        self.inner.search(query)
    }

    fn update(
        &mut self,
        id: HistoryItemId,
        updater: &dyn Fn(HistoryItem) -> HistoryItem,
    ) -> Result<()> {
        self.inner.update(id, updater)
    }

    fn clear(&mut self) -> Result<()> {
        self.inner.clear()
    }

    fn delete(&mut self, h: HistoryItemId) -> Result<()> {
        self.inner.delete(h)
    }

    fn sync(&mut self) -> std::io::Result<()> {
        self.inner.sync()
    }

    fn session(&self) -> Option<HistorySessionId> {
        self.inner.session()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(cmd: &str) -> HistoryItem {
        HistoryItem::from_command_line(cmd)
    }

    #[test]
    fn hystcontrol_parsing() {
        let c = HistControl::default();
        c.set_from_value(Some("ignoredups"));
        assert!(!c.erasedups(), "ignoredups alone is built-in, no flag");
        c.set_from_value(Some("erasedups"));
        assert!(c.erasedups());
        c.set_from_value(Some("ignoreboth"));
        assert!(c.erasedups());
        c.set_from_value(Some("ignoredups,erasedups"));
        assert!(c.erasedups());
        c.set_from_value(Some("ignoredups:erasedups"));
        assert!(c.erasedups());
        c.set_from_value(None);
        assert!(!c.erasedups());
    }

    #[test]
    fn erasedups_keeps_only_the_newest_copy() {
        let ctl = Arc::new(HistControl::default());
        ctl.erasedups.store(true, Ordering::Relaxed);
        let mut h = BrishHistory::new(FileBackedHistory::default(), ctl);
        h.save(item("ls")).unwrap();
        h.save(item("cd /tmp")).unwrap();
        h.save(item("ls")).unwrap(); // dup → rebuild
        let all = h
            .search(SearchQuery::everything(SearchDirection::Forward, None))
            .unwrap();
        let lines: Vec<&str> = all.iter().map(|i| i.command_line.as_str()).collect();
        // kept order for non-dups; `ls` only once, at the end
        assert_eq!(lines, vec!["cd /tmp", "ls"]);
    }

    #[test]
    fn off_saves_duplicates_like_plain_history() {
        let ctl = Arc::new(HistControl::default());
        let mut h = BrishHistory::new(FileBackedHistory::default(), ctl);
        h.save(item("ls")).unwrap();
        h.save(item("ls")).unwrap();
        // FileBacked still collapses *consecutive* dups itself
        let all = h
            .search(SearchQuery::everything(SearchDirection::Forward, None))
            .unwrap();
        assert_eq!(all.len(), 1);
    }
}
