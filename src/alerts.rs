//! Alerts and thresholds (spec 05-alerts.md is the source of truth).
//!
//! The public API below is used by every module that evaluates thresholds.
//!
//! Hot-path rule: `eval` and `decorate` take pre-resolved `StatId`/`Limits` so no string
//! formatting or map lookups happen per cycle (07-performance.md).

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::config::{Config, Table, Value};
use crate::ui::panel::Role;

/// Resolved careful/warning/critical limits, in percent-of-maximum units (ALR-0xx).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Limits {
    pub careful: Option<f64>,
    pub warning: Option<f64>,
    pub critical: Option<f64>,
    /// Record events for this stat (ALR `log`).
    pub log: bool,
    /// Hysteresis in percent-of-maximum units (0 = Glances behaviour).
    pub hysteresis: f64,
}

impl Limits {
    pub const fn new(careful: f64, warning: f64, critical: f64, log: bool) -> Limits {
        Limits { careful: Some(careful), warning: Some(warning), critical: Some(critical), log, hysteresis: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// No limits defined.
    Default,
    Ok,
    Careful,
    Warning,
    Critical,
}

/// Handle to a registered stat (05 threshold-state table).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StatId(pub u32);

/// Hint for automatic process sorting (ALR auto-sort contract, PROC-06x).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoSort {
    Cpu,
    Mem,
    Io,
}

/// Implemented by the processes module so the event engine can capture the
/// top-3 process names when an event is created / reaches CRITICAL (ALR-02x).
pub trait TopProcesses {
    /// Fill `out` with up to `n` process names from the UNFILTERED list sorted by `key`.
    fn top_names(&self, key: AutoSort, n: usize, out: &mut Vec<String>);
}

/// One row of the threshold-state table (ALR-006).
struct Stat {
    limits: Limits,
    state: Level,
    /// Index into `Alerts::types`.
    ty: u32,
    live: bool,
}

/// Interned event type: stored once at register time and never freed, because finished
/// events may outlive the stat that produced them.
struct EventType {
    name: Box<str>,
    sort: AutoSort,
    /// An event of this type is ongoing (ALR-009 fast skip).
    ongoing: bool,
}

/// Event record (ALR-016). Read by the alert panel renderer.
pub(crate) struct Event {
    pub(crate) ty: u32,
    /// `Warning` or `Critical`; only escalates (ALR-020).
    pub(crate) state: Level,
    /// Wall clock of creation, Unix seconds (display only).
    pub(crate) begin_wall: f64,
    pub(crate) begin_mono: Instant,
    /// `None` = ongoing.
    pub(crate) end_mono: Option<Instant>,
    pub(crate) min: f64,
    pub(crate) max: f64,
    pub(crate) sum: f64,
    pub(crate) count: u32,
    /// Up to 3 process names (ALR-021).
    pub(crate) top: Vec<String>,
    top_counts: Vec<(String, u32)>,
    /// A CRITICAL input arrived this cycle; the top-process step runs in `end_cycle`
    /// because only then is the process list available.
    pending_top: bool,
}

/// ALR-027 global message table (row 0, "EVENTS history", is the fallback).
const MSG_ROWS: [(&str, &str); 8] = [
    ("cpu.user", "High CPU user mode"),
    ("cpu.system", "High CPU kernel usage"),
    ("cpu.iowait", "High CPU I/O waiting"),
    ("cpu.steal", "Large CPU stolen time. System running the hypervisor is too busy."),
    ("cpu.nice", "High CPU niced value"),
    ("load", "System overloaded in the last 5 minutes"),
    ("memswap", "High swap (paging) usage"),
    ("mem", "High memory consumption"),
];
const MSG_MIN_WEIGHT: u32 = 2;
const EVENTS_HISTORY: &str = "EVENTS history";
const NO_STAT: u32 = u32::MAX;

pub struct Alerts {
    stats: Vec<Stat>,
    names: HashMap<Box<str>, u32>,
    free: Vec<u32>,
    types: Vec<EventType>,
    /// Newest first; capacity `max_events + 1`.
    events: Vec<Event>,
    /// Stat ids for the rows of `MSG_ROWS`.
    msg_ids: [u32; 8],
    max_events: usize,
    min_duration: Duration,
    min_interval: Duration,
    /// Reused buffer for `TopProcesses::top_names`.
    scratch: Vec<String>,
}

impl Alerts {
    /// Reads `[alert]` (ALR-041). Warnings go to stderr, so call before the UI starts.
    pub fn new(cfg: &Config) -> Alerts {
        // ALR-042: out-of-range values warn and fall back to the default.
        let max_events = alert_num(cfg, "max_events", 10.0, |v| v >= 1.0 && v.fract() == 0.0) as usize;
        let min_duration = alert_num(cfg, "min_duration", 6.0, |v| v >= 0.0);
        let min_interval = alert_num(cfg, "min_interval", 6.0, |v| v >= 0.0);
        let _ = alert_num(cfg, "hysteresis", 0.0, |v| v >= 0.0);
        warn_unordered(cfg);
        Alerts {
            stats: Vec::new(),
            names: HashMap::new(),
            free: Vec::new(),
            types: Vec::new(),
            events: Vec::with_capacity(max_events + 1),
            msg_ids: [NO_STAT; 8],
            max_events,
            min_duration: Duration::from_secs_f64(min_duration),
            min_interval: Duration::from_secs_f64(min_interval),
            scratch: Vec::with_capacity(6),
        }
    }

    /// Resolve limits from `[<module>.thresholds]` using the 05 lookup order:
    /// item+field, item, field, module-wide, then `default`. Returns None when the
    /// resolved level is disabled (`false`) or nothing is defined.
    /// Call at init or when a new item (interface, disk, mount, sensor) first appears.
    pub fn resolve(
        cfg: &Config,
        module: &str,
        item: Option<&str>,
        field: Option<&str>,
        default: Option<Limits>,
    ) -> Option<Limits> {
        // ALR-011 / ALR-012: the applicable sets, most specific first.
        let thr = cfg.table(&[module, "thresholds"]);
        let item_set = match (thr, item) {
            (Some(t), Some(i)) => sub_table(t, "items").and_then(|it| sub_table(it, i)),
            _ => None,
        };
        let item_field = match (item_set, field) {
            (Some(t), Some(f)) => sub_table(t, f),
            _ => None,
        };
        let field_set = match (thr, field) {
            (Some(t), Some(f)) => sub_table(t, f),
            _ => None,
        };
        let sets = [item_field, item_set, field_set, thr];

        // Per-key lookup: the first set that defines the key wins; `false` disables (ALR-014).
        let level = |key: &str, def: Option<f64>| -> Option<f64> {
            for s in sets.iter().flatten() {
                match s.get(key) {
                    Some(Value::Int(i)) => return Some(*i as f64),
                    Some(Value::Float(f)) => return Some(*f),
                    Some(Value::Bool(false)) => return None,
                    _ => {}
                }
            }
            def
        };
        let d = default.unwrap_or_default();
        let careful = level("careful", d.careful);
        let warning = level("warning", d.warning);
        let critical = level("critical", d.critical);
        if careful.is_none() && warning.is_none() && critical.is_none() {
            return None;
        }
        let mut log = None;
        let mut hyst = None;
        for s in sets.iter().flatten() {
            if log.is_none() {
                if let Some(Value::Bool(b)) = s.get("log") {
                    log = Some(*b);
                }
            }
            if hyst.is_none() {
                match s.get("hysteresis") {
                    Some(Value::Int(i)) if *i >= 0 => hyst = Some(*i as f64),
                    Some(Value::Float(f)) if *f >= 0.0 => hyst = Some(*f),
                    _ => {}
                }
            }
        }
        let log = log.unwrap_or(d.log);
        // Step 6: a compiled-in hysteresis of 0 means "not set", so fall back to [alert].
        let hysteresis = hyst.unwrap_or(if d.hysteresis > 0.0 {
            d.hysteresis
        } else {
            match cfg.f64(&["alert", "hysteresis"]) {
                Some(h) if h >= 0.0 => h,
                _ => 0.0,
            }
        });
        Some(Limits { careful, warning, critical, log, hysteresis })
    }

    /// Register a stat in the threshold-state table. `name` is the stat name
    /// (e.g. "cpu.total", "fs[/home]"), `event_type` the event label (e.g. "CPU_TOTAL").
    /// Registering the same name twice returns the existing id.
    pub fn register(&mut self, name: &str, event_type: &str, limits: Limits) -> StatId {
        if let Some(&id) = self.names.get(name) {
            self.stats[id as usize].limits = limits;
            return StatId(id);
        }
        let ty = match self.types.iter().position(|t| &*t.name == event_type) {
            Some(i) => i as u32,
            None => {
                // ALR-024: auto-sort key per event type.
                let sort = if event_type.starts_with("MEM") {
                    AutoSort::Mem
                } else if event_type.starts_with("CPU_IOWAIT") {
                    AutoSort::Io
                } else {
                    AutoSort::Cpu
                };
                self.types.push(EventType { name: event_type.into(), sort, ongoing: false });
                (self.types.len() - 1) as u32
            }
        };
        let stat = Stat { limits, state: Level::Default, ty, live: true };
        let id = match self.free.pop() {
            Some(id) => {
                self.stats[id as usize] = stat;
                id
            }
            None => {
                self.stats.push(stat);
                (self.stats.len() - 1) as u32
            }
        };
        self.names.insert(name.into(), id);
        if let Some(row) = MSG_ROWS.iter().position(|r| r.0 == name) {
            self.msg_ids[row] = id;
        }
        StatId(id)
    }

    /// Forget a stat whose item disappeared (closes any ongoing event).
    pub fn unregister(&mut self, id: StatId) {
        let Some(st) = self.stats.get_mut(id.0 as usize) else { return };
        if !st.live {
            return;
        }
        st.live = false;
        st.state = Level::Default;
        let ty = st.ty;
        self.names.retain(|_, v| *v != id.0);
        for m in self.msg_ids.iter_mut() {
            if *m == id.0 {
                *m = NO_STAT;
            }
        }
        self.free.push(id.0);
        if self.types[ty as usize].ongoing {
            if let Some(i) = self.events.iter().position(|e| e.ty == ty && e.end_mono.is_none()) {
                self.close(i, Instant::now());
            }
        }
    }

    /// Evaluate a registered stat: value% = current*100/maximum. Updates state,
    /// creates/updates/closes events if `log`, returns the decoration role
    /// (`*_LOG` variant when logged).
    pub fn eval(&mut self, id: StatId, current: f64, maximum: f64) -> Role {
        let Some(st) = self.stats.get_mut(id.0 as usize) else { return Role::Default };
        if !st.live {
            return Role::Default;
        }
        let raw = Self::level(&st.limits, current, maximum);
        let pct = current * 100.0 / maximum;
        // ALR-004: hysteresis only holds back downward moves.
        let lvl = if raw != Level::Default && st.limits.hysteresis > 0.0 && raw < st.state {
            hold(&st.limits, st.state, raw, pct)
        } else {
            raw
        };
        st.state = lvl;
        let log = st.limits.log;
        let ty = st.ty;
        // ALR-006 step 3; ALR-009: skip the engine when it would do nothing.
        if log && lvl != Level::Default && (lvl >= Level::Warning || self.types[ty as usize].ongoing) {
            self.feed(ty, lvl, pct);
        }
        role_for(lvl, log)
    }

    /// Colour-only evaluation (ALR-007): never registers, never logs.
    pub fn decorate(limits: &Limits, current: f64, maximum: f64) -> Role {
        role_for(Self::level(limits, current, maximum), false)
    }

    /// Level only (for callers that need the level, e.g. MAX rule).
    pub fn level(limits: &Limits, current: f64, maximum: f64) -> Level {
        // ALR-002 (highlight_zero and the minimum rule are applied by callers).
        if !current.is_finite() || !maximum.is_finite() || maximum == 0.0 {
            return Level::Default;
        }
        let pct = current * 100.0 / maximum;
        match (limits.careful, limits.warning, limits.critical) {
            (_, _, Some(c)) if pct >= c => Level::Critical,
            (_, Some(w), _) if pct >= w => Level::Warning,
            (Some(c), _, _) if pct >= c => Level::Careful,
            (None, None, None) => Level::Default,
            _ => Level::Ok,
        }
    }

    /// Called once per cycle after all modules updated. Finalises events, captures
    /// top processes where required. `wall` = Unix seconds now.
    pub fn end_cycle(&mut self, wall: f64, procs: Option<&dyn TopProcesses>) {
        let _ = wall;
        for i in 0..self.events.len() {
            if !self.events[i].pending_top {
                continue;
            }
            self.events[i].pending_top = false;
            // ALR-021: with process collection off, `top` stays unchanged.
            let Some(p) = procs else { continue };
            let key = self.types[self.events[i].ty as usize].sort;
            // The callee replaces the contents; not clearing here lets it reuse the
            // existing String slots instead of allocating new names each cycle.
            p.top_names(key, 6, &mut self.scratch);
            let e = &mut self.events[i];
            for name in self.scratch.iter().take(6) {
                match e.top_counts.iter_mut().find(|c| c.0 == *name) {
                    Some(c) => c.1 += 1,
                    None => e.top_counts.push((name.clone(), 1)),
                }
            }
            // Top 3 by count; ties keep first appearance.
            let mut picked = [usize::MAX; 3];
            let mut n = 0;
            while n < 3 {
                let mut best: Option<usize> = None;
                for (j, c) in e.top_counts.iter().enumerate() {
                    if picked[..n].contains(&j) {
                        continue;
                    }
                    if best.map_or(true, |b| c.1 > e.top_counts[b].1) {
                        best = Some(j);
                    }
                }
                let Some(b) = best else { break };
                picked[n] = b;
                n += 1;
            }
            e.top.truncate(n);
            for (k, &j) in picked[..n].iter().enumerate() {
                if k < e.top.len() {
                    e.top[k].clone_from(&e.top_counts[j].0);
                } else {
                    e.top.push(e.top_counts[j].0.clone());
                }
            }
        }
    }

    /// Sort key that auto-sort should use now (ALR auto-sort contract).
    pub fn auto_sort(&self) -> AutoSort {
        // ALR-025: most recently created ongoing event (the list is newest first).
        self.events
            .iter()
            .find(|e| e.end_mono.is_none())
            .map_or(AutoSort::Cpu, |e| self.types[e.ty as usize].sort)
    }

    /// `w` key (ALR-034).
    pub fn clear_finished(&mut self) {
        self.events.retain(|e| e.end_mono.is_none() || e.state == Level::Critical);
    }

    /// `x` key (ALR-035).
    pub fn clear_all_finished(&mut self) {
        self.events.retain(|e| e.end_mono.is_none());
    }

    /// Event list, newest first (alert panel).
    pub(crate) fn events(&self) -> &[Event] {
        &self.events
    }

    pub(crate) fn type_name(&self, ty: u32) -> &str {
        &self.types[ty as usize].name
    }

    /// Panel title (ALR-027, ALR-028), computed live.
    pub(crate) fn title(&self) -> &'static str {
        if !self.events.iter().any(|e| e.end_mono.is_none()) {
            return EVENTS_HISTORY;
        }
        // Highest weight wins; ties keep the earlier row (strict `>`).
        let mut best = (0u32, EVENTS_HISTORY);
        for (row, &id) in self.msg_ids.iter().enumerate() {
            let w = match self.stats.get(id as usize) {
                Some(s) if s.live => match s.state {
                    Level::Careful => 1,
                    Level::Warning => 2,
                    Level::Critical => 3,
                    _ => 0,
                },
                _ => 0,
            };
            if w > best.0 {
                best = (w, MSG_ROWS[row].1);
            }
        }
        if best.0 >= MSG_MIN_WEIGHT { best.1 } else { EVENTS_HISTORY }
    }

    /// Event engine input (ALR-017..ALR-022).
    fn feed(&mut self, ty: u32, lvl: Level, pct: f64) {
        let now = Instant::now();
        let min_interval = self.min_interval;
        // ALR-017: ongoing, or finished less than min_interval ago (monotonic).
        let pos = self.events.iter().position(|e| {
            e.ty == ty
                && match e.end_mono {
                    None => true,
                    Some(end) => now.saturating_duration_since(end) < min_interval,
                }
        });
        match pos {
            None => {
                if lvl < Level::Warning {
                    return;
                }
                // ALR-018: create at the front; top step on CRITICAL creation.
                let begin_wall = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
                self.events.insert(
                    0,
                    Event {
                        ty,
                        state: lvl,
                        begin_wall,
                        begin_mono: now,
                        end_mono: None,
                        min: pct,
                        max: pct,
                        sum: pct,
                        count: 1,
                        top: Vec::new(),
                        top_counts: Vec::new(),
                        pending_top: lvl == Level::Critical,
                    },
                );
                self.types[ty as usize].ongoing = true;
                // ALR-022: drop the oldest finished event, else the oldest ongoing one.
                if self.events.len() > self.max_events {
                    let last = self.events.len() - 1;
                    let i = self.events.iter().rposition(|e| e.end_mono.is_some()).unwrap_or(last);
                    let e = self.events.remove(i);
                    if e.end_mono.is_none() {
                        self.types[e.ty as usize].ongoing = false;
                    }
                }
            }
            // ALR-019
            Some(i) if lvl < Level::Warning => self.close(i, now),
            Some(i) => {
                // ALR-020: reopen, update statistics, state only escalates.
                let e = &mut self.events[i];
                e.end_mono = None;
                e.min = e.min.min(pct);
                e.max = e.max.max(pct);
                e.sum += pct;
                e.count += 1;
                if lvl > e.state {
                    e.state = lvl;
                }
                if lvl == Level::Critical {
                    e.pending_top = true;
                } else {
                    e.top_counts.clear();
                    e.pending_top = false;
                }
                self.types[ty as usize].ongoing = true;
            }
        }
    }

    /// ALR-019: close an ongoing event; events shorter than min_duration are noise.
    fn close(&mut self, i: usize, now: Instant) {
        let min_duration = self.min_duration;
        let e = &mut self.events[i];
        if e.end_mono.is_some() {
            return;
        }
        let keep = now.saturating_duration_since(e.begin_mono) >= min_duration;
        e.pending_top = false;
        if keep {
            e.end_mono = Some(now);
        }
        let ty = e.ty;
        self.types[ty as usize].ongoing = false;
        if !keep {
            self.events.remove(i);
        }
    }
}

/// ALR-004: keep the previous (higher) state while value% stays within `h` of its bound.
fn hold(l: &Limits, prev: Level, raw: Level, pct: f64) -> Level {
    let mut lv = prev;
    while lv > raw {
        let (bound, lower) = match lv {
            Level::Critical => (l.critical, Level::Warning),
            Level::Warning => (l.warning, Level::Careful),
            Level::Careful => (l.careful, Level::Ok),
            _ => return raw,
        };
        if let Some(b) = bound {
            if pct >= b - l.hysteresis {
                return lv;
            }
        }
        lv = lower;
    }
    raw
}

/// ALR-038: decoration for a state.
fn role_for(l: Level, log: bool) -> Role {
    match (l, log) {
        (Level::Default, _) => Role::Default,
        (Level::Ok, false) => Role::Ok,
        (Level::Careful, false) => Role::Careful,
        (Level::Warning, false) => Role::Warning,
        (Level::Critical, false) => Role::Critical,
        (Level::Ok, true) => Role::OkLog,
        (Level::Careful, true) => Role::CarefulLog,
        (Level::Warning, true) => Role::WarningLog,
        (Level::Critical, true) => Role::CriticalLog,
    }
}

fn sub_table<'a>(t: &'a Table, key: &str) -> Option<&'a Table> {
    match t.get(key) {
        Some(Value::Table(s)) => Some(s),
        _ => None,
    }
}

/// Read a numeric `[alert]` key; mistyped or out-of-range values warn and use the default (ALR-042).
fn alert_num(cfg: &Config, key: &str, default: f64, ok: impl Fn(f64) -> bool) -> f64 {
    if cfg.get(&["alert", key]).is_none() {
        return default;
    }
    match cfg.f64(&["alert", key]) {
        Some(v) if ok(v) => v,
        _ => {
            eprintln!("ogle: [alert] {key}: invalid value, using default {default}");
            default
        }
    }
}

/// ALR-003: one stderr warning per configured limit set whose limits are not ordered.
fn warn_unordered(cfg: &Config) {
    fn num(t: &Table, k: &str) -> Option<f64> {
        match t.get(k) {
            Some(Value::Int(i)) => Some(*i as f64),
            Some(Value::Float(f)) => Some(*f),
            _ => None,
        }
    }
    fn check(t: &Table, module: &str, place: &str) {
        let (c, w, k) = (num(t, "careful"), num(t, "warning"), num(t, "critical"));
        let bad = matches!((c, w), (Some(c), Some(w)) if c > w) || matches!((w, k), (Some(w), Some(k)) if w > k);
        if bad {
            eprintln!("ogle: [{module}.thresholds]{place}: limits not ordered careful <= warning <= critical");
        }
    }
    for (module, v) in &cfg.root {
        let Value::Table(m) = v else { continue };
        let Some(thr) = sub_table(m, "thresholds") else { continue };
        check(thr, module, "");
        for (k, v) in thr {
            let Value::Table(set) = v else { continue };
            if k != "items" {
                check(set, module, &format!(" {k}"));
                continue;
            }
            for (item, v) in set {
                let Value::Table(is) = v else { continue };
                check(is, module, &format!(" items.\"{item}\""));
                for (f, v) in is {
                    if let Value::Table(fs) = v {
                        check(fs, module, &format!(" items.\"{item}\".{f}"));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_order_and_inclusive() {
        let l = Limits::new(50.0, 70.0, 90.0, false);
        assert_eq!(Alerts::level(&l, 49.9, 100.0), Level::Ok);
        assert_eq!(Alerts::level(&l, 50.0, 100.0), Level::Careful);
        assert_eq!(Alerts::level(&l, 70.0, 100.0), Level::Warning);
        assert_eq!(Alerts::level(&l, 9.0, 10.0), Level::Critical);
        assert_eq!(Alerts::level(&l, 9.0, 0.0), Level::Default);
        assert_eq!(Alerts::level(&Limits::default(), 9.0, 100.0), Level::Default);
    }

    #[test]
    fn hysteresis_holds_downward() {
        let mut l = Limits::new(50.0, 70.0, 90.0, false);
        l.hysteresis = 5.0;
        assert_eq!(hold(&l, Level::Warning, Level::Careful, 66.0), Level::Warning);
        assert_eq!(hold(&l, Level::Warning, Level::Careful, 64.9), Level::Careful);
        assert_eq!(hold(&l, Level::Critical, Level::Ok, 40.0), Level::Ok);
        assert_eq!(hold(&l, Level::Critical, Level::Ok, 46.0), Level::Careful);
    }

    #[test]
    fn engine_create_close_and_sort() {
        let mut a = Alerts::new(&Config::default());
        a.min_duration = Duration::ZERO;
        let id = a.register("mem", "MEM", Limits::new(50.0, 70.0, 90.0, true));
        assert_eq!(a.eval(id, 75.0, 100.0), Role::WarningLog);
        assert_eq!(a.events().len(), 1);
        assert_eq!(a.auto_sort(), AutoSort::Mem);
        assert_eq!(a.title(), "High memory consumption");
        a.eval(id, 95.0, 100.0);
        assert_eq!(a.events()[0].state, Level::Critical);
        a.eval(id, 10.0, 100.0);
        assert!(a.events()[0].end_mono.is_some());
        assert_eq!(a.auto_sort(), AutoSort::Cpu);
        assert_eq!(a.title(), EVENTS_HISTORY);
        a.clear_finished();
        assert_eq!(a.events().len(), 1);
        a.clear_all_finished();
        assert!(a.events().is_empty());
    }
}
