//! Terminal layer (GEN-013, 01 sections 3.3, 4 and 7.1).
//!
//!
//! Design: raw mode via termios (Unix) / console modes + VT (Windows); input decoded by a
//! byte-level escape-sequence parser; output via a cell buffer diffed against the previous
//! frame and flushed with ONE write per frame (UI-080, PERF-015).

use std::io;
use std::sync::Once;
use std::time::{Duration, Instant};

use crate::ui::panel::{char_width, Role};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    /// Ctrl + letter, lower-case letter given ('r' for Ctrl-R). Ctrl-Z is `Suspend`.
    Ctrl(char),
    Enter,
    Esc,
    Backspace,
    Delete,
    Tab,
    Up,
    Down,
    Left,
    Right,
    ShiftLeft,
    ShiftRight,
    PageUp,
    PageDown,
    Home,
    End,
    Insert,
    /// Function key number (F5 = F(5)).
    F(u8),
    Suspend,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Key(Key),
    /// New size (cols, rows).
    Resize(u16, u16),
    /// Deadline reached with no input.
    Timeout,
    /// SIGINT/SIGTERM/SIGHUP or console close: exit cleanly (UI-021/022).
    Quit,
    /// Resumed after SIGTSTP/SIGCONT: full redraw needed.
    Resumed,
}

/// SGR sequence per role, built by `ui::style` (palette depends on colour mode).
pub struct Palette {
    /// Full SGR string for each role, e.g. "\x1b[0;1;31m". Index = `Role as usize`.
    pub sgr: Vec<String>,
}

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
mod input;

/// Owns the tty: raw mode, alternate screen, signal wakeups.
pub struct Terminal {
    #[cfg(unix)]
    inner: unix::Tty,
    #[cfg(windows)]
    inner: windows::Console,
    // Windows translates console key records directly and never reads the byte decoder.
    #[cfg_attr(windows, allow(dead_code))]
    decoder: input::Decoder,
}

impl Terminal {
    /// Open the controlling terminal, enter raw mode + alternate screen, hide cursor,
    /// disable line wrap, install signal handlers / console handlers. Also installs a
    /// panic hook that restores the terminal (UI-021).
    pub fn open(escape_timeout: Duration) -> io::Result<Terminal> {
        #[cfg(unix)]
        let inner = unix::Tty::open()?;
        #[cfg(windows)]
        let inner = windows::Console::open()?;
        install_panic_hook();
        Ok(Terminal { inner, decoder: input::Decoder::new(escape_timeout) })
    }
    /// (cols, rows)
    pub fn size(&self) -> (u16, u16) {
        self.inner.size()
    }
    /// Block until input, resize, signal or timeout (`None` = no timeout). Exactly one
    /// blocking call (poll/WaitForMultipleObjects); no fixed tick (UI-025).
    pub fn wait(&mut self, timeout: Option<Duration>) -> io::Result<Event> {
        let deadline = timeout.map(|t| Instant::now() + t);
        loop {
            #[cfg(unix)]
            {
                // Signals first: quit, then resume, then one collapsed resize (UI-031).
                let sig = self.inner.take_signals();
                if sig != 0 {
                    let (bit, ev) = if sig & unix::SIG_QUIT != 0 {
                        (unix::SIG_QUIT, Event::Quit)
                    } else if sig & unix::SIG_CONT != 0 {
                        // Resumed after an external stop: re-enter the UI state (UI-023).
                        self.inner.enter()?;
                        (unix::SIG_CONT, Event::Resumed)
                    } else {
                        let (w, h) = self.inner.size();
                        (unix::SIG_WINCH, Event::Resize(w, h))
                    };
                    self.inner.requeue_signals(sig & !bit);
                    return Ok(ev);
                }
                let partial = match self.decoder.next() {
                    input::Step::Key(k) => return Ok(Event::Key(k)),
                    input::Step::Partial => true,
                    input::Step::Empty => false,
                };
                let remaining = deadline.map(|d| d.saturating_duration_since(Instant::now()));
                // UI-033: with a partial sequence buffered, wait at most the escape timeout.
                let esc = self.decoder.esc_timeout;
                let esc_due = partial && remaining.map_or(true, |r| esc <= r);
                let wait = if esc_due { Some(esc) } else { remaining };
                match self.inner.wait(wait, self.decoder.space())? {
                    unix::Raw::Input(n) => self.decoder.commit(n),
                    unix::Raw::Eof => return Ok(Event::Quit),
                    unix::Raw::Wake => {}
                    unix::Raw::Timeout => {
                        if esc_due {
                            if let Some(k) = self.decoder.expire() {
                                return Ok(Event::Key(k));
                            }
                        } else {
                            return Ok(Event::Timeout);
                        }
                    }
                }
            }
            #[cfg(windows)]
            {
                if let Some(ev) = self.inner.next_event() {
                    return Ok(ev);
                }
                let remaining = deadline.map(|d| d.saturating_duration_since(Instant::now()));
                if !self.inner.wait(remaining)? {
                    return Ok(Event::Timeout);
                }
            }
        }
    }
    /// Write raw bytes (one syscall per call; callers pass a whole frame).
    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.inner.write_all(bytes)
    }
    /// Restore terminal, raise SIGTSTP; on return re-enter UI mode (UI-023). No-op on Windows.
    /// The caller must invalidate the `Screen` afterwards.
    pub fn suspend(&mut self) -> io::Result<()> {
        #[cfg(unix)]
        self.inner.suspend()?;
        Ok(())
    }
    /// Restore terminal state. Idempotent. Also called from Drop and the panic hook.
    pub fn restore(&mut self) {
        restore_global();
    }
}

fn restore_global() {
    #[cfg(unix)]
    unix::restore_global();
    #[cfg(windows)]
    windows::restore_global();
}

/// UI-021: restore the terminal before the default hook prints the panic message.
/// Runs with `panic = "abort"` too, since the hook is called before aborting.
fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_global();
            prev(info);
        }));
    });
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.restore();
    }
}

/// One screen cell. `ch == '\0'` marks the right half of a wide glyph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub role: Role,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', role: Role::Default }
    }
}

/// Double-buffered cell grid with diff output (UI-080).
pub struct Screen {
    w: u16,
    h: u16,
    cur: Vec<Cell>,
    prev: Vec<Cell>,
    out: Vec<u8>,
    full: bool,
    /// Requested real-cursor position for this frame (None = hidden).
    cursor: Option<(u16, u16)>,
    /// Cursor state last sent to the terminal.
    shown: Option<(u16, u16)>,
}

impl Screen {
    pub fn new(w: u16, h: u16) -> Screen {
        let n = w as usize * h as usize;
        Screen { w, h, cur: vec![Cell::default(); n], prev: vec![Cell::default(); n], out: Vec::with_capacity(16 * 1024), full: true, cursor: None, shown: None }
    }
    pub fn size(&self) -> (u16, u16) {
        (self.w, self.h)
    }
    /// Resize buffers and force a full redraw.
    pub fn resize(&mut self, w: u16, h: u16) {
        let n = w as usize * h as usize;
        self.w = w;
        self.h = h;
        self.cur.clear();
        self.cur.resize(n, Cell::default());
        self.prev.clear();
        self.prev.resize(n, Cell::default());
        self.full = true;
    }
    /// Force the next flush to clear and repaint everything (F5, resume).
    pub fn invalidate(&mut self) {
        self.full = true;
    }
    /// Show the real terminal cursor at (x, y) after the next flush, or hide it (None).
    /// Escape sequences are emitted only when the state changes (or drawing moved it).
    pub fn set_cursor(&mut self, pos: Option<(u16, u16)>) {
        self.cursor = pos.filter(|&(x, y)| x < self.w && y < self.h);
    }
    /// Blank the back buffer (start of a frame).
    pub fn clear(&mut self) {
        self.cur.fill(Cell::default());
    }
    /// Draw `s` at (x, y) clipped to `max_x` (exclusive) and the screen. Returns the x
    /// after the last drawn cell. Wide chars that would straddle `max_x` are not drawn.
    pub fn put(&mut self, x: u16, y: u16, max_x: u16, s: &str, role: Role) -> u16 {
        let lim = max_x.min(self.w);
        let mut x = x;
        if y >= self.h {
            return x;
        }
        for c in s.chars() {
            if x >= lim {
                break;
            }
            if !self.put_char(&mut x, y, lim, c, role) {
                break;
            }
        }
        x
    }
    /// Fill `n` cells with `ch`.
    pub fn fill(&mut self, x: u16, y: u16, n: u16, ch: char, role: Role) {
        // Zero-width chars would never advance x.
        if y >= self.h || char_width(ch) == 0 {
            return;
        }
        let lim = x.saturating_add(n).min(self.w);
        let mut x = x;
        while x < lim && self.put_char(&mut x, y, lim, ch, role) {}
    }
    /// Diff against the previous frame, emit cursor moves + SGR changes + glyphs into one
    /// buffer, write it with one call, swap buffers.
    pub fn flush(&mut self, term: &mut Terminal, pal: &Palette) -> io::Result<()> {
        let w = self.w as usize;
        let full = self.full;
        let (cur, prev, out) = (&self.cur, &self.prev, &mut self.out);
        out.clear();
        // On a full redraw the screen is cleared to the reset state, so default blank cells
        // need no output when the default role's SGR is a plain reset.
        let skip_blank = full && matches!(pal.sgr.get(Role::Default as usize).map(String::as_str), Some("\x1b[0m" | "\x1b[m"));
        // UI-080: clear only on full redraw.
        let mut pos: Option<(usize, usize)> = None;
        if full {
            out.extend_from_slice(b"\x1b[0m\x1b[H\x1b[2J");
            pos = Some((0, 0));
        }
        let mut last: Option<Role> = None;
        for y in 0..self.h as usize {
            let row = y * w;
            let mut x = 0;
            while x < w {
                let i = row + x;
                let c = cur[i];
                if c.ch == '\0' {
                    // Stray continuation cell; its left half carries the glyph.
                    x += 1;
                    continue;
                }
                let wide = x + 1 < w && cur[i + 1].ch == '\0';
                let cells = 1 + wide as usize;
                let changed = if full {
                    !(skip_blank && c == Cell::default())
                } else {
                    c != prev[i] || (wide && cur[i + 1] != prev[i + 1])
                };
                if !changed {
                    x += cells;
                    continue;
                }
                // Minimal cursor motion: none when contiguous, re-emit a short same-role
                // ASCII gap, forward on the same row, else absolute.
                match pos {
                    Some((px, py)) if py == y && px == x => {}
                    Some((px, py)) if py == y && px < x => {
                        let gap = &cur[row + px..i];
                        let reuse = gap.len() <= 4
                            && last.is_some()
                            && gap.iter().all(|g| Some(g.role) == last && g.ch.is_ascii() && g.ch != '\0');
                        if reuse {
                            out.extend(gap.iter().map(|g| g.ch as u8));
                        } else {
                            out.extend_from_slice(b"\x1b[");
                            push_num(out, x - px);
                            out.push(b'C');
                        }
                    }
                    _ => {
                        out.extend_from_slice(b"\x1b[");
                        push_num(out, y + 1);
                        out.push(b';');
                        push_num(out, x + 1);
                        out.push(b'H');
                    }
                }
                // SGR only when the role changes.
                if last != Some(c.role) {
                    let sgr = pal.sgr.get(c.role as usize).map_or("\x1b[0m", String::as_str);
                    out.extend_from_slice(sgr.as_bytes());
                    last = Some(c.role);
                }
                let mut enc = [0u8; 4];
                out.extend_from_slice(c.ch.encode_utf8(&mut enc).as_bytes());
                x += cells;
                // With wrap off the cursor sticks at the last column: position unknown.
                pos = if x < w { Some((x, y)) } else { None };
            }
        }
        if last.is_some() {
            out.extend_from_slice(b"\x1b[0m");
        }
        // Real cursor (filter popup): re-position whenever drawing moved it; the terminal
        // starts hidden (ENTER sequence), and a full redraw follows re-entry after resume.
        match self.cursor {
            Some((cx, cy)) => {
                if full || !out.is_empty() || self.shown != self.cursor {
                    out.extend_from_slice(b"\x1b[");
                    push_num(out, cy as usize + 1);
                    out.push(b';');
                    push_num(out, cx as usize + 1);
                    out.push(b'H');
                    if full || self.shown.is_none() {
                        out.extend_from_slice(b"\x1b[?25h");
                    }
                }
            }
            None => {
                if self.shown.is_some() {
                    out.extend_from_slice(b"\x1b[?25l");
                }
            }
        }
        if !out.is_empty() {
            // PERF-015, UI-080: one write per frame.
            if let Err(e) = term.write_all(out) {
                self.full = true;
                return Err(e);
            }
        }
        self.prev.copy_from_slice(&self.cur);
        self.full = false;
        self.shown = self.cursor;
        Ok(())
    }

    /// Write one char at `*x` (clipped to `lim`), advancing `*x`. Returns false when the
    /// char does not fit (wide char at the edge).
    fn put_char(&mut self, x: &mut u16, y: u16, lim: u16, c: char, role: Role) -> bool {
        let u = c as u32;
        // Never send control characters to the terminal.
        let c = if u < 0x20 || (0x7f..0xa0).contains(&u) { '?' } else { c };
        let cw = char_width(c);
        if cw == 0 {
            return true;
        }
        let wide = cw >= 2;
        if wide && *x + 1 >= lim {
            return false;
        }
        let w = self.w as usize;
        let xs = *x as usize;
        let i = y as usize * w + xs;
        // Overwriting the right half of a wide glyph: blank its left half.
        if self.cur[i].ch == '\0' && xs > 0 {
            self.cur[i - 1].ch = ' ';
        }
        // Overwriting the left half of a wide glyph after us: blank its right half.
        let end = xs + wide as usize;
        if end + 1 < w && self.cur[i + wide as usize + 1].ch == '\0' {
            self.cur[i + wide as usize + 1].ch = ' ';
        }
        self.cur[i] = Cell { ch: c, role };
        if wide {
            self.cur[i + 1] = Cell { ch: '\0', role };
        }
        *x += cw as u16;
        true
    }
}

/// Append a decimal number without allocating.
fn push_num(out: &mut Vec<u8>, mut n: usize) {
    let mut b = [0u8; 20];
    let mut i = b.len();
    loop {
        i -= 1;
        b[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&b[i..]);
}
