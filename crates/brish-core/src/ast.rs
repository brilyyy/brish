//! Shell AST: immutable IR produced by the parser, consumed by the
//! expander and execution engine. Every node keeps source spans where
//! diagnostics may point.

use crate::lexer::{Span, Word};

/// A parsed script: sequence of items separated by `;`, `&` or newline.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    pub items: Vec<Item>,
}

/// One and-or list with optional background marker.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub andor: AndOr,
    pub background: bool,
}

/// `pipeline && pipeline || pipeline ...`
#[derive(Debug, Clone, PartialEq)]
pub struct AndOr {
    pub first: Pipeline,
    pub rest: Vec<(AndOrOp, Pipeline)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AndOrOp {
    And,
    Or,
}

/// `[!] cmd | cmd | ...`
#[derive(Debug, Clone, PartialEq)]
pub struct Pipeline {
    pub cmds: Vec<Cmd>,
    pub negated: bool,
}

/// Any command construct.
#[derive(Debug, Clone, PartialEq)]
pub enum Cmd {
    Simple(Simple),
    /// `((expr))` — evaluate, status 0 iff non-zero.
    Arith(String),
    Subshell(Program),
    Group(Program),
    If {
        cond: Program,
        then: Program,
        elif: Vec<(Program, Program)>,
        els: Option<Program>,
    },
    Loop {
        cond: Program,
        body: Program,
        until: bool,
    },
    For {
        var: String,
        words: Option<Vec<Word>>,
        body: Program,
    },
    Case {
        word: Word,
        arms: Vec<CaseArm>,
    },
    FuncDef {
        name: String,
        body: Box<Cmd>,
    },
    /// Trailing redirections on a compound command: `(...) > f`.
    Redirected {
        inner: Box<Cmd>,
        redirs: Vec<Redir>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct CaseArm {
    pub pats: Vec<Word>,
    pub body: Program,
}

/// Simple command: `var=val ... cmd args ... redirs`.
#[derive(Debug, Clone, PartialEq)]
pub struct Simple {
    pub assigns: Vec<Assign>,
    pub redirs: Vec<Redir>,
    pub words: Vec<Word>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Assign {
    pub name: String,
    pub value: Word,
    pub span: Span,
}

/// Redirection; `fd` is the explicit IO number or the kind's default.
#[derive(Debug, Clone, PartialEq)]
pub enum Redir {
    /// `< target`
    Input { fd: usize, target: Word },
    /// `> target`
    Output { fd: usize, target: Word },
    /// `>> target`
    Append { fd: usize, target: Word },
    /// `|> target`
    Clobber { fd: usize, target: Word },
    /// `<& n|-`
    DupIn { fd: usize, target: Word },
    /// `>& n|-`
    DupOut { fd: usize, target: Word },
    /// `<<[-]DELIM` with body captured at parse time.
    Heredoc {
        fd: usize,
        text: String,
        expand: bool,
    },
    /// `<<<word` — expanded word plus a trailing newline.
    HereString { fd: usize, target: Word },
}

impl Redir {
    /// The fd this redirection writes to (for `2>&1`-style resolution).
    pub fn fd(&self) -> usize {
        match self {
            Redir::Input { fd, .. } | Redir::DupIn { fd, .. } | Redir::HereString { fd, .. } => *fd,
            Redir::Output { fd, .. }
            | Redir::Append { fd, .. }
            | Redir::Clobber { fd, .. }
            | Redir::DupOut { fd, .. }
            | Redir::Heredoc { fd, .. } => *fd,
        }
    }
}
