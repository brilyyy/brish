use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestError {
    /// Malformed expression (unbalanced parens, junk operator, ...).
    BadArg,
    /// An operator is present but its operand is not.
    MissingOperand,
    /// Unknown or misplaced operator.
    BadOperator,
}

impl fmt::Display for TestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadArg => f.write_str("test: bad expression"),
            Self::MissingOperand => f.write_str("test: missing operand"),
            Self::BadOperator => f.write_str("test: unknown operator"),
        }
    }
}

impl std::error::Error for TestError {}

/// One-argument (unary) operators.
const UNARY: &[&str] = &[
    "-e", "-f", "-d", "-r", "-w", "-x", "-s", "-z", "-n", "-h", "-L", "-b", "-c", "-p", "-S", "-g",
    "-u", "-k",
];

/// Two-operand operators.
const BINARY: &[&str] = &[
    "=", "!=", "<", ">", "-eq", "-ne", "-lt", "-le", "-gt", "-ge", "-ef", "-nt", "-ot",
];

/// Shells expand unset/empty variables to nothing, so arithmetic predicates see
/// an environment with no variables at all.
struct EmptyEnv;

impl crate::arith::ArithEnv for EmptyEnv {
    fn get(&mut self, _name: &str) -> Option<i64> {
        None
    }
    fn set(&mut self, _name: &str, _value: i64) {}
}

/// Evaluate a POSIX `test` / `[ ... ]` expression from already-expanded arguments.
/// * 0 args => true
/// * 1 arg => non-empty string
/// * 2/3/4 args => the POSIX table, plus `!`, `( ... )`, `-a`/`-o` chaining and nested parens
///
/// Returns `Err` on malformed expressions so the caller can print a diagnostic
/// and exit with status 2.
pub fn eval(args: &[String]) -> Result<bool, TestError> {
    go(args, 0)
}

fn go(args: &[String], depth: usize) -> Result<bool, TestError> {
    if depth > 128 {
        return Err(TestError::BadArg);
    }
    // No arguments: false (POSIX `test` with no operands, status 1).
    if args.is_empty() {
        return Ok(false);
    }
    // `! expr` (may be chained).
    let mut start = 0;
    let mut negate = false;
    loop {
        match args.get(start) {
            Some(a) if a == "!" => {
                negate = !negate;
                start += 1;
            }
            _ => break,
        }
    }
    let rest = &args[start..];
    let v = go_body(rest, depth)?;
    Ok(if negate { !v } else { v })
}

fn go_body(args: &[String], depth: usize) -> Result<bool, TestError> {
    // `( expr )` only when the group spans the whole expression.
    // Chained groups — `( a ) -a ( b )` — fall through to `-a`/`-o` below.
    // An unbalanced `(` is a hard error either way.
    if args.first().map(String::as_str) == Some("(") {
        match matching_paren(args, 0) {
            Ok(close) if close + 1 == args.len() => return go(&args[1..close], depth + 1),
            Ok(_) => { /* chained groups: fall through */ }
            Err(e) => return Err(e),
        }
    }
    // Unbalanced parens anywhere make the expression malformed.
    let mut parens = 0i64;
    for a in args {
        match a.as_str() {
            "(" => parens += 1,
            ")" => {
                parens -= 1;
                if parens < 0 {
                    return Err(TestError::BadArg);
                }
            }
            _ => {}
        }
    }
    if parens != 0 {
        return Err(TestError::BadArg);
    }

    // `-a` / `-o` chaining: leftmost top-level occurrence splits the expression.
    if let Some(i) = args.iter().position(|a| a == "-a" || a == "-o") {
        if i == 0 || i + 1 == args.len() {
            return Err(TestError::MissingOperand);
        }
        let l = go(&args[..i], depth + 1)?;
        let r = go(&args[i + 1..], depth + 1)?;
        return Ok(if args[i] == "-a" { l && r } else { l || r });
    }

    // Binary operator: leftmost top-level occurrence.
    if let Some(i) = args.iter().position(|a| BINARY.contains(&a.as_str())) {
        if i == 0 || i + 1 >= args.len() {
            return Err(TestError::MissingOperand);
        }
        if i + 2 != args.len() {
            // Operands present but trailing junk follows the expression.
            return Err(TestError::BadArg);
        }
        return binary(&args[i - 1], args[i].as_str(), &args[i + 1]);
    }
    // Unary operator with its operand — check both positions.
    if args.len() == 2 {
        if UNARY.contains(&args[0].as_str()) {
            return unary(&args[1], args[0].as_str());
        }
        if UNARY.contains(&args[1].as_str()) {
            return unary(&args[0], args[1].as_str());
        }
        return Err(TestError::BadOperator);
    }

    // A lone word.
    if args.len() == 1 {
        if UNARY.contains(&args[0].as_str()) || BINARY.contains(&args[0].as_str()) {
            return Err(TestError::MissingOperand);
        }
        return Ok(!args[0].is_empty());
    }
    if args.iter().any(|a| UNARY.contains(&a.as_str())) {
        return Err(TestError::MissingOperand);
    }
    // No operators anywhere: `test a -zzz b` / `test a b c` are errors
    // (bash: "binary operator expected", status 2).
    Err(TestError::BadOperator)
}

/// Index of the `)` matching the `(` at `open`, or `Err` when unbalanced.
fn matching_paren(args: &[String], open: usize) -> Result<usize, TestError> {
    let mut depth = 0i64;
    for (i, a) in args.iter().enumerate().skip(open) {
        match a.as_str() {
            "(" => depth += 1,
            ")" => {
                depth -= 1;
                if depth == 0 {
                    return Ok(i);
                }
                if depth < 0 {
                    return Err(TestError::BadArg);
                }
            }
            _ => {}
        }
    }
    Err(TestError::BadArg)
}

fn binary(lhs: &str, op: &str, rhs: &str) -> Result<bool, TestError> {
    match op {
        "=" | "!=" => {
            let eq = lhs == rhs;
            Ok(if op == "=" { eq } else { !eq })
        }
        "<" | ">" => {
            let ord = lhs.cmp(rhs);
            Ok(if op == "<" { ord.is_lt() } else { ord.is_gt() })
        }
        "-eq" | "-ne" | "-lt" | "-le" | "-gt" | "-ge" => {
            let l = arith_int(lhs)?;
            let r = arith_int(rhs)?;
            Ok(match op {
                "-eq" => l == r,
                "-ne" => l != r,
                "-lt" => l < r,
                "-le" => l <= r,
                "-gt" => l > r,
                _ => l >= r,
            })
        }
        _ => {
            let l = fs_meta(lhs)?;
            let r = fs_meta(rhs)?;
            Ok(match (op, l, r) {
                ("-ef", Some(l), Some(r)) => same_file(&l, &r),
                ("-nt", Some(l), Some(r)) => l.modified().ok() > r.modified().ok(),
                ("-ot", Some(l), Some(r)) => l.modified().ok() < r.modified().ok(),
                _ => false,
            })
        }
    }
}

fn arith_int(s: &str) -> Result<i64, TestError> {
    let s = s.trim();
    if s.is_empty() || !is_integer_literal(s) {
        return Err(TestError::BadArg);
    }
    crate::arith::eval(s, &mut EmptyEnv).map_err(|_| TestError::BadArg)
}

fn is_integer_literal(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() {
        return false;
    }
    let lower = s.to_ascii_lowercase();
    let rest = if lower.starts_with("0x") || lower.starts_with("0o") || lower.starts_with("0b") {
        &lower[2..]
    } else {
        &lower[..]
    };
    if rest.is_empty() {
        return false;
    }
    rest.chars().all(|c| c.is_ascii_digit())
}

/// POSIX mode-bit test (`0o444` readable, `0o222` writable, `0o111`
/// executable; set/sticky bits pass their own masks).
#[cfg(unix)]
fn mode_has(m: &std::fs::Metadata, mask: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    m.mode() & mask != 0
}

#[cfg(not(unix))]
fn mode_has(_m: &std::fs::Metadata, _mask: u32) -> bool {
    // ponytail: no POSIX mode bits on Windows; assume not set. Give
    // Windows real ACL checks if -r/-w/-x matter there.
    false
}

/// `-ef`: same file (device + inode on Unix).
#[cfg(unix)]
fn same_file(l: &std::fs::Metadata, r: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    l.dev() == r.dev() && l.ino() == r.ino()
}

#[cfg(not(unix))]
fn same_file(_l: &std::fs::Metadata, _r: &std::fs::Metadata) -> bool {
    // ponytail: no file-index compare on Windows yet.
    false
}

/// Device/special-file + set/sticky-bit tests behind the final `_` arm.
#[cfg(unix)]
fn file_type_test(op: &str, m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::FileTypeExt;
    let ft = m.file_type();
    match op {
        "-b" => ft.is_block_device(),
        "-c" => ft.is_char_device(),
        "-p" => ft.is_fifo(),
        "-S" => ft.is_socket(),
        "-g" => mode_has(m, 0o2000),
        "-u" => mode_has(m, 0o4000),
        "-k" => mode_has(m, 0o1000),
        _ => false,
    }
}

#[cfg(not(unix))]
fn file_type_test(_op: &str, _m: &std::fs::Metadata) -> bool {
    // ponytail: special files and set/sticky bits don't exist here.
    false
}

fn fs_meta(p: &str) -> Result<Option<std::fs::Metadata>, TestError> {
    let m = if p.len() > 1 && p.ends_with('/') {
        std::fs::metadata(p).ok().filter(std::fs::Metadata::is_dir)
    } else {
        std::fs::metadata(p).ok()
    };
    Ok(m)
}
fn unary(arg: &str, op: &str) -> Result<bool, TestError> {
    match op {
        "-z" => Ok(arg.is_empty()),
        "-n" => Ok(!arg.is_empty()),
        "-h" | "-L" => Ok(std::fs::symlink_metadata(arg)
            .map(|m| m.is_symlink())
            .unwrap_or(false)),
        _ => {
            let Some(m) = fs_meta(arg)? else {
                return Ok(false);
            };
            Ok(match op {
                "-e" => true,
                "-f" => m.is_file(),
                "-d" => m.is_dir(),
                "-r" => mode_has(&m, 0o444),
                "-w" => mode_has(&m, 0o222),
                "-x" => mode_has(&m, 0o111),
                "-s" => m.len() > 0,
                _ => file_type_test(op, &m),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Result<bool, TestError> {
        let args: Vec<String> = items.iter().map(|s| (*s).to_string()).collect();
        eval(&args)
    }

    struct Tmp(std::path::PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("brish-words-test-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            let _ = std::fs::create_dir_all(&p);
            Self(p)
        }
        fn file(&self, name: &str, body: &str) -> String {
            let f = self.0.join(name);
            let _ = std::fs::write(&f, body);
            f.to_string_lossy().into_owned()
        }
        fn sub(&self, name: &str) -> String {
            let f = self.0.join(name);
            let _ = std::fs::create_dir_all(&f);
            f.to_string_lossy().into_owned()
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn arity() {
        assert!(!v(&[]).unwrap());
        assert!(!v(&[""]).unwrap());
        assert!(v(&["x"]).unwrap());
    }

    #[test]
    fn string_tests() {
        assert!(!v(&["a", "-z"]).unwrap());
        assert!(v(&["", "-z"]).unwrap());
        assert!(v(&["a", "-n"]).unwrap());
        assert!(!v(&["", "-n"]).unwrap());
    }

    #[test]
    fn arith_binary() {
        assert!(v(&["1", "-eq", "1"]).unwrap());
        // Real shells reject non-integer operands (`integer expression
        // expected`, exit 2): bash and dash both refuse `1+1` and bare
        // identifiers here — expansion happens before `test` runs.
        assert_eq!(v(&["1+1", "-eq", "2"]).unwrap_err(), TestError::BadArg);
        assert!(v(&["2", "-ne", "1"]).unwrap());
        assert!(v(&["1", "-lt", "2"]).unwrap());
        assert!(v(&["2", "-le", "2"]).unwrap());
        assert!(v(&["3", "-gt", "2"]).unwrap());
        assert!(!v(&["3", "-ge", "4"]).unwrap());
        assert_eq!(v(&["unsetvar", "-eq", "0"]).unwrap_err(), TestError::BadArg);
        assert_eq!(v(&["1", "-eq", "x"]).unwrap_err(), TestError::BadArg);
        assert_eq!(
            v(&["1", "-eq", "1 ", "extra"]).unwrap_err(),
            TestError::BadArg
        );
    }

    #[test]
    fn string_binary() {
        assert!(v(&["abc", "=", "abc"]).unwrap());
        assert!(v(&["abc", "!=", "abd"]).unwrap());
        assert!(v(&["a", "<", "b"]).unwrap());
        assert!(!v(&["b", "<", "a"]).unwrap());
    }

    #[test]
    fn negation_and_parens() {
        assert!(v(&["!", "-z", "x"]).unwrap());
        assert!(!v(&["!", "!", "-z", "x"]).unwrap());
        assert!(v(&["(", "-n", "x", ")"]).unwrap());
        assert!(v(&["(", "(", "-n", "x", ")", ")"]).unwrap());
        assert_eq!(v(&["(", "-n", "x"]).unwrap_err(), TestError::BadArg);
        assert_eq!(v(&["-n", "x", ")"]).unwrap_err(), TestError::BadArg);
    }

    #[test]
    fn chaining() {
        assert!(v(&["-n", "a", "-a", "-n", "b"]).unwrap());
        assert!(!v(&["-n", "a", "-a", "-n", ""]).unwrap());
        assert!(v(&["-n", "a", "-o", "-n", ""]).unwrap());
        assert!(!v(&["-z", "a", "-o", "-z", "b"]).unwrap());
        assert!(v(&["(", "-n", "a", ")", "-a", "(", "-n", "b", ")"]).unwrap());
        assert_eq!(
            v(&["-n", "a", "-a"]).unwrap_err(),
            TestError::MissingOperand
        );
    }

    #[test]
    fn unknown_operator_is_error() {
        assert_eq!(v(&["a", "-zzz", "b"]).unwrap_err(), TestError::BadOperator);
        assert_eq!(v(&["a", "b", "c"]).unwrap_err(), TestError::BadOperator);
        assert_eq!(
            v(&["a", "b", "c", "d"]).unwrap_err(),
            TestError::BadOperator
        );
    }

    #[test]
    fn missing_operand() {
        assert_eq!(v(&["-z"]).unwrap_err(), TestError::MissingOperand);
        assert_eq!(v(&["-e"]).unwrap_err(), TestError::MissingOperand);
        assert_eq!(v(&["-eq"]).unwrap_err(), TestError::MissingOperand);
        assert_eq!(v(&["1", "-eq"]).unwrap_err(), TestError::MissingOperand);
        assert_eq!(v(&["-a"]).unwrap_err(), TestError::MissingOperand);
    }

    #[test]
    fn unknown_operator() {
        assert_eq!(v(&["a", "--bogus"]).unwrap_err(), TestError::BadOperator);
    }

    // ponytail: symlink half needs Unix; Windows gets file tests when
    // someone ports these to std::os::windows.
    #[cfg(unix)]
    #[test]
    fn file_tests() {
        let t = Tmp::new("file");
        let f = t.file("f.txt", "hello");
        let empty = t.file("empty", "");
        let d = t.sub("d");
        let link = t.0.join("link");
        let _ = std::fs::remove_file(&link);
        let _ = std::os::unix::fs::symlink(&f, &link);
        let dangling = t.0.join("dangling");
        let _ = std::fs::remove_file(&dangling);
        let _ = std::os::unix::fs::symlink(t.0.join("nope"), &dangling);

        let lp = link.to_string_lossy().into_owned();
        let dp = dangling.to_string_lossy().into_owned();
        // Make -nt/-ot deterministic: back-to-back writes can share an
        // mtime on fast filesystems.
        let older = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        let newer = std::time::SystemTime::now();
        let set_mtime = |p: &str, t: std::time::SystemTime| {
            if let Ok(f) = std::fs::File::options().write(true).open(p) {
                let _ = f.set_modified(t);
            }
        };
        set_mtime(&empty, older);
        set_mtime(&f, newer);
        assert!(v(&[&f, "-e"]).unwrap());
        assert!(v(&[&f, "-f"]).unwrap());
        assert!(!v(&[&f, "-d"]).unwrap());
        assert!(v(&[&f, "-s"]).unwrap());
        assert!(!v(&[&empty, "-s"]).unwrap());
        assert!(v(&[&d, "-d"]).unwrap());
        assert!(v(&[&f, "-r"]).unwrap());
        // symlink semantics: -e follows, -h does not
        assert!(v(&[&lp, "-e"]).unwrap());
        assert!(v(&[&lp, "-f"]).unwrap());
        assert!(v(&[&lp, "-h"]).unwrap());
        assert!(v(&[&lp, "-L"]).unwrap());
        assert!(v(&[&dp, "-h"]).unwrap());
        assert!(!v(&[&dp, "-e"]).unwrap());
        assert!(!v(&[&dp, "-f"]).unwrap());
        assert!(!v(&["/definitely/not/here", "-e"]).unwrap());
        assert!(!v(&["/definitely/not/here", "-f"]).unwrap());
        // -ef follows symlinks
        assert!(v(&[&f, "-ef", &lp]).unwrap());
        assert!(v(&[&f, "-nt", &empty]).unwrap());
        assert!(v(&[&empty, "-ot", &f]).unwrap());
    }

    #[test]
    fn no_panic_on_junk() {
        let junk = [
            vec!["(", "(", "(", ")", ")"],
            vec!["!", "!", "!", "!"],
            vec![")", "("],
            vec!["-a", "-o", "-a", "-o"],
            vec!["(", "-z"],
            vec!["1", "-eq", "(", "2", ")"],
            vec!["a", "=", "b", "=", "c"],
        ];
        for args in junk {
            let owned: Vec<String> = args.into_iter().map(String::from).collect();
            let _ = eval(&owned);
        }
        let deep: Vec<String> = std::iter::repeat_n("(".to_string(), 500).collect();
        assert!(eval(&deep).is_err());
    }
}
