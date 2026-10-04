//! Syntax highlighting for the interactive REPL (plan §5.1), wired by
//! the config-gated `syntax-highlight` plugin.
//!
//! Token-level coloring over the real lexer: keywords, builtins,
//! strings, `$vars`, substitutions, operators/redirections, comments.
//! Incomplete input (unterminated `$(`, quote) falls back to the
//! longest prefix that lexes, with the dangling tail marked as an
//! unclosed token. The styled buffer always concatenates back to the
//! exact input line.

use brish_core::lexer::{self, Op, Part, Tok, Token, Word};
use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};

/// POSIX/bash reserved words, colored at word level.
const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "while", "until", "do", "done", "for", "in", "case",
    "esac", "function", "select", "time",
];

/// How far we walk back from an unlexable tail before giving up.
const MAX_TRIM: usize = 128;

pub struct BrishHighlighter;

impl Highlighter for BrishHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let color = brish_plugin::color_enabled();
        let mut out = StyledText::new();
        let (tokens, incomplete) = lex_best(line);
        let mut pos = 0usize;
        for t in &tokens {
            let start = t.span.start.min(line.len());
            let end = t.span.end.clamp(start, line.len());
            if start > pos {
                gap(&mut out, &line[pos..start], color);
            }
            if end > start {
                let style = style_for(&t.tok, &line[start..end], color);
                out.push((style, line[start..end].to_string()));
            }
            pos = pos.max(end);
        }
        if pos < line.len() {
            tail(&mut out, &line[pos..], incomplete, color);
        }
        out
    }
}

/// Plugin that installs the built-in syntax highlighter into the registry.
pub struct SyntaxHighlightPlugin;

impl brish_plugin::Plugin for SyntaxHighlightPlugin {
    fn name(&self) -> &str {
        "syntax-highlight"
    }

    fn install(&self, reg: &mut brish_plugin::Registry) {
        struct F;
        impl brish_plugin::HighlighterFactory for F {
            fn create(&self) -> Box<dyn reedline::Highlighter> {
                Box::new(BrishHighlighter)
            }
        }
        reg.highlighter_factories.push(Box::new(F));
    }
}

/// Lex `line`, falling back to the longest suffix-trimmed prefix when
/// the input is incomplete (`Error::Incomplete`) or otherwise unlexable.
fn lex_best(line: &str) -> (Vec<Token>, bool) {
    match lexer::lex(line) {
        Ok(l) => (l.tokens, false),
        Err(_) => {
            let mut end = line.len();
            let mut best = Vec::new();
            for _ in 0..MAX_TRIM {
                if end == 0 {
                    break;
                }
                end = prev_boundary(line, end);
                if let Ok(l) = lexer::lex(&line[..end]) {
                    best = l.tokens;
                    break;
                }
            }
            (best, true)
        }
    }
}

/// Largest byte offset `< i` that is a char boundary.
fn prev_boundary(s: &str, mut i: usize) -> usize {
    i -= 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn style_for(tok: &Tok, src: &str, color: bool) -> Style {
    if !color {
        return Style::new();
    }
    match tok {
        Tok::Op(op) => match op {
            Op::Pipe
            | Op::And
            | Op::Or
            | Op::Semi
            | Op::Dsemi
            | Op::Amp
            | Op::LParen
            | Op::RParen => Style::new().fg(Color::Purple),
            _ => Style::new().fg(Color::Purple).bold(), // redirections
        },
        Tok::IoNumber(_) => Style::new().fg(Color::Purple).bold(),
        Tok::ArithCmd(_) => Style::new().fg(Color::Yellow),
        Tok::Newline => Style::new(),
        Tok::Word(w) => word_style(w, src),
    }
}

fn word_style(w: &Word, src: &str) -> Style {
    match w.parts.first() {
        Some(Part::Single(_)) | Some(Part::Double(_)) => Style::new().fg(Color::Green),
        Some(Part::Param(_)) => Style::new().fg(Color::Cyan),
        Some(Part::Subst(_)) | Some(Part::Arith(_)) => Style::new().fg(Color::Yellow),
        _ => {
            // Plain word: keyword vs builtin (a plain word's source is
            // exactly the word text; quotes/escapes keep it a command).
            if plain(src) {
                if KEYWORDS.contains(&src) {
                    Style::new().fg(Color::Blue).bold()
                } else if crate::completion::is_builtin(src) {
                    Style::new().fg(Color::Cyan).bold()
                } else {
                    Style::new()
                }
            } else {
                Style::new()
            }
        }
    }
}

/// True when `src` is a bare name (no quotes, metacharacters, `$`).
fn plain(src: &str) -> bool {
    !src.is_empty() && src.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// Gaps between tokens: whitespace, newlines, comments. A `#` starts a
/// comment only when everything before it on its line is whitespace
/// (heredoc bodies and `a#b` stay plain).
fn gap(out: &mut StyledText, text: &str, color: bool) {
    let mut rest = text;
    while !rest.is_empty() {
        let (head, after) = match rest.find('\n') {
            Some(i) => (&rest[..i], Some(&rest[i..])),
            None => (rest, None),
        };
        let comment_at = head
            .find('#')
            .filter(|&i| head[..i].chars().all(char::is_whitespace));
        match comment_at {
            Some(i) => {
                push_plain(out, &head[..i]);
                if color && i < head.len() {
                    out.push((Style::new().fg(Color::DarkGray), head[i..].to_string()));
                } else {
                    push_plain(out, &head[i..]);
                }
            }
            None => push_plain(out, head),
        }
        match after {
            Some(a) => {
                push_plain(out, "\n");
                rest = &a[1..];
            }
            None => rest = "",
        }
    }
}

/// Final gap: when the line didn't lex (incomplete input), mark from
/// the first dangling quote to the end of its line as unclosed.
fn tail(out: &mut StyledText, text: &str, incomplete: bool, color: bool) {
    if incomplete
        && color
        && let Some(i) = text.find(['"', '\''])
    {
        gap(out, &text[..i], color);
        let rest = &text[i..];
        match rest.find('\n') {
            Some(j) => {
                out.push((Style::new().fg(Color::Red).bold(), rest[..j].to_string()));
                gap(out, &rest[j..], color);
            }
            None => out.push((Style::new().fg(Color::Red).bold(), rest.to_string())),
        }
        return;
    }
    gap(out, text, color);
}

fn push_plain(out: &mut StyledText, text: &str) {
    if !text.is_empty() {
        out.push((Style::new(), text.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(line: &str) -> StyledText {
        BrishHighlighter.highlight(line, 0)
    }

    fn joined(st: &StyledText) -> String {
        st.buffer.iter().map(|(_, s)| s.as_str()).collect()
    }

    fn style_of<'a>(st: &'a StyledText, text: &str) -> Option<&'a Style> {
        st.buffer.iter().find(|(_, s)| s == text).map(|(s, _)| s)
    }

    #[test]
    fn reconstruction_is_lossless() {
        for line in [
            "echo hi",
            "echo \"a b\" | grep $x",
            "a # comment",
            "",
            "\n",
            "cat <<EOF\nbody # not comment\nEOF\n",
            "echo 'unterminated",
            "echo \"open $(sub",
            "for i in {1..3}; do echo $i; done",
            "2>&1 foo=bar",
            "(a; b) && c || d",
            "héllo wörld",
        ] {
            let st = render(line);
            assert_eq!(joined(&st), line, "lossy for {line:?}");
        }
    }

    #[test]
    fn keywords_and_strings_get_distinct_styles() {
        let st = render("if true; then echo \"s\"; fi");
        let keyword = Style::new().fg(Color::Blue).bold();
        let builtin = Style::new().fg(Color::Cyan).bold();
        let string = Style::new().fg(Color::Green);
        assert_eq!(style_of(&st, "if"), Some(&keyword));
        assert_eq!(style_of(&st, "echo"), Some(&builtin));
        assert_eq!(style_of(&st, "\"s\""), Some(&string));
        // `true`/`then` are... then = keyword
        assert_eq!(style_of(&st, "then"), Some(&keyword));
    }

    #[test]
    fn comments_and_vars() {
        let st = render("echo $HOME # where");
        let comment = Style::new().fg(Color::DarkGray);
        assert_eq!(style_of(&st, "# where"), Some(&comment));
        // unquoted param word is a var; a quoted one is a string
        let st = render("echo $HOME");
        assert_eq!(style_of(&st, "$HOME"), Some(&Style::new().fg(Color::Cyan)));
        let st = render("printf '%s' \"$USER\"");
        let string = Style::new().fg(Color::Green);
        assert_eq!(style_of(&st, "'%s'"), Some(&string));
        assert_eq!(style_of(&st, "\"$USER\""), Some(&string));
        // `#` mid-word is not a comment (stays a plain word)
        let st = render("echo a#b");
        assert_eq!(style_of(&st, "a#b"), Some(&Style::new()));
        assert_eq!(joined(&st), "echo a#b");
    }

    #[test]
    fn incomplete_quote_marks_unclosed_tail() {
        let st = render("echo \"abc");
        let unclosed = Style::new().fg(Color::Red).bold();
        assert_eq!(style_of(&st, "\"abc"), Some(&unclosed));
        assert_eq!(joined(&st), "echo \"abc");
    }

    #[test]
    fn operators_and_redirections() {
        let st = render("a | b > c 2>&1");
        let op = Style::new().fg(Color::Purple);
        let redir = Style::new().fg(Color::Purple).bold();
        assert_eq!(style_of(&st, "|"), Some(&op));
        assert_eq!(style_of(&st, ">"), Some(&redir));
        // `2>&1` lexes as IoNumber + op + word, all redir-colored
        assert_eq!(style_of(&st, "2"), Some(&redir));
        assert_eq!(style_of(&st, ">&"), Some(&redir));
    }

    #[test]
    fn prev_boundary_steps_back_one_char() {
        assert_eq!(prev_boundary("ab", 2), 1);
        assert_eq!(prev_boundary("héllo", "héllo".len()), "héll".len());
        assert_eq!(prev_boundary("x", 1), 0);
    }
}
