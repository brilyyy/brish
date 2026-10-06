//! `z` frecency store: a plain text file of `<visits>\t<epoch>\t<path>`.
//!
//! Format is one line per directory so `sort`, `grep`, and friends work
//! on it directly. Records are read whole, mutated in memory, written
//! back atomically (temp file + rename) — good enough for shell scale.
//!
//! ponytail: no locking beyond atomic replace; last concurrent writer wins.
//! Add a lockfile when two interactive shells visibly clobber the db.

use std::path::Path;

/// One directory's visit count and last-visit epoch seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub path: String,
    pub visits: u64,
    pub epoch: u64,
}

/// Zero for this process (sec resolution); tests pass explicit values.
pub fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Read every well-formed line, skipping blanks and malformed rows.
pub fn load(db: &Path) -> Vec<Row> {
    let Ok(text) = std::fs::read_to_string(db) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\t');
            let visits = it.next()?.parse().ok()?;
            let epoch = it.next()?.parse().ok()?;
            Some(Row {
                path: it.next()?.to_string(),
                visits,
                epoch,
            })
        })
        .filter(|r| !r.path.is_empty())
        .collect()
}

/// Write all `rows`, best first (keeps the file human-scannable).
fn save(db: &Path, rows: &[Row]) {
    if let Some(parent) = db.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut rows: Vec<&Row> = rows.iter().collect();
    let now = now_epoch();
    rows.sort_by_key(|r| std::cmp::Reverse(score(r, now)));
    let text: String = rows
        .iter()
        .map(|r| format!("{}\t{}\t{}\n", r.visits, r.epoch, r.path))
        .collect();
    // Atomic: temp + rename so a crash never half-writes the db.
    let tmp = db.with_extension("tmp");
    if std::fs::write(&tmp, text).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, db);
}

/// Record a visit to `dir` at `epoch` (cwd should be absolute + normalized
/// by the caller so `/a` and `/a/` merge). Missing db is created.
pub fn record(db: &Path, dir: &str, epoch: u64) {
    if dir.is_empty() {
        return;
    }
    let mut rows = load(db);
    match rows.iter_mut().find(|r| r.path == dir) {
        Some(r) => {
            r.visits = r.visits.saturating_add(1);
            r.epoch = epoch;
        }
        None => rows.push(Row {
            path: dir.to_string(),
            visits: 1,
            epoch,
        }),
    }
    save(db, &rows);
}

/// Frecency score: visits decayed by age. Recent wins; visits break ties.
fn score(r: &Row, now: u64) -> u64 {
    let age_days = now.saturating_sub(r.epoch) / 86_400;
    // +1-day guard: a visit this minute still outranks ancient count.
    r.visits.saturating_mul(100).saturating_div(age_days + 1)
}

/// Rows sorted best-first, filtered to `query`'s substring match
/// (case-insensitive on the full path; `None` = all).
pub fn rank(rows: &[Row], query: Option<&str>) -> Vec<Row> {
    let now = now_epoch();
    let mut out: Vec<Row> = match query {
        None => rows.to_vec(),
        Some(q) => {
            let needle = q.to_lowercase();
            rows.iter()
                .filter(|r| r.path.to_lowercase().contains(&needle))
                .cloned()
                .collect()
        }
    };
    out.sort_by_key(|r| std::cmp::Reverse(score(r, now)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::CWD_LOCK;
    use std::path::PathBuf;
    fn tmp_db(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("brish-z-test-{}-{}", std::process::id(), tag))
    }

    #[test]
    fn record_and_rank_roundtrip() {
        let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let db = tmp_db("roundtrip");
        let _ = std::fs::remove_file(&db);
        // Distinct epochs so decay orders deterministically.
        record(&db, "/b/old", 1_000);
        record(&db, "/a/new", 2_000);
        record(&db, "/b/old", 2_001);
        let rows = load(&db);
        assert_eq!(rows.len(), 2);
        let old = rows.iter().find(|r| r.path == "/b/old").unwrap();
        assert_eq!((old.visits, old.epoch), (2, 2_001));
        let ranked = rank(&rows, Some("a/"));
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].path, "/a/new");
        let _ = std::fs::remove_file(&db);
    }

    #[test]
    fn malformed_lines_skipped() {
        let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let db = tmp_db("malformed");
        std::fs::write(&db, "junk\n3\tnotnum\t/x\n2\t5\t/ok\n\n").unwrap();
        let rows = load(&db);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].path, "/ok");
        let _ = std::fs::remove_file(&db);
    }
}
