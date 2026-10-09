//! Startup banner art (Catppuccin Mocha palette).
//! Shown on first init and when a version-check update is found.
//!
//! Respects `NO_COLOR` and non-tty stdout (no ANSI codes emitted).

use nu_ansi_term::{Color, Style};
use std::io::{IsTerminal, Write};

const BLUE: Color = Color::Rgb(137, 180, 250);
const PINK: Color = Color::Rgb(245, 194, 231);
const MAUVE: Color = Color::Rgb(203, 166, 247);
const WHITE: Color = Color::Rgb(205, 214, 244);

fn supports_color() -> bool {
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// Paint a single text segment. Returns plain text when color unsupported.
fn seg(s: &str, color: Color) -> String {
    if supports_color() {
        Style::new().fg(color).paint(s).to_string()
    } else {
        s.to_string()
    }
}

/// Concatenate colored segments into one line (no nesting issues).
fn line(segments: &[(&str, Color)]) -> String {
    segments.iter().map(|(s, c)| seg(s, *c)).collect()
}

/// Print the briSH startup banner to stdout.
/// Call on first init and when a version-check finds an update.
pub fn print_banner() {
    let version = env!("CARGO_PKG_VERSION");

    let face = vec![
        line(&[("          ✦", PINK)]),
        line(&[("             ▄██▄", BLUE)]),
        line(&[("            ██████", BLUE)]),
        line(&[("      ╭──────────────╮", MAUVE)]),
        line(&[
            ("     ╱ ", MAUVE),
            ("●", BLUE),
            (" ", MAUVE),
            ("●", MAUVE),
            (" ", MAUVE),
            ("●", PINK),
            ("          ╲", MAUVE),
        ]),
        line(&[("    │                │", MAUVE)]),
        line(&[("    │ ", MAUVE), (">_", BLUE), ("             │", MAUVE)]),
        line(&[
            ("    │   ", MAUVE),
            ("◕", WHITE),
            ("      ", MAUVE),
            ("◕", WHITE),
            ("     │", MAUVE),
        ]),
        line(&[("    │       ", MAUVE), ("ᴗ", PINK), ("        │", MAUVE)]),
        line(&[("     ╲______________╱", MAUVE)]),
        line(&[("        ╱╲    ╱╲", MAUVE)]),
    ];

    let header = line(&[
        ("       ✦ ", MAUVE),
        ("bri", BLUE),
        ("SH", PINK),
        (" ✦", MAUVE),
    ]);

    let version_line = line(&[("        - v", MAUVE), (version, MAUVE), (" -", MAUVE)]);

    let tagline = seg(r#"  "small tool, big vibes""#, WHITE);
    let footer = seg("          (˶ᵔ ᵕ ᵔ˶)", PINK);

    let mut out = String::new();
    out.push_str(&face.join("\n"));
    out.push_str("\n\n");
    out.push_str(&header);
    out.push('\n');
    out.push_str(&version_line);
    out.push_str("\n\n");
    out.push_str(&tagline);
    out.push('\n');
    out.push_str(&footer);
    out.push_str("\n\n");
    print!("{out}");
    std::io::stdout().flush().ok();
}
