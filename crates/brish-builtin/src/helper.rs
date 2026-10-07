//! Helper subprocess bridge for store plugins (`docs/PLUGINS.md`).
//!
//! One generic runner: spawn argv, capture stdout, enforce a deadline
//! (kill on timeout), never panic. Adapters wrap it as prompt segment,
//! completion provider and pre/post/chdir hooks. A failing helper warns
//! once and does nothing — it can never wedge the shell.

use crate::store::SegmentDecl;
use brish_plugin::{
    CmdCtx, Completion, CompletionCtx, CompletionProvider, HookAction, PreExecHook, PromptSegment,
};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Hook deadline (segment/completion deadlines come from the manifest).
const HOOK_TIMEOUT_MS: u64 = 1_000;

/// Result of a helper run that exited (any exit code) before deadline.
pub struct RunResult {
    pub status: i32,
    pub stdout: String,
}

/// Spawn `argv[0]` with `args`, cwd + optional `BRISH_PLUGIN_DIR`.
/// `None` = spawn failure or timeout (child killed).
pub fn run(
    argv: &[String],
    args: &[String],
    cwd: &Path,
    timeout_ms: u64,
    plugin_dir: Option<&Path>,
) -> Option<RunResult> {
    let mut cmd = std::process::Command::new(argv.first()?);
    cmd.args(&argv[1..])
        .args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    if let Some(dir) = plugin_dir {
        cmd.env("BRISH_PLUGIN_DIR", dir);
    }
    let mut child = cmd.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    // Reader thread so a chatty helper can't fill the pipe and deadlock.
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        match child.try_wait() {
            Ok(Some(code)) => {
                let buf = reader.join().ok()?;
                return Some(RunResult {
                    status: code.code().unwrap_or(-1),
                    stdout: String::from_utf8_lossy(&buf).into_owned(),
                });
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    // ponytail: reader thread detaches here — it ends when
                    // the pipe closes, i.e. when any surviving grandchild
                    // exits. Joining would let one hung helper block the
                    // shell for its full runtime.
                    return None;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = reader.join();
                return None;
            }
        }
    }
}

fn warn_once(flag: &AtomicBool, msg: String) {
    if !flag.swap(true, Ordering::Relaxed) {
        eprintln!("brish: {msg}");
    }
}

/// Prompt segment backed by a command: run, take first line, cache by
/// (cwd, status) for `cache_secs`. Nonzero exit / timeout → skip.
pub struct HelperSegment {
    plugin: String,
    dir: PathBuf,
    cmd: Vec<String>,
    name: String,
    cache_secs: u64,
    timeout_ms: u64,
    warned: AtomicBool,
    cache: Mutex<Option<(PathBuf, i32, Instant, String)>>,
}

impl HelperSegment {
    pub fn new(plugin: &str, dir: PathBuf, decl: &SegmentDecl) -> Self {
        Self {
            plugin: plugin.to_string(),
            dir,
            cmd: decl.cmd.clone(),
            name: decl.name.clone(),
            cache_secs: decl.cache_secs,
            timeout_ms: decl.timeout_ms,
            warned: AtomicBool::new(false),
            cache: Mutex::new(None),
        }
    }
}

impl PromptSegment for HelperSegment {
    fn render(&self, status: i32, cwd: &Path) -> Option<String> {
        if self.cache_secs > 0 {
            let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((c, s, at, text)) = guard.as_ref()
                && c == cwd
                && *s == status
                && at.elapsed() < Duration::from_secs(self.cache_secs)
            {
                return Some(text.clone());
            }
        }
        let args = vec![
            "segment".to_string(),
            "--name".into(),
            self.name.clone(),
            "--cwd".into(),
            cwd.display().to_string(),
            "--status".into(),
            status.to_string(),
        ];
        let Some(out) = run(&self.cmd, &args, cwd, self.timeout_ms, Some(&self.dir)) else {
            warn_once(
                &self.warned,
                format!(
                    "segment `{}` ({}) timed out or failed to start",
                    self.name, self.plugin
                ),
            );
            return None;
        };
        if out.status != 0 {
            warn_once(
                &self.warned,
                format!(
                    "segment `{}` ({}) exited with {status}",
                    self.name, self.plugin
                ),
            );
            return None;
        }
        let text = out
            .stdout
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        if text.is_empty() {
            return None;
        }
        if self.cache_secs > 0 {
            let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            *guard = Some((cwd.to_path_buf(), status, Instant::now(), text.clone()));
        }
        Some(text)
    }
}

/// Completion provider forwarding to `helper complete`.
pub struct HelperCompletion {
    plugin: String,
    cmd: Vec<String>,
    dir: PathBuf,
    timeout_ms: u64,
    warned: AtomicBool,
}

impl HelperCompletion {
    pub fn new(plugin: &str, cmd: Vec<String>, dir: PathBuf, timeout_ms: u64) -> Self {
        Self {
            plugin: plugin.to_string(),
            cmd,
            dir,
            timeout_ms,
            warned: AtomicBool::new(false),
        }
    }
}

impl CompletionProvider for HelperCompletion {
    fn complete(&self, ctx: &CompletionCtx<'_>) -> Vec<Completion> {
        if ctx.after_dollar {
            return Vec::new();
        }
        let mut args = vec![
            "complete".to_string(),
            "--word".into(),
            ctx.word.to_string(),
            "--cwd".into(),
            ctx.cwd.display().to_string(),
            "--before".into(),
            ctx.line_before.to_string(),
        ];
        if ctx.is_command {
            args.push("--command".into());
        }
        let Some(out) = run(&self.cmd, &args, ctx.cwd, self.timeout_ms, Some(&self.dir)) else {
            warn_once(
                &self.warned,
                format!("completion helper ({}) timed out", self.plugin),
            );
            return Vec::new();
        };
        if out.status != 0 {
            warn_once(
                &self.warned,
                format!(
                    "completion helper ({}) exited with {}",
                    self.plugin, out.status
                ),
            );
            return Vec::new();
        }
        out.stdout
            .lines()
            .filter_map(|line| {
                let mut parts = line.split('\t');
                let value = parts.next()?;
                if value.is_empty() {
                    return None;
                }
                let desc = parts.next().filter(|d| !d.is_empty()).map(str::to_string);
                let drop_space = parts.next() == Some("drop");
                Some(Completion {
                    value: value.to_string(),
                    description: desc,
                    keep_typing: drop_space,
                })
            })
            .collect()
    }
}

/// Which hook event a `HelperHook` serves (also the protocol verb).
#[derive(Clone, Copy)]
pub enum HookEvent {
    Pre,
    Post,
    Chdir,
}

/// Hook that runs an external command per event.
///
/// Pre semantics: exit 0 + stdout `abort N` → `Abort(N)`; anything
/// else (failure, timeout, malformed output) → `Continue`. A broken
/// guard must never block the shell.
pub struct HelperHook {
    plugin: String,
    dir: PathBuf,
    cmd: Vec<String>,
    event: HookEvent,
    warned: AtomicBool,
}

impl HelperHook {
    pub fn new(plugin: &str, dir: PathBuf, cmd: Vec<String>, event: HookEvent) -> Self {
        Self {
            plugin: plugin.to_string(),
            dir,
            cmd,
            event,
            warned: AtomicBool::new(false),
        }
    }

    fn verb(&self) -> &'static str {
        match self.event {
            HookEvent::Pre => "pre",
            HookEvent::Post => "post",
            HookEvent::Chdir => "chdir",
        }
    }

    fn fire(&self, args: Vec<String>, cwd: &Path) -> Option<RunResult> {
        let Some(out) = run(&self.cmd, &args, cwd, HOOK_TIMEOUT_MS, Some(&self.dir)) else {
            warn_once(
                &self.warned,
                format!("hook {} ({}) timed out", self.verb(), self.plugin),
            );
            return None;
        };
        Some(out)
    }
}

impl PreExecHook for HelperHook {
    fn before(&self, ctx: &CmdCtx<'_>) -> HookAction {
        let args = vec![
            "hook".to_string(),
            self.verb().into(),
            "--cwd".into(),
            ctx.cwd.display().to_string(),
            "--status".into(),
            ctx.status_before.to_string(),
            "--".into(),
        ];
        let args: Vec<String> = args.into_iter().chain(ctx.argv.iter().cloned()).collect();
        let Some(out) = self.fire(args, ctx.cwd) else {
            return HookAction::Continue;
        };
        if out.status == 0
            && let Some(n) = out
                .stdout
                .trim()
                .strip_prefix("abort ")
                .and_then(|s| s.trim().parse::<i32>().ok())
        {
            return HookAction::Abort(n);
        }
        HookAction::Continue
    }
}

/// Post/chdir hooks need the post/chdir traits — implemented below.
impl brish_plugin::PostExecHook for HelperHook {
    fn after(&self, ctx: &CmdCtx<'_>, status: i32) {
        let args = vec![
            "hook".to_string(),
            self.verb().into(),
            "--cwd".into(),
            ctx.cwd.display().to_string(),
            "--status".into(),
            status.to_string(),
            "--".into(),
        ];
        let args: Vec<String> = args.into_iter().chain(ctx.argv.iter().cloned()).collect();
        let _ = self.fire(args, ctx.cwd);
    }
}

impl brish_plugin::ChdirHook for HelperHook {
    fn on_cd(&self, old: &Path, new: &Path) {
        let args = vec![
            "hook".to_string(),
            self.verb().into(),
            "--cwd".into(),
            new.display().to_string(),
            "--old".into(),
            old.display().to_string(),
        ];
        let _ = self.fire(args, new);
    }
}

/// Resolve `helper keymap` once at startup into binding pairs.
pub fn keymap_pairs(cmd: &[String], dir: &Path, timeout_ms: u64) -> Vec<(String, String)> {
    let Some(out) = run(cmd, &["keymap".to_string()], dir, timeout_ms, Some(dir)) else {
        eprintln!("brish: keymap helper ({}) timed out", dir.display());
        return Vec::new();
    };
    if out.status != 0 {
        eprintln!(
            "brish: keymap helper ({}) exited with {}",
            dir.display(),
            out.status
        );
        return Vec::new();
    }
    out.stdout
        .lines()
        .filter_map(|line| {
            let (k, e) = line.split_once('\t')?;
            if k.is_empty() || e.is_empty() {
                return None;
            }
            Some((k.to_string(), e.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_script(dir: &Path, name: &str, body: &str) -> Vec<String> {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&path).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&path, perm).unwrap();
        vec![path.display().to_string()]
    }

    #[test]
    fn run_captures_stdout_and_status() {
        let tmp = tempfile::tempdir().unwrap();
        let argv = write_script(tmp.path(), "ok.sh", "#!/bin/sh\necho hi\nexit 3\n");
        let out = run(&argv, &[], tmp.path(), 1_000, Some(tmp.path())).unwrap();
        assert_eq!(out.status, 3);
        assert_eq!(out.stdout.trim(), "hi");
    }

    #[test]
    fn run_missing_binary_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let argv = vec![tmp.path().join("nope.sh").display().to_string()];
        assert!(run(&argv, &[], tmp.path(), 200, None).is_none());
    }

    #[test]
    fn run_times_out_and_kills() {
        let tmp = tempfile::tempdir().unwrap();
        let argv = write_script(tmp.path(), "slow.sh", "#!/bin/sh\nsleep 5\n");
        let started = Instant::now();
        let out = run(&argv, &[], tmp.path(), 100, None);
        assert!(out.is_none());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "killed promptly, took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn segment_renders_and_caches() {
        let tmp = tempfile::tempdir().unwrap();
        let counter = tmp.path().join("count");
        let argv = write_script(
            tmp.path(),
            "seg.sh",
            "#!/bin/sh\necho x >> count\necho up\n",
        );
        let decl = SegmentDecl {
            name: "up".into(),
            cmd: argv,
            cache_secs: 60,
            timeout_ms: 10_000,
        };
        let seg = HelperSegment::new("demo", tmp.path().to_path_buf(), &decl);
        let cwd = tmp.path();
        assert_eq!(seg.render(0, cwd).as_deref(), Some("up"));
        assert_eq!(seg.render(0, cwd).as_deref(), Some("up"), "cached");
        let runs = std::fs::read_to_string(&counter).unwrap().lines().count();
        assert_eq!(runs, 1, "second render served from cache");
        // status change busts the cache key
        assert_eq!(seg.render(1, cwd).as_deref(), Some("up"));
        let runs = std::fs::read_to_string(&counter).unwrap().lines().count();
        assert_eq!(runs, 2);
    }

    #[test]
    fn segment_nonzero_exit_skips() {
        let tmp = tempfile::tempdir().unwrap();
        let argv = write_script(tmp.path(), "bad.sh", "#!/bin/sh\necho nope\nexit 1\n");
        let decl = SegmentDecl {
            name: "bad".into(),
            cmd: argv,
            cache_secs: 1,
            timeout_ms: 10_000,
        };
        let seg = HelperSegment::new("demo", tmp.path().to_path_buf(), &decl);
        assert!(seg.render(0, tmp.path()).is_none());
    }

    #[test]
    fn completion_parses_tab_protocol() {
        let tmp = tempfile::tempdir().unwrap();
        let argv = write_script(
            tmp.path(),
            "c.sh",
            "#!/bin/sh\nprintf 'alpha\\tdemo option\\nbeta\\t\\tdrop\\nbare\\n\\n'\n",
        );
        let p = HelperCompletion::new("demo", argv, tmp.path().to_path_buf(), 1_000);
        let ctx = CompletionCtx {
            word: "",
            is_command: false,
            after_dollar: false,
            cwd: tmp.path(),
            line_before: "",
            algorithm: brish_plugin::Algorithm::Prefix,
            match_description: false,
        };
        let out = p.complete(&ctx);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].value, "alpha");
        assert_eq!(out[0].description.as_deref(), Some("demo option"));
        assert!(!out[0].keep_typing);
        assert_eq!(out[1].value, "beta");
        assert!(out[1].keep_typing, "drop flag");
        assert!(out[1].description.is_none());
        assert_eq!(out[2].value, "bare");
    }

    #[test]
    fn completion_nonzero_exit_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let argv = write_script(tmp.path(), "c.sh", "#!/bin/sh\nexit 4\n");
        let p = HelperCompletion::new("demo", argv, tmp.path().to_path_buf(), 1_000);
        let ctx = CompletionCtx {
            word: "",
            is_command: false,
            after_dollar: false,
            cwd: tmp.path(),
            line_before: "",
            algorithm: brish_plugin::Algorithm::Prefix,
            match_description: false,
        };
        assert!(p.complete(&ctx).is_empty());
    }

    fn cmd_ctx<'a>(argv: &'a [String], cwd: &'a Path) -> CmdCtx<'a> {
        CmdCtx {
            argv,
            status_before: 0,
            cwd,
        }
    }

    #[test]
    fn pre_hook_abort_protocol() {
        let tmp = tempfile::tempdir().unwrap();
        let argv = write_script(tmp.path(), "g.sh", "#!/bin/sh\necho 'abort 7'\n");
        let h = HelperHook::new("demo", tmp.path().to_path_buf(), argv, HookEvent::Pre);
        let words = vec!["rm".to_string(), "-rf".into(), "/".into()];
        let ctx = cmd_ctx(&words, tmp.path());
        assert!(matches!(h.before(&ctx), HookAction::Abort(7)));
    }

    #[test]
    fn pre_hook_failure_never_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        // nonzero exit, garbage output
        let argv = write_script(tmp.path(), "g.sh", "#!/bin/sh\necho crash\nexit 9\n");
        let h = HelperHook::new("demo", tmp.path().to_path_buf(), argv, HookEvent::Pre);
        let words = vec!["ls".to_string()];
        let ctx = cmd_ctx(&words, tmp.path());
        assert!(matches!(h.before(&ctx), HookAction::Continue));

        // malformed abort line
        let argv = write_script(tmp.path(), "g2.sh", "#!/bin/sh\necho 'abort banana'\n");
        let h = HelperHook::new("demo", tmp.path().to_path_buf(), argv, HookEvent::Pre);
        assert!(matches!(h.before(&ctx), HookAction::Continue));

        // helper hangs → timeout → Continue, fast
        let argv = write_script(tmp.path(), "g3.sh", "#!/bin/sh\nsleep 30\n");
        let h = HelperHook::new("demo", tmp.path().to_path_buf(), argv, HookEvent::Pre);
        let started = Instant::now();
        assert!(matches!(h.before(&ctx), HookAction::Continue));
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn post_and_chdir_hooks_fire() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("log");
        let argv = write_script(tmp.path(), "p.sh", "#!/bin/sh\necho \"$1 $2\" >> log\n");
        let h = HelperHook::new(
            "demo",
            tmp.path().to_path_buf(),
            argv.clone(),
            HookEvent::Post,
        );
        let words = vec!["echo".to_string(), "hi".into()];
        let ctx = cmd_ctx(&words, tmp.path());
        brish_plugin::PostExecHook::after(&h, &ctx, 5);
        let h2 = HelperHook::new("demo", tmp.path().to_path_buf(), argv, HookEvent::Chdir);
        brish_plugin::ChdirHook::on_cd(&h2, Path::new("/old"), tmp.path());
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(logged.contains("hook post"), "{logged}");
        assert!(logged.contains("hook chdir"), "{logged}");
    }

    #[test]
    fn keymap_pairs_parse() {
        let tmp = tempfile::tempdir().unwrap();
        let argv = write_script(
            tmp.path(),
            "k.sh",
            "#!/bin/sh\nprintf 'ctrl-g\\tmenu-next\\nzzz\\n\\n'\n",
        );
        let pairs = keymap_pairs(&argv, tmp.path(), 1_000);
        assert_eq!(pairs, vec![("ctrl-g".to_string(), "menu-next".to_string())]);
    }

    #[test]
    fn completion_skips_dollar_words_without_spawning() {
        let tmp = tempfile::tempdir().unwrap();
        // helper would fail hard if spawned — dollar words skip it
        let argv = vec![tmp.path().join("nope.sh").display().to_string()];
        let p = HelperCompletion::new("demo", argv, tmp.path().to_path_buf(), 100);
        let ctx = CompletionCtx {
            word: "HO",
            is_command: false,
            after_dollar: true,
            cwd: tmp.path(),
            line_before: "",
            algorithm: brish_plugin::Algorithm::Prefix,
            match_description: false,
        };
        assert!(p.complete(&ctx).is_empty());
    }
}
