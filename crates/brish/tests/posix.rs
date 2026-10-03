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
    ("echo a{b,c}d", "a{b,c}d\n", 0),
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
