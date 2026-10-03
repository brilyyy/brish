//! `$PATH` search for external commands (shared by `command`/`type` and
//! the Phase 4 exec engine).

use std::path::{Path, PathBuf};

/// Default PATH when unset (`POSIX confstr` approximation).
pub const DEFAULT_PATH: &str = "/usr/bin:/bin";

/// Find `cmd` on `path_var` (colon-separated; `None` → [`DEFAULT_PATH`]).
/// Names containing `/` are checked directly. Only executable regular
/// files match.
pub fn find_in_path(cmd: &str, path_var: Option<&str>) -> Option<PathBuf> {
    if cmd.contains('/') {
        let p = Path::new(cmd);
        return is_exec(p).then(|| p.to_path_buf());
    }
    let path = path_var.unwrap_or(DEFAULT_PATH);
    for dir in path.split(':') {
        // Empty component = current directory (POSIX).
        let dir = if dir.is_empty() { "." } else { dir };
        let candidate = Path::new(dir).join(cmd);
        if is_exec(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn is_exec(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_path() {
        assert!(find_in_path("/definitely/not/there", None).is_none());
        // A directory is not executable-command material.
        assert!(find_in_path("/tmp", None).is_none());
    }

    #[test]
    fn not_on_path() {
        assert!(find_in_path("definitely-not-a-command-xyz", None).is_none());
        assert!(find_in_path("sh", Some("")).is_none());
    }

    #[test]
    fn finds_sh() {
        // /bin/sh exists on any Unix CI host.
        let found = find_in_path("sh", Some("/bin:/usr/bin"));
        assert!(found.is_some(), "sh should be on /bin:/usr/bin");
    }
}
