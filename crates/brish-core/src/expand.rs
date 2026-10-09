//! POSIX word expansion in required order: tilde → parameter / command
//! substitution / arithmetic → field splitting → pathname globbing →
//! quote removal.
//!
//! Bash extras: brace expansion runs first (before everything above,
//! on unquoted parts only); `~user` tilde lookup hits the platform
//! user database.

use crate::env::Env;
use crate::error::Error;
use crate::lexer::{self, Part, Word};
use brish_words::pattern::trim;
use brish_words::{eval_arith, expand as glob_expand, fields as split_fields};

/// Command-substitution runner: executes `$(...)` source, returns stdout.
pub type CmdSubst<'a> = &'a mut dyn FnMut(&str) -> Result<String, Error>;

/// Process-substitution runner: forks `body` asynchronously wired to a
/// pipe and returns the path naming that pipe's end (`/dev/fd/N`).
/// `out` selects the `>(...)` form — the shell writes, the body reads.
pub type ProcSubst<'a> = &'a mut dyn FnMut(&str, bool) -> Result<String, Error>;

/// Recursion cap for nested substitutions / default words.
const MAX_DEPTH: usize = 64;

/// Evaluate an array subscript expression as arithmetic. Exposed so the
/// engine can resolve `a[i]=v` targets with the same rules the expander
/// uses for `${a[i]}` (including `$var` splicing).
pub fn eval_subscript(src: &str, env: &mut Env) -> Result<usize, Error> {
    let mut ex = Ex {
        env,
        cs: &mut |_s: &str| -> Result<String, Error> { Ok(String::new()) },
        ps: &mut |s: &str, _out: bool| Ok(format!("/dev/fd/{s}")),
        depth: 0,
    };
    ex.array_index(src)
}

/// Expand one word into fields (splitting, globbing, quote removal).
pub fn expand_word(
    env: &mut Env,
    w: &Word,
    cs: CmdSubst,
    ps: ProcSubst,
) -> Result<Vec<String>, Error> {
    Ex {
        env,
        cs,
        ps,
        depth: 0,
    }
    .expand_word(w)
}

/// Expand words, concatenating all resulting fields in order.
pub fn expand_words(
    env: &mut Env,
    ws: &[Word],
    cs: CmdSubst,
    ps: ProcSubst,
) -> Result<Vec<String>, Error> {
    let mut out = Vec::new();
    for w in ws {
        out.append(&mut expand_word(env, w, cs, ps)?);
    }
    Ok(out)
}

/// Expand a word to one string: no field splitting, no globbing
/// (assignment values, here-doc bodies, patterns).
pub fn expand_value(env: &mut Env, w: &Word, cs: CmdSubst, ps: ProcSubst) -> Result<String, Error> {
    Ex {
        env,
        cs,
        ps,
        depth: 0,
    }
    .value_of_word(w)
}

/// Expand raw text (here-doc body): substitutions only; lexing failure
/// falls back to the text unchanged (unterminated quote stays literal).
pub fn expand_text(env: &mut Env, src: &str, cs: CmdSubst, ps: ProcSubst) -> Result<String, Error> {
    let lexed = match lexer::lex(src) {
        Ok(l) => l,
        Err(_) => return Ok(src.to_string()),
    };
    let mut ex = Ex {
        env,
        cs,
        ps,
        depth: 0,
    };
    let mut out = String::new();
    let mut prev_end = 0;
    for t in &lexed.tokens {
        // Raw gap between tokens: whitespace, comments, operators.
        out.push_str(&src[prev_end..t.span.start]);
        prev_end = t.span.end;
        if let lexer::Tok::Word(w) = &t.tok {
            // no brace expansion in here-doc bodies (bash)
            out.push_str(&ex.value_of_parts(&w.parts, false)?);
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

// ---- brace expansion (bash, unquoted parts only) ----

/// One rendered character of a `Raw`/`Esc` run.
struct BraceCh {
    c: char,
    /// `true` for `Raw` text; `false` for `Esc` chars (escaped braces
    /// and commas never act as separators).
    active: bool,
    part: usize,
    /// Byte offset of `c` inside `parts[part]` (`usize::MAX` for Esc).
    off: usize,
}

/// Bash brace expansion: `{a,b}`, `{m..n}`, `{m..n..step}` (integers
/// with optional zero-padding, `{a..z}` letters), nested, applied to
/// unquoted `Raw`/`Esc` runs only — a quoted or `$` part inside the
/// braces suppresses expansion (`{$x,y}` stays literal — ponytail
/// ceiling; extend by scanning across parts if anyone ever needs it).
/// Returns every variant with all levels resolved, or a single copy
/// of `parts` when nothing expands.
pub fn brace_expand(parts: &[Part]) -> Vec<Vec<Part>> {
    match brace_once(parts) {
        None => vec![parts.to_vec()],
        Some(vs) => vs.into_iter().flat_map(|v| brace_expand(&v)).collect(),
    }
}

fn brace_chs(parts: &[Part], run_start: usize, run_end: usize) -> Vec<BraceCh> {
    let mut chs = Vec::new();
    for (pi, part) in parts.iter().enumerate().take(run_end).skip(run_start) {
        match part {
            Part::Raw(t) => {
                for (off, c) in t.char_indices() {
                    chs.push(BraceCh {
                        c,
                        active: true,
                        part: pi,
                        off,
                    });
                }
            }
            Part::Esc(c) => chs.push(BraceCh {
                c: *c,
                active: false,
                part: pi,
                off: usize::MAX,
            }),
            _ => {}
        }
    }
    chs
}

/// Parts covering `ch[a..b]` (boundaries always fall on active chars,
/// so `Esc` parts are never split).
fn brace_region(parts: &[Part], chs: &[BraceCh], a: usize, b: usize) -> Vec<Part> {
    let mut out = Vec::new();
    let mut k = a;
    while k < b {
        let pi = chs[k].part;
        let mut min_off = usize::MAX;
        let mut max_end = 0usize;
        let mut count = 0usize;
        while k < b && chs[k].part == pi {
            if chs[k].active {
                min_off = min_off.min(chs[k].off);
                max_end = max_end.max(chs[k].off + chs[k].c.len_utf8());
            }
            count += 1;
            k += 1;
        }
        match &parts[pi] {
            Part::Raw(t) => {
                let whole = min_off == 0 && max_end == t.len() && count == t.chars().count();
                if whole {
                    out.push(parts[pi].clone());
                } else {
                    out.push(Part::Raw(t[min_off..max_end].to_string()));
                }
            }
            other => out.push(other.clone()),
        }
    }
    out
}

/// One pass: find the first expandable brace pair and split it.
fn brace_once(parts: &[Part]) -> Option<Vec<Vec<Part>>> {
    let mut i = 0;
    while i < parts.len() {
        if !matches!(parts[i], Part::Raw(_)) {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < parts.len() && matches!(parts[j], Part::Raw(_) | Part::Esc(_)) {
            j += 1;
        }
        if let Some(res) = brace_scan(parts, i, j) {
            return Some(res);
        }
        i = j;
    }
    None
}

fn brace_scan(parts: &[Part], run_start: usize, run_end: usize) -> Option<Vec<Vec<Part>>> {
    let chs = brace_chs(parts, run_start, run_end);
    let mut cursor = 0;
    while cursor < chs.len() {
        // next active `{`
        let o = chs[cursor..]
            .iter()
            .position(|ch| ch.active && ch.c == '{')
            .map(|p| p + cursor)?;
        let mut depth: i32 = 0;
        let mut close: Option<usize> = None;
        let mut seps: Vec<usize> = Vec::new();
        for (p, ch) in chs.iter().enumerate().skip(o) {
            if !ch.active {
                continue;
            }
            match ch.c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(p);
                        break;
                    }
                }
                ',' if depth == 1 => seps.push(p),
                _ => {}
            }
        }
        let Some(c) = close else {
            cursor = o + 1;
            continue;
        };
        let variant = |inside: Vec<Part>| {
            let mut out = brace_region(parts, &chs, 0, o);
            out.extend(inside);
            out.extend(brace_region(parts, &chs, c + 1, chs.len()));
            let mut full = parts[..run_start].to_vec();
            full.extend(out);
            full.extend_from_slice(&parts[run_end..]);
            if full.is_empty() {
                full.push(Part::Raw(String::new()));
            }
            full
        };
        if !seps.is_empty() {
            let mut variants = Vec::new();
            let mut start = o + 1;
            for &sep in &seps {
                variants.push(variant(brace_region(parts, &chs, start, sep)));
                start = sep + 1;
            }
            variants.push(variant(brace_region(parts, &chs, start, c)));
            return Some(variants);
        }
        // Range form: fully literal inside, `a..b` or `a..b..step`.
        let inside = brace_region(parts, &chs, o + 1, c);
        let literal = inside.iter().all(|p| matches!(p, Part::Raw(_)));
        if literal {
            let text: String = inside
                .iter()
                .map(|p| match p {
                    Part::Raw(t) => t.as_str(),
                    _ => "",
                })
                .collect();
            if let Some(items) = brace_range(&text) {
                return Some(
                    items
                        .into_iter()
                        .map(|item| variant(vec![Part::Raw(item)]))
                        .collect(),
                );
            }
        }
        cursor = o + 1; // literal pair: look for an inner expandable one
    }
    None
}

/// `{m..n}` / `{m..n..step}` / `{a..z..step}` → items, or `None`.
fn brace_range(text: &str) -> Option<Vec<String>> {
    // `a..b` or `a..b..step` (no further `..`)
    let (a, rest) = text.split_once("..")?;
    let (b, step_txt) = match rest.split_once("..") {
        Some((b, s)) if !s.contains("..") => (b, Some(s)),
        Some(_) => return None,
        None => (rest, None),
    };
    let step: i64 = match step_txt {
        Some(s) => s.parse().ok()?,
        None => 1,
    };
    if step == 0 {
        return None;
    }
    // integers
    if let (Ok(x), Ok(y)) = (a.parse::<i64>(), b.parse::<i64>()) {
        let mut step = step;
        if (y - x).signum() != 0 && (y - x).signum() != step.signum() {
            step = -step;
        }
        let pad = (a.starts_with('0') && a.len() > 1 || b.starts_with('0') && b.len() > 1)
            && !a.starts_with('-')
            && !b.starts_with('-');
        let width = a.chars().count().max(b.chars().count());
        let mut out = Vec::new();
        let mut v = x;
        loop {
            if (step > 0 && v > y) || (step < 0 && v < y) {
                break;
            }
            let item = if pad {
                format!("{v:0width$}")
            } else {
                v.to_string()
            };
            out.push(item);
            if out.len() > 4096 {
                return None; // ponytail: refuse runaway ranges
            }
            v += step;
        }
        if out.is_empty() {
            return None;
        }
        return Some(out);
    }
    // single letters, same case
    let ca: Vec<char> = a.chars().collect();
    let cb: Vec<char> = b.chars().collect();
    if ca.len() == 1 && cb.len() == 1 && ca[0].is_ascii_alphabetic() && cb[0].is_ascii_alphabetic()
    {
        let (x, y) = (ca[0] as i64, cb[0] as i64);
        let mut step = step;
        if (y - x).signum() != 0 && (y - x).signum() != step.signum() {
            step = -step;
        }
        let mut out = Vec::new();
        let mut v = x;
        loop {
            if (step > 0 && v > y) || (step < 0 && v < y) {
                break;
            }
            out.push(char::from_u32(v as u32)?.to_string());
            if out.len() > 4096 {
                return None;
            }
            v += step;
        }
        if out.is_empty() {
            return None;
        }
        return Some(out);
    }
    None
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
    ps: ProcSubst<'a>,
    depth: usize,
}

impl<'a> Ex<'a> {
    fn expand_word(&mut self, w: &Word) -> Result<Vec<String>, Error> {
        let variants = brace_expand(&w.parts);
        // Unset params expand to zero fields; only an actual brace
        // split contributes an empty field (`{,a}` → "" "a").
        let braced = variants.len() > 1 || variants[0] != w.parts;
        let mut out = Vec::new();
        for parts in variants {
            let segs = self.segs_of_parts(&parts)?;
            let fields = self.assemble(segs, !self.env.opts.noglob)?;
            if fields.is_empty() && braced {
                out.push(String::new());
            } else {
                out.extend(fields);
            }
        }
        Ok(out)
    }

    fn value_of_word(&mut self, w: &Word) -> Result<String, Error> {
        self.value_of_parts(&w.parts, true)
    }

    /// `braces: false` for heredoc bodies (bash does not brace-expand
    /// here-doc content).
    fn value_of_parts(&mut self, parts: &[Part], braces: bool) -> Result<String, Error> {
        let variants = if braces {
            brace_expand(parts)
        } else {
            vec![parts.to_vec()]
        };
        let mut outs = Vec::with_capacity(variants.len());
        for parts in variants {
            let segs = self.segs_of_parts(&parts)?;
            let mut s = String::new();
            for seg in segs {
                match seg {
                    Seg::T { text, .. } => s.push_str(&text),
                    Seg::F { items, .. } => s.push_str(&items.join(" ")),
                }
            }
            outs.push(s);
        }
        Ok(outs.join(" "))
    }

    fn segs_of_parts(&mut self, parts: &[Part]) -> Result<Vec<Seg>, Error> {
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
        let (name, tail) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        match name {
            "+" => {
                let pwd = self.env.get("PWD")?;
                let mut s = pwd.to_string();
                s.push_str(tail);
                Some(s)
            }
            "-" => {
                let old = self.env.get("OLDPWD")?;
                let mut s = old.to_string();
                s.push_str(tail);
                Some(s)
            }
            // `~user`: unknown users stay literal (bash behavior).
            _ if crate::env::is_name(name) => {
                let mut s = brish_platform::user::user_home(name)?;
                s.push_str(tail);
                Some(s)
            }
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
                Part::ProcSubst { out: is_out, body } => {
                    let v = self.procsubst(body, *is_out)?;
                    // Always quoted: the result is a literal `/dev/fd/N`
                    // path, never subject to field splitting or globbing.
                    out.push(Seg::T {
                        text: v,
                        quoted: true,
                        glob: false,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Fork `body` onto a pipe and name the near end. Recursion-capped
    /// like [`Ex::cmdsubst`] — `<(cat <(cat <(…)))` nests.
    fn procsubst(&mut self, body: &str, out: bool) -> Result<String, Error> {
        if self.depth >= MAX_DEPTH {
            return Err(Error::expand("process substitution recursion too deep"));
        }
        self.depth += 1;
        let r = (self.ps)(body, out);
        self.depth -= 1;
        r
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
        let expanded = self.expand_arith_params(src);
        let v = eval_arith(&expanded, &mut *self.env)
            .map_err(|e| Error::expand(format!("arithmetic: {e}")))?;
        Ok(v.to_string())
    }

    /// Inline `$`-parameter references before the arithmetic parser
    /// (which rejects `$` outright). Bare names already work because the
    /// parser resolves them via `ArithEnv`; `$1`, `$n`, `${n}` do not, so
    /// their values are spliced in. Unset/empty → `0`, matching the
    /// bare-name path through `ArithEnv::get` (which the parser treats
    /// as 0 for unset).
    fn expand_arith_params(&mut self, src: &str) -> String {
        let b = src.as_bytes();
        let mut out = String::with_capacity(src.len());
        let mut i = 0;
        while i < b.len() {
            if b[i] == b'$' && i + 1 < b.len() {
                if b[i + 1] == b'{' {
                    if let Some(rel) = src[i + 2..].find('}') {
                        let inner = &src[i + 2..i + 2 + rel];
                        out.push_str(&self.arith_param(inner));
                        i += 3 + rel;
                        continue;
                    }
                } else {
                    let j = i + 1;
                    let (len, name): (usize, &str) = if b[j].is_ascii_digit() {
                        let mut k = j;
                        while k < b.len() && b[k].is_ascii_digit() {
                            k += 1;
                        }
                        (k - j, &src[j..k])
                    } else if b[j].is_ascii_alphabetic() || b[j] == b'_' {
                        let mut k = j;
                        while k < b.len() && (b[k].is_ascii_alphanumeric() || b[k] == b'_') {
                            k += 1;
                        }
                        (k - j, &src[j..k])
                    } else {
                        (1, &src[j..j + 1])
                    };
                    out.push_str(&self.arith_param(name));
                    i = j + len;
                    continue;
                }
            }
            out.push(b[i] as char);
            i += 1;
        }
        out
    }

    fn arith_param(&self, name: &str) -> String {
        match self.param_value(name) {
            Some(v) if !v.trim().is_empty() => v,
            _ => "0".to_string(),
        }
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
            Form::Index { name, sub, op } => self.expand_index(name, sub, op, quoted),
            Form::LenIndex { name, sub } => {
                let text = match parse_subscript(sub)? {
                    // `${#a[@]}` / `${#a[*]}`: element count.
                    Sub::All => self.env.array(name).map_or(0, <[String]>::len).to_string(),
                    Sub::Index(i) => {
                        let idx = self.array_index(i)?;
                        match self.env.array(name).and_then(|a| a.get(idx)) {
                            Some(v) => v.chars().count().to_string(),
                            None => "0".to_string(),
                        }
                    }
                };
                Ok(vec![Seg::T {
                    text,
                    quoted,
                    glob: false,
                }])
            }
        }
    }

    /// `${a[i]}` and `${a[@]}`. `@`/`*` behave exactly like `$@`: quoted
    /// they stay one field per element, unquoted they word-split.
    ///
    /// A non-array `name` falls back to its scalar value, so `${x[0]}` on
    /// a plain variable is `${x}` rather than an error (bash warns; an
    /// empty result is closer to the POSIX "unset is empty" rule).
    fn expand_index(
        &mut self,
        name: &str,
        sub: &str,
        op: Option<(&str, &str)>,
        quoted: bool,
    ) -> Result<Vec<Seg>, Error> {
        // Cloned because `array_index` may need `&mut env` below.
        let items = self.env.array(name).map(<[String]>::to_vec);
        let Some(items) = items else {
            let v = match self.param_value(name) {
                Some(v) => v,
                None if self.env.opts.nounset && !nounset_exempt(name) => {
                    return Err(Error::expand(format!("{name}: unbound variable")));
                }
                None => String::new(),
            };
            // `${x[0]:-d}` still applies the operator to the scalar.
            let v = match op {
                Some((o, w)) => self.op_on(name, Some(v), o, w)?,
                None => v,
            };
            return Ok(vec![Seg::T {
                glob: !quoted && has_meta(&v),
                text: v,
                quoted,
            }]);
        };
        match parse_subscript(sub)? {
            Sub::All => {
                if items.is_empty() {
                    return Ok(Vec::new());
                }
                Ok(vec![Seg::F { items, quoted }])
            }
            Sub::Index(i) => {
                let idx = self.array_index(i)?;
                // An out-of-range element is empty, and `${a[9]:-d}` must
                // therefore take the default.
                let current = items.get(idx).cloned();
                let v = match op {
                    Some((o, w)) => self.op_on(name, current, o, w)?,
                    None => current.unwrap_or_default(),
                };
                Ok(vec![Seg::T {
                    glob: !quoted && has_meta(&v),
                    text: v,
                    quoted,
                }])
            }
        }
    }

    /// Array subscript arithmetic. An unset/non-numeric subscript is 0,
    /// which makes `${a[x]}` degrade to element 0 instead of failing.
    fn array_index(&mut self, sub: &str) -> Result<usize, Error> {
        let expanded = self.expand_arith_params(sub);
        let v = eval_arith(&expanded, &mut *self.env)
            .map_err(|e| Error::expand(format!("bad array subscript: {e}")))?;
        Ok(v.max(0) as usize)
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
        self.op_on(name, current, op, word)
    }

    /// The `:-`/`:=`/`##`/… machinery, over an already-resolved value.
    /// `${a[i]:-x}` passes the element rather than re-reading the variable.
    fn op_on(
        &mut self,
        name: &str,
        current: Option<String>,
        op: &str,
        word: &str,
    ) -> Result<String, Error> {
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
        // A quoted segment (even empty `""`) must yield a field.
        let mut quoted_seen = false;

        for seg in segs {
            match seg {
                Seg::T { text, quoted, glob } => {
                    if quoted {
                        cur.push_str(&text);
                        produced = true;
                        quoted_seen = true;
                    } else if !text.is_empty() {
                        produced = true;
                        if glob {
                            cur_glob = true;
                        }
                        // IFS whitespace at the expansion's edges is a
                        // field delimiter even against adjacent literals
                        // (POSIX 2.6): `[$x]` with x='  a  ' →
                        // fields `[`, `a`, `]` — echo joins them
                        // `[ a ]`. Without this, `[`+split+`]` glues.
                        let ifs_ws = |c: char| matches!(c, ' ' | '\t' | '\n') && ifs.contains(c);
                        let lead = text.starts_with(ifs_ws);
                        let trail = text.ends_with(ifs_ws);
                        if lead && !cur.is_empty() {
                            self.flush_field(&mut fields, &mut cur, &mut cur_glob, do_glob)?;
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
                        if trail && !cur.is_empty() {
                            self.flush_field(&mut fields, &mut cur, &mut cur_glob, do_glob)?;
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
        // Skip the empty tail left by a trail-delimiter flush (`$x`
        // with trailing IFS ws already pushed its field); quoted
        // segments still force their field (`set -- ""` → $# = 1).
        if produced && (!cur.is_empty() || quoted_seen) {
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
            match glob_expand(&text, true, self.env.opts.globstar) {
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
pub fn expand_assign_value(
    env: &mut Env,
    w: &Word,
    cs: CmdSubst,
    ps: ProcSubst,
) -> Result<String, Error> {
    let mut ex = Ex {
        env,
        cs,
        ps,
        depth: 0,
    };
    // Brace expansion applies to assignment values (bash: x={a,b} -> "a b").
    let mut outs = Vec::new();
    for variant in brace_expand(&w.parts) {
        let mut out = String::new();
        for (i, p) in variant.iter().enumerate() {
            match p {
                Part::Raw(t) if i == 0 => {
                    // Full form `NAME=value`: emit `NAME=` then the value.
                    // Value-only form (parser already split the name):
                    // same handling, no prefix.
                    let (prefix, rest) = match t.find('=') {
                        Some(eq) => (&t[..=eq], &t[eq + 1..]),
                        None => ("", t.as_str()),
                    };
                    out.push_str(prefix);
                    // Tilde at value start (~/bin, ~root/bin); quoted parts
                    // are not Raw so they never hit this branch.
                    if let Some(r) = rest.strip_prefix('~')
                        && let Some(h) = ex.tilde(r)
                    {
                        out.push_str(&h);
                    } else {
                        out.push_str(rest);
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
        outs.push(out);
    }
    Ok(outs.join(" "))
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

/// Glob metacharacters present. A `[` only counts when a `]` follows it:
/// an unterminated bracket expression cannot match anything, so it is
/// literal. Without this, every `[ ... ]` test command globs the command
/// word `[` and read_dir's the cwd — 6 syscalls per loop iteration.
fn has_meta(s: &str) -> bool {
    if s.contains('*') || s.contains('?') {
        return true;
    }
    match s.find('[') {
        Some(i) => s[i + 1..].contains(']'),
        None => false,
    }
}

enum Form<'a> {
    Plain(&'a str),
    Len(&'a str),
    Op {
        name: &'a str,
        op: &'a str,
        word: &'a str,
    },
    /// `${a[i]}` — one element, optionally followed by an operator
    /// (`${a[i]:-default}`).
    Index {
        name: &'a str,
        sub: &'a str,
        op: Option<(&'a str, &'a str)>,
    },
    /// `${#a[i]}` — length of the array, or of one element.
    LenIndex {
        name: &'a str,
        sub: &'a str,
    },
}

/// Split a `[…]` subscript off the front of `rest`. Returns the inside,
/// the text that followed the closing `]`, and whether a complete `[…]`
/// was present. `after` is `None` when there is no `[` at all; it is
/// `Some("")` for a subscript with nothing trailing.
fn split_subscript(rest: &str) -> Option<(&str, Option<&str>)> {
    let inner = rest.strip_prefix('[')?;
    let close = inner.find(']')?;
    Some((&inner[..close], Some(&inner[close + 1..])))
}

/// Subscript `@`/`*` = all elements; anything else is an arithmetic index.
enum Sub<'a> {
    All,
    Index(&'a str),
}

fn parse_subscript(sub: &str) -> Result<Sub<'_>, Error> {
    match sub {
        "@" | "*" => Ok(Sub::All),
        other => Ok(Sub::Index(other)),
    }
}

fn parse_form(expr: &str) -> Result<Form<'_>, Error> {
    if expr.is_empty() {
        return Err(Error::expand("bad substitution"));
    }
    if let Some(rest) = expr.strip_prefix('#') {
        if rest.is_empty() {
            return Ok(Form::Plain("#"));
        }
        // `${#a[@]}` / `${#a[2]}` — array length, not a `#` operator.
        // `rest` is the whole `a[@]`; split it into name and subscript.
        if let Some(bracket) = rest.find('[')
            && rest.ends_with(']')
            && crate::env::is_name(&rest[..bracket])
        {
            let sub = &rest[bracket + 1..rest.len() - 1];
            if !sub.is_empty() {
                return Ok(Form::LenIndex {
                    name: &rest[..bracket],
                    sub,
                });
            }
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
    // `${a[0]}` / `${a[@]}` — a subscript is not an operator, so it is
    // peeled off before the `:-`/`##` scan below. Anything after the
    // closing `]` is the operator.
    if let Some((sub, Some(after))) = split_subscript(rest) {
        let op = parse_trailing_op(after)?;
        if after.is_empty() {
            return Ok(Form::Index {
                name,
                sub,
                op: None,
            });
        }
        if let Some((o, w)) = op {
            return Ok(Form::Index {
                name,
                sub,
                op: Some((o, w)),
            });
        }
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

/// Recognize a trailing `:-`-style operator after a subscript. `None` for
/// empty or unrecognized text; the caller falls through to the normal
/// bad-substitution error.
fn parse_trailing_op(after: &str) -> Result<Option<(&str, &str)>, Error> {
    if after.is_empty() {
        return Ok(None);
    }
    for op in [
        ":-", ":=", ":?", ":+", "##", "%%", "-", "=", "?", "+", "#", "%",
    ] {
        if let Some(word) = after.strip_prefix(op) {
            return Ok(Some((op, word)));
        }
    }
    Ok(None)
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

    /// Process-substitution stub for tests: returns a fixed fake path
    /// (no fork), asserting the requested direction was threaded through.
    fn ps_stub(body: &str, out: bool) -> Result<String, Error> {
        Ok(format!("/dev/fd/stub:{out}:{body}"))
    }

    fn expand(env: &mut Env, parts: Vec<Part>) -> Vec<String> {
        let w = one_word(parts);
        let mut cs = |_src: &str| -> Result<String, Error> { Ok(String::new()) };
        let mut ps = |src: &str, out: bool| ps_stub(src, out);
        expand_word(env, &w, &mut cs, &mut ps).unwrap()
    }

    fn value(env: &mut Env, parts: Vec<Part>) -> String {
        let w = one_word(parts);
        let mut cs = |_src: &str| -> Result<String, Error> { Ok(String::new()) };
        let mut ps = |src: &str, out: bool| ps_stub(src, out);
        expand_value(env, &w, &mut cs, &mut ps).unwrap()
    }

    fn value_err(env: &mut Env, parts: Vec<Part>) -> bool {
        let w = one_word(parts);
        let mut cs = |_src: &str| -> Result<String, Error> { Ok(String::new()) };
        let mut ps = |src: &str, out: bool| ps_stub(src, out);
        expand_value(env, &w, &mut cs, &mut ps).is_err()
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
        // `~user` hits the platform user database (`root` always exists)
        let got = expand(&mut e, vec![raw("~root/x")]);
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].starts_with('/') && !got[0].contains('~'), "{got:?}");
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
    fn unterminated_bracket_is_literal_not_meta() {
        // `[` alone is the `test` builtin name, not a bracket expression:
        // it must not be globbed (that read_dir'd the cwd every `[` test).
        assert!(!has_meta("["));
        assert!(!has_meta("[a"));
        assert!(!has_meta("foo[bar"));
        // A real bracket expression still globs.
        assert!(has_meta("[abc]"));
        assert!(has_meta("[a-c]*"));
        // ...even when the `[` is not the first character.
        assert!(has_meta("x[abc]"));
        assert!(has_meta("a*"));
        assert!(has_meta("a?"));
        assert!(!has_meta(""));
        // And the `[` command word survives expansion unchanged.
        let mut e = env_with(&[]);
        assert_eq!(expand(&mut e, vec![raw("[")]), vec!["["]);
    }

    #[test]
    fn cmdsubst_trailing_newlines_stripped() {
        let mut e = env_with(&[]);
        let mut cs = |_src: &str| -> Result<String, Error> { Ok("out\n\n".into()) };
        let mut ps = |src: &str, out: bool| ps_stub(src, out);
        let w = one_word(vec![Part::Subst("echo hi".into())]);
        assert_eq!(
            expand_word(&mut e, &w, &mut cs, &mut ps).unwrap(),
            vec!["out"]
        );
    }

    #[test]
    fn cmdsubst_value_spliced() {
        let mut e = env_with(&[]);
        let mut cs = |src: &str| -> Result<String, Error> { Ok(format!("X{src}X")) };
        let mut ps = |src: &str, out: bool| ps_stub(src, out);
        let w = one_word(vec![raw("a"), Part::Subst("cmd".into()), raw("b")]);
        assert_eq!(
            expand_word(&mut e, &w, &mut cs, &mut ps).unwrap(),
            vec!["aXcmdXb"]
        );
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
                let mut ps = |src: &str, out: bool| ps_stub(src, out);
                expand_word(&mut e, &w2, &mut cs, &mut ps).is_err()
            }
        );
        assert_eq!(
            expand(&mut e, vec![Part::Arith("n = n + 1".into())]),
            vec!["11"]
        );
        assert_eq!(e.get("n"), Some("11"));
        e.set_positional(vec!["10".into(), "20".into()]);
        assert_eq!(
            expand(&mut e, vec![Part::Arith("$1 + $2".into())]),
            vec!["30"]
        );
        assert_eq!(
            expand(&mut e, vec![Part::Arith("${1} + 5".into())]),
            vec!["15"]
        );
        assert_eq!(
            expand(&mut e, vec![Part::Arith("$n * 2".into())]),
            vec!["22"]
        );
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
            expand_assign_value(
                &mut e,
                &w,
                &mut |_s: &str| -> Result<String, Error> { Ok(String::new()) },
                &mut |src: &str, out: bool| ps_stub(src, out),
            )
            .unwrap(),
            "FOO=/h/x"
        );
        let w = one_word(vec![raw("FOO=*")]);
        assert_eq!(
            expand_assign_value(
                &mut e,
                &w,
                &mut |_s: &str| -> Result<String, Error> { Ok(String::new()) },
                &mut |src: &str, out: bool| ps_stub(src, out),
            )
            .unwrap(),
            "FOO=*"
        );
        let w = one_word(vec![raw("FOO="), Part::Double(vec![raw("a b")])]);
        assert_eq!(
            expand_assign_value(
                &mut e,
                &w,
                &mut |_s: &str| -> Result<String, Error> { Ok(String::new()) },
                &mut |src: &str, out: bool| ps_stub(src, out),
            )
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
        let mut ps = |src: &str, out: bool| ps_stub(src, out);
        let mut e = env_with(&[]);
        assert_eq!(expand_value(&mut e, &v, &mut cs, &mut ps).unwrap(), "bar");

        let w2 = one_word(vec![raw("FOO="), Part::Param("X".into())]);
        let (n2, v2) = split_assign(&w2).unwrap();
        assert_eq!(n2, "FOO");
        assert_eq!(v2.parts.len(), 1);

        let w3 = one_word(vec![raw("cmd")]);
        assert!(!is_assign_word(&w3));
        assert!(split_assign(&w3).is_none());
    }

    fn assign_value(env: &mut Env, parts: Vec<Part>) -> String {
        let w = one_word(parts);
        let mut cs = |_s: &str| -> Result<String, Error> { Ok(String::new()) };
        let mut ps = |src: &str, out: bool| ps_stub(src, out);
        expand_assign_value(env, &w, &mut cs, &mut ps).unwrap()
    }

    #[test]
    fn brace_expansion() {
        let mut e = env_with(&[]);
        assert_eq!(expand(&mut e, vec![raw("{a,b}")]), vec!["a", "b"]);
        assert_eq!(expand(&mut e, vec![raw("x{a,b}y")]), vec!["xay", "xby"]);
        assert_eq!(expand(&mut e, vec![raw("{1..3}")]), vec!["1", "2", "3"]);
        assert_eq!(
            expand(&mut e, vec![raw("{01..03}")]),
            vec!["01", "02", "03"]
        );
        assert_eq!(expand(&mut e, vec![raw("{a..c}")]), vec!["a", "b", "c"]);
        assert_eq!(
            expand(&mut e, vec![raw("{5..1}")]),
            vec!["5", "4", "3", "2", "1"]
        );
        assert_eq!(expand(&mut e, vec![raw("{a,{b,c}}")]), vec!["a", "b", "c"]);
        // empty variant keeps one empty field
        assert_eq!(expand(&mut e, vec![raw("{,a}")]), vec!["", "a"]);
        // literal: no separator, unbalanced, quoted, escaped
        assert_eq!(expand(&mut e, vec![raw("{foo}")]), vec!["{foo}"]);
        assert_eq!(expand(&mut e, vec![raw("x{1,")]), vec!["x{1,"]);
        assert_eq!(
            expand(&mut e, vec![Part::Double(vec![raw("{a,b}")])]),
            vec!["{a,b}"]
        );
        assert_eq!(
            expand(&mut e, vec![Part::Esc('{'), raw("a,b"), Part::Esc('}')]),
            vec!["{a,b}"]
        );
        // quoted separator inside braces doesn't split
        assert_eq!(
            expand(&mut e, vec![raw("{a,\"b,c\"}")]),
            vec!["a", "\"b", "c\""]
        );
    }

    #[test]
    fn assign_value_tilde_and_braces() {
        let mut e = env_with(&[("HOME", "/h"), ("PWD", "/w"), ("OLDPWD", "/o")]);
        // value-only form (parser already split the name)
        assert_eq!(assign_value(&mut e, vec![raw("~/x")]), "/h/x");
        assert_eq!(assign_value(&mut e, vec![raw("~")]), "/h");
        assert_eq!(assign_value(&mut e, vec![raw("~+")]), "/w");
        // full NAME=value form still works
        assert_eq!(assign_value(&mut e, vec![raw("FOO=~/x")]), "FOO=/h/x");
        // braces in assignment values
        assert_eq!(assign_value(&mut e, vec![raw("{a,b}")]), "a b");
        assert_eq!(assign_value(&mut e, vec![raw("p{1..2}")]), "p1 p2");
        // quoted braces stay literal
        assert_eq!(
            assign_value(&mut e, vec![Part::Double(vec![raw("{a,b}")])]),
            "{a,b}"
        );
        // other parts still expand (no glob)
        assert_eq!(assign_value(&mut e, vec![raw("*")]), "*");
    }

    #[test]
    fn expand_text_raw() {
        let mut e = env_with(&[("X", "1")]);
        let mut cs = |_s: &str| -> Result<String, Error> { Ok(String::new()) };
        let mut ps = |src: &str, out: bool| ps_stub(src, out);
        let out = expand_text(&mut e, "a $X b\nc\n", &mut cs, &mut ps).unwrap();
        assert_eq!(out, "a 1 b\nc\n");
        let out = expand_text(&mut e, "cost: $", &mut cs, &mut ps).unwrap();
        assert_eq!(out, "cost: $");
        let out = expand_text(&mut e, "bad \'quote", &mut cs, &mut ps).unwrap();
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
