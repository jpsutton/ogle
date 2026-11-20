//! Shared number formatting and history helpers (02-system-modules.md SYS-001..SYS-00x).
//!
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
    auto_core(n as f64, true, low_precision, min)
}

/// Same for float inputs (rates).
pub fn auto_unit_f(n: f64, low_precision: bool, min: MinSym) -> Small {
    auto_core(n, false, low_precision, min)
}

const SYMS: [char; 8] = ['K', 'M', 'G', 'T', 'P', 'E', 'Z', 'Y'];

fn auto_core(n: f64, is_int: bool, low: bool, min: MinSym) -> Small {
    // SYS-002 step 2 (negative and NaN inputs are clamped to 0, SYS-005).
    if !(n > 0.0) {
        return crate::small!("0");
    }
    let kmin: usize = if min == MinSym::M { 2 } else { 1 };
    // SYS-002 step 3: first symbol from the largest down where v >= 1.
    let mut k = SYMS.len();
    while k >= kmin && n / 1024f64.powi(k as i32) < 1.0 {
        k -= 1;
    }
    if k < kmin {
        // SYS-002 step 4.
        return if is_int || n.fract() == 0.0 { crate::small!("{}", n as u64) } else { crate::small!("{:.2}", n) };
    }
    loop {
        let v = n / 1024f64.powi(k as i32);
        let sym = SYMS[k - 1];
        // SYS-002 step 5.
        let mut d: usize = if v < 9.995 { 2 } else if v < 99.95 { 1 } else { 0 };
        if sym == 'K' {
            d = 0;
        }
        if low {
            d = if sym == 'K' || sym == 'M' { 0 } else { d.min(1) };
        }
        let mut s = crate::small!("{:.*}", d, v);
        // SYS-002 step 7: carry when the printed integer part reaches 1024.
        let int_part = s.as_str().split('.').next().and_then(|t| t.parse::<u64>().ok()).unwrap_or(0);
        if int_part >= 1024 && k < SYMS.len() {
            k += 1;
            continue;
        }
        let _ = fmt::Write::write_char(&mut s, sym);
        return s;
    }
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
    // SYS-008
    let n = r.len();
    if n < 30 {
        return Trend::Unknown;
    }
    let base = n - 30;
    let (mut old, mut new) = (0.0f64, 0.0f64);
    for i in 0..15 {
        old += r.get(base + i) as f64;
        new += r.get(base + 15 + i) as f64;
    }
    let t = (new - old) / 15.0;
    // SYS-009
    if t > 1.0 {
        Trend::Up
    } else if t < -1.0 {
        Trend::Down
    } else {
        Trend::Flat
    }
}

/// Glyph for a trend (Unicode or ASCII per GEN-017).
pub fn trend_glyph(t: Trend, ascii: bool) -> &'static str {
    // SYS-009, GEN-017
    match (t, ascii) {
        (Trend::Unknown, _) => " ",
        (Trend::Up, false) => "\u{2191}",
        (Trend::Up, true) => "^",
        (Trend::Down, false) => "\u{2193}",
        (Trend::Down, true) => "v",
        (Trend::Flat, _) => "-",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SYS-004 normative vectors.
    const V: [(u64, &str, &str); 11] = [
        (0, "0", "0"),
        (1000, "1000", "1000"),
        (1024, "1K", "1K"),
        (613421788, "585M", "585M"),
        (1073741824, "1.00G", "1.0G"),
        (1181116006, "1.10G", "1.1G"),
        (5307033647, "4.94G", "4.9G"),
        (44968414685, "41.9G", "41.9G"),
        (838471403472, "781G", "781G"),
        (9683209690677, "8.81T", "8.8T"),
        (1048575, "1.00M", "1M"),
    ];

    #[test]
    fn auto_unit_vectors() {
        for (n, hi, lo) in V {
            assert_eq!(auto_unit(n, false, MinSym::K).as_str(), hi, "n={n}");
            assert_eq!(auto_unit(n, true, MinSym::K).as_str(), lo, "n={n} low");
            assert_eq!(auto_unit_f(n as f64, false, MinSym::K).as_str(), hi, "f n={n}");
        }
    }

    #[test]
    fn auto_unit_misc() {
        assert_eq!(auto_unit_f(12.5, false, MinSym::K).as_str(), "12.50");
        assert_eq!(auto_unit(500_000, false, MinSym::M).as_str(), "500000");
        assert_eq!(auto_unit(1023 * 1024 + 1000, false, MinSym::K).as_str(), "1.00M");
    }

    #[test]
    fn trend_rule() {
        let mut r = Ring::new(30);
        for _ in 0..29 {
            r.push(10.0);
        }
        assert_eq!(trend(&r), Trend::Unknown);
        r.push(10.0);
        assert_eq!(trend(&r), Trend::Flat);
        for _ in 0..15 {
            r.push(20.0);
        }
        assert_eq!(trend(&r), Trend::Up);
        for _ in 0..30 {
            r.push(0.0);
        }
        assert_eq!(trend(&r), Trend::Flat);
        assert_eq!(trend_glyph(Trend::Down, true), "v");
        assert_eq!(trend_glyph(Trend::Unknown, false), " ");
    }
}
