//! Alert panel (05-alerts.md section 8). The event engine and `[alert]` config live in
//! `crate::alerts`; this file only renders. No allocation per frame: every part is
//! formatted straight into the panel arena.

use crate::addf;
use crate::alerts::Level;
use crate::modules::{Area, ModuleId, World};
use crate::ui::panel::{flags, Align, Panel, Role};

/// ALR-029..ALR-032.
pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    // ALR-036: hiding only stops the display.
    if !w.ui.alert_panel || !w.ui.on(ModuleId::Alert) {
        return Align::Bottom;
    }
    let a = &w.alerts;
    let events = a.events();
    // ALR-030 empty list takes no lines; UI-065 fewer than 2 rows is not drawn.
    if events.is_empty() || area.max_height < 2 {
        return Align::Bottom;
    }
    // ALR-031 title.
    out.add(Role::Title, 0, a.title());
    // ALR-029: newest events that fit below the title.
    let n = events.len().min(area.max_height as usize - 1);
    let sp = flags::SPLITTABLE;
    for e in &events[..n] {
        out.newline();
        // 1. Begin time, local, `%Y-%m-%d %H:%M:%S(%z)`.
        let t = local_time(e.begin_wall);
        let (sign, off) = if t.off_min < 0 { ('-', -t.off_min) } else { ('+', t.off_min) };
        addf!(
            out,
            Role::Default,
            sp,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}({}{:02}{:02})",
            t.year,
            t.month,
            t.day,
            t.hour,
            t.min,
            t.sec,
            sign,
            off / 60,
            off % 60
        );
        let ty = a.type_name(e.ty);
        let state = if e.state == Level::Critical { "CRITICAL" } else { "WARNING" };
        match e.end_mono {
            // 2.-4. Finished: ` (H:MM:SS) - <STATE> on <TYPE>` (Python timedelta style).
            Some(end) => {
                let s = end.saturating_duration_since(e.begin_mono).as_secs();
                let (days, rem) = (s / 86_400, s % 86_400);
                if days > 0 {
                    let plural = if days == 1 { "" } else { "s" };
                    addf!(out, Role::Default, sp, " ({} day{}, ", days, plural);
                } else {
                    out.add(Role::Default, sp, " (");
                }
                addf!(
                    out,
                    Role::Default,
                    sp,
                    "{}:{:02}:{:02}) - {} on {}",
                    rem / 3600,
                    rem % 3600 / 60,
                    rem % 60,
                    state,
                    ty
                );
            }
            // Ongoing: TYPE decorated with its state (no _LOG variant).
            None => {
                out.add(Role::Default, sp, " (ongoing) - ");
                let role = if e.state == Level::Critical { Role::Critical } else { Role::Warning };
                out.add(role, sp, ty);
            }
        }
        // 5. Statistics, one decimal.
        let mean = if e.count > 0 { e.sum / e.count as f64 } else { 0.0 };
        if (e.max - e.min).abs() <= 0.1 * e.min.abs().max(e.max.abs()) {
            addf!(out, Role::Default, sp, " ({:.1})", mean);
        } else {
            addf!(out, Role::Default, sp, " (Min:{:.1} Mean:{:.1} Max:{:.1})", e.min, mean, e.max);
        }
        // 6. Top processes.
        for (i, name) in e.top.iter().enumerate() {
            out.add(Role::Default, sp, if i == 0 { ": " } else { ", " });
            out.add(Role::Default, sp, name);
        }
    }
    Align::Bottom
}

/// Broken-down local time plus UTC offset in minutes.
struct LocalTm {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    min: u32,
    sec: u32,
    off_min: i64,
}

#[cfg(unix)]
fn local_time(t: f64) -> LocalTm {
    let tt = t.floor() as libc::time_t;
    // SAFETY: `tm` is plain old data; zeroed is a valid initial value.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the duration of the call; localtime_r is
    // the reentrant variant and writes only into `tm`.
    let r = unsafe { libc::localtime_r(&tt, &mut tm) };
    if r.is_null() {
        return civil(t.floor() as i64, 0);
    }
    LocalTm {
        year: tm.tm_year as i64 + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        min: tm.tm_min as u32,
        sec: tm.tm_sec as u32,
        off_min: tm.tm_gmtoff as i64 / 60,
    }
}

/// Windows: the current UTC offset is `GetLocalTime - GetSystemTime`, applied to the
/// event's timestamp. Events from before a DST change show the current offset.
#[cfg(windows)]
fn local_time(t: f64) -> LocalTm {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    fn secs(s: &SYSTEMTIME) -> i64 {
        days_from_civil(s.wYear as i64, s.wMonth as u32, s.wDay as u32) * 86_400
            + s.wHour as i64 * 3600
            + s.wMinute as i64 * 60
            + s.wSecond as i64
    }
    // SAFETY: SYSTEMTIME is plain old data; zeroed is a valid initial value.
    let mut utc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    let mut loc: SYSTEMTIME = unsafe { std::mem::zeroed() };
    // SAFETY: both functions only write into the provided, valid SYSTEMTIME structs.
    unsafe {
        GetSystemTime(&mut utc);
        GetLocalTime(&mut loc);
    }
    // Round to whole minutes: the two calls may straddle a second boundary.
    let diff = secs(&loc) - secs(&utc);
    let off_min = (diff + if diff >= 0 { 30 } else { -30 }) / 60;
    civil(t.floor() as i64 + off_min * 60, off_min)
}

#[cfg(not(any(unix, windows)))]
fn local_time(t: f64) -> LocalTm {
    civil(t.floor() as i64, 0)
}

/// Split seconds since the epoch (already shifted to local time) into calendar fields.
#[allow(dead_code)]
fn civil(secs: i64, off_min: i64) -> LocalTm {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    LocalTm {
        year,
        month,
        day,
        hour: (rem / 3600) as u32,
        min: (rem % 3600 / 60) as u32,
        sec: (rem % 60) as u32,
        off_min,
    }
}

/// Inverse of `civil` (days since 1970-01-01).
#[cfg(windows)]
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        let t = civil(0, 0);
        assert_eq!((t.year, t.month, t.day, t.hour), (1970, 1, 1, 0));
        // 2026-11-19 18:43:07 UTC
        let t = civil(1_795_113_787, 0);
        assert_eq!((t.year, t.month, t.day, t.hour, t.min, t.sec), (2026, 11, 19, 18, 43, 7));
        let t = civil(951_782_400, 0); // 2000-02-29
        assert_eq!((t.year, t.month, t.day), (2000, 2, 29));
    }
}
