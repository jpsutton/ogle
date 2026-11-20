//! Colour palette and glyph selection (01 section 7).

use crate::term::Palette;
use crate::ui::panel::Role;

/// Colour depth actually used (UI-086, UI-090).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    Mono,
    Color8,
    Color256,
}

/// Resolve the colour depth from the `[ui] color` value and the environment.
pub fn detect_depth(color: &str) -> Depth {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    // UI-090: "never", or NO_COLOR (unless --color=always) selects monochrome.
    if color == "never" || (color != "always" && !env("NO_COLOR").is_empty()) {
        return Depth::Mono;
    }
    // UI-086
    let auto = if cfg!(windows) {
        Depth::Color256
    } else {
        let term = env("TERM");
        let ct = env("COLORTERM");
        if ct == "truecolor" || ct == "24bit" || term.contains("256color") {
            Depth::Color256
        } else if term.is_empty() || term == "dumb" {
            Depth::Mono
        } else {
            Depth::Color8
        }
    };
    if color == "always" && auto == Depth::Mono {
        Depth::Color8
    } else {
        auto
    }
}

/// UI-081: true when the ASCII glyph set must be used. `unicode` is the `[ui] unicode` value.
pub fn ascii_glyphs(unicode: &str) -> bool {
    match unicode {
        "always" => false,
        "never" => true,
        _ => {
            if cfg!(windows) {
                return false;
            }
            for k in ["LC_ALL", "LC_CTYPE", "LANG"] {
                if let Ok(v) = std::env::var(k) {
                    if !v.is_empty() {
                        let v = v.to_ascii_lowercase();
                        return !(v.contains("utf-8") || v.contains("utf8"));
                    }
                }
            }
            true
        }
    }
}

// Colour numbers (SGR 30+n / 40+n).
const RED: u8 = 1;
const GREEN: u8 = 2;
const YELLOW: u8 = 3;
const BLUE: u8 = 4;
const MAGENTA: u8 = 5;
const CYAN: u8 = 6;
const WHITE: u8 = 7;

#[derive(Clone, Copy, Default)]
struct Style {
    fg: Option<u8>,
    /// 256-colour foreground index (overrides `fg` in 256-colour mode).
    fg256: Option<u8>,
    bg: Option<u8>,
    bold: bool,
    underline: bool,
    reverse: bool,
}

const fn st(fg: Option<u8>, bold: bool) -> Style {
    Style { fg, fg256: None, bg: None, bold, underline: false, reverse: false }
}

/// Colour-mode style of a role (UI-083, 8/16 and 256 columns).
fn color_style(r: Role, depth: Depth) -> Style {
    let c256 = depth == Depth::Color256;
    let log = |c: u8| Style { fg: Some(WHITE), bg: Some(c), bold: true, ..Style::default() };
    match r {
        Role::Default => Style::default(),
        Role::Title | Role::Bold => st(None, true),
        Role::Ok | Role::Process | Role::Status => st(Some(GREEN), false),
        Role::Careful | Role::Info => st(Some(BLUE), false),
        Role::Warning => st(Some(MAGENTA), true),
        Role::Critical => st(Some(RED), true),
        // UI-087: CAREFUL_LOG is white on blue in every depth.
        Role::OkLog => log(GREEN),
        Role::CarefulLog => log(BLUE),
        Role::WarningLog => log(MAGENTA),
        Role::CriticalLog => log(RED),
        Role::Max => st(Some(GREEN), true),
        Role::ProcessSelected => Style { underline: true, ..st(Some(GREEN), false) },
        Role::CpuTime => st(Some(MAGENTA), false),
        Role::Sort => Style { underline: true, ..st(None, true) },
        Role::Selected => st(if c256 { Some(YELLOW) } else { None }, true),
        Role::Filter => st(if c256 { Some(CYAN) } else { None }, true),
        Role::Error => st(Some(if c256 { YELLOW } else { RED }), true),
        Role::Separator => Style { fg256: if c256 { Some(244) } else { None }, ..Style::default() },
        Role::Underline => Style { underline: true, ..Style::default() },
    }
}

/// Monochrome style of a role (UI-083 monochrome column, UI-091).
fn mono_style(r: Role) -> Style {
    let (bold, underline, reverse) = match r {
        Role::Default | Role::Ok | Role::OkLog | Role::Process | Role::Status | Role::CpuTime | Role::Separator => {
            (false, false, false)
        }
        Role::Title | Role::Careful | Role::CarefulLog | Role::Max | Role::Selected | Role::Filter | Role::Info | Role::Bold => {
            (true, false, false)
        }
        Role::Warning | Role::ProcessSelected | Role::Underline => (false, true, false),
        Role::Critical | Role::Error => (false, false, true),
        Role::WarningLog | Role::Sort => (true, true, false),
        Role::CriticalLog => (true, false, true),
    };
    Style { bold, underline, reverse, ..Style::default() }
}

const ALL_ROLES: [Role; Role::COUNT] = [
    Role::Default,
    Role::Title,
    Role::Ok,
    Role::Careful,
    Role::Warning,
    Role::Critical,
    Role::OkLog,
    Role::CarefulLog,
    Role::WarningLog,
    Role::CriticalLog,
    Role::Max,
    Role::Process,
    Role::ProcessSelected,
    Role::Status,
    Role::CpuTime,
    Role::Sort,
    Role::Selected,
    Role::Filter,
    Role::Info,
    Role::Error,
    Role::Separator,
    Role::Bold,
    Role::Underline,
];

/// Build the SGR string for every role. `bold`/`background` are the `[ui]` flags.
pub fn palette(depth: Depth, bold: bool, background: bool) -> Palette {
    use std::fmt::Write;
    let mut sgr = Vec::with_capacity(Role::COUNT);
    for r in ALL_ROLES {
        let mut s = if depth == Depth::Mono { mono_style(r) } else { color_style(r, depth) };
        if depth != Depth::Mono {
            // UI-092: *_LOG roles keep their colour as foreground, lose the background.
            if !background {
                if let Some(bg) = s.bg.take() {
                    s.fg = Some(bg);
                }
            }
            // UI-093: no bold in colour modes.
            if !bold {
                s.bold = false;
            }
        }
        let mut out = String::from("\x1b[0");
        if s.bold {
            out.push_str(";1");
        }
        if s.underline {
            out.push_str(";4");
        }
        if s.reverse {
            out.push_str(";7");
        }
        match (depth, s.fg256, s.fg) {
            (Depth::Color256, Some(n), _) => {
                let _ = write!(out, ";38;5;{n}");
            }
            (Depth::Mono, _, _) => {}
            (_, _, Some(c)) => {
                let _ = write!(out, ";{}", 30 + c);
            }
            _ => {}
        }
        if depth != Depth::Mono {
            if let Some(b) = s.bg {
                let _ = write!(out, ";{}", 40 + b);
            }
        }
        out.push('m');
        sgr.push(out);
    }
    Palette { sgr }
}

/// Glyphs that depend on the glyph set (UI-081).
pub struct Glyphs {
    pub hline: char,
    pub vline: char,
    pub tl: char,
    pub tr: char,
    pub bl: char,
    pub br: char,
}

pub fn glyphs(ascii: bool) -> Glyphs {
    if ascii {
        Glyphs { hline: '-', vline: '|', tl: '+', tr: '+', bl: '+', br: '+' }
    } else {
        Glyphs { hline: '─', vline: '│', tl: '┌', tr: '┐', bl: '└', br: '┘' }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_in_order() {
        for (i, r) in ALL_ROLES.iter().enumerate() {
            assert_eq!(*r as usize, i);
        }
    }

    #[test]
    fn mono_has_no_colour() {
        let p = palette(Depth::Mono, true, true);
        for s in &p.sgr {
            let body = s.trim_start_matches("\x1b[").trim_end_matches('m');
            assert!(body.split(';').all(|c| matches!(c, "0" | "1" | "4" | "7")), "{s:?}");
        }
        assert_eq!(p.sgr[Role::Critical as usize], "\x1b[0;7m");
    }

    #[test]
    fn no_bg_keeps_fg() {
        let p = palette(Depth::Color8, true, false);
        assert_eq!(p.sgr[Role::CarefulLog as usize], "\x1b[0;1;34m");
        let p = palette(Depth::Color8, true, true);
        assert_eq!(p.sgr[Role::CarefulLog as usize], "\x1b[0;1;37;44m");
        assert_eq!(p.sgr[Role::Separator as usize], "\x1b[0m");
        let p = palette(Depth::Color256, false, true);
        assert_eq!(p.sgr[Role::Separator as usize], "\x1b[0;38;5;244m");
        assert_eq!(p.sgr[Role::Error as usize], "\x1b[0;33m");
    }
}
