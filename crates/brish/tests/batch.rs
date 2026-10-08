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

#[test]
fn fg_waits_for_background_job() {
    let o = run(&["-c", "sleep 0.1 & fg; echo $?"]);
    assert_eq!(out(&o), "sleep 0.1\n0\n", "stderr: {}", err(&o));
}

#[test]
fn stopped_background_job_recovers_via_fg() {
    let o = run(&[
        "-c",
        "sleep 0.3 & kill -STOP $!; sleep 0.05; jobs; fg; echo $?",
    ]);
    let text = out(&o);
    assert!(text.contains("Stopped"), "jobs must show Stopped: {text:?}");
    assert!(text.trim().ends_with('0'), "fg resumed job to 0: {text:?}");
}

#[test]
fn arith_command_sets_status() {
    let o = run(&["-c", "((0)); echo $((x)) $?"]);
    assert_eq!(out(&o), "0 1\n");
    let o = run(&["-c", "((1)); echo $?"]);
    assert_eq!(out(&o), "0\n");
    // nonzero value -> status 0; $((x > 0)) evaluates to 1
    let o = run(&["-c", "((x = 41 + 1)); echo $x; ((x)); echo $((x > 0)) $?"]);
    assert_eq!(out(&o), "42\n1 0\n");
    // arithmetic error: status 1, diagnostic, shell keeps going
    let o = run(&["-c", "((1/0)); echo $?"]);
    assert_eq!(out(&o), "1\n");
    assert!(err(&o).contains("division by zero"), "err={}", err(&o));
}

#[test]
fn arith_command_in_pipeline_and_negation() {
    let o = run(&["-c", "((2)) | cat; echo $((x = 7)) | cat; ! ((0)); echo $?"]);
    assert_eq!(out(&o), "7\n0\n");
}

#[test]
fn ansi_c_quoting() {
    let o = run(&["-c", "printf '<%s>' $'a\\tb'; echo"]);
    assert_eq!(out(&o), "<a\tb>\n");
    let o = run(&["-c", "printf '<%s>' $'x\\ny'; echo"]);
    assert_eq!(out(&o), "<x\ny>\n");
    let o = run(&["-c", "printf '<%s>' $'A\\x41\\u0042'; echo"]);
    assert_eq!(out(&o), "<AAB>\n");
    // unquoted result is still one field (literal, like single quotes)
    let o = run(&["-c", "printf '<%s>' $'a b'; echo"]);
    assert_eq!(out(&o), "<a b>\n");
}

#[test]
fn here_string_feeds_stdin() {
    let o = run(&["-c", "cat <<< hello"]);
    assert_eq!(out(&o), "hello\n");
    let o = run(&["-c", "cat <<< $'a\\nb'"]);
    assert_eq!(out(&o), "a\nb\n");
    let o = run(&[
        "-c",
        "while read -r line; do echo \"[$line]\"; done <<< 'x y'",
    ]);
    assert_eq!(out(&o), "[x y]\n");
    let o = run(&["-c", "cat 3<<< x 2>&1 3>&-"]);
    assert_eq!(out(&o), "");
}

#[test]
fn brace_expansion() {
    let o = run(&["-c", "echo {a,b} x{1..3}y"]);
    assert_eq!(out(&o), "a b x1y x2y x3y\n");
    let o = run(&["-c", "echo {01..03} {a..c} {5..1}"]);
    assert_eq!(out(&o), "01 02 03 a b c 5 4 3 2 1\n");
    let o = run(&["-c", "echo {a,{b,c}}"]);
    assert_eq!(out(&o), "a b c\n");
    // literal: no separator, quoted, escaped braces
    let o = run(&["-c", "echo {foo} \"{a,b}\" a\\{b,c\\}"]);
    assert_eq!(out(&o), "{foo} {a,b} a{b,c}\n");
    // empty variant is one empty field
    let o = run(&["-c", "printf '<%s>' {,x}; echo"]);
    assert_eq!(out(&o), "<><x>\n");
    // for-loop word list and assignment/here-string values
    let o = run(&["-c", "for i in {1..3}; do printf %s \"$i\"; done; echo"]);
    assert_eq!(out(&o), "123\n");
    let o = run(&["-c", "v={a,b}; echo \"$v\"; cat <<< {1..2}"]);
    assert_eq!(out(&o), "a b\n1 2\n");
}

#[test]
fn globstar_option() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::create_dir_all(root.join("d/e")).expect("mkdir");
    for f in ["a.txt", "d/a.txt", "d/e/a.txt", "d/e/b.log"] {
        std::fs::write(root.join(f), "").expect("write");
    }
    let on = format!(
        "cd {}; set -o globstar; printf '%s\\n' **/*.txt",
        root.display()
    );
    let o = run(&["-c", &on]);
    assert_eq!(out(&o), "a.txt\nd/a.txt\nd/e/a.txt\n");
    let on = format!("cd {}; set -o globstar; printf '%s\\n' **", root.display());
    let all = out(&run(&["-c", &on]));
    let mut lines: Vec<&str> = all.split('\n').collect();
    lines.pop(); // trailing empty
    lines.sort();
    assert_eq!(
        lines,
        ["a.txt", "d", "d/a.txt", "d/e", "d/e/a.txt", "d/e/b.log"]
    );
    // globstar off: `**` degrades to a single `*`
    let off = format!("cd {}; printf '%s\\n' **/*.txt", root.display());
    assert_eq!(out(&run(&["-c", &off])), "d/a.txt\n");
}

#[test]
fn pipefail_option() {
    let o = run(&["-c", "false | true; echo $?"]);
    assert_eq!(out(&o), "0\n");
    let o = run(&["-c", "set -o pipefail; false | true; echo $?"]);
    assert_eq!(out(&o), "1\n");
    let o = run(&["-c", "set -o pipefail; true | false | true; echo $?"]);
    assert_eq!(out(&o), "1\n");
    let o = run(&["-c", "set -o pipefail; true | true; echo $?"]);
    assert_eq!(out(&o), "0\n");
    let o = run(&[
        "-c",
        "set -o pipefail; set +o pipefail; false | true; echo $?",
    ]);
    assert_eq!(out(&o), "0\n");
}

#[test]
fn echo_escape_and_newline_flags() {
    let o = run(&["-c", "echo -e 'a\\tb\\nc'"]);
    assert_eq!(out(&o), "a\tb\nc\n");
    // backslash stays literal without -e
    let o = run(&["-c", "echo 'a\\tb'"]);
    assert_eq!(out(&o), "a\\tb\n");
    // -E forces escapes off even after -e
    let o = run(&["-c", "echo -e -E 'a\\tb'"]);
    assert_eq!(out(&o), "a\\tb\n");
    // -n suppresses the newline (bare "abc\n" proves it: no -n gives
    // "abc\n\n")
    let o = run(&["-c", "echo -n abc; echo"]);
    assert_eq!(out(&o), "abc\n");
    // \c truncates the rest of the line (including the newline)
    let o = run(&["-c", "echo -e 'x\\cy'"]);
    assert_eq!(out(&o), "x");
    // unknown escapes keep the backslash
    let o = run(&["-c", "echo -e 'a\\qb'"]);
    assert_eq!(out(&o), "a\\qb\n");
}

#[test]
fn prefix_assignments_reach_child() {
    // POSIX: assignments before a command always hit the child env,
    // even when the shell var itself is not exported.
    let o = run(&["-c", "FOO=abc printenv FOO"]);
    assert_eq!(out(&o), "abc\n");
    // restored after the command
    let o = run(&["-c", "FOO=abc printenv FOO >/dev/null; printenv FOO"]);
    assert_eq!(out(&o), "");
    // previously-exported var keeps its export flag and old value
    let o = run(&["-c", "FOO=old; export FOO; FOO=tmp true; printenv FOO"]);
    assert_eq!(out(&o), "old\n");
    // plain assignment stays unexported
    let o = run(&["-c", "BAR=1; printenv BAR"]);
    assert_eq!(out(&o), "");
}

#[test]
fn tilde_and_assign_expansion() {
    // value-only assignment form: tilde at value start (bash)
    let o = run(&["-c", "v=~; printf '%s' \"$v\""]);
    assert_eq!(out(&o), test_home().display().to_string());
    // `~user` resolves via the platform user database
    let o = run(&["-c", "v=~root; printf '%s' \"$v\""]);
    assert!(out(&o).starts_with('/') && !out(&o).contains('~'));
}

/// Private `$HOME` with a `config.toml` body, so config-driven
/// behaviour can be tested without touching the shared test home.
fn run_with_config(args: &[&str], config: &str) -> Output {
    let home = tempfile::tempdir().expect("home");
    let dir = home.path().join(".config/brish");
    std::fs::create_dir_all(&dir).expect("config dir");
    std::fs::write(dir.join("config.toml"), config).expect("config.toml");
    Command::new(env!("CARGO_BIN_EXE_brish"))
        .args(args)
        .env("HOME", home.path())
        .output()
        .expect("spawn brish")
}

const BAD_FOR: &str = "echo hi\nfor x in | ; do echo $x; done";

#[test]
fn batch_syntax_error_is_one_line_without_a_caret() {
    // Batch (non-tty stderr) keeps the old single-line text: scripts and
    // the dash cross-check in posix.rs must not see extra output.
    let o = run(&["-c", BAD_FOR]);
    assert_eq!(code(&o), 2);
    assert_eq!(
        err(&o),
        "brish: parse error: unexpected token in for word list\n"
    );
}

#[test]
fn errors_style_fancy_adds_excerpt_and_caret() {
    let o = run_with_config(&["-c", BAD_FOR], "[errors]\nstyle = \"fancy\"\n");
    assert_eq!(code(&o), 2);
    let e = err(&o);
    assert!(
        e.starts_with("brish: parse error: unexpected token in for word list\n"),
        "{e}"
    );
    assert!(e.contains("2 | for x in | ; do echo $x; done"), "{e}");
    assert!(e.contains("(line 2, col 10)"), "{e}");
}

#[test]
fn errors_style_plain_drops_the_program_prefix() {
    let o = run_with_config(&["-c", BAD_FOR], "[errors]\nstyle = \"plain\"\n");
    let e = err(&o);
    assert_eq!(e, "parse error: unexpected token in for word list\n");
}

#[test]
fn not_found_config_never_touches_batch_output() {
    // Batch keeps the POSIX line even when config asks for fancy:
    // scripts, pipes and the dash cross-check must not see extras.
    let o = run_with_config(
        &["-c", "no_such_command_xyzzy"],
        "[not_found]\nstyle = \"fancy\"\nsuggest = false\n",
    );
    assert_eq!(code(&o), 127);
    assert_eq!(err(&o), "brish: no_such_command_xyzzy: command not found\n");
}

#[test]
fn relconf_e_runs_the_editor_then_reloads() {
    // `EDITOR` is set per-spawn, so no env mutation in-process.
    let home = tempfile::tempdir().expect("home");
    let cfg_dir = home.path().join(".config/brish");
    std::fs::create_dir_all(&cfg_dir).expect("config dir");
    let cfg = cfg_dir.join("config.toml");
    std::fs::write(&cfg, "[theme]\nname = \"briiish-plain\"\n").expect("config");

    // Editor succeeds → config is (re)loaded, status 0.
    let o = Command::new(env!("CARGO_BIN_EXE_brish"))
        .args(["-c", "relconf -e"])
        .env("HOME", home.path())
        .env_remove("VISUAL")
        .env("EDITOR", "true")
        .output()
        .expect("spawn brish");
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );

    // Editor fails → non-zero, shell survives to the next command.
    let o = Command::new(env!("CARGO_BIN_EXE_brish"))
        .args(["-c", "relconf -e; echo after"])
        .env("HOME", home.path())
        .env_remove("VISUAL")
        .env("EDITOR", "false")
        .output()
        .expect("spawn brish");
    assert!(String::from_utf8_lossy(&o.stderr).contains("relconf -e:"));
    assert_eq!(out(&o), "after\n");

    // No editor configured → clear message, no crash.
    let o = Command::new(env!("CARGO_BIN_EXE_brish"))
        .args(["-c", "relconf -e"])
        .env("HOME", home.path())
        .env_remove("VISUAL")
        .env_remove("EDITOR")
        .output()
        .expect("spawn brish");
    assert!(String::from_utf8_lossy(&o.stderr).contains("$VISUAL or $EDITOR"));
}
