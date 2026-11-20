//! Module cpu: whole-machine CPU use, plus the shared per-CPU sample used by percpu and
//! quicklook (02-system-modules.md sections 3 and 7, SYS-011, SYS-014..SYS-016, SYS-050..SYS-058).
//!
//! Also holds small helpers shared by the system-module renderers.

use std::time::{Duration, Instant};

use crate::alerts::{Alerts, Limits, StatId};
use crate::config::Config;
use crate::fmt::{auto_unit_f, MinSym};
use crate::modules::{Area, Ctx, ModuleId, UiState, World};
use crate::platform::{CpuCounters, CpuTimes, Platform, Sys};
use crate::ui::panel::{flags, Align, Panel, Role};

// Percentage field indices (SYS-050).
pub const TOTAL: usize = 0;
pub const USER: usize = 1;
pub const SYSTEM: usize = 2;
pub const IDLE: usize = 3;
pub const NICE: usize = 4;
pub const IOWAIT: usize = 5;
pub const IRQ: usize = 6;
pub const STEAL: usize = 7;
pub const GUEST: usize = 8;
pub const DPC: usize = 9;
pub const NF: usize = 10;
/// Field names, also the threshold field names and the percpu column headers.
pub const NAMES: [&str; NF] = ["total", "user", "system", "idle", "nice", "iowait", "irq", "steal", "guest", "dpc"];

// Rate field indices (SYS-051).
const CTX: usize = 0;
const NR: usize = 4;
const RATE_NAMES: [&str; NR] = ["ctx_switches", "interrupts", "soft_interrupts", "syscalls"];
const RATE_LABELS: [&str; NR] = ["ctx_sw", "inter", "sw_int", "sys_call"];

/// Percentages for one CPU (or the whole machine). `None` = unknown or absent.
pub type Pct = [Option<f64>; NF];

pub struct Cpu {
    refresh: Option<Duration>,
    logical: u32,
    topo_stale: bool,
    max_cpu_display: usize,

    prev: CpuTimes,
    cur: CpuTimes,
    have_prev: bool,
    pub(crate) pct: Pct,
    /// Field exists on this platform (from the last sample).
    pub(crate) present: [bool; NF],

    per_prev: Vec<CpuTimes>,
    per_cur: Vec<CpuTimes>,
    per_have: bool,
    /// Per-core percentages, index = core number. Empty when percpu mode is off.
    pub(crate) per_pct: Vec<Pct>,
    /// SYS-063 ordering: core numbers; the first `shown` are displayed.
    pub(crate) order: Vec<u16>,
    pub(crate) shown: usize,
    /// Mean of the cores that are not shown (`CPU*` row), if any.
    pub(crate) summary: Option<Pct>,
    /// `percpu.<field>` decoration limits (SYS-066).
    pub(crate) percpu_limits: [Option<Limits>; NF],

    prev_counters: [Option<u64>; NR],
    prev_t: Option<Instant>,
    rates: [Option<f64>; NR],
    rate_present: [bool; NR],

    ids: [Option<StatId>; NF],
    roles: [Role; NF],
    rate_ids: [Option<StatId>; NR],
    rate_roles: [Role; NR],
}

impl Cpu {
    pub fn new(cfg: &Config, ui: &UiState, alerts: &mut Alerts) -> Self {
        let _ = ui;
        let mut ids = [None; NF];
        let mut percpu_limits = [None; NF];
        for f in 0..NF {
            // SYS-055: defaults; other fields are evaluated only when the user sets limits.
            let def = match f {
                TOTAL => Some(Limits::new(65.0, 75.0, 85.0, true)),
                IOWAIT => Some(Limits::new(50.0, 70.0, 90.0, true)),
                USER | SYSTEM | STEAL => Some(Limits::new(50.0, 70.0, 90.0, false)),
                _ => None,
            };
            ids[f] = register(cfg, alerts, "cpu", NAMES[f], def);
            // SYS-066: decoration only.
            let pdef = match f {
                USER | SYSTEM | IOWAIT => Some(Limits::new(50.0, 70.0, 90.0, false)),
                _ => None,
            };
            percpu_limits[f] = Alerts::resolve(cfg, "percpu", None, Some(NAMES[f]), pdef);
        }
        let mut rate_ids = [None; NR];
        for i in 0..NR {
            let def = if i == CTX { Some(Limits::new(40000.0, 45000.0, 50000.0, false)) } else { None };
            rate_ids[i] = register(cfg, alerts, "cpu", RATE_NAMES[i], def);
        }
        let mut present = [false; NF];
        for f in [TOTAL, USER, SYSTEM, IDLE] {
            present[f] = true;
        }
        Cpu {
            refresh: cfg_refresh(cfg, "cpu"),
            logical: 0,
            topo_stale: true,
            max_cpu_display: cfg.i64(&["percpu", "max_cpu_display"]).filter(|v| *v > 0).unwrap_or(4) as usize,
            prev: CpuTimes::default(),
            cur: CpuTimes::default(),
            have_prev: false,
            pct: [None; NF],
            present,
            per_prev: Vec::new(),
            per_cur: Vec::new(),
            per_have: false,
            per_pct: Vec::new(),
            order: Vec::new(),
            shown: 0,
            summary: None,
            percpu_limits,
            prev_counters: [None; NR],
            prev_t: None,
            rates: [None; NR],
            rate_present: [false; NR],
            ids,
            roles: [Role::Default; NF],
            rate_ids,
            rate_roles: [Role::Default; NR],
        }
    }

    pub fn refresh(&self) -> Option<Duration> {
        self.refresh
    }

    /// Logical core count (SYS-013), 0 until the first update.
    pub fn logical_cores(&self) -> u32 {
        self.logical
    }

    /// Shared busy percentage (SYS-015), None until two samples exist.
    pub fn total(&self) -> Option<f64> {
        self.pct[TOTAL]
    }

    /// Explicit refresh (F5): re-read the core counts on the next update (SYS-013).
    pub fn reset_caches(&mut self) {
        self.topo_stale = true;
    }

    pub fn update(&mut self, sys: &mut Sys, ctx: &Ctx, ui: &UiState, alerts: &mut Alerts) {
        if self.topo_stale || self.logical == 0 {
            self.logical = sys.cpu_topology().logical;
            self.topo_stale = false;
        }
        // SYS-061: per-core times only in percpu mode. SYS-011: one cpu_times call per cycle.
        let want_per = ui.percpu;
        let ok = sys.cpu_times(&mut self.cur, if want_per { Some(&mut self.per_cur) } else { None });
        if ok {
            let c = &self.cur;
            self.present = [true, true, true, true, c.nice.is_some(), c.iowait.is_some(), c.irq.is_some(), c.steal.is_some(), c.guest.is_some(), c.dpc.is_some()];
            if self.have_prev {
                compute(&self.prev, &self.cur, &mut self.pct);
            }
            self.prev = self.cur;
            self.have_prev = true;
        }

        if want_per && ok {
            let n = self.per_cur.len();
            if self.per_have && self.per_prev.len() == n {
                self.per_pct.resize(n, [None; NF]);
                for i in 0..n {
                    compute(&self.per_prev[i], &self.per_cur[i], &mut self.per_pct[i]);
                }
            } else {
                if self.per_have {
                    // CPU hotplug: core count changed (PLT-040).
                    self.topo_stale = true;
                }
                self.per_pct.clear();
                self.per_pct.resize(n, [None; NF]);
            }
            std::mem::swap(&mut self.per_prev, &mut self.per_cur);
            self.per_have = true;
            self.order_cores();
        } else if !want_per {
            self.per_have = false;
            self.per_prev.clear();
            self.per_pct.clear();
            self.order.clear();
            self.shown = 0;
            self.summary = None;
        }

        // SYS-051, GEN-007: per-second rates, unknown on the first sample or on wrap.
        let cnt = counters(&sys.cpu_counters());
        let dt = self.prev_t.map(|t| ctx.now.saturating_duration_since(t).as_secs_f64());
        for i in 0..NR {
            self.rate_present[i] = cnt[i].is_some();
            self.rates[i] = match (self.prev_counters[i], cnt[i], dt) {
                (Some(p), Some(c), Some(dt)) if c >= p && dt > 0.0 => Some((c - p) as f64 / dt),
                _ => None,
            };
        }
        self.prev_counters = cnt;
        self.prev_t = Some(ctx.now);

        // SYS-055 thresholds. Absent or unknown fields are not evaluated.
        for f in 0..NF {
            self.roles[f] = match (self.ids[f], self.pct[f]) {
                (Some(id), Some(v)) if self.present[f] => alerts.eval(id, v, 100.0),
                _ => Role::Default,
            };
        }
        for i in 0..NR {
            let v = match self.rates[i] {
                // SYS-055/SYS-056: switches per second per core, only once a rate exists.
                Some(r) if i == CTX => (self.logical > 0).then(|| r / self.logical as f64),
                v => v,
            };
            self.rate_roles[i] = match (self.rate_ids[i], v) {
                (Some(id), Some(v)) => alerts.eval(id, v, 100.0),
                _ => Role::Default,
            };
        }
    }

    /// SYS-063: core selection and ordering shared by percpu and quicklook.
    fn order_cores(&mut self) {
        let n = self.per_pct.len();
        self.order.clear();
        self.order.extend(0..n as u16);
        if n <= self.max_cpu_display {
            self.shown = n;
            self.summary = None;
            return;
        }
        let per = &self.per_pct;
        let tot = |i: u16| per[i as usize][TOTAL].unwrap_or(-1.0);
        // Unstable sort does not allocate; ties are broken by core number.
        self.order.sort_unstable_by(|&a, &b| tot(b).partial_cmp(&tot(a)).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b)));
        self.shown = self.max_cpu_display;
        let mut sum = [0.0f64; NF];
        let mut cnt = [0u32; NF];
        for &c in &self.order[self.shown..] {
            for f in 0..NF {
                if let Some(v) = per[c as usize][f] {
                    sum[f] += v;
                    cnt[f] += 1;
                }
            }
        }
        let mut s = [None; NF];
        for f in 0..NF {
            if cnt[f] > 0 {
                s[f] = Some(sum[f] / cnt[f] as f64);
            }
        }
        self.summary = Some(s);
    }
}

/// SYS-014..SYS-016. Returns false (and keeps `out`) when no time elapsed.
fn compute(prev: &CpuTimes, cur: &CpuTimes, out: &mut Pct) -> bool {
    let dt = cur.total() - prev.total();
    if !(dt > 0.0) {
        return false;
    }
    let (a, b) = (times(prev), times(cur));
    for f in 1..NF {
        out[f] = match (a[f], b[f]) {
            (Some(x), Some(y)) => Some(((y - x) / dt * 100.0).clamp(0.0, 100.0)),
            _ => None,
        };
    }
    // SYS-015: iowait counts as idle.
    out[TOTAL] = Some((100.0 - out[IDLE].unwrap_or(0.0) - out[IOWAIT].unwrap_or(0.0)).clamp(0.0, 100.0));
    true
}

fn times(t: &CpuTimes) -> Pct {
    [None, Some(t.user), Some(t.system), Some(t.idle), t.nice, t.iowait, t.irq, t.steal, t.guest, t.dpc]
}

fn counters(c: &CpuCounters) -> [Option<u64>; NR] {
    [c.ctx_switches, c.interrupts, c.soft_interrupts, c.syscalls]
}

#[derive(Clone, Copy)]
enum Slot {
    P(usize),
    R(usize),
}

/// Pick the first 4 present slots of a column (SYS-052).
fn column(c: &Cpu, list: &[Slot]) -> ([Slot; 4], usize) {
    let mut out = [Slot::P(0); 4];
    let mut n = 0;
    for &s in list {
        let here = match s {
            Slot::P(f) => c.present[f],
            Slot::R(i) => c.rate_present[i],
        };
        if here && n < 4 {
            out[n] = s;
            n += 1;
        }
    }
    (out, n)
}

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    let c = &w.cpu;
    // SYS-052 slot lists. Column 1 line 1 is the title + total.
    let (c1, n1) = column(c, &[Slot::P(USER), Slot::P(SYSTEM), Slot::P(IOWAIT), Slot::P(DPC)]);
    let n1 = n1.min(3);
    let (c2, mut n2) = column(c, &[Slot::P(IDLE), Slot::P(IRQ), Slot::P(NICE), Slot::P(STEAL)]);
    let (c3, mut n3) = column(c, &[Slot::R(0), Slot::R(1), Slot::R(2), Slot::P(GUEST), Slot::R(3)]);
    if area.narrow {
        // SYS-053: columns 2 and 3 are optional.
        n2 = 0;
        n3 = 0;
    }
    let height = (1 + n1).max(n2).max(n3).min(area.max_height.max(1) as usize);
    let opt = flags::OPTIONAL;
    for i in 0..height {
        if i > 0 {
            out.newline();
        }
        let later = i < n2 || i < n3;
        if i == 0 {
            out.addf(Role::Title, 0, format_args!("{:<8}", "CPU"));
            pct_val(out, 0, c.roles[TOTAL], c.pct[TOTAL], 6);
        } else if i <= n1 {
            if let Slot::P(f) = c1[i - 1] {
                label(out, 0, "", NAMES[f], 8);
                pct_val(out, 0, c.roles[f], c.pct[f], 6);
            }
        } else if later {
            pad(out, 0, 14);
        }
        if i < n2 {
            if let Slot::P(f) = c2[i] {
                if f == IDLE {
                    label(out, opt, "  ", NAMES[f], 8);
                    pct_val(out, opt, c.roles[f], c.pct[f], 5);
                } else {
                    label(out, opt, "  ", NAMES[f], 7);
                    pct_val(out, opt, c.roles[f], c.pct[f], 6);
                }
            }
        } else if n2 > 0 && i < n3 {
            pad(out, opt, 15);
        }
        if i < n3 {
            match c3[i] {
                Slot::P(f) => {
                    label(out, opt, "  ", NAMES[f], 8);
                    pct_val(out, opt, c.roles[f], c.pct[f], 5);
                }
                Slot::R(r) => {
                    label(out, opt, "  ", RATE_LABELS[r], 8);
                    // SYS-054: `-` until two samples exist.
                    match c.rates[r] {
                        Some(v) => out.addf(c.rate_roles[r], opt, format_args!("{:>5}", auto_unit_f(v, false, MinSym::K))),
                        None => out.addf(Role::Default, opt, format_args!("{:>5}", "-")),
                    }
                }
            }
        }
    }
    Align::Top
}

// ---- helpers shared by the system-module renderers ----

/// `[<module>] refresh` in seconds, if set and valid.
pub(crate) fn cfg_refresh(cfg: &Config, module: &str) -> Option<Duration> {
    cfg.f64(&[module, "refresh"]).filter(|v| v.is_finite() && *v > 0.0).map(Duration::from_secs_f64)
}

/// Resolve `[module.thresholds]` for `field` and register `module.field` when limits exist.
fn register(cfg: &Config, alerts: &mut Alerts, module: &str, field: &str, def: Option<Limits>) -> Option<StatId> {
    let lim = Alerts::resolve(cfg, module, None, Some(field), def)?;
    let stat = format!("{module}.{field}");
    let ev = format!("{}_{}", module.to_ascii_uppercase(), field.to_ascii_uppercase());
    Some(alerts.register(&stat, &ev, lim))
}

/// Quicklook is visible (it consumes cpu frequency and per-core rows).
pub(crate) fn ql_visible(ui: &UiState) -> bool {
    ui.top && ui.on(ModuleId::Quicklook) && (ui.quicklook || ui.full_quicklook)
}

/// `n` blank cells.
pub(crate) fn pad(out: &mut Panel, fl: u8, n: usize) {
    if n > 0 {
        out.addf(Role::Default, fl, format_args!("{:n$}", "", n = n));
    }
}

/// `<prefix><label padded to l>` (SYS-007).
pub(crate) fn label(out: &mut Panel, fl: u8, prefix: &str, text: &str, l: usize) {
    out.addf(Role::Default, fl, format_args!("{prefix}{text:<l$}", l = l));
}

/// Percentage value cell `w` wide including `%`; `-` when unknown (SYS-005, SYS-006).
pub(crate) fn pct_val(out: &mut Panel, fl: u8, role: Role, v: Option<f64>, w: usize) {
    match v {
        Some(v) => out.addf(role, fl, format_args!("{:>w$.1}%", v.max(0.0), w = w - 1)),
        None => out.addf(Role::Default, fl, format_args!("{:>w$}", "-", w = w)),
    }
}
