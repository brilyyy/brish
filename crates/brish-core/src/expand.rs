//! POSIX word expansion in required order: tilde → parameter / command
//! substitution / arithmetic → field splitting → pathname globbing →
//! quote removal.
//!
//! Brace expansion is out of scope (non-POSIX, plan §roadmap).

use crate::env::Env;
use crate::error::Error;
use crate::lexer::{self, Part, Word};
use brish_words::pattern::trim;
use brish_words::{eval_arith, expand as glob_expand, fields as split_fields};

/// Command-substitution runner: executes `$(...)` source, returns stdout.
pub type CmdSubst<'a> = &'a mut dyn FnMut(&str) -> Result<String, Error>;

/// Recursion cap for nested substitutions / default words.
const MAX_DEPTH: usize = 64;

/// Expand one word into fields (splitting, globbing, quote removal).
pub fn expand_word(env: &mut Env, w: &Word, cs: CmdSubst) -> Result<Vec<String>, Error> {
    Ex { env, cs, depth: 0 }.expand_word(w)
}

/// Expand words, concatenating all resulting fields in order.
pub fn expand_words(env: &mut Env, ws: &[Word], cs: CmdSubst) -> Result<Vec<String>, Error> {
    let mut out = Vec::new();
    for w in ws {
        out.append(&mut expand_word(env, w, cs)?);
    }
    Ok(out)
}

/// Expand a word to one string: no field splitting, no globbing
/// (assignment values, here-doc bodies, patterns).
pub fn expand_value(env: &mut Env, w: &Word, cs: CmdSubst) -> Result<String, Error> {
    Ex { env, cs, depth: 0 }.value_of_word(w)
}

/// Expand raw text (here-doc body): substitutions only; lexing failure
/// falls back to the text unchanged (unterminated quote stays literal).
pub fn expand_text(env: &mut Env, src: &str, cs: CmdSubst) -> Result<String, Error> {
    let lexed = match lexer::lex(src) {
        Ok(l) => l,
        Err(_) => return Ok(src.to_string()),
    };
    let mut ex = Ex { env, cs, depth: 0 };
    let mut out = String::new();
    let mut prev_end = 0;
    for t in &lexed.tokens {
        // Raw gap between tokens: whitespace, comments, operators.
        out.push_str(&src[prev_end..t.span.start]);
        prev_end = t.span.end;
        if let lexer::Tok::Word(w) = &t.tok {
            out.push_str(&ex.value_of_word(w)?);
        } else {
            out.push_str(&src[t.span.start..t.span.end]);
        }
    }
    out.push_str(&src[prev_end..]);
    if crate::debug_on("expand") {
        eprintln!("brish[expand]: {src:?} -> {out:?}");
    }
    Ok(out)
}

/// One piece of an expanded word. `quoted` suppresses splitting/globbing.
#[derive(Debug, Clone)]
enum Seg {
    T {
        text: String,
        quoted: bool,
        glob: bool,
    },
    /// `$@`-style: each item is a separate field (prefix merges into first,
    /// suffix into last — handled by the assembler).
    F { items: Vec<String>, quoted: bool },
}

struct Ex<'a> {
    env: &'a mut Env,
    cs: CmdSubst<'a>,
    depth: usize,
}

impl<'a> Ex<'a> {
    fn expand_word(&mut self, w: &Word) -> Result<Vec<String>, Error> {
        let segs = self.segs_of_word(w)?;
        self.assemble(segs, !self.env.opts.noglob)
    }

    fn value_of_word(&mut self, w: &Word) -> Result<String, Error> {
        let segs = self.segs_of_word(w)?;
        let mut s = String::new();
        for seg in segs {
            match seg {
                Seg::T { text, .. } => s.push_str(&text),
                Seg::F { items, .. } => s.push_str(&items.join(" ")),
            }
        }
        Ok(s)
    }

    fn segs_of_word(&mut self, w: &Word) -> Result<Vec<Seg>, Error> {
        let parts = w.parts.as_slice();
        // Tilde only from a literal `~` at word start (unquoted).
        if let Some(Part::Raw(t)) = parts.first()
            && let Some(rest) = t.strip_prefix('~')
            && let Some(text) = self.tilde(rest)
        {
            let glob = has_meta(&text);
            return Ok(vec![Seg::T {
                text,
                quoted: false,
                glob,
            }]);
        }
        self.parts_segs(parts, false)
    }

    fn tilde(&self, rest: &str) -> Option<String> {
        if rest.is_empty() || rest.starts_with('/') {
            let home = self.env.get("HOME")?;
            let mut s = home.to_string();
            s.push_str(rest);
            return Some(s);
        }
        let first = rest.chars().next()?;
        let tail = &rest[first.len_utf8()..];
        match first {
            '+' => {
                let pwd = self.env.get("PWD")?;
                let mut s = pwd.to_string();
                s.push_str(tail);
                Some(s)
            }
            '-' => {
                let old = self.env.get("OLDPWD")?;
                let mut s = old.to_string();
                s.push_str(tail);
                Some(s)
            }
            // `~user`: needs getpwnam (libc) — ponytail: leave literal until
            // brish-platform grows a user-database lookup.
            _ => None,
        }
    }

    fn parts_segs(&mut self, parts: &[Part], quoted: bool) -> Result<Vec<Seg>, Error> {
        let mut out = Vec::new();
        for p in parts {
            match p {
                Part::Raw(t) => out.push(Seg::T {
                    text: t.clone(),
                    quoted,
                    glob: !quoted && has_meta(t),
                }),
                Part::Esc(c) => out.push(Seg::T {
                    text: c.to_string(),
                    quoted: true,
                    glob: false,
                }),
                Part::Single(t) => out.push(Seg::T {
                    text: t.clone(),
                    quoted: true,
                    glob: false,
                }),
                Part::Double(inner) => {
                    if inner.is_empty() {
                        // `""` is one empty quoted field, not zero fields.
                        out.push(Seg::T {
                            text: String::new(),
                            quoted: true,
                            glob: false,
                        });
                    } else {
                        out.extend(self.parts_segs(inner, true)?);
                    }
                }
                Part::Param(expr) => out.extend(self.param(expr, quoted)?),
                Part::Subst(src) => {
                    let v = self.cmdsubst(src)?;
                    out.push(Seg::T {
                        glob: !quoted && has_meta(&v),
                        text: v,
                        quoted,
                    });
                }
                Part::Arith(src) => {
                    let v = self.arith(src)?;
                    out.push(Seg::T {
                        text: v,
                        quoted,
                        glob: false,
                    });
                }
            }
        }
        Ok(out)
    }

    fn cmdsubst(&mut self, src: &str) -> Result<String, Error> {
        if self.depth >= MAX_DEPTH {
            return Err(Error::expand("command substitution recursion too deep"));
        }
        self.depth += 1;
        let raw = (self.cs)(src)?;
        self.depth -= 1;
        // POSIX: strip ALL trailing newlines.
        Ok(raw.trim_end_matches('\n').to_string())
    }

    fn arith(&mut self, src: &str) -> Result<String, Error> {
        let v = eval_arith(src, &mut *self.env)
            .map_err(|e| Error::expand(format!("arithmetic: {e}")))?;
        Ok(v.to_string())
    }

    // ---- parameter expansion ----

    fn param(&mut self, expr: &str, quoted: bool) -> Result<Vec<Seg>, Error> {
        if self.depth >= MAX_DEPTH {
            return Err(Error::expand("parameter expansion recursion too deep"));
        }
        self.depth += 1;
        let r = self.param_inner(expr, quoted);
        self.depth -= 1;
        r
    }

    fn param_inner(&mut self, expr: &str, quoted: bool) -> Result<Vec<Seg>, Error> {
        let form = parse_form(expr)?;
        match form {
            Form::Plain(name) => {
                if name == "@" {
                    let items = self.positional_items(quoted)?;
                    if items.is_empty() {
                        return Ok(Vec::new());
                    }
                    return Ok(vec![Seg::F { items, quoted }]);
                }
                if name == "*" {
                    let joined = self.positional_joined();
                    return Ok(vec![Seg::T {
                        glob: !quoted && has_meta(&joined),
                        text: joined,
                        quoted,
                    }]);
                }
                let v = match self.param_value(name) {
                    Some(v) => v,
                    None if self.env.opts.nounset && !nounset_exempt(name) => {
                        return Err(Error::expand(format!("{name}: unbound variable")));
                    }
                    None => String::new(),
                };
                Ok(vec![Seg::T {
                    glob: !quoted && has_meta(&v),
                    text: v,
                    quoted,
                }])
            }
            Form::Len(name) => {
                let v = self.param_value(name).unwrap_or_default();
                let n = v.chars().count();
                Ok(vec![Seg::T {
                    text: n.to_string(),
                    quoted,
                    glob: false,
                }])
            }
            Form::Op { name, op, word } => {
                let v = self.op_value(name, op, word)?;
                Ok(vec![Seg::T {
                    glob: !quoted && has_meta(&v),
                    text: v,
                    quoted,
                }])
            }
        }
    }

    fn positional_items(&mut self, quoted: bool) -> Result<Vec<String>, Error> {
        let params: Vec<String> = self.env.positional().to_vec();
        if quoted {
            return Ok(params);
        }
        let ifs = self.env.ifs().to_string();
        Ok(params.iter().flat_map(|p| split_fields(p, &ifs)).collect())
    }

    fn positional_joined(&self) -> String {
        let ifs = self.env.ifs();
        let sep: String = ifs.chars().next().map(String::from).unwrap_or_default();
        self.env.positional().join(&sep)
    }

    /// Current value of a parameter (`None` = unset).
    fn param_value(&self, name: &str) -> Option<String> {
        if let Some(rest) = name.strip_prefix(|c: char| c.is_ascii_digit()) {
            // positional, incl. `${10}`; `$0` is the shell name (below)
            if rest.chars().all(|c| c.is_ascii_digit()) && name != "0" {
                let idx: usize = name.parse().ok()?;
                return self.env.positional().get(idx.checked_sub(1)?).cloned();
            }
            // e.g. "12abc": fall through to var lookup (unset)
        }
        match name {
            "?" => Some(self.env.status.to_string()),
            "$" => Some(self.env.pid.to_string()),
            "!" => Some(self.env.last_bg.map(|p| p.to_string()).unwrap_or_default()),
            "#" => Some(self.env.positional().len().to_string()),
            "-" => Some(format!("{}{}", self.env.flags, self.env.opts.letters())),
            "0" => Some(self.env.name.clone()),
            _ => self.env.get(name).map(str::to_string),
        }
    }

    fn op_value(&mut self, name: &str, op: &str, word: &str) -> Result<String, Error> {
        let current = self.param_value(name);
        let colon = op.starts_with(':');
        let kind = op.trim_start_matches(':');
        let unset = current.is_none();
        let empty = current.as_deref().is_none_or(str::is_empty);
        let cond = if colon { unset || empty } else { unset };

        match kind.as_bytes().first().copied().unwrap_or(b'?') {
            b'-' => {
                if cond {
                    let w = self.lex_one(word)?;
                    self.value_of_word(&w)
                } else {
                    Ok(current.unwrap_or_default())
                }
            }
            b'=' => {
                if !is_assign_target(name) {
                    return Err(Error::expand(format!("cannot assign in this way: {name}")));
                }
                if cond {
                    let w = self.lex_one(word)?;
                    let v = self.value_of_word(&w)?;
                    self.env.set(name, v.clone())?;
                    Ok(v)
                } else {
                    Ok(current.unwrap_or_default())
                }
            }
            b'?' => {
                if cond {
                    let w = self.lex_one(word)?;
                    let msg = self.value_of_word(&w)?;
                    if msg.is_empty() {
                        Err(Error::expand(format!("{name}: parameter null or not set")))
                    } else {
                        Err(Error::expand(format!("{name}: {msg}")))
                    }
                } else {
                    Ok(current.unwrap_or_default())
                }
            }
            b'+' => {
                if cond {
                    Ok(String::new())
                } else {
                    let w = self.lex_one(word)?;
                    self.value_of_word(&w)
                }
            }
            b'#' | b'%' => {
                let base = current.unwrap_or_default();
                let longest = kind.len() == 2;
                let from_start = kind.starts_with('#');
                let w = self.lex_one(word)?;
                let pat = self.value_of_word(&w)?;
                Ok(trim(&base, &pat, from_start, longest).unwrap_or(base))
            }
            _ => Err(Error::expand(format!(
                "bad substitution: {}",
                expr_dbg(name, op, word)
            ))),
        }
    }

    /// Lex a default/pattern fragment as one word (may be empty).
    fn lex_one(&self, src: &str) -> Result<Word, Error> {
        if src.is_empty() {
            return Ok(Word {
                parts: Vec::new(),
                span: lexer::Span { start: 0, end: 0 },
            });
        }
        let lexed = lexer::lex(src)?;
        let mut words = Vec::new();
        for t in lexed.tokens {
            if let lexer::Tok::Word(w) = t.tok {
                words.push(w);
            }
        }
        match words.len() {
            0 => Ok(Word {
                parts: Vec::new(),
                span: lexer::Span { start: 0, end: 0 },
            }),
            // `${x:-a b}` — multiple tokens joined as raw text (no splitting
            // at this level; outer assembly splits the final string).
            1 => Ok(words
                .pop()
                .ok_or_else(|| Error::expand("bad substitution"))?),
            _ => {
                // Join with literal spaces, re-lex-safe: build one Raw word
                // from already-expanded text later; here just concatenate
                // parts preserving quoting of each token.
                let mut parts = Vec::new();
                for (i, w) in words.iter().enumerate() {
                    if i > 0 {
                        parts.push(Part::Raw(" ".into()));
                    }
                    parts.extend(w.parts.iter().cloned());
                }
                Ok(Word {
                    parts,
                    span: lexer::Span {
                        start: 0,
                        end: src.len(),
                    },
                })
            }
        }
    }

    // ---- assembly: splitting, globbing, quote removal ----

    fn assemble(&mut self, segs: Vec<Seg>, do_glob: bool) -> Result<Vec<String>, Error> {
        let ifs = self.env.ifs().to_string();
        let mut fields: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut cur_glob = false;
        let mut produced = false;

        for seg in segs {
            match seg {
                Seg::T { text, quoted, glob } => {
                    if quoted {
                        cur.push_str(&text);
                        produced = true;
                    } else if !text.is_empty() {
                        produced = true;
                        if glob {
                            cur_glob = true;
                        }
                        let chunks = split_fields(&text, &ifs);
                        let mut it = chunks.into_iter();
                        if let Some(first) = it.next() {
                            cur.push_str(&first);
                        }
                        for c in it {
                            self.flush_field(&mut fields, &mut cur, &mut cur_glob, do_glob)?;
                            cur.push_str(&c);
                        }
                    }
                }
                Seg::F { items, quoted } => {
                    let mut it = items.into_iter();
                    if let Some(first) = it.next() {
                        produced = true;
                        if !quoted && has_meta(&first) {
                            cur_glob = true;
                        }
                        cur.push_str(&first);
                        for item in it {
                            self.flush_field(&mut fields, &mut cur, &mut cur_glob, do_glob)?;
                            if !quoted && has_meta(&item) {
                                cur_glob = true;
                            }
                            cur = item;
                        }
                    }
                }
            }
        }
        if produced {
            self.flush_field(&mut fields, &mut cur, &mut cur_glob, do_glob)?;
        }
        Ok(fields)
    }

    fn flush_field(
        &mut self,
        fields: &mut Vec<String>,
        cur: &mut String,
        cur_glob: &mut bool,
        do_glob: bool,
    ) -> Result<(), Error> {
        let glob = std::mem::take(cur_glob);
        let text = std::mem::take(cur);
        if do_glob && glob && has_meta(&text) {
            match glob_expand(&text, true) {
                Ok(Some(matches)) if matches.is_empty() => {
                    fields.push(text);
                    return Ok(());
                }
                Ok(Some(mut matches)) => {
                    matches.sort();
                    for m in matches {
                        fields.push(path_to_string(&m));
                    }
                    return Ok(());
                }
                Ok(None) => {
                    // No match: keep the literal pattern (bash default).
                    fields.push(text);
                    return Ok(());
                }
                Err(e) => return Err(Error::expand(format!("glob: {e}"))),
            }
        }
        fields.push(text);
        Ok(())
    }
}

fn path_to_string(p: &std::path::Path) -> String {
    p.as_os_str()
        .to_str()
        .map(str::to_string)
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// `name=value` first part of an assignment word: recompute value with
/// tilde handling at value start, no split/glob.
pub fn expand_assign_value(env: &mut Env, w: &Word, cs: CmdSubst) -> Result<String, Error> {
    let mut ex = Ex { env, cs, depth: 0 };
    let mut out = String::new();
    for (i, p) in w.parts.iter().enumerate() {
        match p {
            Part::Raw(t) if i == 0 => {
                if let Some(eq) = t.find('=') {
                    out.push_str(&t[..=eq]);
                    let rest = &t[eq + 1..];
                    // Tilde at value start (FOO=~/bin); quoted parts later
                    // are not Raw so they never hit this branch.
                    if let Some(r) = rest.strip_prefix('~')
                        && let Some(h) = ex.tilde(r)
                    {
                        out.push_str(&h);
                        continue;
                    }
                    out.push_str(rest);
                } else {
                    out.push_str(t);
                }
            }
            _ => {
                for s in ex.parts_segs(std::slice::from_ref(p), false)? {
                    match s {
                        Seg::T { text, .. } => out.push_str(&text),
                        Seg::F { items, .. } => out.push_str(&items.join(" ")),
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Word is an assignment prefix (`NAME=...`).
pub fn is_assign_word(w: &Word) -> bool {
    match w.parts.first() {
        Some(Part::Raw(t)) => match t.find('=') {
            Some(eq) => crate::env::is_name(&t[..eq]),
            None => false,
        },
        _ => false,
    }
}

/// Split `NAME=...` into name and value word.
pub fn split_assign(w: &Word) -> Option<(String, Word)> {
    let eq = match w.parts.first() {
        Some(Part::Raw(t)) => t.find('=')?,
        _ => return None,
    };
    let name = {
        let t = match w.parts.first() {
            Some(Part::Raw(t)) => t,
            _ => return None,
        };
        if !crate::env::is_name(&t[..eq]) {
            return None;
        }
        t[..eq].to_string()
    };
    let mut parts = Vec::new();
    let first = match w.parts.first() {
        Some(Part::Raw(t)) => t,
        _ => return None,
    };
    let rest = &first[eq + 1..];
    if !rest.is_empty() {
        parts.push(Part::Raw(rest.to_string()));
    }
    parts.extend(w.parts.iter().skip(1).cloned());
    Some((
        name,
        Word {
            parts,
            span: w.span,
        },
    ))
}

fn expr_dbg(name: &str, op: &str, word: &str) -> String {
    format!("{name}{op}{word}")
}

fn is_assign_target(name: &str) -> bool {
    crate::env::is_name(name)
}

fn has_meta(s: &str) -> bool {
    s.contains(['*', '?', '['])
}

enum Form<'a> {
    Plain(&'a str),
    Len(&'a str),
    Op {
        name: &'a str,
        op: &'a str,
        word: &'a str,
    },
}

fn parse_form(expr: &str) -> Result<Form<'_>, Error> {
    if expr.is_empty() {
        return Err(Error::expand("bad substitution"));
    }
    if let Some(rest) = expr.strip_prefix('#') {
        if rest.is_empty() {
            return Ok(Form::Plain("#"));
        }
        if is_param_name(rest) {
            return Ok(Form::Len(rest));
        }
        return Err(Error::expand(format!("bad substitution: ${{{expr}}}")));
    }
    // Name: alnum/underscore run, or one special char.
    let name_len = expr
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .map(char::len_utf8)
        .sum::<usize>();
    let (name, rest) = if name_len > 0 {
        (&expr[..name_len], &expr[name_len..])
    } else {
        match expr.chars().next() {
            Some(c) if "?@$*!-".contains(c) => (&expr[..c.len_utf8()], &expr[c.len_utf8()..]),
            _ => return Err(Error::expand(format!("bad substitution: ${{{expr}}}"))),
        }
    };
    if rest.is_empty() {
        return Ok(Form::Plain(name));
    }
    for op in [":-", ":=", ":?", ":+", "##", "%%"] {
        if let Some(word) = rest.strip_prefix(op) {
            return Ok(Form::Op { name, op, word });
        }
    }
    for op in ["-", "=", "?", "+", "#", "%"] {
        if let Some(word) = rest.strip_prefix(op) {
            return Ok(Form::Op { name, op, word });
        }
    }
    Err(Error::expand(format!("bad substitution: ${{{expr}}}")))
}

fn is_param_name(s: &str) -> bool {
    crate::env::is_name(s) || (s.chars().count() == 1 && "?@$*!-0123456789#".contains(s))
}

/// Special params that never trip `set -u` (POSIX: `$0`, `$?`, `$-`, ...).
fn nounset_exempt(name: &str) -> bool {
    matches!(name, "?" | "$" | "!" | "#" | "-" | "0" | "@" | "*") || name.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(vars: &[(&str, &str)]) -> Env {
        let mut e = Env::new();
        for (k, v) in vars {
            e.set(k, *v).unwrap();
        }
        e
    }

    fn one_word(parts: Vec<Part>) -> Word {
        Word {
            parts,
            span: lexer::Span { start: 0, end: 0 },
        }
    }

    fn raw(s: &str) -> Part {
        Part::Raw(s.to_string())
    }

    fn param(expr: &str) -> Part {
        Part::Param(expr.to_string())
    }

    fn expand(env: &mut Env, parts: Vec<Part>) -> Vec<String> {
        let w = one_word(parts);
        let mut cs = |_src: &str| -> Result<String, Error> { Ok(String::new()) };
        expand_word(env, &w, &mut cs).unwrap()
    }

    fn value(env: &mut Env, parts: Vec<Part>) -> String {
        let w = one_word(parts);
        let mut cs = |_src: &str| -> Result<String, Error> { Ok(String::new()) };
        expand_value(env, &w, &mut cs).unwrap()
    }

    fn value_err(env: &mut Env, parts: Vec<Part>) -> bool {
        let w = one_word(parts);
        let mut cs = |_src: &str| -> Result<String, Error> { Ok(String::new()) };
        expand_value(env, &w, &mut cs).is_err()
    }

    #[test]
    fn plain_and_quoted() {
        let mut e = env_with(&[]);
        assert_eq!(expand(&mut e, vec![raw("abc")]), vec!["abc"]);
        assert_eq!(
            expand(&mut e, vec![Part::Double(vec![raw("a b")])]),
            vec!["a b"]
        );
        assert_eq!(expand(&mut e, vec![raw("a b")]), vec!["a", "b"]);
    }

    #[test]
    fn empty_quoted_word_is_a_field() {
        let mut e = env_with(&[]);
        // `cmd ""` passes one empty argument; `cmd $unset` passes none.
        assert_eq!(expand(&mut e, vec![Part::Double(vec![])]), vec![""]);
        assert_eq!(expand(&mut e, vec![Part::Single(String::new())]), vec![""]);
        assert!(expand(&mut e, vec![Part::Param("no_such".into())]).is_empty());
    }

    #[test]
    fn param_splitting() {
        let mut e = env_with(&[("L", "a b  c")]);
        assert_eq!(expand(&mut e, vec![param("L")]), vec!["a", "b", "c"]);
        assert_eq!(
            expand(&mut e, vec![Part::Double(vec![param("L")])]),
            vec!["a b  c"]
        );
        assert!(expand(&mut e, vec![param("NOPE")]).is_empty());
        assert_eq!(
            expand(&mut e, vec![Part::Double(vec![param("NOPE")])]),
            vec![""]
        );
    }

    #[test]
    fn ifs_custom() {
        let mut e = env_with(&[("L", "a:b::c"), ("IFS", ":")]);
        assert_eq!(expand(&mut e, vec![param("L")]), vec!["a", "b", "", "c"]);
        e.set("IFS", "").unwrap();
        assert_eq!(expand(&mut e, vec![param("L")]), vec!["a:b::c"]);
    }

    #[test]
    fn default_operators() {
        let mut e = env_with(&[("E", ""), ("S", "val")]);
        assert_eq!(value(&mut e, vec![param("E:-d")]), "d");
        assert_eq!(value(&mut e, vec![param("E-d")]), "");
        assert_eq!(value(&mut e, vec![param("NOPE-d")]), "d");
        assert_eq!(value(&mut e, vec![param("E:+x")]), "");
        assert_eq!(value(&mut e, vec![param("S:+x")]), "x");
        assert_eq!(value(&mut e, vec![param("S:-d")]), "val");
        assert_eq!(value(&mut e, vec![param("NEW:=init")]), "init");
        assert_eq!(e.get("NEW"), Some("init"));
        assert_eq!(value(&mut e, vec![param("NEW:=other")]), "init");
        assert!(value_err(&mut e, vec![param("E:?required")]));
        assert!(!value_err(&mut e, vec![param("S:?required")]));
    }

    #[test]
    fn length_operator() {
        let mut e = env_with(&[("S", "héllo")]);
        assert_eq!(value(&mut e, vec![param("#S")]), "5");
        e.set("N", "42").unwrap();
        assert_eq!(value(&mut e, vec![param("#N")]), "2");
        assert_eq!(value(&mut e, vec![param("#")]), "0");
    }

    #[test]
    fn pattern_trim() {
        let mut e = env_with(&[("P", "/usr/local/bin")]);
        assert_eq!(value(&mut e, vec![param("P#*/")]), "usr/local/bin");
        assert_eq!(value(&mut e, vec![param("P##*/")]), "bin");
        assert_eq!(value(&mut e, vec![param("P%/*")]), "/usr/local");
        assert_eq!(value(&mut e, vec![param("P%%/*")]), "");
        assert_eq!(value(&mut e, vec![param("P#zz*")]), "/usr/local/bin");
    }

    #[test]
    fn special_params() {
        let mut e = env_with(&[]);
        e.status = 3;
        e.pid = 999;
        e.set_positional(vec!["x".into(), "y".into()]);
        assert_eq!(value(&mut e, vec![param("?")]), "3");
        assert_eq!(value(&mut e, vec![param("$")]), "999");
        assert_eq!(value(&mut e, vec![param("#")]), "2");
        assert_eq!(value(&mut e, vec![param("1")]), "x");
        assert_eq!(value(&mut e, vec![param("2")]), "y");
        assert!(expand(&mut e, vec![param("3")]).is_empty());
    }

    #[test]
    fn at_and_star() {
        let mut e = env_with(&[]);
        e.set_positional(vec!["a b".into(), "c".into()]);
        assert_eq!(expand(&mut e, vec![param("@")]), vec!["a", "b", "c"]);
        assert_eq!(expand(&mut e, vec![param("*")]), vec!["a", "b", "c"]);
        assert_eq!(
            expand(&mut e, vec![Part::Double(vec![param("@")])]),
            vec!["a b", "c"]
        );
        assert_eq!(value(&mut e, vec![Part::Double(vec![param("*")])]), "a b c");
        e.set_positional(vec![]);
        assert!(expand(&mut e, vec![param("@")]).is_empty());
        assert_eq!(value(&mut e, vec![Part::Double(vec![param("*")])]), "");
        e.set_positional(vec!["p".into()]);
        assert_eq!(
            expand(
                &mut e,
                vec![raw("a"), Part::Double(vec![param("@")]), raw("b")]
            ),
            vec!["apb"]
        );
        e.set_positional(vec!["p".into(), "q".into()]);
        assert_eq!(
            expand(
                &mut e,
                vec![raw("a"), Part::Double(vec![param("@")]), raw("b")]
            ),
            vec!["ap", "qb"]
        );
    }

    #[test]
    fn star_in_ifs_colon() {
        let mut e = env_with(&[("IFS", ":")]);
        e.set_positional(vec!["a".into(), "b".into()]);
        assert_eq!(
            expand(&mut e, vec![Part::Double(vec![param("*")])]),
            vec!["a:b"]
        );
    }

    #[test]
    fn tilde() {
        let mut e = env_with(&[("HOME", "/home/u"), ("PWD", "/work"), ("OLDPWD", "/old")]);
        assert_eq!(expand(&mut e, vec![raw("~")]), vec!["/home/u"]);
        assert_eq!(expand(&mut e, vec![raw("~/x")]), vec!["/home/u/x"]);
        assert_eq!(expand(&mut e, vec![raw("~+")]), vec!["/work"]);
        assert_eq!(expand(&mut e, vec![raw("~-")]), vec!["/old"]);
        assert_eq!(expand(&mut e, vec![Part::Single("~".into())]), vec!["~"]);
        let mut e2 = env_with(&[]);
        assert_eq!(expand(&mut e2, vec![raw("~")]), vec!["~"]);
        assert_eq!(expand(&mut e, vec![raw("~root/x")]), vec!["~root/x"]);
    }

    #[test]
    fn globbing() {
        let mut e = env_with(&[]);
        // meta + match: expanded
        assert_eq!(expand(&mut e, vec![raw("/dev/n?ll")]), vec!["/dev/null"]);
        // meta + no match: literal kept (bash default)
        assert_eq!(
            expand(&mut e, vec![raw("*.nomatch-here")]),
            vec!["*.nomatch-here"]
        );
        // quoted meta: no glob
        assert_eq!(
            expand(&mut e, vec![Part::Single("*.rs".into())]),
            vec!["*.rs"]
        );
        // no meta: never globbed, even when missing
        assert_eq!(
            expand(&mut e, vec![raw("/definitely/not/a/file")]),
            vec!["/definitely/not/a/file"]
        );
    }

    #[test]
    fn cmdsubst_trailing_newlines_stripped() {
        let mut e = env_with(&[]);
        let mut cs = |_src: &str| -> Result<String, Error> { Ok("out\n\n".into()) };
        let w = one_word(vec![Part::Subst("echo hi".into())]);
        assert_eq!(expand_word(&mut e, &w, &mut cs).unwrap(), vec!["out"]);
    }

    #[test]
    fn cmdsubst_value_spliced() {
        let mut e = env_with(&[]);
        let mut cs = |src: &str| -> Result<String, Error> { Ok(format!("X{src}X")) };
        let w = one_word(vec![raw("a"), Part::Subst("cmd".into()), raw("b")]);
        assert_eq!(expand_word(&mut e, &w, &mut cs).unwrap(), vec!["aXcmdXb"]);
    }

    #[test]
    fn arith_expansion() {
        let mut e = env_with(&[("n", "10")]);
        let w = one_word(vec![Part::Arith("n * 2 + 1".into())]);
        assert_eq!(
            expand(&mut e, vec![Part::Arith("n * 2 + 1".into())]),
            vec!["21"]
        );
        assert_eq!(
            expand(&mut e, vec![Part::Arith("2 + 3 * 4".into())]),
            vec!["14"]
        );
        assert!(
            value_err(&mut e, vec![Part::Arith("1/0".into())]) || {
                let w2 = one_word(vec![Part::Arith("1/0".into())]);
                let mut cs = |_s: &str| -> Result<String, Error> { Ok(String::new()) };
                expand_word(&mut e, &w2, &mut cs).is_err()
            }
        );
        assert_eq!(
            expand(&mut e, vec![Part::Arith("n = n + 1".into())]),
            vec!["11"]
        );
        assert_eq!(e.get("n"), Some("11"));
        let _ = w;
    }

    #[test]
    fn esc_is_quoted() {
        let mut e = env_with(&[]);
        assert_eq!(expand(&mut e, vec![Part::Esc('*')]), vec!["*"]);
        assert_eq!(expand(&mut e, vec![Part::Esc('$')]), vec!["$"]);
    }

    #[test]
    fn assign_value_no_split_no_glob() {
        let mut e = env_with(&[("HOME", "/h")]);
        let w = one_word(vec![raw("FOO=~/x")]);
        assert_eq!(
            expand_assign_value(&mut e, &w, &mut |_s: &str| -> Result<String, Error> {
                Ok(String::new())
            })
            .unwrap(),
            "FOO=/h/x"
        );
        let w = one_word(vec![raw("FOO=*")]);
        assert_eq!(
            expand_assign_value(&mut e, &w, &mut |_s: &str| -> Result<String, Error> {
                Ok(String::new())
            })
            .unwrap(),
            "FOO=*"
        );
        let w = one_word(vec![raw("FOO="), Part::Double(vec![raw("a b")])]);
        assert_eq!(
            expand_assign_value(&mut e, &w, &mut |_s: &str| -> Result<String, Error> {
                Ok(String::new())
            })
            .unwrap(),
            "FOO=a b"
        );
    }

    #[test]
    fn split_assign_word() {
        let w = one_word(vec![raw("FOO=bar")]);
        assert!(is_assign_word(&w));
        let (n, v) = split_assign(&w).unwrap();
        assert_eq!(n, "FOO");
        let mut cs = |_s: &str| -> Result<String, Error> { Ok(String::new()) };
        let mut e = env_with(&[]);
        assert_eq!(expand_value(&mut e, &v, &mut cs).unwrap(), "bar");

        let w2 = one_word(vec![raw("FOO="), Part::Param("X".into())]);
        let (n2, v2) = split_assign(&w2).unwrap();
        assert_eq!(n2, "FOO");
        assert_eq!(v2.parts.len(), 1);

        let w3 = one_word(vec![raw("cmd")]);
        assert!(!is_assign_word(&w3));
        assert!(split_assign(&w3).is_none());
    }

    #[test]
    fn expand_text_raw() {
        let mut e = env_with(&[("X", "1")]);
        let mut cs = |_s: &str| -> Result<String, Error> { Ok(String::new()) };
        let out = expand_text(&mut e, "a $X b\nc\n", &mut cs).unwrap();
        assert_eq!(out, "a 1 b\nc\n");
        let out = expand_text(&mut e, "cost: $", &mut cs).unwrap();
        assert_eq!(out, "cost: $");
        let out = expand_text(&mut e, "bad \'quote", &mut cs).unwrap();
        assert_eq!(out, "bad \'quote");
    }

    #[test]
    fn bad_substitution_errors() {
        let mut e = env_with(&[]);
        for bad in ["", "?bad", "#:", "x!!y"] {
            assert!(
                value_err(&mut e, vec![param(bad)]),
                "want error for {bad:?}"
            );
        }
    }
}
