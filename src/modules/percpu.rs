//! Module percpu: per-core breakdown, render only; data lives in `World::cpu`
//! (02-system-modules.md section 8, SYS-060..SYS-067).

use crate::alerts::Alerts;
use crate::modules::cpu::{self, Pct, NAMES, TOTAL};
use crate::modules::{Area, World};
use crate::ui::panel::{Align, Panel, Role};

/// SYS-062 column order for this platform; absent fields are dropped at render time.
#[cfg(windows)]
const COLS: &[usize] = &[cpu::USER, cpu::SYSTEM, cpu::DPC, cpu::IRQ];
#[cfg(not(windows))]
const COLS: &[usize] = &[cpu::USER, cpu::SYSTEM, cpu::IOWAIT, cpu::IDLE, cpu::IRQ, cpu::NICE, cpu::STEAL, cpu::GUEST];

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    let c = &w.cpu;
    // SYS-064 vs SYS-065: quicklook provides the row labels when it is shown.
    let labels = !cpu::ql_visible(&w.ui);
    let max_lines = area.max_height.max(1) as usize;

    if labels {
        out.addf(Role::Title, 0, format_args!("{:<5}", "CPU"));
        out.addf(Role::Default, 0, format_args!("{:>7}", NAMES[TOTAL]));
    }
    for &f in COLS {
        if c.present[f] {
            out.addf(Role::Default, 0, format_args!("{:>7}", NAMES[f]));
        }
    }

    // SYS-063: shared ordering computed once per cycle by the cpu module.
    let mut lines = 1;
    for &core in c.order.iter().take(c.shown) {
        if lines >= max_lines {
            return Align::Top;
        }
        let Some(p) = c.per_pct.get(core as usize) else { continue };
        out.newline();
        lines += 1;
        if labels {
            if core < 10 {
                out.addf(Role::Default, 0, format_args!("CPU{core} "));
            } else {
                out.addf(Role::Default, 0, format_args!("{core:>4} "));
            }
        }
        row(w, p, labels, out);
    }
    if let Some(s) = &c.summary {
        if lines < max_lines {
            out.newline();
            if labels {
                out.add(Role::Default, 0, "CPU* ");
            }
            row(w, s, labels, out);
        }
    }
    Align::Top
}

fn row(w: &World, p: &Pct, with_total: bool, out: &mut Panel) {
    let c = &w.cpu;
    if with_total {
        cell(out, Role::Default, p[TOTAL]);
    }
    for &f in COLS {
        if !c.present[f] {
            continue;
        }
        // SYS-066: decoration only, never registered.
        let role = match (&c.percpu_limits[f], p[f]) {
            (Some(l), Some(v)) => Alerts::decorate(l, v, 100.0),
            _ => Role::Default,
        };
        cell(out, role, p[f]);
    }
}

fn cell(out: &mut Panel, role: Role, v: Option<f64>) {
    match v {
        Some(v) => out.addf(role, 0, format_args!("{:6.1}%", v.max(0.0))),
        None => out.add(Role::Default, 0, "     ?%"),
    }
}
