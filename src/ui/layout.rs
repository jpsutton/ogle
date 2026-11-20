//! Layout engine (01 section 6): places module panels on the screen and clips them.

use std::fmt::Write as _;

use crate::modules::{self, Area, ModuleId, UiState, World};
use crate::term::Screen;
use crate::ui::panel::{flags, Align, Panel, Role};

/// One reusable panel buffer per module.
pub type Panels = [Panel; ModuleId::COUNT];

/// Minimum usable size (UI-071).
pub const MIN_W: u16 = 50;
pub const MIN_H: u16 = 12;

/// Render module `m` into `out` (cleared first).
pub fn render(m: ModuleId, w: &World, area: Area, out: &mut Panel) -> Align {
    out.clear();
    match m {
        ModuleId::System => modules::system::render(w, area, out),
        ModuleId::Uptime => modules::uptime::render(w, area, out),
        ModuleId::Now => modules::now::render(w, area, out),
        ModuleId::Cpu => modules::cpu::render(w, area, out),
        ModuleId::Percpu => modules::percpu::render(w, area, out),
        ModuleId::Load => modules::load::render(w, area, out),
        ModuleId::Mem => modules::mem::render(w, area, out),
        ModuleId::Memswap => modules::memswap::render(w, area, out),
        ModuleId::Quicklook => modules::quicklook::render(w, area, out),
        ModuleId::Network => modules::network::render(w, area, out),
        ModuleId::Diskio => modules::diskio::render(w, area, out),
        ModuleId::Fs => modules::fs::render(w, area, out),
        ModuleId::Sensors => modules::sensors::render(w, area, out),
        ModuleId::Processcount => modules::processcount::render(w, area, out),
        ModuleId::Processlist => modules::processlist::render(w, area, out),
        ModuleId::Programlist => modules::programlist::render(w, area, out),
        ModuleId::Alert => modules::alert::render(w, area, out),
    }
}

/// Draw lines `first..` of `p` into the rectangle (x, y, width, max_h). Spans with a flag
/// in `skip` are left out. A non-splittable span that crosses the right edge ends the
/// line (UI-067); a splittable one is cut (UI-068). `split_all` treats every span as
/// splittable (popup content, UI-100). Returns the number of rows used.
#[allow(clippy::too_many_arguments)]
pub fn blit(scr: &mut Screen, p: &Panel, first: usize, x: u16, y: u16, width: u16, max_h: u16, skip: u8, split_all: bool) -> u16 {
    let right = x.saturating_add(width);
    let n = p.n_lines().saturating_sub(first).min(max_h as usize);
    for i in 0..n {
        let row = y + i as u16;
        let mut cx = x as u32;
        for s in p.line(first + i) {
            if s.flags & skip != 0 {
                continue;
            }
            if cx + s.width as u32 > right as u32 {
                if split_all || s.flags & flags::SPLITTABLE != 0 {
                    scr.put(cx as u16, row, right, p.span_text(s), s.role);
                }
                break;
            }
            scr.put(cx as u16, row, right, p.span_text(s), s.role);
            cx += s.width as u32;
        }
    }
    n as u16
}

/// Separator line (UI-082). When separators are off the row stays blank.
pub fn separator(scr: &mut Screen, ui: &UiState, y: u16) {
    let (w, h) = scr.size();
    if ui.separator && y < h {
        scr.fill(0, y, w, if ui.ascii { '-' } else { '─' }, Role::Separator);
    }
}

pub fn quicklook_shown(ui: &UiState) -> bool {
    ui.top && ui.on(ModuleId::Quicklook) && (ui.quicklook || ui.full_quicklook)
}

pub fn cpu_id(ui: &UiState) -> ModuleId {
    if ui.percpu {
        ModuleId::Percpu
    } else {
        ModuleId::Cpu
    }
}

/// Visibility of cpu/percpu, mem, memswap and load in the top row (UI-044).
pub fn top_shown(ui: &UiState, m: ModuleId) -> bool {
    ui.top && ui.on(m) && (m == ModuleId::Load || !ui.full_quicklook)
}

/// UI-059
pub fn sidebar_width(w: u16) -> u16 {
    34u16.min(23u16.max(w.saturating_sub(105)))
}

pub fn sidebar_shown(ui: &UiState, w: u16) -> bool {
    ui.left_sidebar && w >= 70
}

/// Draw the main screen. Returns the number of process rows the list showed (UI-062).
pub fn draw_main(w: &World, order: &[ModuleId], panels: &mut Panels, scr: &mut Screen) -> u16 {
    let (_, sh) = scr.size();
    header(w, panels, scr);
    separator(scr, &w.ui, 1);
    let mut y = 2u16;
    let th = top_row(w, panels, scr, y, sh.saturating_sub(4).max(1));
    if th > 0 {
        y += th;
        separator(scr, &w.ui, y);
        y += 1;
    }
    body(w, order, panels, scr, y)
}

/// UI-053
fn header(w: &World, panels: &mut Panels, scr: &mut Screen) {
    let (sw, _) = scr.size();
    let area = Area { width: sw, max_height: 1, narrow: false };
    let (s, u) = (ModuleId::System as usize, ModuleId::Uptime as usize);
    let mut up_w = 0usize;
    if w.ui.on(ModuleId::Uptime) {
        render(ModuleId::Uptime, w, area, &mut panels[u]);
        up_w = panels[u].width(0);
        if up_w >= sw as usize {
            up_w = 0;
        }
    }
    let mut skip = 0u8;
    if w.ui.on(ModuleId::System) {
        render(ModuleId::System, w, area, &mut panels[s]);
        if up_w > 0 && (sw as usize) < panels[s].width(0) + up_w + 1 {
            skip = flags::OPTIONAL;
            if panels[s].width(flags::OPTIONAL) + up_w + 1 > sw as usize {
                up_w = 0;
            }
        }
        let right = if up_w > 0 { sw - up_w as u16 - 1 } else { sw };
        blit(scr, &panels[s], 0, 0, 0, right, 1, skip, false);
    }
    if up_w > 0 {
        blit(scr, &panels[u], 0, sw - up_w as u16, 0, up_w as u16, 1, 0, false);
    }
}

/// UI-054 to UI-057. Returns the row height (0 = no top row).
fn top_row(w: &World, panels: &mut Panels, scr: &mut Screen, y: u16, max_h: u16) -> u16 {
    let ui = &w.ui;
    if !ui.top {
        return 0;
    }
    let (sw, _) = scr.size();
    let swi = sw as i32;
    let area = Area { width: sw, max_height: max_h, narrow: false };
    let mut ids = [ModuleId::Quicklook; 5];
    let mut wid = [0usize; 5];
    let mut skip = [0u8; 5];
    let mut n = 0usize;
    let mut has_ql = quicklook_shown(ui);
    if has_ql {
        n = 1;
    }
    let cpu = cpu_id(ui);
    for m in [cpu, ModuleId::Mem, ModuleId::Memswap, ModuleId::Load] {
        if top_shown(ui, m) {
            render(m, w, area, &mut panels[m as usize]);
            ids[n] = m;
            wid[n] = panels[m as usize].width(0);
            n += 1;
        }
    }
    // Drop quicklook (always at index 0).
    let drop_ql = |ids: &mut [ModuleId; 5], wid: &mut [usize; 5], skip: &mut [u8; 5], n: &mut usize| {
        ids.copy_within(1..*n, 0);
        wid.copy_within(1..*n, 0);
        skip.copy_within(1..*n, 0);
        *n -= 1;
    };
    if has_ql {
        // UI-055
        let others: usize = wid[1..n].iter().sum();
        let avail = swi - (others as i32 + 8 + 3 * n as i32);
        let ql = if ui.full_quicklook { avail } else { avail.min(43) };
        if ql < 20 {
            drop_ql(&mut ids, &mut wid, &mut skip, &mut n);
            has_ql = false;
        } else {
            let q = ModuleId::Quicklook as usize;
            render(ModuleId::Quicklook, w, Area { width: ql as u16, ..area }, &mut panels[q]);
            wid[0] = ql as usize;
        }
    }
    if n == 0 {
        return 0;
    }
    let total = |wid: &[usize]| wid.iter().sum::<usize>() as i32;
    let raw_gap = |t: i32, n: usize| if n <= 1 { i32::MAX } else { (swi - t) / (n as i32 - 1) };
    // UI-057 steps 1 and 2: narrow mem, then cpu.
    for m in [ModuleId::Mem, cpu] {
        if raw_gap(total(&wid[..n]), n) >= 3 {
            break;
        }
        if let Some(i) = ids[..n].iter().position(|&x| x == m) {
            let p = &mut panels[m as usize];
            render(m, w, Area { narrow: true, ..area }, p);
            wid[i] = p.width(flags::OPTIONAL);
            skip[i] = flags::OPTIONAL;
        }
    }
    // Step 3
    if has_ql && total(&wid[..n]) + (n as i32 - 1) > swi {
        drop_ql(&mut ids, &mut wid, &mut skip, &mut n);
        has_ql = false;
    }
    if n == 0 {
        return 0;
    }
    // UI-056
    let gap = if n == 1 { 0 } else { raw_gap(total(&wid[..n]), n).max(1) };
    let mut x = 0i32;
    let mut h = 0u16;
    for i in 0..n {
        let pw = wid[i] as i32;
        if x + pw > swi {
            break; // Step 4: never draw a panel cut in half.
        }
        let rows = blit(scr, &panels[ids[i] as usize], 0, x as u16, y, pw as u16, max_h, skip[i], false);
        h = h.max(rows);
        x += pw + if has_ql && i == 0 { 1 } else { gap };
    }
    h
}

/// Left sidebar (UI-059, UI-064 to UI-066) and right column (UI-060 to UI-063).
fn body(w: &World, order: &[ModuleId], panels: &mut Panels, scr: &mut Screen, y0: u16) -> u16 {
    let ui = &w.ui;
    let (sw, sh) = scr.size();
    if y0 >= sh {
        return 0;
    }
    let bh = sh - y0;
    let side = sidebar_shown(ui, sw);
    let sbw = sidebar_width(sw);
    if side {
        let area = |h: u16| Area { width: sbw, max_height: h, narrow: false };
        let mut bottom = sh;
        let now = ModuleId::Now;
        if order.contains(&now) && ui.on(now) && bh >= 2 {
            let p = &mut panels[now as usize];
            render(now, w, area(bh), p);
            let lines = (p.n_lines() as u16).min(bh);
            if lines > 0 {
                blit(scr, p, 0, 0, sh - lines, sbw, lines, 0, false);
                bottom = (sh - lines).saturating_sub(1);
            }
        }
        let mut y = y0;
        for &m in order {
            if m == now || !ui.on(m) {
                continue;
            }
            if y >= bottom || bottom - y < 2 {
                break; // UI-065
            }
            let avail = bottom - y;
            let p = &mut panels[m as usize];
            render(m, w, area(avail), p);
            let rows = blit(scr, p, 0, 0, y, sbw, avail, 0, false);
            if rows > 0 {
                y += rows + 1;
            }
        }
    }

    let rx = if side { sbw + 3 } else { 0 };
    if sw < rx + 23 {
        return 0;
    }
    let rw = sw - rx;
    let area = |h: u16| Area { width: rw, max_height: h, narrow: false };
    let mut y = y0;
    let pc = ModuleId::Processcount;
    if ui.on(pc) && bh >= 2 {
        let p = &mut panels[pc as usize];
        render(pc, w, area(bh), p);
        let rows = blit(scr, p, 0, rx, y, rw, bh, 0, false);
        if rows > 0 {
            y += rows + 1;
        }
    }
    let mut bottom = sh;
    if ui.alert_panel && y < sh && sh - y >= 2 {
        // UI-066: the alert panel sits on the last rows; the list flows above it.
        let avail = sh - y;
        let a = ModuleId::Alert as usize;
        render(ModuleId::Alert, w, area(avail), &mut panels[a]);
        let lines = (panels[a].n_lines() as u16).min(avail);
        if lines > 0 {
            blit(scr, &panels[a], 0, rx, sh - lines, rw, lines, 0, false);
            bottom = (sh - lines).saturating_sub(1);
        }
    }
    let list = if ui.programs { ModuleId::Programlist } else { ModuleId::Processlist };
    let mut rows = 0;
    if ui.processes && ui.on(list) && bottom > y && bottom - y >= 2 {
        let h = bottom - y;
        rows = modules::processlist::rows_for_height(w, h);
        let p = &mut panels[list as usize];
        // PROC/UI rule: the list drops optional columns on terminals <= 102 columns,
        // independent of the top-row narrowing.
        render(list, w, Area { narrow: sw <= 102, ..area(h) }, p);
        blit(scr, p, 0, rx, y, rw, h, 0, false);
    }
    rows
}

/// UI-071: centred "Terminal too small" message. `buf` is a reusable scratch string.
pub fn too_small(scr: &mut Screen, buf: &mut String) {
    let (w, h) = scr.size();
    buf.clear();
    let _ = write!(buf, "Terminal too small: {w}x{h} (need at least {MIN_W}x{MIN_H})");
    let len = buf.len() as u16;
    let x = w.saturating_sub(len) / 2;
    scr.put(x, h / 2, w, buf, Role::Default);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_width_formula() {
        assert_eq!(sidebar_width(80), 23);
        assert_eq!(sidebar_width(130), 25);
        assert_eq!(sidebar_width(200), 34);
    }
}
