//! Shell pattern matching (`*`, `?`, `[...]`) for `case`, `${x#pat}` and
//! `${x%pat}`. No pathname I/O here — pure text.

/// Match `text` against shell `pattern`. `[...]` with optional `!` negation
/// supported; POSIX character classes (`[:alpha:]`) not (see `test::eval`).
pub fn pattern_matches(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    m(&p, &t)
}

fn m(p: &[char], t: &[char]) -> bool {
    if p.is_empty() {
        return t.is_empty();
    }
    match p[0] {
        '*' => (0..=t.len()).any(|i| m(&p[1..], &t[i..])),
        '?' => !t.is_empty() && m(&p[1..], &t[1..]),
        '[' => match_bracket(p, t),
        c => !t.is_empty() && t[0] == c && m(&p[1..], &t[1..]),
    }
}

/// Try to match `[...]` at `p[0]` against `t`. Returns true when the whole
/// bracket expression matches one char and the rest matches too.
fn match_bracket(p: &[char], t: &[char]) -> bool {
    if t.is_empty() {
        return false;
    }
    let mut i = 1;
    let neg = matches!(p.get(i), Some('!') | Some('^'));
    if neg {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    let c = t[0];
    while i < p.len() {
        if p[i] == ']' && !first {
            i += 1;
            break;
        }
        first = false;
        // Range `a-z` (but `-` at edges is literal).
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            if p[i] <= c && c <= p[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if p[i] == c {
                matched = true;
            }
            i += 1;
        }
    }
    if i > p.len() {
        return false; // unterminated bracket: no match
    }
    if matched == neg {
        return false;
    }
    m(&p[i..], &t[1..])
}

/// Remove the shortest (`longest == false`) or longest prefix (`from_start`)
/// / suffix of `text` matched by `pattern`. `None` when nothing matches.
pub fn trim(text: &str, pattern: &str, from_start: bool, longest: bool) -> Option<String> {
    let t: Vec<char> = text.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    // Candidate cut points as char indices, ordered by preference:
    // shortest first, where "shortest" is prefix for `#`/`##`, suffix for
    // `%`/`%%`.
    let cuts: Vec<usize> = if from_start ^ longest {
        (0..=t.len()).collect()
    } else {
        (0..=t.len()).rev().collect()
    };
    for cut in cuts {
        let ok = if from_start {
            m(&p, &t[..cut])
        } else {
            m(&p, &t[cut..])
        };
        if ok {
            let s: String = if from_start {
                t[cut..].iter().collect()
            } else {
                t[..cut].iter().collect()
            };
            return Some(s);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_matches() {
        assert!(pattern_matches("*", ""));
        assert!(pattern_matches("*", "abc"));
        assert!(pattern_matches("a*c", "abc"));
        assert!(!pattern_matches("a*c", "ab"));
        assert!(pattern_matches("a?c", "abc"));
        assert!(!pattern_matches("?", "ab"));
        assert!(pattern_matches("[abc]x", "bx"));
        assert!(!pattern_matches("[!abc]x", "bx"));
        assert!(pattern_matches("[!abc]x", "dx"));
        assert!(pattern_matches("[a-c]x", "cx"));
        assert!(!pattern_matches("[a-c]", "d"));
        assert!(pattern_matches("a[.]b", "a.b"));
    }

    #[test]
    fn trim_prefix() {
        assert_eq!(
            trim("/usr/bin", "*/", true, false).as_deref(),
            Some("usr/bin")
        );
        assert_eq!(trim("/usr/bin", "*/", true, true).as_deref(), Some("bin"));
        assert_eq!(trim("abc", "*", true, false).as_deref(), Some("abc"));
        assert_eq!(trim("abc", "*b", true, false).as_deref(), Some("c"));
        assert_eq!(trim("abc", "x*", true, false), None);
        assert_eq!(trim("abc", "?", true, false).as_deref(), Some("bc"));
    }

    #[test]
    fn trim_suffix() {
        assert_eq!(
            trim("/usr/bin", "/*", false, false).as_deref(),
            Some("/usr")
        );
        assert_eq!(trim("aXbXc", "X*", false, false).as_deref(), Some("aXb"));
        assert_eq!(trim("aXbXc", "X*", false, true).as_deref(), Some("a"));
        assert_eq!(trim("abc", "c", false, false).as_deref(), Some("ab"));
        assert_eq!(trim("abc", "*c", false, false).as_deref(), Some("ab"));
        assert_eq!(trim("abc", "*c", false, true).as_deref(), Some(""));
        assert_eq!(trim("abc", "z*", false, false), None);
    }
}
