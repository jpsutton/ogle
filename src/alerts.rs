//! Alerts and thresholds (spec 05-alerts.md is the source of truth).
//!
//! OWNER: alerts agent fills `todo!()` bodies (and may add private items).
//! Signatures below are a contract used by module agents; do not change them.
//!
//! Hot-path rule: `eval` and `decorate` take pre-resolved `StatId`/`Limits` so no string
//! formatting or map lookups happen per cycle (07-performance.md).

use crate::config::Config;
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

pub struct Alerts {
    // private fields chosen by the alerts agent
    _private: (),
}

impl Alerts {
    pub fn new(cfg: &Config) -> Alerts {
        let _ = cfg;
        todo!()
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
        let _ = (cfg, module, item, field, default);
        todo!()
    }

    /// Register a stat in the threshold-state table. `name` is the stat name
    /// (e.g. "cpu.total", "fs[/home]"), `event_type` the event label (e.g. "CPU_TOTAL").
    /// Registering the same name twice returns the existing id.
    pub fn register(&mut self, name: &str, event_type: &str, limits: Limits) -> StatId {
        let _ = (name, event_type, limits);
        todo!()
    }

    /// Forget a stat whose item disappeared (closes any ongoing event).
    pub fn unregister(&mut self, id: StatId) {
        let _ = id;
        todo!()
    }

    /// Evaluate a registered stat: value% = current*100/maximum. Updates state,
    /// creates/updates/closes events if `log`, returns the decoration role
    /// (`*_LOG` variant when logged).
    pub fn eval(&mut self, id: StatId, current: f64, maximum: f64) -> Role {
        let _ = (id, current, maximum);
        todo!()
    }

    /// Colour-only evaluation (ALR-007): never registers, never logs.
    pub fn decorate(limits: &Limits, current: f64, maximum: f64) -> Role {
        let _ = (limits, current, maximum);
        todo!()
    }

    /// Level only (for callers that need the level, e.g. MAX rule).
    pub fn level(limits: &Limits, current: f64, maximum: f64) -> Level {
        let _ = (limits, current, maximum);
        todo!()
    }

    /// Called once per cycle after all modules updated. Finalises events, captures
    /// top processes where required. `wall` = Unix seconds now.
    pub fn end_cycle(&mut self, wall: f64, procs: Option<&dyn TopProcesses>) {
        let _ = (wall, procs);
        todo!()
    }

    /// Sort key that auto-sort should use now (ALR auto-sort contract).
    pub fn auto_sort(&self) -> AutoSort {
        todo!()
    }

    /// `w` key (ALR-034).
    pub fn clear_finished(&mut self) {
        todo!()
    }

    /// `x` key (ALR-035).
    pub fn clear_all_finished(&mut self) {
        todo!()
    }
}
