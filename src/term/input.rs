//! Byte-level input decoder (01 section 4, UI-032..UI-035).
//!
//! Bytes are read straight into the decoder's fixed buffer (no per-read allocation) and
//! turned into `Key`s. Partial sequences stay buffered; the caller waits at most
//! `esc_timeout` for the rest and then calls `expire()` (UI-033).
//!
//! On Windows the console delivers key records, so this decoder is only used on Unix.
#![cfg_attr(windows, allow(dead_code))]

use std::time::Duration;

use super::Key;

const CAP: usize = 256;

pub struct Decoder {
    buf: [u8; CAP],
    len: usize,
    pub esc_timeout: Duration,
}

pub enum Step {
    Key(Key),
    /// Bytes buffered that may be the start of a longer sequence.
    Partial,
    Empty,
}

/// Result of parsing the front of the buffer.
#[derive(Debug, PartialEq, Eq)]
enum Parse {
    Key(Key, usize),
    /// Unknown or malformed: drop this many bytes as a whole (UI-034).
    Drop(usize),
    Need,
}

impl Decoder {
    pub fn new(esc_timeout: Duration) -> Decoder {
        Decoder { buf: [0; CAP], len: 0, esc_timeout }
    }

    /// Free space to read new bytes into; follow with `commit(n)`.
    pub fn space(&mut self) -> &mut [u8] {
        &mut self.buf[self.len..]
    }

    pub fn commit(&mut self, n: usize) {
        self.len = (self.len + n).min(CAP);
    }

    pub fn next(&mut self) -> Step {
        loop {
            if self.len == 0 {
                return Step::Empty;
            }
            match parse(&self.buf[..self.len]) {
                Parse::Key(k, n) => {
                    self.consume(n);
                    return Step::Key(k);
                }
                Parse::Drop(n) => self.consume(n.max(1)),
                Parse::Need => {
                    if self.len == CAP {
                        // A "sequence" longer than the buffer is garbage: drop it all.
                        self.len = 0;
                        return Step::Empty;
                    }
                    return Step::Partial;
                }
            }
        }
    }

    /// The escape timeout passed with a partial sequence still buffered (UI-033).
    /// A lone ESC is the Esc key; any other incomplete sequence is dropped whole.
    pub fn expire(&mut self) -> Option<Key> {
        let lone_esc = self.len == 1 && self.buf[0] == 0x1b;
        self.len = 0;
        if lone_esc {
            Some(Key::Esc)
        } else {
            None
        }
    }

    fn consume(&mut self, n: usize) {
        let n = n.min(self.len);
        self.buf.copy_within(n..self.len, 0);
        self.len -= n;
    }
}

fn parse(b: &[u8]) -> Parse {
    let c = b[0];
    match c {
        0x1b => parse_esc(b),
        b'\r' | b'\n' => Parse::Key(Key::Enter, 1),
        b'\t' => Parse::Key(Key::Tab, 1),
        0x7f | 0x08 => Parse::Key(Key::Backspace, 1),
        // UI-023: Ctrl-Z suspends.
        0x1a => Parse::Key(Key::Suspend, 1),
        0x01..=0x19 => Parse::Key(Key::Ctrl((b'a' + c - 1) as char), 1),
        0x00 | 0x1c..=0x1f => Parse::Drop(1),
        0x20..=0x7e => Parse::Key(Key::Char(c as char), 1),
        _ => parse_utf8(b),
    }
}

fn parse_utf8(b: &[u8]) -> Parse {
    let n = match b[0] {
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Parse::Drop(1),
    };
    let have = b.len().min(n);
    if b[1..have].iter().any(|&x| !(0x80..=0xbf).contains(&x)) {
        return Parse::Drop(1);
    }
    if b.len() < n {
        return Parse::Need;
    }
    match std::str::from_utf8(&b[..n]).ok().and_then(|s| s.chars().next()) {
        // C1 controls are not printable.
        Some(ch) if (ch as u32) >= 0xa0 => Parse::Key(Key::Char(ch), n),
        Some(_) => Parse::Drop(n),
        None => Parse::Drop(1),
    }
}

fn parse_esc(b: &[u8]) -> Parse {
    if b.len() == 1 {
        return Parse::Need;
    }
    match b[1] {
        b'[' => parse_csi(b),
        b'O' => parse_ss3(b),
        // Alt+key: no Alt key variant exists, so the pair is consumed as a whole so the
        // character does not leak through as a separate key press (UI-034).
        0x20..=0x7e => Parse::Drop(2),
        // ESC followed by a control byte, another ESC or a UTF-8 lead: the first ESC is
        // the Esc key, the rest is decoded on its own.
        _ => Parse::Key(Key::Esc, 1),
    }
}

/// `ESC [` params intermediates final.
fn parse_csi(b: &[u8]) -> Parse {
    if b.len() < 3 {
        return Parse::Need;
    }
    // Linux console F1-F5: `ESC [ [ A` .. `ESC [ [ E`.
    if b[2] == b'[' {
        if b.len() < 4 {
            return Parse::Need;
        }
        return match b[3] {
            b'A'..=b'E' => Parse::Key(Key::F(b[3] - b'A' + 1), 4),
            0x40..=0x7e => Parse::Drop(4),
            _ => Parse::Drop(3),
        };
    }
    let mut p = [0u32; 4];
    let mut np = 0usize;
    let mut odd = false; // private marker or intermediate byte
    let mut i = 2;
    while i < b.len() {
        let c = b[i];
        match c {
            b'0'..=b'9' => {
                if np < p.len() {
                    p[np] = p[np].saturating_mul(10).saturating_add((c - b'0') as u32);
                }
            }
            b';' | b':' => np += 1,
            0x3c..=0x3f | 0x20..=0x2f => odd = true,
            0x40..=0x7e => {
                return match csi_key(c, p[0], p[1]) {
                    Some(k) if !odd => Parse::Key(k, i + 1),
                    _ => Parse::Drop(i + 1),
                };
            }
            // Malformed: drop what we have; the offending byte is decoded on its own.
            _ => return Parse::Drop(i),
        }
        i += 1;
    }
    Parse::Need
}

/// `mods` is the xterm modifier parameter (1 + bitmask: 1 Shift, 2 Alt, 4 Ctrl).
fn csi_key(fin: u8, p1: u32, mods: u32) -> Option<Key> {
    let shift = mods >= 2 && (mods - 1) & 1 != 0;
    Some(match fin {
        b'A' => Key::Up,
        b'B' => Key::Down,
        // `ESC [1;2C` / `ESC [1;2D`: Shift-Right / Shift-Left.
        b'C' if shift => Key::ShiftRight,
        b'D' if shift => Key::ShiftLeft,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        b'P'..=b'S' => Key::F(fin - b'P' + 1),
        b'~' => match p1 {
            1 | 7 => Key::Home,
            4 | 8 => Key::End,
            2 => Key::Insert,
            3 => Key::Delete,
            5 => Key::PageUp,
            6 => Key::PageDown,
            11..=15 => Key::F((p1 - 10) as u8),
            17..=21 => Key::F((p1 - 11) as u8),
            23 | 24 => Key::F((p1 - 12) as u8),
            _ => return None,
        },
        _ => return None,
    })
}

/// `ESC O` [modifier digits] final.
fn parse_ss3(b: &[u8]) -> Parse {
    let mut mods = 0u32;
    let mut i = 2;
    while i < b.len() {
        let c = b[i];
        match c {
            b'0'..=b'9' => mods = mods.saturating_mul(10).saturating_add((c - b'0') as u32),
            b';' => {}
            0x40..=0x7e => {
                return match ss3_key(c, mods) {
                    Some(k) => Parse::Key(k, i + 1),
                    None => Parse::Drop(i + 1),
                };
            }
            _ => return Parse::Drop(i),
        }
        i += 1;
    }
    Parse::Need
}

fn ss3_key(fin: u8, mods: u32) -> Option<Key> {
    Some(match fin {
        b'A'..=b'H' | b'P'..=b'S' => return csi_key(fin, 1, mods),
        // UI-035: keypad Enter.
        b'M' => Key::Enter,
        // Application keypad digits and operators.
        b'p'..=b'y' => Key::Char((b'0' + fin - b'p') as char),
        b'j' => Key::Char('*'),
        b'k' => Key::Char('+'),
        b'l' => Key::Char(','),
        b'm' => Key::Char('-'),
        b'n' => Key::Char('.'),
        b'o' => Key::Char('/'),
        b'X' => Key::Char('='),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(input: &[u8]) -> (Vec<Key>, bool) {
        let mut d = Decoder::new(Duration::from_millis(30));
        let mut out = Vec::new();
        let mut partial = false;
        for chunk in input.chunks(7) {
            d.space()[..chunk.len()].copy_from_slice(chunk);
            d.commit(chunk.len());
            loop {
                match d.next() {
                    Step::Key(k) => out.push(k),
                    Step::Partial => {
                        partial = true;
                        break;
                    }
                    Step::Empty => {
                        partial = false;
                        break;
                    }
                }
            }
        }
        (out, partial)
    }

    #[test]
    fn basics() {
        let (k, _) = keys(b"aB\r\n\t\x7f\x08\x03\x1a");
        assert_eq!(
            k,
            vec![
                Key::Char('a'),
                Key::Char('B'),
                Key::Enter,
                Key::Enter,
                Key::Tab,
                Key::Backspace,
                Key::Backspace,
                Key::Ctrl('c'),
                Key::Suspend
            ]
        );
    }

    #[test]
    fn sequences() {
        let (k, p) = keys(b"\x1b[A\x1bOB\x1b[1;2D\x1b[1;2C\x1b[5~\x1b[6~\x1b[1~\x1b[4~\x1b[H\x1bOF\x1b[15~\x1b[24~\x1bOP\x1bOM\x1b[[A\x1b[3~");
        assert!(!p);
        assert_eq!(
            k,
            vec![
                Key::Up,
                Key::Down,
                Key::ShiftLeft,
                Key::ShiftRight,
                Key::PageUp,
                Key::PageDown,
                Key::Home,
                Key::End,
                Key::Home,
                Key::End,
                Key::F(5),
                Key::F(12),
                Key::F(1),
                Key::Enter,
                Key::F(1),
                Key::Delete
            ]
        );
    }

    #[test]
    fn unknown_dropped_whole() {
        // UI-034: unknown sequences must not leak 'A'/'B' or digits.
        let (k, _) = keys(b"\x1b[99~\x1b[?1;2Zx\x1b[200~q");
        assert_eq!(k, vec![Key::Char('x'), Key::Char('q')]);
    }

    #[test]
    fn utf8_and_partial() {
        let (k, p) = keys("é─\u{1F600}".as_bytes());
        assert!(!p);
        assert_eq!(k, vec![Key::Char('é'), Key::Char('─'), Key::Char('\u{1F600}')]);
        let mut d = Decoder::new(Duration::from_millis(30));
        d.space()[0] = 0x1b;
        d.commit(1);
        assert!(matches!(d.next(), Step::Partial));
        assert_eq!(d.expire(), Some(Key::Esc));
        d.space()[..2].copy_from_slice(b"\x1b[");
        d.commit(2);
        assert!(matches!(d.next(), Step::Partial));
        assert_eq!(d.expire(), None);
    }
}
