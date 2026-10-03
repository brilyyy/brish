//! Batch-mode integration tests: run the real binary, assert exit
//! codes and actual fd 1/2 output (libtest's capture can't reach these).

// Test crate: helpers panic loudly on spawn failures.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
use std::io::Write;
use std::process::{Command, Output, Stdio};

/// Isolated `$HOME` so the real `~/.config/brish` (config/rc/history)
/// can never leak into test output.
fn test_home() -> &'static std::path::Path {
    static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    HOME.get_or_init(|| tempfile::tempdir().expect("test home"))
        .path()
}

fn brish() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_brish"));
    cmd.env("HOME", test_home());
    cmd
}

fn run(args: &[&str]) -> Output {
    brish().args(args).output().expect("spawn brish")
}

fn run_stdin(args: &[&str], input: &str) -> Output {
    let mut child = brish()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn brish");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait")
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn code(o: &Output) -> i32 {
    o.status.code().expect("exit status")
}

#[test]
fn echo_writes_stdout_and_zero() {
    let o = run(&["-c", "echo hello world"]);
    assert_eq!(out(&o), "hello world\n");
    assert_eq!(code(&o), 0);
}

#[test]
fn exit_codes_match_bash() {
    assert_eq!(code(&run(&["-c", "true"])), 0);
    assert_eq!(code(&run(&["-c", "false"])), 1);
    // syntax errors: status 2
    assert_eq!(code(&run(&["-c", "echo ("])), 2);
    // `exit N` propagates
    assert_eq!(code(&run(&["-c", "exit 7"])), 7);
    // command not found: 127 + message on stderr
    let o = run(&["-c", "no_such_command_xyzzy"]);
    assert_eq!(code(&o), 127);
    assert!(err(&o).contains("command not found"));
}

#[test]
fn pipeline_carries_builtin_stdout() {
    let o = run(&["-c", "echo a b | tr a-z A-Z"]);
    assert_eq!(out(&o), "A B\n");
    assert_eq!(code(&o), 0);
    let o = run(&["-c", "echo one | cat | cat | wc -c"]);
    assert_eq!(out(&o).trim(), "4");
    // last stage decides the status
    assert_eq!(code(&run(&["-c", "true | false"])), 1);
    assert_eq!(code(&run(&["-c", "false | true"])), 0);
}

#[test]
fn builtin_output_reaches_redirected_file() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("out.txt");
    let o = run(&["-c", &format!("echo to-file > {}", f.display())]);
    assert_eq!(code(&o), 0);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "to-file\n");
    // append
    let o = run(&["-c", &format!("echo more >> {}", f.display())]);
    assert_eq!(code(&o), 0);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "to-file\nmore\n");
    // noclobber refuses, `>|` forces
    let src = format!("set -o noclobber; echo nope > {}; true", f.display());
    assert_eq!(code(&run(&["-c", &src])), 0);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "to-file\nmore\n");
    let src = format!("set -o noclobber; echo forced >| {}; true", f.display());
    assert_eq!(code(&run(&["-c", &src])), 0);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "forced\n");
}

#[test]
fn builtin_stdout_stderr_stay_separate() {
    let o = run(&["-c", "echo out; echo err >&2"]);
    assert_eq!(out(&o), "out\n");
    assert_eq!(err(&o), "err\n");
    assert_eq!(code(&o), 0);
}

#[test]
fn command_substitution_sees_builtin_output() {
    let o = run(&["-c", "echo [$(echo hi)]"]);
    assert_eq!(out(&o), "[hi]\n");
    let o = run(&["-c", "x=$(printf 'a b'); echo $x"]);
    assert_eq!(out(&o), "a b\n");
    // substitution status reaches $? (assignment-only keeps it)
    let o = run(&["-c", "x=$(false); echo $?"]);
    assert_eq!(out(&o), "1\n");
}

#[test]
fn heredoc_feeds_stdin() {
    let o = run_stdin(&["-c", "cat <<EOF\nline-one\nEOF\n"], "");
    assert_eq!(out(&o), "line-one\n");
    assert_eq!(code(&o), 0);
}

#[test]
fn reads_program_from_stdin() {
    let o = run_stdin(&[], "echo one\necho two\n");
    assert_eq!(out(&o), "one\ntwo\n");
    assert_eq!(code(&o), 0);
}

#[test]
fn runs_script_file_with_args() {
    let d = tempfile::tempdir().unwrap();
    let s = d.path().join("s.sh");
    std::fs::write(&s, "echo \"$0\" \"$1\"\n").unwrap();
    let o = run(&[s.to_str().unwrap(), "arg1"]);
    assert_eq!(out(&o), format!("{} arg1\n", s.display()));
    assert_eq!(code(&o), 0);
    // missing script: status 1
    assert_eq!(code(&run(&["/no/such/script.sh"])), 1);
}

#[test]
fn control_flow_batch() {
    let o = run(&[
        "-c",
        "i=0; while [ $i -lt 3 ]; do i=$((i+1)); done; echo $i",
    ]);
    assert_eq!(out(&o), "3\n");
    let o = run(&["-c", "for x in a b c; do printf '%s-' $x; done; echo"]);
    assert_eq!(out(&o), "a-b-c-\n");
    let o = run(&["-c", "if false; then echo no; else echo yes; fi"]);
    assert_eq!(out(&o), "yes\n");
    let o = run(&["-c", "f() { echo fn:$1; }; f arg"]);
    assert_eq!(out(&o), "fn:arg\n");
    let o = run(&["-c", "case foo in f*) echo matched;; *) echo no;; esac"]);
    assert_eq!(out(&o), "matched\n");
}

#[test]
fn break_and_continue_are_zero_status() {
    let o = run(&["-c", "while true; do break; done; echo $?"]);
    assert_eq!(out(&o), "0\n");
    let o = run(&["-c", "for x in 1 2; do continue; echo never; done; echo $?"]);
    assert_eq!(out(&o), "0\n");
}

#[test]
fn assignments_and_expansions() {
    let o = run(&["-c", "x=42; echo $x"]);
    assert_eq!(out(&o), "42\n");
    let o = run(&["-c", "set -- a b c; echo $# $2"]);
    assert_eq!(out(&o), "3 b\n");
    let o = run(&["-c", "echo ${HOME:+home-set} ${NOPE:-fallback}"]);
    assert_eq!(out(&o), "home-set fallback\n");
    let o = run(&["-c", "echo $((2 + 3 * 4))"]);
    assert_eq!(out(&o), "14\n");
    // unset with nounset is fatal (status 1)
    assert_eq!(code(&run(&["-c", "set -u; echo $NOPE_XYZ"])), 1);
}

#[test]
fn subshell_isolates_but_group_leaks() {
    let o = run(&["-c", "( x=1 ); echo ${x:-unset}; { y=1; }; echo $y"]);
    assert_eq!(out(&o), "unset\n1\n");
}

#[test]
fn redirects_do_not_leak_fds() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().display().to_string();
    // 30 redirection rounds; the fd-table size must not grow.
    let src = format!(
        "before=$(ls /dev/fd | wc -l)\n         i=0\n         while [ $i -lt 30 ]; do echo pad > {dir}/f; cat < {dir}/f > /dev/null; i=$((i+1)); done\n         after=$(ls /dev/fd | wc -l)\n         echo $before $after\n"
    );
    let o = run(&["-c", &src]);
    assert_eq!(code(&o), 0, "stderr: {}", err(&o));
    let text = out(&o);
    let mut ns = text.split_whitespace();
    let before: i64 = ns.next().expect("before").parse().expect("num");
    let after: i64 = ns.next().expect("after").parse().expect("num");
    assert_eq!(before, after, "fd count grew: {before} -> {after}");
}

#[test]
fn sigpipe_at_default_kills_producers() {
    // `yes` must die of SIGPIPE when `head` closes the pipe (plan 4.9);
    // Rust's std starts with SIG_IGN, which would leak into children.
    let o = run(&["-c", "yes | head -2"]);
    assert_eq!(out(&o), "y\ny\n");
    assert_eq!(code(&o), 0);
}

#[test]
fn sigint_terminates_script() {
    let o = run(&["-c", "kill -INT $$"]);
    assert!(!o.status.success(), "SIGINT should not report success");
}

#[test]
fn wait_waits_for_background_status() {
    let o = run(&["-c", "sleep 0.1 & wait $!; echo $?"]);
    assert_eq!(out(&o), "0\n");
}

#[test]
fn jobs_lists_background() {
    let o = run(&["-c", "sleep 0.2 & jobs"]);
    assert!(out(&o).contains("[1]"), "got: {:?}", out(&o));
    assert!(out(&o).contains("sleep 0.2"), "got: {:?}", out(&o));
}

#[test]
fn kill_and_wait_reports_signal_status() {
    let o = run(&["-c", "sleep 5 & kill %1; wait %1; echo $?"]);
    assert_eq!(out(&o), "143\n");
}

#[test]
fn wait_unknown_pid_is_127() {
    let o = run(&["-c", "wait 99999999; echo $?"]);
    assert_eq!(out(&o), "127\n");
}

#[test]
fn background_in_its_own_process_group() {
    // Plan 4.10: background jobs get their own pgid so fg signals
    // (Ctrl-C) cannot hit them. Compare pgids via ps.
    let o = run(&[
        "-c",
        "sleep 0.3 & p=$!; ps -o pid=,pgid= -p $p; kill %1; wait %1",
    ]);
    let line = out(&o);
    let nums: Vec<i64> = line
        .split_whitespace()
        .filter_map(|t| t.parse::<i64>().ok())
        .collect();
    assert_eq!(nums.len(), 2, "ps output: {line:?}");
    // Own group leader: pgid == pid (inheriting the shell's group would
    // make pgid the *shell's* pid, which differs from the child's).
    assert_eq!(nums[0], nums[1], "bg job must lead its own group: {line:?}");
}
