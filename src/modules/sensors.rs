//! Sensors module (03-io-modules.md section 5): temperatures, fans, batteries.

use std::fmt::Write as _;
use std::time::Duration;

use crate::alerts::{Alerts, Limits, StatId};
use crate::config::Config;
use crate::fmt::Small;
use crate::modules::network::{
    cell, cfg_refresh, has_levels, header, name_width, natural_cmp, register_item, thresh_item, thresholds_cfg,
    event_type, stat_name, Aliases, Filter, Fit, Trunc,
};
use crate::modules::{Area, Ctx, UiState, World};
use crate::platform::{Battery, BatteryStatus, Platform, Sensor, SensorKind, Sys};
use crate::ui::panel::{Align, Panel, Role};

const MODULE: &str = "sensors";
/// IO-079 compiled-in default for `battery` (applies to 100 - charge).
const BATTERY_DEFAULT: Limits = Limits::new(70.0, 80.0, 90.0, false);
/// IO-023 default refresh.
const DEFAULT_REFRESH: Duration = Duration::from_secs(10);

/// Row kinds, in display group order (IO-072).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Temperature,
    Fan,
    Battery,
}

impl Kind {
    fn field(self) -> &'static str {
        match self {
            Kind::Temperature => "temperature",
            Kind::Fan => "fan",
            Kind::Battery => "battery",
        }
    }
}

struct SItem {
    kind: Kind,
    /// Real label, including the ` 2`, ` 3` duplicate suffix (IO-067).
    label: String,
    /// Length of the label without the duplicate suffix.
    base_len: usize,
    /// 1 for the first sensor with this base label, 2 for the second, ...
    dup: u32,
    alias: Option<String>,
    /// Passed hide/show (computed once, IO-014).
    allowed: bool,
    seen: bool,
    value: f64,
    status: BatteryStatus,
    id: Option<StatId>,
    /// Hardware limits with a critical but no high value: below critical is DEFAULT
    /// (IO-077 step 3).
    ok_is_default: bool,
    role: Role,
}

impl SItem {
    fn base(&self) -> &str {
        &self.label[..self.base_len]
    }
    fn disp(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.label)
    }
}

pub struct Sensors {
    filter: Filter,
    aliases: Aliases,
    tcfg: Config,
    refresh: Duration,
    items: Vec<SItem>,
    sbuf: Vec<Sensor>,
    bbuf: Vec<Battery>,
    scratch: String,
}

/// One reading for `Sensors::see`.
struct Reading {
    kind: Kind,
    value: f64,
    high: Option<f64>,
    critical: Option<f64>,
    status: BatteryStatus,
}

impl Sensors {
    pub fn new(cfg: &Config, _ui: &UiState, _alerts: &mut Alerts) -> Self {
        Sensors {
            filter: Filter::new(cfg, MODULE, &["unknown.*"]), // IO-070
            aliases: Aliases::new(cfg, MODULE),
            tcfg: thresholds_cfg(cfg, MODULE),
            refresh: cfg_refresh(cfg, MODULE).unwrap_or(DEFAULT_REFRESH),
            items: Vec::new(),
            sbuf: Vec::new(),
            bbuf: Vec::new(),
            scratch: String::new(),
        }
    }

    pub fn refresh(&self) -> Option<Duration> {
        Some(self.refresh)
    }

    pub fn update(&mut self, sys: &mut Sys, _ctx: &Ctx, _ui: &UiState, alerts: &mut Alerts) {
        sys.sensors(true, true, &mut self.sbuf);
        sys.batteries(&mut self.bbuf);
        for it in &mut self.items {
            it.seen = false;
        }
        let mut scratch = std::mem::take(&mut self.scratch);
        let mut added = false;
        for i in 0..self.sbuf.len() {
            let s = &self.sbuf[i];
            if !s.value.is_finite() {
                continue; // IO-068
            }
            // IO-067: platform label, else "<chip> <index>".
            scratch.clear();
            if s.label.is_empty() {
                let idx = 1 + self.sbuf[..i]
                    .iter()
                    .filter(|o| o.label.is_empty() && o.kind == s.kind && o.chip == s.chip)
                    .count();
                let _ = write!(scratch, "{} {}", s.chip, idx);
            } else {
                scratch.push_str(&s.label);
            }
            let r = Reading {
                kind: match s.kind {
                    SensorKind::Temperature => Kind::Temperature,
                    SensorKind::Fan => Kind::Fan,
                },
                value: s.value,
                high: s.high,
                critical: s.critical,
                status: BatteryStatus::Unknown,
            };
            added |= self.see(&scratch, r, alerts);
        }
        for i in 0..self.bbuf.len() {
            let b = &self.bbuf[i];
            if !b.percent.is_finite() {
                continue; // IO-068
            }
            scratch.clear();
            scratch.push_str(if b.name.is_empty() { "Battery" } else { b.name.as_str() });
            let r = Reading { kind: Kind::Battery, value: b.percent, high: None, critical: None, status: b.status };
            added |= self.see(&scratch, r, alerts);
        }
        self.scratch = scratch;

        self.items.retain(|it| {
            if !it.seen {
                if let Some(id) = it.id {
                    alerts.unregister(id);
                }
            }
            it.seen
        });
        if added {
            // IO-072: grouped by kind, natural order of the display label.
            self.items.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| natural_cmp(a.disp(), b.disp())));
        }

        // IO-077..IO-081.
        for it in &mut self.items {
            it.role = Role::Default;
            let (Some(id), true) = (it.id, it.allowed) else { continue };
            let role = match it.kind {
                Kind::Temperature => alerts.eval(id, it.value, 100.0),
                Kind::Fan => alerts.eval(id, it.value, 100.0),
                Kind::Battery => alerts.eval(id, 100.0 - it.value, 100.0),
            };
            it.role = match role {
                Role::Ok | Role::OkLog if it.ok_is_default => Role::Default,
                r => r,
            };
        }
    }

    /// Record one reading under base label `base`. Returns true when a new item was added.
    fn see(&mut self, base: &str, r: Reading, alerts: &mut Alerts) -> bool {
        // IO-067 duplicates: ` 2`, ` 3`, ... in discovery order, counted per sensor kind
        // (a fan and a temperature may share a label; they are shown in separate groups).
        let dup = 1 + self.items.iter().filter(|i| i.seen && i.kind == r.kind && i.base() == base).count() as u32;
        if let Some(it) = self.items.iter_mut().find(|i| i.kind == r.kind && i.dup == dup && i.base() == base) {
            it.seen = true;
            it.value = r.value;
            it.status = r.status;
            return false;
        }
        let mut label = base.to_string();
        if dup > 1 {
            let _ = write!(label, " {dup}");
        }
        // IO-071: label, then `<label>_<kind>`.
        let alias = self
            .aliases
            .get(&label)
            .or_else(|| self.aliases.get(&format!("{}_{}", label, r.kind.field())))
            .map(str::to_string);
        let allowed = self.filter.allows(&[&label, alias.as_deref().unwrap_or("")]);
        let (id, ok_is_default) = if allowed {
            self.register(&label, alias.as_deref(), &r, alerts)
        } else {
            (None, false)
        };
        self.items.push(SItem {
            kind: r.kind,
            label,
            base_len: base.len(),
            dup,
            alias,
            allowed,
            seen: true,
            value: r.value,
            status: r.status,
            id,
            ok_is_default,
            role: Role::Default,
        });
        true
    }

    fn register(&self, label: &str, alias: Option<&str>, r: &Reading, alerts: &mut Alerts) -> (Option<StatId>, bool) {
        let field = Some(r.kind.field());
        match r.kind {
            Kind::Fan => (register_item(&self.tcfg, alerts, MODULE, label, alias, field, None), false),
            Kind::Battery => {
                (register_item(&self.tcfg, alerts, MODULE, label, alias, field, Some(BATTERY_DEFAULT)), false)
            }
            Kind::Temperature => {
                // IO-077 step 1: user limits (no compiled-in default).
                let item = thresh_item(&self.tcfg, MODULE, label, alias);
                let user = Alerts::resolve(&self.tcfg, MODULE, Some(item), field, None);
                let limits = match user {
                    Some(l) if has_levels(&l) => l,
                    _ => {
                        // Step 3: hardware limits; no critical means DEFAULT.
                        let Some(crit) = r.critical else { return (None, false) };
                        let base = user.unwrap_or_default();
                        Limits { careful: None, warning: r.high, critical: Some(crit), ..base }
                    }
                };
                let hw_crit_only = !has_levels(&user.unwrap_or_default()) && r.high.is_none();
                let disp = alias.unwrap_or(label);
                let id = alerts.register(&stat_name(MODULE, label, field), &event_type(MODULE, disp, field), limits);
                (Some(id), hw_crit_only)
            }
        }
    }
}

/// IO-075 value text.
fn value_text(it: &SItem, fahrenheit: bool, ascii: bool) -> Small {
    match it.kind {
        Kind::Temperature => {
            if fahrenheit {
                crate::small!("{}F", (it.value * 1.8 + 32.0).round() as i64)
            } else {
                crate::small!("{}C", it.value.round() as i64)
            }
        }
        Kind::Fan => crate::small!("{}RPM", it.value.round() as i64),
        Kind::Battery => {
            let g = match (it.status, ascii) {
                (BatteryStatus::Charging, false) => "↑",
                (BatteryStatus::Charging, true) => "^",
                (BatteryStatus::Discharging, false) => "↓",
                (BatteryStatus::Discharging, true) => "v",
                (BatteryStatus::Full, false) => "✓",
                (BatteryStatus::Full, true) => "=",
                (BatteryStatus::NotCharging | BatteryStatus::Unknown, _) => "",
            };
            crate::small!("{}%{}", it.value.round() as i64, g)
        }
    }
}

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    let s = &w.sensors;
    let Some(n) = name_width(area) else { return Align::Top };
    if !s.items.iter().any(|it| it.allowed) {
        return Align::Top; // IO-005
    }
    header(out, "SENSORS", n, &[], 14); // IO-074: no column headers
    let mut rows = 1u16;
    for it in s.items.iter().filter(|it| it.allowed) {
        if rows >= area.max_height {
            break; // IO-006, IO-073
        }
        rows += 1;
        out.newline();
        out.addf(Role::Default, 0, format_args!("{}", Fit { s: it.disp(), width: n, mode: Trunc::Cut }));
        cell(out, it.role, value_text(it, w.ui.fahrenheit, w.ui.ascii).as_str(), 14);
    }
    Align::Top
}
