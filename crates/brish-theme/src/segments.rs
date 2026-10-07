//! Shared plumbing for the environment-reading prompt segments
//! (`venv`, `aws`, `kubectx`, `docker`).
//!
//! All four read cheap ambient state (env vars, one small file) and must
//! stay non-blocking for the prompt: no subprocess, and one lookup per
//! render even when a tab-complete storm redraws the same cwd many times.
//! [`Ttl`] gives them that second guarantee in one place.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// ponytail: fixed 1s TTL, no event-driven invalidation — bump
/// precision (invalidate on chdir/exec) only if staleness is visible.
pub(crate) const TTL: Duration = Duration::from_secs(1);

/// Last computed value for one cwd, plus when it was computed.
type Entry = (PathBuf, Instant, Option<String>);

/// One-entry memo keyed by cwd, expiring after [`TTL`].
pub(crate) struct Ttl {
    slot: Mutex<Option<Entry>>,
}

impl Ttl {
    pub(crate) fn new() -> Self {
        Self {
            slot: Mutex::new(None),
        }
    }

    /// Cached value for `cwd`, else `compute()`'s result (also cached).
    pub(crate) fn get(
        &self,
        cwd: &Path,
        compute: impl FnOnce() -> Option<String>,
    ) -> Option<String> {
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((c, at, v)) = slot.as_ref()
            && c == cwd
            && at.elapsed() < TTL
        {
            return v.clone();
        }
        let v = compute();
        *slot = Some((cwd.to_path_buf(), Instant::now(), v.clone()));
        v
    }
}

/// First non-empty value among `names`, read from the environment.
pub(crate) fn env_first(names: &[&str]) -> Option<String> {
    names
        .iter()
        .filter_map(std::env::var_os)
        .map(|v| v.to_string_lossy().into_owned())
        .find(|v| !v.trim().is_empty())
}

/// Wrap `text` in an SGR color, or leave it bare when color is off.
pub(crate) fn paint(sgr: &str, text: impl AsRef<str>) -> String {
    if brish_plugin_api::color_enabled() {
        format!("\x1b[{sgr}m{}\x1b[0m", text.as_ref())
    } else {
        text.as_ref().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_per_cwd() {
        let t = Ttl::new();
        let a = std::env::temp_dir().join("ttl-a");
        let b = std::env::temp_dir().join("ttl-b");
        let calls = std::cell::Cell::new(0);
        let compute = || {
            calls.set(calls.get() + 1);
            Some(format!("v{}", calls.get()))
        };
        assert_eq!(t.get(&a, compute).as_deref(), Some("v1"));
        assert_eq!(
            t.get(&a, compute).as_deref(),
            Some("v1"),
            "second hit cached"
        );
        assert_eq!(calls.get(), 1, "compute ran once");
        assert_eq!(
            t.get(&b, compute).as_deref(),
            Some("v2"),
            "other cwd recomputes"
        );
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn caches_none_without_recomputing() {
        let t = Ttl::new();
        let cwd = std::env::temp_dir();
        let calls = std::cell::Cell::new(0);
        let compute = || {
            calls.set(calls.get() + 1);
            None
        };
        assert_eq!(t.get(&cwd, compute), None);
        assert_eq!(t.get(&cwd, compute), None);
        assert_eq!(calls.get(), 1, "absent value is cached too");
    }

    #[test]
    fn env_first_skips_unset_and_blank() {
        // SAFETY: test-scoped, single-threaded here.
        unsafe {
            std::env::remove_var("BRISH_TTL_PROBE_A");
            std::env::remove_var("BRISH_TTL_PROBE_B");
        }
        assert_eq!(env_first(&["BRISH_TTL_PROBE_A", "BRISH_TTL_PROBE_B"]), None);
        // SAFETY: as above.
        unsafe { std::env::set_var("BRISH_TTL_PROBE_A", "   ") };
        // SAFETY: as above.
        unsafe { std::env::set_var("BRISH_TTL_PROBE_B", "second") };
        assert_eq!(
            env_first(&["BRISH_TTL_PROBE_A", "BRISH_TTL_PROBE_B"]).as_deref(),
            Some("second"),
            "blank is skipped"
        );
        // SAFETY: cleanup.
        unsafe {
            std::env::remove_var("BRISH_TTL_PROBE_A");
            std::env::remove_var("BRISH_TTL_PROBE_B");
        }
    }

    #[test]
    fn paint_honors_color_setting() {
        let expected = if brish_plugin_api::color_enabled() {
            "\x1b[35mmag\x1b[0m".to_string()
        } else {
            "mag".to_string()
        };
        assert_eq!(paint("35", "mag"), expected);
    }
}
