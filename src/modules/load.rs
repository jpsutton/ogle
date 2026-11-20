//! Module load: load averages (02-system-modules.md section 9, SYS-070..SYS-075).

use std::time::Duration;

use crate::alerts::{Alerts, Limits, StatId};
use crate::config::Config;
use crate::fmt::{trend, trend_glyph, Ring};
use crate::modules::cpu::{cfg_refresh, label};
use crate::modules::{Area, Ctx, UiState, World};
use crate::platform::{LoadAvg, Platform, Sys};
use crate::ui::panel::{Align, Panel, Role};

pub struct Load {
    refresh: Option<Duration>,
    avg: Option<LoadAvg>,
    cores: u32,
    topo_stale: bool,
    limits: Option<Limits>,
    id: Option<StatId>,
    role15: Role,
    role5: Role,
    /// SYS-074: min1 series for the trend arrow.
    hist: Ring,
}

impl Load {
    pub fn new(cfg: &Config, ui: &UiState, alerts: &mut Alerts) -> Self {
        let _ = ui;
        // SYS-073: percent of the core count; min15 is registered and logged as `load`.
        let limits = Alerts::resolve(cfg, "load", None, None, Some(Limits::new(70.0, 100.0, 500.0, true)));
        let id = limits.map(|l| alerts.register("load", "LOAD", l));
        Load {
            refresh: cfg_refresh(cfg, "load"),
            avg: None,
            cores: 0,
            topo_stale: true,
            limits,
            id,
            role15: Role::Default,
            role5: Role::Default,
            hist: Ring::new(30),
        }
    }

    pub fn refresh(&self) -> Option<Duration> {
        self.refresh
    }

    /// Latest 15-minute load average, None when the platform has none.
    pub fn min15(&self) -> Option<f64> {
        self.avg.map(|a| a.min15)
    }

    /// Explicit refresh (F5): re-read the core count (SYS-013).
    pub fn reset_caches(&mut self) {
        self.topo_stale = true;
    }

    pub fn update(&mut self, sys: &mut Sys, ctx: &Ctx, ui: &UiState, alerts: &mut Alerts) {
        let _ = ctx;
        if self.topo_stale || self.cores == 0 {
            // PLT-181: logical cores.
            self.cores = sys.cpu_topology().logical;
            self.topo_stale = false;
        }
        self.avg = sys.load_avg();
        let (Some(a), max) = (self.avg, self.cores as f64) else {
            self.role15 = Role::Default;
            self.role5 = Role::Default;
            return;
        };
        if ui.history {
            self.hist.push(a.min1 as f32);
        } else {
            self.hist.clear();
        }
        if self.cores == 0 {
            return;
        }
        // SYS-073: min15 drives the state; min5 is decoration only.
        self.role15 = match self.id {
            Some(id) => alerts.eval(id, a.min15, max),
            None => Role::Default,
        };
        self.role5 = match &self.limits {
            Some(l) => Alerts::decorate(l, a.min5, max),
            None => Role::Default,
        };
    }
}

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    let _ = area;
    let l = &w.load;
    // SYS-070: absent without a load average.
    let Some(a) = l.avg else { return Align::Top };
    // SYS-071 line 1.
    out.add(Role::Title, 0, "LOAD");
    out.addf(Role::Default, 0, format_args!(" {}", trend_glyph(trend(&l.hist), w.ui.ascii)));
    if l.cores > 0 {
        out.addf(Role::Default, 0, format_args!("{:>3}core", l.cores));
    }
    // SYS-072: per-core percent mode follows key `0`.
    let pct = w.ui.normalize_cpu && l.cores > 0;
    for (name, v, role) in [("1 min", a.min1, Role::Default), ("5 min", a.min5, l.role5), ("15 min", a.min15, l.role15)] {
        out.newline();
        label(out, 0, "", name, 7);
        if pct {
            out.addf(role, 0, format_args!("{:>5.1}%", v / l.cores as f64 * 100.0));
        } else {
            out.addf(role, 0, format_args!("{:>6.2}", v));
        }
    }
    Align::Top
}
