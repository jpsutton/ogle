//! Network module (03-io-modules.md section 2), plus the helpers shared by the sidebar
//! I/O modules (diskio, fs, sensors): hide/show filters, aliases, natural sort, name
//! fitting, threshold lookup helpers.

use std::cmp::Ordering;
use std::fmt::{self, Write as _};
use std::time::{Duration, Instant};

use regex_lite::Regex;

use crate::alerts::{Alerts, Limits, StatId};
use crate::config::{Config, Table, Value};
use crate::fmt::{auto_unit, auto_unit_f, MinSym, Small};
use crate::modules::{Area, Ctx, UiState, World};
use crate::platform::{NetIf, Platform, Sys};
use crate::ui::panel::{char_width, str_width, Align, Panel, Role};

// ---------------------------------------------------------------------------------------
// Shared helpers for the sidebar I/O modules.
// ---------------------------------------------------------------------------------------

/// Compiled `hide` / `show` lists (IO-010..IO-013). Patterns are anchored at both ends
/// and case-insensitive. Compiled once at init.
pub(crate) struct Filter {
    hide: Vec<Regex>,
    show: Vec<Regex>,
}

impl Filter {
    pub(crate) fn new(cfg: &Config, module: &str, default_hide: &[&str]) -> Filter {
        let hide = match cfg.str_list(&[module, "hide"]) {
            Some(v) => compile(module, "hide", v.iter().map(|s| s.as_str())),
            None => compile(module, "hide", default_hide.iter().copied()),
        };
        let show = match cfg.str_list(&[module, "show"]) {
            Some(v) => compile(module, "show", v.iter().map(|s| s.as_str())),
            None => Vec::new(),
        };
        Filter { hide, show }
    }

    /// True when an item with these match keys is shown. Empty keys are ignored.
    pub(crate) fn allows(&self, keys: &[&str]) -> bool {
        let hit = |r: &Regex| keys.iter().any(|k| !k.is_empty() && r.is_match(k));
        if !self.show.is_empty() {
            // IO-011: a non-empty show list wins and hide is ignored.
            return self.show.iter().any(hit);
        }
        !self.hide.iter().any(hit)
    }
}

fn compile<'a>(module: &str, key: &str, pats: impl Iterator<Item = &'a str>) -> Vec<Regex> {
    pats.filter_map(|p| match Regex::new(&format!("(?i)^(?:{p})$")) {
        Ok(r) => Some(r),
        Err(e) => {
            // IO-013: one warning line, pattern ignored.
            eprintln!("ogle: [{module}] {key}: invalid pattern {p:?} ignored: {e}");
            None
        }
    })
    .collect()
}

/// `[<module>] alias` table (IO-015). Keys are matched case-insensitively.
pub(crate) struct Aliases(Vec<(String, String)>);

impl Aliases {
    pub(crate) fn new(cfg: &Config, module: &str) -> Aliases {
        let mut v = Vec::new();
        if let Some(t) = cfg.table(&[module, "alias"]) {
            for (k, val) in t {
                if let Value::Str(s) = val {
                    v.push((k.to_lowercase(), s.clone()));
                }
            }
        }
        Aliases(v)
    }

    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k.chars().eq(name.chars().flat_map(char::to_lowercase)))
            .map(|(_, v)| v.as_str())
    }
}

/// Natural order (IO-016): digit runs compare as integers, other runs as
/// case-insensitive text. Ties fall back to plain byte order for determinism.
pub(crate) fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let (x, y) = (a[i], b[j]);
        let (dx, dy) = (x.is_ascii_digit(), y.is_ascii_digit());
        if dx && dy {
            let si = i;
            while i < a.len() && a[i].is_ascii_digit() {
                i += 1;
            }
            let sj = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            let na = trim_zeros(&a[si..i]);
            let nb = trim_zeros(&b[sj..j]);
            let o = na.len().cmp(&nb.len()).then_with(|| na.cmp(nb));
            if o != Ordering::Equal {
                return o;
            }
        } else if dx {
            // The other side's text run continues, ours ended: shorter run first.
            return Ordering::Less;
        } else if dy {
            return Ordering::Greater;
        } else {
            let o = x.to_ascii_lowercase().cmp(&y.to_ascii_lowercase());
            if o != Ordering::Equal {
                return o;
            }
            i += 1;
            j += 1;
        }
    }
    (a.len() - i).cmp(&(b.len() - j)).then_with(|| a.cmp(b))
}

fn trim_zeros(d: &[u8]) -> &[u8] {
    let k = d.iter().position(|&c| c != b'0').unwrap_or(d.len());
    &d[k..]
}

/// Truncation rule for a name column (IO-004).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trunc {
    /// Keep the head, mark with `_` at the end (diskio, fs).
    Head,
    /// Keep the tail, mark with `_` at the start (network).
    Tail,
    /// Keep the head, no marker (sensors).
    Cut,
}

/// A name fitted into a column of `width` cells: at most `width - 1` cells of text,
/// then padded with spaces to exactly `width` cells. Formats without allocation.
pub(crate) struct Fit<'a> {
    pub s: &'a str,
    pub width: usize,
    pub mode: Trunc,
}

impl fmt::Display for Fit<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let max = self.width.saturating_sub(1);
        let w = str_width(self.s);
        let used = if w <= max {
            f.write_str(self.s)?;
            w
        } else {
            match self.mode {
                Trunc::Cut => {
                    let (end, cw) = head_cut(self.s, max);
                    f.write_str(&self.s[..end])?;
                    cw
                }
                Trunc::Head => {
                    let (end, cw) = head_cut(self.s, max.saturating_sub(1));
                    f.write_str(&self.s[..end])?;
                    if max > 0 {
                        f.write_char('_')?;
                        cw + 1
                    } else {
                        cw
                    }
                }
                Trunc::Tail => {
                    let (start, cw) = tail_cut(self.s, max.saturating_sub(1));
                    if max > 0 {
                        f.write_char('_')?;
                    }
                    f.write_str(&self.s[start..])?;
                    cw + (max > 0) as usize
                }
            }
        };
        for _ in used..self.width {
            f.write_char(' ')?;
        }
        Ok(())
    }
}

/// Byte end and cell width of the longest prefix fitting in `cells`.
fn head_cut(s: &str, cells: usize) -> (usize, usize) {
    let mut acc = 0;
    for (i, c) in s.char_indices() {
        let cw = char_width(c);
        if acc + cw > cells {
            return (i, acc);
        }
        acc += cw;
    }
    (s.len(), acc)
}

/// Byte start and cell width of the longest suffix fitting in `cells`.
fn tail_cut(s: &str, cells: usize) -> (usize, usize) {
    let mut acc = 0;
    let mut start = s.len();
    for (i, c) in s.char_indices().rev() {
        let cw = char_width(c);
        if acc + cw > cells {
            break;
        }
        acc += cw;
        start = i;
    }
    (start, acc)
}

/// Header row (IO-003): title padded to `n`, then column headers right-aligned in
/// `colw`-cell columns.
pub(crate) fn header(out: &mut Panel, title: &str, n: usize, cols: &[&str], colw: usize) {
    out.addf(Role::Title, 0, format_args!("{}", Fit { s: title, width: n, mode: Trunc::Cut }));
    for c in cols {
        out.addf(Role::Default, 0, format_args!("{:>w$}", c, w = colw));
    }
}

/// Right-aligned value cell.
pub(crate) fn cell(out: &mut Panel, role: Role, s: &str, w: usize) {
    out.addf(role, 0, format_args!("{:>w$}", s, w = w));
}

/// Panel geometry: returns the name column width `N = P - 14` (IO-002), or None when the
/// area cannot hold a header plus one row.
pub(crate) fn name_width(area: Area) -> Option<usize> {
    let p = area.width as usize;
    if p < 16 || area.max_height < 2 {
        return None;
    }
    Some(p - 14)
}

/// Module refresh from `[<module>] refresh` (IO-023).
pub(crate) fn cfg_refresh(cfg: &Config, module: &str) -> Option<Duration> {
    cfg.f64(&[module, "refresh"]).filter(|s| *s > 0.0).map(Duration::from_secs_f64)
}

/// Seconds since the previous sample when rates may be computed. Returns None on the
/// first sample or after a gap much longer than the refresh interval, which means the
/// module was toggled off and on again (IO-024: old counters are thrown away).
pub(crate) fn rate_dt(last: Option<Instant>, now: Instant, refresh: Option<Duration>, global: f64) -> Option<f64> {
    let dt = now.checked_duration_since(last?)?.as_secs_f64();
    let interval = refresh.map(|d| d.as_secs_f64()).unwrap_or(global).max(0.1);
    (dt > 0.0 && dt <= interval * 3.0 + 1.0).then_some(dt)
}

/// Copy of the parts of the config needed to resolve limits for items that appear later
/// (`update` receives no `Config`): `[<module>.thresholds]` and `[alert]`.
pub(crate) fn thresholds_cfg(cfg: &Config, module: &str) -> Config {
    let mut root = Table::new();
    if let Some(t) = cfg.table(&[module, "thresholds"]) {
        let mut m = Table::new();
        m.insert("thresholds".to_string(), Value::Table(t.clone()));
        root.insert(module.to_string(), Value::Table(m));
    }
    if let Some(a) = cfg.get(&["alert"]) {
        root.insert("alert".to_string(), a.clone());
    }
    Config { root, source: None }
}

/// ALR-013 / IO-015: thresholds use the alias when it has an item entry, else the real
/// name.
pub(crate) fn thresh_item<'a>(tcfg: &Config, module: &str, real: &'a str, alias: Option<&'a str>) -> &'a str {
    match alias {
        Some(a) if tcfg.get(&[module, "thresholds", "items", a]).is_some() => a,
        _ => real,
    }
}

/// Stat name `module[item].field` (05 section 3).
pub(crate) fn stat_name(module: &str, item: &str, field: Option<&str>) -> String {
    let mut s = String::with_capacity(module.len() + item.len() + 12);
    s.push_str(module);
    s.push('[');
    s.push_str(item);
    s.push(']');
    if let Some(f) = field {
        s.push('.');
        s.push_str(f);
    }
    s
}

/// Event type `MODULE_ITEM_FIELD` (ALR-008). Non-ASCII characters are kept unchanged.
pub(crate) fn event_type(module: &str, item: &str, field: Option<&str>) -> String {
    let mut s = String::with_capacity(module.len() + item.len() + 12);
    s.push_str(module);
    s.push('_');
    s.push_str(item);
    if let Some(f) = field {
        s.push('_');
        s.push_str(f);
    }
    s.make_ascii_uppercase();
    s
}

pub(crate) fn has_levels(l: &Limits) -> bool {
    l.careful.is_some() || l.warning.is_some() || l.critical.is_some()
}

/// Resolve and register a per-item stat; None when no level is defined.
pub(crate) fn register_item(
    tcfg: &Config,
    alerts: &mut Alerts,
    module: &str,
    real: &str,
    alias: Option<&str>,
    field: Option<&str>,
    default: Option<Limits>,
) -> Option<StatId> {
    let item = thresh_item(tcfg, module, real, alias);
    let l = Alerts::resolve(tcfg, module, Some(item), field, default)?;
    if !has_levels(&l) {
        return None;
    }
    let disp = alias.unwrap_or(real);
    Some(alerts.register(&stat_name(module, real, field), &event_type(module, disp, field), l))
}

// ---------------------------------------------------------------------------------------
// Network
// ---------------------------------------------------------------------------------------

const MODULE: &str = "network";
/// IO-038 group 2 compiled-in default (percent of link speed).
const NET_DEFAULT: Limits = Limits::new(70.0, 80.0, 90.0, false);
/// IO-026: address/state refresh period.
const ADDR_PERIOD: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
struct NetStat {
    id: StatId,
    /// Group 1 (absolute bits/s, maximum 100) or group 2 (percent of link speed).
    absolute: bool,
}

struct NetItem {
    name: String,
    alias: Option<String>,
    /// Passed hide/show (computed once, IO-014).
    allowed: bool,
    seen: bool,
    /// Counters of the previous sample are valid.
    primed: bool,
    rx: u64,
    tx: u64,
    rx_rate: Option<f64>,
    tx_rate: Option<f64>,
    up: bool,
    has_ip: bool,
    speed: Option<u64>,
    /// hide_zero latches (IO-017).
    rx_vis: bool,
    tx_vis: bool,
    rx_stat: Option<NetStat>,
    tx_stat: Option<NetStat>,
    rx_role: Role,
    tx_role: Role,
}

impl NetItem {
    fn disp(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

pub struct Network {
    filter: Filter,
    aliases: Aliases,
    tcfg: Config,
    hide_no_up: bool,
    hide_no_ip: bool,
    hide_zero: bool,
    zero_thr: f64,
    refresh: Option<Duration>,
    items: Vec<NetItem>,
    buf: Vec<NetIf>,
    last: Option<Instant>,
    last_addrs: Option<Instant>,
}

impl Network {
    pub fn new(cfg: &Config, _ui: &UiState, _alerts: &mut Alerts) -> Self {
        Network {
            filter: Filter::new(cfg, MODULE, &["lo", "docker.*", "veth.*"]), // IO-027
            aliases: Aliases::new(cfg, MODULE),
            tcfg: thresholds_cfg(cfg, MODULE),
            hide_no_up: cfg.bool(&[MODULE, "hide_no_up"]).unwrap_or(true), // IO-028
            hide_no_ip: cfg.bool(&[MODULE, "hide_no_ip"]).unwrap_or(true), // IO-029
            hide_zero: cfg.bool(&[MODULE, "hide_zero"]).unwrap_or(false),
            zero_thr: cfg.f64(&[MODULE, "hide_threshold_bytes"]).unwrap_or(0.0),
            refresh: cfg_refresh(cfg, MODULE),
            items: Vec::new(),
            buf: Vec::new(),
            last: None,
            last_addrs: None,
        }
    }

    pub fn refresh(&self) -> Option<Duration> {
        self.refresh
    }

    fn shown(&self, it: &NetItem) -> bool {
        it.allowed
            && (!self.hide_no_up || it.up)
            && (!self.hide_no_ip || it.has_ip)
            && (!self.hide_zero || it.rx_vis || it.tx_vis)
    }

    fn set_changed(&self) -> bool {
        self.buf.len() != self.items.len()
            || self.buf.iter().any(|b| !self.items.iter().any(|i| i.name == b.name))
    }

    pub fn update(&mut self, sys: &mut Sys, ctx: &Ctx, ui: &UiState, alerts: &mut Alerts) {
        let dt = rate_dt(self.last, ctx.now, self.refresh, ui.refresh);
        // IO-026: addresses at most every 10 s, and at once when the set changes.
        let mut addrs = self.hide_no_ip
            && self.last_addrs.map_or(true, |t| ctx.now.saturating_duration_since(t) >= ADDR_PERIOD);
        sys.net_interfaces(addrs, &mut self.buf);
        if self.hide_no_ip && !addrs && self.set_changed() {
            sys.net_interfaces(true, &mut self.buf);
            addrs = true;
        }
        if addrs {
            self.last_addrs = Some(ctx.now);
        }

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
            if !it.allowed {
                continue; // IO-014
            }
            // IO-008: unknown on the first sample or when a counter goes backwards.
            match dt {
                Some(dt) if it.primed => {
                    it.rx_rate = (b.rx_bytes >= it.rx).then(|| (b.rx_bytes - it.rx) as f64 / dt);
                    it.tx_rate = (b.tx_bytes >= it.tx).then(|| (b.tx_bytes - it.tx) as f64 / dt);
                }
                _ => {
                    it.rx_rate = None;
                    it.tx_rate = None;
                }
            }
            it.rx = b.rx_bytes;
            it.tx = b.tx_bytes;
            it.primed = true;
            it.up = b.is_up;
            it.speed = b.speed_bps.filter(|&s| s > 0);
            if addrs {
                it.has_ip = b.has_ip;
            }
            // IO-017 latches.
            if it.rx_rate.is_some_and(|r| r > self.zero_thr) {
                it.rx_vis = true;
            }
            if it.tx_rate.is_some_and(|r| r > self.zero_thr) {
                it.tx_vis = true;
            }
        }
        self.items.retain(|it| {
            if !it.seen {
                for s in [it.rx_stat, it.tx_stat].into_iter().flatten() {
                    alerts.unregister(s.id);
                }
            }
            it.seen
        });
        if added {
            self.items.sort_by(|a, b| natural_cmp(a.disp(), b.disp())); // IO-031
        }

        // IO-037: thresholds only in plain rate mode, on displayed rows.
        let rate_mode = !ui.net_sum && !ui.net_cumulative;
        for i in 0..self.items.len() {
            let show = rate_mode && self.shown(&self.items[i]);
            let it = &mut self.items[i];
            if show {
                it.rx_role = eval_net(alerts, it.rx_stat, it.rx_rate, it.speed);
                it.tx_role = eval_net(alerts, it.tx_stat, it.tx_rate, it.speed);
            } else {
                it.rx_role = Role::Default;
                it.tx_role = Role::Default;
            }
        }
        self.last = Some(ctx.now);
    }
}

fn new_item(b: &NetIf, filter: &Filter, aliases: &Aliases, tcfg: &Config, alerts: &mut Alerts) -> NetItem {
    let alias = aliases.get(&b.name).map(str::to_string);
    let allowed = filter.allows(&[&b.name, alias.as_deref().unwrap_or("")]);
    let (rx_stat, tx_stat) = if allowed {
        (
            net_stat(tcfg, alerts, &b.name, alias.as_deref(), "rx"),
            net_stat(tcfg, alerts, &b.name, alias.as_deref(), "tx"),
        )
    } else {
        (None, None)
    };
    NetItem {
        name: b.name.clone(),
        alias,
        allowed,
        seen: true,
        primed: false,
        rx: 0,
        tx: 0,
        rx_rate: None,
        tx_rate: None,
        up: b.is_up,
        has_ip: b.has_ip,
        speed: None,
        rx_vis: false,
        tx_vis: false,
        rx_stat,
        tx_stat,
        rx_role: Role::Default,
        tx_role: Role::Default,
    }
}

/// Level value in a limit set: Some(Some(v)) number, Some(None) disabled (`false`),
/// None absent.
fn level_in(t: Option<&Table>, key: &str) -> Option<Option<f64>> {
    match t?.get(key)? {
        Value::Int(i) => Some(Some(*i as f64)),
        Value::Float(f) => Some(Some(*f)),
        Value::Bool(false) => Some(None),
        _ => None,
    }
}

/// IO-038: group 1 (item + field, item: absolute bits/s) wins when it defines any level;
/// otherwise group 2 (field, module-wide, default: percent of link speed).
fn net_stat(tcfg: &Config, alerts: &mut Alerts, name: &str, alias: Option<&str>, field: &str) -> Option<NetStat> {
    let item = thresh_item(tcfg, MODULE, name, alias);
    let full = Alerts::resolve(tcfg, MODULE, Some(item), Some(field), Some(NET_DEFAULT));
    let t_if = tcfg.table(&[MODULE, "thresholds", "items", item, field]);
    let t_i = tcfg.table(&[MODULE, "thresholds", "items", item]);
    let g1 = ["careful", "warning", "critical"].map(|k| level_in(t_if, k).or_else(|| level_in(t_i, k)));
    let (limits, absolute) = if g1.iter().any(Option::is_some) {
        let base = full.unwrap_or_default();
        let l = Limits { careful: g1[0].flatten(), warning: g1[1].flatten(), critical: g1[2].flatten(), ..base };
        (l, true)
    } else {
        let g2 = Alerts::resolve(tcfg, MODULE, None, Some(field), Some(NET_DEFAULT))?;
        let base = full.unwrap_or(g2);
        let l = Limits { careful: g2.careful, warning: g2.warning, critical: g2.critical, ..base };
        (l, false)
    };
    if !has_levels(&limits) {
        return None;
    }
    let disp = alias.unwrap_or(name);
    let id = alerts.register(&stat_name(MODULE, name, Some(field)), &event_type(MODULE, disp, Some(field)), limits);
    Some(NetStat { id, absolute })
}

/// IO-037..IO-039, IO-022.
fn eval_net(alerts: &mut Alerts, st: Option<NetStat>, rate: Option<f64>, speed: Option<u64>) -> Role {
    let (Some(st), Some(rate)) = (st, rate) else { return Role::Default };
    let bits = rate * 8.0;
    if st.absolute {
        alerts.eval(st.id, bits, 100.0)
    } else if let Some(sp) = speed {
        alerts.eval(st.id, bits, sp as f64)
    } else {
        Role::Default
    }
}

/// IO-034: bits (x8, suffix `b`) or bytes.
fn rate_text(v: f64, bytes: bool) -> Small {
    if bytes {
        auto_unit_f(v, false, MinSym::None)
    } else {
        let s = auto_unit_f(v * 8.0, false, MinSym::None);
        crate::small!("{}b", s)
    }
}

fn total_text(v: u64, bytes: bool) -> Small {
    if bytes {
        auto_unit(v, false, MinSym::None)
    } else {
        let s = auto_unit(v.saturating_mul(8), false, MinSym::None);
        crate::small!("{}b", s)
    }
}

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    let net = &w.network;
    let ui = &w.ui;
    let Some(n) = name_width(area) else { return Align::Top };
    if !net.items.iter().any(|it| net.shown(it)) {
        return Align::Top; // IO-005
    }
    let (sum, cum, bytes) = (ui.net_sum, ui.net_cumulative, ui.net_bytes);
    match (sum, cum) {
        (true, false) => header(out, "NETWORK", n, &["Rx+Tx/s"], 14),
        (true, true) => header(out, "NETWORK", n, &["Rx+Tx"], 14),
        (false, true) => header(out, "NETWORK", n, &["Rx", "Tx"], 7),
        (false, false) => header(out, "NETWORK", n, &["Rx/s", "Tx/s"], 7),
    }
    let mut rows = 1u16;
    for it in net.items.iter().filter(|it| net.shown(it)) {
        if rows >= area.max_height {
            break; // IO-006
        }
        rows += 1;
        out.newline();
        out.addf(Role::Default, 0, format_args!("{}", Fit { s: it.disp(), width: n, mode: Trunc::Tail }));
        match (sum, cum) {
            (true, false) => match (it.rx_rate, it.tx_rate) {
                (Some(r), Some(t)) => cell(out, Role::Default, rate_text(r + t, bytes).as_str(), 14),
                _ => cell(out, Role::Default, "-", 14),
            },
            (true, true) => {
                cell(out, Role::Default, total_text(it.rx.saturating_add(it.tx), bytes).as_str(), 14)
            }
            (false, true) => {
                cell(out, Role::Default, total_text(it.rx, bytes).as_str(), 7);
                cell(out, Role::Default, total_text(it.tx, bytes).as_str(), 7);
            }
            (false, false) => {
                for (r, role) in [(it.rx_rate, it.rx_role), (it.tx_rate, it.tx_role)] {
                    match r {
                        Some(v) => cell(out, role, rate_text(v, bytes).as_str(), 7),
                        None => cell(out, Role::Default, "-", 7),
                    }
                }
            }
        }
    }
    Align::Top
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order() {
        assert_eq!(natural_cmp("eth2", "eth10"), Ordering::Less);
        assert_eq!(natural_cmp("sda", "sdb"), Ordering::Less);
        assert_eq!(natural_cmp("SDA", "sdb"), Ordering::Less);
        assert_eq!(natural_cmp("nvme0n1", "nvme0n1p2"), Ordering::Less);
        assert_eq!(natural_cmp("eth10", "eth9"), Ordering::Greater);
    }

    #[test]
    fn fit_rules() {
        let f = |s: &str, w: usize, m: Trunc| Fit { s, width: w, mode: m }.to_string();
        assert_eq!(f("eth0", 6, Trunc::Tail), "eth0  ");
        assert_eq!(f("enp0s31f6", 6, Trunc::Tail), "_31f6 ");
        assert_eq!(f("nvme0n1p2", 6, Trunc::Head), "nvme_ ");
        assert_eq!(f("Package id 0", 6, Trunc::Cut), "Packa ");
    }
}
