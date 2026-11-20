//! Shared number formatting and history helpers (02-system-modules.md SYS-001..SYS-00x).
//!
//! OWNER: system-modules agent fills `todo!()` bodies. Signatures are a contract.
//! All formatting returns stack-allocated `Small` strings: no heap allocation per cell.

use std::fmt;

/// Fixed-capacity inline string (up to 23 bytes), `Display`-able.
#[derive(Clone, Copy, Default)]
pub struct Small {
    buf: [u8; 23],
    len: u8,
}

impl Small {
    pub fn as_str(&self) -> &str {
        // SAFETY: only valid UTF-8 is ever written via `fmt::Write`.
        unsafe { std::str::from_utf8_unchecked(&self.buf[..self.len as usize]) }
    }
}

impl fmt::Write for Small {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let b = s.as_bytes();
        let n = b.len().min(self.buf.len() - self.len as usize);
        // Never split a UTF-8 char: back off to a char boundary.
        let mut n2 = n;
        while n2 > 0 && !s.is_char_boundary(n2) {
            n2 -= 1;
        }
        self.buf[self.len as usize..self.len as usize + n2].copy_from_slice(&b[..n2]);
        self.len += n2 as u8;
        if n2 < b.len() { Err(fmt::Error) } else { Ok(()) }
    }
}

impl fmt::Display for Small {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_str())
    }
}

/// `small!("{:.1}", x)` -> Small
#[macro_export]
macro_rules! small {
    ($($arg:tt)*) => {{
        let mut s = $crate::fmt::Small::default();
        let _ = ::std::fmt::Write::write_fmt(&mut s, format_args!($($arg)*));
        s
    }};
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum MinSym {
    /// Allow bare numbers below 1024.
    None,
    K,
    M,
}

/// SYS-001..SYS-004 auto-unit: binary steps, carry rule, `low_precision` variant.
/// `None` input is handled by callers (they print `-`).
pub fn auto_unit(n: u64, low_precision: bool, min: MinSym) -> Small {
    let _ = (n, low_precision, min);
    todo!()
}

/// Same for float inputs (rates).
pub fn auto_unit_f(n: f64, low_precision: bool, min: MinSym) -> Small {
    let _ = (n, low_precision, min);
    todo!()
}

/// Fixed-size ring buffer for history (GEN-008). Allocated once.
pub struct Ring {
    buf: Box<[f32]>,
    head: usize,
    len: usize,
}

impl Ring {
    pub fn new(cap: usize) -> Ring {
        Ring { buf: vec![0.0; cap.max(1)].into_boxed_slice(), head: 0, len: 0 }
    }
    pub fn push(&mut self, v: f32) {
        let cap = self.buf.len();
        self.buf[(self.head + self.len) % cap] = v;
        if self.len < cap {
            self.len += 1;
        } else {
            self.head = (self.head + 1) % cap;
        }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }
    /// i = 0 is the oldest retained sample.
    pub fn get(&self, i: usize) -> f32 {
        self.buf[(self.head + i) % self.buf.len()]
    }
    pub fn last(&self) -> Option<f32> {
        if self.len == 0 { None } else { Some(self.get(self.len - 1)) }
    }
    pub fn clear(&mut self) {
        self.len = 0;
        self.head = 0;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trend {
    Unknown,
    Up,
    Down,
    Flat,
}

/// SYS trend rule: needs 30 samples; mean(newest 15) - mean(oldest 15) vs threshold 1.
pub fn trend(r: &Ring) -> Trend {
    let _ = r;
    todo!()
}

/// Glyph for a trend (Unicode or ASCII per GEN-017).
pub fn trend_glyph(t: Trend, ascii: bool) -> &'static str {
    let _ = (t, ascii);
    todo!()
}
