//! The literal `~/.config/brish` path family (project rule: same path
//! on every platform, `dirs::home_dir()` based). Central home so the
//! engine's `plugin` builtin and the binary agree on one root.

use std::path::{Path, PathBuf};

/// `~/.config/brish`.
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/brish")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn rc_path() -> PathBuf {
    config_dir().join(".brishrc")
}

pub fn history_path() -> PathBuf {
    config_dir().join(".brish_history")
}

/// `~/.config/brish/z` — frecency database for `z` builtin.
pub fn z_path() -> PathBuf {
    config_dir().join("z")
}

/// `~/.config/brish/plugins` — one directory per installed store plugin.
pub fn plugins_dir() -> PathBuf {
    config_dir().join("plugins")
}

/// `~/.config/brish/index` — cached clone of the plugin index repo.
pub fn index_cache_dir() -> PathBuf {
    config_dir().join("index")
}

/// `~/.config/brish/.update_check` — mtime-based stamp for update checks.
pub fn update_stamp_path() -> PathBuf {
    config_dir().join(".update_check")
}

/// `~/.config/brish/.first_startup` — marker that first-run banner shown.
pub fn startup_stamp_path() -> PathBuf {
    config_dir().join(".first_startup")
}

/// Create `~/.config/brish` with mode `0700`. Existing dirs get
/// tightened when group/world-accessible (history/config live here).
pub fn ensure_config_dir() -> std::io::Result<PathBuf> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)?;
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&dir)?.permissions().mode();
        if mode & 0o077 != 0 {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(dir)
}

/// Create/overwrite `path` with mode `0600`; tighten pre-existing
/// files that are group/world-accessible. Used for history, config,
/// and the `z` database (commands and paths may hold secrets).
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
    f.write_all(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn write_private_is_0600_and_tightens() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("hist");
        write_private(&f, b"a").unwrap();
        assert_eq!(mode(&f), 0o600);
        // pre-existing wide file gets tightened on rewrite
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&f, b"b").unwrap();
        assert_eq!(mode(&f), 0o600);
        assert_eq!(std::fs::read(&f).unwrap(), b"b");
    }
}
