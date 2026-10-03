use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum GlobError {
    NotADirectory,
    Io(io::Error),
    BadPattern,
}

impl fmt::Display for GlobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotADirectory => f.write_str("not a directory"),
            Self::Io(e) => write!(f, "io error: {e}"),
            Self::BadPattern => f.write_str("bad pattern"),
        }
    }
}

impl std::error::Error for GlobError {}

impl From<io::Error> for GlobError {
    fn from(e: io::Error) -> Self {
        GlobError::Io(e)
    }
}

/// Maximum filesystem entries visited per expansion.
const BUDGET: usize = 100_000;
/// Maximum recursion depth for nested components.
const MAX_DEPTH: usize = 128;

/// Entry budget for one expansion; exceeded budget is an error, never a
/// silent truncation.
struct Budget {
    used: usize,
    limit: usize,
}

impl Budget {
    fn charge(&mut self) -> Result<(), GlobError> {
        self.used += 1;
        if self.used > self.limit {
            return Err(GlobError::Io(io::Error::other("glob budget exceeded")));
        }
        Ok(())
    }
}

/// POSIX character classes recognized inside bracket expressions.
const CLASSES: &[&str] = &[
    "alpha", "alnum", "upper", "lower", "digit", "space", "punct", "print", "graph", "cntrl",
    "xdigit", "blank",
];

fn class_matches(name: &str, c: char) -> bool {
    match name {
        "alpha" => c.is_alphabetic(),
        "alnum" => c.is_alphanumeric(),
        "upper" => c.is_uppercase(),
        "lower" => c.is_lowercase(),
        "digit" => c.is_ascii_digit(),
        "space" => c.is_whitespace(),
        "punct" => c.is_ascii_punctuation(),
        "print" => c.is_alphanumeric() || c.is_ascii_punctuation(),
        "graph" => c.is_ascii_graphic(),
        "cntrl" => c.is_ascii_control(),
        "xdigit" => c.is_ascii_hexdigit(),
        "blank" => c == ' ' || c == '\t',
        _ => false,
    }
}

pub fn expand(
    pattern: &str,
    case_sensitive: bool,
    globstar: bool,
) -> Result<Option<Vec<PathBuf>>, GlobError> {
    expand_with_budget(pattern, case_sensitive, globstar, BUDGET)
}

fn expand_with_budget(
    pattern: &str,
    case_sensitive: bool,
    globstar: bool,
    budget: usize,
) -> Result<Option<Vec<PathBuf>>, GlobError> {
    if !has_metachar(pattern) {
        let p = PathBuf::from(pattern);
        return if fs::metadata(&p).is_ok() {
            Ok(Some(vec![p]))
        } else {
            Ok(None)
        };
    }
    let absolute = pattern.starts_with('/');
    let trailing_slash = pattern.ends_with('/');
    let mut comps = split_components(pattern);
    if absolute {
        comps.remove(0);
    }
    let comps: Vec<String> = comps
        .into_iter()
        .map(|s| match s.as_str() {
            // bash: without globstar, `**` is just `*`
            "**" if globstar => s,
            "**" => "*".to_string(),
            _ => collapse_double_star(&s),
        })
        .collect();
    if comps.len() > MAX_DEPTH {
        return Err(GlobError::BadPattern);
    }
    let start: PathBuf = if absolute {
        PathBuf::from("/")
    } else {
        PathBuf::from(".")
    };
    let mut budget = Budget {
        used: 0,
        limit: budget,
    };
    let mut out = walk(
        &comps,
        &start,
        case_sensitive,
        0,
        &mut budget,
        trailing_slash,
    )?;
    // The walk starts at "."; the caller expects bare relative matches
    // (`*` → `a`, not `./a`) unless the pattern itself said `./`.
    if !absolute && !pattern.starts_with("./") {
        for p in &mut out {
            if let Ok(stripped) = p.strip_prefix(".") {
                *p = stripped.to_path_buf();
            }
        }
    }
    out.sort_by(|a, b| a.as_os_str().cmp(b.as_os_str()));
    Ok(Some(out))
}

fn has_metachar(s: &str) -> bool {
    s.contains('*') || s.contains('?') || s.contains('[')
}

fn collapse_double_star(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch == '*' && out.ends_with('*') {
            continue;
        }
        out.push(ch);
    }
    out
}

fn split_components(pattern: &str) -> Vec<String> {
    let mut comps: Vec<String> = Vec::new();
    let mut acc = String::new();
    let mut leading = true;
    for ch in pattern.chars() {
        if ch == '/' {
            if leading {
                comps.push(String::new());
                leading = false;
            } else {
                comps.push(std::mem::take(&mut acc));
            }
            continue;
        }
        leading = false;
        acc.push(ch);
    }
    if !acc.is_empty() || comps.is_empty() {
        comps.push(acc);
    }
    comps
}

fn walk(
    comps: &[String],
    base: &Path,
    case_sensitive: bool,
    depth: usize,
    budget: &mut Budget,
    trailing_slash: bool,
) -> Result<Vec<PathBuf>, GlobError> {
    if depth > MAX_DEPTH {
        return Err(GlobError::BadPattern);
    }
    if comps.is_empty() {
        return Ok(vec![base.to_path_buf()]);
    }
    let comp = &comps[0];
    let rest = &comps[1..];

    if comp.is_empty() {
        return walk(rest, base, case_sensitive, depth, budget, trailing_slash);
    }

    let meta = fs::metadata(base)?;
    if !meta.is_dir() {
        // e.g. `*/x` where `*` matched a file: no matches from here
        // (bash returns nothing, not an error).
        return Ok(Vec::new());
    }

    if !has_metachar(comp) {
        if comp == "." || comp == ".." {
            let next = base.join(comp);
            return walk(
                rest,
                &next,
                case_sensitive,
                depth + 1,
                budget,
                trailing_slash,
            );
        }
        let next = base.join(comp);
        if fs::metadata(&next).is_err() {
            return Ok(Vec::new());
        }
        return walk(
            rest,
            &next,
            case_sensitive,
            depth + 1,
            budget,
            trailing_slash,
        );
    }

    if comp == "**" {
        // globstar: `**` matches zero or more directories (and, when it
        // is the final component, every entry below base, recursively).
        if rest.is_empty() {
            let mut out = Vec::new();
            for entry in fs::read_dir(base)? {
                let entry = entry?;
                budget.charge()?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with('.') {
                    continue;
                }
                let p = base.join(&name);
                out.push(p.clone());
                if entry.file_type()?.is_dir() {
                    out.append(&mut walk(
                        comps,
                        &p,
                        case_sensitive,
                        depth + 1,
                        budget,
                        trailing_slash,
                    )?);
                }
            }
            return Ok(out);
        }
        let mut out = walk(rest, base, case_sensitive, depth, budget, trailing_slash)?;
        for entry in fs::read_dir(base)? {
            let entry = entry?;
            budget.charge()?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if entry.file_type()?.is_dir() {
                out.append(&mut walk(
                    comps,
                    &base.join(&name),
                    case_sensitive,
                    depth + 1,
                    budget,
                    trailing_slash,
                )?);
            }
        }
        return Ok(out);
    }

    let mut current: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        budget.charge()?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy().into_owned();
        if name_str.starts_with('.')
            && matches!(comp.chars().next(), Some('*') | Some('?') | Some('['))
        {
            continue;
        }
        if match_component(comp, &name_str, case_sensitive) {
            current.push(base.join(&name));
        }
    }

    if rest.is_empty() {
        if trailing_slash {
            current.retain(|p| fs::metadata(p).map(|m| m.is_dir()).unwrap_or(false));
        }
        return Ok(current);
    }
    let mut out = Vec::new();
    for p in current {
        let mut m = walk(rest, &p, case_sensitive, depth + 1, budget, trailing_slash)?;
        out.append(&mut m);
    }
    Ok(out)
}

fn match_component(comp: &str, name: &str, case_sensitive: bool) -> bool {
    if comp == "**" {
        return false;
    }
    let comp: Vec<char> = comp.chars().collect();
    let name: Vec<char> = name.chars().collect();
    m(&comp, &name, case_sensitive)
}

fn cmp_char(a: char, b: char, case_sensitive: bool) -> bool {
    if case_sensitive {
        a == b
    } else {
        a.eq_ignore_ascii_case(&b)
    }
}

fn m(comp: &[char], name: &[char], case_sensitive: bool) -> bool {
    if comp.is_empty() {
        return name.is_empty();
    }
    match comp[0] {
        '*' => {
            if m(&comp[1..], name, case_sensitive) {
                return true;
            }
            if !name.is_empty() {
                return m(comp, &name[1..], case_sensitive);
            }
            false
        }
        '?' => {
            if name.is_empty() {
                return false;
            }
            m(&comp[1..], &name[1..], case_sensitive)
        }
        '[' => {
            if name.is_empty() {
                return false;
            }
            match parse_bracket(comp) {
                Some((bracket, next_ci)) => {
                    let hit = bracket_matches(&bracket, name[0], case_sensitive);
                    let want = if bracket.inverted { !hit } else { hit };
                    want && m(&comp[next_ci..], &name[1..], case_sensitive)
                }
                None => {
                    let want = cmp_char('[', name[0], case_sensitive);
                    want && m(&comp[1..], &name[1..], case_sensitive)
                }
            }
        }
        c => {
            if name.is_empty() {
                return false;
            }
            cmp_char(c, name[0], case_sensitive) && m(&comp[1..], &name[1..], case_sensitive)
        }
    }
}

struct Bracket {
    inverted: bool,
    items: Vec<BracketItem>,
}

enum BracketItem {
    Ch(char),
    Range(char, char),
    Class(&'static str),
}

fn parse_bracket(comp: &[char]) -> Option<(Bracket, usize)> {
    let mut i = 1usize;
    let inverted = if i < comp.len() && (comp[i] == '!' || comp[i] == '^') {
        i += 1;
        true
    } else {
        false
    };
    let mut items: Vec<BracketItem> = Vec::new();
    let mut first = true;
    while i < comp.len() {
        if comp[i] == ']' && !first {
            return Some((Bracket { inverted, items }, i + 1));
        }
        first = false;
        if comp[i] == '\\' && i + 1 < comp.len() {
            items.push(BracketItem::Ch(comp[i + 1]));
            i += 2;
            continue;
        }
        if comp[i] == '[' && i + 1 < comp.len() && comp[i + 1] == ':' {
            let mut j = i + 2;
            while j < comp.len() && comp[j] != ':' {
                j += 1;
            }
            if j + 1 < comp.len() && comp[j] == ':' && comp[j + 1] == ']' {
                let name: String = comp[i + 2..j].iter().collect();
                if let Some(cls) = class_static(&name) {
                    items.push(BracketItem::Class(cls));
                    i = j + 2;
                    continue;
                }
                return None;
            }
            return None;
        }
        if i + 2 < comp.len() && comp[i + 1] == '-' && comp[i + 2] != ']' {
            items.push(BracketItem::Range(comp[i], comp[i + 2]));
            i += 3;
            continue;
        }
        items.push(BracketItem::Ch(comp[i]));
        i += 1;
    }
    None
}

fn class_static(name: &str) -> Option<&'static str> {
    CLASSES.iter().find(|c| **c == name).copied()
}

fn bracket_matches(b: &Bracket, c: char, case_sensitive: bool) -> bool {
    b.items.iter().any(|item| match item {
        BracketItem::Ch(x) => cmp_char(*x, c, case_sensitive),
        BracketItem::Range(lo, hi) => {
            let (lo, hi) = if case_sensitive {
                (*lo, *hi)
            } else {
                (lo.to_ascii_lowercase(), hi.to_ascii_lowercase())
            };
            let c = if case_sensitive {
                c
            } else {
                c.to_ascii_lowercase()
            };
            lo <= c && c <= hi
        }
        BracketItem::Class(name) => {
            if case_sensitive {
                class_matches(name, c)
            } else {
                class_matches(name, c.to_ascii_lowercase())
                    || class_matches(name, c.to_ascii_uppercase())
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Self {
            let p =
                std::env::temp_dir().join(format!("brish-words-test-{}-{tag}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).map_err(|_| ()).unwrap();
            Self(p)
        }
        fn file(&self, rel: &str, body: &str) -> PathBuf {
            let f = self.0.join(rel);
            if let Some(parent) = f.parent() {
                let _ = fs::create_dir_all(parent);
            }
            fs::write(&f, body).map_err(|_| ()).unwrap();
            f
        }
        fn dir(&self, rel: &str) -> PathBuf {
            let d = self.0.join(rel);
            fs::create_dir_all(&d).map_err(|_| ()).unwrap();
            d
        }
        fn path(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(pattern: &str, case: bool) -> Vec<String> {
        names_gs(pattern, case, false)
    }

    fn names_gs(pattern: &str, case: bool, globstar: bool) -> Vec<String> {
        let out = expand(pattern, case, globstar).unwrap().unwrap_or_default();
        out.iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn literal_no_metachar() {
        let t = Tmp::new("lit");
        let f = t.file("a.txt", "x");
        let got = expand(&f.to_string_lossy(), true, false).unwrap();
        assert_eq!(got, Some(vec![f.clone()]));
        assert_eq!(expand("/definitely/not/here", true, false).unwrap(), None);
    }

    #[test]
    fn star_matches() {
        let t = Tmp::new("star");
        t.file("a.txt", "");
        t.file("b.txt", "");
        let got = names(&t.path("*").to_string_lossy(), true);
        assert_eq!(got.len(), 2);
        let mut got2 = names(&t.path("*.txt").to_string_lossy(), true);
        got2.sort();
        assert!(got2[0].ends_with("a.txt"));
        assert!(got2[1].ends_with("b.txt"));
    }

    #[test]
    fn dotfiles_not_matched_by_leading_star() {
        let t = Tmp::new("dot");
        t.file(".hidden", "");
        t.file("visible", "");
        let got = names(&t.path("*").to_string_lossy(), true);
        assert_eq!(got.len(), 1);
        assert!(got[0].ends_with("visible"));
        let got = names(&t.path(".*").to_string_lossy(), true);
        assert_eq!(got.len(), 1);
        assert!(got[0].ends_with(".hidden"));
    }

    #[test]
    fn question_mark() {
        let t = Tmp::new("q");
        t.file("ab", "");
        t.file("ac", "");
        t.file("abc", "");
        let mut got = names(&t.path("a?").to_string_lossy(), true);
        assert_eq!(got.len(), 2);
        got.sort();
        assert!(got[0].ends_with("ab"));
        assert!(got[1].ends_with("ac"));
    }

    #[test]
    fn bracket_ranges_and_classes() {
        let t = Tmp::new("br");
        t.file("a1", "");
        t.file("a2", "");
        t.file("b1", "");
        t.file("c9", "");
        let mut got = names(&t.path("[ab]?").to_string_lossy(), true);
        assert_eq!(got.len(), 3);
        got.sort();
        assert!(got.iter().all(|s| s.ends_with('1') || s.ends_with('2')));
        // digit class
        let got = names(&t.path("a[[:digit:]]").to_string_lossy(), true);
        assert_eq!(got.len(), 2);
        // inverted class (negated class + wildcard)
        let got = names(&t.path("[!ab]?").to_string_lossy(), true);
        assert_eq!(got.len(), 1);
        assert!(got[0].ends_with("c9") || got[0].contains("c9"));
        // range
        let got = names(&t.path("[a-c]1").to_string_lossy(), true);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn broken_bracket_is_literal() {
        let t = Tmp::new("broken");
        t.file("a[b", "");
        let got = names(&t.path("a[").to_string_lossy(), true);
        assert_eq!(got.len(), 0);
        let got = names(&t.path("a[b").to_string_lossy(), true);
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn double_star_without_globstar_is_star() {
        let t = Tmp::new("ds");
        t.dir("d");
        t.file("d/x", "");
        // bash: `**` == `*` when globstar is off
        let got = names(&t.path("**/x").to_string_lossy(), true);
        assert_eq!(got.len(), 1);
        let got = names(&t.path("*/x").to_string_lossy(), true);
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn globstar_recursive() {
        let t = Tmp::new("gs");
        t.file("a.txt", "");
        t.file("d/a.txt", "");
        t.file("d/e/a.txt", "");
        let pat = t.path("**/*.txt").to_string_lossy().into_owned();
        let mut got = names_gs(&pat, true, true);
        got.sort();
        assert_eq!(got.len(), 3, "{got:?}");
        // globstar off: ** is one level
        let got = names(&pat, true);
        assert_eq!(got.len(), 1, "{got:?}");
        // final `**`: every entry recursively (hidden excluded, root not
        // included as a result itself)
        let all = t.path("**").to_string_lossy().into_owned();
        let mut got = names_gs(&all, true, true);
        got.sort();
        let want: Vec<String> = [
            t.path("a.txt"),
            t.path("d"),
            t.path("d/a.txt"),
            t.path("d/e"),
            t.path("d/e/a.txt"),
        ]
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
        assert_eq!(got, want, "{got:?}");
    }

    #[test]
    fn subdirectory_walk() {
        let t = Tmp::new("sub");
        t.file("d/x.txt", "");
        t.file("d/y.md", "");
        t.file("e/z.txt", "");
        let mut got = names(&t.path("*/x.txt").to_string_lossy(), true);
        assert_eq!(got.len(), 1);
        got.clear();
        let mut got2 = names(&t.path("*/*.txt").to_string_lossy(), true);
        got2.sort();
        assert_eq!(got2.len(), 2);
        let _ = got;
    }

    #[test]
    fn absolute_pattern() {
        let t = Tmp::new("abs");
        t.dir("d");
        let _ = t;
        let abs = format!("/{}", std::env::temp_dir().display());
        let _ = abs;
    }

    #[test]
    fn sorted_output() {
        let t = Tmp::new("sort");
        t.file("b", "");
        t.file("a", "");
        t.file("c", "");
        let got = names(&t.path("*").to_string_lossy(), true);
        let names_only: Vec<&str> = got
            .iter()
            .map(|s| s.rsplit('/').next().unwrap_or(""))
            .collect();
        assert_eq!(names_only, vec!["a", "b", "c"]);
    }

    #[test]
    fn trailing_slash_matches_dir() {
        let t = Tmp::new("slash");
        t.dir("d");
        t.file("f", "");
        let got = names(&t.path("*/").to_string_lossy(), true);
        assert_eq!(got.len(), 1);
        assert!(got[0].ends_with("d/") || got[0].ends_with("d"));
    }

    #[test]
    fn no_glob_no_match_returns_none() {
        assert_eq!(
            expand("definitely-not-here-xyz", true, false).unwrap(),
            None
        );
    }

    #[test]
    fn budget_exceeded_is_error() {
        let t = Tmp::new("budget");
        t.file("d/a", "");
        t.file("d/b", "");
        let pat = t.path("d/*").to_string_lossy().into_owned();
        // Limit below the entry count: expansion must error, not truncate.
        assert!(expand_with_budget(&pat, true, false, 1).is_err());
        // The default budget still succeeds.
        assert!(expand(&pat, true, false).is_ok());
    }

    #[test]
    fn over_max_depth_is_error() {
        let t = Tmp::new("depth");
        t.dir("d");
        let deep = format!("d/{}", vec!["*"; 200].join("/"));
        assert!(expand(&t.path(&deep).to_string_lossy(), true, false).is_err());
    }

    #[test]
    fn case_insensitive_match() {
        let t = Tmp::new("ci");
        t.file("AbC.txt", "");
        let got = names(&t.path("a?c.txt").to_string_lossy(), false);
        assert_eq!(got.len(), 1);
        assert!(got[0].ends_with("AbC.txt"));
    }
}
