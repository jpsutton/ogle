//! Styled-text panel buffer shared by every module renderer (spec 01 section 6).
//!
//! Allocation discipline: a `Panel` keeps one text arena and one span vector. `clear()`
//! keeps capacity, so after warm-up rendering allocates nothing.

use std::fmt::{self, Write as _};

/// Colour roles (01-runtime-ui.md section 7.2). The palette lives in `ui::style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum Role {
    #[default]
    Default,
    Title,
    Ok,
    Careful,
    Warning,
    Critical,
    OkLog,
    CarefulLog,
    WarningLog,
    CriticalLog,
    Max,
    Process,
    ProcessSelected,
    Status,
    CpuTime,
    Sort,
    Selected,
    Filter,
    Info,
    Error,
    Separator,
    Bold,
    Underline,
}

impl Role {
    pub const COUNT: usize = Role::Underline as usize + 1;
}

/// Span flags (01 section 6.3).
pub mod flags {
    /// Dropped when the layout is narrow (UI "optional").
    pub const OPTIONAL: u8 = 1;
    /// May be truncated at the right edge instead of being skipped whole.
    pub const SPLITTABLE: u8 = 2;
    /// Process "additional" columns (hidden on some platforms).
    pub const ADDITIONAL: u8 = 4;
}

#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub start: u32,
    pub len: u32,
    pub role: Role,
    pub flags: u8,
    /// Display width in terminal cells (precomputed).
    pub width: u16,
}

/// One line = range of spans.
#[derive(Clone, Copy, Debug)]
pub struct LineRef {
    pub first_span: u32,
    pub n_spans: u32,
}

#[derive(Default)]
pub struct Panel {
    text: String,
    spans: Vec<Span>,
    lines: Vec<LineRef>,
}

/// Alignment hint returned with a rendered panel (01 section 6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Align {
    #[default]
    Top,
    /// Placed at the bottom of its region (now, alert).
    Bottom,
    /// Right-aligned on its row (uptime).
    Right,
}

impl Panel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.spans.clear();
        self.lines.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.spans.is_empty()
    }

    fn ensure_line(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(LineRef { first_span: self.spans.len() as u32, n_spans: 0 });
        }
    }

    /// Start a new line. An empty panel starts with line 0 implicitly.
    pub fn newline(&mut self) {
        self.ensure_line();
        self.lines.push(LineRef { first_span: self.spans.len() as u32, n_spans: 0 });
    }

    /// Append literal text.
    pub fn add(&mut self, role: Role, flags: u8, s: &str) {
        self.ensure_line();
        let start = self.text.len() as u32;
        self.text.push_str(s);
        self.push_span(start, role, flags);
    }

    /// Append formatted text without a temporary String.
    pub fn addf(&mut self, role: Role, flags: u8, args: fmt::Arguments<'_>) {
        self.ensure_line();
        let start = self.text.len() as u32;
        let _ = self.text.write_fmt(args);
        self.push_span(start, role, flags);
    }

    fn push_span(&mut self, start: u32, role: Role, flags: u8) {
        let len = self.text.len() as u32 - start;
        if len == 0 {
            return;
        }
        let width = str_width(&self.text[start as usize..]) as u16;
        self.spans.push(Span { start, len, role, flags, width });
        if let Some(l) = self.lines.last_mut() {
            l.n_spans += 1;
        }
    }

    pub fn n_lines(&self) -> usize {
        self.lines.len()
    }

    pub fn line(&self, i: usize) -> &[Span] {
        let l = self.lines[i];
        &self.spans[l.first_span as usize..(l.first_span + l.n_spans) as usize]
    }

    pub fn span_text(&self, s: &Span) -> &str {
        &self.text[s.start as usize..(s.start + s.len) as usize]
    }

    /// Width of line `i` in cells, counting spans with flags in `skip` as absent.
    pub fn line_width(&self, i: usize, skip: u8) -> usize {
        self.line(i).iter().filter(|s| s.flags & skip == 0).map(|s| s.width as usize).sum()
    }

    /// Max line width, skipping spans with any flag in `skip`.
    pub fn width(&self, skip: u8) -> usize {
        (0..self.n_lines()).map(|i| self.line_width(i, skip)).max().unwrap_or(0)
    }
}

/// Terminal cell width of a string. ogle treats every char as width 1 except
/// East Asian wide/fullwidth ranges (2) and combining marks / zero-width (0).
pub fn str_width(s: &str) -> usize {
    if s.is_ascii() {
        return s.len();
    }
    s.chars().map(char_width).sum()
}

pub fn char_width(c: char) -> usize {
    let u = c as u32;
    if u < 0x300 {
        return 1;
    }
    if (0x300..0x370).contains(&u) || (0x200B..=0x200F).contains(&u) || u == 0xFE0F {
        return 0;
    }
    if (0x1100..=0x115F).contains(&u)
        || (0x2E80..=0xA4CF).contains(&u)
        || (0xAC00..=0xD7A3).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFE30..=0xFE4F).contains(&u)
        || (0xFF00..=0xFF60).contains(&u)
        || (0xFFE0..=0xFFE6).contains(&u)
        || (0x1F300..=0x1FAFF).contains(&u)
        || (0x20000..=0x3FFFD).contains(&u)
    {
        return 2;
    }
    1
}

/// `panel_addf!(p, role, flags, "fmt", args...)`
#[macro_export]
macro_rules! addf {
    ($p:expr, $role:expr, $flags:expr, $($arg:tt)*) => {
        $p.addf($role, $flags, format_args!($($arg)*))
    };
}
