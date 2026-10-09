//! Startup update check + self-update (oh-my-zsh style prompt).

use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

const RELEASES_LATEST_URL: &str = "https://github.com/brilyyy/brish/releases/latest";
const RELEASE_ASSET_BASE: &str = "https://github.com/brilyyy/brish/releases/download";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Parse a version string like "v0.3.0" or "0.3.0" into (major, minor, patch).
fn parse_version(s: &str) -> Option<(u64, u64, u64)> {
    let s = s.trim().strip_prefix('v').unwrap_or(s.trim());
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// True if `tag` represents a newer version than `CURRENT_VERSION`.
pub fn is_newer(tag: &str) -> bool {
    parse_version(tag).zip(parse_version(CURRENT_VERSION)).is_some_and(|(t, c)| t > c)
}

/// Extract tag from GitHub's effective URL after following redirect.
/// e.g. "https://github.com/brilyyy/brish/releases/tag/v0.3.0" -> "v0.3.0"
fn tag_from_effective_url(url: &str) -> Option<String> {
    url.strip_prefix("https://github.com/brilyyy/brish/releases/tag/")
        .map(|s| s.to_string())
}

/// Fetch the latest release tag from GitHub via the redirect trick.
/// Uses `curl --max-time 3 -fsSL -o /dev/null -w '%{url_effective}'`.
pub fn latest_tag() -> Option<String> {
    let output = Command::new("curl")
        .args([
            "--max-time",
            "3",
            "-fsSL",
            "-o",
            "/dev/null",
            "-w",
            "%{url_effective}",
            RELEASES_LATEST_URL,
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    tag_from_effective_url(&url)
}

/// Current install target triple (e.g. "x86_64-unknown-linux-gnu").
fn target_triple() -> String {
    let arch = std::env::consts::ARCH;
    let os = match std::env::consts::OS {
        "linux" => "unknown-linux-gnu",
        "macos" => "apple-darwin",
        other => other,
    };
    format!("{arch}-{os}")
}

/// Asset filename for a given tag and target.
fn asset_name(tag: &str) -> String {
    format!("brish-{tag}-{}.tar.gz", target_triple())
}

/// Path to the update check stamp file.
fn stamp_path() -> std::path::PathBuf {
    brish_builtin::paths::config_dir().join(".update_check")
}

/// Whether a new check is due (stamp missing or older than `interval_days`).
/// Accepts an explicit stamp path for testing.
fn check_due_impl(stamp: &Path, interval_days: u64) -> bool {
    let meta = match fs::metadata(stamp) {
        Ok(m) => m,
        Err(_) => return true, // no stamp -> due
    };
    let modified = match meta.modified() {
        Ok(t) => t,
        Err(_) => return true,
    };
    let elapsed = match SystemTime::now().duration_since(modified) {
        Ok(d) => d,
        Err(_) => return true,
    };
    elapsed >= Duration::from_secs(interval_days * 86400)
}

/// Whether a new check is due (stamp missing or older than `interval_days`).
fn check_due(interval_days: u64) -> bool {
    check_due_impl(&stamp_path(), interval_days)
}

/// Touch the stamp file (create or update mtime).
fn touch_stamp() {
    let stamp = stamp_path();
    if let Some(parent) = stamp.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&stamp, "");
}

/// Read a single keypress from stdin (tty). Returns true for Y/y/Enter/empty.
fn read_yes(prompt: &str) -> bool {
    print!("{prompt}");
    io::stdout().flush().ok();
    let mut buf = String::new();
    match io::stdin().read_line(&mut buf) {
        Ok(_) => {
            let c = buf.trim().chars().next().unwrap_or('n');
            matches!(c, 'y' | 'Y' | '\n' | '\0')
        }
        Err(_) => false,
    }
}

/// Print colored text if stdout is a terminal.
fn print_color(color: &str, text: &str) {
    if io::stdout().is_terminal() {
        println!("\x1b[{color}m{text}\x1b[0m");
    } else {
        println!("{text}");
    }
}

/// Run the interactive update prompt (blocking, synchronous).
/// Returns true if update was performed, false if declined/skipped.
fn prompt_update(tag: &str) -> bool {
    let current = CURRENT_VERSION;
    print_color("1;33", &format!("briSH {tag} is available (you have {current})."));
    if !read_yes("Would you like to update now? [Y/n] ") {
        print_color("0;36", "Run `brish --self-update` anytime to update.");
        return false;
    }
    match self_update(tag) {
        Ok(_) => {
            touch_stamp();
            print_color("1;32", "Update successful. Restart shell to take effect.");
            true
        }
        Err(e) => {
            eprintln!("Update failed: {e}");
            false
        }
    }
}

/// Self-update: download asset, verify sha256, extract, replace current exe.
pub fn self_update(tag: &str) -> Result<(), String> {
    let asset = asset_name(tag);
    let url = format!("{RELEASE_ASSET_BASE}/{tag}/{asset}");
    let sha_url = format!("{url}.sha256");

    let tmp = std::env::temp_dir().join(format!("brish-update-{}", std::process::id()));
    fs::create_dir_all(&tmp).map_err(|e| format!("create temp dir: {e}"))?;

    // Download asset
    let asset_path = tmp.join(&asset);
    let asset_path_str = asset_path.to_str().ok_or("asset path not valid UTF-8")?;
    let status = Command::new("curl")
        .args(["-fsSL", "-o", asset_path_str, &url])
        .status()
        .map_err(|e| format!("curl: {e}"))?;
    if !status.success() {
        return Err(format!("download failed: {status}"));
    }

    // Download sha256
    let sha_path = tmp.join(format!("{asset}.sha256"));
    let sha_path_str = sha_path.to_str().ok_or("sha path not valid UTF-8")?;
    let status = Command::new("curl")
        .args(["-fsSL", "-o", sha_path_str, &sha_url])
        .status()
        .map_err(|e| format!("curl sha: {e}"))?;
    if !status.success() {
        return Err("sha256 download failed".into());
    }

    // Verify sha256
    let sha_cmd = if Command::new("sha256sum").arg("-c").stdin(Stdio::null()).status().is_ok() {
        "sha256sum"
    } else {
        "shasum"
    };
    let sha_file = sha_path.file_name().ok_or("sha path has no file name")?;
    let sha_file_str = sha_file.to_str().ok_or("sha file name not valid UTF-8")?;
    let status = Command::new(sha_cmd)
        .args(["-a", "256", "-c"])
        .arg(sha_file_str)
        .current_dir(&tmp)
        .status()
        .map_err(|e| format!("sha256sum: {e}"))?;
    if !status.success() {
        return Err("sha256 mismatch".into());
    }

    // Extract
    let asset_file = asset_path.file_name().ok_or("asset path has no file name")?;
    let asset_file_str = asset_file.to_str().ok_or("asset file name not valid UTF-8")?;
    let status = Command::new("tar")
        .args(["-xzf", asset_file_str])
        .current_dir(&tmp)
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    if !status.success() {
        return Err("extract failed".into());
    }

    // Replace current exe
    let current_exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let new_exe = tmp.join("brish");
    if !new_exe.exists() {
        return Err("extracted binary not found".into());
    }
    let tmp_exe = current_exe.with_extension("brish-new");
    fs::rename(&new_exe, &tmp_exe).map_err(|e| format!("rename to tmp: {e}"))?;
    fs::rename(&tmp_exe, &current_exe).map_err(|e| format!("atomic replace: {e}"))?;

    // Cleanup
    let _ = fs::remove_dir_all(&tmp);
    Ok(())
}

/// Called from `repl()` on tty interactive startup.
/// Runs synchronous check + prompt if due and enabled.
pub fn maybe_check_and_prompt(enabled: bool, interval_days: u64) {
    if !enabled || !check_due(interval_days) {
        return;
    }
    // Synchronous check with timeout — only once per interval.
    let Some(tag) = latest_tag() else { return };
    if is_newer(&tag) {
        // Touch stamp after successful network resolution to avoid
        // retry storms on failure. If user declines or update fails,
        // we touch stamp in those paths. If we hit timeout/network
        // error here, we don't touch -> retry next startup.
        touch_stamp();
        let _ = prompt_update(&tag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use filetime::{set_file_times, FileTime};
    use std::time::SystemTime;

    #[test]
    fn parse_version_basic() {
        let r1 = parse_version("v0.2.0");
        let r2 = parse_version("0.3.0");
        let r3 = parse_version("v1.0.0");
        eprintln!("r1={r1:?} r2={r2:?} r3={r3:?}");
        assert_eq!(r1, Some((0, 2, 0)));
        assert_eq!(r2, Some((0, 3, 0)));
        assert_eq!(r3, Some((1, 0, 0)));
    }

    #[test]
    fn parse_version_invalid() {
        assert_eq!(parse_version(""), None);
        assert_eq!(parse_version("v0"), None);
        assert_eq!(parse_version("0.2"), None);
        assert_eq!(parse_version("v0.2.x"), None);
    }

    #[test]
    fn is_newer_works() {
        assert!(is_newer("v0.3.0")); // 0.3.0 > 0.2.0
        assert!(!is_newer("v0.1.0"));
        assert!(!is_newer("v0.2.0"));
    }

    #[test]
    fn tag_from_effective_url_works() {
        assert_eq!(
            tag_from_effective_url("https://github.com/brilyyy/brish/releases/tag/v0.3.0"),
            Some("v0.3.0".into())
        );
        assert_eq!(
            tag_from_effective_url("https://github.com/brilyyy/brish/releases/tag/v1.0.0"),
            Some("v1.0.0".into())
        );
        assert_eq!(tag_from_effective_url("https://example.com/other"), None);
    }

    #[test]
    fn target_triple_known() {
        let t = target_triple();
        assert!(t.contains("-unknown-linux-gnu") || t.contains("-apple-darwin"));
    }

    #[test]
    fn asset_name_contains_tag_and_target() {
        let name = asset_name("v0.3.0");
        assert!(name.starts_with("brish-v0.3.0-"));
        assert!(name.ends_with(".tar.gz"));
    }

    #[test]
    fn check_due_missing_stamp() {
        let tmp = tempfile::tempdir().unwrap();
        let stamp = tmp.path().join(".update_check");
        // stamp missing -> due
        assert!(check_due_impl(&stamp, 13));
    }

    #[test]
    fn check_due_old_stamp() {
        let tmp = tempfile::tempdir().unwrap();
        let stamp = tmp.path().join(".update_check");
        std::fs::write(&stamp, "").unwrap();
        // set mtime to 14 days ago
        let old = SystemTime::now()
            .checked_sub(Duration::from_secs(14 * 86400))
            .unwrap();
        let ft = FileTime::from_system_time(old);
        set_file_times(&stamp, ft, ft).unwrap();
        assert!(check_due_impl(&stamp, 13));
    }

    #[test]
    fn check_due_fresh_stamp() {
        let tmp = tempfile::tempdir().unwrap();
        let stamp = tmp.path().join(".update_check");
        std::fs::write(&stamp, "").unwrap();
        assert!(!check_due_impl(&stamp, 13));
    }
}