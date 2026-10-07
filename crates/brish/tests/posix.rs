//! POSIX.1 subset conformance (plan 6.1): a curated table of
//! non-interactive scripts with expected stdout + exit status. When a
//! `dash` binary is on PATH (plan's reference shell, `--posix` mode),
//! every case is cross-checked against it too — so a wrong expectation
//! cannot slip through as a "pass".

use std::path::Path;
use std::process::Command;

/// (source, expected stdout, expected exit status).
/// `{dir}` in the expectation is replaced with the scratch cwd.
const CASES: &[(&str, &str, i32)] = &[
    ("echo hello", "hello\n", 0),
    ("echo 'a  b'", "a  b\n", 0),
    ("echo \"a  b\"", "a  b\n", 0),
    ("echo a\\ b", "a b\n", 0),
    ("printf '%s-%s\\n' one two", "one-two\n", 0),
    ("printf '%d\\n' 42", "42\n", 0),
    ("x=1; echo $x", "1\n", 0),
    ("echo ${x:-default}", "default\n", 0),
    ("echo \"${x:-a b}\"", "a b\n", 0),
    ("x=hello; y=${x}world; echo $y", "helloworld\n", 0),
    ("x='a  b'; set -- $x; echo $#", "2\n", 0),
    ("IFS=:; x=a:b; set -- $x; echo $#", "2\n", 0),
    ("IFS=:; x=a:b; set -- $x; echo $1", "a\n", 0),
    ("set -- a b; for i in \"$@\"; do echo $i; done", "a\nb\n", 0),
    ("echo $#", "0\n", 0),
    ("echo $((1+2*3))", "7\n", 0),
    ("echo $((10/3))", "3\n", 0),
    ("echo $((10%3))", "1\n", 0),
    ("echo $((010+2))", "10\n", 0),
    ("false; echo $?", "1\n", 0),
    ("true; echo $?", "0\n", 0),
    ("false || echo yes", "yes\n", 0),
    ("false && echo no; echo after", "after\n", 0),
    ("true && echo yes", "yes\n", 0),
    ("echo a | cat", "a\n", 0),
    ("echo a | cat | cat", "a\n", 0),
    ("false | true; echo $?", "0\n", 0),
    ("true | false; echo $?", "1\n", 0),
    ("echo abc | cut -b2", "b\n", 0),
    ("if true; then echo yes; fi", "yes\n", 0),
    ("if false; then echo no; else echo other; fi", "other\n", 0),
    (
        "i=0; while [ $i -lt 3 ]; do i=$((i+1)); done; echo $i",
        "3\n",
        0,
    ),
    (
        "i=0; until [ $i -ge 3 ]; do i=$((i+1)); done; echo $i",
        "3\n",
        0,
    ),
    ("for i in a b c; do printf '%s' $i; done; echo", "abc\n", 0),
    (
        "case abc in abc) echo match;; *) echo no;; esac",
        "match\n",
        0,
    ),
    ("x=ab; case $x in a*) echo star;; esac", "star\n", 0),
    ("f() { echo in-f; }; f", "in-f\n", 0),
    ("f() { echo \"$1\"; }; f arg", "arg\n", 0),
    ("(x=1); echo \"[$x]\"", "[]\n", 0),
    ("{ x=1; }; echo $x", "1\n", 0),
    (
        "f=/tmp/bsh_posix_$$.txt; echo hi > $f; cat $f; rm $f",
        "hi\n",
        0,
    ),
    (
        "f=/tmp/bsh_posix_$$.txt; echo a > $f; echo b >> $f; cat $f; rm $f",
        "a\nb\n",
        0,
    ),
    ("cat <<EOF\nhere\nEOF", "here\n", 0),
    ("[ 1 -lt 2 ] && echo lt", "lt\n", 0),
    ("[ abc = abc ] && echo eq", "eq\n", 0),
    ("test -z \"\" && echo empty", "empty\n", 0),
    ("! false", "", 0),
    ("! true", "", 1),
    ("exit 3", "", 3),
    ("false; exit", "", 1),
    ("x=1; export x; sh -c 'echo $x'", "1\n", 0),
    (": ; echo ok", "ok\n", 0),
    (
        "mkdir g_$$; cd g_$$; touch a b c; echo *; cd ..; rm -rf g_$$",
        "a b c\n",
        0,
    ),
    ("x=1; { x=2; }; echo $x", "2\n", 0),
    (
        "i=0; while [ $i -lt 2 ]; do i=$((i+1)); echo $i; done",
        "1\n2\n",
        0,
    ),
    ("command cat <<EOF\nvia-command\nEOF", "via-command\n", 0),
    // --- parameter expansion (plan 3.2) ---
    ("x=abc; echo ${#x}", "3\n", 0),
    ("x=abcdef; echo ${x#abc}", "def\n", 0),
    ("x=abcdef; echo ${x##a*c}", "def\n", 0),
    ("x=abcabc; echo ${x%abc}", "abc\n", 0),
    ("x=abcabc; echo ${x%%a*c}", "\n", 0),
    ("x=val; echo ${x:+set}", "set\n", 0),
    ("x=; echo ${x:+set}", "\n", 0),
    ("x=base; echo ${x-base}", "base\n", 0),
    ("x=file.txt; echo ${x%.txt}", "file\n", 0),
    ("x=dir/file; echo ${x##*/}", "file\n", 0),
    ("x=a; y=${x:?missing}; echo $y", "a\n", 0),
    (
        "unset x; echo ${x=assigned}; echo $x",
        "assigned\nassigned\n",
        0,
    ),
    // positional after shift
    ("set -- a b c; shift; echo $1 $2", "b c\n", 0),
    ("set -- a b c; shift 2; echo $# $1", "1 c\n", 0),
    ("set -- a b; echo ${#1} ${#2}", "1 1\n", 0),
    // --- arithmetic extras ---
    ("echo $((2 + 3 * 4))", "14\n", 0),
    ("echo $((10 / 3 * 3))", "9\n", 0),
    ("echo $((1 << 4))", "16\n", 0),
    ("echo $((7 & 3)) $((7 | 1)) $((7 ^ 3))", "3 7 4\n", 0),
    ("echo $((-5 + 3))", "-2\n", 0),
    ("echo $((1 == 1)) $((2 < 1))", "1 0\n", 0),
    // --- quoting / expansion order ---
    ("echo \"$(echo nested)\"", "nested\n", 0),
    ("echo \"$(echo a)$(echo b)\"", "ab\n", 0),
    ("echo `echo tick`", "tick\n", 0),
    ("x='  spaced  '; echo [$x]", "[ spaced ]\n", 0),
    // `echo` backslash handling is undefined (dash XSI echo vs bash/brish
    // plain) — use printf for escape cases.
    ("printf '%s\n' 'back\\slash'", "back\\slash\n", 0),
    ("printf '%b\n' 'tab\\there'", "tab\there\n", 0),
    // --- redirections ---
    ("echo out 2>&1", "out\n", 0),
    ("sh -c 'echo err 1>&2' 2>/dev/null", "", 0),
    ("sh -c 'echo e 1>&2' 2>&1 | cat", "e\n", 0),
    ("cat < /dev/null; echo empty-ok", "empty-ok\n", 0),
    (
        "printf 'x\n' >| /tmp/bsh_force_$$.txt; cat /tmp/bsh_force_$$.txt; rm /tmp/bsh_force_$$.txt",
        "x\n",
        0,
    ),
    ("cat <<-EOT\n\tstripped\nEOT", "stripped\n", 0),
    ("cat <<'E'\n$literal\nE", "$literal\n", 0),
    ("X=val; cat <<EOT\n$X-set\nEOT", "val-set\n", 0),
    // --- subshells / functions ---
    ("f() { return 4; }; f; echo $?", "4\n", 0),
    ("f() { echo before; return; echo after; }; f", "before\n", 0),
    ("(exit 2); echo $?", "2\n", 0),
    ("(echo sub; exit 0) | cat", "sub\n", 0),
    ("f() { g; }; g() { echo gg; }; f", "gg\n", 0),
    // --- test/[ operators ---
    ("[ -n abc ] && echo n", "n\n", 0),
    ("[ 5 -ge 5 ] && echo ge", "ge\n", 0),
    ("[ 2 -ne 2 ]; echo $?", "1\n", 0),
    ("[ a != b ] && echo ne", "ne\n", 0),
    ("[ -z \"$unsetvar\" ] && echo z", "z\n", 0),
    ("test 1 -eq 1 -a 2 -eq 2 && echo conj", "conj\n", 0),
    // --- pipelines & lists ---
    ("echo one two | wc -w", "       2\n", 0),
    ("false || false || echo third", "third\n", 0),
    ("true && true && echo chain", "chain\n", 0),
    (
        "ls /definitely/not/here 2>/dev/null; echo survived",
        "survived\n",
        0,
    ),
    ("echo a; echo b; echo c", "a\nb\nc\n", 0),
    // --- set / unset / export ---
    ("set -e; true; echo errexit-ok", "errexit-ok\n", 0),
    ("x=1; unset x; echo [${x-unset}]", "[unset]\n", 0),
    ("set -- ; echo $#", "0\n", 0),
    // readonly reassign is fatal in dash programs per POSIX; skip it.
    ("readonly r=1; echo $r", "1\n", 0),
    // --- command substitution edge ---
    ("out=$(echo inner); echo $out", "inner\n", 0),
    ("echo $(printf '%s-%s' a b)", "a-b\n", 0),
    ("n=$(echo 1; echo 2 | tail -1); echo $n", "1 2\n", 0),
    // --- trap EXIT (dash supports) ---
    ("trap 'echo bye' EXIT; echo hi", "hi\nbye\n", 0),
    // --- getopts (dash builtin) ---
    (
        "OPTIND=1; while getopts ab opt; do case $opt in a) echo A;; b) echo B;; esac; done; echo done",
        "done\n",
        0,
    ),
    (
        "OPTIND=1; while getopts ab opt; do case $opt in a) echo A;; b) echo B;; esac; done; echo left=$OPTIND",
        "left=1\n",
        0,
    ),
    // --- alias NOT expanded in batch (POSIX default) ---
    // not-found diagnostics go to stderr; stdout stays empty.
    ("alias e=echo; e hi", "", 127),
    // --- globbing ---
    ("echo /no/such/glob_*.txt", "/no/such/glob_*.txt\n", 0),
    // --- read builtin ---
    (
        "echo line1 | while read x; do echo got:$x; done",
        "got:line1\n",
        0,
    ),
    // --- here-string is bash-only: skip ---
    // --- nested quotes / words ---
    ("echo \"'single' in double\"", "'single' in double\n", 0),
    ("echo 'don'\\''t' ", "don't\n", 0),
    ("printf '%5s|' ab; echo", "   ab|\n", 0),
    ("printf '%-5s|' ab; echo", "ab   |\n", 0),
    ("printf '%.3s' abcdef", "abc", 0),
    ("printf '%x %X %o' 255 255 8", "ff FF 10", 0),
    // Format not reused once args run out; no trailing newline.
    ("printf '%s %s' only-one", "only-one ", 0),
    ("printf 'no-args %d\n'", "no-args 0\n", 0),
    // --- test/-t: the operand is a descriptor, not a path ---
    // The harness runs with pipes, so stdin is never a tty here.
    ("test -t 0; echo $?", "1\n", 0),
    ("[ -t 0 ] && echo tty || echo no", "no\n", 0),
    // A closed descriptor is false, not an error.
    ("test -t 99; echo $?", "1\n", 0),
    // Non-numeric operand is an error (dash: "Illegal number"): `test`
    // exits 2, then `echo $?` exits 0 — so the LIST status is 0.
    ("test -t x 2>/dev/null; echo $?", "2\n", 0),
    // POSIX 2.6.1: one argument → true iff non-empty, even when it
    // looks like an operator. dash and bash both exit 0.
    ("test -t; echo $?", "0\n", 0),
    ("test -z; echo $?", "0\n", 0),
    ("test -e; echo $?", "0\n", 0),
    ("test !; echo $?", "0\n", 0),
    ("test -eq; echo $?", "0\n", 0),
    ("test \"\"; echo $?", "1\n", 0),
    // --- test/-r/-w/-x: access(2), not raw mode bits ---
    // Each case must be re-runnable: the harness runs every case twice in
    // the same {dir} (briSH, then dash), so an unreadable file left behind
    // by run #1 would break `: >` in run #2. Restore the mode first.
    (
        "d={dir}/acc; mkdir -p $d; chmod 700 $d 2>/dev/null; rm -f $d/f; : > $d/f; chmod 400 $d/f; test -r $d/f && echo r; chmod 700 $d 2>/dev/null",
        "r\n",
        0,
    ),
    (
        "d={dir}/acc; mkdir -p $d; chmod 700 $d 2>/dev/null; rm -f $d/f; : > $d/f; chmod 600 $d/f; test -x $d/f || echo nox; chmod 700 $d 2>/dev/null",
        "nox\n",
        0,
    ),
    (
        "d={dir}/acc; mkdir -p $d; chmod 700 $d 2>/dev/null; rm -f $d/f; : > $d/f; chmod 644 $d/f; test -w $d/f && echo w; chmod 700 $d 2>/dev/null",
        "w\n",
        0,
    ),
    (
        "d={dir}/acc; mkdir -p $d; chmod 700 $d 2>/dev/null; rm -f $d/f; : > $d/f; chmod 000 $d/f; test -r $d/f || echo not-readable; chmod 700 $d 2>/dev/null",
        "not-readable\n",
        0,
    ),
    (
        "test -r /no/such/file_xyz && echo yes || echo no",
        "no\n",
        0,
    ),
    // --- && / || equal precedence, left-to-right (POSIX 2.9.4) ---
    // A short-circuit skips only the NEXT command, never the rest of the
    // list. These were silently broken: the engine returned on the first
    // skip, so `false && a || b` never reached `b`.
    ("false && echo a || echo b", "b\n", 0),
    ("true || echo a && echo b", "b\n", 0),
    ("true && echo a || echo b", "a\n", 0),
    ("false || echo a && echo b", "a\nb\n", 0),
    ("true && false || echo c", "c\n", 0),
    ("false && echo a || echo b || echo c", "b\n", 0),
    ("true && echo a && echo b", "a\nb\n", 0),
    // short-circuit still applies within one operator
    ("false && echo a && echo b; echo $?", "1\n", 0),
    // errexit must stay suspended inside the list
    (
        "set -e; false && echo a || echo b; echo survived",
        "b\nsurvived\n",
        0,
    ),
];

fn run(prog: &str, args: &[&str], cwd: &Path) -> (String, i32) {
    // Isolated $HOME: the real ~/.config/brish must not affect runs.
    static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    let home =
        HOME.get_or_init(|| tempfile::tempdir().unwrap_or_else(|e| panic!("test home: {e}")));
    let out = Command::new(prog)
        .args(args)
        .current_dir(cwd)
        .env("HOME", home.path())
        .output()
        .unwrap_or_else(|e| panic!("spawn {prog}: {e}"));
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code().unwrap_or(-1),
    )
}

fn dash_available() -> bool {
    Command::new("dash")
        .args(["-c", "exit 0"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn posix_subset_conformance() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path();
    let have_dash = dash_available();
    for (src, want_out, want_st) in CASES {
        let expect = want_out.replace("{dir}", &cwd.display().to_string());
        let (got, st) = run(env!("CARGO_BIN_EXE_brish"), &["-c", src], cwd);
        assert_eq!(
            got, expect,
            "brish stdout for {src:?}: got {got:?}, want {expect:?}"
        );
        assert_eq!(st, *want_st, "brish status for {src:?}: got {st}");
        if have_dash {
            let (dout, dst) = run("dash", &["-c", src], cwd);
            assert_eq!(
                dout, expect,
                "dash disagrees with expectation for {src:?}: got {dout:?}"
            );
            assert_eq!(dst, *want_st, "dash status for {src:?}");
        }
    }
    if !have_dash {
        eprintln!("note: dash not on PATH; ran expectations only");
    }
}
