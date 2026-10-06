//! Aligned-column rendering for shell builtin lists (NOTES.md 3).
//!
//! `auto` aligns when ≤ 3 columns and every row fits ~100 cols;
//! `always` always aligns; `off` prints space-joined rows.

use crate::ucfg::{TableMode, load};

/// Render rows as aligned columns per `[output] table` mode.
/// Every row must have the same column count.
pub fn render(rows: &[Vec<String>]) -> String {
    render_with(rows, load().table)
}

/// Same, with an explicit mode (tests).
pub fn render_with(rows: &[Vec<String>], mode: TableMode) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let cols = rows[0].len();
    let align = match mode {
        TableMode::Always => true,
        TableMode::Off => false,
        TableMode::Auto => cols <= 3 && rows.iter().all(|r| joined_width(r) <= 100),
    };
    if !align {
        return rows
            .iter()
            .map(|r| r.join(" "))
            .collect::<Vec<_>>()
            .join("\n");
    }
    let mut widths = vec![0usize; cols];
    for r in rows {
        for (i, cell) in r.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for (n, r) in rows.iter().enumerate() {
        if n > 0 {
            out.push('\n');
        }
        for (i, cell) in r.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            if i + 1 == cols {
                out.push_str(cell); // last col: no pad (trailing spaces)
            } else {
                out.push_str(cell);
                let pad = widths[i] - cell.chars().count();
                out.extend(std::iter::repeat_n(' ', pad));
            }
        }
    }
    out
}

fn joined_width(r: &[String]) -> usize {
    r.iter().map(|c| c.chars().count()).sum::<usize>() + r.len().saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_joins_with_spaces() {
        let rows = vec![
            vec!["plugin".into(), "on".into()],
            vec!["other".into(), "off".into()],
        ];
        assert_eq!(render_with(&rows, TableMode::Off), "plugin on\nother off");
    }

    #[test]
    fn always_aligns_columns() {
        let rows = vec![
            vec!["a".into(), "1".into()],
            vec!["bbbb".into(), "22".into()],
        ];
        assert_eq!(render_with(&rows, TableMode::Always), "a    1\nbbbb 22");
    }

    #[test]
    fn auto_skips_wide_or_many_columns() {
        let wide = vec![vec!["x".repeat(200), "1".into()]];
        assert_eq!(
            render_with(&wide, TableMode::Auto),
            format!("{} 1", "x".repeat(200))
        );
        let four = vec![vec!["a".into(), "b".into(), "c".into(), "d".into()]];
        assert_eq!(render_with(&four, TableMode::Auto), "a b c d");
    }

    #[test]
    fn empty_is_empty() {
        assert_eq!(render_with(&[], TableMode::Always), "");
    }
}
