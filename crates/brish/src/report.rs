//! Error reporting: message + optional source excerpt with a caret.
//!
//! `brish-core` stays dependency-free, so the caret rendering lives
//! here in the binary. Three styles (nushell's `error_style` spirit):
//!
//! | Style | Output |
//! |---|---|
//! | [`Style::Fancy`] | `brish: parse error: …` + source line + caret (interactive default) |
//! | [`Style::Short`] | `brish: parse error: …` (batch default; unchanged POSIX-ish text) |
//! | [`Style::Plain`] | `parse error: …` (no `brish:` prefix, no source) |
//!
//! No ANSI: the caret is plain text, so `NO_COLOR` needs no handling.
//! ponytail: byte columns (a tab shifts the caret visually); a
//! tab-aware column is a fix if anyone actually complains.

use brish_core::error::Error;

/// Longest source excerpt shown in fancy mode.
const MAX_EXCERPT: usize = 120;

/// How much context an error message carries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    /// Message only, no `brish:` prefix, no source.
    Plain,
    /// `brish: <msg>` — the batch default, so script output and the
    /// `dash` cross-check in `tests/posix.rs` do not move.
    #[default]
    Short,
    /// Message + the offending source line + a caret.
    Fancy,
}

impl Style {
    /// `[errors] style` value; unknown names fall back to `default`.
    pub fn from_name(name: &str, default: Style) -> Style {
        match name.trim().to_ascii_lowercase().as_str() {
            "fancy" => Style::Fancy,
            "short" => Style::Short,
            "plain" => Style::Plain,
            _ => default,
        }
    }
}

/// 1-based `(line, column)` of `off` and the text of that line.
/// `None` when `src` is empty or `off` is out of bounds.
pub fn locate(src: &str, off: usize) -> Option<(usize, usize, &str)> {
    if src.is_empty() || off > src.len() {
        return None;
    }
    let head = &src[..off];
    let line = head.bytes().filter(|b| *b == b'\n').count() + 1;
    let line_start = head.rfind('\n').map_or(0, |i| i + 1);
    let line_end = src[line_start..]
        .find('\n')
        .map_or(src.len(), |i| line_start + i);
    Some((line, off - line_start + 1, &src[line_start..line_end]))
}

/// Excerpt around the column, trimmed to [`MAX_EXCERPT`]. Returns the
/// (possibly cropped) text and the caret column *within it* — the
/// caller keeps the original column for the `(line, col)` footer.
fn excerpt(text: &str, col: usize) -> (String, usize) {
    if text.len() <= MAX_EXCERPT {
        return (text.replace('\t', " "), col);
    }
    // Keep ~40 columns of lead-in. Cuts are pulled back to a char
    // boundary so a UTF-8 line never gets sliced mid-char.
    let start = (0..=col.saturating_sub(40))
        .rev()
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(0);
    let mut end = (start + MAX_EXCERPT).min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let caret = col.saturating_sub(start).max(1);
    (text[start..end].replace('\t', " "), caret)
}

/// Render one fatal error. `src` is the unit that failed (whole file,
/// `-c` string, or accumulated REPL buffer).
pub fn render(src: &str, err: &Error, style: Style) -> String {
    let msg = err.to_string();
    let head = match style {
        Style::Plain => msg,
        _ => format!("brish: {msg}"),
    };
    if style != Style::Fancy {
        return head;
    }
    match err.span().and_then(|sp| locate(src, sp.start)) {
        Some((line, col, text)) => {
            let (text, caret) = excerpt(text, col);
            let num = line.to_string();
            let pad = " ".repeat(num.len() + 3);
            format!(
                "{head}\n{pad}{num} | {text}\n{pad}  | {}^\n  (line {line}, col {col})",
                " ".repeat(caret - 1)
            )
        }
        None => head,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brish_core::lexer::Span;

    /// Zero-width span at `off` — what a caller would build when it
    /// knows a position but no error variant carries one yet.
    fn at(off: usize) -> Span {
        Span {
            start: off,
            end: off,
        }
    }

    const SRC: &str = "echo hi\nls | fi\npwd\n";

    #[test]
    fn locate_reports_one_based_line_and_column() {
        // `fi` starts at byte 13 on line 2
        let (line, col, text) = locate(SRC, 13).expect("locate");
        assert_eq!((line, col, text), (2, 6, "ls | fi"));
    }

    #[test]
    fn locate_handles_start_and_end() {
        assert_eq!(locate(SRC, 0).map(|(l, c, _)| (l, c)), Some((1, 1)));
        assert_eq!(
            locate(SRC, SRC.len() - 1).map(|(l, c, _)| (l, c)),
            Some((3, 4))
        );
        assert!(locate("", 0).is_none());
        assert!(locate(SRC, SRC.len() + 5).is_none());
    }

    #[test]
    fn fancy_prints_excerpt_and_caret_under_the_span() {
        let e = Error::parse_at("unexpected `fi'", at(13));
        let out = render(SRC, &e, Style::Fancy);
        assert_eq!(
            out,
            concat!(
                "brish: parse error: unexpected `fi'\n",
                "    2 | ls | fi\n",
                "      |      ^\n",
                "  (line 2, col 6)"
            )
        );
    }

    #[test]
    fn short_is_the_batch_default_and_plain_drops_the_prefix() {
        let e = Error::parse_at("boom", at(13));
        assert_eq!(render(SRC, &e, Style::Short), "brish: parse error: boom");
        assert_eq!(render(SRC, &e, Style::Plain), "parse error: boom");
    }

    #[test]
    fn fancy_falls_back_to_a_bare_message_without_a_span() {
        let e = Error::parse("bad substitution");
        assert_eq!(
            render(SRC, &e, Style::Fancy),
            "brish: parse error: bad substitution"
        );
    }

    #[test]
    fn long_lines_are_cropped_around_the_caret() {
        let long = format!("{}{}", "x".repeat(400), "!boom");
        let off = long.len() - 4; // the `!`
        let e = Error::parse_at("unexpected", at(off));
        let out = render(&long, &e, Style::Fancy);
        let line = out.lines().nth(1).expect("excerpt line");
        assert!(line.len() < 200, "excerpt cropped: {line}");
        // footer keeps the ORIGINAL column, the caret the cropped one
        assert!(out.contains("(line 1, col 402)"), "{out}");
        let caret = out.lines().nth(2).expect("caret line");
        let at_col = caret.find('^').expect("caret");
        let text_at = line.find("!boom").expect("cropped anchor");
        assert_eq!(at_col - text_at, 1, "caret sits under the `!':\n{out}");
    }

    #[test]
    fn style_names_parse_and_unknown_falls_back() {
        assert_eq!(Style::from_name("Fancy", Style::Short), Style::Fancy);
        assert_eq!(Style::from_name(" short ", Style::Fancy), Style::Short);
        assert_eq!(Style::from_name("plain", Style::Short), Style::Plain);
        assert_eq!(Style::from_name("nope", Style::Fancy), Style::Fancy);
    }
}
