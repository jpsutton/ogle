//! processlist panel (PROC-040..053, 080..082, 120..124). Also renders the program list
//! (`render_list(.., programs = true)`, PROC-132) for `programlist.rs`.
//!
//! Only visible rows are formatted, straight into the panel arena (no allocation).

use std::fmt;

use crate::alerts::{Alerts, Limits};
use crate::fmt::{auto_unit, auto_unit_f, MinSym, Small};
use crate::modules::processes::{col, disp_name, time_plus, Agree, Processes, SortKey, HAS_IO};
use crate::modules::{Area, World};
use crate::ui::panel::{char_width, flags, str_width, Align, Panel, Role};

/// Lines of the pinned block: title, 3 detail lines, blank (PROC-122); 2 when exited.
fn ext_lines(p: &Processes) -> u16 {
    match &p.pin {
        Some(pin) if pin.exited => 2,
        Some(_) => 5,
        None => 0,
    }
}

fn summary_active(p: &Processes) -> bool {
    p.filter.is_some() || !p.focus.is_empty()
}

/// PROC-053 / PROC-082: process rows that fit in `max_height` (header, pinned block and
/// filter summary come first). The app passes this to `Processes::set_page_rows`.
pub fn rows_for_height(w: &World, max_height: u16) -> u16 {
    if !w.ui.processes {
        return 0;
    }
    let p = &w.processes;
    let mut used = 1 + ext_lines(p);
    if summary_active(p) {
        used += 4; // separator + current/min/max
    }
    max_height.saturating_sub(used)
}

pub fn render(w: &World, area: Area, out: &mut Panel) -> Align {
    render_list(w, area, out, false)
}

/// Visible columns (PROC-051, 052, 054, 132).
struct Cols {
    virt: bool,
    res: bool,
    time: bool,
    thr: bool,
    ni: bool,
    s: bool,
    ior: bool,
    iow: bool,
    core: bool,
    pidw: usize,
}

impl Cols {
    fn new(p: &Processes, area: Area, programs: bool) -> Cols {
        let opt = !area.narrow;
        let on = |c: u16| p.disabled & c == 0;
        Cols {
            virt: opt && on(col::VIRT),
            res: opt && on(col::RES),
            time: opt && on(col::TIME),
            thr: on(col::THR),
            ni: on(col::NI),
            s: on(col::S),
            ior: opt && HAS_IO && on(col::IOR),
            iow: opt && HAS_IO && on(col::IOW),
            core: opt && !programs && on(col::CORE),
            pidw: if programs { 7 } else { p.pid_width },
        }
    }
    /// Width of everything before the Command column.
    fn fixed(&self) -> usize {
        let b = |on: bool, w: usize| if on { w } else { 0 };
        1 + 6 + 6 + b(self.virt, 6) + b(self.res, 6) + self.pidw + 1 + 10 + b(self.time, 9) + b(self.thr, 4)
            + b(self.ni, 4) + b(self.s, 2) + b(self.ior, 5) + b(self.iow, 5) + b(self.core, 4)
    }
}

/// Start a new line unless the panel is empty.
fn line(out: &mut Panel) {
    if !out.is_empty() {
        out.newline();
    }
}

/// Append `s`, cut to `rem` terminal cells (PROC-046 step 7).
fn put_cut(out: &mut Panel, role: Role, s: &str, rem: &mut usize) {
    if *rem == 0 || s.is_empty() {
        return;
    }
    let w = str_width(s);
    if w <= *rem {
        out.add(role, flags::SPLITTABLE, s);
        *rem -= w;
        return;
    }
    let mut used = 0;
    let mut end = 0;
    for (i, c) in s.char_indices() {
        let cw = char_width(c);
        if used + cw > *rem {
            break;
        }
        used += cw;
        end = i + c.len_utf8();
    }
    out.add(role, flags::SPLITTABLE, &s[..end]);
    *rem = 0;
}

/// `n` copies of a glyph, written without a temporary String.
struct Rep(&'static str, usize);

impl fmt::Display for Rep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for _ in 0..self.1 {
            f.write_str(self.0)?;
        }
        Ok(())
    }
}

fn blank(out: &mut Panel, n: usize) {
    crate::addf!(out, Role::Default, 0, "{:n$}", "", n = n);
}

/// First 9 characters of a user name.
fn cut9(s: &str) -> &str {
    s.char_indices().nth(9).map_or(s, |(i, _)| &s[..i])
}

fn level_role(lim: &Option<Limits>, v: f64, is_max: bool) -> Role {
    let r = match lim {
        Some(l) => Alerts::decorate(l, v, 100.0),
        None => Role::Default,
    };
    // ALR-040: MAX only when the cell would otherwise be OK and the value is > 0.
    if is_max && r == Role::Ok && v > 0.0 {
        Role::Max
    } else {
        r
    }
}

fn cpu_disp(w: &World, raw: f64) -> f64 {
    if w.ui.normalize_cpu {
        raw / w.processes.ncpu.max(1) as f64 // PROC-044
    } else {
        raw
    }
}

/// CPU% cell: `{:<6.1}`, `{:<6.0}` from 100, `-` unknown.
fn cpu_cell(out: &mut Panel, w: &World, raw: f64, is_max: bool) {
    if raw.is_nan() || raw < 0.0 {
        crate::addf!(out, Role::Default, 0, "{:<6}", "-");
        return;
    }
    let v = cpu_disp(w, raw);
    let role = level_role(&w.processes.cpu_lim, v, is_max);
    if v >= 100.0 {
        crate::addf!(out, role, 0, "{:<6.0}", v);
    } else {
        crate::addf!(out, role, 0, "{:<6.1}", v);
    }
}

fn mem_cell(out: &mut Panel, p: &Processes, v: f64, is_max: bool) {
    if v.is_nan() || v < 0.0 {
        crate::addf!(out, Role::Default, 0, "{:<5} ", "-");
    } else {
        crate::addf!(out, level_role(&p.mem_lim, v, is_max), 0, "{:<5.1} ", v);
    }
}

fn bytes_cell(out: &mut Panel, v: Option<u64>) {
    match v {
        Some(b) => crate::addf!(out, Role::Default, 0, "{:<5} ", auto_unit(b, false, MinSym::None)),
        None => crate::addf!(out, Role::Default, 0, "{:<5} ", "?"),
    }
}

fn rate_str(v: f64) -> Small {
    if v.is_nan() {
        crate::small!("-")
    } else if v < 0.0 {
        crate::small!("?")
    } else if v == 0.0 {
        crate::small!("0") // PROC-048
    } else {
        auto_unit_f(v.round(), true, MinSym::None)
    }
}

fn time_cell(out: &mut Panel, t: f64) {
    let role = if t >= 3600.0 { Role::CpuTime } else { Role::Default };
    crate::addf!(out, role, 0, "{:>8} ", time_plus(t));
}

/// NI decoration from the `nice_*` lists, checked critical first (PROC-041).
fn nice_role(p: &Processes, nice: Option<i32>, class: Option<&str>) -> Role {
    const ROLES: [Role; 4] = [Role::Ok, Role::Careful, Role::Warning, Role::Critical];
    (0..4).rev().find(|&k| p.nice_lists[k].matches(nice, class)).map_or(Role::Default, |k| ROLES[k])
}

fn status_role(p: &Processes, s: u8) -> Role {
    const ROLES: [Role; 4] = [Role::Ok, Role::Careful, Role::Warning, Role::Critical];
    if s >= 128 {
        return Role::Default;
    }
    (0..4).rev().find(|&k| p.status_lists[k] & (1u128 << s) != 0).map_or(Role::Default, |k| ROLES[k])
}

fn ni_cell(out: &mut Panel, p: &Processes, nice: Option<i32>, class: Option<&'static str>) {
    let role = nice_role(p, nice, class);
    match (class, nice) {
        (Some(c), _) => crate::addf!(out, role, 0, "{:>3} ", c), // PROC-043
        (None, Some(n)) => crate::addf!(out, role, 0, "{:>3} ", n),
        (None, None) => crate::addf!(out, Role::Default, 0, "{:>3} ", "?"),
    }
}

fn s_cell(out: &mut Panel, p: &Processes, s: u8) {
    let c = if s.is_ascii_graphic() { s as char } else { '?' };
    crate::addf!(out, status_role(p, s), 0, "{:>1} ", c);
}

/// One header cell padded to `width` (cell includes its trailing space).
fn hcell(out: &mut Panel, w: &World, key: Option<SortKey>, label: &str, width: usize, right: bool) {
    let p = &w.processes;
    let active = key == Some(p.eff);
    let role = if active { Role::Sort } else { Role::Default };
    let mut s = Small::default();
    let _ = fmt::Write::write_str(&mut s, label);
    // PROC-064: reverse marker where the column width allows.
    if active && p.reverse && label.chars().count() + 2 <= width {
        let _ = fmt::Write::write_str(&mut s, if w.ui.ascii { "^" } else { "↑" });
    }
    let n = width - 1;
    if right {
        crate::addf!(out, role, 0, "{:>n$} ", s, n = n);
    } else {
        crate::addf!(out, role, 0, "{:<n$} ", s, n = n);
    }
}

fn header(out: &mut Panel, w: &World, c: &Cols, programs: bool, has_cursor: bool, cmd_w: usize) {
    let p = &w.processes;
    out.add(Role::Default, 0, " ");
    // PROC-044: normalised header; CPU% has no trailing separator of its own.
    let label = if !w.ui.normalize_cpu {
        crate::small!("CPU%")
    } else if p.ncpu < 10 {
        crate::small!("CPU%/{}", p.ncpu)
    } else {
        crate::small!("CPUi")
    };
    if label.as_str().len() >= 6 {
        let role = if p.eff == SortKey::Cpu { Role::Sort } else { Role::Default };
        crate::addf!(out, role, 0, "{:<6}", label);
    } else {
        hcell(out, w, Some(SortKey::Cpu), label.as_str(), 6, false);
    }
    hcell(out, w, Some(SortKey::Mem), "MEM%", 6, false);
    if c.virt {
        hcell(out, w, None, "VIRT", 6, false);
    }
    if c.res {
        hcell(out, w, None, "RES", 6, false);
    }
    hcell(out, w, Some(SortKey::Pid), if programs { "NPROCS" } else { "PID" }, c.pidw + 1, true);
    hcell(out, w, Some(SortKey::User), "USER", 10, false);
    if c.time {
        hcell(out, w, Some(SortKey::Time), "TIME+", 9, true);
    }
    if c.thr {
        hcell(out, w, None, "THR", 4, false);
    }
    if c.ni {
        hcell(out, w, None, "NI", 4, true);
    }
    if c.s {
        hcell(out, w, None, "S", 2, true);
    }
    if c.ior {
        hcell(out, w, Some(SortKey::Io), "R/s", 5, true);
    }
    if c.iow {
        hcell(out, w, Some(SortKey::Io), "W/s", 5, false);
    }
    if c.core {
        hcell(out, w, Some(SortKey::Core), "CPU", 4, true);
    }
    let mut rem = cmd_w;
    let role = if p.eff == SortKey::Name { Role::Sort } else { Role::Default };
    put_cut(out, role, if programs { "Programs" } else { "Command" }, &mut rem);
    if p.eff == SortKey::Name && p.reverse {
        put_cut(out, role, if w.ui.ascii { "^" } else { "↑" }, &mut rem);
    }
    if has_cursor {
        let hint = if programs { " ('k' to kill)" } else { " ('e' to pin | 'k' to kill)" };
        put_cut(out, Role::Default, hint, &mut rem);
    }
}

/// Arguments with the shared scroll offset (PROC-046 step 6).
fn args_cut(out: &mut Panel, w: &World, args: &str, rem: &mut usize) {
    if args.is_empty() {
        return;
    }
    put_cut(out, Role::Default, " ", rem);
    let off = w.processes.arg_scroll;
    if off == 0 {
        put_cut(out, Role::Default, args, rem);
    } else {
        put_cut(out, Role::Default, if w.ui.ascii { "~" } else { "…" }, rem);
        let skip = args.char_indices().nth(off).map_or(args.len(), |(b, _)| b);
        put_cut(out, Role::Default, &args[skip..], rem);
    }
}

fn proc_row(out: &mut Panel, w: &World, c: &Cols, k: usize, cmd_w: usize) {
    let p = &w.processes;
    let i = p.rows[k] as usize;
    let (pr, d) = (&p.procs[i], &p.der[i]);
    let e = &p.slab[d.slot as usize];
    let cur = k == p.cursor;
    out.add(if cur { Role::Selected } else { Role::Default }, 0, if cur { ">" } else { " " });
    cpu_cell(out, w, d.cpu, k == p.max_cpu);
    mem_cell(out, p, d.mem, k == p.max_mem);
    if c.virt {
        bytes_cell(out, pr.vms);
    }
    if c.res {
        bytes_cell(out, Some(pr.rss));
    }
    crate::addf!(out, Role::Default, 0, "{:>n$} ", pr.pid, n = c.pidw);
    match pr.uid {
        None => crate::addf!(out, Role::Default, 0, "{:<9} ", "?"),
        Some(u) => match p.users.by_uid(u) {
            Some(n) => crate::addf!(out, Role::Default, 0, "{:<9} ", cut9(n)),
            None => crate::addf!(out, Role::Default, 0, "{:<9} ", u),
        },
    }
    if c.time {
        time_cell(out, pr.cpu_user + pr.cpu_system);
    }
    if c.thr {
        crate::addf!(out, Role::Default, 0, "{:<3} ", pr.num_threads);
    }
    if c.ni {
        ni_cell(out, p, pr.nice, pr.priority_class);
    }
    if c.s {
        s_cell(out, p, pr.status);
    }
    if c.ior {
        crate::addf!(out, Role::Default, 0, "{:>4} ", rate_str(d.ior));
    }
    if c.iow {
        crate::addf!(out, Role::Default, 0, "{:<4} ", rate_str(d.iow));
    }
    if c.core {
        match pr.last_cpu {
            Some(n) => crate::addf!(out, Role::Default, 0, "{:>3} ", n),
            None => crate::addf!(out, Role::Default, 0, "{:>3} ", "-"),
        }
    }

    // PROC-046 command cell; PROC-124 pinned row uses PROCESS_SELECTED.
    let pinned = p.pin.as_ref().map_or(false, |x| x.pid == pr.pid && x.start == pr.start_time);
    let prole = if cur || pinned { Role::ProcessSelected } else { Role::Process };
    let mut rem = cmd_w;
    let name = disp_name(pr, e);
    match e.cmd.as_deref() {
        None => put_cut(out, prole, name, &mut rem),
        Some("") => {
            put_cut(out, prole, "[", &mut rem);
            put_cut(out, prole, name, &mut rem);
            put_cut(out, prole, "]", &mut rem);
        }
        Some(s) => {
            let a0 = &s[..e.arg0 as usize];
            let args = s.get(e.arg0 as usize + 1..).unwrap_or("");
            let (path, exe) = if a0.starts_with(pr.name.as_str()) {
                ("", a0)
            } else {
                match a0.rfind(|ch: char| ch == '/' || (cfg!(windows) && ch == '\\')) {
                    Some(k) => (&a0[..=k], &a0[k + 1..]),
                    None => ("", a0),
                }
            };
            if w.ui.long_names {
                put_cut(out, Role::Default, path, &mut rem);
            }
            put_cut(out, prole, exe, &mut rem);
            args_cut(out, w, args, &mut rem);
        }
    }
}

fn prog_row(out: &mut Panel, w: &World, c: &Cols, k: usize, cmd_w: usize) {
    let p = &w.processes;
    let g = &p.progs[p.prows[k] as usize];
    let cur = k == p.cursor;
    out.add(if cur { Role::Selected } else { Role::Default }, 0, if cur { ">" } else { " " });
    cpu_cell(out, w, g.cpu, k == p.max_cpu);
    mem_cell(out, p, g.mem, k == p.max_mem);
    if c.virt {
        bytes_cell(out, g.vms);
    }
    if c.res {
        bytes_cell(out, Some(g.rss));
    }
    crate::addf!(out, Role::Default, 0, "{:>7} ", g.n);
    match g.uid {
        Agree::Mixed => crate::addf!(out, Role::Default, 0, "{:<9} ", "_"),
        Agree::Same(None) => crate::addf!(out, Role::Default, 0, "{:<9} ", "?"),
        Agree::Same(Some(u)) => match p.users.by_uid(u) {
            Some(n) => crate::addf!(out, Role::Default, 0, "{:<9} ", cut9(n)),
            None => crate::addf!(out, Role::Default, 0, "{:<9} ", u),
        },
    }
    if c.time {
        time_cell(out, g.time);
    }
    if c.thr {
        crate::addf!(out, Role::Default, 0, "{:<3} ", g.thr);
    }
    if c.ni {
        match (g.nice, g.class) {
            (Agree::Same(n), Agree::Same(cl)) => ni_cell(out, p, n, cl),
            _ => crate::addf!(out, Role::Default, 0, "{:>3} ", "_"),
        }
    }
    if c.s {
        if g.status == b'_' {
            crate::addf!(out, Role::Default, 0, "{:>1} ", "_");
        } else {
            s_cell(out, p, g.status);
        }
    }
    if c.ior {
        crate::addf!(out, Role::Default, 0, "{:>4} ", rate_str(g.ior));
    }
    if c.iow {
        crate::addf!(out, Role::Default, 0, "{:<4} ", rate_str(g.iow));
    }
    // PROC-132: `[name]`, or name + arguments of the top-CPU member in long-name mode.
    let prole = if cur { Role::ProcessSelected } else { Role::Process };
    let mut rem = cmd_w;
    if w.ui.long_names {
        put_cut(out, prole, &g.name, &mut rem);
        let i = g.top as usize;
        if let Some(e) = p.der.get(i).map(|d| &p.slab[d.slot as usize]) {
            if let Some(s) = e.cmd.as_deref() {
                args_cut(out, w, s.get(e.arg0 as usize + 1..).unwrap_or(""), &mut rem);
            }
        }
    } else {
        put_cut(out, prole, "[", &mut rem);
        put_cut(out, prole, &g.name, &mut rem);
        put_cut(out, prole, "]", &mut rem);
    }
}

/// PROC-080: separator plus current/min/max rows.
fn summary(out: &mut Panel, w: &World, c: &Cols, cmd_w: usize) {
    let p = &w.processes;
    out.newline();
    let glyph = if w.ui.ascii { "_" } else { "─" };
    crate::addf!(out, Role::Separator, 0, "{}", Rep(glyph, c.fixed()));
    let rows: [(&[f64; 6], &str); 3] = [
        (&p.summary.cur, "< current"),
        (&p.summary.min, "< min ('M' to reset)"),
        (&p.summary.max, "< max ('M' to reset)"),
    ];
    let f = Role::Filter;
    for (v, label) in rows {
        out.newline();
        out.add(f, 0, " ");
        if v[0].is_nan() {
            crate::addf!(out, f, 0, "{:<6}", "-");
        } else {
            crate::addf!(out, f, 0, "{:<6.1}", cpu_disp(w, v[0]));
        }
        if v[1].is_nan() {
            crate::addf!(out, f, 0, "{:<5} ", "-");
        } else {
            crate::addf!(out, f, 0, "{:<5.1} ", v[1]);
        }
        for (on, x) in [(c.virt, v[2]), (c.res, v[3])] {
            if on {
                if x.is_nan() {
                    crate::addf!(out, f, 0, "{:<5} ", "-");
                } else {
                    crate::addf!(out, f, 0, "{:<5} ", auto_unit(x as u64, false, MinSym::None));
                }
            }
        }
        let b = |on: bool, n: usize| if on { n } else { 0 };
        blank(out, c.pidw + 1 + 10 + b(c.time, 9) + b(c.thr, 4) + b(c.ni, 4) + b(c.s, 2));
        if c.ior {
            crate::addf!(out, f, 0, "{:>4} ", rate_str(v[4]));
        }
        if c.iow {
            crate::addf!(out, f, 0, "{:<4} ", rate_str(v[5]));
        }
        if c.core {
            blank(out, 4);
        }
        let mut rem = cmd_w;
        put_cut(out, f, label, &mut rem);
    }
}

fn pct3(out: &mut Panel, s: &crate::modules::processes::Stat3, div: f64) {
    if s.n == 0 {
        out.add(Role::Default, 0, "     -%     -%     -%");
    } else {
        crate::addf!(
            out,
            Role::Default,
            0,
            "{:>5.1}% {:>5.1}% {:>5.1}%",
            s.min / div,
            s.max / div,
            s.sum / s.n as f64 / div
        );
    }
}

fn opt_u<T: fmt::Display>(v: Option<T>) -> OptD<T> {
    OptD(v)
}

/// Displays the value or `?` (denied, PROC-122).
struct OptD<T>(Option<T>);

impl<T: fmt::Display> fmt::Display for OptD<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(v) => v.fmt(f),
            None => f.write_str("?"),
        }
    }
}

/// PROC-122 extended block for the pinned process.
fn extended(out: &mut Panel, w: &World) {
    let p = &w.processes;
    let Some(pin) = &p.pin else { return };
    line(out);
    if pin.exited {
        crate::addf!(out, Role::Title, 0, "Pinned process {} (pid {}) exited", pin.name, pin.pid);
        out.newline();
        return;
    }
    crate::addf!(out, Role::Title, 0, "Pinned process {} (pid {})", pin.name, pin.pid);
    out.add(Role::Default, 0, " ('e' to unpin)");
    let x = pin.ext.as_ref();
    let d = Role::Default;
    let div = if w.ui.normalize_cpu { p.ncpu.max(1) as f64 } else { 1.0 };
    let bsd_lite = cfg!(any(target_os = "openbsd", target_os = "netbsd"));

    out.newline();
    out.add(d, 0, " CPU  min/max/mean: ");
    pct3(out, &pin.cpu, div);
    if !(cfg!(target_os = "macos") || bsd_lite) {
        crate::addf!(out, d, 0, "     Affinity: {} cores", opt_u(x.and_then(|x| x.affinity_cores)));
    }
    if cfg!(target_os = "linux") {
        match x.and_then(|x| x.ionice) {
            Some((cls, lvl)) => crate::addf!(out, d, 0, "     IO nice: {} {}", cls, lvl),
            None => out.add(d, 0, "     IO nice: ?"),
        }
    }

    out.newline();
    out.add(d, 0, " MEM  min/max/mean: ");
    pct3(out, &pin.mem, 1.0);
    crate::addf!(out, d, 0, "     RSS {}", auto_unit(pin.rss, false, MinSym::None));
    match pin.vms {
        Some(v) => crate::addf!(out, d, 0, "  VIRT {}", auto_unit(v, false, MinSym::None)),
        None => out.add(d, 0, "  VIRT ?"),
    }
    if let Some(s) = x.and_then(|x| x.swap_bytes) {
        crate::addf!(out, d, 0, "  Swap {}", auto_unit(s, false, MinSym::None));
    } else if cfg!(target_os = "linux") {
        out.add(d, 0, "  Swap ?");
    }
    if cfg!(target_os = "linux") {
        match x.and_then(|x| x.shared_bytes) {
            Some(s) => crate::addf!(out, d, 0, "  Shared {}", auto_unit(s, false, MinSym::None)),
            None => out.add(d, 0, "  Shared ?"),
        }
    }

    out.newline();
    crate::addf!(out, d, 0, " Open: {} threads", pin.threads);
    if cfg!(windows) {
        crate::addf!(out, d, 0, ", {} handles", opt_u(x.and_then(|x| x.num_handles)));
    } else if !bsd_lite {
        crate::addf!(out, d, 0, ", {} fds", opt_u(x.and_then(|x| x.num_fds)));
    }
    if !bsd_lite {
        crate::addf!(
            out,
            d,
            0,
            ", {} TCP, {} UDP",
            opt_u(x.and_then(|x| x.tcp)),
            opt_u(x.and_then(|x| x.udp))
        );
    }
    if x.map_or(false, |x| x.ctx_voluntary.is_some()) {
        match pin.ctx_rate {
            Some((v, i)) => crate::addf!(out, d, 0, "          Ctx sw: {:.0} vol, {:.0} invol", v, i),
            None => out.add(d, 0, "          Ctx sw: - vol, - invol"),
        }
    }
    out.newline(); // blank line before the header
}

/// Shared by processlist and programlist.
pub(super) fn render_list(w: &World, area: Area, out: &mut Panel, programs: bool) -> Align {
    let p = &w.processes;
    if !w.ui.processes || area.max_height == 0 {
        return Align::Top;
    }
    p.wide.set(!area.narrow); // PROC-051: tells `update` whether R/s, W/s are shown.
    let c = Cols::new(p, area, programs);
    let cmd_w = (area.width as usize).saturating_sub(c.fixed());
    let rows = rows_for_height(w, area.max_height) as usize;
    // The held views belong to one mode; render nothing stale while modes disagree.
    let len = match (programs, p.programs) {
        (true, true) => p.prows.len(),
        (false, false) => p.rows.len(),
        _ => 0,
    };
    if !programs {
        extended(out, w);
    }
    line(out);
    header(out, w, &c, programs, len > 0, cmd_w);
    let start = p.window(rows);
    for k in start..(start + rows).min(len) {
        out.newline();
        if programs {
            prog_row(out, w, &c, k, cmd_w);
        } else {
            proc_row(out, w, &c, k, cmd_w);
        }
    }
    if summary_active(p) {
        summary(out, w, &c, cmd_w);
    }
    Align::Top
}
