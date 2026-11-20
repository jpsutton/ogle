//! Module memswap: swap space (02-system-modules.md section 11, SYS-087..SYS-090).

use std::time::Duration;

use crate::alerts::{Alerts, Limits, StatId};
use crate::config::Config;
use crate::fmt::{auto_unit, trend, trend_glyph, MinSym, Ring};
use crate::modules::cpu::{cfg_refresh, label, pct_val};
use crate::modules::{Area, Ctx, UiState, World};
use crate::platform::{Platform, Swap, Sys};
use crate::ui::panel::{Align, Panel, Role};

pub struct MemSwap {
    refresh: Option<Duration>,
    s: Option<Swap>,
    percent: Option<f64>,
    id: Option<StatId>,
    role: Role,
    /// SYS-090: percent series for the trend arrow.
    hist: Ring,
}

impl MemSwap {
    pub fn new(cfg: &Config, ui: &UiState, alerts: &mut Alerts) -> Self {
        let _ = ui;
        // SYS-089
        let id = Alerts::resolve(cfg, "memswap", None, None, Some(Limits::new(50.0, 70.0, 90.0, true)))
            .map(|l| alerts.register("memswap", "MEMSWAP", l));
        MemSwap {
            refresh: cfg_refresh(cfg, "memswap"),
            s: None,
            percent: None,
            id,
            role: Role::Default,
            hist: Ring::new(30),
        }
    }

    pub fn refresh(&self) -> Option<Duration> {
        self.refresh
    }

    /// The platform reports swap at all (SYS-087); quicklook skips `swap` otherwise.
    pub fn present(&self) -> bool {
        self.s.is_some()
    }

    /// Used percentage; None when unknown or total swap is 0.
    pub fn percent(&self) -> Option<f64> {
        self.percent
    }

    pub fn update(&mut self, sys: &mut Sys, ctx: &Ctx, ui: &UiState, alerts: &mut Alerts) {
        let _ = ctx;
        self.s = sys.swap();
        let Some(s) = self.s.filter(|s| s.total > 0) else {
            // SYS-087/SYS-089: no swap -> percent `-`, not evaluated.
            self.percent = None;
            self.role = Role::Default;
            return;
        };
        let p = (s.used as f64 * 100.0 / s.total as f64).clamp(0.0, 100.0);
        self.percent = Some(p);
        if ui.history {
            self.hist.push(p as f32);
        } else {
            self.hist.clear();
        }
        self.role = match self.id {
            Some(id) => alerts.eval(id, s.used as f64, s.total as f64),
            None => Role::Default,
        };
    }
}

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    let _ = area;
    let m = &w.memswap;
    let Some(s) = m.s else { return Align::Top };
    // SYS-088
    out.add(Role::Title, 0, "SWAP");
    out.addf(Role::Default, 0, format_args!(" {:<2}", trend_glyph(trend(&m.hist), w.ui.ascii)));
    pct_val(out, 0, m.role, m.percent, 6);
    for (name, v) in [("total", s.total), ("used", s.used), ("free", s.free)] {
        out.newline();
        label(out, 0, "", name, 8);
        out.addf(Role::Default, 0, format_args!("{:>5}", auto_unit(v, false, MinSym::K)));
    }
    Align::Top
}
