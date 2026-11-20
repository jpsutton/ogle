//! Disk I/O module (03-io-modules.md section 3).

use std::time::{Duration, Instant};

use crate::alerts::{Alerts, Limits, StatId};
use crate::config::Config;
use crate::fmt::{auto_unit_f, MinSym, Small};
use crate::modules::network::{
    cell, cfg_refresh, header, name_width, natural_cmp, rate_dt, register_item, thresholds_cfg, Aliases, Filter,
    Fit, Trunc,
};
use crate::modules::{Area, Ctx, DiskioMode, UiState, World};
use crate::platform::{DiskIo as PDisk, Platform, Sys};
use crate::ui::panel::{Align, Panel, Role};

const MODULE: &str = "diskio";
/// IO-051 compiled-in latency default (ms).
const LAT_DEFAULT: Limits = Limits::new(10.0, 20.0, 50.0, false);

/// Previous raw counters.
#[derive(Clone, Copy)]
struct Prev {
    rb: u64,
    wb: u64,
    rc: u64,
    wc: u64,
    rt: Option<u64>,
    wt: Option<u64>,
}

struct DiskItem {
    name: String,
    alias: Option<String>,
    /// Passed hide/show (computed once, IO-014).
    allowed: bool,
    seen: bool,
    prev: Option<Prev>,
    read_rate: Option<f64>,
    write_rate: Option<f64>,
    read_iops: Option<f64>,
    write_iops: Option<f64>,
    read_lat: Option<f64>,
    write_lat: Option<f64>,
    /// hide_zero latches (IO-017, IO-018).
    r_vis: bool,
    w_vis: bool,
    /// Stats: read, write (bytes/s), read_latency, write_latency (ms).
    ids: [Option<StatId>; 4],
    roles: [Role; 2],
}

impl DiskItem {
    fn disp(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

pub struct DiskIo {
    filter: Filter,
    aliases: Aliases,
    tcfg: Config,
    hide_zero: bool,
    zero_thr: f64,
    refresh: Option<Duration>,
    items: Vec<DiskItem>,
    buf: Vec<PDisk>,
    last: Option<Instant>,
}

/// Delta of two cumulative counters; None when the counter went backwards (IO-008).
fn delta(now: u64, prev: u64) -> Option<u64> {
    now.checked_sub(prev)
}

impl DiskIo {
    pub fn new(cfg: &Config, _ui: &UiState, _alerts: &mut Alerts) -> Self {
        DiskIo {
            filter: Filter::new(cfg, MODULE, &["loop.*", "/dev/loop.*"]), // IO-042
            aliases: Aliases::new(cfg, MODULE),
            tcfg: thresholds_cfg(cfg, MODULE),
            hide_zero: cfg.bool(&[MODULE, "hide_zero"]).unwrap_or(false),
            zero_thr: cfg.f64(&[MODULE, "hide_threshold_bytes"]).unwrap_or(0.0),
            refresh: cfg_refresh(cfg, MODULE),
            items: Vec::new(),
            buf: Vec::new(),
            last: None,
        }
    }

    pub fn refresh(&self) -> Option<Duration> {
        self.refresh
    }

    /// IO-043 RAM disks are hidden unless show_ramfs; applied before hide/show.
    fn visible(it: &DiskItem, ui: &UiState) -> bool {
        it.allowed && (ui.show_ramfs || !it.name.starts_with("ram"))
    }

    fn shown(&self, it: &DiskItem, ui: &UiState) -> bool {
        Self::visible(it, ui) && (!self.hide_zero || it.r_vis || it.w_vis)
    }

    pub fn update(&mut self, sys: &mut Sys, ctx: &Ctx, ui: &UiState, alerts: &mut Alerts) {
        let dt = rate_dt(self.last, ctx.now, self.refresh, ui.refresh);
        sys.disk_io(&mut self.buf);
        for it in &mut self.items {
            it.seen = false;
        }
        let mut added = false;
        for b in &self.buf {
            let idx = match self.items.iter().position(|i| i.name == b.name) {
                Some(i) => i,
                None => {
                    self.items.push(new_item(b, &self.filter, &self.aliases, &self.tcfg, alerts));
                    added = true;
                    self.items.len() - 1
                }
            };
            let it = &mut self.items[idx];
            it.seen = true;
            if !Self::visible(it, ui) {
                // IO-014: no work; forget counters so a later re-show starts unknown.
                it.prev = None;
                continue;
            }
            let cur = Prev {
                rb: b.read_bytes,
                wb: b.write_bytes,
                rc: b.read_count,
                wc: b.write_count,
                rt: b.read_time_ms,
                wt: b.write_time_ms,
            };
            let (p, dt) = match (it.prev, dt) {
                (Some(p), Some(dt)) => (p, dt),
                _ => {
                    it.read_rate = None;
                    it.write_rate = None;
                    it.read_iops = None;
                    it.write_iops = None;
                    it.read_lat = None;
                    it.write_lat = None;
                    it.prev = Some(cur);
                    continue;
                }
            };
            it.read_rate = delta(cur.rb, p.rb).map(|d| d as f64 / dt);
            it.write_rate = delta(cur.wb, p.wb).map(|d| d as f64 / dt);
            let drc = delta(cur.rc, p.rc);
            let dwc = delta(cur.wc, p.wc);
            it.read_iops = drc.map(|d| d as f64 / dt);
            it.write_iops = dwc.map(|d| d as f64 / dt);
            it.read_lat = latency(cur.rt, p.rt, drc);
            it.write_lat = latency(cur.wt, p.wt, dwc);
            it.prev = Some(cur);
            if it.read_rate.is_some_and(|r| r > self.zero_thr) {
                it.r_vis = true;
            }
            if it.write_rate.is_some_and(|r| r > self.zero_thr) {
                it.w_vis = true;
            }
        }
        self.items.retain(|it| {
            if !it.seen {
                for id in it.ids.into_iter().flatten() {
                    alerts.unregister(id);
                }
            }
            it.seen
        });
        if added {
            self.items.sort_by(|a, b| natural_cmp(a.disp(), b.disp())); // IO-045
        }

        // IO-050..IO-052: evaluate the stats of the active mode on displayed rows.
        for i in 0..self.items.len() {
            let show = self.shown(&self.items[i], ui);
            let it = &mut self.items[i];
            it.roles = [Role::Default; 2];
            if !show {
                continue;
            }
            let (base, vals) = match ui.diskio_mode {
                DiskioMode::Bytes => (0, [it.read_rate, it.write_rate]),
                DiskioMode::Latency => (2, [it.read_lat, it.write_lat]),
                DiskioMode::Iops => continue,
            };
            for k in 0..2 {
                if let (Some(id), Some(v)) = (it.ids[base + k], vals[k]) {
                    it.roles[k] = alerts.eval(id, v, 100.0);
                }
            }
        }
        self.last = Some(ctx.now);
    }
}

/// IO-040: mean ms per operation from raw deltas; 0 when no operations; absent when the
/// platform has no busy time.
fn latency(now: Option<u64>, prev: Option<u64>, dcount: Option<u64>) -> Option<f64> {
    let dt = delta(now?, prev?)?;
    let dc = dcount?;
    Some(if dc == 0 { 0.0 } else { dt as f64 / dc as f64 })
}

fn new_item(b: &PDisk, filter: &Filter, aliases: &Aliases, tcfg: &Config, alerts: &mut Alerts) -> DiskItem {
    let alias = aliases.get(&b.name).map(str::to_string);
    let allowed = filter.allows(&[&b.name, alias.as_deref().unwrap_or("")]);
    let mut ids = [None; 4];
    if allowed {
        let a = alias.as_deref();
        // IO-050 (no default), IO-051 (latency default), IO-053 field names.
        ids[0] = register_item(tcfg, alerts, MODULE, &b.name, a, Some("read"), None);
        ids[1] = register_item(tcfg, alerts, MODULE, &b.name, a, Some("write"), None);
        ids[2] = register_item(tcfg, alerts, MODULE, &b.name, a, Some("read_latency"), Some(LAT_DEFAULT));
        ids[3] = register_item(tcfg, alerts, MODULE, &b.name, a, Some("write_latency"), Some(LAT_DEFAULT));
    }
    DiskItem {
        name: b.name.clone(),
        alias,
        allowed,
        seen: true,
        prev: None,
        read_rate: None,
        write_rate: None,
        read_iops: None,
        write_iops: None,
        read_lat: None,
        write_lat: None,
        r_vis: false,
        w_vis: false,
        ids,
        roles: [Role::Default; 2],
    }
}

/// IO-047 latency format: one decimal below 10, integers up to 999, then auto-unit
/// low precision.
fn latency_text(v: f64) -> Small {
    if v < 9.95 {
        crate::small!("{:.1}", v)
    } else if v < 999.5 {
        crate::small!("{:.0}", v)
    } else {
        auto_unit_f(v, true, MinSym::None)
    }
}

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    let d = &w.diskio;
    let ui = &w.ui;
    let Some(n) = name_width(area) else { return Align::Top };
    if !d.items.iter().any(|it| d.shown(it, ui)) {
        return Align::Top; // IO-005
    }
    let mode = ui.diskio_mode;
    let cols: [&str; 2] = match mode {
        DiskioMode::Bytes => ["R/s", "W/s"],
        DiskioMode::Iops => ["IOR/s", "IOW/s"],
        DiskioMode::Latency => ["ms/opR", "ms/opW"],
    };
    header(out, "DISK I/O", n, &cols, 7);
    let mut rows = 1u16;
    for it in d.items.iter().filter(|it| d.shown(it, ui)) {
        if rows >= area.max_height {
            break; // IO-006
        }
        rows += 1;
        out.newline();
        out.addf(Role::Default, 0, format_args!("{}", Fit { s: it.disp(), width: n, mode: Trunc::Head }));
        let vals = match mode {
            DiskioMode::Bytes => [it.read_rate, it.write_rate],
            DiskioMode::Iops => [it.read_iops, it.write_iops],
            DiskioMode::Latency => [it.read_lat, it.write_lat],
        };
        for k in 0..2 {
            match vals[k] {
                Some(v) => {
                    let s = match mode {
                        DiskioMode::Latency => latency_text(v),
                        _ => auto_unit_f(v.round(), false, MinSym::None),
                    };
                    cell(out, it.roles[k], s.as_str(), 7);
                }
                None => cell(out, Role::Default, "-", 7),
            }
        }
    }
    Align::Top
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_math() {
        assert_eq!(latency(Some(30), Some(10), Some(40)), Some(0.5));
        assert_eq!(latency(Some(30), Some(10), Some(0)), Some(0.0));
        assert_eq!(latency(None, Some(10), Some(4)), None);
        assert_eq!(latency(Some(5), Some(10), Some(4)), None);
    }

    #[test]
    fn latency_format() {
        assert_eq!(latency_text(0.26).as_str(), "0.3");
        assert_eq!(latency_text(9.9).as_str(), "9.9");
        assert_eq!(latency_text(12.4).as_str(), "12");
    }
}
