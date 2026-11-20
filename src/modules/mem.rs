//! Module mem: physical memory (02-system-modules.md section 10, SYS-080..SYS-086).

use std::time::Duration;

use crate::alerts::{Alerts, Limits, StatId};
use crate::config::Config;
use crate::fmt::{auto_unit, trend, trend_glyph, MinSym, Ring};
use crate::modules::cpu::{cfg_refresh, label, pad, pct_val};
use crate::modules::{Area, Ctx, UiState, World};
use crate::platform::{Memory, Platform, Sys};
use crate::ui::panel::{flags, Align, Panel, Role};

pub struct Mem {
    refresh: Option<Duration>,
    show_available: bool,
    m: Option<Memory>,
    used: u64,
    percent: Option<f64>,
    id: Option<StatId>,
    role: Role,
    /// SYS-085: percent series for the trend arrow.
    hist: Ring,
}

impl Mem {
    pub fn new(cfg: &Config, ui: &UiState, alerts: &mut Alerts) -> Self {
        let _ = ui;
        // SYS-084
        let id = Alerts::resolve(cfg, "mem", None, None, Some(Limits::new(50.0, 70.0, 90.0, true)))
            .map(|l| alerts.register("mem", "MEM", l));
        Mem {
            refresh: cfg_refresh(cfg, "mem"),
            show_available: cfg.bool(&["mem", "show_available"]).unwrap_or(false),
            m: None,
            used: 0,
            percent: None,
            id,
            role: Role::Default,
            hist: Ring::new(30),
        }
    }

    pub fn refresh(&self) -> Option<Duration> {
        self.refresh
    }

    /// Physical RAM in bytes, None until sampled or when the platform cannot report it.
    pub fn total(&self) -> Option<u64> {
        self.m.map(|m| m.total)
    }

    /// Used percentage (SYS-080), shared with quicklook (SYS-012).
    pub fn percent(&self) -> Option<f64> {
        self.percent
    }

    pub fn update(&mut self, sys: &mut Sys, ctx: &Ctx, ui: &UiState, alerts: &mut Alerts) {
        let _ = ctx;
        // ZFS ARC adjustment (SYS-081) is applied by the platform backend.
        self.m = sys.memory();
        let Some(m) = self.m.filter(|m| m.total > 0) else {
            self.percent = None;
            self.role = Role::Default;
            return;
        };
        // SYS-080: used = max(0, total - available) on every platform.
        self.used = m.total.saturating_sub(m.available);
        let p = (self.used as f64 * 100.0 / m.total as f64).clamp(0.0, 100.0);
        self.percent = Some(p);
        if ui.history {
            self.hist.push(p as f32);
        } else {
            self.hist.clear();
        }
        self.role = match self.id {
            Some(id) => alerts.eval(id, self.used as f64, m.total as f64),
            None => Role::Default,
        };
    }
}

/// Auto-unit stat cell (SYS-007); blank space of the same width when absent (SYS-006).
fn unit_cell(out: &mut Panel, fl: u8, prefix: &str, name: &str, l: usize, v: Option<u64>) {
    match v {
        Some(v) => {
            label(out, fl, prefix, name, l);
            out.addf(Role::Default, fl, format_args!("{:>5}", auto_unit(v, false, MinSym::K)));
        }
        None => pad(out, fl, prefix.len() + l + 5),
    }
}

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    let s = &w.mem;
    let Some(m) = s.m else { return Align::Top };
    let opt = flags::OPTIONAL;
    // SYS-083: column 2 is optional.
    let col2 = !area.narrow;
    // SYS-082 line 1.
    out.add(Role::Title, 0, "MEM");
    out.addf(Role::Default, 0, format_args!(" {:<2}", trend_glyph(trend(&s.hist), w.ui.ascii)));
    pct_val(out, 0, s.role, s.percent, 7);
    if col2 {
        unit_cell(out, opt, "  ", "active", 9, m.active);
    }
    let (l3, v3) = if s.show_available { ("avail", m.available) } else { ("used", s.used) };
    let (c2l3, c2v3) = if m.buffers.is_some() { ("buffer", m.buffers) } else { ("wired", m.wired) };
    let rows = [
        ("total", Some(m.total), "inacti", m.inactive),
        (l3, Some(v3), c2l3, c2v3),
        ("free", m.free, "cached", m.cached),
    ];
    for (n1, v1, n2, v2) in rows {
        out.newline();
        unit_cell(out, 0, "", n1, 8, v1);
        if col2 {
            unit_cell(out, opt, "  ", n2, 9, v2);
        }
    }
    Align::Top
}
