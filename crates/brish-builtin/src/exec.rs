//! POSIX execution engine: simple commands, pipelines, redirections,
//! lists, compound commands, subshells, functions.
//!
//! Process model (ponytail: std primitives over hand-rolled forks):
//! - simple builtins run in-process (fd redirection via [`FdScope`]);
//! - externals spawn through `std::process::Command`;
//! - every pipeline stage, subshell, background item and command
//!   substitution runs in a forked child, which gives POSIX subshell
//!   semantics for free;
//! - redirection failures skip the command (status 1); expansion and
//!   parse failures kill the shell, like bash's bad-substitution rule.
//!
//! Non-local control flow ([`Stop`]) rides in `Result`'s error arm.
//!
//! (Plan deviation: lives here, not in `brish-core` — `brish-core` cannot
//! depend on this crate without a dependency cycle.)

use brish_core::ast::{self, CaseArm, Cmd, Program, Redir, Simple};
use brish_core::env::Env;
use brish_core::error::Error;
use brish_core::expand::{self, CmdSubst};
use brish_core::lexer::Word;
use brish_core::path::find_in_path;

use brish_plugin_api::{CmdCtx, DEFAULT_THEME, HookAction, Registry};

use crate::{BuiltIn, Flow, run as run_builtin};
use brish_platform::proc::{
    ChildState, FdOp, FdSetup, SIGCONT, fork_run, fork_spawn, kill_group, open_tty, poll_pid,
    preexec_fd_ops, send_signal, set_group_leader, shell_pgrp, signal_by_name, tcsetpgrp_fd,
    wait_pid, with_fds,
};

use brish_platform::RawFd;
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command as Proc, Stdio};
use std::sync::Arc;

/// Non-local control flow inside the engine.
#[derive(Debug)]
pub enum Stop {
    /// `exit N` — terminate the shell.
    Exit(i32),
    /// `return N` — leave the enclosing function / `source`.
    Return(i32),
    /// `break [n]` — unwind `n` loops.
    Break(usize),
    /// `continue [n]` — next iteration of loop `n`.
    Continue(usize),
    /// Fatal error: stop the shell (bad substitution, I/O on eval input, ...).
    Fail(Error),
}

pub type R<T> = Result<T, Stop>;

impl From<Error> for Stop {
    fn from(e: Error) -> Self {
        Stop::Fail(e)
    }
}

/// What happened after running a whole program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Shell keeps going; status is `$?`.
    Status(i32),
    /// `exit N` asked us to terminate with this code.
    Exit(i32),
}

/// Source of one redirected fd.
enum Src {
    File(File),
    Closed,
    /// Dup this shell fd onto the target (`>&2`, `3>&4`).
    Dup(RawFd),
}

impl Src {
    fn try_clone(&self) -> std::io::Result<Src> {
        Ok(match self {
            Src::File(f) => Src::File(f.try_clone()?),
            Src::Closed => Src::Closed,
            Src::Dup(n) => Src::Dup(*n),
        })
    }
}

type Plan = HashMap<usize, Src>;

/// 128 + SIGTSTP: `$?` after a job stops under `fg` (bash parity;
/// platform-correct — macOS TSTP=18, Linux TSTP=20).
const STOPPED_STATUS: i32 = 128 + brish_platform::SIGTSTP;

/// One job: `&` background, or an interactive foreground pipeline that
/// stopped (Ctrl-Z). `pids` are waited in order; the last stage's
/// status wins (POSIX pipeline). `own_pgrp` = launched as a group
/// leader (bg) — else the pids share the shell's group.
struct Job {
    pids: Vec<i32>,
    /// Per-pid cached status; `None` = not reaped yet.
    st: Vec<Option<i32>>,
    cmd: String,
    /// A stop was observed (SIGTSTP/SIGTTIN/SIGTTOU).
    stopped: bool,
    /// `fg` handed this job the terminal (released on stop/exit/bg).
    tty_held: bool,
    /// `jobs`/notify already printed this job's state.
    notified: bool,
    own_pgrp: bool,
}

impl Job {
    fn done(&self) -> bool {
        self.st.iter().all(Option::is_some)
    }

    /// Pipeline status: last stage (POSIX); 0 if nothing reaped yet.
    fn status(&self) -> i32 {
        self.st.last().and_then(|s| *s).unwrap_or(0)
    }
}

/// The shell: environment, function table, background jobs, plugins.
pub struct Engine {
    pub env: Env,
    funcs: HashMap<String, Cmd>,
    depth: usize,
    bg: Vec<Job>,
    /// Interactive job control on: WUNTRACED waits, terminal handoff.
    /// Batch scripts keep POSIX semantics (a stopped fg job blocks).
    job_control: bool,
    /// Did the last batch of expansions run a command substitution?
    cs_seen: bool,
    /// Static plugin registry (immutable after startup).
    hooks: Arc<Registry>,
    /// True inside forked children: plugin hooks run in the parent only.
    in_child: bool,
    /// Active theme name (`theme` builtin switches it at runtime).
    pub theme: String,
    /// `[hooks] command_not_found` command line (binary passes it from
    /// config; `relconf` refreshes it). Interactive only.
    not_found_hook: Option<String>,
    /// `relconf` rebuild hook (binary builds it): re-reads config,
    /// returns (fresh registry, resolved theme). `None` in tests/batch.
    relconf_fn: Option<Arc<dyn Fn() -> Reload + Send + Sync>>,
}

fn platform_err(e: impl std::fmt::Display) -> Stop {
    Stop::Fail(Error::Exec(e.to_string()))
}

/// Map a plan onto the platform fd juggling. `Inherit` needs no entry.
fn plan_setups(plan: Plan) -> Vec<(RawFd, FdSetup)> {
    let mut out: Vec<(RawFd, FdSetup)> = Vec::new();
    for (fd, src) in plan {
        match src {
            Src::File(f) => out.push((fd as RawFd, FdSetup::File(f))),
            Src::Closed => out.push((fd as RawFd, FdSetup::Close)),
            Src::Dup(n) => out.push((fd as RawFd, FdSetup::Dup(n))),
        }
    }
    out
}

/// Expand interactive aliases on the command word: replace `argv[0]`
/// while the chain keeps changing. Cycle/depth guard, no expansion
/// after `command`/`alias`/`unalias`.
///
/// ponytail: alias bodies split on whitespace — quoted arguments inside
/// an alias body (`alias x='cmd "a b"'`) split wrong. Ceiling: re-lex
/// the body with `brish_core::lexer` when aliases need quoted args.
fn expand_aliases(argv: Vec<String>, aliases: &HashMap<String, String>) -> Vec<String> {
    const MAX_DEPTH: usize = 64;
    let mut argv = argv;
    let mut seen: HashSet<String> = HashSet::new();
    let mut depth = 0;
    while depth < MAX_DEPTH {
        let first = argv[0].clone();
        if first == "command" || first == "alias" || first == "unalias" {
            break;
        }
        if !seen.insert(first.clone()) {
            break; // alias loop: `alias a='a b'`
        }
        let Some(body) = aliases.get(&first) else {
            break;
        };
        let parts: Vec<String> = body.split_whitespace().map(str::to_string).collect();
        if parts.is_empty() {
            break;
        }
        argv.splice(0..1, parts);
        depth += 1;
    }
    argv
}

/// Best-effort display text of an and-or list (for `jobs`, plan 4.10).
fn ao_text(ao: &ast::AndOr) -> String {
    let mut s = pipeline_text(&ao.first);
    for (op, p) in &ao.rest {
        s.push_str(match op {
            ast::AndOrOp::And => " && ",
            ast::AndOrOp::Or => " || ",
        });
        s.push_str(&pipeline_text(p));
    }
    s
}

fn pipeline_text(p: &ast::Pipeline) -> String {
    let mut s = String::new();
    if p.negated {
        s.push_str("! ");
    }
    s.push_str(&p.cmds.iter().map(cmd_text).collect::<Vec<_>>().join(" | "));
    s
}

fn cmd_text(c: &Cmd) -> String {
    match c {
        Cmd::Simple(sm) => sm.words.iter().map(word_text).collect::<Vec<_>>().join(" "),
        // ponytail: compound background commands show a generic marker;
        // precise `while ...; done` rendering is not worth the code.
        _ => "...".into(),
    }
}

fn word_text(w: &Word) -> String {
    use brish_core::lexer::Part;
    fn push(s: &mut String, p: &Part) {
        match p {
            Part::Raw(t) | Part::Single(t) => s.push_str(t),
            Part::Esc(c) => s.push(*c),
            Part::Double(inner) => inner.iter().for_each(|ip| push(s, ip)),
            Part::Param(n) => {
                s.push('$');
                s.push_str(n);
            }
            Part::Subst(c) => {
                s.push_str("$(");
                s.push_str(c);
                s.push(')');
            }
            Part::Arith(c) => {
                s.push_str("$((");
                s.push_str(c);
                s.push_str("))");
            }
        }
    }
    let mut s = String::new();
    w.parts.iter().for_each(|p| push(&mut s, p));
    s
}

/// Signal spec for `kill`: bare number or common name (plan 4.10).
fn parse_signal(spec: &str) -> Option<i32> {
    if let Ok(n) = spec.parse::<i32>() {
        return (n > 0).then_some(n);
    }
    // Platform-correct numbers (macOS USR1 is 30, not 10, etc.).
    signal_by_name(&spec.to_ascii_uppercase())
}

/// Apply `plan` around `f` in-process (restores on exit *and* panic).
fn apply_plan<T>(plan: Plan, f: impl FnOnce() -> T) -> R<T> {
    if plan.is_empty() {
        return Ok(f());
    }
    with_fds(plan_setups(plan), f).map_err(platform_err)
}

fn plan_clone(plan: &Plan) -> R<Plan> {
    let mut out = Plan::new();
    for (fd, src) in plan {
        out.insert(
            *fd,
            src.try_clone()
                .map_err(|e| Stop::Fail(Error::Exec(format!("redirection: {e}"))))?,
        );
    }
    Ok(out)
}

// Pipe ends convert to File through each platform's owned-handle type.
fn pipe_reader_file(r: std::io::PipeReader) -> File {
    File::from(std::os::fd::OwnedFd::from(r))
}

fn pipe_writer_file(w: std::io::PipeWriter) -> File {
    File::from(std::os::fd::OwnedFd::from(w))
}

/// Open `path` for `>`/`>>`/`>|` honoring `noclobber` (`>|` forces).
/// Anonymous temp file holding `bytes`, unlinked at creation (no leak,
/// 0600), rewound to 0 — shared by heredoc and here-string inputs.
fn src_from_bytes(bytes: &[u8]) -> Result<Src, String> {
    let mut f = tempfile::tempfile().map_err(|e| e.to_string())?;
    f.write_all(bytes)
        .and_then(|_| f.seek(SeekFrom::Start(0)))
        .map_err(|e| e.to_string())?;
    Ok(Src::File(f))
}

fn open_out(path: &str, append: bool, force: bool, noclobber: bool) -> std::io::Result<File> {
    let mut o = OpenOptions::new();
    o.write(true);
    if append {
        o.append(true);
    } else {
        o.truncate(true);
    }
    if noclobber && !append && !force {
        o.create_new(true);
    } else {
        o.create(true);
    }
    match o.open(path) {
        Err(e)
            if noclobber && !append && !force && e.kind() == std::io::ErrorKind::AlreadyExists =>
        {
            Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "cannot overwrite existing file",
            ))
        }
        other => other,
    }
}

/// Run `src` as a command substitution in a forked child, capturing
/// stdout. Returns `(output, exit_status)`.
fn run_subst(src: &str, env: &Env, funcs: &HashMap<String, Cmd>) -> Result<(String, i32), Error> {
    let prog = brish_core::parser::parse(src)?;
    let (mut rd, wr) = std::io::pipe().map_err(|e| Error::Exec(format!("pipe: {e}")))?;
    let setups = vec![(1, FdSetup::File(pipe_writer_file(wr)))];
    let pid = fork_spawn(setups, false, || {
        let mut child = Engine {
            env: env.clone(),
            funcs: funcs.clone(),
            depth: 0,
            bg: Vec::new(),
            job_control: false,
            cs_seen: false,
            hooks: Arc::new(Registry::default()),
            in_child: true,
            theme: DEFAULT_THEME.to_string(),
            not_found_hook: None,
            relconf_fn: None,
        };
        match child.program(&prog, true) {
            Ok(()) => child.env.status,
            Err(Stop::Exit(c)) | Err(Stop::Return(c)) => c,
            Err(Stop::Break(_)) | Err(Stop::Continue(_)) => child.env.status,
            Err(Stop::Fail(e)) => {
                eprintln!("brish: {e}");
                1
            }
        }
    })
    .map_err(|e| Error::Exec(e.to_string()))?;
    // Read *before* waiting: large outputs would deadlock a wait-first
    // parent against a child blocked on a full pipe.
    let mut out = String::new();
    rd.read_to_string(&mut out)?;
    // Trap-pending EINTR: retry; flags are drained by the caller's
    // xexpand once the borrow on Engine is free (cmd-subst runs inside
    // an expansion closure — cannot drain reentrantly here).
    let code = loop {
        match wait_pid(pid) {
            Ok(c) => break c,
            Err(brish_platform::PlatformError::Interrupted) => continue,
            Err(e) => return Err(Error::Exec(e.to_string())),
        }
    };
    Ok((out, code))
}

/// Single-quote `word` for safe re-entry into a command line.
fn quote_word(word: &str) -> String {
    let mut q = String::with_capacity(word.len() + 2);
    q.push('\'');
    for c in word.chars() {
        if c == '\'' {
            q.push_str("'\\''");
        } else {
            q.push(c);
        }
    }
    q.push('\'');
    q
}

/// Levenshtein distance over chars (two rows, no allocation per step
/// beyond the two `Vec`s).
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.chars().enumerate() {
        cur[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Executable names on `$PATH` (one read_dir per entry; only called
/// when a command is missing, so the cost does not show up otherwise).
fn path_executables(path: Option<&str>) -> Vec<String> {
    let Some(path) = path else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for dir in path.split(':').filter(|d| !d.is_empty()) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            continue;
        };
        for e in rd.flatten() {
            if let Some(n) = e.file_name().to_str() {
                out.push(n.to_string());
            }
        }
    }
    out
}

/// What `relconf` rebuilds: plugin registry, resolved theme, and the
/// `[hooks] command_not_found` line.
pub struct Reload {
    pub registry: Registry,
    pub theme: String,
    pub command_not_found: Option<String>,
}

/// Saved state per temp assignment: name -> (value, exported).
type TempSaved = Vec<(String, Option<(String, bool)>)>;

impl Engine {
    pub fn new() -> Self {
        Self::with_env(Env::from_std())
    }

    pub fn with_env(env: Env) -> Self {
        Self {
            env,
            funcs: HashMap::new(),
            depth: 0,
            bg: Vec::new(),
            job_control: false,
            cs_seen: false,
            hooks: Arc::new(Registry::default()),
            in_child: false,
            theme: DEFAULT_THEME.to_string(),
            not_found_hook: None,
            relconf_fn: None,
        }
    }

    /// Plugin registry backing hooks, themes, segments, providers.
    pub fn hooks(&self) -> &Arc<Registry> {
        &self.hooks
    }

    /// Swap in the startup-built registry (binary calls once).
    pub fn set_hooks(&mut self, hooks: Arc<Registry>) {
        self.hooks = hooks;
    }

    /// `[hooks] command_not_found` line (interactive only; batch keeps
    /// POSIX output so scripts do not see extra text).
    pub fn set_command_not_found(&mut self, hook: Option<String>) {
        self.not_found_hook = hook;
    }

    /// Install the `relconf` rebuild hook (binary only).
    pub fn set_relconf(&mut self, f: Arc<dyn Fn() -> Reload + Send + Sync>) {
        self.relconf_fn = Some(f);
    }

    /// Turn interactive job control on (tty REPL does; batch never).
    pub fn set_job_control(&mut self, on: bool) {
        self.job_control = on;
    }

    /// Working directory as the shell sees it (PWD, no syscall when set).
    fn shell_cwd(&self) -> PathBuf {
        self.env
            .get("PWD")
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// Run a parsed program. `Ok(Outcome)` for normal completion or
    /// `exit`; `Err` only for fatal errors.
    pub fn run(&mut self, prog: &Program) -> Result<Outcome, Error> {
        match self.program(prog, true) {
            Ok(()) => Ok(Outcome::Status(self.env.status)),
            Err(Stop::Exit(c)) => Ok(Outcome::Exit(c)),
            Err(Stop::Return(c)) => {
                self.env.status = c;
                Ok(Outcome::Status(c))
            }
            Err(Stop::Break(_)) | Err(Stop::Continue(_)) => Ok(Outcome::Status(self.env.status)),
            Err(Stop::Fail(e)) => Err(e),
        }
    }

    /// Reap finished/stopped job members (opportunistic; no zombies
    /// pile up), caching status for a later `wait`.
    pub fn reap_bg(&mut self) {
        for j in &mut self.bg {
            for i in 0..j.pids.len() {
                if j.st[i].is_none()
                    && let Ok(Some(state)) = poll_pid(j.pids[i])
                {
                    match state {
                        ChildState::Exited(code) => {
                            j.st[i] = Some(code);
                            if brish_core::debug_on("jobs") {
                                eprintln!("brish[jobs]: reaped {} -> {code}", j.pids[i]);
                            }
                        }
                        ChildState::Stopped => j.stopped = true,
                    }
                }
            }
        }
        // ponytail: remember at most 64 finished jobs; drop oldest done.
        while self.bg.iter().filter(|j| j.done()).count() > 64 {
            match self.bg.iter().position(|j| j.done()) {
                Some(i) => {
                    self.bg.remove(i);
                }
                None => break,
            }
        }
    }

    /// Done/stopped notices printed before the next prompt (bash-style
    /// asynchronous notification). Interactive only. Drains pending
    /// signal traps first so an idle shell still runs `trap ... TERM`.
    pub fn job_notifications(&mut self) -> R<Vec<String>> {
        self.drain_traps()?;
        self.reap_bg();
        let mut out = Vec::new();
        for (n, j) in self.bg.iter_mut().enumerate() {
            if j.notified {
                continue;
            }
            let state = if j.done() {
                "Done"
            } else if j.stopped {
                "Stopped"
            } else {
                continue;
            };
            out.push(format!("[{}]+ {}  {}", n + 1, state, j.cmd));
            j.notified = true;
        }
        Ok(out)
    }

    /// Drain pending signal traps (flags set by the platform handler)
    /// and run their commands. Flags swap first so a trap that raises
    /// the same signal cannot self-requeue. Parent only.
    ///
    /// ponytail: signal traps fire at command/prompt boundaries — a
    /// foreground `sleep 100` is not interrupted mid-wait. Ceiling:
    /// self-pipe + EINTR-aware wait for mid-command delivery.
    pub fn drain_traps(&mut self) -> R<()> {
        if self.in_child {
            return Ok(());
        }
        let pending = brish_platform::peek_pending_traps();
        if pending == 0 {
            return Ok(());
        }
        // Clear + run only the bits this shell has a trap for — a flag
        // set by our self-signal must not be swallowed by a parallel
        // engine that has no matching trap (it would defer/stall the
        // right owner, e.g. mid-wait trap delivery tests).
        let mut run_mask = 0u32;
        let mut cmds: Vec<String> = Vec::new();
        for (bit, name) in [
            (brish_platform::TRAP_BIT_INT, "INT"),
            (brish_platform::TRAP_BIT_TERM, "TERM"),
            (brish_platform::TRAP_BIT_HUP, "HUP"),
            (brish_platform::TRAP_BIT_QUIT, "QUIT"),
        ] {
            if pending & bit != 0
                && let Some(c) = self.env.traps.get(name)
            {
                run_mask |= bit;
                cmds.push(c.clone());
            }
        }
        if run_mask == 0 {
            return Ok(());
        }
        brish_platform::clear_pending_traps(run_mask);
        for cmd in cmds {
            self.run_trap_body(&cmd)?;
        }
        Ok(())
    }

    /// Run one trap/`EXIT` action body. Parse errors warn and are
    /// swallowed (a garbage trap must not kill the shell); `Return`
    /// and stray loop-control are absorbed; `Exit`/`Fail` propagate.
    fn run_trap_body(&mut self, src: &str) -> R<()> {
        let prog = match brish_core::parser::parse(src) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("brish: trap: {e}");
                return Ok(());
            }
        };
        match self.program(&prog, true) {
            Err(Stop::Return(s)) => {
                self.env.status = s;
                Ok(())
            }
            Err(Stop::Break(_)) | Err(Stop::Continue(_)) => Ok(()),
            other => other,
        }
    }

    /// Run the `EXIT` trap once, at process exit (main calls before
    /// `process::exit`). `Exit` inside it is absorbed — we're leaving
    /// anyway.
    pub fn run_exit_trap(&mut self) {
        if self.in_child {
            return;
        }
        let Some(cmd) = self.env.traps.get("EXIT").cloned() else {
            return;
        };
        match self.run_trap_body(&cmd) {
            Err(Stop::Fail(e)) => eprintln!("brish: {e}"),
            Err(Stop::Exit(_))
            | Err(Stop::Return(_))
            | Err(Stop::Break(_))
            | Err(Stop::Continue(_))
            | Ok(()) => {}
        }
    }

    /// Blocking wait that honors pending signal traps *mid-wait*:
    /// EINTR + flags → drain (run trap bodies) → retry. `Exit` from a
    /// trap propagates. This closes the "traps only fire at command
    /// boundaries" ceiling for foreground waits.
    fn wait_captured(&mut self, pid: i32) -> R<i32> {
        use brish_platform::PlatformError;
        loop {
            match brish_platform::wait_pid(pid) {
                Ok(c) => return Ok(c),
                Err(PlatformError::Interrupted) => self.drain_traps()?,
                Err(e) => return Err(platform_err(e)),
            }
        }
    }

    /// Same as [`Self::wait_captured`] but reports stops (`WUNTRACED`).
    fn wait_untraced_captured(&mut self, pid: i32) -> R<ChildState> {
        use brish_platform::PlatformError;
        loop {
            match brish_platform::wait_untraced(pid) {
                Ok(s) => return Ok(s),
                Err(PlatformError::Interrupted) => self.drain_traps()?,
                Err(e) => return Err(platform_err(e)),
            }
        }
    }

    // ---- lists ----

    fn program(&mut self, prog: &Program, errexit_ctx: bool) -> R<()> {
        for item in &prog.items {
            // Checked per item: `set -n` itself must run first.
            if self.env.opts.noexec {
                return Ok(());
            }
            self.item(item, errexit_ctx)?;
        }
        Ok(())
    }

    fn item(&mut self, item: &ast::Item, errexit_ctx: bool) -> R<()> {
        if item.background {
            return self.background(&item.andor);
        }
        if brish_core::debug_on("exec") {
            eprintln!("brish[exec]: {}", ao_text(&item.andor));
        }
        self.reap_bg();
        self.drain_traps()?;
        self.and_or(&item.andor, errexit_ctx)
    }

    fn background(&mut self, ao: &ast::AndOr) -> R<()> {
        let _ = Write::flush(&mut std::io::stdout());
        let ao = ao.clone();
        let text = ao_text(&ao);
        let pid = fork_spawn(Vec::new(), true, || {
            self.in_child = true;
            match self.and_or(&ao, true) {
                Ok(()) => self.env.status,
                Err(Stop::Exit(c)) | Err(Stop::Return(c)) => c,
                Err(Stop::Break(_)) | Err(Stop::Continue(_)) => self.env.status,
                Err(Stop::Fail(e)) => {
                    eprintln!("brish: {e}");
                    1
                }
            }
        })
        .map_err(platform_err)?;
        set_group_leader(pid);
        self.env.status = 0;
        self.env.last_bg = Some(pid as u32);
        self.bg.push(Job {
            pids: vec![pid],
            st: vec![None],
            cmd: text,
            stopped: false,
            tty_held: false,
            notified: false,
            own_pgrp: true,
        });
        Ok(())
    }

    // ---- job control (plan 4.10 minimal: wait / jobs / kill) ----

    /// Blocking wait for the job at `i` (a stopped job blocks until
    /// it is continued); caches and returns its status, or `None` if a
    /// pid is not a child of this shell.
    /// `R<Option<i32>>`: `None` = not our child (POSIX wait semantics);
    /// `Err` = a trap body asked to exit while we waited.
    fn wait_job_blocking(&mut self, i: usize) -> R<Option<i32>> {
        use brish_platform::PlatformError;
        if self.bg[i].done() {
            return Ok(Some(self.bg[i].status()));
        }
        for k in 0..self.bg[i].pids.len() {
            if self.bg[i].st[k].is_none() {
                let pid = self.bg[i].pids[k];
                loop {
                    match wait_pid(pid) {
                        Ok(c) => {
                            self.bg[i].st[k] = Some(c);
                            break;
                        }
                        Err(PlatformError::Interrupted) => self.drain_traps()?,
                        Err(_) => return Ok(None),
                    }
                }
            }
        }
        Ok(Some(self.bg[i].status()))
    }

    fn wait_cmd(&mut self, argv: &[String]) -> R<()> {
        if argv.len() == 1 {
            // POSIX: no operands waits for every known job. bash returns 0
            // even when a waited job failed (`false & wait` → 0).
            for i in 0..self.bg.len() {
                let _ = self.wait_job_blocking(i)?;
            }
            self.env.status = 0;
            return Ok(());
        }
        let mut status = 0;
        for arg in &argv[1..] {
            if let Some(n) = arg.strip_prefix('%') {
                let idx = n.parse::<usize>().ok().and_then(|n| n.checked_sub(1));
                status = match idx {
                    Some(i) if i < self.bg.len() => self.wait_job_blocking(i)?.unwrap_or(127),
                    _ => {
                        eprintln!("brish: wait: {arg}: no such job");
                        127
                    }
                };
            } else {
                let idx = arg
                    .parse::<i32>()
                    .ok()
                    .and_then(|pid| self.bg.iter().position(|j| j.pids.contains(&pid)));
                status = match idx {
                    Some(i) => self.wait_job_blocking(i)?.unwrap_or(127),
                    _ => {
                        eprintln!("brish: wait: {arg}: not a child of this shell");
                        127
                    }
                };
            }
        }
        self.env.status = status;
        Ok(())
    }

    fn jobs_cmd(&mut self) -> R<()> {
        self.reap_bg();
        let mut rows: Vec<Vec<String>> = Vec::new();
        for (n, j) in self.bg.iter_mut().enumerate() {
            let state = if j.done() {
                "Done"
            } else if j.stopped {
                "Stopped"
            } else {
                "Running"
            };
            rows.push(vec![
                format!("[{}]", n + 1),
                state.to_string(),
                format!("{} &", j.cmd),
            ]);
            j.notified = true;
        }
        println!("{}", crate::table::render(&rows));
        Ok(())
    }

    fn kill_cmd(&mut self, argv: &[String]) -> R<()> {
        let mut sig: i32 = 15; // SIGTERM, POSIX default
        let mut targets: Vec<&String> = Vec::new();
        for a in &argv[1..] {
            if let Some(spec) = a.strip_prefix('-') {
                match parse_signal(spec) {
                    Some(v) => sig = v,
                    None => {
                        eprintln!("brish: kill: {a}: invalid signal spec");
                        self.env.status = 1;
                        return Ok(());
                    }
                }
            } else {
                targets.push(a);
            }
        }
        if targets.is_empty() {
            eprintln!("brish: kill: usage: kill [-SIGNAME | -N] pid | %job ...");
            self.env.status = 1;
            return Ok(());
        }
        let mut status = 0;
        for t in targets {
            if let Some(n) = t.strip_prefix('%') {
                let idx = n
                    .parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .filter(|i| *i < self.bg.len());
                match idx {
                    Some(i) => {
                        // bg jobs lead their own group: signal it whole;
                        // interrupted fg jobs share ours: hit each pid.
                        let j = &self.bg[i];
                        let res = if j.own_pgrp {
                            kill_group(j.pids[0], sig)
                        } else {
                            j.pids.iter().try_for_each(|&p| send_signal(p, sig))
                        };
                        if let Err(e) = res {
                            eprintln!("brish: kill: {t}: {e}");
                            status = 1;
                        }
                    }
                    None => {
                        eprintln!("brish: kill: {t}: no such job");
                        status = 1;
                    }
                }
            } else {
                match t.parse::<i32>().ok() {
                    Some(p) => {
                        if let Err(e) = send_signal(p, sig) {
                            eprintln!("brish: kill: ({p}): {e}");
                            status = 1;
                        }
                    }
                    None => {
                        eprintln!("brish: kill: {t}: no such job");
                        status = 1;
                    }
                }
            }
        }
        self.env.status = status;
        Ok(())
    }

    /// Current job index: the newest one that is not done.
    fn current_job(&self) -> Option<usize> {
        self.bg.iter().rposition(|j| !j.done())
    }

    /// Resolve `[%]N` (default = current); `None` = no such/no
    /// current job (message printed). Call after `reap_bg`.
    fn job_arg(&self, argv: &[String], cmd: &str) -> Option<usize> {
        match argv.get(1).map(String::as_str) {
            None => self.current_job().or_else(|| {
                eprintln!("{cmd}: no current job");
                None
            }),
            Some(a) => match a.strip_prefix('%') {
                Some(n) => {
                    let idx = n
                        .parse::<usize>()
                        .ok()
                        .and_then(|n| n.checked_sub(1))
                        .filter(|i| *i < self.bg.len() && !self.bg[*i].done());
                    if idx.is_none() {
                        eprintln!("{cmd}: {a}: no such job");
                    }
                    idx
                }
                None => {
                    eprintln!("{cmd}: {a}: no such job");
                    None
                }
            },
        }
    }

    /// Hand the terminal to job `i` (interactive, different group).
    fn hold_terminal(&mut self, i: usize) -> bool {
        if !self.job_control {
            return false;
        }
        let pgrp = self.bg[i].pids[0];
        if pgrp == shell_pgrp() {
            return false;
        }
        let Ok(tty) = open_tty() else {
            return false;
        };
        if tcsetpgrp_fd(tty.as_raw_fd(), pgrp).is_ok() {
            self.bg[i].tty_held = true;
            true
        } else {
            false
        }
    }

    /// Give the terminal back to the shell if job `i` holds it.
    fn release_terminal(&mut self, i: usize) {
        if self.bg[i].tty_held {
            if let Ok(tty) = open_tty() {
                let _ = tcsetpgrp_fd(tty.as_raw_fd(), shell_pgrp());
            }
            self.bg[i].tty_held = false;
        }
    }

    /// SIGCONT a stopped job (group leader → whole group; else each pid).
    fn cont_job(&mut self, i: usize) {
        let own = self.bg[i].own_pgrp;
        let first = self.bg[i].pids[0];
        if own {
            let _ = kill_group(first, SIGCONT);
        } else {
            for &p in &self.bg[i].pids.clone() {
                let _ = send_signal(p, SIGCONT);
            }
        }
        self.bg[i].stopped = false;
    }

    /// `fg [%job]`: bring a job to the foreground and wait for it.
    fn fg_cmd(&mut self, argv: &[String]) -> R<()> {
        self.reap_bg();
        let Some(i) = self.job_arg(argv, "fg") else {
            self.env.status = 1;
            return Ok(());
        };
        println!("{}", self.bg[i].cmd);
        let held = self.hold_terminal(i);
        // CONT unconditionally: no-op on a running group, and it also
        // covers a stop that raced with our reap.
        self.cont_job(i);
        let mut stopped_now = false;
        for k in 0..self.bg[i].pids.len() {
            if self.bg[i].st[k].is_none() {
                let pid = self.bg[i].pids[k];
                match self.wait_untraced_captured(pid) {
                    Ok(ChildState::Exited(c)) => self.bg[i].st[k] = Some(c),
                    Ok(ChildState::Stopped) => {
                        self.bg[i].stopped = true;
                        stopped_now = true;
                        break;
                    }
                    Err(Stop::Fail(e)) => {
                        eprintln!("brish: fg: {e}");
                        break;
                    }
                    Err(other) => return Err(other), // trap `exit` propagates
                }
            }
        }
        if held {
            self.release_terminal(i);
        }
        // ponytail: a stop event not yet drained by reap_bg can be
        // reported here as a stale "Stopped"; the next fg converges
        // (the prompt reaper normally drains stops first).
        if stopped_now {
            // bash reports the stop right away; notify stays quiet.
            println!("[{}] + Stopped  {}", i + 1, self.bg[i].cmd);
            self.bg[i].notified = true;
            self.env.status = STOPPED_STATUS;
        } else {
            self.env.status = self.bg[i].status();
        }
        Ok(())
    }

    /// `bg [%job]`: continue a stopped job in the background.
    fn bg_cmd(&mut self, argv: &[String]) -> R<()> {
        self.reap_bg();
        let Some(i) = self.job_arg(argv, "bg") else {
            self.env.status = 1;
            return Ok(());
        };
        self.release_terminal(i);
        if self.bg[i].stopped {
            self.cont_job(i);
        }
        println!("[{}] + {} &", i + 1, self.bg[i].cmd);
        self.env.status = 0;
        Ok(())
    }

    fn and_or(&mut self, ao: &ast::AndOr, errexit_ctx: bool) -> R<()> {
        use ast::AndOrOp;
        self.pipeline(&ao.first)?;
        if ao.rest.is_empty() {
            return self.check_errexit(errexit_ctx, ao.first.negated);
        }
        // POSIX: `&&` and `||` have EQUAL precedence and associate
        // left-to-right, so a short-circuit skips only the *next* command,
        // never the rest of the list: `false && a || b` still runs `b`.
        // `continue` (not `return`/`break`) is what makes that work — the
        // skipped pipeline leaves the status untouched, so the next
        // operator still judges a meaningful status. `true && a || b`
        // runs `a` then stops, because `||` sees success.
        for (i, (op, pipe)) in ao.rest.iter().enumerate() {
            let failed = self.env.status != 0;
            match op {
                AndOrOp::And if failed => continue,
                AndOrOp::Or if !failed => continue,
                _ => {}
            }
            self.pipeline(pipe)?;
            if i + 1 == ao.rest.len() {
                self.check_errexit(errexit_ctx, pipe.negated)?;
            }
        }
        Ok(())
    }

    /// errexit: a failing pipeline that is not a `&&`/`||` condition
    /// (and not negated) terminates the shell.
    fn check_errexit(&mut self, ctx: bool, negated: bool) -> R<()> {
        if ctx && self.env.opts.errexit && self.env.status != 0 && !negated {
            return Err(Stop::Exit(self.env.status));
        }
        Ok(())
    }

    fn pipeline(&mut self, p: &ast::Pipeline) -> R<()> {
        if p.cmds.len() == 1 {
            self.cmd(&p.cmds[0])?;
        } else {
            self.pipeline_multi(p)?;
        }
        if p.negated {
            self.env.status = i32::from(self.env.status == 0);
        }
        Ok(())
    }

    /// Every stage runs in a forked child whose pipe fds are applied on
    /// entry. Parent spawns all stages first (concurrency!), then waits
    /// in order; the last stage's status wins (POSIX).
    fn pipeline_multi(&mut self, p: &ast::Pipeline) -> R<()> {
        let cmds = p.cmds.as_slice();
        let n = cmds.len();
        // pipes[i] connects stage i → i+1: (read end, write end)
        let mut pipes: Vec<(std::io::PipeReader, std::io::PipeWriter)> = Vec::new();
        for _ in 0..n - 1 {
            let (r, w) = std::io::pipe().map_err(|e| platform_err(format!("pipe: {e}")))?;
            pipes.push((r, w));
        }

        let mut pids: Vec<i32> = Vec::with_capacity(n);
        for (i, stage) in cmds.iter().enumerate() {
            let mut setups: Vec<(RawFd, FdSetup)> = Vec::new();
            if i > 0 {
                let (r, _) = &pipes[i - 1];
                let f = r
                    .try_clone()
                    .map_err(|e| platform_err(format!("pipe dup: {e}")))?;
                setups.push((0, FdSetup::File(pipe_reader_file(f))));
            }
            if i < n - 1 {
                let (_, w) = &pipes[i];
                let f = w
                    .try_clone()
                    .map_err(|e| platform_err(format!("pipe dup: {e}")))?;
                setups.push((1, FdSetup::File(pipe_writer_file(f))));
            }
            // Every child inherits ALL raw pipe fds; keeping a write
            // end open anywhere means readers never see EOF. Close the
            // raws after the 0/1 dups (FdSetup applies in order).
            // ponytail: on non-Unix, fork_spawn below fails first, so
            // the raw-close pass (AsRawFd) only needs to exist on Unix.
            for (r, w) in &pipes {
                for raw in [r.as_raw_fd(), w.as_raw_fd()] {
                    if raw > 1 && !setups.iter().any(|(t, _)| *t == raw) {
                        setups.push((raw, FdSetup::Close));
                    }
                }
            }
            let stage = stage.clone();
            let pid = fork_spawn(setups, false, || {
                self.in_child = true;
                match self.cmd(&stage) {
                    Ok(()) => self.env.status,
                    Err(Stop::Exit(c)) | Err(Stop::Return(c)) => c,
                    Err(Stop::Break(_)) | Err(Stop::Continue(_)) => self.env.status,
                    Err(Stop::Fail(e)) => {
                        eprintln!("brish: {e}");
                        1
                    }
                }
            })
            .map_err(platform_err)?;
            pids.push(pid);
        }
        // Drop our copies now that every stage holds its own.
        drop(pipes);

        let mut sts: Vec<i32> = Vec::with_capacity(pids.len());
        for (k, pid) in pids.iter().enumerate() {
            if !self.job_control {
                sts.push(self.wait_captured(*pid)?);
                continue;
            }
            // Interactive: Ctrl-Z stops the stage group; hand the
            // remaining stages to the job table and return to prompt.
            match self.wait_untraced_captured(*pid)? {
                ChildState::Exited(c) => sts.push(c),
                ChildState::Stopped => {
                    let rest = pids[k..].to_vec();
                    self.bg.push(Job {
                        st: vec![None; rest.len()],
                        pids: rest,
                        cmd: pipeline_text(p),
                        stopped: true,
                        tty_held: false,
                        notified: false,
                        own_pgrp: false,
                    });
                    self.env.status = STOPPED_STATUS;
                    return Ok(());
                }
            }
        }
        // pipefail: rightmost non-zero stage status, else zero.
        self.env.status = if self.env.opts.pipefail {
            sts.iter().rev().find(|&&s| s != 0).copied().unwrap_or(0)
        } else {
            sts.last().copied().unwrap_or(0)
        };
        Ok(())
    }

    // ---- commands ----

    fn cmd(&mut self, c: &Cmd) -> R<()> {
        match c {
            Cmd::Simple(s) => self.simple(s, &[]),
            Cmd::Arith(src) => {
                let v = brish_words::eval_arith(src, &mut self.env);
                self.env.status = match v {
                    Ok(0) => 1,
                    Ok(_) => 0,
                    Err(e) => {
                        eprintln!("brish: ((: {e}");
                        1
                    }
                };
                Ok(())
            }
            Cmd::Group(p) => self.program(p, true),
            Cmd::Subshell(p) => self.subshell(p, Plan::new()),
            Cmd::Redirected { inner, redirs } => self.redirected(inner, redirs),
            Cmd::If {
                cond,
                then,
                elif,
                els,
            } => self.if_cmd(cond, then, elif, els.as_ref()),
            Cmd::Loop { cond, body, until } => self.loop_cmd(cond, body, *until),
            Cmd::For { var, words, body } => self.for_cmd(var, words.as_deref(), body),
            Cmd::Case { word, arms } => self.case_cmd(word, arms),
            Cmd::FuncDef { name, body } => {
                self.funcs.insert(name.clone(), (**body).clone());
                self.env.status = 0;
                Ok(())
            }
        }
    }

    fn redirected(&mut self, inner: &Cmd, redirs: &[Redir]) -> R<()> {
        match inner {
            Cmd::Simple(s) => self.simple(s, redirs),
            other => {
                let Some(plan) = self.plan_or_skip(redirs.iter())? else {
                    return Ok(());
                };
                apply_plan(plan, || self.cmd(other))?
            }
        }
    }

    fn subshell(&mut self, p: &Program, plan: Plan) -> R<()> {
        let code = fork_run(plan_setups(plan), || {
            self.in_child = true;
            match self.program(p, true) {
                Ok(()) => self.env.status,
                Err(Stop::Exit(c)) | Err(Stop::Return(c)) => c,
                Err(Stop::Break(_)) | Err(Stop::Continue(_)) => self.env.status,
                Err(Stop::Fail(e)) => {
                    eprintln!("brish: {e}");
                    1
                }
            }
        })
        .map_err(platform_err)?;
        self.env.status = code;
        Ok(())
    }

    fn if_cmd(
        &mut self,
        cond: &Program,
        then: &Program,
        elif: &[(Program, Program)],
        els: Option<&Program>,
    ) -> R<()> {
        self.program(cond, false)?;
        if self.env.status == 0 {
            return self.program(then, true);
        }
        for (c, t) in elif {
            self.program(c, false)?;
            if self.env.status == 0 {
                return self.program(t, true);
            }
        }
        match els {
            Some(e) => self.program(e, true),
            None => {
                self.env.status = 0;
                Ok(())
            }
        }
    }

    /// Loop exit status = last completed body status; 0 when break was
    /// hit or the body never finished an iteration (bash-verified).
    fn loop_run(
        &mut self,
        mut cond_step: impl FnMut(&mut Self) -> R<bool>,
        body: &Program,
    ) -> R<()> {
        let mut last_body = 0i32;
        loop {
            if !cond_step(self)? {
                break;
            }
            match self.program(body, true) {
                Ok(()) => last_body = self.env.status,
                Err(Stop::Break(n)) if n <= 1 => {
                    self.env.status = 0;
                    return Ok(());
                }
                Err(Stop::Break(n)) => return Err(Stop::Break(n - 1)),
                Err(Stop::Continue(n)) if n <= 1 => {
                    last_body = 0;
                    continue;
                }
                Err(Stop::Continue(n)) => return Err(Stop::Continue(n - 1)),
                Err(other) => return Err(other),
            }
        }
        self.env.status = last_body;
        Ok(())
    }

    fn loop_cmd(&mut self, cond: &Program, body: &Program, until: bool) -> R<()> {
        let mut step = move |me: &mut Self| -> R<bool> {
            me.program(cond, false)?;
            let cs = me.env.status;
            // POSIX: while runs while cond is true (0); until runs while
            // cond is false (!= 0).
            Ok(if until { cs != 0 } else { cs == 0 })
        };
        self.loop_run(&mut step, body)
    }

    fn for_cmd(&mut self, var: &str, words: Option<&[Word]>, body: &Program) -> R<()> {
        let items = match words {
            Some(ws) => self.xwords(ws)?,
            None => self.env.positional().to_vec(),
        };
        self.env.status = 0;
        let mut last_body = 0i32;
        for w in items {
            self.env.set(var, w).map_err(Stop::Fail)?;
            match self.program(body, true) {
                Ok(()) => last_body = self.env.status,
                Err(Stop::Break(n)) if n <= 1 => {
                    self.env.status = 0;
                    return Ok(());
                }
                Err(Stop::Break(n)) => return Err(Stop::Break(n - 1)),
                Err(Stop::Continue(n)) if n <= 1 => {
                    last_body = 0;
                    continue;
                }
                Err(Stop::Continue(n)) => return Err(Stop::Continue(n - 1)),
                Err(other) => return Err(other),
            }
        }
        self.env.status = last_body;
        Ok(())
    }

    fn case_cmd(&mut self, word: &Word, arms: &[CaseArm]) -> R<()> {
        let w = self.xvalue(word)?;
        for arm in arms {
            for pw in &arm.pats {
                let pat = self.xvalue(pw)?;
                if brish_words::pattern_matches(&pat, &w) {
                    return self.program(&arm.body, true);
                }
            }
        }
        self.env.status = 0;
        Ok(())
    }

    // ---- simple commands ----

    fn simple(&mut self, s: &Simple, extra: &[Redir]) -> R<()> {
        self.cs_seen = false;
        // Parser already split leading assignments into s.assigns.
        let assign_words: Vec<(String, Word)> = s
            .assigns
            .iter()
            .map(|a| (a.name.clone(), a.value.clone()))
            .collect();
        let cmd_words = &s.words;

        // Command words expand first, then redirections (bash order).
        let mut argv = if cmd_words.is_empty() {
            Vec::new()
        } else {
            self.xwords(cmd_words)?
        };

        // Interactive aliases (POSIX: batch never expands). `command`/
        // `alias`/`unalias` as first word suppress expansion inside
        // expand_aliases.
        if self.env.flags.contains('i') && !argv.is_empty() {
            argv = expand_aliases(argv, &self.env.aliases);
        }
        let Some(plan) = self.plan_or_skip(s.redirs.iter().chain(extra.iter()))? else {
            return Ok(());
        };

        let mut assigns: Vec<(String, String)> = Vec::with_capacity(assign_words.len());
        for (n, w) in &assign_words {
            let v = self.xvalue_assign(w)?;
            assigns.push((n.clone(), v));
        }

        if argv.is_empty() {
            // Assignment-only command: persists, status 0.
            for (n, v) in assigns {
                if let Err(e) = self.env.set(&n, v) {
                    eprintln!("brish: {e}");
                    self.env.status = 1;
                    return Ok(());
                }
            }
            // Plain assignment → 0, but `x=$(false)` keeps the subst status.
            if !self.cs_seen {
                self.env.status = 0;
            }
            return Ok(());
        }

        if self.env.opts.xtrace {
            eprintln!("+ {}", argv.join(" "));
        }

        // Temp assignments live only for the command (bash-verified).
        let saved = match self.push_temp(&assigns) {
            Ok(s) => s,
            Err(()) => return Ok(()), // status already set
        };
        let out = self.exec_words(argv, plan, false);
        self.pop_temp(saved);
        out
    }

    fn push_temp(&mut self, assigns: &[(String, String)]) -> Result<TempSaved, ()> {
        let mut saved = Vec::with_capacity(assigns.len());
        for (n, v) in assigns {
            let prev = self.env.snapshot(n);
            if self.env.set(n, v).is_err() {
                eprintln!("brish: {n}: readonly variable");
                self.pop_temp(saved);
                self.env.status = 1;
                return Err(());
            }
            // Pre-command assignments always reach the child's
            // environment (POSIX), even for unexported shell vars.
            let _ = self.env.export(n);
            saved.push((n.clone(), prev));
        }
        Ok(saved)
    }

    fn pop_temp(&mut self, saved: TempSaved) {
        for (n, prev) in saved.into_iter().rev() {
            self.env.restore(&n, prev);
        }
    }

    /// Dispatch an already-expanded argv (no re-expansion).
    ///
    /// Plugin wrapper: fires pre/post/chdir hooks around the real
    /// dispatch — parent process only. Forked work (pipeline stages,
    /// background items, subshells, command substitutions) enters with
    /// `in_child` set and skips hooks entirely (plan: hooks run in the
    /// parent, documented gap for forked stages).
    fn exec_words(&mut self, argv: Vec<String>, plan: Plan, skip_funcs: bool) -> R<()> {
        if self.in_child {
            return self.exec_inner(&argv, plan, skip_funcs);
        }
        let cwd = self.shell_cwd();
        let ctx = CmdCtx {
            argv: &argv,
            status_before: self.env.status,
            cwd: &cwd,
        };
        if let HookAction::Abort(s) = self.hooks.run_pre(&ctx) {
            self.env.status = s;
            return Ok(());
        }
        let pwd_before = self.env.get("PWD").map(str::to_string);
        let out = self.exec_inner(&argv, plan, skip_funcs);
        self.hooks.run_post(&ctx, self.env.status);
        // `cd` success is observed as a PWD change (hooks see every
        // form, including function/eval/`command cd`).
        if argv[0] == "cd"
            && let Some(before) = pwd_before
            && let Some(after) = self.env.get("PWD")
            && before != after
        {
            self.hooks.run_chdir(Path::new(&before), Path::new(after));
        }
        out
    }

    /// Real dispatch, no hook bookkeeping.
    fn exec_inner(&mut self, argv: &[String], plan: Plan, skip_funcs: bool) -> R<()> {
        let name = argv[0].to_string();

        if !skip_funcs && self.funcs.contains_key(&name) {
            let body = self.funcs[&name].clone();
            return self.call_function(body, argv.to_vec(), plan);
        }
        // These builtins need the shell itself:
        if name == "eval" {
            return self.eval_cmd(argv, plan);
        }
        if name == "." || name == "source" {
            return self.source_cmd(argv, plan);
        }
        // Job-control builtins need the engine's job table (plan 4.10);
        // `theme`/`plugin` need the plugin registry (plan 6.6).
        if name == "wait" || name == "jobs" || name == "kill" || name == "fg" || name == "bg" {
            // All five return R<()>; apply_plan wraps → flatten.
            return apply_plan(plan, || match name.as_str() {
                "wait" => self.wait_cmd(argv),
                "jobs" => self.jobs_cmd(),
                "kill" => self.kill_cmd(argv),
                "fg" => self.fg_cmd(argv),
                _ => self.bg_cmd(argv),
            })
            .and_then(|r| r);
        }
        if name == "theme" {
            return apply_plan(plan, || self.theme_cmd(argv));
        }
        if name == "plugin" {
            return apply_plan(plan, || self.plugin_cmd(argv));
        }
        if name == "relconf" {
            return apply_plan(plan, || self.relconf_cmd(argv));
        }
        if name == "exec" {
            return self.exec_cmd(argv, plan);
        }
        if let Some(b) = BuiltIn::from_name(&name) {
            if b == BuiltIn::Command {
                // Keep the original plan: `command ls > f` redirects the
                // *target*, but `command -v x > f` redirects the builtin.
                let run_plan = plan_clone(&plan)?;
                let res = apply_plan(run_plan, || run_builtin(b, argv, &mut self.env))?;
                return match res? {
                    Flow::Status(s) => {
                        self.env.status = s;
                        Ok(())
                    }
                    Flow::Exec(v) => self.exec_words(v, plan, true),
                    other => self.flow(other),
                };
            }
            let res = apply_plan(plan, || run_builtin(b, argv, &mut self.env))?;
            return self.flow(res?);
        }
        self.spawn_external(argv, plan)
    }

    /// `exec [cmd [args…]]` (POSIX special builtin).
    ///
    /// With a command: replace the shell — fork, exec, and exit with the
    /// child's status so the rest of the script does not run. A bare
    /// `exec` (no command) applies its redirections to the *current*
    /// shell and returns, which is the standard fd-juggling idiom
    /// (`exec 3>&1` … `exec 3>&-`). Previously `exec` was not a builtin
    /// at all and fell through to the PATH search, so `exec 3>&1` died
    /// with "command not found".
    fn exec_cmd(&mut self, argv: &[String], plan: Plan) -> R<()> {
        if argv.len() == 1 {
            // Redirections only: make them permanent for this shell.
            // `apply_bare` dup2s without the save/restore that
            // `apply_plan` would undo on scope exit.
            let _ = std::io::Write::flush(&mut std::io::stdout());
            brish_platform::apply_bare(plan_setups(plan_clone(&plan)?)).map_err(platform_err)?;
            self.env.status = 0;
            return Ok(());
        }
        // With a command: the shell is replaced. fork+exec+wait keeps the
        // Rust process alive to flush and run EXIT traps; the observable
        // behaviour (no further script lines, same status) matches.
        let child_argv = argv[1..].to_vec();
        // `plan_clone` deep-copies the redirection sources (a File can be
        // handed to the child, the saved copy stays for this shell).
        let exec_plan = plan_clone(&plan)?;
        // A failure to even spawn leaves the status set (127 for a
        // missing command, 126 for a bad redirection); `exec` still
        // exits, per POSIX.
        let _ = apply_plan(plan, || self.spawn_external(&child_argv, exec_plan));
        let code = self.env.status;
        // `exec` never returns to the script.
        Err(Stop::Exit(code))
    }

    /// `theme [name]`: list registered themes (current marked `*`) or
    /// switch the active one. Unknown name → status 1, keeps current.
    fn theme_cmd(&mut self, argv: &[String]) {
        if argv.len() == 1 {
            let mut rows: Vec<Vec<String>> = Vec::new();
            for t in self.hooks.themes.iter() {
                let mark = if t.name() == self.theme { "*" } else { "" };
                rows.push(vec![mark.to_string(), t.name().to_string()]);
            }
            println!("{}", crate::table::render(&rows));
            self.env.status = 0;
            return;
        }
        let want = &argv[1];
        if self.hooks.themes.iter().any(|t| t.name() == want) {
            self.theme.clone_from(want);
            self.env.status = 0;
        } else {
            eprintln!("brish: unknown theme: {want}");
            self.env.status = 1;
        }
    }

    /// `plugin`: list catalog plugins and whether they are installed.
    fn plugin_cmd(&mut self, argv: &[String]) {
        self.env.status =
            crate::store_cmd::dispatch(argv, self.hooks.installed(), &self.shell_cwd());
    }

    /// `relconf`: reload `config.toml` + rebuild the registry + re-apply
    /// the theme (NOTES.md 4). Prompt/hooks/completions read the live
    /// registry; reedline-owned boxes (highlighter, menus, edit mode)
    /// need a shell restart — documented ceiling.
    fn relconf_cmd(&mut self, argv: &[String]) {
        // `relconf [-e]`: `-e` opens `config.toml` in `$VISUAL`/
        // `$EDITOR` first, so the reload below picks the edits up.
        let edit = argv.get(1).is_some_and(|a| a == "-e");
        if argv.len() > usize::from(edit) + 1 {
            eprintln!("relconf: usage: relconf [-e]");
            self.env.status = 2;
            return;
        }
        if edit && !self.edit_config() {
            self.env.status = 1;
            return;
        }
        let Some(build) = self.relconf_fn.clone() else {
            eprintln!("brish: relconf: unavailable in this shell");
            self.env.status = 1;
            return;
        };
        let Reload {
            registry,
            theme,
            command_not_found,
        } = build();
        self.hooks = Arc::new(registry);
        self.theme = theme;
        self.not_found_hook = command_not_found;
        self.env.status = 0;
    }

    /// Open `config.toml` in `$VISUAL`/`$EDITOR` and wait. False when
    /// no editor is configured, the path cannot be opened, or the
    /// editor exits non-zero.
    fn edit_config(&mut self) -> bool {
        let path = crate::paths::config_path();
        let editor = std::env::var("VISUAL")
            .ok()
            .or_else(|| std::env::var("EDITOR").ok())
            .filter(|e| !e.trim().is_empty());
        let Some(editor) = editor else {
            eprintln!("relconf -e: set $VISUAL or $EDITOR first");
            return false;
        };
        let mut parts = editor.split_whitespace();
        let Some(prog) = parts.next() else {
            return false;
        };
        if !path.exists() {
            if let Err(e) = crate::paths::ensure_config_dir() {
                eprintln!("relconf -e: {}: {e}", path.display());
                return false;
            }
            if let Err(e) = std::fs::write(&path, "") {
                eprintln!("relconf -e: {}: {e}", path.display());
                return false;
            }
        }
        let status = Proc::new(prog).args(parts).arg(&path).status();
        match status {
            Ok(s) if s.success() => true,
            Ok(s) => {
                eprintln!("relconf -e: {prog} exited with {s}");
                false
            }
            Err(e) => {
                eprintln!("relconf -e: {prog}: {e}");
                false
            }
        }
    }

    /// Translate a builtin's `Flow` into engine control flow.
    fn flow(&mut self, flow: Flow) -> R<()> {
        match flow {
            Flow::Status(s) => {
                self.env.status = s;
                Ok(())
            }
            Flow::Exit(s) => {
                self.env.status = s;
                Err(Stop::Exit(s))
            }
            Flow::Return(s) => {
                self.env.status = s;
                Err(Stop::Return(s))
            }
            Flow::Break(n) => Err(Stop::Break(n)),
            Flow::Continue(n) => Err(Stop::Continue(n)),
            Flow::Exec(v) => self.exec_words(v, Plan::new(), true),
        }
    }

    fn call_function(&mut self, body: Cmd, argv: Vec<String>, plan: Plan) -> R<()> {
        // ponytail: engine frames are chunky (~KBs, and grew when
        // `Error` started carrying a span); 128 keeps runaway recursion
        // off the 2MiB test/REPL stack. Raise if frames shrink.
        if self.depth >= 128 {
            eprintln!("brish: {}: function nesting limit", argv[0]);
            self.env.status = 1;
            return Ok(());
        }
        let saved_pos = self.env.positional().to_vec();
        self.env.set_positional(argv[1..].to_vec());
        self.env.push_fn_locals();
        self.depth += 1;
        let res = apply_plan(plan, || self.cmd(&body));
        self.depth -= 1;
        self.env.pop_fn_locals();
        self.env.set_positional(saved_pos);
        match res? {
            Err(Stop::Return(s)) => {
                self.env.status = s;
                Ok(())
            }
            r => r,
        }
    }

    fn eval_cmd(&mut self, argv: &[String], plan: Plan) -> R<()> {
        let src = argv[1..].join(" ");
        let prog = brish_core::parser::parse(&src).map_err(Stop::Fail)?;
        let inner = apply_plan(plan, || self.program(&prog, true))?;
        match inner {
            Err(Stop::Return(s)) => {
                self.env.status = s;
                Ok(())
            }
            r => r,
        }
    }

    fn source_cmd(&mut self, argv: &[String], plan: Plan) -> R<()> {
        let Some(path) = argv.get(1) else {
            eprintln!("source: filename argument required");
            self.env.status = 2;
            return Ok(());
        };
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("brish: {path}: {e}");
                self.env.status = 1;
                return Ok(());
            }
        };
        let prog = match brish_core::parser::parse(&src) {
            Ok(p) => p,
            Err(Error::Incomplete) => {
                eprintln!("brish: {path}: unexpected end of file");
                self.env.status = 2;
                return Ok(());
            }
            Err(e) => {
                eprintln!("brish: {path}: {e}");
                self.env.status = 2;
                return Ok(());
            }
        };
        let inner = apply_plan(plan, || self.program(&prog, true))?;
        match inner {
            Err(Stop::Return(s)) => {
                self.env.status = s;
                Ok(())
            }
            r => r,
        }
    }

    // ---- command not found (interactive aid) ----

    /// `command not found` extras: a `did you mean` line and the
    /// `[hooks] command_not_found` command. Interactive only — batch
    /// keeps POSIX output (`name: command not found`, status 127) so
    /// scripts and the dash cross-check see nothing new.
    fn report_not_found(&mut self, name: &str) {
        if self.in_child || !self.env.flags.contains('i') {
            return;
        }
        if let Some(best) = self.suggest_command(name) {
            eprintln!("brish: did you mean '{best}'?");
        }
        let Some(hook) = self.not_found_hook.clone() else {
            return;
        };
        let src = format!("{hook} {}", quote_word(name));
        match brish_core::parser::parse(&src) {
            Ok(prog) => {
                // The hook's own output is the answer; its exit status
                // must not turn our 127 into something else.
                let saved = self.env.status;
                let _ = self.program(&prog, true);
                self.env.status = saved;
            }
            Err(e) => eprintln!("brish: command_not_found hook: {e}"),
        }
    }

    /// Closest known command within edit distance 2 (builtins, then
    /// aliases, then `$PATH`). `None` when nothing is close enough.
    fn suggest_command(&self, name: &str) -> Option<String> {
        if name.len() < 3 {
            return None;
        }
        let budget = if name.len() <= 4 { 1 } else { 2 };
        let mut best: Option<(usize, String)> = None;
        let consider = |cand: &str, best: &mut Option<(usize, String)>| {
            let d = edit_distance(name, cand);
            if d > budget || d == 0 {
                return;
            }
            match best {
                Some((bd, _)) if *bd <= d => {}
                _ => *best = Some((d, cand.to_string())),
            }
        };
        for c in BuiltIn::names() {
            consider(c, &mut best);
        }
        for c in self.env.aliases.keys() {
            consider(c, &mut best);
        }
        // The `$PATH` scan is a read_dir per entry — do it only when no
        // builtin/alias was close enough (common typo case stays free).
        if best.is_none() {
            for c in path_executables(self.env.get("PATH")) {
                consider(&c, &mut best);
            }
        }
        best.map(|(_, c)| c)
    }

    // ---- externals ----

    fn spawn_external(&mut self, argv: &[String], plan: Plan) -> R<()> {
        let name = &argv[0];
        let path: PathBuf = if name.contains('/') {
            PathBuf::from(name)
        } else {
            match find_in_path(name, self.env.get("PATH")) {
                Some(p) => p,
                None => {
                    eprintln!("brish: {name}: command not found");
                    self.env.status = 127;
                    self.report_not_found(name);
                    return Ok(());
                }
            }
        };

        let mut cmd = Proc::new(&path);
        cmd.args(&argv[1..]);
        for (k, v) in self.env.child_env() {
            cmd.env(k, v);
        }

        let mut ops: Vec<FdOp> = Vec::new();
        // Extra-fd sources must stay open until spawn: pre_exec dups them.
        let mut keep: Vec<File> = Vec::new();
        for fd in 0..3usize {
            let src = match plan.get(&fd) {
                Some(s) => Some(s.try_clone().map_err(platform_err)?),
                None => None,
            };
            let stdio = match src {
                Some(Src::File(f)) => Stdio::from(f),
                Some(Src::Closed) => {
                    ops.push(FdOp::Close(fd as RawFd));
                    Stdio::null()
                }
                None => Stdio::inherit(),
                Some(Src::Dup(n)) => match brish_platform::proc::dup_fd(n) {
                    Ok(f) => Stdio::from(f),
                    // parent fd already closed: behave like inherit
                    Err(_) => Stdio::inherit(),
                },
            };
            match fd {
                0 => {
                    cmd.stdin(stdio);
                }
                1 => {
                    cmd.stdout(stdio);
                }
                _ => {
                    cmd.stderr(stdio);
                }
            }
        }
        // Extra fds ride in via pre_exec (Unix only; the non-unix
        // preexec stub is a no-op, so those redirections must error).
        for (&fd, src) in &plan {
            if fd < 3 {
                continue;
            }
            match src {
                Src::File(f) => {
                    let c = f
                        .try_clone()
                        .map_err(|e| platform_err(format!("redirection: {e}")))?;
                    ops.push(FdOp::Dup {
                        from: c.as_raw_fd(),
                        to: fd as RawFd,
                    });
                    keep.push(c);
                }
                Src::Closed => ops.push(FdOp::Close(fd as RawFd)),
                Src::Dup(n) => ops.push(FdOp::Dup {
                    from: *n,
                    to: fd as RawFd,
                }),
            }
        }
        preexec_fd_ops(&mut cmd, ops);

        match cmd.spawn() {
            Ok(child) => {
                if self.job_control {
                    // Interactive: a Ctrl-Z stop must return us to the
                    // prompt instead of hanging in waitpid.
                    let pid = child.id() as i32;
                    drop(child); // waitpid below owns the reaping
                    return match self.wait_untraced_captured(pid) {
                        Ok(ChildState::Exited(c)) => {
                            self.env.status = c;
                            Ok(())
                        }
                        Ok(ChildState::Stopped) => {
                            self.bg.push(Job {
                                pids: vec![pid],
                                st: vec![None],
                                cmd: argv.join(" "),
                                stopped: true,
                                tty_held: false,
                                notified: false,
                                own_pgrp: false,
                            });
                            self.env.status = STOPPED_STATUS;
                            Ok(())
                        }
                        Err(x) => Err(x), // trap Exit/Fail propagates
                    };
                }
                let pid = child.id() as i32;
                drop(child);
                self.env.status = self.wait_captured(pid)?;
                Ok(())
            }
            Err(e) => {
                let (code, why) = match e.kind() {
                    std::io::ErrorKind::NotFound => (127, "No such file or directory"),
                    std::io::ErrorKind::PermissionDenied => (126, "Permission denied"),
                    _ => (126, "cannot execute"),
                };
                eprintln!("brish: {name}: {why}");
                self.env.status = code;
                Ok(())
            }
        }
    }

    // ---- redirections ----

    /// Build the fd plan. `None` = an open failed (caller skips the
    /// command with status 1); `Err(Stop)` = fatal expansion error.
    fn plan_or_skip<'a>(&mut self, redirs: impl Iterator<Item = &'a Redir>) -> R<Option<Plan>> {
        let mut plan: Plan = HashMap::new();
        for r in redirs {
            match self.plan_one(&plan, r)? {
                Ok((fd, src)) => {
                    plan.insert(fd, src);
                }
                Err(msg) => {
                    eprintln!("brish: {msg}");
                    self.env.status = 1;
                    return Ok(None);
                }
            }
        }
        Ok(Some(plan))
    }

    fn plan_one(&mut self, plan: &Plan, r: &Redir) -> R<Result<(usize, Src), String>> {
        match r {
            Redir::Input { fd, target } => {
                let t = self.xvalue(target)?;
                Ok(match File::open(&t) {
                    Ok(f) => Ok((*fd, Src::File(f))),
                    Err(e) => Err(format!("{t}: {e}")),
                })
            }
            Redir::Output { fd, target } => {
                let t = self.xvalue(target)?;
                Ok(match open_out(&t, false, false, self.env.opts.noclobber) {
                    Ok(f) => Ok((*fd, Src::File(f))),
                    Err(e) => Err(format!("{t}: {e}")),
                })
            }
            Redir::Append { fd, target } => {
                let t = self.xvalue(target)?;
                Ok(match open_out(&t, true, false, self.env.opts.noclobber) {
                    Ok(f) => Ok((*fd, Src::File(f))),
                    Err(e) => Err(format!("{t}: {e}")),
                })
            }
            Redir::Clobber { fd, target } => {
                let t = self.xvalue(target)?;
                Ok(match open_out(&t, false, true, self.env.opts.noclobber) {
                    Ok(f) => Ok((*fd, Src::File(f))),
                    Err(e) => Err(format!("{t}: {e}")),
                })
            }
            Redir::DupIn { fd, target } | Redir::DupOut { fd, target } => {
                let t = self.xvalue(target)?;
                if t == "-" {
                    return Ok(Ok((*fd, Src::Closed)));
                }
                match t.parse::<usize>() {
                    Ok(n) => {
                        let src = match plan.get(&n) {
                            Some(s) => s
                                .try_clone()
                                .map_err(|e| Stop::Fail(Error::Exec(format!("dup: {e}"))))?,
                            None => Src::Dup(n as RawFd),
                        };
                        Ok(Ok((*fd, src)))
                    }
                    Err(_) => Ok(Err(format!("bad file descriptor: {t}"))),
                }
            }
            Redir::Heredoc {
                fd,
                text,
                expand: do_expand,
            } => {
                let body = if *do_expand {
                    self.xtext(text)?
                } else {
                    text.clone()
                };
                Ok(src_from_bytes(body.as_bytes())
                    .map_err(|e| format!("heredoc: {e}"))
                    .map(|s| (*fd, s)))
            }
            Redir::HereString { fd, target } => {
                let mut body = self.xvalue(target)?;
                body.push('\n');
                Ok(src_from_bytes(body.as_bytes())
                    .map_err(|e| format!("here-string: {e}"))
                    .map(|s| (*fd, s)))
            }
        }
    }

    // ---- expansion helpers (each wires in command substitution) ----

    /// Run one expansion with command substitution wired in: every
    /// substitution updates `st`, which lands in `env.status` (and
    /// `cs_seen`) afterwards — `x=$(false)` keeps status 1.
    fn xexpand<T>(&mut self, f: impl FnOnce(&mut Env, CmdSubst) -> Result<T, Error>) -> R<T> {
        let (env_c, funcs_c) = (self.env.clone(), self.funcs.clone());
        let mut st: Option<i32> = None;
        let r = {
            let mut cs = |src: &str| -> Result<String, Error> {
                let (out, code) = run_subst(src, &env_c, &funcs_c)?;
                st = Some(code);
                Ok(out)
            };
            f(&mut self.env, &mut cs)
        };
        // Traps signaled while expansion waits (cmd-subst) run now
        // that the Engine borrow is free — then expansion status wins.
        if brish_platform::peek_pending_traps() != 0 {
            self.drain_traps()?;
        }
        if let Some(c) = st {
            self.env.status = c;
            self.cs_seen = true;
        }
        r.map_err(Stop::Fail)
    }

    fn xwords(&mut self, ws: &[Word]) -> R<Vec<String>> {
        self.xexpand(|env, cs| expand::expand_words(env, ws, cs))
    }

    fn xvalue(&mut self, w: &Word) -> R<String> {
        self.xexpand(|env, cs| expand::expand_value(env, w, cs))
    }

    fn xvalue_assign(&mut self, w: &Word) -> R<String> {
        self.xexpand(|env, cs| expand::expand_assign_value(env, w, cs))
    }

    fn xtext(&mut self, src: &str) -> R<String> {
        self.xexpand(|env, cs| expand::expand_text(env, src, cs))
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `src`; returns the exit/status result.
    fn sh(src: &str) -> Result<Outcome, Error> {
        let mut e = Engine::new();
        let prog = brish_core::parser::parse(src)?;
        e.run(&prog)
    }

    fn status(src: &str) -> i32 {
        match sh(src) {
            Ok(Outcome::Status(s)) => s,
            Ok(Outcome::Exit(s)) => s,
            Err(e) => panic!("unexpected fatal error: {e}"),
        }
    }

    #[test]
    fn simple_true_false() {
        assert_eq!(status("true"), 0);
        assert_eq!(status("false"), 1);
    }

    #[test]
    fn and_or_lists() {
        assert_eq!(status("true && true"), 0);
        assert_eq!(status("true && false"), 1);
        assert_eq!(status("false || true"), 0);
        assert_eq!(status("false && true"), 1);
        assert_eq!(status("false || false"), 1);
        assert_eq!(status("! true"), 1);
        assert_eq!(status("! false"), 0);
        // Short-circuit: right side never runs.
        assert_eq!(status("false && nosuchcmd123"), 1);
        assert_eq!(status("true || nosuchcmd123"), 0);
    }

    #[test]
    fn exec_replaces_the_shell_or_redirects_it() {
        // `exec cmd` replaces the shell: no further script lines run and
        // the child's status becomes the shell's.
        assert_eq!(status("exec true; printf NOT-REACHED"), 0);
        assert_eq!(status("exec false; printf NOT-REACHED"), 1);
        // `exec` with only redirections applies them to THIS shell, the
        // standard fd-juggling idiom. It used to fall through to the PATH
        // search and die with "command not found".
        //
        // The redirection half is NOT tested here: `exec > f` applies
        // with `apply_bare`, permanently dup2'ing the file onto THIS
        // process's fd 1 — the libtest harness shares it, so every
        // later "... ok" line vanishes into the temp file (deleted with
        // the tempdir) and the suite reports exit 0 with no result line.
        // tests/posix.rs covers `exec 3>&1` and `exec > f` against the
        // real binary, in its own process, where that is safe.
        let mut e = Engine::new();
        // The exec'd command's status propagates (dash-compatible).
        let prog = brish_core::parser::parse("exec sh -c 'exit 42'").unwrap();
        assert_eq!(e.run(&prog).unwrap(), Outcome::Exit(42));
        // A missing command still exits (POSIX), never continuing.
        assert_eq!(status("exec nosuchcmd-xyz; printf NOT-REACHED"), 127);
    }

    #[test]
    fn and_or_equal_precedence_skips_only_the_next_command() {
        // POSIX 2.9.4: `&&` and `||` associate left-to-right at one
        // level, so a short-circuit abandons the NEXT command only —
        // `false && a || b` must still reach `b`.
        let mut e = Engine::new();
        let run = |e: &mut Engine, src: &str| {
            let prog = brish_core::parser::parse(src).expect("parse");
            e.run(&prog).expect("run")
        };
        // Capture via files: the skipped command must never write, so the
        // file proves which side actually executed. Separate files per
        // list — `>` truncates, so sharing one would lose the first half.
        let d = tempfile::tempdir().expect("tmp");
        let (f1, f2) = (d.path().join("o1"), d.path().join("o2"));
        let src = format!(
            "false && printf SKIP_A > {f1} || printf RAN_B > {f1}; \
             true || printf SKIP_C > {f2} && printf RAN_D > {f2}",
            f1 = f1.display(),
            f2 = f2.display()
        );
        // `printf`, not `echo`: builtin echo output is captured by
        // libtest and never reaches a redirected file under test (same
        // trap as noclobber_refuses_overwrite above).
        //
        // Distinctive multi-char markers, never bare letters: a parallel
        // test's libtest line ("test exec::tests::aliases_... ok") can
        // land in the file during the dup2 window, and a single-letter
        // marker collides with ordinary output often enough to flake.
        let out = run(&mut e, &src);
        assert_eq!(out, Outcome::Status(0));
        let one = std::fs::read_to_string(&f1).expect("o1");
        let two = std::fs::read_to_string(&f2).expect("o2");
        assert!(
            one.contains("RAN_B"),
            "b must have run (and a must not): {one:?}"
        );
        assert!(
            !one.contains("SKIP_A"),
            "the short-circuited `a` must not run: {one:?}"
        );
        assert!(
            two.contains("RAN_D"),
            "d must have run (and c must not): {two:?}"
        );
        assert!(
            !two.contains("SKIP_C"),
            "the short-circuited `c` must not run: {two:?}"
        );
        // within one operator, the short-circuit still stops the chain
        assert_eq!(status("false && nosuchcmd123 && nosuchcmd123"), 1);
    }

    #[test]
    fn pipeline_status_is_last() {
        assert_eq!(status("true | false"), 1);
        assert_eq!(status("false | true"), 0);
        assert_eq!(status("! true | false"), 0); // ! applies to pipeline
    }

    #[test]
    fn exit_terminates_with_code() {
        assert_eq!(sh("exit 7").ok(), Some(Outcome::Exit(7)));
        assert_eq!(sh("exit").ok(), Some(Outcome::Exit(0)));
        assert_eq!(sh("true; exit 3; false").ok(), Some(Outcome::Exit(3)));
    }

    #[test]
    fn assigns_persist_and_status_zero() {
        let mut e = Engine::new();
        let prog = brish_core::parser::parse("x=42").unwrap();
        e.run(&prog).unwrap();
        assert_eq!(e.env.get("x"), Some("42"));
        assert_eq!(e.env.status, 0);
    }

    #[test]
    fn temp_assigns_do_not_persist() {
        let mut e = Engine::new();
        // `true` is external; env must be untouched after.
        let prog = brish_core::parser::parse("x=1 true").unwrap();
        e.run(&prog).unwrap();
        assert_eq!(e.env.get("x"), None);
    }

    #[test]
    fn if_elif_else() {
        assert_eq!(status("if true; then :; fi"), 0);
        assert_eq!(status("if false; then :; fi"), 0);
        assert_eq!(status("if false; then :; else false; fi"), 1);
        assert_eq!(status("if false; then :; elif true; then false; fi"), 1);
        assert_eq!(status("if true; then exit 5; fi"), 5);
    }

    #[test]
    fn while_loop_runs_body_while_cond_true() {
        assert_eq!(status("i=0; while false; do :; done"), 0);
        // break resets status to 0 (bash-verified).
        assert_eq!(status("while true; do false; break; done"), 0);
        assert_eq!(status("while true; do break; done"), 0);
        assert_eq!(
            status("i=0; while [ $i -lt 3 ]; do i=$((i+1)); done; [ $i -eq 3 ]"),
            0
        );
    }

    #[test]
    fn until_loop() {
        assert_eq!(
            status("i=0; until [ $i -ge 2 ]; do i=$((i+1)); done; [ $i -eq 2 ]"),
            0
        );
    }

    #[test]
    fn for_loops() {
        assert_eq!(status("for x in a b c; do :; done"), 0);
        assert_eq!(status("for x in 1 2 3; do [ $x -eq 3 ] && break; done"), 0);
        assert_eq!(status("for x in; do false; done"), 0); // zero iterations
        // continue skips to next iteration; loop still ends with 0.
        assert_eq!(status("for x in 1 2; do false; continue; done"), 0);
        // positional fallback when `in` is absent
        assert_eq!(status("set -- a b; for x; do :; done"), 0);
    }

    #[test]
    fn case_matching() {
        assert_eq!(status("case abc in abc) false;; esac"), 1);
        assert_eq!(status("case abc in xyz) false;; esac"), 0);
        assert_eq!(status("case abc in a*) true;; *) false;; esac"), 0);
        assert_eq!(status("case abc in b) false;; c) false;; esac"), 0); // first match only
        assert_eq!(status("case abc in *b*) false;; esac"), 1);
    }

    #[test]
    fn functions_define_call_return() {
        let mut e = Engine::new();
        let prog = brish_core::parser::parse("f() { return 5; }; f").unwrap();
        let out = e.run(&prog).unwrap();
        assert_eq!(out, Outcome::Status(5));
        // Positional restored after call.
        let prog =
            brish_core::parser::parse("set -- a b; f() { :; }; f x y z; [ $# -eq 2 ]").unwrap();
        assert_eq!(e.run(&prog).unwrap(), Outcome::Status(0));
        // Recursion guard: infinite recursion ends with status 1, no stack overflow.
        let prog = brish_core::parser::parse("r() { r; }; r").unwrap();
        assert_eq!(e.run(&prog).unwrap(), Outcome::Status(1));
    }

    #[test]
    fn subshell_isolates_vars() {
        let mut e = Engine::new();
        let prog = brish_core::parser::parse("( x=1 ); [ -z \"${x-}\" ]").unwrap();
        assert_eq!(e.run(&prog).unwrap(), Outcome::Status(0));
        // Status propagates out of subshells.
        let prog = brish_core::parser::parse("( exit 4 )").unwrap();
        assert_eq!(e.run(&prog).unwrap(), Outcome::Status(4));
    }

    #[test]
    fn errexit_kills_shell_only_outside_conditions() {
        let mut e = Engine::new();
        let prog = brish_core::parser::parse("set -e; false; echo hi").unwrap();
        assert_eq!(e.run(&prog).unwrap(), Outcome::Exit(1));
        // In `&&`/`||` conditions errexit is suspended.
        assert_eq!(status("set -e; false || true; echo hi"), 0);
        // ...and in `if` conditions.
        assert_eq!(status("set -e; if false; then :; fi; echo hi"), 0);
        // negation exempt
        assert_eq!(status("set -e; ! false; echo hi"), 0);
    }

    #[test]
    fn nounset_is_fatal() {
        assert!(sh("set -u; echo $no_such_var_xyz").is_err());
    }

    #[test]
    fn question_mark_status() {
        assert_eq!(status("false; [ $? -eq 1 ]"), 0);
        assert_eq!(status("true; [ $? -eq 0 ]"), 0);
    }

    #[test]
    fn cmdsubst_runs_and_sets_status() {
        assert_eq!(status("x=$(true); [ $? -eq 0 ]"), 0);
        assert_eq!(status("x=$(false); [ $? -eq 1 ]"), 0);
        // printf, not echo: libtest captures the builtin's println so it
        // would bypass the substitution pipe under test.
        assert_eq!(status("[ \"$(printf 'ab')\" = 'ab' ]"), 0);
        assert_eq!(status("[ \"$(printf 'a b')\" = 'a b' ]"), 0);
        // Substitution output splits like normal words when unquoted.
        assert_eq!(status("set -- $(printf 'a b'); [ $# -eq 2 ]"), 0);
    }

    #[test]
    fn background_returns_zero_and_is_reaped() {
        let mut e = Engine::new();
        let prog = brish_core::parser::parse("true &").unwrap();
        assert_eq!(e.run(&prog).unwrap(), Outcome::Status(0));
        assert!(e.env.last_bg.is_some());
        // Give the child a moment, then reap until the status is cached.
        for _ in 0..100 {
            e.reap_bg();
            if e.bg.iter().all(|j| j.done()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            e.bg.iter().all(|j| j.done()),
            "background job must be reaped"
        );
    }

    #[test]
    fn wait_jobs_kill_semantics() {
        let mut e = Engine::new();
        // wait for a background job by pid → job's own status.
        let prog = brish_core::parser::parse("sleep 0.05 & wait $!; echo done").unwrap();
        assert_eq!(e.run(&prog).unwrap(), Outcome::Status(0));
        // unknown pid → POSIX 127.
        let prog = brish_core::parser::parse("wait 99999999").unwrap();
        assert_eq!(e.run(&prog).unwrap(), Outcome::Status(127));
        // Kill a running background job, then wait → 128+SIGKILL.
        // KILL (not TERM): untrappable, so leftover SIGTERM handlers
        // from earlier tests can't absorb the signal — bash shows the
        // same "trap in parent + killpg spares the job" quirk, and the
        // job-kill semantics under test don't care which signal.
        let mut e = Engine::new();
        let prog = brish_core::parser::parse("sleep 5 & kill -KILL %1; wait %1").unwrap();
        assert_eq!(e.run(&prog).unwrap(), Outcome::Status(137));
        // no-operand wait is always 0 (bash/POSIX).
        let prog = brish_core::parser::parse("true & wait").unwrap();
        assert_eq!(e.run(&prog).unwrap(), Outcome::Status(0));
    }

    #[test]
    fn eval_and_source_and_command() {
        assert_eq!(status("eval 'false'"), 1);
        assert_eq!(status("eval 'true; true'"), 0);
        // `command` suppresses function lookup.
        assert_eq!(status("true() { false; }; command true"), 0);
        // source runs in the current shell (var persists).
        let mut e = Engine::new();
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("lib.sh");
        std::fs::write(&f, "SOURCED=1\n").unwrap();
        let src = format!("source {}", f.display());
        let prog = brish_core::parser::parse(&src).unwrap();
        e.run(&prog).unwrap();
        assert_eq!(e.env.get("SOURCED"), Some("1"));
        // missing file: status 1, not fatal
        assert_eq!(status("source /no/such/file.sh"), 1);
    }

    #[test]
    fn noclobber_refuses_overwrite() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("c");
        std::fs::write(&f, "old").unwrap();
        let src = format!("set -o noclobber; echo new > {}; false", f.display());
        assert_eq!(status(&src), 1);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "old");
        // `>|` forces
        let src = format!("set -o noclobber; echo new > {}; true", f.display());
        status(&src); // refusals print to stderr; ignored here
        // printf, not echo: builtin echo output is captured by libtest
        // and never reaches the redirected file under test. The harness
        // may also emit its own fd1 lines into the file during the
        // dup2 window (parallel tests) — assert our bytes arrived,
        // not byte-exact content.
        let src = format!(
            "set -o noclobber; printf 'forced\\n' >| {}; true",
            f.display()
        );
        assert_eq!(status(&src), 0);
        let got = std::fs::read_to_string(&f).unwrap();
        assert!(
            got.contains("forced\n"),
            "redirected printf output missing, got {got:?}"
        );
    }

    #[test]
    fn bad_redirection_skips_command_status_1() {
        assert_eq!(status("true > /nonexistent-dir-xyz/f"), 1);
        assert_eq!(status("cat < /no/such/file-xyz"), 1);
        // Shell survives.
        assert_eq!(status("true > /nonexistent-dir-xyz/f; true"), 0);
    }

    #[test]
    fn shift_changes_positionals() {
        assert_eq!(status("set -- a b c; shift 2; [ \"$1\" = c ]"), 0);
        assert_eq!(status("set -- a; shift 5"), 1);
    }

    #[test]
    fn group_runs_in_current_shell() {
        let mut e = Engine::new();
        let prog = brish_core::parser::parse("{ x=1; }").unwrap();
        e.run(&prog).unwrap();
        assert_eq!(e.env.get("x"), Some("1"));
    }

    #[test]
    fn noexec_parses_but_skips() {
        assert_eq!(status("set -n; nosuchcmd123"), 0);
    }

    #[test]
    fn command_not_found_is_127_and_nonfatal() {
        assert_eq!(status("nosuchcmd-definitely-xyz"), 127);
        assert_eq!(status("nosuchcmd-definitely-xyz; true"), 0);
    }

    // ---- plugin hooks (plan 6.6) ----

    use brish_plugin_api::{ChdirHook, PostExecHook, PreExecHook, PromptSegment, Theme};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Spy(Arc<Mutex<Vec<String>>>);

    impl Spy {
        fn log(&self, s: String) {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).push(s);
        }
        fn lines(&self) -> Vec<String> {
            self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
        }
    }

    impl PreExecHook for Spy {
        fn before(&self, ctx: &CmdCtx<'_>) -> HookAction {
            self.log(format!("pre:{}", ctx.argv[0]));
            HookAction::Continue
        }
    }
    impl PostExecHook for Spy {
        fn after(&self, ctx: &CmdCtx<'_>, status: i32) {
            self.log(format!("post:{}:{status}", ctx.argv[0]));
        }
    }
    impl ChdirHook for Spy {
        fn on_cd(&self, old: &Path, new: &Path) {
            self.log(format!("cd:{}->{}", old.display(), new.display()));
        }
    }

    fn engine_with(reg: Registry) -> Engine {
        let mut e = Engine::new();
        e.set_hooks(Arc::new(reg));
        e
    }

    fn run_src(e: &mut Engine, src: &str) -> i32 {
        let prog = brish_core::parser::parse(src).expect("parse");
        match e.run(&prog) {
            Ok(Outcome::Status(s)) | Ok(Outcome::Exit(s)) => s,
            Err(err) => panic!("fatal: {err}"),
        }
    }

    /// Engine with interactive flag set (alias expansion gate).
    fn interactive_engine() -> Engine {
        let mut e = Engine::new();
        e.env.flags.push('i');
        e
    }

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("echo", "echo"), 0);
        assert_eq!(edit_distance("echo", "ech"), 1);
        assert_eq!(edit_distance("exit", "exits"), 1);
        assert_eq!(edit_distance("ls", "echo"), 4);
        assert_eq!(edit_distance("", "abc"), 3);
    }

    #[test]
    fn suggest_command_finds_close_builtins() {
        let e = interactive_engine();
        assert_eq!(e.suggest_command("ech").as_deref(), Some("echo"));
        assert_eq!(e.suggest_command("exprt").as_deref(), Some("export"));
        // too short / nothing close → no advice
        assert_eq!(e.suggest_command("x"), None);
        assert_eq!(e.suggest_command("zzzzzzzz"), None);
    }

    #[test]
    fn command_not_found_hook_runs_without_changing_status() {
        let mut e = interactive_engine();
        // hook that fails must not turn 127 into 1
        e.set_command_not_found(Some("false".to_string()));
        assert_eq!(run_src(&mut e, "nosuchcmd-xyz"), 127);
        // hook output is the answer; our own status survives
        e.set_command_not_found(Some("echo found-it".to_string()));
        assert_eq!(run_src(&mut e, "nosuchcmd-xyz"), 127);
        // batch (no `i` flag): hook never runs
        let mut b = Engine::new();
        b.set_command_not_found(Some("echo found-it".to_string()));
        assert_eq!(run_src(&mut b, "nosuchcmd-xyz"), 127);
    }

    #[test]
    fn quote_word_is_shell_safe() {
        assert_eq!(quote_word("ls"), "'ls'");
        assert_eq!(quote_word("it's"), r"'it'\''s'");
        assert_eq!(quote_word("a b"), "'a b'");
    }

    #[test]
    fn aliases_expand_interactive_only() {
        let mut e = interactive_engine();
        run_src(&mut e, "alias f=false");
        assert_eq!(run_src(&mut e, "f"), 1, "alias expands in interactive");
        // Batch: no `i` flag — alias never expands, command not found.
        let mut b = Engine::new();
        run_src(&mut b, "alias f=false");
        assert_eq!(run_src(&mut b, "f"), 127, "batch must not expand aliases");
    }

    #[test]
    fn alias_chain_and_cycle_guard() {
        let mut e = interactive_engine();
        run_src(&mut e, "alias a=b; alias b=true");
        assert_eq!(run_src(&mut e, "a"), 0, "alias chain a->b->true");
        // Self-loop must terminate, not hang: `a` -> `a x`, seen, stop;
        // argv stays `a x` -> command a not found.
        run_src(&mut e, "alias a='a x'");
        assert_eq!(run_src(&mut e, "a"), 127);
    }

    #[test]
    fn alias_suppressed_by_command_and_unalias() {
        let mut e = interactive_engine();
        run_src(&mut e, "alias true=false");
        // `command` suppresses expansion (POSIX).
        assert_eq!(run_src(&mut e, "command true"), 0);
        // unalias removes it.
        run_src(&mut e, "unalias true");
        assert_eq!(run_src(&mut e, "true"), 0);
    }

    #[test]
    fn plugin_hooks_fire_in_parent_in_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut reg = Registry::default();
        reg.pre_exec.push(Box::new(Spy(Arc::clone(&log))));
        reg.post_exec.push(Box::new(Spy(Arc::clone(&log))));
        let mut e = engine_with(reg);
        assert_eq!(run_src(&mut e, "false"), 1);
        assert_eq!(
            Spy(Arc::clone(&log)).lines(),
            vec!["pre:false".to_string(), "post:false:1".to_string()]
        );
        assert_eq!(e.env.status, 1);
    }

    struct AbortTouch;
    impl PreExecHook for AbortTouch {
        fn before(&self, ctx: &CmdCtx<'_>) -> HookAction {
            if ctx.argv[0] == "touch" {
                HookAction::Abort(7)
            } else {
                HookAction::Continue
            }
        }
    }

    #[test]
    fn abort_skips_command_and_sets_status() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let file = dir.path().join("nope");
        let mut reg = Registry::default();
        reg.pre_exec.push(Box::new(AbortTouch));
        let mut e = engine_with(reg);
        let src = format!("touch {}", file.display());
        assert_eq!(run_src(&mut e, &src), 7);
        assert_eq!(e.env.status, 7);
        assert!(!file.exists(), "aborted command must not run");
        // other commands unaffected
        assert_eq!(run_src(&mut e, "true"), 0);
    }

    #[test]
    fn hooks_skip_forked_children() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut reg = Registry::default();
        reg.pre_exec.push(Box::new(Spy(Arc::clone(&log))));
        reg.post_exec.push(Box::new(Spy(Arc::clone(&log))));
        let mut e = engine_with(reg);
        run_src(&mut e, "(true)");
        run_src(&mut e, "true | true");
        run_src(&mut e, "true &");
        run_src(&mut e, "wait");
        run_src(&mut e, "echo $(true)");
        let lines = Spy(Arc::clone(&log)).lines();
        // Only the parent-run `echo` fires; subshell/pipeline/background/
        // command-substitution stages are hook-silent.
        assert_eq!(
            lines,
            vec![
                "pre:wait".to_string(),
                "post:wait:0".to_string(),
                "pre:echo".to_string(),
                "post:echo:0".to_string(),
            ],
            "forked stages must not fire hooks: {lines:?}"
        );
    }

    #[test]
    fn chdir_hook_fires_on_success_only() {
        let _g = crate::test_util::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let start = std::env::current_dir().expect("cwd");
        let tmp = tempfile::tempdir().expect("tmpdir");
        let target = tmp.path().to_str().expect("utf8").to_string();

        let log = Arc::new(Mutex::new(Vec::new()));
        let mut reg = Registry::default();
        reg.on_chdir.push(Box::new(Spy(Arc::clone(&log))));
        let mut e = engine_with(reg);

        assert_eq!(run_src(&mut e, &format!("cd {target}")), 0);
        assert_eq!(Spy(Arc::clone(&log)).lines().len(), 1, "one chdir event");
        assert!(
            Spy(Arc::clone(&log)).lines()[0].contains(&target),
            "new dir recorded"
        );

        assert_eq!(run_src(&mut e, "cd /definitely/not/a/dir"), 1);
        assert_eq!(
            Spy(Arc::clone(&log)).lines().len(),
            1,
            "failed cd fires nothing"
        );
        let _ = std::env::set_current_dir(start);
    }

    struct Dummy(&'static str);
    impl Theme for Dummy {
        fn name(&self) -> &str {
            self.0
        }
        fn render(&self, _status: i32, _cwd: &Path, _segments: &[&dyn PromptSegment]) -> String {
            self.0.to_string()
        }
    }

    #[test]
    fn theme_builtin_switches_and_rejects_unknown() {
        let mut reg = Registry::default();
        reg.themes.push(Box::new(Dummy("a")));
        reg.themes.push(Box::new(Dummy("b")));
        let mut e = engine_with(reg);
        assert_eq!(e.theme, DEFAULT_THEME);

        assert_eq!(run_src(&mut e, "theme b"), 0);
        assert_eq!(e.theme, "b");

        assert_eq!(run_src(&mut e, "theme nope"), 1);
        assert_eq!(e.theme, "b", "unknown keeps current");

        assert_eq!(run_src(&mut e, "theme"), 0);
        assert_eq!(run_src(&mut e, "theme a"), 0);
        assert_eq!(e.theme, "a");
    }

    #[test]
    fn plugin_builtin_reports_catalog() {
        let mut reg = Registry::default();
        reg.record("x", true);
        reg.record("y", false);
        let mut e = engine_with(reg);
        assert_eq!(run_src(&mut e, "plugin"), 0);
        assert_eq!(
            e.hooks().installed(),
            &[("x".to_string(), true), ("y".to_string(), false)]
        );
    }

    #[test]
    fn fg_waits_for_background_job() {
        let mut e = Engine::new();
        let prog = brish_core::parser::parse("sleep 0.05 & fg").unwrap();
        assert_eq!(e.run(&prog).unwrap(), Outcome::Status(0));
        assert!(e.bg[0].done(), "fg must have reaped the job");
        assert_eq!(e.bg[0].status(), 0);
    }

    #[test]
    fn fg_with_no_current_job_is_status_1() {
        let mut e = Engine::new();
        let prog = brish_core::parser::parse("fg").unwrap();
        assert_eq!(e.run(&prog).unwrap(), Outcome::Status(1));
    }

    #[test]
    fn stopped_job_recovers_via_fg() {
        let mut e = Engine::new();
        let prog = brish_core::parser::parse("sleep 0.3 & kill -STOP $!").unwrap();
        e.run(&prog).unwrap();
        // Poll until the child's self-stop is observed.
        for _ in 0..200 {
            e.reap_bg();
            if e.bg[0].stopped {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(e.bg[0].stopped, "self-stopped job must reap as Stopped");
        assert!(!e.bg[0].done(), "stopped job is not done");
        let prog = brish_core::parser::parse("fg").unwrap();
        e.run(&prog).unwrap();
        assert_eq!(e.env.status, 0, "fg continues the job to completion");
        assert!(e.bg[0].done());
    }

    #[test]
    fn bg_continues_stopped_job() {
        let mut e = Engine::new();
        let prog = brish_core::parser::parse("sleep 0.3 & kill -STOP $!").unwrap();
        e.run(&prog).unwrap();
        // Generous budgets: parallel test threads steal scheduler time.
        for _ in 0..600 {
            e.reap_bg();
            if e.bg[0].stopped {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(e.bg[0].stopped);
        let prog = brish_core::parser::parse("bg").unwrap();
        e.run(&prog).unwrap();
        assert_eq!(e.env.status, 0);
        assert!(!e.bg[0].stopped, "bg resumes the job");
        for _ in 0..600 {
            e.reap_bg();
            if e.bg[0].done() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(e.bg[0].done(), "continued job runs to completion");
        assert_eq!(e.bg[0].status(), 0);
    }

    #[test]
    fn interactive_fg_stop_returns_148_and_job() {
        let mut e = Engine::new();
        e.set_job_control(true);
        let prog = brish_core::parser::parse("sh -c 'kill -STOP $$'").unwrap();
        e.run(&prog).unwrap();
        assert_eq!(e.env.status, STOPPED_STATUS, "128 + SIGTSTP");
        assert_eq!(e.bg.len(), 1);
        assert!(e.bg[0].stopped);
        // `jobs` reports it; fg resumes to completion.
        let prog = brish_core::parser::parse("fg").unwrap();
        e.run(&prog).unwrap();
        assert_eq!(e.env.status, 0);
        assert!(e.bg[0].done());
    }

    #[test]
    fn fg_waits_multi_pid_job_last_status_wins() {
        // Pipeline-shaped job (two members), `fg` waits every member
        // and the last stage's status wins (POSIX). Stop/resume is
        // covered by `stopped_job_recovers_via_fg` + the batch test.
        let mut e = Engine::new();
        let spawn = || {
            brish_platform::proc::fork_spawn(vec![], true, || {
                std::thread::sleep(std::time::Duration::from_millis(20));
                7
            })
            .unwrap()
        };
        let a = spawn();
        let b = spawn();
        e.bg.push(Job {
            pids: vec![a, b],
            st: vec![None, None],
            cmd: "sleeper | sleeper".into(),
            stopped: false,
            tty_held: false,
            notified: false,
            own_pgrp: true,
        });
        let prog = brish_core::parser::parse("fg").unwrap();
        e.run(&prog).unwrap();
        assert_eq!(e.env.status, 7, "last stage wins");
        assert!(e.bg[0].done());
    }

    #[test]
    fn notify_reports_done_jobs_once() {
        let mut e = Engine::new();
        let prog = brish_core::parser::parse("true &").unwrap();
        e.run(&prog).unwrap();
        for _ in 0..200 {
            e.reap_bg();
            if e.bg.iter().all(|j| j.done()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let notes = e.job_notifications().expect("no exit from drain");
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(
            notes[0].contains("[1]+") && notes[0].contains("Done"),
            "{}",
            notes[0]
        );
        assert!(
            e.job_notifications()
                .expect("no exit from drain")
                .is_empty(),
            "notify once"
        );
    }

    /// FD-leak smoke: pipelines/redirs/substs must not grow the
    /// shell's descriptor table (plan 4.11; /proc/self/fd on Linux —
    /// macOS has no equivalent cheap check, so gate on Linux).
    #[cfg(target_os = "linux")]
    #[test]
    fn no_fd_leak_across_pipelines_and_redirs() {
        fn count_fds() -> usize {
            std::fs::read_dir("/proc/self/fd")
                .map(|rd| rd.count())
                .unwrap_or(0)
        }
        let mut e = Engine::new();
        let dir = tempfile::tempdir().expect("tmpdir");
        let out = dir.path().join("o");
        // Warm any once-init fds (PATH scan, etc.) first.
        run_src(&mut e, "echo x | cat > /dev/null");
        let before = count_fds();
        for i in 0..25 {
            run_src(&mut e, "echo x | cat | cat > /dev/null");
            run_src(&mut e, &format!("echo {i} >> {}", out.display()));
            run_src(&mut e, "x=$(echo inner) ; true");
        }
        let after = count_fds();
        assert!(
            after <= before + 2,
            "fd leak: {before} -> {after} descriptors"
        );
    }

    #[test]
    fn trap_int_fires_during_foreground_wait() {
        let _trap_g = crate::test_util::TRAP_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // Proof of mid-wait delivery: SIGINT lands while `sleep` is
        // blocked in waitpid; the marker must vanish BEFORE the sleep
        // finishes (a boundary-only drain would remove it ≥600ms).
        let dir = tempfile::tempdir().expect("tmpdir");
        let marker = dir.path().join("trapped");
        std::fs::write(&marker, "x").expect("write");
        let mut e = Engine::new();
        let src = format!("trap 'rm -f {}' INT", marker.display());
        assert_eq!(run_src(&mut e, &src), 0);

        let t = std::thread::spawn(move || {
            run_src(&mut e, "sleep 1.0");
            e
        });
        // Resend SIGINT while polling: a signal that lands before
        // waitpid is entered sets the flag but cannot EINTR it, and
        // parallel tests' drains may consume our flag — several sends
        // raise the odds one EINTRs *this* wait. Mid-wait is proven by
        // timing: the marker must vanish before the 1.0s sleep ends.
        let start = std::time::Instant::now();
        let mut removed = None;
        for _ in 0..200 {
            brish_platform::send_signal(
                std::process::id() as i32,
                brish_platform::signal_by_name("INT").unwrap_or(2),
            )
            .expect("kill self");
            std::thread::sleep(std::time::Duration::from_millis(10));
            if !marker.exists() {
                removed = Some(start.elapsed());
                break;
            }
        }
        match removed {
            // Removal before the sleep finished ⇒ drained mid-wait.
            Some(removed) if removed.as_millis() < 950 => {
                eprintln!("trap ran {removed:?} into a 1s wait");
            }
            other => {
                assert!(
                    other.is_some(),
                    "trap never ran; delta {:?}",
                    start.elapsed()
                );
                panic!("trap ran at boundary, not mid-wait: {:?}", other);
            }
        }
        let mut e = t.join().expect("sleep thread");
        // Drop the entry but KEEP the handler: restoring SIG_DFL while
        // a self-directed SIGINT may still be pending lets it kill the
        // suite (delivery can lag past kill() on other threads).
        e.env.traps.remove("INT");
    }

    #[test]
    fn relconf_swaps_registry_and_theme() {
        let mut e = Engine::new();
        e.theme = "briiish".into();
        e.set_relconf(Arc::new(|| {
            let mut reg = Registry::default();
            struct T;
            impl brish_plugin_api::Theme for T {
                fn name(&self) -> &str {
                    "t2"
                }
                fn render(
                    &self,
                    _s: i32,
                    _c: &std::path::Path,
                    _seg: &[&dyn brish_plugin_api::PromptSegment],
                ) -> String {
                    "t2 ".into()
                }
            }
            reg.themes.push(Box::new(T));
            Reload {
                registry: reg,
                theme: "t2".into(),
                command_not_found: Some("echo reloaded".into()),
            }
        }));
        assert_eq!(run_src(&mut e, "relconf"), 0);
        assert_eq!(e.theme, "t2");
        assert_eq!(e.hooks.themes.len(), 1);
        // the reload also refreshes `[hooks] command_not_found`
        assert_eq!(e.not_found_hook.as_deref(), Some("echo reloaded"));
        // usage error
        assert_eq!(run_src(&mut e, "relconf now"), 2);
        // `-e` edits first, then reloads (editor behaviour itself is
        // covered end-to-end in tests/batch.rs — this crate forbids
        // `unsafe`, and Rust 2024 makes env mutation unsafe).
        assert_eq!(run_src(&mut e, "relconf -e extra"), 2);
        // no hook → status 1, shell survives
        let mut plain = Engine::new();
        assert_eq!(run_src(&mut plain, "relconf"), 1);
    }

    #[test]
    fn local_scopes_to_function() {
        // global value restored after the function returns
        assert_eq!(
            status("x=0; f() { local x=1; [ \"$x\" -eq 1 ]; }; f; [ \"$x\" -eq 0 ]"),
            0
        );
        // a local that did not exist before vanishes on return
        assert_eq!(status("f() { local fresh=1; }; f; [ -z \"${fresh-}\" ]"), 0);
        // nested frames: inner local does not clobber outer local
        assert_eq!(
            status(
                "outer() { local v=1; inner; [ \"$v\" -eq 1 ]; }; \
                 inner() { local v=2; [ \"$v\" -eq 2 ]; }; outer"
            ),
            0
        );
        // outside a function → error, shell survives
        assert_eq!(status("local y=1"), 1);
        assert_eq!(status("true"), 0);
    }

    #[test]
    fn getopts_end_to_end() {
        // clustered options, option-argument, silent mode
        let src = "\
            parse() { \
                local opt; \
                OPTIND=1; \
                while getopts ':ab:c' opt; do \
                    case \"$opt\" in \
                        a) echo A ;; \
                        b) echo \"B:$OPTARG\" ;; \
                        c) echo C ;; \
                        :) echo \"MISS:$OPTARG\" ;; \
                        \\?) echo BAD ;; \
                    esac; \
                done; \
                echo \"left:$OPTIND\"; \
            }; \
            parse -a -b val -c";
        assert_eq!(status(src), 0);
    }

    #[test]
    fn getopts_builtin_unit() {
        let mut e = Env::new();
        let g = |e: &mut Env, spec: &str, extra: &[&str]| -> i32 {
            let mut argv = vec!["getopts".to_string(), spec.to_string(), "opt".to_string()];
            argv.extend(extra.iter().map(|s| s.to_string()));
            match run_builtin(BuiltIn::Getopts, &argv, e).expect("getopts") {
                Flow::Status(s) => s,
                other => panic!("unexpected flow {other:?}"),
            }
        };
        // OPTIND persists across calls — reset between scenarios.
        let reset = |e: &mut Env| e.set_unchecked("OPTIND", "1");
        // cluster -ac: two chars across two calls, then end
        assert_eq!(g(&mut e, ":ac", &["-ac"]), 0);
        assert_eq!(e.get("opt"), Some("a"));
        assert_eq!(g(&mut e, ":ac", &["-ac"]), 0);
        assert_eq!(e.get("opt"), Some("c"));
        assert_eq!(g(&mut e, ":ac", &["-ac"]), 1, "cluster done");
        // option-argument from next operand
        reset(&mut e);
        assert_eq!(g(&mut e, ":f:", &["-f", "file"]), 0);
        assert_eq!(e.get("opt"), Some("f"));
        assert_eq!(e.get("OPTARG"), Some("file"));
        assert_eq!(e.get("OPTIND"), Some("3"));
        // silent missing-arg: ':' + OPTARG
        reset(&mut e);
        assert_eq!(g(&mut e, ":f:", &["-f"]), 1);
        assert_eq!(e.get("opt"), Some(":"));
        assert_eq!(e.get("OPTARG"), Some("f"));
        // unknown option in silent mode
        let mut e2 = Env::new();
        assert_eq!(g(&mut e2, ":a", &["-z"]), 0);
        assert_eq!(e2.get("opt"), Some(":"));
        assert_eq!(e2.get("OPTARG"), Some("z"));
        // end of operands
        let mut e3 = Env::new();
        assert_eq!(g(&mut e3, ":a", &["x"]), 1);
    }

    #[test]
    fn exit_trap_runs_at_process_exit() {
        let _trap_g = crate::test_util::TRAP_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut e = Engine::new();
        run_src(&mut e, "trap 'echo bye > /nonexistent-xyz' EXIT");
        // EXIT body runs via run_exit_trap; parse/garbage never panics
        e.run_exit_trap();
        // setting + listing through the builtin path
        run_src(&mut e, "trap 'true' EXIT; trap - EXIT");
        assert!(!e.env.traps.contains_key("EXIT"));
    }

    #[test]
    fn trap_builtin_stores_and_lists() {
        let _trap_g = crate::test_util::TRAP_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut e = Engine::new();
        assert_eq!(run_src(&mut e, "trap 'echo hi' INT"), 0);
        assert_eq!(e.env.traps.get("INT").map(String::as_str), Some("echo hi"));
        assert_eq!(run_src(&mut e, "trap 'echo bye' TERM HUP"), 0);
        assert!(e.env.traps.contains_key("TERM"));
        assert!(e.env.traps.contains_key("HUP"));
        // SIG prefix accepted
        assert_eq!(run_src(&mut e, "trap 'true' SIGQUIT"), 0);
        assert!(e.env.traps.contains_key("QUIT"));
        // invalid signal -> status 1, shell survives
        assert_eq!(run_src(&mut e, "trap 'true' NOPE"), 1);
        assert_eq!(run_src(&mut e, "trap 'true' KILL"), 1);
        // reset
        assert_eq!(run_src(&mut e, "trap - INT"), 0);
        assert!(!e.env.traps.contains_key("INT"));
        // bad body parses later — storing is fine, drain swallows errors
        assert_eq!(run_src(&mut e, "trap 'if' INT"), 0);
        // Clear entries but keep handlers installed (restoring DFL
        // races a pending self-signal — see trap_int test).
        e.env.traps.clear();
        assert!(e.env.traps.is_empty());
    }

    #[test]
    fn trap_signal_fires_drain() {
        let _trap_g = crate::test_util::TRAP_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut e = Engine::new();
        // marker file the trap will remove
        let dir = tempfile::tempdir().expect("tmpdir");
        let marker = dir.path().join("trapped");
        std::fs::write(&marker, "x").expect("write");
        let src = format!("trap 'rm -f {}' INT", marker.display());
        assert_eq!(run_src(&mut e, &src), 0);
        // Inject the pending bit rather than sending a real SIGINT — a
        // real kill races with parallel tests restoring SIG_DFL and
        // would terminate the whole test process. The flag word is
        // process-global, so a parallel test's drain may steal the bit;
        // re-inject until *this* engine observes it.
        for _ in 0..100 {
            brish_platform::inject_pending_trap(brish_platform::TRAP_BIT_INT);
            assert_eq!(run_src(&mut e, "true"), 0, "drain runs the INT trap");
            if !marker.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(!marker.exists(), "trap body must have run (marker removed)");
        // Drop entry, keep handler (DFL-restore is unsafe with a
        // possibly-pending self-signal — see trap_int test).
        e.env.traps.remove("INT");
        // pending flags without a matching trap entry run nothing
        brish_platform::inject_pending_trap(brish_platform::TRAP_BIT_INT);
        std::fs::write(&marker, "x").expect("write");
        assert_eq!(run_src(&mut e, "true"), 0);
        assert!(marker.exists(), "no trap entry → no body");
    }
}
