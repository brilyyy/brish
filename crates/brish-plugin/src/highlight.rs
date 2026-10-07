//! Syntax highlighting for the interactive REPL (plan §5.1), wired by
//! the config-gated `brish-syntax-highlight` plugin.
//!
//! Static (always): token-level coloring over the real lexer —
//! keywords, builtins, strings, `$vars`, substitutions,
//! operators/redirections, comments.
//!
//! Dynamic (`[highlight] dynamic = true`, default; zsh-patina
//! reference): command words are cyan when resolvable (builtin / alias
//! / `$PATH` / executable) and **red when missing**; non-command words
//! that name an existing file/dir are **underlined**. Disabled above
//! [`MAX_DYNAMIC_LEN`] bytes (patina's `max_line_length` idea).
//!
//! Incomplete input falls back to the longest prefix that lexes, with
//! the dangling tail marked unclosed. The styled buffer always
//! concatenates back to the exact input line.

use brish_core::lexer::{self, Op, Part, Tok, Token, Word};
use nu_ansi_term::{Color, Style};
use reedline::{Highlighter, StyledText};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// POSIX/bash reserved words, colored at word level.
const KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "while", "until", "do", "done", "for", "in", "case",
    "esac", "function", "select", "time",
];

/// Precommands: the *next* word is still a callable (patina
/// `highlighting.precommands`, mode=default subset).
const PRECOMMANDS: &[&str] = &[
    "sudo", "env", "nohup", "nice", "command", "exec", "builtin", "doas", "strace",
];

/// Skip dynamic lookups beyond this many bytes (patina `max_line_length`).
const MAX_DYNAMIC_LEN: usize = 2000;

/// How far we walk back from an unlexable tail before giving up.
const MAX_TRIM: usize = 128;

pub struct BrishHighlighter {
    /// `[highlight] dynamic` (default true).
    pub dynamic: bool,
    /// Live alias names — refreshed by the REPL each prompt (shared
    /// with completion's var snapshot pattern).
    pub aliases: Arc<Mutex<Vec<String>>>,
    /// `$PATH` command-existence cache: name → found.
    cmd_cache: Mutex<HashMap<String, bool>>,
}

impl BrishHighlighter {
    pub fn new(dynamic: bool, aliases: Arc<Mutex<Vec<String>>>) -> Self {
        Self {
            dynamic,
            aliases,
            cmd_cache: Mutex::new(HashMap::new()),
        }
    }

    fn known_command(&self, name: &str) -> bool {
        if crate::completion::is_builtin(name) {
            return true;
        }
        if let Ok(map) = self.cmd_cache.lock()
            && let Some(hit) = map.get(name)
        {
            return *hit;
        }
        let found = if name.contains('/') {
            Path::new(name).is_file()
        } else {
            brish_core::path::find_in_path(name, std::env::var("PATH").ok().as_deref()).is_some()
        };
        if let Ok(mut map) = self.cmd_cache.lock() {
            // ponytail: unbounded cache; a shell session adds a few
            // dozen names. Ceiling: cap at 4k entries + clear on PATH change.
            map.insert(name.to_string(), found);
        }
        found
    }

    fn is_alias(&self, name: &str) -> bool {
        self.aliases
            .lock()
            .map(|a| a.iter().any(|x| x == name))
            .unwrap_or(false)
    }
}

impl Highlighter for BrishHighlighter {
    fn highlight(&self, line: &str, _cursor: usize) -> StyledText {
        let color = brish_plugin_api::color_enabled();
        let dyn_on = color && self.dynamic && line.len() <= MAX_DYNAMIC_LEN;
        let mut out = StyledText::new();
        let (tokens, incomplete) = lex_best(line);
        let mut pos = 0usize;
        // Command-position state: true at line start and after
        // ; && || | ( newline; stays true through a precommand word.
        let mut cmd_pos = true;
        for t in &tokens {
            let start = t.span.start.min(line.len());
            let end = t.span.end.clamp(start, line.len());
            if start > pos {
                gap(&mut out, &line[pos..start], color);
            }
            if end > start {
                let src = &line[start..end];
                let style = match &t.tok {
                    Tok::Word(w) => {
                        let s = self.word_style(w, src, cmd_pos, dyn_on);
                        if dyn_on && plain(src) && cmd_pos && PRECOMMANDS.contains(&src) {
                            // precommand: next word is still a callable
                        } else {
                            cmd_pos = false;
                        }
                        s
                    }
                    Tok::Op(op) => {
                        if matches!(
                            op,
                            Op::Pipe | Op::And | Op::Or | Op::Semi | Op::Amp | Op::LParen
                        ) {
                            cmd_pos = true;
                        }
                        style_for(&t.tok, src, color)
                    }
                    Tok::Newline => {
                        cmd_pos = true;
                        Style::new()
                    }
                    _ => style_for(&t.tok, src, color),
                };
                out.push((style, src.to_string()));
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
pub struct SyntaxHighlightPlugin {
    /// Shared alias snapshot (see [`BrishHighlighter::aliases`]).
    pub aliases: Arc<Mutex<Vec<String>>>,
    /// `[highlight] dynamic`.
    pub dynamic: bool,
}

impl brish_plugin_api::Plugin for SyntaxHighlightPlugin {
    fn name(&self) -> &str {
        "brish-syntax-highlight"
    }

    fn install(&self, reg: &mut brish_plugin_api::Registry) {
        let aliases = Arc::clone(&self.aliases);
        let dynamic = self.dynamic;
        struct F {
            aliases: Arc<Mutex<Vec<String>>>,
            dynamic: bool,
        }
        impl brish_plugin_api::HighlighterFactory for F {
            fn create(&self) -> Box<dyn reedline::Highlighter> {
                Box::new(BrishHighlighter::new(
                    self.dynamic,
                    Arc::clone(&self.aliases),
                ))
            }
        }
        reg.highlighter_factories
            .push(Box::new(F { aliases, dynamic }));
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

fn style_for(tok: &Tok, _src: &str, color: bool) -> Style {
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
        Tok::Word(_) => Style::new(), // words go through word_style
    }
}

fn word_style_base(w: &Word, src: &str) -> Option<Style> {
    match w.parts.first() {
        Some(Part::Single(_)) | Some(Part::Double(_)) => Some(Style::new().fg(Color::Green)),
        Some(Part::Param(_)) => Some(Style::new().fg(Color::Cyan)),
        Some(Part::Subst(_)) | Some(Part::Arith(_)) => Some(Style::new().fg(Color::Yellow)),
        _ => None,
    }
    .or_else(|| {
        if plain(src) {
            if KEYWORDS.contains(&src) {
                Some(Style::new().fg(Color::Blue).bold())
            } else {
                None
            }
        } else {
            None
        }
    })
}

impl BrishHighlighter {
    fn word_style(&self, w: &Word, src: &str, cmd_pos: bool, dyn_on: bool) -> Style {
        if let Some(base) = word_style_base(w, src) {
            return base;
        }
        // Plain (or mixed) word, no keyword/string/param marker.
        if !plain(src) {
            return Style::new();
        }
        if crate::completion::is_builtin(src) {
            return Style::new().fg(Color::Cyan).bold();
        }
        if !dyn_on {
            return Style::new();
        }
        if cmd_pos {
            if self.is_alias(src) || self.known_command(src) {
                Style::new().fg(Color::Cyan)
            } else {
                // dynamic.callable.missing
                Style::new().fg(Color::Red).bold()
            }
        } else if Path::new(src).exists() {
            // dynamic.path
            Style::new().underline()
        } else {
            Style::new()
        }
    }
}

/// True when `src` is a bare name (no quotes, metacharacters, `$`).
fn plain(src: &str) -> bool {
    !src.is_empty()
        && src
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
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

/// Final gap: when the line didn'tlex (incomplete input), mark from
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

    fn hl() -> BrishHighlighter {
        BrishHighlighter::new(true, Arc::new(Mutex::new(Vec::new())))
    }

    fn render(line: &str) -> StyledText {
        hl().highlight(line, 0)
    }

    fn joined(st: &StyledText) -> String {
        st.buffer.iter().map(|(_, s)| s.as_str()).collect()
    }

    fn style_of<'a>(st: &'a StyledText, text: &str) -> Option<&'a Style> {
        st.buffer.iter().find(|(_, s)| s == text).map(|(s, _)| s)
    }
    /// Expected style: colored when colors are on, plain under `NO_COLOR`.
    fn want(fg: Color, bold: bool) -> Style {
        if !brish_plugin_api::color_enabled() {
            return Style::new();
        }
        let s = Style::new().fg(fg);
        if bold { s.bold() } else { s }
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
        let keyword = want(Color::Blue, true);
        let builtin = want(Color::Cyan, true);
        let string = want(Color::Green, false);
        assert_eq!(style_of(&st, "if"), Some(&keyword));
        assert_eq!(style_of(&st, "echo"), Some(&builtin));
        assert_eq!(style_of(&st, "\"s\""), Some(&string));
        assert_eq!(style_of(&st, "then"), Some(&keyword));
    }

    #[test]
    fn dynamic_missing_command_is_red_existing_file_underlined() {
        if !brish_plugin_api::color_enabled() {
            return;
        }
        // missing first word → red bold (dynamic.callable.missing)
        let st = render("definitely-not-a-cmd-xyz");
        assert_eq!(
            style_of(&st, "definitely-not-a-cmd-xyz"),
            Some(&Style::new().fg(Color::Red).bold())
        );
        // builtin first word stays cyan bold even with dynamic on
        let st = render("echo");
        assert_eq!(style_of(&st, "echo"), Some(&want(Color::Cyan, true)));
        // existing file in argument position → underline
        let st = render("cat README.md"); // README.md exists at repo root cwd
        if Path::new("README.md").exists() {
            assert_eq!(style_of(&st, "README.md"), Some(&Style::new().underline()));
        }
        // non-existent argument stays plain
        let st = render("cat definitely-no-such-file-xyz");
        assert_eq!(
            style_of(&st, "definitely-no-such-file-xyz"),
            Some(&Style::new())
        );
    }

    #[test]
    fn aliases_count_as_known_and_precommands_keep_command_pos() {
        if !brish_plugin_api::color_enabled() {
            return;
        }
        let aliases = Arc::new(Mutex::new(vec!["myalias".to_string()]));
        let h = BrishHighlighter::new(true, aliases);
        let st = h.highlight("myalias", 0);
        assert_eq!(style_of(&st, "myalias"), Some(&want(Color::Cyan, false)));
        // sudo is a precommand → next word is still a callable (red if missing)
        let st = h.highlight("sudo definitely-not-a-cmd-xyz", 0);
        assert_eq!(
            style_of(&st, "definitely-not-a-cmd-xyz"),
            Some(&Style::new().fg(Color::Red).bold())
        );
        // after a plain command, args are not callables → missing word plain
        let st = h.highlight("echo definitely-not-a-cmd-xyz", 0);
        assert_eq!(
            style_of(&st, "definitely-not-a-cmd-xyz"),
            Some(&Style::new())
        );
    }

    #[test]
    fn dynamic_off_keeps_static_behavior() {
        let h = BrishHighlighter::new(false, Arc::new(Mutex::new(Vec::new())));
        let st = h.highlight("definitely-not-a-cmd-xyz", 0);
        assert_eq!(
            style_of(&st, "definitely-not-a-cmd-xyz"),
            Some(&Style::new())
        );
    }

    #[test]
    fn comments_and_vars() {
        let st = render("echo $HOME # where");
        let comment = want(Color::DarkGray, false);
        assert_eq!(style_of(&st, "# where"), Some(&comment));
        let st = render("echo $HOME");
        assert_eq!(style_of(&st, "$HOME"), Some(&want(Color::Cyan, false)));
        let st = render("printf '%s' \"$USER\"");
        let string = want(Color::Green, false);
        assert_eq!(style_of(&st, "'%s'"), Some(&string));
        assert_eq!(style_of(&st, "\"$USER\""), Some(&string));
        let st = render("echo a#b");
        assert_eq!(style_of(&st, "a#b"), Some(&Style::new()));
        assert_eq!(joined(&st), "echo a#b");
    }

    #[test]
    fn incomplete_quote_marks_unclosed_tail() {
        let st = render("echo \"abc");
        let (style, text) = st.buffer.last().unwrap();
        assert!(text.trim_start().ends_with("\"abc"), "{text:?}");
        assert_eq!(*style, want(Color::Red, true));
        assert_eq!(joined(&st), "echo \"abc");
    }

    #[test]
    fn operators_and_redirections() {
        let st = render("a | b > c 2>&1");
        let op = want(Color::Purple, false);
        let redir = want(Color::Purple, true);
        assert_eq!(style_of(&st, "|"), Some(&op));
        assert_eq!(style_of(&st, ">"), Some(&redir));
        assert_eq!(style_of(&st, "2"), Some(&redir));
        assert_eq!(style_of(&st, ">&"), Some(&redir));
    }

    #[test]
    fn prev_boundary_steps_back_one_char() {
        assert_eq!(prev_boundary("ab", 2), 1);
        assert_eq!(prev_boundary("héllo", "héllo".len()), "héll".len());
        assert_eq!(prev_boundary("x", 1), 0);
    }

    /// REPL keystroke-latency smoke (plan §9: typical line render
    /// < 50ms). 2k renders of realistic lines must stay far under that.
    #[test]
    fn highlight_latency_smoke() {
        let h = BrishHighlighter::new(true, Arc::new(Mutex::new(Vec::new())));
        let lines: Vec<String> = (0..2000)
            .map(|i| {
                if i % 2 == 0 {
                    format!(
                        "for i in {{1..{}}}; do echo \"$i\" | grep {} > /tmp/out_{}; done",
                        i % 20 + 1,
                        "$HOME",
                        i
                    )
                } else {
                    format!(
                        "command ls -la /var/tmp/path with spaces/{} 2>&1 && echo 'ok'",
                        i
                    )
                }
            })
            .collect();
        let start = std::time::Instant::now();
        for l in &lines {
            let _ = h.highlight(l, 0);
        }
        let total = start.elapsed();
        // ~µs per render; 50ms is the per-keystroke budget (plan §9).
        assert!(
            total.as_millis() < 500,
            "2000 renders took {:?} — per-line budget blown",
            total
        );
        eprintln!("highlight: {:?} / 2000 renders", total);
    }
}
