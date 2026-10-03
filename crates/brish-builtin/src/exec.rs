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

use crate::{BuiltIn, Flow, run as run_builtin};
use brish_platform::proc::{
    FdOp, FdSetup, fork_run, fork_spawn, preexec_fd_ops, send_signal, try_wait, wait_pid, with_fds,
};

use brish_platform::RawFd;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::{Command as Proc, Stdio};

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

/// A background job: pid, display text (for `jobs`), cached status
/// once reaped (so `wait` still sees it).
struct Job {
    pid: i32,
    cmd: String,
    status: Option<i32>,
}

/// The shell: environment, function table, background jobs.
pub struct Engine {
    pub env: Env,
    funcs: HashMap<String, Cmd>,
    depth: usize,
    bg: Vec<Job>,
    /// Did the last batch of expansions run a command substitution?
    cs_seen: bool,
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
    Some(match spec.to_ascii_uppercase().as_str() {
        "HUP" => 1,
        "INT" => 2,
        "QUIT" => 3,
        "KILL" => 9,
        "USR1" => 10,
        "USR2" => 12,
        "TERM" => 15,
        "CONT" => 18,
        "TSTP" => 20,
        _ => return None,
    })
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

fn decode_status(st: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = st.signal() {
            return 128 + sig;
        }
    }
    st.code().unwrap_or(1)
}

// Pipe ends convert to File through each platform's owned-handle type.
#[cfg(unix)]
fn pipe_reader_file(r: std::io::PipeReader) -> File {
    File::from(std::os::fd::OwnedFd::from(r))
}

#[cfg(not(unix))]
fn pipe_reader_file(r: std::io::PipeReader) -> File {
    File::from(std::os::windows::io::OwnedHandle::from(r))
}

#[cfg(unix)]
fn pipe_writer_file(w: std::io::PipeWriter) -> File {
    File::from(std::os::fd::OwnedFd::from(w))
}

#[cfg(not(unix))]
fn pipe_writer_file(w: std::io::PipeWriter) -> File {
    File::from(std::os::windows::io::OwnedHandle::from(w))
}

/// Open `path` for `>`/`>>`/`>|` honoring `noclobber` (`>|` forces).
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
            cs_seen: false,
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
    let code = wait_pid(pid).map_err(|e| Error::Exec(e.to_string()))?;
    Ok((out, code))
}

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
            cs_seen: false,
        }
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

    /// Reap finished background jobs (opportunistic; no zombies pile up),
    /// caching status for a later `wait`.
    pub fn reap_bg(&mut self) {
        for j in &mut self.bg {
            if j.status.is_none()
                && let Ok(Some(code)) = try_wait(j.pid)
            {
                j.status = Some(code);
                if brish_core::debug_on("jobs") {
                    eprintln!("brish[jobs]: reaped {} -> {code}", j.pid);
                }
            }
        }
        // ponytail: remember at most 64 finished jobs; drop oldest done.
        while self.bg.iter().filter(|j| j.status.is_some()).count() > 64 {
            match self.bg.iter().position(|j| j.status.is_some()) {
                Some(i) => {
                    self.bg.remove(i);
                }
                None => break,
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
        self.and_or(&item.andor, errexit_ctx)
    }

    fn background(&mut self, ao: &ast::AndOr) -> R<()> {
        let _ = Write::flush(&mut std::io::stdout());
        let ao = ao.clone();
        let text = ao_text(&ao);
        let pid = fork_spawn(Vec::new(), true, || match self.and_or(&ao, true) {
            Ok(()) => self.env.status,
            Err(Stop::Exit(c)) | Err(Stop::Return(c)) => c,
            Err(Stop::Break(_)) | Err(Stop::Continue(_)) => self.env.status,
            Err(Stop::Fail(e)) => {
                eprintln!("brish: {e}");
                1
            }
        })
        .map_err(platform_err)?;
        self.env.status = 0;
        self.env.last_bg = Some(pid as u32);
        self.bg.push(Job {
            pid,
            cmd: text,
            status: None,
        });
        Ok(())
    }

    // ---- job control (plan 4.10 minimal: wait / jobs / kill) ----

    /// Blocking wait for the job at `i`; caches and returns its status,
    /// or `None` if the pid is not a child of this shell.
    fn wait_at(&mut self, i: usize) -> Option<i32> {
        if let Some(c) = self.bg[i].status {
            return Some(c);
        }
        match wait_pid(self.bg[i].pid) {
            Ok(code) => {
                self.bg[i].status = Some(code);
                Some(code)
            }
            Err(_) => None,
        }
    }

    fn wait_cmd(&mut self, argv: &[String]) {
        if argv.len() == 1 {
            // POSIX: no operands waits for every known job. bash returns 0
            // even when a waited job failed (`false & wait` → 0).
            for i in 0..self.bg.len() {
                let _ = self.wait_at(i);
            }
            self.env.status = 0;
            return;
        }
        let mut status = 0;
        for arg in &argv[1..] {
            if let Some(n) = arg.strip_prefix('%') {
                let idx = n.parse::<usize>().ok().and_then(|n| n.checked_sub(1));
                status = match idx {
                    Some(i) if i < self.bg.len() => self.wait_at(i).unwrap_or(127),
                    _ => {
                        eprintln!("brish: wait: {arg}: no such job");
                        127
                    }
                };
            } else {
                let idx = arg
                    .parse::<i32>()
                    .ok()
                    .and_then(|pid| self.bg.iter().position(|j| j.pid == pid));
                status = match idx {
                    Some(i) => self.wait_at(i).unwrap_or(127),
                    _ => {
                        eprintln!("brish: wait: {arg}: not a child of this shell");
                        127
                    }
                };
            }
        }
        self.env.status = status;
    }

    fn jobs_cmd(&mut self) {
        self.reap_bg();
        for (n, j) in self.bg.iter().enumerate() {
            let state = if j.status.is_some() {
                "Done"
            } else {
                "Running"
            };
            println!("[{}]+  {}  {} &", n + 1, state, j.cmd);
        }
    }

    fn kill_cmd(&mut self, argv: &[String]) {
        let mut sig: i32 = 15; // SIGTERM, POSIX default
        let mut targets: Vec<&String> = Vec::new();
        for a in &argv[1..] {
            if let Some(spec) = a.strip_prefix('-') {
                match parse_signal(spec) {
                    Some(v) => sig = v,
                    None => {
                        eprintln!("brish: kill: {a}: invalid signal spec");
                        self.env.status = 1;
                        return;
                    }
                }
            } else {
                targets.push(a);
            }
        }
        if targets.is_empty() {
            eprintln!("brish: kill: usage: kill [-SIGNAME | -N] pid | %job ...");
            self.env.status = 1;
            return;
        }
        let mut status = 0;
        for t in targets {
            let pid = if let Some(n) = t.strip_prefix('%') {
                n.parse::<usize>()
                    .ok()
                    .and_then(|n| n.checked_sub(1))
                    .filter(|i| *i < self.bg.len())
                    .map(|i| self.bg[i].pid)
            } else {
                t.parse::<i32>().ok()
            };
            match pid {
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
        self.env.status = status;
    }

    fn and_or(&mut self, ao: &ast::AndOr, errexit_ctx: bool) -> R<()> {
        use ast::AndOrOp;
        self.pipeline(&ao.first)?;
        if ao.rest.is_empty() {
            return self.check_errexit(errexit_ctx, ao.first.negated);
        }
        for (i, (op, pipe)) in ao.rest.iter().enumerate() {
            let failed = self.env.status != 0;
            match op {
                AndOrOp::And if failed => return Ok(()),
                AndOrOp::Or if !failed => return Ok(()),
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
            self.pipeline_multi(&p.cmds)?;
        }
        if p.negated {
            self.env.status = i32::from(self.env.status == 0);
        }
        Ok(())
    }

    /// Every stage runs in a forked child whose pipe fds are applied on
    /// entry. Parent spawns all stages first (concurrency!), then waits
    /// in order; the last stage's status wins (POSIX).
    fn pipeline_multi(&mut self, cmds: &[Cmd]) -> R<()> {
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
            #[cfg(unix)]
            for (r, w) in &pipes {
                for raw in [r.as_raw_fd(), w.as_raw_fd()] {
                    if raw > 1 && !setups.iter().any(|(t, _)| *t == raw) {
                        setups.push((raw, FdSetup::Close));
                    }
                }
            }
            #[cfg(not(unix))]
            let _ = &pipes;
            let stage = stage.clone();
            let pid = fork_spawn(setups, false, || match self.cmd(&stage) {
                Ok(()) => self.env.status,
                Err(Stop::Exit(c)) | Err(Stop::Return(c)) => c,
                Err(Stop::Break(_)) | Err(Stop::Continue(_)) => self.env.status,
                Err(Stop::Fail(e)) => {
                    eprintln!("brish: {e}");
                    1
                }
            })
            .map_err(platform_err)?;
            pids.push(pid);
        }
        // Drop our copies now that every stage holds its own.
        drop(pipes);

        let mut last = 0;
        for pid in pids {
            last = wait_pid(pid).map_err(platform_err)?;
        }
        self.env.status = last;
        Ok(())
    }

    // ---- commands ----

    fn cmd(&mut self, c: &Cmd) -> R<()> {
        match c {
            Cmd::Simple(s) => self.simple(s, &[]),
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
        let code = fork_run(plan_setups(plan), || match self.program(p, true) {
            Ok(()) => self.env.status,
            Err(Stop::Exit(c)) | Err(Stop::Return(c)) => c,
            Err(Stop::Break(_)) | Err(Stop::Continue(_)) => self.env.status,
            Err(Stop::Fail(e)) => {
                eprintln!("brish: {e}");
                1
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
        let argv = if cmd_words.is_empty() {
            Vec::new()
        } else {
            self.xwords(cmd_words)?
        };
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

    fn push_temp(
        &mut self,
        assigns: &[(String, String)],
    ) -> Result<Vec<(String, Option<String>)>, ()> {
        let mut saved = Vec::with_capacity(assigns.len());
        for (n, v) in assigns {
            let prev = self.env.get(n).map(str::to_owned);
            if self.env.set(n, v).is_err() {
                eprintln!("brish: {n}: readonly variable");
                self.pop_temp(saved);
                self.env.status = 1;
                return Err(());
            }
            saved.push((n.clone(), prev));
        }
        Ok(saved)
    }

    fn pop_temp(&mut self, saved: Vec<(String, Option<String>)>) {
        for (n, prev) in saved.into_iter().rev() {
            match prev {
                Some(v) => {
                    let _ = self.env.set(&n, v);
                }
                None => {
                    let _ = self.env.unset(&n);
                }
            }
        }
    }

    /// Dispatch an already-expanded argv (no re-expansion).
    fn exec_words(&mut self, argv: Vec<String>, plan: Plan, skip_funcs: bool) -> R<()> {
        let name = argv[0].clone();

        if !skip_funcs && self.funcs.contains_key(&name) {
            let body = self.funcs[&name].clone();
            return self.call_function(body, argv, plan);
        }
        // These builtins need the shell itself:
        if name == "eval" {
            return self.eval_cmd(&argv, plan);
        }
        if name == "." || name == "source" {
            return self.source_cmd(&argv, plan);
        }
        // Job-control builtins need the engine's job table (plan 4.10).
        if name == "wait" || name == "jobs" || name == "kill" {
            return apply_plan(plan, || match name.as_str() {
                "wait" => self.wait_cmd(&argv),
                "jobs" => self.jobs_cmd(),
                _ => self.kill_cmd(&argv),
            });
        }
        if let Some(b) = BuiltIn::from_name(&name) {
            if b == BuiltIn::Command {
                // Keep the original plan: `command ls > f` redirects the
                // *target*, but `command -v x > f` redirects the builtin.
                let run_plan = plan_clone(&plan)?;
                let res = apply_plan(run_plan, || run_builtin(b, &argv, &mut self.env))?;
                return match res? {
                    Flow::Status(s) => {
                        self.env.status = s;
                        Ok(())
                    }
                    Flow::Exec(v) => self.exec_words(v, plan, true),
                    other => self.flow(other),
                };
            }
            let res = apply_plan(plan, || run_builtin(b, &argv, &mut self.env))?;
            return self.flow(res?);
        }
        self.spawn_external(&argv, plan)
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
        // ponytail: engine frames are chunky (~KBs); 200 keeps runaway
        // recursion off the 2MiB test/REPL stack. Raise if frames shrink.
        if self.depth >= 200 {
            eprintln!("brish: {}: function nesting limit", argv[0]);
            self.env.status = 1;
            return Ok(());
        }
        let saved_pos = self.env.positional().to_vec();
        self.env.set_positional(argv[1..].to_vec());
        self.depth += 1;
        let res = apply_plan(plan, || self.cmd(&body));
        self.depth -= 1;
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
        #[cfg(unix)]
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
        #[cfg(unix)]
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
        #[cfg(not(unix))]
        if plan.keys().any(|&fd| fd >= 3) {
            return Err(Stop::Fail(Error::Exec(
                "redirection to fd >= 3 unsupported on this platform".into(),
            )));
        }
        preexec_fd_ops(&mut cmd, ops);

        match cmd.spawn() {
            Ok(mut child) => {
                let st = child
                    .wait()
                    .map_err(|e| Stop::Fail(Error::Exec(format!("{name}: wait: {e}"))))?;
                self.env.status = decode_status(st);
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
                // Anonymous temp file: unlinked at creation, no leak, 0600.
                Ok(match tempfile::tempfile() {
                    Ok(mut f) => {
                        if let Err(e) = f
                            .write_all(body.as_bytes())
                            .and_then(|_| f.seek(SeekFrom::Start(0)))
                        {
                            Err(format!("heredoc: {e}"))
                        } else {
                            Ok((*fd, Src::File(f)))
                        }
                    }
                    Err(e) => Err(format!("heredoc: {e}")),
                })
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
            if e.bg.iter().all(|j| j.status.is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(
            e.bg.iter().all(|j| j.status.is_some()),
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
        // kill a running background job, then wait → 128+SIGTERM.
        let mut e = Engine::new();
        let prog = brish_core::parser::parse("sleep 5 & kill %1; wait %1").unwrap();
        assert_eq!(e.run(&prog).unwrap(), Outcome::Status(143));
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
        // and never reaches the redirected file under test.
        let src = format!(
            "set -o noclobber; printf 'forced\\n' >| {}; true",
            f.display()
        );
        assert_eq!(status(&src), 0);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "forced\n");
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
}
