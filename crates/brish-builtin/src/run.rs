//! Built-in implementations.
//!
//! All diagnostics go to stderr; usage errors return status 2, runtime
//! failures 1 (bash convention). `args[0]` is the command name.

use std::io::{BufRead, Write};

use brish_core::env::Env;
use brish_core::error::Error;
use brish_core::path::find_in_path;
use brish_words::eval_test;

use crate::builtins::BuiltIn;

/// What the shell does after a built-in runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flow {
    /// Continue with this exit status.
    Status(i32),
    /// Terminate the shell (`exit N`).
    Exit(i32),
    /// Return from the enclosing function or `source` (`return N`).
    Return(i32),
    /// Run these already-expanded args as a command (`command cmd ...`).
    Exec(Vec<String>),
    /// `break [n]` out of `n` enclosing loops.
    Break(usize),
    /// `continue [n]` to the next iteration of loop `n`.
    Continue(usize),
}

/// Run built-in `b` with full argv (`args[0]` = command name).
pub fn run(b: BuiltIn, args: &[String], env: &mut Env) -> Result<Flow, Error> {
    match b {
        BuiltIn::Echo => Ok(echo(args)),
        BuiltIn::Cd => Ok(cd(args, env)),
        BuiltIn::Pwd => Ok(pwd(args, env)),
        BuiltIn::Exit => Ok(exit(args, env)),
        BuiltIn::Return => Ok(ret(args, env)),
        BuiltIn::Export => Ok(export(args, env)),
        BuiltIn::Readonly => Ok(readonly(args, env)),
        BuiltIn::Unset => Ok(unset(args, env)),
        BuiltIn::Set => Ok(set(args, env)),
        BuiltIn::Shift => Ok(shift(args, env)),
        BuiltIn::Read => read(args, env),
        BuiltIn::Test => Ok(test(args)),
        BuiltIn::Command => Ok(command(args, env)),
        BuiltIn::Type => Ok(r#type(args, env)),
        BuiltIn::Colon => Ok(Flow::Status(0)),
        BuiltIn::Break => Ok(loop_control(args, Flow::Break)),
        BuiltIn::Continue => Ok(loop_control(args, Flow::Continue)),
        BuiltIn::Z => Ok(z_cmd(args, env)),
        BuiltIn::History => Ok(history_cmd(args)),
        BuiltIn::Alias | BuiltIn::Source | BuiltIn::Trap => {
            Err(Error::Exec(format!("{}: not yet implemented", b.name())))
        }
    }
}

/// Dispatch `history` builtin.
/// Without args, lists the history file. `history -c` clears it.
fn history_cmd(args: &[String]) -> Flow {
    let path = crate::paths::history_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            // No history file yet — nothing to list.
            if args.is_empty() || args[0] != "-c" {
                return Flow::Status(0);
            }
            let _ = std::fs::write(&path, "");
            return Flow::Status(0);
        }
    };

    if args.is_empty() || args[0] != "-c" {
        for line in text.lines() {
            let s = line.trim();
            if !s.is_empty() {
                println!("{s}");
            }
        }
    } else {
        let _ = std::fs::write(&path, "");
    }
    Flow::Status(0)
}

/// `z` frecency builtin: jump to a frecent directory (substring match).
/// `z` alone lists the top `limit` (default 10, `-N` overrides);
/// `z -c` clears the db. Visits are recorded by `cd`.
///
/// ponytail: plain `<visits>\t<epoch>\t<path>` file, no concurrent-writer
/// merge and no age-window config. Add when two shells visibly clobber.
fn z_cmd(args: &[String], env: &mut Env) -> Flow {
    let db = crate::paths::z_path();
    if args.get(1).map(String::as_str) == Some("-c") {
        let _ = std::fs::write(&db, b"");
        return Flow::Status(0);
    }

    let mut limit = 10usize;
    let mut query: Option<&str> = None;
    for a in &args[1..] {
        if let Some(n) = a.strip_prefix('-').and_then(|s| s.parse::<usize>().ok()) {
            limit = n.max(1);
        } else if query.is_none() {
            query = Some(a);
        }
    }

    let rows = crate::z::load(&db);
    let Some(q) = query else {
        for r in crate::z::rank(&rows, None).into_iter().take(limit) {
            println!("{}", r.path);
        }
        return Flow::Status(0);
    };

    let hits = crate::z::rank(&rows, Some(q));
    let Some(best) = hits.into_iter().next() else {
        return Flow::Status(1);
    };
    if std::env::set_current_dir(&best.path).is_err() {
        return Flow::Status(1);
    }
    let old = env.get("PWD").unwrap_or_default().to_string();
    env.set_unchecked("OLDPWD", &old);
    env.set_unchecked("PWD", &best.path);
    println!("{}", best.path);
    Flow::Status(0)
}

fn loop_control(args: &[String], mk: fn(usize) -> Flow) -> Flow {
    let Some(a) = args.get(1) else {
        return mk(1);
    };
    match a.parse::<usize>() {
        Ok(n) => mk(n),
        Err(_) => {
            eprintln!("break: {a}: numeric argument required");
            Flow::Status(2)
        }
    }
}

// ---- echo ----

/// `echo` output: (text, add_newline). Only `-n` runs are recognized as
/// flags; `-e`/`-E` print literally (ponytail: escapes when scripts need).
/// Leading `echo` flags: `-n` (no newline), `-e`/`-E` (escape
/// interpretation on/off; off is the default, like bash).
fn echo_text(args: &[String]) -> (String, bool, bool) {
    let mut i = 1;
    let mut newline = true;
    let mut escape = false;
    while i < args.len() {
        let a = &args[i];
        if a.starts_with('-') && a.len() > 1 && a[1..].chars().all(|c| matches!(c, 'n' | 'e' | 'E'))
        {
            for c in a[1..].chars() {
                match c {
                    'n' => newline = false,
                    'e' => escape = true,
                    'E' => escape = false,
                    _ => {}
                }
            }
            i += 1;
        } else {
            break;
        }
    }
    (args[i..].join(" "), newline, escape)
}

fn echo(args: &[String]) -> Flow {
    let (text, newline, escape) = echo_text(args);
    let (text, cut) = if escape {
        brish_core::lexer::ansi_c_decode(&text, true, true)
    } else {
        (text, false)
    };
    if newline && !cut {
        println!("{text}");
    } else {
        print!("{text}");
        let _ = std::io::stdout().flush();
    }
    Flow::Status(0)
}

// ---- cd / pwd ----

fn cd(args: &[String], env: &mut Env) -> Flow {
    let target = if args.len() == 1 {
        match env.get("HOME") {
            Some(h) => h.to_string(),
            None => {
                eprintln!("cd: HOME not set");
                return Flow::Status(1);
            }
        }
    } else if args[1] == "-" {
        match env.get("OLDPWD") {
            Some(o) => o.to_string(),
            None => {
                eprintln!("cd: OLDPWD not set");
                return Flow::Status(1);
            }
        }
    } else {
        args[1].clone()
    };

    let old_pwd = env
        .get("PWD")
        .map(str::to_string)
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        })
        .unwrap_or_default();

    let (path, via_cdpath) = resolve_cd(&target, env);
    if let Err(e) = std::env::set_current_dir(&path) {
        eprintln!("cd: {target}: {e}");
        return Flow::Status(1);
    }
    // Logical PWD (bash-style): keep the path we asked for, no symlink
    // resolution — physical lookup is `pwd -P`.
    let new_pwd = path.to_string_lossy().into_owned();
    // PWD/OLDPWD are shell bookkeeping: bypass the readonly guard (the
    // script itself cannot reassign them by accident this way either).
    env.set_unchecked("OLDPWD", old_pwd);
    env.set_unchecked("PWD", &new_pwd);
    // `cd -` and CDPATH hits both announce the new directory.
    if (args.len() > 1 && args[1] == "-") || via_cdpath {
        println!("{new_pwd}");
    }
    crate::z::record(&crate::paths::z_path(), &new_pwd, crate::z::now_epoch());
    Flow::Status(0)
}

/// Resolve a `cd` target: absolute, dot-relative, cwd, then CDPATH.
fn resolve_cd(target: &str, env: &Env) -> (std::path::PathBuf, bool) {
    use std::path::{Path, PathBuf};
    let p = Path::new(target);
    if p.is_absolute() {
        return (p.to_path_buf(), false);
    }
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let direct = cwd.join(p);
    if target.starts_with('.') || direct.exists() {
        return (direct, false);
    }
    if let Some(cdpath) = env.get("CDPATH") {
        for part in cdpath.split(':') {
            let dir = if part.is_empty() { "." } else { part };
            let cand = Path::new(dir).join(p);
            let cand = if cand.is_absolute() {
                cand
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(cand)
            };
            if cand.exists() {
                return (cand, true);
            }
        }
    }
    (direct, false)
}

fn pwd(args: &[String], env: &Env) -> Flow {
    let physical = match args.get(1).map(String::as_str) {
        None | Some("-L") => false,
        Some("-P") => true,
        Some(other) => {
            eprintln!("pwd: {other}: invalid option");
            return Flow::Status(2);
        }
    };
    let dir = if physical {
        std::env::current_dir().map(|p| p.to_string_lossy().into_owned())
    } else {
        env.get("PWD")
            .map(str::to_string)
            .ok_or_else(|| std::io::Error::other("PWD unset"))
            .or_else(|_| std::env::current_dir().map(|p| p.to_string_lossy().into_owned()))
    };
    match dir {
        Ok(d) => {
            println!("{d}");
            Flow::Status(0)
        }
        Err(e) => {
            eprintln!("pwd: {e}");
            Flow::Status(1)
        }
    }
}

// ---- exit / return ----

fn status_arg(args: &[String], env: &Env, name: &str) -> Result<i32, ()> {
    if args.len() > 2 {
        eprintln!("{name}: too many arguments");
        return Err(());
    }
    match args.get(1) {
        None => Ok(env.status.rem_euclid(256)),
        Some(a) => match a.parse::<i32>() {
            Ok(n) => Ok(n.rem_euclid(256)),
            Err(_) => {
                eprintln!("{name}: {a}: numeric argument required");
                Err(())
            }
        },
    }
}

fn exit(args: &[String], env: &Env) -> Flow {
    match status_arg(args, env, "exit") {
        Ok(n) => Flow::Exit(n),
        // Parse failures still exit (status 2), only `too many` refuses.
        Err(()) if args.len() > 2 => Flow::Status(2),
        Err(()) => Flow::Exit(2),
    }
}

fn ret(args: &[String], env: &Env) -> Flow {
    match status_arg(args, env, "return") {
        Ok(n) => Flow::Return(n),
        Err(()) => Flow::Status(2),
    }
}

// ---- export / readonly / unset ----

fn export(args: &[String], env: &mut Env) -> Flow {
    if args.len() == 1 || args.get(1).map(String::as_str) == Some("-p") {
        let mut names: Vec<&String> = env.vars_iter().map(|(k, _)| k).collect();
        names.sort();
        for n in names {
            let v = env.get_var(n);
            if v.is_some_and(|v| v.exported) {
                println!("export {}={}", n, env.get(n).unwrap_or(""));
            }
        }
        return Flow::Status(0);
    }
    let mut status = 0;
    for a in &args[1..] {
        if let Some(rest) = a.strip_prefix('-')
            && !a.eq_ignore_ascii_case("-p")
            && rest != "-"
        {
            eprintln!("export: {a}: invalid option");
            status = 2;
            continue;
        }
        if let Some((name, val)) = a.split_once('=') {
            if brish_core::env::is_name(name) {
                if env.set(name, val).is_err() || env.export(name).is_err() {
                    eprintln!("export: {name}: cannot assign");
                    status = 1;
                }
            } else {
                eprintln!("export: {name}: not a valid identifier");
                status = 1;
            }
        } else if env.export(a).is_err() {
            eprintln!("export: {a}: not a valid identifier");
            status = 1;
        }
    }
    Flow::Status(status)
}

fn readonly(args: &[String], env: &mut Env) -> Flow {
    if args.len() == 1 {
        let mut names: Vec<&String> = env.vars_iter().map(|(k, _)| k).collect();
        names.sort();
        for n in names {
            if env.get_var(n).is_some_and(|v| v.readonly) {
                println!("readonly {}={}", n, env.get(n).unwrap_or(""));
            }
        }
        return Flow::Status(0);
    }
    let mut status = 0;
    for a in &args[1..] {
        let (name, val) = match a.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (a.as_str(), None),
        };
        if !brish_core::env::is_name(name) {
            eprintln!("readonly: {name}: not a valid identifier");
            status = 1;
            continue;
        }
        if let Some(v) = val
            && env.set(name, v).is_err()
        {
            eprintln!("readonly: {name}: readonly variable");
            status = 1;
            continue;
        }
        if env.set_readonly(name).is_err() {
            eprintln!("readonly: {name}: not a valid identifier");
            status = 1;
        }
    }
    Flow::Status(status)
}

fn unset(args: &[String], env: &mut Env) -> Flow {
    let mut i = 1;
    let mut funcs = false;
    if args.get(1).map(String::as_str) == Some("-f") {
        funcs = true;
        i = 2;
    } else if args.get(1).map(String::as_str) == Some("-v") {
        i = 2;
    }
    let mut status = 0;
    for a in &args[i..] {
        if funcs {
            // Function table lands Phase 4: unsetting an absent name is a
            // no-op success (POSIX).
            continue;
        }
        if !brish_core::env::is_name(a) {
            eprintln!("unset: {a}: not a valid identifier");
            status = 1;
            continue;
        }
        if env.unset(a).is_err() {
            eprintln!("unset: {a}: readonly variable");
            status = 1;
        }
    }
    Flow::Status(status)
}

// ---- set ----

fn set(args: &[String], env: &mut Env) -> Flow {
    if args.len() == 1 {
        list_vars(env);
        return Flow::Status(0);
    }
    let mut i = 1;
    let mut assign = false;
    while i < args.len() {
        let a = &args[i];
        if a == "--" {
            i += 1;
            assign = true;
            break;
        }
        if a == "-o" || a == "+o" {
            let on = a.starts_with('-');
            if i + 1 >= args.len() {
                list_opts(env);
                return Flow::Status(0);
            }
            let name = &args[i + 1];
            if !env.opts.set_opt(name, on) {
                eprintln!("set: {name}: invalid option name");
                return Flow::Status(2);
            }
            i += 2;
            continue;
        }
        if a.starts_with('-') && a.len() > 1 {
            for c in a[1..].chars() {
                if !env.opts.set_letter(c, true) {
                    eprintln!("set: -{c}: invalid option");
                    return Flow::Status(2);
                }
            }
            i += 1;
            continue;
        }
        if a.starts_with('+') && a.len() > 1 {
            for c in a[1..].chars() {
                if !env.opts.set_letter(c, false) {
                    eprintln!("set: +{c}: invalid option");
                    return Flow::Status(2);
                }
            }
            i += 1;
            continue;
        }
        assign = true;
        break;
    }
    if assign {
        env.set_positional(args[i..].to_vec());
    }
    Flow::Status(0)
}

fn list_vars(env: &Env) {
    for (idx, p) in env.positional().iter().enumerate() {
        println!("{}\t{}", idx + 1, p);
    }
    let mut names: Vec<&String> = env.vars_iter().map(|(k, _)| k).collect();
    names.sort();
    for n in names {
        println!("{}={}", n, env.get(n).unwrap_or(""));
    }
}

fn list_opts(env: &Env) {
    let names = [
        ("allexport", env.opts.allexport),
        ("noclobber", env.opts.noclobber),
        ("errexit", env.opts.errexit),
        ("noglob", env.opts.noglob),
        ("noexec", env.opts.noexec),
        ("nounset", env.opts.nounset),
        ("verbose", env.opts.verbose),
        ("xtrace", env.opts.xtrace),
        ("monitor", env.opts.monitor),
        ("ignoreeof", env.opts.ignore_eof),
        ("globstar", env.opts.globstar),
        ("pipefail", env.opts.pipefail),
    ];
    for (n, on) in names {
        println!("{n} {}", if on { "on" } else { "off" });
    }
}

// ---- shift ----

fn shift(args: &[String], env: &mut Env) -> Flow {
    let n: i64 = if args.len() > 1 {
        match args[1].parse() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("shift: {}: numeric argument required", args[1]);
                return Flow::Status(2);
            }
        }
    } else {
        1
    };
    let len = env.positional().len() as i64;
    if n < 0 || n > len {
        eprintln!("shift: can't shift {n} times");
        return Flow::Status(1);
    }
    let rest = env.positional()[(n as usize)..].to_vec();
    env.set_positional(rest);
    Flow::Status(0)
}

// ---- read ----

fn read(args: &[String], env: &mut Env) -> Result<Flow, Error> {
    let mut i = 1;
    let mut raw = false;
    while i < args.len() {
        match args[i].as_str() {
            "-r" => {
                raw = true;
                i += 1;
            }
            _ => break,
        }
    }
    let vars: Vec<String> = if i >= args.len() {
        vec!["REPLY".to_string()]
    } else {
        args[i..].to_vec()
    };

    // Read one logical line; backslash-newline continues unless -r.
    let mut line = String::new();
    let stdin = std::io::stdin();
    let mut lock = stdin.lock();
    loop {
        let mut buf = String::new();
        match lock.read_line(&mut buf) {
            Ok(0) => {
                if line.is_empty() {
                    return Ok(Flow::Status(1));
                }
                break;
            }
            Ok(_) => {
                line.push_str(&buf);
                if !raw && line.ends_with("\\\n") {
                    continue;
                }
                break;
            }
            Err(e) => {
                eprintln!("read: {e}");
                return Ok(Flow::Status(1));
            }
        }
    }
    if line.ends_with('\n') {
        line.pop();
        if line.ends_with('\r') {
            line.pop();
        }
    }
    let processed = if raw { line } else { unescape_line(&line) };
    let ifs = env.ifs().to_string();
    assign_read(&vars, &ifs, &processed, env).map_err(|e| {
        eprintln!("read: {e}");
        Error::Exec(e.to_string())
    })?;
    Ok(Flow::Status(0))
}

/// POSIX `read` backslash processing (non `-r`): `\<newline>` removed,
/// `\c` → `c`.
fn unescape_line(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('\n') => {} // line continuation already joined
                Some(c2) => out.push(c2),
                None => {} // trailing lone backslash dropped
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Split `line` per POSIX `read` semantics and assign to `vars`.
/// First vars get leading fields; the last var gets the remainder.
fn assign_read(vars: &[String], ifs: &str, line: &str, env: &mut Env) -> Result<(), Error> {
    let ws: Vec<char> = ifs.chars().filter(|c| " \t\n".contains(*c)).collect();
    let n = vars.len();
    if n == 1 {
        let mut v = line.to_string();
        if !ws.is_empty() {
            let start = v
                .chars()
                .take_while(|c| ws.contains(c))
                .map(char::len_utf8)
                .sum::<usize>();
            let end = v.chars().rev().take_while(|c| ws.contains(c)).count();
            v = v[start..v.len().saturating_sub(end)].to_string();
        }
        env.set(&vars[0], v)?;
        return Ok(());
    }
    let fields = brish_words::fields(line, ifs);
    let sep: String = ifs.chars().next().map(String::from).unwrap_or_default();
    for (k, name) in vars.iter().enumerate() {
        let val = if k + 1 < n {
            fields.get(k).cloned().unwrap_or_default()
        } else if k < fields.len() {
            fields[k..].join(&sep)
        } else {
            String::new()
        };
        env.set(name, val)?;
    }
    Ok(())
}

// ---- test / [ ----

fn test(args: &[String]) -> Flow {
    let mut a: Vec<String> = args[1..].to_vec();
    if args[0] == "[" {
        if a.last().map(String::as_str) != Some("]") {
            eprintln!("[: missing `]'");
            return Flow::Status(2);
        }
        a.pop();
    }
    if a.is_empty() {
        return Flow::Status(1);
    }
    match eval_test(&a) {
        Ok(true) => Flow::Status(0),
        Ok(false) => Flow::Status(1),
        Err(e) => {
            eprintln!("test: {e}");
            Flow::Status(2)
        }
    }
}

// ---- command / type ----

fn command(args: &[String], env: &Env) -> Flow {
    let mut i = 1;
    let mut v = false;
    let mut verbose = false;
    while i < args.len() {
        match args[i].as_str() {
            "-v" => {
                v = true;
                i += 1;
            }
            "-V" => {
                verbose = true;
                i += 1;
            }
            "-p" => i += 1, // default PATH: same search (ponytail: PATH override later)
            _ => break,
        }
    }
    if i >= args.len() {
        eprintln!("command: usage: command [-v|-V] name ...");
        return Flow::Status(2);
    }
    if v || verbose {
        let mut status = 0;
        for name in &args[i..] {
            if BuiltIn::from_name(name).is_some() {
                if verbose {
                    println!("{name} is a shell builtin");
                } else {
                    println!("{name}");
                }
                continue;
            }
            match find_in_path(name, env.get("PATH")) {
                Some(p) => {
                    let p = p.to_string_lossy();
                    if verbose {
                        println!("{name} is {p}");
                    } else {
                        println!("{p}");
                    }
                }
                None => {
                    if verbose {
                        eprintln!("{name} not found");
                    }
                    status = 1;
                }
            }
        }
        Flow::Status(status)
    } else {
        Flow::Exec(args[i..].to_vec())
    }
}

fn r#type(args: &[String], env: &Env) -> Flow {
    if args.len() == 1 {
        eprintln!("type: usage: type name ...");
        return Flow::Status(2);
    }
    let mut status = 0;
    for name in &args[1..] {
        if BuiltIn::from_name(name).is_some() {
            println!("{name} is a shell builtin");
        } else if let Some(p) = find_in_path(name, env.get("PATH")) {
            println!("{name} is {}", p.to_string_lossy());
        } else {
            eprintln!("{name} not found");
            status = 1;
        }
    }
    Flow::Status(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// Run and unwrap (builtins under test never return engine errors).
    fn flow(b: BuiltIn, a: &[String], e: &mut Env) -> Flow {
        run(b, a, e).expect("builtin error")
    }

    fn env() -> Env {
        let mut e = Env::new();
        e.set("HOME", "/home/test").unwrap();
        e
    }

    #[test]
    fn echo_flags() {
        assert_eq!(
            echo_text(&args(&["echo", "a", "b"])),
            ("a b".to_string(), true, false)
        );
        assert_eq!(
            echo_text(&args(&["echo", "-n", "x"])),
            ("x".to_string(), false, false)
        );
        assert_eq!(
            echo_text(&args(&["echo", "-nnn", "x"])),
            ("x".to_string(), false, false)
        );
        assert_eq!(
            echo_text(&args(&["echo", "-n"])),
            (String::new(), false, false)
        );
        // -e/-E select escape interpretation (default off, like bash)
        assert_eq!(
            echo_text(&args(&["echo", "-e", "x"])),
            ("x".to_string(), true, true)
        );
        assert_eq!(
            echo_text(&args(&["echo", "-ne", "x"])),
            ("x".to_string(), false, true)
        );
        assert_eq!(
            echo_text(&args(&["echo", "-e", "-E", "x"])),
            ("x".to_string(), true, false)
        );
        // `-` alone or unknown flags end the flag run (printed literally)
        assert_eq!(
            echo_text(&args(&["echo", "-", "x"])),
            ("- x".to_string(), true, false)
        );
        assert_eq!(
            echo_text(&args(&["echo", "-q", "x"])),
            ("-q x".to_string(), true, false)
        );
    }

    #[test]
    fn exit_flows() {
        let mut e = env();
        e.status = 7;
        assert_eq!(flow(BuiltIn::Exit, &args(&["exit"]), &mut e), Flow::Exit(7));
        assert_eq!(
            flow(BuiltIn::Exit, &args(&["exit", "5"]), &mut e),
            Flow::Exit(5)
        );
        assert_eq!(
            flow(BuiltIn::Exit, &args(&["exit", "-3"]), &mut e),
            Flow::Exit(253)
        );
        assert_eq!(
            flow(BuiltIn::Exit, &args(&["exit", "xyz"]), &mut e),
            Flow::Exit(2)
        );
        assert_eq!(
            flow(BuiltIn::Exit, &args(&["exit", "1", "2"]), &mut e),
            Flow::Status(2)
        );
    }

    #[test]
    fn return_flows() {
        let mut e = env();
        e.status = 3;
        assert_eq!(
            flow(BuiltIn::Return, &args(&["return"]), &mut e),
            Flow::Return(3)
        );
        assert_eq!(
            flow(BuiltIn::Return, &args(&["return", "9"]), &mut e),
            Flow::Return(9)
        );
        assert_eq!(
            flow(BuiltIn::Return, &args(&["return", "x"]), &mut e),
            Flow::Status(2)
        );
    }

    #[test]
    fn export_sets_and_marks() {
        let mut e = env();
        let st = flow(BuiltIn::Export, &args(&["export", "A=1", "B"]), &mut e);
        assert_eq!(st, Flow::Status(0));
        assert_eq!(e.get("A"), Some("1"));
        assert!(e.get_var("A").unwrap().exported);
        assert!(e.get_var("B").unwrap().exported);
        assert_eq!(
            e.child_env(),
            vec![
                ("A".to_string(), "1".to_string()),
                ("B".to_string(), String::new())
            ]
        );
        // bad identifier
        assert_eq!(
            flow(BuiltIn::Export, &args(&["export", "1BAD=x"]), &mut e),
            Flow::Status(1)
        );
    }

    #[test]
    fn readonly_blocks_unset() {
        let mut e = env();
        assert_eq!(
            flow(BuiltIn::Readonly, &args(&["readonly", "R=1"]), &mut e),
            Flow::Status(0)
        );
        assert_eq!(e.get("R"), Some("1"));
        assert_eq!(
            flow(BuiltIn::Unset, &args(&["unset", "R"]), &mut e),
            Flow::Status(1)
        );
        assert_eq!(
            flow(BuiltIn::Unset, &args(&["unset", "FREE", "1BAD"]), &mut e),
            Flow::Status(1)
        );
        assert!(!e.is_set("FREE"));
    }

    #[test]
    fn set_options_and_positionals() {
        let mut e = env();
        assert_eq!(
            flow(BuiltIn::Set, &args(&["set", "-e", "-x"]), &mut e),
            Flow::Status(0)
        );
        assert!(e.opts.errexit && e.opts.xtrace);
        assert_eq!(
            flow(BuiltIn::Set, &args(&["set", "+ex"]), &mut e),
            Flow::Status(0)
        );
        assert!(!e.opts.errexit && !e.opts.xtrace);
        assert_eq!(
            flow(BuiltIn::Set, &args(&["set", "-o", "nounset"]), &mut e),
            Flow::Status(0)
        );
        assert!(e.opts.nounset);
        assert_eq!(
            flow(BuiltIn::Set, &args(&["set", "-q"]), &mut e),
            Flow::Status(2)
        );
        assert_eq!(
            flow(BuiltIn::Set, &args(&["set", "-e", "--", "a", "-b"]), &mut e),
            Flow::Status(0)
        );
        assert!(e.opts.errexit);
        assert_eq!(e.positional(), ["a", "-b"]);
        assert_eq!(
            flow(BuiltIn::Set, &args(&["set", "--"]), &mut e),
            Flow::Status(0)
        );
        assert!(e.positional().is_empty());
        // allexport
        assert_eq!(
            flow(BuiltIn::Set, &args(&["set", "-a"]), &mut e),
            Flow::Status(0)
        );
        e.set("NEWVAR", "v").unwrap();
        assert!(e.get_var("NEWVAR").unwrap().exported);
    }

    #[test]
    fn shift_basics() {
        let mut e = env();
        e.set_positional(vec!["a".into(), "b".into(), "c".into()]);
        assert_eq!(
            flow(BuiltIn::Shift, &args(&["shift"]), &mut e),
            Flow::Status(0)
        );
        assert_eq!(e.positional(), ["b", "c"]);
        assert_eq!(
            flow(BuiltIn::Shift, &args(&["shift", "9"]), &mut e),
            Flow::Status(1)
        );
        assert_eq!(
            flow(BuiltIn::Shift, &args(&["shift", "x"]), &mut e),
            Flow::Status(2)
        );
        assert_eq!(
            flow(BuiltIn::Shift, &args(&["shift", "0"]), &mut e),
            Flow::Status(0)
        );
    }

    #[test]
    fn assign_read_splitting() {
        let mut e = env();
        let vs = |items: &[&str]| args(items);
        let v = vs(&["a", "b"]);
        assign_read(&v, " \t\n", "x y z", &mut e).unwrap();
        assert_eq!(e.get("a"), Some("x"));
        assert_eq!(e.get("b"), Some("y z"));

        let v = vs(&["line"]);
        assign_read(&v, " \t\n", "  spaced  ", &mut e).unwrap();
        assert_eq!(e.get("line"), Some("spaced"));

        let v = vs(&["a", "b"]);
        assign_read(&v, ":", "x:y:z", &mut e).unwrap();
        assert_eq!(e.get("a"), Some("x"));
        assert_eq!(e.get("b"), Some("y:z"));

        let v = vs(&["a"]);
        assign_read(&v, ":", "x:y", &mut e).unwrap();
        assert_eq!(e.get("a"), Some("x:y"));

        let v = vs(&["a", "b", "c"]);
        assign_read(&v, " \t\n", "only", &mut e).unwrap();
        assert_eq!(e.get("a"), Some("only"));
        assert_eq!(e.get("b"), Some(""));
        assert_eq!(e.get("c"), Some(""));
    }

    #[test]
    fn unescape() {
        assert_eq!(unescape_line("a\\nb"), "anb");
        assert_eq!(unescape_line("a\\ b"), "a b");
        assert_eq!(unescape_line("trail\\"), "trail");
        assert_eq!(unescape_line("plain"), "plain");
    }

    #[test]
    fn test_builtin() {
        let mut e = env();
        let mut ok = |items: &[&str]| flow(BuiltIn::Test, &args(items), &mut e);
        assert_eq!(ok(&["test", "1", "-eq", "1"]), Flow::Status(0));
        assert_eq!(ok(&["test", "1", "-eq", "2"]), Flow::Status(1));
        assert_eq!(ok(&["[", "x", "=", "x", "]"]), Flow::Status(0));
        assert_eq!(ok(&["[", "x", "=", "y", "]"]), Flow::Status(1));
        assert_eq!(ok(&["[", "x", "=", "y"]), Flow::Status(2));
        assert_eq!(ok(&["test"]), Flow::Status(1));
        assert_eq!(ok(&["test", "a", "-zzz", "b"]), Flow::Status(2));
    }

    #[test]
    fn command_v() {
        let mut e = env();
        assert_eq!(
            flow(BuiltIn::Command, &args(&["command", "-v", "cd"]), &mut e),
            Flow::Status(0)
        );
        assert_eq!(
            flow(
                BuiltIn::Command,
                &args(&["command", "-v", "definitely-not-real-xyz"]),
                &mut e,
            ),
            Flow::Status(1)
        );
        assert_eq!(
            flow(BuiltIn::Command, &args(&["command", "ls"]), &mut e),
            Flow::Exec(vec!["ls".to_string()])
        );
        assert_eq!(
            flow(BuiltIn::Command, &args(&["command", "-v"]), &mut e),
            Flow::Status(2)
        );
    }

    #[test]
    fn type_reports() {
        let mut e = env();
        assert_eq!(
            flow(BuiltIn::Type, &args(&["type", "pwd", "sh"]), &mut e),
            Flow::Status(0)
        );
        assert_eq!(
            flow(
                BuiltIn::Type,
                &args(&["type", "definitely-not-real-xyz"]),
                &mut e,
            ),
            Flow::Status(1)
        );
    }

    #[test]
    fn cd_relative_and_dash() {
        let _g = crate::test_util::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut e = env();
        let start = std::env::current_dir().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().to_str().unwrap().to_string();
        assert_eq!(
            flow(BuiltIn::Cd, &args(&["cd", &target]), &mut e),
            Flow::Status(0)
        );
        assert_eq!(
            std::env::current_dir().unwrap().canonicalize().unwrap(),
            tmp.path().canonicalize().unwrap()
        );
        assert_eq!(e.get("PWD").map(|p| p.to_string()), Some(target.clone()));
        // go back
        assert_eq!(
            flow(BuiltIn::Cd, &args(&["cd", "-"]), &mut e),
            Flow::Status(0)
        );
        assert_eq!(
            std::env::current_dir().unwrap().canonicalize().unwrap(),
            start.canonicalize().unwrap()
        );
        // failure
        assert_eq!(
            flow(BuiltIn::Cd, &args(&["cd", "/definitely/not/a/dir"]), &mut e,),
            Flow::Status(1)
        );
        let _ = std::env::set_current_dir(start);
    }

    #[test]
    fn cd_home_unset() {
        let _g = crate::test_util::CWD_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let orig = std::env::current_dir().unwrap();
        let mut e = Env::new();
        assert_eq!(flow(BuiltIn::Cd, &args(&["cd"]), &mut e), Flow::Status(1));
        e.set("HOME", "/tmp").unwrap();
        assert_eq!(flow(BuiltIn::Cd, &args(&["cd"]), &mut e), Flow::Status(0));
        assert_eq!(e.get("PWD"), Some("/tmp"));
        let _ = std::env::set_current_dir(orig);
    }

    #[test]
    fn not_yet_implemented() {
        let mut e = env();
        for b in [BuiltIn::Alias, BuiltIn::Source, BuiltIn::Trap] {
            assert!(run(b, &[b.name().to_string()], &mut e).is_err());
        }
    }
}
