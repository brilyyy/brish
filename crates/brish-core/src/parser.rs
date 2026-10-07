//! Recursive-descent parser: tokens -> [`Program`] AST.
//!
//! Grammar (POSIX subset): and-or lists, pipelines, simple commands,
//! redirections, subshell/group, if/while/until/for/case, functions.
//! EOF in a non-terminator position yields [`Error::Incomplete`] so the
//! REPL can continue with PS2.

use crate::ast::{AndOr, AndOrOp, Assign, CaseArm, Cmd, Item, Pipeline, Program, Redir, Simple};
use crate::error::Error;
use crate::lexer::{self, Op, Tok, Token, Word, literal_text};

/// Parse full source into a program (lex + parse).
pub fn parse(src: &str) -> Result<Program, Error> {
    let prog = parse_lexed(lexer::lex(src)?)?;
    if crate::debug_on("parser") {
        eprintln!("brish[parser]: {} items", prog.items.len());
    }
    Ok(prog)
}

/// Parse an already-lexed program (heredoc bodies embedded into AST).
pub fn parse_lexed(lexed: lexer::Lexed) -> Result<Program, Error> {
    let mut p = Parser {
        tokens: lexed.tokens,
        heredocs: lexed.heredocs,
        hd_index: 0,
        idx: 0,
    };
    let prog = p.parse_program(Stops::top())?;
    if p.idx < p.tokens.len() {
        let span = p.tokens[p.idx].span;
        return Err(Error::parse_at("unexpected trailing token", span));
    }
    Ok(prog)
}

/// Closing keywords that may not start a command.
const CLOSERS: &[&str] = &["then", "elif", "else", "fi", "do", "done", "esac"];

/// Reserved words recognized at command position.
const RESERVED: &[&str] = &[
    "if", "then", "else", "elif", "fi", "do", "done", "while", "until", "for", "in", "case",
    "esac", "{", "}", "!",
];

/// Terminator set for one `parse_program` invocation.
#[derive(Clone, Copy)]
struct Stops {
    /// EOF ends the program (top level); otherwise `Incomplete`.
    eof_ok: bool,
    rbrace: bool,
    rparen: bool,
    dsemi: bool,
    kws: &'static [&'static str],
}

impl Stops {
    fn top() -> Self {
        Stops {
            eof_ok: true,
            rbrace: false,
            rparen: false,
            dsemi: false,
            kws: &[],
        }
    }
    fn of(kws: &'static [&'static str]) -> Self {
        Stops {
            eof_ok: false,
            rbrace: false,
            rparen: false,
            dsemi: false,
            kws,
        }
    }
    fn dsemi() -> Self {
        Stops {
            eof_ok: false,
            rbrace: false,
            rparen: false,
            dsemi: true,
            kws: &["esac"],
        }
    }
    fn rparen() -> Self {
        Stops {
            eof_ok: false,
            rbrace: false,
            rparen: true,
            dsemi: false,
            kws: &[],
        }
    }
    fn rbrace() -> Self {
        Stops {
            eof_ok: false,
            rbrace: true,
            rparen: false,
            dsemi: false,
            kws: &[],
        }
    }
}

struct Parser {
    tokens: Vec<Token>,
    heredocs: Vec<lexer::HeredocBody>,
    hd_index: usize,
    idx: usize,
}

impl Parser {
    fn peek_tok(&self) -> Option<&Tok> {
        self.tokens.get(self.idx).map(|t| &t.tok)
    }

    fn peek_op(&self) -> Option<Op> {
        match self.peek_tok() {
            Some(Tok::Op(o)) => Some(*o),
            _ => None,
        }
    }

    fn peek_word_literal(&self) -> Option<String> {
        match self.peek_tok() {
            Some(Tok::Word(w)) => literal_text(w),
            _ => None,
        }
    }

    fn at_eof(&self) -> bool {
        self.idx >= self.tokens.len()
    }

    /// Parse error located at the current token (last token at EOF), so
    /// the REPL can point a caret at it.
    fn err(&self, msg: impl Into<String>) -> Error {
        match self.tokens.get(self.idx).or_else(|| self.tokens.last()) {
            Some(t) => Error::parse_at(msg, t.span),
            None => Error::parse(msg),
        }
    }

    fn skip_newlines(&mut self) {
        while matches!(self.peek_tok(), Some(Tok::Newline)) {
            self.idx += 1;
        }
    }

    fn eat_op(&mut self, op: Op) -> bool {
        if self.peek_op() == Some(op) {
            self.idx += 1;
            true
        } else {
            false
        }
    }

    fn eat_word(&mut self, kw: &str) -> bool {
        if self.peek_word_literal().as_deref() == Some(kw) {
            self.idx += 1;
            true
        } else {
            false
        }
    }

    fn expect_word(&mut self, kw: &str) -> Result<(), Error> {
        match self.peek_word_literal() {
            Some(w) if w == kw => {
                self.idx += 1;
                Ok(())
            }
            Some(_) => Err(self.err(format!("expected `{kw}'"))),
            None if self.at_eof() => Err(Error::Incomplete),
            None => Err(self.err(format!("expected `{kw}'"))),
        }
    }

    /// `[list]` terminated by EOF or the given stop set.
    fn parse_program(&mut self, stops: Stops) -> Result<Program, Error> {
        let mut items = Vec::new();
        loop {
            self.skip_newlines();
            if self.at_eof() {
                if stops.eof_ok {
                    break;
                }
                return Err(Error::Incomplete);
            }
            if let Some(w) = self.peek_word_literal() {
                if stops.kws.contains(&w.as_str()) {
                    break;
                }
                if stops.rbrace && w == "}" {
                    break;
                }
                if CLOSERS.contains(&w.as_str()) {
                    return Err(self.err(format!("unexpected `{w}'")));
                }
            }
            if stops.rparen && self.peek_op() == Some(Op::RParen) {
                break;
            }
            if stops.dsemi && self.peek_op() == Some(Op::Dsemi) {
                break;
            }

            let andor = self.parse_andor()?;
            let mut background = false;
            match self.peek_tok() {
                Some(Tok::Op(Op::Amp)) => {
                    self.idx += 1;
                    background = true;
                }
                Some(Tok::Op(Op::Semi)) => {
                    self.idx += 1;
                }
                Some(Tok::Newline) => self.skip_newlines(),
                _ => {}
            }
            items.push(Item { andor, background });
        }
        Ok(Program { items })
    }

    fn parse_andor(&mut self) -> Result<AndOr, Error> {
        let first = self.parse_pipeline()?;
        let mut rest = Vec::new();
        loop {
            let op = match self.peek_op() {
                Some(Op::And) => AndOrOp::And,
                Some(Op::Or) => AndOrOp::Or,
                _ => break,
            };
            self.idx += 1;
            self.skip_newlines();
            let pipe = self.parse_pipeline()?;
            rest.push((op, pipe));
        }
        Ok(AndOr { first, rest })
    }

    fn parse_pipeline(&mut self) -> Result<Pipeline, Error> {
        let mut negated = false;
        loop {
            if self.peek_word_literal().as_deref() == Some("!") && self.starts_command(self.idx + 1)
            {
                negated = true;
                self.idx += 1;
            } else {
                break;
            }
        }
        let mut cmds = vec![self.parse_command()?];
        while self.eat_op(Op::Pipe) {
            self.skip_newlines();
            cmds.push(self.parse_command()?);
        }
        Ok(Pipeline { cmds, negated })
    }

    /// Token at `i` can start a command (for `!` negation lookahead).
    fn starts_command(&self, i: usize) -> bool {
        matches!(
            self.tokens.get(i).map(|t| &t.tok),
            Some(Tok::Word(_)) | Some(Tok::IoNumber(_)) | Some(Tok::ArithCmd(_))
        ) || matches!(
            self.tokens.get(i).map(|t| &t.tok),
            Some(Tok::Op(
                Op::LParen
                    | Op::Less
                    | Op::Great
                    | Op::DGreat
                    | Op::DLess
                    | Op::DLessDash
                    | Op::DLessLess
                    | Op::GtAmp
                    | Op::LtAmp
                    | Op::Clobber
            ))
        )
    }

    fn parse_command(&mut self) -> Result<Cmd, Error> {
        // Function definition: NAME ( ) compound
        if let (Some(Tok::Word(_)), Some(Tok::Op(Op::LParen))) = (
            self.peek_tok(),
            self.tokens.get(self.idx + 1).map(|t| &t.tok),
        ) {
            let name = self.peek_word_literal().unwrap_or_default();
            if !name.is_empty() && !RESERVED.contains(&name.as_str()) {
                self.idx += 2; // name (
                if !self.eat_op(Op::RParen) {
                    return Err(self.err("expected `)' after function name"));
                }
                // POSIX puts no_newlines between `name()`, `do`/`{` and
                // the command word, but real scripts still write
                // `name()\n{` (Debian's hwclock.sh, dpkg-realpath, ldd all
                // do). Skip the newlines before the body.
                self.skip_newlines();
                let body = self.parse_command()?;
                let ok = matches!(body, Cmd::Group(_) | Cmd::Subshell(_));
                let body = Cmd::FuncDef {
                    name,
                    body: Box::new(body),
                };
                if !ok {
                    return Err(self.err("function body must be a compound command"));
                }
                return self.parse_trailing_redirs(body);
            }
        }

        let cmd = match self.peek_tok() {
            None => return Err(Error::Incomplete),
            Some(Tok::Op(Op::LParen)) => self.parse_subshell()?,
            Some(Tok::ArithCmd(src)) => {
                let src = src.clone();
                self.idx += 1;
                Cmd::Arith(src)
            }
            Some(Tok::Word(_)) => {
                let lit = self.peek_word_literal();
                match lit.as_deref() {
                    Some("if") => self.parse_if()?,
                    Some("while") | Some("until") => self.parse_loop()?,
                    Some("for") => self.parse_for()?,
                    Some("case") => self.parse_case()?,
                    Some("{") => self.parse_group()?,
                    _ => self.parse_simple()?,
                }
            }
            Some(Tok::IoNumber(_)) | Some(Tok::Op(_)) => self.parse_simple()?,
            Some(Tok::Newline) => return Err(self.err("expected command")),
        };
        self.parse_trailing_redirs(cmd)
    }

    fn parse_subshell(&mut self) -> Result<Cmd, Error> {
        self.idx += 1; // (
        let prog = self.parse_program(Stops::rparen())?;
        if !self.eat_op(Op::RParen) {
            return Err(self.err("expected `)'"));
        }
        Ok(Cmd::Subshell(prog))
    }

    fn parse_group(&mut self) -> Result<Cmd, Error> {
        self.idx += 1; // {
        let prog = self.parse_program(Stops::rbrace())?;
        self.expect_word("}")?;
        Ok(Cmd::Group(prog))
    }

    fn parse_if(&mut self) -> Result<Cmd, Error> {
        self.idx += 1; // if
        let cond = self.parse_program(Stops::of(&["then"]))?;
        self.expect_word("then")?;
        let then = self.parse_program(Stops::of(&["elif", "else", "fi"]))?;
        let mut elif = Vec::new();
        let mut els = None;
        loop {
            if self.eat_word("elif") {
                let c = self.parse_program(Stops::of(&["then"]))?;
                self.expect_word("then")?;
                let b = self.parse_program(Stops::of(&["elif", "else", "fi"]))?;
                elif.push((c, b));
            } else if self.eat_word("else") {
                els = Some(self.parse_program(Stops::of(&["fi"]))?);
                self.expect_word("fi")?;
                break;
            } else {
                self.expect_word("fi")?;
                break;
            }
        }
        Ok(Cmd::If {
            cond,
            then,
            elif,
            els,
        })
    }

    fn parse_loop(&mut self) -> Result<Cmd, Error> {
        let until = self.peek_word_literal().as_deref() == Some("until");
        self.idx += 1; // while / until
        let cond = self.parse_program(Stops::of(&["do"]))?;
        self.expect_word("do")?;
        let body = self.parse_program(Stops::of(&["done"]))?;
        self.expect_word("done")?;
        Ok(Cmd::Loop { cond, body, until })
    }

    fn parse_for(&mut self) -> Result<Cmd, Error> {
        self.idx += 1; // for
        let var = match self.peek_tok() {
            Some(Tok::Word(w)) => {
                let name = literal_text(w)
                    .filter(|n| is_name(n))
                    .ok_or_else(|| self.err("bad for-loop variable name"))?;
                self.idx += 1;
                name
            }
            None => return Err(Error::Incomplete),
            _ => return Err(self.err("expected for-loop variable name")),
        };
        let words = if self.eat_word("in") {
            let mut ws = Vec::new();
            loop {
                match self.peek_tok() {
                    Some(Tok::Word(_)) if self.peek_word_literal().as_deref() == Some("do") => {
                        break;
                    }
                    Some(Tok::Word(_)) => {
                        ws.push(match self.peek_tok() {
                            Some(Tok::Word(w)) => w.clone(),
                            _ => unreachable!(),
                        });
                        self.idx += 1;
                    }
                    Some(Tok::Newline) | Some(Tok::Op(Op::Semi)) => {
                        self.idx += 1;
                        self.skip_newlines();
                        break;
                    }
                    None => return Err(Error::Incomplete),
                    _ => return Err(self.err("unexpected token in for word list")),
                }
            }
            Some(ws)
        } else if self.peek_word_literal().as_deref() == Some("do") {
            // `for i do … done` — POSIX lets the `in` list and its
            // separator be omitted entirely; the loop then runs over
            // "$@" (dash and bash accept this, /usr/bin/zforce uses it).
            None
        } else {
            if !matches!(
                self.peek_tok(),
                Some(Tok::Newline) | Some(Tok::Op(Op::Semi))
            ) {
                if self.at_eof() {
                    return Err(Error::Incomplete);
                }
                return Err(self.err("expected `;', newline, or `do'"));
            }
            self.idx += 1;
            self.skip_newlines();
            None
        };
        self.expect_word("do")?;
        let body = self.parse_program(Stops::of(&["done"]))?;
        self.expect_word("done")?;
        Ok(Cmd::For { var, words, body })
    }

    fn parse_case(&mut self) -> Result<Cmd, Error> {
        self.idx += 1; // case
        let word = match self.peek_tok() {
            Some(Tok::Word(w)) => {
                let w = w.clone();
                self.idx += 1;
                w
            }
            None => return Err(Error::Incomplete),
            _ => return Err(self.err("expected word after `case'")),
        };
        self.skip_newlines();
        self.expect_word("in")?;
        let mut arms = Vec::new();
        loop {
            self.skip_newlines();
            if self.eat_word("esac") {
                break;
            }
            if self.at_eof() {
                return Err(Error::Incomplete);
            }
            // Patterns: word ('|' word)* ')'
            let mut pats = Vec::new();
            // POSIX 2.6.4 case_pattern: `'(' pattern` — a leading `(`
            // opens a *group*, and the parens themselves are NOT part of
            // the pattern. So `case ab in (ab) …` matches the plain
            // string `ab`. /usr/bin/zgrep and which.debianutils write
            // `case $x in (*[!:]:) …` and expect exactly that.
            // (`set -n` on those files failed before this was allowed.)
            let mut grouping = false;
            loop {
                match self.peek_tok() {
                    Some(Tok::Word(_)) => {
                        pats.push(match self.peek_tok() {
                            Some(Tok::Word(w)) => w.clone(),
                            _ => unreachable!(),
                        });
                        self.idx += 1;
                    }
                    Some(Tok::Op(Op::Pipe)) => {
                        self.idx += 1;
                    }
                    Some(Tok::Op(Op::LParen)) if pats.is_empty() && !grouping => {
                        // Only legal as the first character of a pattern;
                        // `a(b` is a syntax error in dash too, so stay
                        // strict about the mid-pattern case.
                        grouping = true;
                        self.idx += 1;
                    }
                    Some(Tok::Op(Op::RParen)) => {
                        self.idx += 1;
                        break;
                    }
                    None => return Err(Error::Incomplete),
                    _ => return Err(self.err("expected `)' in case pattern")),
                }
            }
            if pats.is_empty() {
                return Err(self.err("empty case pattern"));
            }
            let body = self.parse_program(Stops::dsemi())?;
            if !self.eat_op(Op::Dsemi)
                && self.peek_word_literal().as_deref() != Some("esac")
                && !self.at_eof()
            {
                return Err(self.err("expected `;;' or `esac'"));
            }
            arms.push(CaseArm { pats, body });
        }
        Ok(Cmd::Case { word, arms })
    }

    /// Simple command: assignments, words, redirections in any order.
    fn parse_simple(&mut self) -> Result<Cmd, Error> {
        let mut assigns = Vec::new();
        let mut redirs = Vec::new();
        let mut words = Vec::new();
        let mut seen_word = false;
        loop {
            match self.peek_tok().cloned() {
                None => break,
                Some(Tok::IoNumber(n)) => {
                    self.idx += 1;
                    let op = match self.peek_op() {
                        Some(o) if is_redir(o) => o,
                        None => return Err(Error::Incomplete),
                        _ => {
                            return Err(self.err("expected redirection operator after IO number"));
                        }
                    };
                    self.idx += 1;
                    redirs.push(self.parse_redir_target(n as usize, op)?);
                }
                Some(Tok::Op(o)) if is_redir(o) => {
                    self.idx += 1;
                    redirs.push(self.parse_redir_target(default_fd(o), o)?);
                }
                Some(Tok::Word(w)) => {
                    self.idx += 1;
                    if !seen_word && is_assign(&w) {
                        assigns.push(make_assign(&w));
                    } else {
                        seen_word = true;
                        words.push(w);
                    }
                }
                _ => break,
            }
        }
        if words.is_empty() && assigns.is_empty() && redirs.is_empty() {
            return Err(self.err("expected command"));
        }
        Ok(Cmd::Simple(Simple {
            assigns,
            redirs,
            words,
        }))
    }

    /// Redirection operator consumed; read its target word.
    fn parse_redir_target(&mut self, fd: usize, op: Op) -> Result<Redir, Error> {
        let target = match self.peek_tok() {
            Some(Tok::Word(w)) => {
                let w = w.clone();
                self.idx += 1;
                w
            }
            None => return Err(Error::Incomplete),
            _ => return Err(self.err("expected redirection target")),
        };
        Ok(match op {
            Op::Less => Redir::Input { fd, target },
            Op::Great => Redir::Output { fd, target },
            Op::DGreat => Redir::Append { fd, target },
            Op::Clobber => Redir::Clobber { fd, target },
            Op::LtAmp => Redir::DupIn { fd, target },
            Op::GtAmp => Redir::DupOut { fd, target },
            Op::DLessLess => Redir::HereString { fd, target },
            Op::DLess | Op::DLessDash => {
                let body = self
                    .heredocs
                    .get(self.hd_index)
                    .cloned()
                    .ok_or_else(|| self.err("missing here-doc body"))?;
                self.hd_index += 1;
                Redir::Heredoc {
                    fd,
                    text: body.text,
                    expand: body.expand,
                }
            }
            _ => return Err(self.err("not a redirection operator")),
        })
    }

    /// Redirections after a compound command: `(...) > f`.
    fn parse_trailing_redirs(&mut self, mut cmd: Cmd) -> Result<Cmd, Error> {
        let mut redirs = Vec::new();
        loop {
            match self.peek_tok().cloned() {
                Some(Tok::IoNumber(n)) => {
                    self.idx += 1;
                    let op = match self.peek_op() {
                        Some(o) if is_redir(o) => o,
                        None => return Err(Error::Incomplete),
                        _ => {
                            return Err(self.err("expected redirection operator after IO number"));
                        }
                    };
                    self.idx += 1;
                    redirs.push(self.parse_redir_target(n as usize, op)?);
                }
                Some(Tok::Op(o)) if is_redir(o) => {
                    self.idx += 1;
                    redirs.push(self.parse_redir_target(default_fd(o), o)?);
                }
                _ => break,
            }
        }
        if !redirs.is_empty() {
            if let Cmd::Redirected {
                redirs: existing, ..
            } = &mut cmd
            {
                existing.extend(redirs);
            } else {
                cmd = Cmd::Redirected {
                    inner: Box::new(cmd),
                    redirs,
                };
            }
        }
        Ok(cmd)
    }
}

fn is_redir(o: Op) -> bool {
    matches!(
        o,
        Op::Less
            | Op::Great
            | Op::DGreat
            | Op::DLess
            | Op::DLessDash
            | Op::DLessLess
            | Op::GtAmp
            | Op::LtAmp
            | Op::Clobber
    )
}

fn default_fd(o: Op) -> usize {
    match o {
        Op::Less | Op::LtAmp | Op::DLess | Op::DLessDash | Op::DLessLess => 0,
        _ => 1,
    }
}

fn is_name(s: &str) -> bool {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Word is a `name=value` assignment prefix (at command start).
fn is_assign(w: &Word) -> bool {
    match w.parts.first() {
        Some(lexer::Part::Raw(t)) => match t.find('=') {
            Some(eq) => is_name(&t[..eq]),
            None => false,
        },
        _ => false,
    }
}

fn make_assign(w: &Word) -> Assign {
    let mut value_parts = Vec::new();
    let mut name = String::new();
    for (i, p) in w.parts.iter().enumerate() {
        match p {
            lexer::Part::Raw(t) if i == 0 => {
                if let Some(eq) = t.find('=') {
                    name = t[..eq].to_string();
                    let rest = &t[eq + 1..];
                    if !rest.is_empty() {
                        value_parts.push(lexer::Part::Raw(rest.to_string()));
                    }
                }
            }
            other => value_parts.push(other.clone()),
        }
    }
    Assign {
        name,
        value: Word {
            parts: value_parts,
            span: w.span,
        },
        span: w.span,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(src: &str) -> Cmd {
        let p = parse(src).unwrap();
        assert_eq!(p.items.len(), 1);
        p.items[0].andor.first.cmds[0].clone()
    }

    fn simple(src: &str) -> Simple {
        match one(src) {
            Cmd::Simple(s) => s,
            other => panic!("not simple: {other:?}"),
        }
    }

    #[test]
    fn simple_cmd() {
        let s = simple("echo hi there");
        assert!(s.assigns.is_empty() && s.redirs.is_empty());
        assert_eq!(s.words.len(), 3);
    }

    #[test]
    fn assign_prefix() {
        let s = simple("FOO=bar BAZ=1 cmd arg");
        assert_eq!(s.assigns.len(), 2);
        assert_eq!(s.assigns[0].name, "FOO");
        assert_eq!(literal_text(&s.assigns[0].value).as_deref(), Some("bar"));
        assert_eq!(s.words.len(), 2);
    }

    #[test]
    fn assign_only() {
        let s = simple("FOO=bar");
        assert_eq!(s.assigns.len(), 1);
        assert!(s.words.is_empty());
    }

    #[test]
    fn not_assign() {
        // digits first, no `=`: ordinary words
        let s = simple("1A=1 cmd");
        assert!(s.assigns.is_empty());
        assert_eq!(s.words.len(), 2);
    }

    #[test]
    fn arith_command_parses() {
        let p = parse("((x = 1 + 2)) && echo done").unwrap();
        assert!(matches!(&p.items[0].andor.first.cmds[0], Cmd::Arith(src) if src == "x = 1 + 2"));
        // trailing redirections on the arith command
        let p = parse("((1)) > f").unwrap();
        assert!(matches!(
            &p.items[0].andor.first.cmds[0],
            Cmd::Redirected { inner, .. } if matches!(**inner, Cmd::Arith(_))
        ));
    }

    #[test]
    fn here_string_parses() {
        let p = parse("cat <<< hi").unwrap();
        let cmds = &p.items[0].andor.first.cmds;
        match &cmds[0] {
            Cmd::Simple(s) => assert!(matches!(
                &s.redirs[0],
                Redir::HereString { fd: 0, target }
                    if literal_text(target).as_deref() == Some("hi")
            )),
            other => panic!("expected simple, got {other:?}"),
        }
        let p = parse("2<<< x").unwrap();
        match &p.items[0].andor.first.cmds[0] {
            Cmd::Simple(s) => assert!(matches!(&s.redirs[0], Redir::HereString { fd: 2, .. })),
            other => panic!("expected simple, got {other:?}"),
        }
    }

    #[test]
    fn pipeline_negated() {
        let p = parse("! echo hi | grep h").unwrap();
        let pipe = &p.items[0].andor.first;
        assert!(pipe.negated);
        assert_eq!(pipe.cmds.len(), 2);
    }

    #[test]
    fn andor_background() {
        let p = parse("a && b || c &").unwrap();
        assert_eq!(p.items.len(), 1);
        assert!(p.items[0].background);
        assert_eq!(p.items[0].andor.rest.len(), 2);
        assert_eq!(p.items[0].andor.rest[0].0, AndOrOp::And);
        assert_eq!(p.items[0].andor.rest[1].0, AndOrOp::Or);
    }

    #[test]
    fn two_items() {
        let p = parse("echo a; echo b\necho c").unwrap();
        assert_eq!(p.items.len(), 3);
    }

    #[test]
    fn subshell_group() {
        match one("(echo hi)") {
            Cmd::Subshell(p) => assert_eq!(p.items.len(), 1),
            other => panic!("{other:?}"),
        }
        match one("{ echo hi; }") {
            Cmd::Group(p) => assert_eq!(p.items.len(), 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn compound_trailing_redir() {
        match one("(echo hi) > f") {
            Cmd::Redirected { inner, redirs } => {
                assert!(matches!(*inner, Cmd::Subshell(_)));
                assert_eq!(redirs.len(), 1);
                assert!(matches!(&redirs[0], Redir::Output { fd: 1, .. }));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn if_elif_else() {
        match one("if a; then b; elif c; then d; else e; fi") {
            Cmd::If {
                cond,
                then,
                elif,
                els,
            } => {
                assert_eq!(cond.items.len(), 1);
                assert_eq!(then.items.len(), 1);
                assert_eq!(elif.len(), 1);
                assert!(els.is_some());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn while_and_until() {
        match one("while a; do b; done") {
            Cmd::Loop { until, .. } => assert!(!until),
            other => panic!("{other:?}"),
        }
        match one("until a; do b; done") {
            Cmd::Loop { until, .. } => assert!(until),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn for_loops() {
        match one("for i in a b c; do echo $i; done") {
            Cmd::For { var, words, .. } => {
                assert_eq!(var, "i");
                assert_eq!(words.unwrap().len(), 3);
            }
            other => panic!("{other:?}"),
        }
        match one("for i; do echo x; done") {
            Cmd::For { words, .. } => assert!(words.is_none()),
            other => panic!("{other:?}"),
        }
        match one("for i in; do echo x; done") {
            Cmd::For { words, .. } => assert_eq!(words.unwrap().len(), 0),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn case_arms() {
        match one("case $x in a|b) echo one;; *) echo two; esac") {
            Cmd::Case { arms, .. } => {
                assert_eq!(arms.len(), 2);
                assert_eq!(arms[0].pats.len(), 2);
                assert_eq!(arms[1].pats.len(), 1);
                assert_eq!(arms[1].body.items.len(), 1);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn case_without_final_dsemi() {
        match one("case x in a) echo;; esac") {
            Cmd::Case { arms, .. } => assert_eq!(arms.len(), 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn function_def() {
        match one("f() { echo hi; }") {
            Cmd::FuncDef { name, body } => {
                assert_eq!(name, "f");
                assert!(matches!(*body, Cmd::Group(_)));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn redirects() {
        let s = simple("cmd >f 2>>g <h >&3 <&4");
        let kinds: Vec<(&str, usize)> = s
            .redirs
            .iter()
            .map(|r| match r {
                Redir::Output { fd, .. } => (">", *fd),
                Redir::Append { fd, .. } => (">>", *fd),
                Redir::Input { fd, .. } => ("<", *fd),
                Redir::DupOut { fd, .. } => (">&", *fd),
                Redir::DupIn { fd, .. } => ("<&", *fd),
                _ => ("?", 0),
            })
            .collect();
        assert_eq!(
            kinds,
            vec![(">", 1), (">>", 2), ("<", 0), (">&", 1), ("<&", 0)]
        );
    }

    #[test]
    fn heredoc_embedded() {
        let s = simple("cat <<'H'\n$x\nH\n");
        match &s.redirs[0] {
            Redir::Heredoc {
                text, expand, fd, ..
            } => {
                assert_eq!(text, "$x\n");
                assert!(!expand);
                assert_eq!(*fd, 0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn incomplete_inputs() {
        for src in [
            "if true; then",
            "while true; do",
            "for i in a; do",
            "case x in a)",
            "a &&",
            "echo hi |",
            "(echo hi",
            "{ echo hi;",
            "f()",
        ] {
            assert!(
                matches!(parse(src), Err(Error::Incomplete)),
                "want Incomplete for {src:?}, got {:?}",
                parse(src)
                    .map(|p| format!("{p:?}"))
                    .map_err(|e| e.to_string())
            );
        }
    }

    #[test]
    fn parse_errors() {
        for src in [
            "then",
            "echo hi )",
            "fi",
            "for 1; do done",
            "case x in ) a) b;; esac",
        ] {
            let r = parse(src);
            assert!(
                matches!(r, Err(Error::Parse { .. })),
                "want Parse error for {src:?}, got {:?}",
                r.map(|p| format!("{p:?}")).map_err(|e| e.to_string())
            );
        }
    }

    #[test]
    fn parse_errors_carry_the_span_the_caret_points_at() {
        let src = "echo hi\nfor x in | ; do echo; done";
        let Err(e) = parse(src) else {
            panic!("expected a parse error");
        };
        let span = e.span().expect("span");
        assert_eq!(&src[span.start..span.end], "|", "{src:?}");
    }

    #[test]
    fn closer_word_error_points_at_the_word() {
        let src = "echo hi\nfi";
        let Err(e) = parse(src) else {
            panic!("expected a parse error");
        };
        let span = e.span().expect("span");
        assert_eq!(&src[span.start..span.end], "fi");
    }

    #[test]
    fn empty_program() {
        let p = parse("").unwrap();
        assert!(p.items.is_empty());
    }

    #[test]
    fn comments_and_continuation() {
        let p = parse("echo a \\\nb # comment\necho c").unwrap();
        assert_eq!(p.items.len(), 2);
    }
}
