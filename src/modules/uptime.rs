//! Module uptime (02-system-modules.md section 5, SYS-030..SYS-032).

use std::time::{Duration, Instant};

use crate::alerts::Alerts;
use crate::config::Config;
use crate::modules::{Area, Ctx, UiState, World};
use crate::platform::{Platform, Sys};
use crate::ui::panel::{Align, Panel, Role};

pub struct Uptime {
    boot: Option<u64>,
    /// Wall and monotonic clocks when `boot` was read, to detect clock jumps (06 section 4).
    anchor: Option<(f64, Instant)>,
    secs: Option<u64>,
}

impl Uptime {
    pub fn new(cfg: &Config, ui: &UiState, alerts: &mut Alerts) -> Self {
        let _ = (cfg, ui, alerts);
        Uptime { boot: None, anchor: None, secs: None }
    }

    /// SYS-032: every cycle.
    pub fn refresh(&self) -> Option<Duration> {
        None
    }

    /// Explicit refresh (F5): re-read the boot time (SYS-030).
    pub fn reset_caches(&mut self) {
        self.anchor = None;
    }

    pub fn update(&mut self, sys: &mut Sys, ctx: &Ctx, ui: &UiState, alerts: &mut Alerts) {
        let _ = (ui, alerts);
        // Re-read when the wall clock jumped more than 1 s against the monotonic clock.
        let stale = match self.anchor {
            None => true,
            Some((wall, mono)) => {
                let mono_d = ctx.now.saturating_duration_since(mono).as_secs_f64();
                ((ctx.wall - wall) - mono_d).abs() > 1.0
            }
        };
        if stale {
            self.boot = sys.boot_time();
            self.anchor = Some((ctx.wall, ctx.now));
        }
        // SYS-030: uptime = now - boot_time.
        self.secs = self.boot.map(|b| (ctx.wall as u64).saturating_sub(b));
    }
}

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    let _ = area;
    let Some(t) = w.uptime.secs else { return Align::Right };
    // SYS-031
    let (days, h, m, s) = (t / 86400, t % 86400 / 3600, t % 3600 / 60, t % 60);
    match days {
        0 => out.addf(Role::Default, 0, format_args!("Uptime: {h}:{m:02}:{s:02}")),
        1 => out.addf(Role::Default, 0, format_args!("Uptime: 1 day, {h}:{m:02}:{s:02}")),
        d => out.addf(Role::Default, 0, format_args!("Uptime: {d} days, {h}:{m:02}:{s:02}")),
    }
    Align::Right
}
