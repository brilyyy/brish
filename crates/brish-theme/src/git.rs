//! `git-prompt` — flagship [`PromptSegment`](brish_plugin_api::PromptSegment)
//! plugin: robbyrussell-style ` git:(branch) *` after the cwd.
//!
//! Fast path (plan: plugins must be fast):
//! - only spawns `git` when an ancestor holds `.git` (dir or worktree
//!   file) — zero process cost outside repositories;
//! - successful summaries are cached per cwd for 1s, so a prompt storm
//!   (tab-complete redraws) costs one stat, not one spawn.

use brish_plugin_api::{Plugin, PromptSegment, color_enabled};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const RESET: &str = "\x1b[0m";
/// ponytail: 1s TTL, no invalidation on git events — raise precision
/// (invalidate on exec of git/mutate hooks) only if staleness bites.
const CACHE_TTL: Duration = Duration::from_secs(1);
/// ponytail: walk-up capped at 64 ancestors (deep symlink loops).
const MAX_ANCESTORS: usize = 64;

/// Parsed `git status --porcelain=v1 --branch` first block.
#[derive(Debug, PartialEq)]
struct Summary {
    branch: String,
    ahead: Option<usize>,
    behind: Option<usize>,
    dirty: bool,
}

impl Summary {
    /// `main`, `main +2`, `HEAD detached` — the text inside `git:(…)`.
    fn text(&self) -> String {
        let mut s = self.branch.clone();
        if let Some(n) = self.ahead {
            s.push_str(&format!(" +{n}"));
        }
        if let Some(n) = self.behind {
            s.push_str(&format!(" -{n}"));
        }
        s
    }
}

/// Parse porcelain stdout. `None` when there is no branch line
/// (non-repository output, empty, garbage).
fn git_summary(stdout: &str) -> Option<Summary> {
    let mut lines = stdout.lines();
    let first = lines.next()?;
    let head = first.strip_prefix("## ")?;
    if head.starts_with("HEAD (no branch)") {
        let dirty = lines.any(|l| !l.trim().is_empty());
        return Some(Summary {
            branch: "HEAD detached".to_string(),
            ahead: None,
            behind: None,
            dirty,
        });
    }
    // `main...origin/main [ahead 1, behind 2]`
    let (branch, tracking) = match head.split_once("...") {
        Some((b, t)) => (b, Some(t)),
        None => (head, None),
    };
    let mut ahead = None;
    let mut behind = None;
    if let Some(t) = tracking
        && let Some(bra) = t
            .rsplit_once(" [")
            .map(|(_, b)| b)
            .and_then(|b| b.strip_suffix(']'))
    {
        for part in bra.split(", ") {
            match part.strip_prefix("ahead ") {
                Some(n) => ahead = n.parse().ok(),
                None => {
                    if let Some(n) = part.strip_prefix("behind ") {
                        behind = n.parse().ok();
                    }
                }
            }
        }
    }
    let dirty = lines.any(|l| !l.trim().is_empty());
    Some(Summary {
        branch: branch.to_string(),
        ahead,
        behind,
        dirty,
    })
}

/// True when `cwd` or an ancestor holds a `.git` entry (dir for normal
/// repos, file for worktrees/submodules).
fn has_git_root(cwd: &Path) -> bool {
    let mut cur = Some(cwd);
    for _ in 0..MAX_ANCESTORS {
        let Some(d) = cur else { break };
        if d.join(".git").exists() {
            return true;
        }
        cur = d.parent();
    }
    false
}

/// One `git status` invocation; `None` outside repos or on failure.
fn query(cwd: &Path) -> Option<Summary> {
    if !has_git_root(cwd) {
        return None;
    }
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["status", "--porcelain=v1", "--branch"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    git_summary(&String::from_utf8_lossy(&out.stdout))
}

fn render_summary(s: &Summary) -> String {
    let text = s.text();
    let star = if s.dirty { "*" } else { "" };
    if color_enabled() {
        format!(" {YELLOW}git:({text}){RESET}{RED}{star}{RESET}")
    } else {
        format!(" git:({text}){star}")
    }
}

/// Catalog plugin `git-prompt`.
#[derive(Default)]
pub struct GitPrompt {
    /// `(cwd, queried_at, rendered)` — see [`CACHE_TTL`].
    cache: Mutex<Option<(PathBuf, Instant, Option<String>)>>,
}

impl GitPrompt {
    fn summary(&self, cwd: &Path) -> Option<String> {
        let mut slot = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((ref c, at, ref v)) = *slot
            && c == cwd
            && at.elapsed() < CACHE_TTL
        {
            return v.clone();
        }
        let v = query(cwd).map(|s| render_summary(&s));
        *slot = Some((cwd.to_path_buf(), Instant::now(), v.clone()));
        v
    }
}

impl PromptSegment for GitPrompt {
    fn render(&self, _status: i32, cwd: &Path) -> Option<String> {
        self.summary(cwd)
    }
}

impl Plugin for GitPrompt {
    fn name(&self) -> &str {
        "brish-git"
    }

    fn install(&self, reg: &mut brish_plugin_api::Registry) {
        // Re-install shares this instance's cache via the catalog's Box;
        // a fresh install would just start cold — acceptable.
        reg.prompt_segments.push(Box::new(Self::default()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_clean_branch() {
        let s = git_summary("## main\n").expect("summary");
        assert_eq!(s.branch, "main");
        assert_eq!(s.ahead, None);
        assert!(!s.dirty);
        assert_eq!(s.text(), "main");
    }

    #[test]
    fn parses_tracking_ahead_behind_dirty() {
        let s = git_summary("## main...origin/main [ahead 2, behind 1]\n M file\n?? new\n")
            .expect("summary");
        assert_eq!(s.branch, "main");
        assert_eq!(s.ahead, Some(2));
        assert_eq!(s.behind, Some(1));
        assert!(s.dirty);
        assert_eq!(s.text(), "main +2 -1");
    }

    #[test]
    fn parses_detached_head() {
        let s = git_summary("## HEAD (no branch)\n").expect("summary");
        assert_eq!(s.branch, "HEAD detached");
        assert!(!s.dirty);
        let s = git_summary("## HEAD (no branch)\nM x\n").expect("summary");
        assert!(s.dirty);
    }

    #[test]
    fn no_branch_line_is_none() {
        assert_eq!(git_summary(""), None);
        assert_eq!(git_summary("nothing to commit\n"), None);
    }

    #[test]
    fn renders_without_color_codes_when_no_color() {
        // color_enabled() reads the real env — assert the plain shape
        // only when colors are off, the colored shape otherwise.
        let s = Summary {
            branch: "main".into(),
            ahead: Some(1),
            behind: None,
            dirty: true,
        };
        let out = render_summary(&s);
        if color_enabled() {
            assert!(out.contains("main +1"), "{out:?}");
            assert!(out.contains('*'), "{out:?}");
            assert!(out.contains(YELLOW), "{out:?}");
        } else {
            assert_eq!(out, " git:(main +1)*");
        }
    }

    #[test]
    fn git_root_walks_ancestors() {
        let tmp = tempfile::tempdir().expect("tmpdir");
        let deep = tmp.path().join("a/b");
        std::fs::create_dir_all(&deep).expect("mkdir");
        assert!(!has_git_root(&deep), "no .git yet");
        std::fs::write(tmp.path().join(".git"), "gitdir: x").expect("git file");
        assert!(has_git_root(&deep), "ancestor .git file (worktree)");
    }

    #[test]
    fn cache_serves_same_cwd_within_ttl() {
        let p = GitPrompt::default();
        let cwd = std::path::Path::new("/definitely/not/a/repo");
        {
            let mut slot = p.cache.lock().unwrap_or_else(|e| e.into_inner());
            *slot = Some((cwd.to_path_buf(), Instant::now(), Some("cached".into())));
        }
        assert_eq!(p.summary(cwd).as_deref(), Some("cached"));
        // different cwd misses the cache (and re-queries → None here)
        let other = std::path::Path::new("/also/not/a/repo");
        assert_eq!(p.summary(other), None);
    }
}
