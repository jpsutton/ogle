//! Module now: local clock (02-system-modules.md section 6, SYS-040..SYS-043).
//!
//! A small strftime subset is implemented here so no allocation happens per frame:
//! `%Y %m %d %H %M %S %z %Z %a %b %e %j %p %I %y %%`. Other codes are printed literally.

use std::fmt::{self, Write as _};
use std::time::Duration;

use crate::alerts::Alerts;
use crate::config::Config;
use crate::modules::{Area, Ctx, UiState, World};
use crate::platform::Sys;
use crate::ui::panel::{Align, Panel, Role};

const DEFAULT_FORMAT: &str = "%Y-%m-%d %H:%M:%S %Z";

pub struct Now {
    format: String,
    /// Default format in use: a zone name longer than 6 chars is replaced by `%z` (SYS-041).
    is_default: bool,
}

impl Now {
    pub fn new(cfg: &Config, ui: &UiState, alerts: &mut Alerts) -> Self {
        let _ = (ui, alerts);
        // The CLI `--strftime` option is applied to `[now] format` by the app.
        let user = cfg.str(&["now", "format"]);
        Now { format: user.unwrap_or(DEFAULT_FORMAT).to_string(), is_default: user.is_none() }
    }

    pub fn refresh(&self) -> Option<Duration> {
        None
    }

    /// Nothing to collect: SYS-040 takes the time at each redraw.
    pub fn update(&mut self, sys: &mut Sys, ctx: &Ctx, ui: &UiState, alerts: &mut Alerts) {
        let _ = (sys, ctx, ui, alerts);
    }
}

/// Broken-down local time.
#[derive(Clone, Copy, Default)]
struct Tm {
    year: i32,
    /// 1..=12
    mon: u32,
    mday: u32,
    hour: u32,
    min: u32,
    sec: u32,
    /// 0 = Sunday
    wday: u32,
    /// 0-based day of the year
    yday: u32,
    /// Seconds east of UTC.
    gmtoff: i64,
    zone: [u8; 16],
    zone_len: usize,
}

impl Tm {
    fn zone(&self) -> &str {
        std::str::from_utf8(&self.zone[..self.zone_len]).unwrap_or("")
    }
}

#[cfg(unix)]
fn local_now() -> Option<Tm> {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let t = secs as libc::time_t;
    // SAFETY: `tm` is plain old data; localtime_r writes it and returns null on failure.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the duration of the call.
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return None;
    }
    let mut out = Tm {
        year: tm.tm_year + 1900,
        mon: (tm.tm_mon + 1) as u32,
        mday: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        min: tm.tm_min as u32,
        sec: tm.tm_sec as u32,
        wday: tm.tm_wday as u32,
        yday: tm.tm_yday as u32,
        gmtoff: tm.tm_gmtoff as i64,
        ..Tm::default()
    };
    if !tm.tm_zone.is_null() {
        // SAFETY: tm_zone points to a NUL-terminated string owned by libc that stays
        // valid until the next tzset; it is copied out immediately.
        let z = unsafe { std::ffi::CStr::from_ptr(tm.tm_zone) }.to_bytes();
        let n = z.len().min(out.zone.len());
        out.zone[..n].copy_from_slice(&z[..n]);
        out.zone_len = n;
    }
    Some(out)
}

#[cfg(windows)]
fn local_now() -> Option<Tm> {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    // SAFETY: SYSTEMTIME is plain old data filled by the calls below.
    let mut lt: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut ut: SYSTEMTIME = unsafe { std::mem::zeroed() };
    // SAFETY: valid out-pointers.
    unsafe {
        GetLocalTime(&mut lt);
        GetSystemTime(&mut ut);
    }
    let abs = |s: &SYSTEMTIME| {
        days_from_civil(s.wYear as i32, s.wMonth as u32, s.wDay as u32) * 86400
            + s.wHour as i64 * 3600
            + s.wMinute as i64 * 60
            + s.wSecond as i64
    };
    // Offset rounded to whole minutes (the two calls may straddle a second).
    let off = ((abs(&lt) - abs(&ut)) as f64 / 60.0).round() as i64 * 60;
    let (y, m, d) = (lt.wYear as i32, lt.wMonth as u32, lt.wDay as u32);
    Some(Tm {
        year: y,
        mon: m,
        mday: d,
        hour: lt.wHour as u32,
        min: lt.wMinute as u32,
        sec: lt.wSecond as u32,
        wday: lt.wDayOfWeek as u32,
        yday: (days_from_civil(y, m, d) - days_from_civil(y, 1, 1)) as u32,
        gmtoff: off,
        // No zone abbreviation without Win32_System_Time: `%Z` prints the offset.
        ..Tm::default()
    })
}

/// Days since 1970-01-01 for a proleptic Gregorian date (H. Hinnant's algorithm).
#[cfg(any(windows, test))]
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = (if m <= 2 { y - 1 } else { y }) as i64;
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

const DAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// Formats `tm` with a strftime subset directly into a `fmt::Write` sink.
struct Strftime<'a> {
    fmt: &'a str,
    tm: &'a Tm,
    /// SYS-041: use `%z` for `%Z` when the zone name is longer than 6 chars (or unknown).
    z_fallback: bool,
}

impl fmt::Display for Strftime<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tm = self.tm;
        let mut chars = self.fmt.chars();
        while let Some(c) = chars.next() {
            if c != '%' {
                f.write_char(c)?;
                continue;
            }
            match chars.next() {
                Some('Y') => write!(f, "{}", tm.year)?,
                Some('y') => write!(f, "{:02}", tm.year.rem_euclid(100))?,
                Some('m') => write!(f, "{:02}", tm.mon)?,
                Some('d') => write!(f, "{:02}", tm.mday)?,
                Some('e') => write!(f, "{:2}", tm.mday)?,
                Some('H') => write!(f, "{:02}", tm.hour)?,
                Some('I') => write!(f, "{:02}", (tm.hour + 11) % 12 + 1)?,
                Some('M') => write!(f, "{:02}", tm.min)?,
                Some('S') => write!(f, "{:02}", tm.sec)?,
                Some('p') => f.write_str(if tm.hour < 12 { "AM" } else { "PM" })?,
                Some('j') => write!(f, "{:03}", tm.yday + 1)?,
                Some('a') => f.write_str(DAYS[tm.wday as usize % 7])?,
                Some('b') => f.write_str(MONTHS[(tm.mon as usize + 11) % 12])?,
                Some('z') => write_offset(f, tm.gmtoff)?,
                Some('Z') => {
                    let z = tm.zone();
                    if z.is_empty() || (self.z_fallback && z.chars().count() > 6) {
                        write_offset(f, tm.gmtoff)?;
                    } else {
                        f.write_str(z)?;
                    }
                }
                Some('%') => f.write_char('%')?,
                Some(o) => {
                    f.write_char('%')?;
                    f.write_char(o)?;
                }
                None => f.write_char('%')?,
            }
        }
        Ok(())
    }
}

fn write_offset(f: &mut fmt::Formatter<'_>, off: i64) -> fmt::Result {
    let sign = if off < 0 { '-' } else { '+' };
    let a = off.unsigned_abs();
    write!(f, "{sign}{:02}{:02}", a / 3600, a % 3600 / 60)
}

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    let _ = area;
    let n = &w.now;
    // SYS-040: time taken at each redraw.
    let Some(tm) = local_now() else { return Align::Bottom };
    out.addf(Role::Default, 0, format_args!("{}", Strftime { fmt: &n.format, tm: &tm, z_fallback: n.is_default }));
    // SYS-042: padded to 23 characters.
    let used = out.line_width(out.n_lines() - 1, 0);
    if used < 23 {
        out.addf(Role::Default, 0, format_args!("{:w$}", "", w = 23 - used));
    }
    Align::Bottom
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt_tm(fmt: &str, tm: &Tm, z: bool) -> String {
        format!("{}", Strftime { fmt, tm, z_fallback: z })
    }

    #[test]
    fn strftime_codes() {
        let mut tm = Tm { year: 2026, mon: 3, mday: 5, hour: 14, min: 7, sec: 9, wday: 4, yday: 63, gmtoff: -18000, ..Tm::default() };
        tm.zone[..3].copy_from_slice(b"EST");
        tm.zone_len = 3;
        assert_eq!(fmt_tm(DEFAULT_FORMAT, &tm, true), "2026-03-05 14:07:09 EST");
        assert_eq!(fmt_tm("%a %b %e %I%p %j %y %z %% %Q", &tm, true), "Thu Mar  5 02PM 064 26 -0500 % %Q");
        let long = b"Pacific Standard Time";
        tm.zone.copy_from_slice(&long[..16]);
        tm.zone_len = 16;
        assert_eq!(fmt_tm("%Z", &tm, true), "-0500");
    }

    #[test]
    fn civil_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
    }
}
