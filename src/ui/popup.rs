//! Popups (01 section 8): the filter line editor and the kill confirmation.

use crate::modules::processes::Selection;
use crate::term::{Key, Screen};
use crate::ui::layout::blit;
use crate::ui::panel::{str_width, Panel, Role};
use crate::ui::style::glyphs;

/// Width of the filter input field in cells (UI-103).
const FIELD: usize = 40;
/// Maximum filter length in characters (UI-103).
const MAX_CHARS: usize = 256;

/// Single-line text editor used by the filter popup.
pub struct LineEditor {
    pub text: String,
    /// Cursor as a byte offset (always on a char boundary).
    cur: usize,
    /// First visible character index.
    scroll: usize,
}

pub enum EditResult {
    /// Key consumed (text or cursor may have changed).
    Edited,
    Apply,
    Cancel,
    /// Not an editing key.
    Ignored,
}

impl LineEditor {
    pub fn new(init: &str) -> LineEditor {
        let mut e = LineEditor { text: init.to_string(), cur: init.len(), scroll: 0 };
        e.fix_scroll();
        e
    }

    fn prev(&self) -> usize {
        self.text[..self.cur].char_indices().next_back().map_or(0, |(i, _)| i)
    }

    fn next(&self) -> usize {
        self.text[self.cur..].chars().next().map_or(self.cur, |c| self.cur + c.len_utf8())
    }

    fn cursor_char(&self) -> usize {
        self.text[..self.cur].chars().count()
    }

    fn fix_scroll(&mut self) {
        let ci = self.cursor_char();
        if ci < self.scroll {
            self.scroll = ci;
        } else if ci >= self.scroll + FIELD {
            self.scroll = ci + 1 - FIELD;
        }
    }

    /// UI-103 editing keys.
    pub fn key(&mut self, k: Key) -> EditResult {
        match k {
            Key::Enter => return EditResult::Apply,
            Key::Esc => return EditResult::Cancel,
            Key::Char(c) if !c.is_control() => {
                if self.text.chars().count() < MAX_CHARS {
                    self.text.insert(self.cur, c);
                    self.cur += c.len_utf8();
                }
            }
            Key::Left | Key::Ctrl('b') => self.cur = self.prev(),
            Key::Right | Key::Ctrl('f') => self.cur = self.next(),
            Key::Home | Key::Ctrl('a') => self.cur = 0,
            Key::End | Key::Ctrl('e') => self.cur = self.text.len(),
            Key::Backspace | Key::Ctrl('h') => {
                let p = self.prev();
                self.text.replace_range(p..self.cur, "");
                self.cur = p;
            }
            Key::Delete | Key::Ctrl('d') => {
                let n = self.next();
                self.text.replace_range(self.cur..n, "");
            }
            Key::Ctrl('u') => {
                self.text.replace_range(..self.cur, "");
                self.cur = 0;
            }
            Key::Ctrl('k') => self.text.truncate(self.cur),
            Key::Ctrl('w') => {
                let head = &self.text[..self.cur];
                let trimmed = head.trim_end_matches(' ');
                let start = trimmed.rfind(' ').map_or(0, |i| i + 1);
                self.text.replace_range(start..self.cur, "");
                self.cur = start;
            }
            _ => return EditResult::Ignored,
        }
        self.fix_scroll();
        EditResult::Edited
    }
}

pub enum Popup {
    Filter { ed: LineEditor, error: String },
    Kill { sel: Selection, content: Panel },
}

/// Build the kill popup for a selection (UI-104). Allocates once, when the popup opens.
pub fn kill_popup(sel: Selection, program: bool) -> Popup {
    let mut p = Panel::new();
    match (program, sel.targets.first()) {
        (false, Some(&(pid, _))) => p.addf(Role::Default, 0, format_args!("Kill process: {} (pid {})?", sel.name, pid)),
        _ => p.addf(Role::Default, 0, format_args!("Kill program: {} ({} processes)?", sel.name, sel.targets.len())),
    }
    p.newline();
    p.newline();
    if cfg!(windows) {
        p.add(Role::Default, 0, "[t/y] terminate   [n/Esc] cancel");
    } else {
        p.add(Role::Default, 0, "[t/y] terminate (SIGTERM)   [K] kill (SIGKILL)   [n/Esc] cancel");
    }
    Popup::Kill { sel, content: p }
}

const FILTER_HEAD: &str = "Process filter pattern:";
const FILTER_TAIL: [&str; 9] = [
    "Examples:",
    "  python            name or command line contains \"python\"",
    "  ^/usr/lib         command line starts with /usr/lib",
    "  name:^nginx$      name is exactly nginx",
    "  cmdline:--config  match on the full command line only",
    "  username:^root$   match on the user name",
    "  pid:^12           PID starts with 12",
    "",
    "Enter: apply   Esc: cancel   empty: clear filter",
];

/// Draw a popup over the frame. `scratch` is a reusable panel for the filter content.
/// Returns where the real terminal cursor goes (filter field), for `Screen::set_cursor`.
pub fn draw(scr: &mut Screen, popup: &Popup, scratch: &mut Panel, ascii: bool) -> Option<(u16, u16)> {
    match popup {
        Popup::Kill { content, .. } => {
            draw_box(scr, content, ascii);
            None
        }
        Popup::Filter { ed, error } => {
            scratch.clear();
            scratch.add(Role::Default, 0, FILTER_HEAD);
            scratch.newline();
            // Placeholder for the input field "[" + 40 cells + "]".
            scratch.addf(Role::Default, 0, format_args!("{:1$}", "", FIELD + 2));
            scratch.newline();
            if !error.is_empty() {
                scratch.add(Role::Error, 0, error);
            }
            for l in FILTER_TAIL {
                scratch.newline();
                if !l.is_empty() {
                    scratch.add(Role::Default, 0, l);
                }
            }
            let (cx, cy, cw, ch) = draw_box(scr, scratch, ascii);
            if ch >= 2 && cw >= 3 {
                draw_field(scr, ed, cx, cy + 1, cx + cw)
            } else {
                None
            }
        }
    }
}

/// Field with sideways scrolling. Returns the cursor cell when it is inside the field.
fn draw_field(scr: &mut Screen, ed: &LineEditor, x: u16, y: u16, max_x: u16) -> Option<(u16, u16)> {
    let fx = x + 1;
    let end = (fx + FIELD as u16).min(max_x.saturating_sub(1));
    scr.put(x, y, max_x, "[", Role::Default);
    scr.fill(fx, y, end.saturating_sub(fx), '_', Role::Default);
    scr.put(end, y, max_x, "]", Role::Default);
    let start = ed.text.char_indices().nth(ed.scroll).map_or(ed.text.len(), |(i, _)| i);
    scr.put(fx, y, end, &ed.text[start..], Role::Filter);
    // Display width, not char count, so wide glyphs place the cursor correctly.
    let col = fx as usize + str_width(&ed.text[start..ed.cur.max(start)]);
    if col < end as usize {
        Some((col as u16, y))
    } else {
        None
    }
}

/// UI-100: centred box sized to the content, clamped to the screen. Returns the content
/// rectangle (x, y, width, height).
pub fn draw_box(scr: &mut Screen, content: &Panel, ascii: bool) -> (u16, u16, u16, u16) {
    let (sw, sh) = scr.size();
    let bw = (content.width(0) + 4).min(sw as usize) as u16;
    let bh = (content.n_lines() + 4).min(sh as usize) as u16;
    if bw < 2 || bh < 2 {
        return (0, 0, 0, 0);
    }
    let x0 = (sw - bw) / 2;
    let y0 = (sh - bh) / 2;
    let g = glyphs(ascii);
    for r in 1..bh - 1 {
        scr.fill(x0, y0 + r, bw, ' ', Role::Default);
        scr.fill(x0, y0 + r, 1, g.vline, Role::Separator);
        scr.fill(x0 + bw - 1, y0 + r, 1, g.vline, Role::Separator);
    }
    scr.fill(x0, y0, bw, g.hline, Role::Separator);
    scr.fill(x0, y0 + bh - 1, bw, g.hline, Role::Separator);
    scr.fill(x0, y0, 1, g.tl, Role::Separator);
    scr.fill(x0 + bw - 1, y0, 1, g.tr, Role::Separator);
    scr.fill(x0, y0 + bh - 1, 1, g.bl, Role::Separator);
    scr.fill(x0 + bw - 1, y0 + bh - 1, 1, g.br, Role::Separator);
    let (cw, ch) = (bw.saturating_sub(4), bh.saturating_sub(4));
    blit(scr, content, 0, x0 + 2, y0 + 2, cw, ch, 0, true);
    (x0 + 2, y0 + 2, cw, ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(s: &str) -> LineEditor {
        let mut e = LineEditor::new("");
        for c in s.chars() {
            e.key(Key::Char(c));
        }
        e
    }

    #[test]
    fn editing() {
        let mut e = typed("foo bar");
        e.key(Key::Ctrl('w'));
        assert_eq!(e.text, "foo ");
        e.key(Key::Home);
        e.key(Key::Delete);
        assert_eq!(e.text, "oo ");
        e.key(Key::End);
        e.key(Key::Backspace);
        assert_eq!(e.text, "oo");
        e.key(Key::Left);
        e.key(Key::Ctrl('k'));
        assert_eq!(e.text, "o");
        e.key(Key::Char('é'));
        e.key(Key::Left);
        e.key(Key::Ctrl('u'));
        assert_eq!(e.text, "é");
    }

    #[test]
    fn length_cap_and_scroll() {
        let e = typed(&"x".repeat(300));
        assert_eq!(e.text.len(), MAX_CHARS);
        assert_eq!(e.scroll, MAX_CHARS + 1 - FIELD);
    }
}
