//! Terminal layer (GEN-013, 01 sections 3.3, 4 and 7.1).
//!
//! OWNER: terminal agent fills bodies and writes `unix.rs`, `windows.rs`, `input.rs`.
//! Signatures here are a contract used by the app/UI agent.
//!
//! Design: raw mode via termios (Unix) / console modes + VT (Windows); input decoded by a
//! byte-level escape-sequence parser; output via a cell buffer diffed against the previous
//! frame and flushed with ONE write per frame (UI-080, PERF-015).

use std::io;
use std::time::Duration;

use crate::ui::panel::Role;

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
    decoder: input::Decoder,
}

impl Terminal {
    /// Open the controlling terminal, enter raw mode + alternate screen, hide cursor,
    /// disable line wrap, install signal handlers / console handlers. Also installs a
    /// panic hook that restores the terminal (UI-021).
    pub fn open(escape_timeout: Duration) -> io::Result<Terminal> {
        let _ = escape_timeout;
        todo!()
    }
    /// (cols, rows)
    pub fn size(&self) -> (u16, u16) {
        todo!()
    }
    /// Block until input, resize, signal or timeout (`None` = no timeout). Exactly one
    /// blocking call (poll/WaitForMultipleObjects); no fixed tick (UI-025).
    pub fn wait(&mut self, timeout: Option<Duration>) -> io::Result<Event> {
        let _ = timeout;
        todo!()
    }
    /// Write raw bytes (one syscall per call; callers pass a whole frame).
    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        let _ = bytes;
        todo!()
    }
    /// Restore terminal, raise SIGTSTP; on return re-enter UI mode (UI-023). No-op on Windows.
    pub fn suspend(&mut self) -> io::Result<()> {
        todo!()
    }
    /// Restore terminal state. Idempotent. Also called from Drop and the panic hook.
    pub fn restore(&mut self) {
        todo!()
    }
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
}

impl Screen {
    pub fn new(w: u16, h: u16) -> Screen {
        let n = w as usize * h as usize;
        Screen { w, h, cur: vec![Cell::default(); n], prev: vec![Cell::default(); n], out: Vec::with_capacity(16 * 1024), full: true }
    }
    pub fn size(&self) -> (u16, u16) {
        (self.w, self.h)
    }
    /// Resize buffers and force a full redraw.
    pub fn resize(&mut self, w: u16, h: u16) {
        let _ = (w, h);
        todo!()
    }
    /// Force the next flush to clear and repaint everything (F5, resume).
    pub fn invalidate(&mut self) {
        self.full = true;
    }
    /// Blank the back buffer (start of a frame).
    pub fn clear(&mut self) {
        todo!()
    }
    /// Draw `s` at (x, y) clipped to `max_x` (exclusive) and the screen. Returns the x
    /// after the last drawn cell. Wide chars that would straddle `max_x` are not drawn.
    pub fn put(&mut self, x: u16, y: u16, max_x: u16, s: &str, role: Role) -> u16 {
        let _ = (x, y, max_x, s, role);
        todo!()
    }
    /// Fill `n` cells with `ch`.
    pub fn fill(&mut self, x: u16, y: u16, n: u16, ch: char, role: Role) {
        let _ = (x, y, n, ch, role);
        todo!()
    }
    /// Diff against the previous frame, emit cursor moves + SGR changes + glyphs into one
    /// buffer, write it with one call, swap buffers.
    pub fn flush(&mut self, term: &mut Terminal, pal: &Palette) -> io::Result<()> {
        let _ = (term, pal);
        todo!()
    }
}
