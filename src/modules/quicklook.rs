//! Module quicklook: CPU name/frequency line and gauge rows (02-system-modules.md
//! section 12, SYS-091..SYS-100). Reuses the cpu, mem, memswap and load samples.

use std::cell::Cell;
use std::fmt;
use std::time::Duration;

use crate::alerts::{Alerts, Limits};
use crate::config::Config;
use crate::fmt::{Ring, Small};
use crate::modules::cpu::{self, ql_visible, TOTAL};
use crate::modules::load::Load;
use crate::modules::mem::Mem;
use crate::modules::memswap::MemSwap;
use crate::modules::{Area, Ctx, UiState, World};
use crate::platform::{Platform, Sys};
use crate::ui::panel::{char_width, str_width, Align, Panel, Role};

const CPU: usize = 0;
const NE: usize = 4;
const ENTRY_NAMES: [&str; NE] = ["cpu", "mem", "swap", "load"];
const LABELS: [&str; NE] = ["CPU  ", "MEM  ", "SWAP ", "LOAD "];
/// SYS-096 sparkline glyphs, index = round(v / 100 * 8).
const SPARK: [char; 9] = [' ', '\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}'];

pub struct QuickLook {
    /// SYS-092: entry indices in display order.
    list: Vec<usize>,
    bar: String,
    /// SYS-097: decoration-only limits per entry.
    limits: [Option<Limits>; NE],
    name: String,
    freq_cur: Option<u64>,
    freq_max: Option<u64>,
    /// `name` holds the model reported by the platform.
    have_model: bool,
    values: [Option<f64>; NE],
    /// Source exists on this platform (SYS-092: absent entries are skipped).
    avail: [bool; NE],
    /// Sparkline histories (SYS-096), allocated only while sparklines are on.
    spark: [Option<Ring>; NE],
    /// Per-core and `CPU*` histories (SYS-098), only in percpu + sparkline mode.
    core_spark: Vec<Ring>,
    sum_spark: Option<Ring>,
    /// Last width given by the layout; the next update sizes the rings to `W - 13`.
    width: Cell<u16>,
}

impl QuickLook {
    pub fn new(cfg: &Config, ui: &UiState, alerts: &mut Alerts) -> Self {
        let _ = (ui, alerts);
        let mut list = Vec::new();
        match cfg.str_list(&["quicklook", "list"]) {
            Some(names) => {
                for n in &names {
                    match ENTRY_NAMES.iter().position(|e| *e == n.as_str()) {
                        Some(i) => list.push(i),
                        // SYS-092: drop only the unknown entry.
                        None => eprintln!("ogle: [quicklook] list: unknown entry \"{n}\" ignored"),
                    }
                }
            }
            None => list.extend([0, 1, 3]),
        }
        let bar = cfg
            .str(&["quicklook", "bar_char"])
            .filter(|s| s.chars().count() == 1 && str_width(s) == 1)
            .unwrap_or("|")
            .to_string();
        let mut limits = [None; NE];
        for (i, l) in limits.iter_mut().enumerate() {
            let def = if i == 3 { Limits::new(70.0, 100.0, 500.0, false) } else { Limits::new(50.0, 70.0, 90.0, false) };
            *l = Alerts::resolve(cfg, "quicklook", None, Some(ENTRY_NAMES[i]), Some(def));
        }
        QuickLook {
            list,
            bar,
            limits,
            name: String::new(),
            freq_cur: None,
            freq_max: None,
            have_model: false,
            values: [None; NE],
            avail: [true; NE],
            spark: [None, None, None, None],
            core_spark: Vec::new(),
            sum_spark: None,
            width: Cell::new(0),
        }
    }

    /// SYS-100: every cycle.
    pub fn refresh(&self) -> Option<Duration> {
        None
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update(
        &mut self,
        sys: &mut Sys,
        ctx: &Ctx,
        ui: &UiState,
        alerts: &mut Alerts,
        cpu: &cpu::Cpu,
        mem: &Mem,
        swap: &MemSwap,
        load: &Load,
    ) {
        let _ = (ctx, alerts);
        // SYS-091: frequency only while visible. Model and max are static: keep the first
        // value reported and never touch the model String again.
        if ql_visible(ui) {
            let f = sys.cpu_freq();
            if !self.have_model {
                if let Some(m) = f.model {
                    self.name = m;
                    self.have_model = true;
                }
            }
            if self.freq_max.is_none() {
                self.freq_max = f.max_hz;
            }
            self.freq_cur = f.current_hz;
        }
        // SYS-012: reuse the other modules' samples.
        let cores = cpu.logical_cores();
        self.values = [
            cpu.total(),
            mem.percent(),
            swap.percent(),
            load.min15().filter(|_| cores > 0).map(|l| l / cores as f64 * 100.0),
        ];
        self.avail = [true, mem.total().is_some(), swap.present(), load.min15().is_some()];

        // SYS-096: sparkline rings exist only while sparklines are on.
        if !(ui.sparkline && ui.history && !ui.ascii) {
            self.spark = [None, None, None, None];
            self.core_spark = Vec::new();
            self.sum_spark = None;
            return;
        }
        let w = self.width.get() as usize;
        let g = if w == 0 { 30 } else { w.saturating_sub(13).max(1) };
        for &e in &self.list {
            let r = fit(&mut self.spark[e], g);
            if let Some(v) = self.values[e] {
                r.push(v as f32);
            }
        }
        // SYS-098: per-core histories only in percpu + sparkline mode.
        if ui.percpu && !cpu.per_pct.is_empty() {
            let n = cpu.per_pct.len();
            if self.core_spark.len() != n {
                self.core_spark.clear();
                self.core_spark.resize_with(n, || Ring::new(g));
            }
            for (r, p) in self.core_spark.iter_mut().zip(&cpu.per_pct) {
                if r.capacity() != g {
                    *r = resized(r, g);
                }
                if let Some(v) = p[TOTAL] {
                    r.push(v as f32);
                }
            }
            // The `CPU*` series records the mean of the cores not shown in this sample.
            let r = fit(&mut self.sum_spark, g);
            if let Some(v) = cpu.summary.and_then(|s| s[TOTAL]) {
                r.push(v as f32);
            }
        } else {
            self.core_spark = Vec::new();
            self.sum_spark = None;
        }
    }
}

/// Ensure `slot` holds a ring of capacity `g`, keeping the newest samples.
fn fit(slot: &mut Option<Ring>, g: usize) -> &mut Ring {
    if slot.as_ref().map_or(true, |r| r.capacity() != g) {
        *slot = Some(match slot.as_ref() {
            Some(r) => resized(r, g),
            None => Ring::new(g),
        });
    }
    slot.as_mut().unwrap()
}

fn resized(r: &Ring, g: usize) -> Ring {
    let mut n = Ring::new(g);
    let keep = r.len().min(g);
    for i in r.len() - keep..r.len() {
        n.push(r.get(i));
    }
    n
}

/// `n` copies of `s`.
struct Rep<'a>(&'a str, usize);

impl fmt::Display for Rep<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for _ in 0..self.1 {
            f.write_str(self.0)?;
        }
        Ok(())
    }
}

/// Newest `g` samples, oldest on the left, right-aligned (SYS-096).
struct Spark<'a>(Option<&'a Ring>, usize);

impl fmt::Display for Spark<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let len = self.0.map_or(0, |r| r.len());
        let n = len.min(self.1);
        write!(f, "{:w$}", "", w = self.1 - n)?;
        if let Some(r) = self.0 {
            for i in len - n..len {
                let v = (r.get(i) as f64).clamp(0.0, 100.0);
                fmt::Write::write_char(f, SPARK[(v / 100.0 * 8.0).round() as usize])?;
            }
        }
        Ok(())
    }
}

/// One gauge row after its label (SYS-094..SYS-096).
fn gauge(out: &mut Panel, q: &QuickLook, w: &World, g: isize, v: Option<f64>, ring: Option<&Ring>, lim: &Option<Limits>) {
    let spark = w.ui.sparkline && w.ui.history && !w.ui.ascii;
    let v = if spark { ring.and_then(|r| r.last()).map(|x| x as f64).or(v) } else { v };
    let role = match (lim, v) {
        (Some(l), Some(v)) => Alerts::decorate(l, v, 100.0),
        _ => Role::Default,
    };
    if g >= 1 {
        let g = g as usize;
        out.add(Role::Bold, 0, "[");
        if spark {
            out.addf(role, 0, format_args!("{}", Spark(ring, g)));
        } else {
            // SYS-095: round the fractional cell at 0.5.
            let fill = g as f64 * v.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0;
            let mut n = fill.floor() as usize;
            if fill - fill.floor() >= 0.5 {
                n += 1;
            }
            let n = n.min(g);
            let bar = if w.ui.ascii && !q.bar.is_ascii() { "|" } else { q.bar.as_str() };
            out.addf(role, 0, format_args!("{}{:w$}", Rep(bar, n), "", w = g - n));
        }
    }
    match v {
        None => out.addf(Role::Default, 0, format_args!("{:>6}", "-")),
        Some(x) if x > 100.0 => out.add(role, 0, " >100%"),
        Some(x) if x == 100.0 => out.add(role, 0, "  100%"),
        Some(x) => out.addf(role, 0, format_args!("{:5.1}%", x.max(0.0))),
    }
    if g >= 1 {
        out.add(Role::Bold, 0, "]");
    }
}

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    let q = &w.quicklook;
    let c = &w.cpu;
    q.width.set(area.width);
    let width = area.width as usize;
    let max_lines = area.max_height.max(1) as usize;

    // SYS-093 line 1: name on the left, frequency text right-aligned.
    let mut freq = match (q.freq_cur, q.freq_max) {
        (Some(cur), Some(max)) => crate::small!(" {:.2}/{:.2}GHz", cur as f64 / 1e9, max as f64 / 1e9),
        (Some(cur), None) => crate::small!(" {:.2}GHz", cur as f64 / 1e9),
        _ => Small::default(),
    };
    if freq.as_str().len() > width {
        freq = Small::default();
    }
    let room = width - freq.as_str().len();
    let nw = str_width(&q.name);
    if nw <= room {
        out.add(Role::Default, 0, &q.name);
        cpu::pad(out, 0, room - nw);
    } else if room > 0 {
        let (mut used, mut end) = (0, 0);
        for (i, ch) in q.name.char_indices() {
            let cw = char_width(ch);
            if used + cw > room - 1 {
                break;
            }
            used += cw;
            end = i + ch.len_utf8();
        }
        out.add(Role::Default, 0, &q.name[..end]);
        out.add(Role::Default, 0, if w.ui.ascii { "~" } else { "\u{2026}" });
        cpu::pad(out, 0, room - 1 - used);
    }
    if !freq.as_str().is_empty() {
        out.add(Role::Default, 0, freq.as_str());
    }

    let g = width as isize - 13;
    let mut lines = 1;
    for &e in &q.list {
        if !q.avail[e] {
            continue;
        }
        if e == CPU && w.ui.percpu && !c.per_pct.is_empty() {
            // SYS-098: one row per shown core (SYS-063 order), then `CPU*`.
            for &core in c.order.iter().take(c.shown) {
                if lines >= max_lines {
                    return Align::Top;
                }
                out.newline();
                lines += 1;
                if core < 10 {
                    out.addf(Role::Default, 0, format_args!("CPU{core} "));
                } else {
                    out.addf(Role::Default, 0, format_args!("{core:>4} "));
                }
                let v = c.per_pct.get(core as usize).and_then(|p| p[TOTAL]);
                gauge(out, q, w, g, v, q.core_spark.get(core as usize), &q.limits[CPU]);
            }
            if let Some(s) = &c.summary {
                if lines >= max_lines {
                    return Align::Top;
                }
                out.newline();
                lines += 1;
                out.add(Role::Default, 0, "CPU* ");
                gauge(out, q, w, g, s[TOTAL], q.sum_spark.as_ref(), &q.limits[CPU]);
            }
            continue;
        }
        if lines >= max_lines {
            return Align::Top;
        }
        out.newline();
        lines += 1;
        out.add(Role::Default, 0, LABELS[e]);
        gauge(out, q, w, g, q.values[e], q.spark[e].as_ref(), &q.limits[e]);
    }
    Align::Top
}
