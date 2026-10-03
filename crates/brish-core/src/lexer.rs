//! Hand-written lexer: source text -> tokens with spans.
//!
//! Hand-written over a generator (plan §10): shell lexing is context-heavy
//! (quoting states, here-doc delimiter stacks, `$((`/`${` nesting) and
//! spans must be exact for diagnostics.

use crate::error::Error;

/// Byte offsets into the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

/// A shell word: sequence of parts with quoting semantics preserved.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub parts: Vec<Part>,
    pub span: Span,
}

/// One piece of a word.
#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    /// Unquoted literal text: subject to field splitting and globbing.
    Raw(String),
    /// Unquoted backslash escape: literal char, behaves as quoted.
    Esc(char),
    /// Single-quoted text: fully literal.
    Single(String),
    /// Double-quoted contents: expansions allowed, no splitting/globbing.
    Double(Vec<Part>),
    /// `$name` / `${...}` — inner text (name or full parameter expression).
    Param(String),
    /// `$(...)` or `` `...` `` — command substitution source.
    Subst(String),
    /// `$((...))` — arithmetic source.
    Arith(String),
}

/// Operator / metacharacter token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Pipe,      // |
    And,       // &&
    Or,        // ||
    Semi,      // ;
    Dsemi,     // ;;
    Amp,       // &
    LParen,    // (
    RParen,    // )
    Less,      // <
    Great,     // >
    DGreat,    // >>
    DLess,     // <<
    DLessDash, // <<-
    DLessLess, // <<<
    GtAmp,     // >&
    LtAmp,     // <&
    Clobber,   // >|
}

/// Token kinds.
#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Word(Word),
    IoNumber(i32),
    Op(Op),
    /// `((expr))` arithmetic command (lexer consumed both paren pairs).
    ArithCmd(String),
    Newline,
}

/// Token with span.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

/// Collected here-doc body.
#[derive(Debug, Clone, PartialEq)]
pub struct HeredocBody {
    pub text: String,
    /// Body undergoes parameter/command/arithmetic expansion
    /// (false when the delimiter was single-quoted).
    pub expand: bool,
}

/// Lexed program: flat token stream plus collected here-doc bodies
/// (in order of their `<<` occurrences).
#[derive(Debug, Clone, PartialEq)]
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub heredocs: Vec<HeredocBody>,
}

struct PendingHd {
    delim: String,
    expand: bool,
    strip_tabs: bool,
}

/// Lex full input. `Error::Incomplete` when input ends mid-construct
/// (unterminated quote, dangling backslash, open `$(`/`${`).
pub fn lex(src: &str) -> Result<Lexed, Error> {
    let out = Lexer::new(src).run()?;
    if crate::debug_on("lexer") {
        eprintln!("brish[lexer]: {} tokens", out.tokens.len());
    }
    Ok(out)
}

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
    tokens: Vec<Token>,
    pending: Vec<PendingHd>,
    heredocs: Vec<HeredocBody>,
    at_word_start: bool,
}

impl<'a> Lexer<'a> {
    fn new(src: &'a str) -> Self {
        Self {
            src,
            pos: 0,
            tokens: Vec::new(),
            pending: Vec::new(),
            heredocs: Vec::new(),
            at_word_start: true,
        }
    }

    fn run(mut self) -> Result<Lexed, Error> {
        while self.pos < self.src.len() {
            let c = self.cur()?;
            match c {
                ' ' | '\t' | '\r' => {
                    self.bump();
                    self.at_word_start = true;
                }
                '\n' => {
                    let nl = self.pos;
                    self.bump(); // consume newline before reading here-doc bodies
                    self.flush_heredocs()?;
                    self.push(Tok::Newline, nl, nl + 1);
                    self.at_word_start = true;
                }
                '#' if self.at_word_start => {
                    while self.pos < self.src.len() && self.cur()? != '\n' {
                        self.bump();
                    }
                }
                '\\' if self.peek_next()? == Some('\n') => {
                    self.bump();
                    self.bump();
                }
                '|' | '&' | ';' | '(' | ')' | '<' | '>' => self.lex_operator()?,
                _ => self.lex_word()?,
            }
        }
        if !self.pending.is_empty() {
            // Unterminated here-doc at EOF: body is the rest (POSIX lenient).
            self.flush_heredocs()?;
        }
        Ok(Lexed {
            tokens: self.tokens,
            heredocs: self.heredocs,
        })
    }

    fn cur(&self) -> Result<char, Error> {
        self.src[self.pos..].chars().next().ok_or(Error::Incomplete)
    }

    fn peek_next(&self) -> Result<Option<char>, Error> {
        let mut it = self.src[self.pos..].chars();
        it.next();
        Ok(it.next())
    }

    fn bump(&mut self) {
        if let Some(c) = self.src[self.pos..].chars().next() {
            self.pos += c.len_utf8();
        }
    }

    fn starts_with(&self, s: &str) -> bool {
        self.src[self.pos..].starts_with(s)
    }

    fn push(&mut self, tok: Tok, start: usize, end: usize) {
        self.tokens.push(Token {
            tok,
            span: Span { start, end },
        });
    }

    /// Collect here-doc bodies for the line just ended.
    fn flush_heredocs(&mut self) -> Result<(), Error> {
        for i in 0..self.pending.len() {
            let (delim, strip, expand) = {
                let p = &self.pending[i];
                (p.delim.clone(), p.strip_tabs, p.expand)
            };
            let text = self.read_heredoc_body(&delim, strip)?;
            self.heredocs.push(HeredocBody { text, expand });
        }
        self.pending.clear();
        Ok(())
    }

    fn read_heredoc_body(&mut self, delim: &str, strip_tabs: bool) -> Result<String, Error> {
        let mut body = String::new();
        loop {
            if self.pos >= self.src.len() {
                break; // EOF: unterminated here-doc, take what we have
            }
            // Read one line (without newline).
            let line_start = self.pos;
            let mut line_end = self.pos;
            while line_end < self.src.len() && !self.src[line_end..].starts_with('\n') {
                line_end += self.src[line_end..]
                    .chars()
                    .next()
                    .map_or(1, char::len_utf8);
            }
            let line = &self.src[line_start..line_end];
            let compare = if strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line
            };
            // Advance past line + newline.
            self.pos = line_end;
            if self.pos < self.src.len() {
                self.bump(); // consume '\n'
            }
            if compare == delim {
                return Ok(body);
            }
            if strip_tabs {
                body.push_str(line.trim_start_matches('\t'));
            } else {
                body.push_str(line);
            }
            body.push('\n');
        }
        Ok(body)
    }

    fn lex_operator(&mut self) -> Result<(), Error> {
        let start = self.pos;
        if self.starts_with("((") {
            // Arithmetic command: always `((expr))`, never two subshells
            // (matches bash: `((` at word start is the arith command).
            self.bump();
            self.bump();
            let content = self.scan_arith()?;
            self.push(Tok::ArithCmd(content), start, self.pos);
            self.at_word_start = true;
            return Ok(());
        }
        let op = if self.starts_with(";;") {
            self.bump();
            self.bump();
            Op::Dsemi
        } else if self.starts_with("&&") {
            self.bump();
            self.bump();
            Op::And
        } else if self.starts_with("||") {
            self.bump();
            self.bump();
            Op::Or
        } else if self.starts_with("<<<") {
            self.bump();
            self.bump();
            self.bump();
            Op::DLessLess
        } else if self.starts_with("<<-") {
            self.bump();
            self.bump();
            self.bump();
            Op::DLessDash
        } else if self.starts_with("<<") {
            self.bump();
            self.bump();
            Op::DLess
        } else if self.starts_with(">>") {
            self.bump();
            self.bump();
            Op::DGreat
        } else if self.starts_with(">&") {
            self.bump();
            self.bump();
            Op::GtAmp
        } else if self.starts_with("<&") {
            self.bump();
            self.bump();
            Op::LtAmp
        } else if self.starts_with(">|") {
            self.bump();
            self.bump();
            Op::Clobber
        } else {
            let c = self.cur()?;
            self.bump();
            match c {
                '|' => Op::Pipe,
                '&' => Op::Amp,
                ';' => Op::Semi,
                '(' => Op::LParen,
                ')' => Op::RParen,
                '<' => Op::Less,
                '>' => Op::Great,
                _ => return Err(Error::Parse("unexpected operator".into())),
            }
        };
        let end = self.pos;
        self.push(Tok::Op(op), start, end);
        self.at_word_start = true;
        Ok(())
    }

    /// IO number: digits directly followed by `<` or `>` at word start
    /// (e.g. `2>`), where the digits are not a longer word.
    fn try_io_number(&mut self) -> Result<bool, Error> {
        let start = self.pos;
        let mut p = self.pos;
        while p < self.src.len() && self.src.as_bytes()[p].is_ascii_digit() {
            p += 1;
        }
        if p == start {
            return Ok(false);
        }
        let next = self.src[p..].chars().next();
        if !matches!(next, Some('<') | Some('>')) {
            return Ok(false);
        }
        // `2>` but not `2 >>`? `>>` follows digits directly: `2>>` valid too.
        let n: i32 = match self.src[start..p].parse() {
            Ok(n) => n,
            Err(_) => return Ok(false),
        };
        self.pos = p;
        self.push(Tok::IoNumber(n), start, p);
        Ok(true)
    }

    fn lex_word(&mut self) -> Result<(), Error> {
        let start = self.pos;
        if self.at_word_start && self.try_io_number()? {
            self.at_word_start = false;
            return Ok(());
        }
        let mut parts: Vec<Part> = Vec::new();
        let mut raw = String::new();
        loop {
            if self.pos >= self.src.len() {
                break;
            }
            let c = self.cur()?;
            match c {
                ' ' | '\t' | '\n' | '\r' => break,
                '|' | '&' | ';' | '(' | ')' => break,
                '<' | '>' => break,
                '\'' => {
                    self.bump();
                    let s = self.read_until('\'')?;
                    flush_raw(&mut raw, &mut parts);
                    parts.push(Part::Single(s));
                }
                '"' => {
                    self.bump();
                    let inner = self.read_dquote()?;
                    flush_raw(&mut raw, &mut parts);
                    parts.push(Part::Double(inner));
                }
                '\\' => {
                    self.bump();
                    match self.cur() {
                        Ok('\n') => {
                            self.bump(); // line continuation: nothing emitted
                        }
                        Ok(c2) => {
                            self.bump();
                            flush_raw(&mut raw, &mut parts);
                            parts.push(Part::Esc(c2));
                        }
                        Err(_) => return Err(Error::Incomplete),
                    }
                }
                '`' => {
                    self.bump();
                    let content = self.read_backtick()?;
                    flush_raw(&mut raw, &mut parts);
                    parts.push(Part::Subst(content));
                }
                '$' => {
                    let part = self.read_dollar()?;
                    flush_raw(&mut raw, &mut parts);
                    parts.push(part);
                }
                _ => {
                    raw.push(c);
                    self.bump();
                }
            }
        }
        flush_raw(&mut raw, &mut parts);
        if parts.is_empty() {
            return Err(Error::Parse("empty word".into()));
        }
        let end = self.pos;
        let word = Word {
            parts,
            span: Span { start, end },
        };
        // Here-doc delimiter: next word after `<<` / `<<-`.
        if let Some(Tok::Op(Op::DLess | Op::DLessDash)) = self.tokens.last().map(|t| &t.tok) {
            let strip = matches!(
                self.tokens.last().map(|t| &t.tok),
                Some(Tok::Op(Op::DLessDash))
            );
            let delim = plain_text(&word)?;
            let expand = !word_contains_single(&word);
            self.pending.push(PendingHd {
                delim,
                expand,
                strip_tabs: strip,
            });
        }
        self.push(Tok::Word(word), start, end);
        self.at_word_start = false;
        Ok(())
    }

    fn read_until(&mut self, q: char) -> Result<String, Error> {
        let mut s = String::new();
        loop {
            if self.pos >= self.src.len() {
                return Err(Error::Incomplete);
            }
            let c = self.cur()?;
            self.bump();
            if c == q {
                return Ok(s);
            }
            s.push(c);
        }
    }

    /// Read double-quoted section contents (opening `"` consumed).
    fn read_dquote(&mut self) -> Result<Vec<Part>, Error> {
        let mut parts: Vec<Part> = Vec::new();
        let mut raw = String::new();
        loop {
            if self.pos >= self.src.len() {
                return Err(Error::Incomplete);
            }
            let c = self.cur()?;
            match c {
                '"' => {
                    self.bump();
                    break;
                }
                '\\' => {
                    self.bump();
                    let c2 = self.cur()?;
                    match c2 {
                        '\n' => {
                            self.bump(); // continuation
                        }
                        '"' | '\\' | '$' | '`' => {
                            self.bump();
                            flush_raw(&mut raw, &mut parts);
                            parts.push(Part::Esc(c2));
                        }
                        _ => raw.push('\\'), // backslash kept literally (POSIX)
                    }
                }
                '`' => {
                    self.bump();
                    let content = self.read_backtick()?;
                    flush_raw(&mut raw, &mut parts);
                    parts.push(Part::Subst(content));
                }
                '$' => {
                    let part = self.read_dollar()?;
                    flush_raw(&mut raw, &mut parts);
                    parts.push(part);
                }
                _ => {
                    raw.push(c);
                    self.bump();
                }
            }
        }
        flush_raw(&mut raw, &mut parts);
        Ok(parts)
    }

    /// Read backtick contents (opening backtick consumed).
    fn read_backtick(&mut self) -> Result<String, Error> {
        let mut s = String::new();
        loop {
            if self.pos >= self.src.len() {
                return Err(Error::Incomplete);
            }
            let c = self.cur()?;
            if c == '`' {
                self.bump();
                return Ok(s);
            }
            if c == '\\' {
                let n = self.peek_next()?;
                if matches!(n, Some('`') | Some('\\')) {
                    self.bump();
                    s.push('\\');
                    s.push(n.unwrap_or('\\'));
                    self.bump();
                    continue;
                }
            }
            s.push(c);
            self.bump();
        }
    }

    /// Read at `$` (current char). Advances past the construct.
    fn read_dollar(&mut self) -> Result<Part, Error> {
        let c1 = self.src[self.pos + 1..].chars().next();
        let c2 = self.src[self.pos + 1..].chars().nth(1);
        if c1 == Some('\'') {
            self.bump(); // `$`
            self.bump(); // `'`
            let s = self.read_ansi_c()?;
            return Ok(Part::Single(s));
        }
        if c1 == Some('(') && c2 == Some('(') {
            self.bump();
            self.bump();
            self.bump(); // consume `$((`
            let content = self.scan_arith()?;
            return Ok(Part::Arith(content));
        }
        if c1 == Some('(') {
            self.bump();
            self.bump(); // consume `$(`
            let content = self.scan_balanced(1, '(', ')')?;
            return Ok(Part::Subst(content));
        }
        if c1 == Some('{') {
            self.bump();
            self.bump(); // consume `${`
            let content = self.scan_balanced(1, '{', '}')?;
            return Ok(Part::Param(content));
        }
        match c1 {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                self.bump(); // `$`
                let mut name = String::new();
                while let Ok(c) = self.cur() {
                    if c.is_ascii_alphanumeric() || c == '_' {
                        name.push(c);
                        self.bump();
                    } else {
                        break;
                    }
                }
                Ok(Part::Param(name))
            }
            Some(c)
                if c.is_ascii_digit() || matches!(c, '?' | '$' | '!' | '#' | '*' | '@' | '-') =>
            {
                self.bump(); // `$`
                let c = self.cur()?;
                self.bump();
                Ok(Part::Param(c.to_string()))
            }
            _ => {
                self.bump(); // lone `$`
                Ok(Part::Raw("$".into()))
            }
        }
    }

    /// Read `$'...'` ANSI-C quoted contents (opening `$'` consumed):
    /// scan to the closing quote (backslash escapes included), then
    /// decode via the shared [`ansi_c_decode`] table.
    fn read_ansi_c(&mut self) -> Result<String, Error> {
        let start = self.pos;
        loop {
            if self.pos >= self.src.len() {
                return Err(Error::Incomplete);
            }
            let c = self.cur()?;
            if c == '\\' {
                self.bump();
                if self.pos >= self.src.len() {
                    return Err(Error::Incomplete);
                }
                self.bump();
                continue;
            }
            if c == '\'' {
                let content = &self.src[start..self.pos];
                self.bump(); // closing quote
                return Ok(ansi_c_decode(content, false, false).0);
            }
            self.bump();
        }
    }

    /// Scan `$((...))` content. `(` and `)` counted from depth 2; content
    /// ends where depth first returns to 1, then both outer closes consumed.
    fn scan_arith(&mut self) -> Result<String, Error> {
        let content_start = self.pos;
        let mut depth: i64 = 2;
        let mut content: Option<String> = None;
        loop {
            if self.pos >= self.src.len() {
                return Err(Error::Incomplete);
            }
            let c = self.cur()?;
            if c == '(' {
                depth += 1;
                self.bump();
            } else if c == ')' {
                depth -= 1;
                if depth == 1 && content.is_none() {
                    content = Some(self.src[content_start..self.pos].to_string());
                }
                self.bump();
                if depth == 0 {
                    return Ok(content.unwrap_or_default());
                }
            } else if c == '\\' {
                self.bump();
                self.bump();
            } else {
                self.bump();
            }
        }
    }

    /// Scan until matching close at depth 0; `start_depth` counts opens
    /// already consumed. Returns inner content (without outer brackets).
    fn scan_balanced(
        &mut self,
        start_depth: usize,
        open: char,
        close: char,
    ) -> Result<String, Error> {
        let content_start = self.pos;
        let mut depth = start_depth;
        loop {
            if self.pos >= self.src.len() {
                return Err(Error::Incomplete);
            }
            let c = self.cur()?;
            if c == open {
                depth += 1;
                self.bump();
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    let content = self.src[content_start..self.pos].to_string();
                    self.bump(); // consume final close
                    // `$((...))` consumed only inner parens up to depth 0:
                    // for arith, `((` opened with depth=2, so both closes consumed.
                    return Ok(content);
                }
                self.bump();
            } else if c == '\'' {
                self.bump();
                self.read_until('\'')?;
            } else if c == '"' {
                self.bump();
                self.read_dquote()?;
            } else if c == '\\' {
                self.bump();
                self.bump();
            } else if c == '`' {
                self.bump();
                self.read_backtick()?;
            } else {
                self.bump();
            }
        }
    }
}

/// Decode ANSI-C backslash escapes (contents of `$'...'`, `echo -e`).
///
/// Supports `\n \t \r \a \b \f \v \e`, `\\ \' \" \?`, octal
/// `\0nnn`, hex `\xHH`, `\uHHHH`, `\UHHHHHHHH`, and `\cX` control
/// escapes. `keep_unknown` retains the backslash of unrecognized
/// escapes (bash `echo -e` keeps `\q`, `$'\q'` drops it). With
/// `c_cut`, a bare `\c` truncates the output and reports `cut` (echo's
/// "no trailing newline" terminator); without it, `\cX` is the
/// control escape from `$'...'` semantics.
pub fn ansi_c_decode(s: &str, keep_unknown: bool, c_cut: bool) -> (String, bool) {
    let mut out = String::new();
    let mut cut = false;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(e) = chars.next() else {
            if keep_unknown {
                out.push('\\');
            }
            break;
        };
        match e {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            'a' => out.push('\x07'),
            'b' => out.push('\x08'),
            'f' => out.push('\x0c'),
            'v' => out.push('\x0b'),
            'e' => out.push('\x1b'),
            '\\' | '\'' | '"' | '?' => out.push(e),
            '0'..='7' => {
                let mut v = e.to_digit(8).unwrap_or_default();
                for _ in 0..2 {
                    match chars.clone().next().and_then(|d| d.to_digit(8)) {
                        Some(x) => {
                            v = v * 8 + x;
                            chars.next();
                        }
                        None => break,
                    }
                }
                out.push(char::from_u32(v).unwrap_or('\u{fffd}'));
            }
            'x' | 'u' | 'U' => {
                let max = match e {
                    'x' => 2,
                    'u' => 4,
                    _ => 8,
                };
                let mut v: Option<u32> = None;
                for _ in 0..max {
                    match chars.clone().next().and_then(|d| d.to_digit(16)) {
                        Some(x) => {
                            v = Some(v.unwrap_or(0) * 16 + x);
                            chars.next();
                        }
                        None => break,
                    }
                }
                if let Some(x) = v.and_then(char::from_u32) {
                    out.push(x);
                }
            }
            'c' => {
                if c_cut {
                    cut = true;
                    break;
                }
                match chars.next() {
                    Some('?') => out.push('\x7f'),
                    Some(d) => out.push(char::from_u32((d as u32) & 0x1f).unwrap_or('\u{fffd}')),
                    None => out.push('\u{fffd}'),
                }
            }
            other => {
                if keep_unknown {
                    out.push('\\');
                }
                out.push(other);
            }
        }
    }
    (out, cut)
}

fn flush_raw(raw: &mut String, parts: &mut Vec<Part>) {
    if !raw.is_empty() {
        parts.push(Part::Raw(std::mem::take(raw)));
    }
}

/// Quote-removed plain text of a word (for here-doc delimiters).
pub fn plain_text(word: &Word) -> Result<String, Error> {
    let mut s = String::new();
    for p in &word.parts {
        match p {
            Part::Raw(t) => s.push_str(t),
            Part::Esc(c) => s.push(*c),
            Part::Single(t) => s.push_str(t),
            Part::Double(inner) => {
                for ip in inner {
                    match ip {
                        Part::Raw(t) => s.push_str(t),
                        Part::Esc(c) => s.push(*c),
                        _ => {
                            return Err(Error::Parse("here-doc delimiter cannot contain $".into()));
                        }
                    }
                }
            }
            _ => return Err(Error::Parse("here-doc delimiter cannot contain $".into())),
        }
    }
    Ok(s)
}

fn word_contains_single(word: &Word) -> bool {
    word.parts.iter().any(|p| matches!(p, Part::Single(_)))
}

/// Plain text of a word if it is fully literal (no expansions).
pub fn literal_text(word: &Word) -> Option<String> {
    fn parts_text(parts: &[Part]) -> Option<String> {
        let mut s = String::new();
        for p in parts {
            match p {
                Part::Raw(t) => s.push_str(t),
                Part::Esc(c) => s.push(*c),
                Part::Single(t) => s.push_str(t),
                Part::Double(inner) => s.push_str(&parts_text(inner)?),
                _ => return None,
            }
        }
        Some(s)
    }
    parts_text(&word.parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(src: &str) -> Vec<Tok> {
        lex(src)
            .unwrap()
            .tokens
            .into_iter()
            .map(|t| t.tok)
            .collect()
    }

    #[test]
    fn simple_word_and_op() {
        let k = kinds("echo hi");
        assert_eq!(k.len(), 2);
        assert!(matches!(&k[0], Tok::Word(w) if literal_text(w).as_deref() == Some("echo")));
        assert!(matches!(&k[1], Tok::Word(w) if literal_text(w).as_deref() == Some("hi")));
    }

    #[test]
    fn quoting() {
        let k = kinds("echo 'a b' \"c $x\"");
        assert_eq!(k.len(), 3);
        let w = match &k[2] {
            Tok::Word(w) => w,
            _ => panic!(),
        };
        assert_eq!(
            w.parts,
            vec![Part::Double(vec![
                Part::Raw("c ".into()),
                Part::Param("x".into())
            ])]
        );
        let w = match &k[1] {
            Tok::Word(w) => w,
            _ => panic!(),
        };
        assert_eq!(w.parts, vec![Part::Single("a b".into())]);
    }

    #[test]
    fn operators() {
        let k = kinds("a && b || c | d ; e & f");
        let ops: Vec<Op> = k
            .iter()
            .filter_map(|t| match t {
                Tok::Op(o) => Some(*o),
                _ => None,
            })
            .collect();
        assert_eq!(ops, vec![Op::And, Op::Or, Op::Pipe, Op::Semi, Op::Amp]);
    }

    #[test]
    fn redirect_tokens() {
        let k = kinds("echo x >f 2>>g <<H <<-T >&1 <&0");
        let ops: Vec<Op> = k
            .iter()
            .filter_map(|t| match t {
                Tok::Op(o) => Some(*o),
                _ => None,
            })
            .collect();
        assert_eq!(
            ops,
            vec![
                Op::Great,
                Op::DGreat,
                Op::DLess,
                Op::DLessDash,
                Op::GtAmp,
                Op::LtAmp
            ]
        );
        assert!(k.iter().any(|t| matches!(t, Tok::IoNumber(2))));
    }

    #[test]
    fn comment_skipped() {
        let k = kinds("echo # not a word\nx");
        assert_eq!(k.len(), 3); // echo, newline, x
    }

    #[test]
    fn line_continuation() {
        let k = kinds("echo \\\nhi");
        assert_eq!(k.len(), 2);
    }

    #[test]
    fn unterminated_quote_is_incomplete() {
        assert!(matches!(lex("echo 'abc"), Err(Error::Incomplete)));
        assert!(matches!(lex("echo \"abc"), Err(Error::Incomplete)));
        assert!(matches!(lex("echo $(foo"), Err(Error::Incomplete)));
        assert!(matches!(lex("echo ${x"), Err(Error::Incomplete)));
        assert!(matches!(lex("echo \\"), Err(Error::Incomplete)));
    }

    #[test]
    fn heredoc_body_collected() {
        let out = lex("cat <<EOF\nhello\nworld\nEOF\necho done").unwrap();
        assert_eq!(out.heredocs.len(), 1);
        assert_eq!(out.heredocs[0].text, "hello\nworld\n");
        assert!(out.heredocs[0].expand);
        let ops: Vec<Op> = out
            .tokens
            .iter()
            .filter_map(|t| match t.tok {
                Tok::Op(o) => Some(o),
                _ => None,
            })
            .collect();
        assert_eq!(ops, vec![Op::DLess]);
    }

    #[test]
    fn heredoc_strip_tabs() {
        let out = lex("cat <<-\tEOF\n\thi\n\tEOF\n").unwrap();
        assert_eq!(out.heredocs[0].text, "hi\n");
    }

    #[test]
    fn heredoc_quoted_delim_no_expand() {
        let out = lex("cat <<'EOF'\n$x\nEOF\n").unwrap();
        assert_eq!(out.heredocs[0].text, "$x\n");
        assert!(!out.heredocs[0].expand);

        let out = lex("cat <<\"E\"OF\n$x\nEOF\n").unwrap();
        assert!(out.heredocs[0].expand);
    }

    #[test]
    fn two_heredocs() {
        let out = lex("cat <<A <<B\none\nA\ntwo\nB\n").unwrap();
        assert_eq!(out.heredocs.len(), 2);
        assert_eq!(out.heredocs[0].text, "one\n");
        assert_eq!(out.heredocs[1].text, "two\n");
    }

    #[test]
    fn dollar_forms() {
        let k = kinds("echo $x ${y:-z} $(cmd) `cmd` $((1+2)) $? $# $@");
        assert_eq!(k.len(), 9);
        let word = |i: usize| match &k[i] {
            Tok::Word(w) => w.clone(),
            _ => panic!(),
        };
        assert_eq!(word(1).parts, vec![Part::Param("x".into())]);
        assert_eq!(word(2).parts, vec![Part::Param("y:-z".into())]);
        assert_eq!(word(3).parts, vec![Part::Subst("cmd".into())]);
        assert_eq!(word(5).parts, vec![Part::Arith("1+2".into())]);
        assert_eq!(word(6).parts, vec![Part::Param("?".into())]);
        assert_eq!(word(8).parts, vec![Part::Param("@".into())]);
    }

    #[test]
    fn ansi_c_quoting() {
        let w = match &kinds("echo $'a\\tb\\n'")[1] {
            Tok::Word(w) => w.clone(),
            _ => panic!(),
        };
        assert_eq!(w.parts, vec![Part::Single("a\tb\n".into())]);
        let w = match &kinds("echo $'\\x41\\u0042\\0'")[1] {
            Tok::Word(w) => w.clone(),
            _ => panic!(),
        };
        assert_eq!(w.parts, vec![Part::Single("AB\0".into())]);
        assert!(matches!(lex("echo $'abc"), Err(Error::Incomplete)));
    }

    #[test]
    fn here_string_operator() {
        let k = kinds("cat <<< hi");
        let ops: Vec<Op> = k
            .iter()
            .filter_map(|t| match t {
                Tok::Op(o) => Some(*o),
                _ => None,
            })
            .collect();
        assert_eq!(ops, vec![Op::DLessLess]);
    }

    #[test]
    fn arith_command_token() {
        let k = kinds("((x = 1 + 2)) && echo done");
        assert!(matches!(&k[0], Tok::ArithCmd(c) if c == "x = 1 + 2"));
        assert!(matches!(lex("((1 + 2"), Err(Error::Incomplete)));
    }

    #[test]
    fn spans_cover_source() {
        let out = lex("echo hi").unwrap();
        assert_eq!(out.tokens[0].span, Span { start: 0, end: 4 });
        assert_eq!(out.tokens[1].span, Span { start: 5, end: 7 });
    }
}
