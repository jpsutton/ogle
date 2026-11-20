//! Small allocation-free helpers shared by platform backends.
//!
//! Parsers work on raw bytes as read from the OS (PERF-0xx: no String conversion,
//! no `split().collect()` in per-cycle paths).

/// Parse leading ASCII digits (after optional leading spaces) as u64.
/// Returns None when no digit is present. Stops at the first non-digit.
pub fn parse_u64(s: &[u8]) -> Option<u64> {
    let s = trim_start(s);
    let mut v: u64 = 0;
    let mut any = false;
    for &b in s {
        if !b.is_ascii_digit() {
            break;
        }
        v = v.wrapping_mul(10).wrapping_add((b - b'0') as u64);
        any = true;
    }
    any.then_some(v)
}

/// Parse an optionally signed decimal integer.
pub fn parse_i64(s: &[u8]) -> Option<i64> {
    let s = trim_start(s);
    match s.first() {
        Some(b'-') => parse_u64(&s[1..]).map(|v| (v as i64).wrapping_neg()),
        Some(b'+') => parse_u64(&s[1..]).map(|v| v as i64),
        _ => parse_u64(s).map(|v| v as i64),
    }
}

/// Parse a hexadecimal number, with or without a `0x` prefix.
pub fn parse_hex(s: &[u8]) -> Option<u64> {
    let mut s = trim(s);
    if s.len() > 1 && s[0] == b'0' && (s[1] == b'x' || s[1] == b'X') {
        s = &s[2..];
    }
    let mut v: u64 = 0;
    let mut any = false;
    for &b in s {
        let d = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            _ => break,
        };
        v = (v << 4) | d as u64;
        any = true;
    }
    any.then_some(v)
}

/// Parse a simple decimal number like `0.52`, `-3`, `12.250`. No exponent support.
pub fn parse_f64(s: &[u8]) -> Option<f64> {
    let s = trim_start(s);
    let (neg, s) = match s.first() {
        Some(b'-') => (true, &s[1..]),
        _ => (false, s),
    };
    let mut int: f64 = 0.0;
    let mut i = 0;
    let mut any = false;
    while i < s.len() && s[i].is_ascii_digit() {
        int = int * 10.0 + (s[i] - b'0') as f64;
        i += 1;
        any = true;
    }
    if i < s.len() && s[i] == b'.' {
        i += 1;
        let mut scale = 0.1;
        while i < s.len() && s[i].is_ascii_digit() {
            int += (s[i] - b'0') as f64 * scale;
            scale *= 0.1;
            i += 1;
            any = true;
        }
    }
    any.then_some(if neg { -int } else { int })
}

pub fn trim_start(mut s: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = s {
        if first.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    s
}

pub fn trim(s: &[u8]) -> &[u8] {
    let mut s = trim_start(s);
    while let [rest @ .., last] = s {
        if last.is_ascii_whitespace() || *last == 0 {
            s = rest;
        } else {
            break;
        }
    }
    s
}

/// Iterator over whitespace-separated fields of a byte slice; no allocation.
pub struct Fields<'a> {
    s: &'a [u8],
}

impl<'a> Iterator for Fields<'a> {
    type Item = &'a [u8];
    fn next(&mut self) -> Option<&'a [u8]> {
        let s = trim_start(self.s);
        if s.is_empty() {
            self.s = s;
            return None;
        }
        let end = s.iter().position(|b| b.is_ascii_whitespace()).unwrap_or(s.len());
        self.s = &s[end..];
        Some(&s[..end])
    }
}

pub fn fields(s: &[u8]) -> Fields<'_> {
    Fields { s }
}

/// Iterator over `\n`-separated lines; no allocation. A trailing empty line is skipped.
pub fn lines(s: &[u8]) -> impl Iterator<Item = &[u8]> {
    s.split(|&b| b == b'\n').filter(|l| !l.is_empty())
}

/// For `Key: value` style files (meminfo, status): value of the first line whose text
/// before the separator equals `key`. Returns the bytes after the separator.
pub fn find_key<'a>(buf: &'a [u8], key: &[u8], sep: u8) -> Option<&'a [u8]> {
    lines(buf).find_map(|l| {
        let p = l.iter().position(|&b| b == sep)?;
        (&l[..p] == key).then(|| &l[p + 1..])
    })
}

/// Append bytes to a String, replacing invalid UTF-8 sequences with '?'.
/// Does not allocate when the String has enough capacity.
pub fn push_bytes_lossy(out: &mut String, mut b: &[u8]) {
    loop {
        match std::str::from_utf8(b) {
            Ok(s) => {
                out.push_str(s);
                return;
            }
            Err(e) => {
                let ok = e.valid_up_to();
                // The prefix up to `ok` is valid UTF-8 by definition.
                out.push_str(std::str::from_utf8(&b[..ok]).unwrap_or(""));
                out.push('?');
                let skip = e.error_len().unwrap_or(b.len() - ok);
                b = &b[ok + skip..];
            }
        }
    }
}

/// Set `out` to the given bytes, reusing its allocation.
pub fn set_string(out: &mut String, b: &[u8]) {
    out.clear();
    push_bytes_lossy(out, b);
}

/// Get slot `i` of a reused output vector, pushing a default entry when needed.
/// Callers truncate to the final count afterwards, so old entries keep their Strings.
pub fn slot<T: Default>(v: &mut Vec<T>, i: usize) -> &mut T {
    if i >= v.len() {
        v.push(T::default());
    }
    &mut v[i]
}

/// Widen a wrapping 32-bit counter to 64 bits (PLT-002).
/// `prev_raw` is the last raw 32-bit value, `acc` the widened value so far.
/// A wrap is assumed only when the old value was above 2^31; otherwise a smaller
/// value is treated as a counter reset and the widened value restarts from it.
pub fn widen_u32(prev_raw: u32, new_raw: u32, acc: u64) -> u64 {
    if new_raw >= prev_raw {
        acc + (new_raw - prev_raw) as u64
    } else if prev_raw > 1 << 31 {
        acc + ((1u64 << 32) - prev_raw as u64) + new_raw as u64
    } else {
        new_raw as u64
    }
}

/// Write a u32 as decimal ASCII into `buf`, returning the number of bytes written.
pub fn write_u32(buf: &mut [u8], mut v: u32) -> usize {
    let mut tmp = [0u8; 10];
    let mut n = 0;
    loop {
        tmp[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    for i in 0..n {
        buf[i] = tmp[n - 1 - i];
    }
    n
}

/// True when every byte is an ASCII digit and the slice is non-empty.
pub fn all_digits(s: &[u8]) -> bool {
    !s.is_empty() && s.iter().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(parse_u64(b"  123 kB"), Some(123));
        assert_eq!(parse_u64(b"x"), None);
        assert_eq!(parse_i64(b"-5"), Some(-5));
        assert_eq!(parse_hex(b"0x1003\n"), Some(0x1003));
        assert!((parse_f64(b"0.52").unwrap() - 0.52).abs() < 1e-9);
        assert_eq!(parse_f64(b"-3"), Some(-3.0));
    }

    #[test]
    fn splitting() {
        let v: Vec<&[u8]> = fields(b"  a bb\tc \n").collect();
        assert_eq!(v, vec![&b"a"[..], b"bb", b"c"]);
        assert_eq!(find_key(b"A: 1\nMemFree:  42 kB\n", b"MemFree", b':').map(parse_u64), Some(Some(42)));
    }

    #[test]
    fn lossy() {
        let mut s = String::new();
        set_string(&mut s, b"ab\xffc");
        assert_eq!(s, "ab?c");
    }

    #[test]
    fn widen() {
        assert_eq!(widen_u32(10, 20, 100), 110);
        assert_eq!(widen_u32(u32::MAX - 5, 4, 100), 110);
        assert_eq!(widen_u32(1000, 5, 100), 5);
    }

    #[test]
    fn write_num() {
        let mut b = [0u8; 12];
        let n = write_u32(&mut b, 40312);
        assert_eq!(&b[..n], b"40312");
    }
}
