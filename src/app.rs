//! Application entry: start-up, the main loop and key dispatch (01 sections 3 and 5).

use std::fs::File;
use std::io::{self, IsTerminal, Write};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::alerts::{Alerts, TopProcesses};
use crate::cli::{self, Cmd};
use crate::config::{Config, Value};
use crate::modules::{self, Ctx, DiskioMode, ModuleId, UiState, World};
use crate::platform::{self, Platform, Signal, Sys};
use crate::term::{Event, Key, Palette, Screen, Terminal};
use crate::ui::keys::{self, Action};
use crate::ui::layout::{self, Panels, MIN_H, MIN_W};
use crate::ui::panel::Panel;
use crate::ui::popup::{self, EditResult, LineEditor, Popup};
use crate::ui::status::Status;
use crate::ui::{help, style};

/// Delay of the priming second sample (UI-020 step 6, PERF-012).
const PRIMING: Duration = Duration::from_millis(500);
/// Tolerance when deciding whether a module is due, so timer jitter never skips a cycle.
const DUE_SLACK: Duration = Duration::from_millis(20);
/// Modules allowed in `[ui] sidebar_order` (UI-016).
const SIDEBAR: [ModuleId; 5] = [ModuleId::Network, ModuleId::Diskio, ModuleId::Fs, ModuleId::Sensors, ModuleId::Now];

const TRY_HELP: &str = "Try 'ogle --help' for more information.";

fn wall_now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn main() -> i32 {
    // UI-020 step 1
    let args = match cli::parse(std::env::args_os().skip(1)) {
        Cmd::Exit { code, text } => {
            let nl = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
            if code == 0 {
                print!("{text}{nl}");
                let _ = io::stdout().flush();
            } else {
                eprint!("{text}{nl}");
            }
            return code;
        }
        Cmd::Run(a) => a,
    };
    let mut cfg = if args.no_config {
        Config::default()
    } else {
        match Config::load(args.config.as_deref()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("ogle: {e}"); // UI-010
                return 2;
            }
        }
    };
    cli::apply_overrides(&args, &mut cfg);
    for w in cfg.unknown_key_warnings() {
        eprintln!("{w}");
    }
    let (order, order_n) = match sidebar_order(&cfg) {
        Ok(o) => o,
        Err(bad) => {
            match &cfg.source {
                Some(p) => eprintln!("ogle: {}: [ui] sidebar_order: invalid module '{bad}'", p.display()),
                None => eprintln!("ogle: [ui] sidebar_order: invalid module '{bad}'"),
            }
            return 2;
        }
    };
    // UI-004
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!("ogle: standard input and output must be a terminal");
        return 1;
    }
    let log = match &args.log_file {
        Some(p) => match File::create(p) {
            Ok(f) => Some(f),
            Err(e) => {
                eprintln!("ogle: {}: {e}", p.display());
                return 1;
            }
        },
        None => None,
    };

    // UI-020 step 2: platform, colours, glyphs.
    let ui = ui_state(&cfg);
    let depth = style::detect_depth(cfg.str(&["ui", "color"]).unwrap_or("auto"));
    let pal = style::palette(depth, cfg.bool(&["ui", "bold"]).unwrap_or(true), cfg.bool(&["ui", "background"]).unwrap_or(true));
    let esc_ms = cfg.i64(&["ui", "escape_timeout_ms"]).unwrap_or(30).clamp(0, 10_000) as u64;
    let sys = platform::new();
    let mut w = build_world(cfg, ui);
    if let Some(f) = w.cfg.str(&["processlist", "filter"]) {
        if let Err(e) = w.processes.set_filter(f) {
            eprintln!("ogle: {e}\n{TRY_HELP}");
            return 2;
        }
    }
    w.processes.set_collecting(w.ui.processes);

    // UI-020 step 3
    let term = match Terminal::open(Duration::from_millis(esc_ms)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ogle: {e}");
            return 1;
        }
    };
    let (cols, rows) = term.size();
    let mut app = App {
        w,
        sys,
        term,
        scr: Screen::new(cols, rows),
        pal,
        panels: std::array::from_fn(|_| Panel::new()),
        popup_panel: Panel::new(),
        help_l: Panel::new(),
        help_r: Panel::new(),
        order,
        order_n,
        popup: None,
        help: None,
        status: Status::default(),
        last: [None; ModuleId::COUNT],
        visible_rows: 0,
        text: String::with_capacity(64),
        log,
    };
    let res = app.run(args.stop_after);
    app.log(format_args!("exit: {res:?}\n"));
    // UI-021: restore the terminal before printing a fatal error.
    drop(app);
    match res {
        Ok(code) => code,
        Err(e) => {
            eprintln!("ogle: {e}");
            1
        }
    }
}

/// `[ui] sidebar_order` (UI-013, UI-016). Err(name) for an invalid entry.
fn sidebar_order(cfg: &Config) -> Result<([ModuleId; 5], usize), String> {
    let mut out = SIDEBAR;
    let list = match cfg.get(&["ui", "sidebar_order"]) {
        None => return Ok((out, SIDEBAR.len())),
        Some(Value::Array(a)) => a,
        Some(_) => return Err("(not an array)".to_string()),
    };
    let mut n = 0;
    for v in list {
        let Value::Str(s) = v else { return Err("(not a string)".to_string()) };
        let name = s.trim().to_ascii_lowercase();
        match ModuleId::from_name(&name) {
            Some(m) if SIDEBAR.contains(&m) && !out[..n].contains(&m) => {
                out[n] = m;
                n += 1;
            }
            _ => return Err(s.clone()),
        }
    }
    Ok((out, n))
}

/// Start-up display state from config (UI-012, UI-013, UI-014 and module keys).
fn ui_state(cfg: &Config) -> UiState {
    let b = |sect: &str, key: &str, d: bool| cfg.bool(&[sect, key]).unwrap_or(d);
    let mut enabled = [true; ModuleId::COUNT];
    for m in ModuleId::ALL {
        enabled[m as usize] = b(m.name(), "enabled", true);
    }
    let history = b("global", "history", true);
    let ascii = style::ascii_glyphs(cfg.str(&["ui", "unicode"]).unwrap_or("auto"));
    let short = b("processlist", "short_name", true);
    UiState {
        enabled,
        percpu: b("ui", "percpu", false),
        left_sidebar: b("ui", "left_sidebar", true),
        quicklook: b("ui", "quicklook", true),
        full_quicklook: b("ui", "full_quicklook", false),
        top: b("ui", "top", true),
        // SYS-096: sparklines need UTF-8 glyphs and history.
        sparkline: b("ui", "sparkline", false) && !ascii && history,
        history,
        normalize_cpu: b("ui", "normalize_cpu", false),
        net_bytes: b("network", "bytes", false),
        net_sum: b("network", "sum", false),
        net_cumulative: b("network", "cumulative", false),
        diskio_mode: match cfg.str(&["diskio", "mode"]) {
            Some("iops") => DiskioMode::Iops,
            Some("latency") => DiskioMode::Latency,
            _ => DiskioMode::Bytes,
        },
        show_ramfs: b("diskio", "show_ramfs", false),
        fs_free: b("fs", "free_space", false),
        fahrenheit: b("sensors", "fahrenheit", false),
        programs: b("processlist", "programs", false),
        long_names: !short,
        hide_kernel_threads: b("processlist", "hide_kernel_threads", false),
        ascii,
        separator: b("ui", "separator", true),
        arrow_keys_sort: b("ui", "arrow_keys_sort", false),
        // PROC-029: `--no-processes` sets `processlist.collect = false`.
        processes: b("processlist", "collect", true),
        alert_panel: enabled[ModuleId::Alert as usize],
        refresh: cfg.f64(&["global", "refresh"]).unwrap_or(2.0).clamp(0.1, 3600.0),
    }
}

/// Build every module state. `Alerts::new` may print config warnings, so this runs
/// before the terminal is opened.
fn build_world(cfg: Config, ui: UiState) -> World {
    let mut alerts = Alerts::new(&cfg);
    let a = &mut alerts;
    let system = modules::system::System::new(&cfg, &ui, a);
    let uptime = modules::uptime::Uptime::new(&cfg, &ui, a);
    let now = modules::now::Now::new(&cfg, &ui, a);
    let cpu = modules::cpu::Cpu::new(&cfg, &ui, a);
    let load = modules::load::Load::new(&cfg, &ui, a);
    let mem = modules::mem::Mem::new(&cfg, &ui, a);
    let memswap = modules::memswap::MemSwap::new(&cfg, &ui, a);
    let quicklook = modules::quicklook::QuickLook::new(&cfg, &ui, a);
    let network = modules::network::Network::new(&cfg, &ui, a);
    let diskio = modules::diskio::DiskIo::new(&cfg, &ui, a);
    let fs = modules::fs::Fs::new(&cfg, &ui, a);
    let sensors = modules::sensors::Sensors::new(&cfg, &ui, a);
    let processes = modules::processes::Processes::new(&cfg, &ui, a);
    World {
        cfg,
        ui,
        alerts,
        system,
        uptime,
        now,
        cpu,
        load,
        mem,
        memswap,
        quicklook,
        network,
        diskio,
        fs,
        sensors,
        processes,
    }
}

/// What the main loop does after a key press.
enum Flow {
    None,
    Redraw,
    /// F5 / Ctrl-R: new cycle now, full repaint (UI-045).
    Refresh,
    Quit,
}

struct App {
    w: World,
    sys: Sys,
    term: Terminal,
    scr: Screen,
    pal: Palette,
    /// One reusable panel per module.
    panels: Panels,
    popup_panel: Panel,
    help_l: Panel,
    help_r: Panel,
    order: [ModuleId; 5],
    order_n: usize,
    popup: Option<Popup>,
    /// Help screen open, with its scroll offset.
    help: Option<u16>,
    status: Status,
    /// Last collection time per module (`Cpu`/`Percpu` share; processes use `Processlist`).
    last: [Option<Instant>; ModuleId::COUNT],
    /// Process rows shown last frame (UI-062).
    visible_rows: u16,
    text: String,
    log: Option<File>,
}

impl App {
    fn log(&mut self, args: std::fmt::Arguments<'_>) {
        if let Some(f) = self.log.as_mut() {
            let _ = f.write_fmt(args);
        }
    }

    fn refresh(&self) -> Duration {
        Duration::from_secs_f64(self.w.ui.refresh)
    }

    /// UI-020 steps 4-6 and the cycle loop (UI-024 to UI-031).
    fn run(&mut self, stop_after: Option<u64>) -> io::Result<i32> {
        let t0 = Instant::now();
        let need = self.needs();
        self.collect(&need, t0);
        self.draw()?;
        self.log(format_args!("first frame after {} us\n", t0.elapsed().as_micros()));
        let mut deadline = t0 + PRIMING;
        let mut force = true; // the priming sample takes every needed module
        let mut cycles = 0u64;
        let mut dirty = false;
        loop {
            let now = Instant::now();
            if now >= deadline {
                // One cycle: collect, derive, draw; the deadline is cycle_start + refresh.
                let mask = self.due_mask(now, force);
                self.collect(&mask, now);
                let procs: Option<&dyn TopProcesses> = if self.w.ui.processes { Some(&self.w.processes) } else { None };
                self.w.alerts.end_cycle(wall_now(), procs);
                self.draw()?;
                deadline = now + self.refresh();
                force = false;
                dirty = false;
                cycles += 1;
                if self.log.is_some() {
                    let us = now.elapsed().as_micros();
                    self.log(format_args!("cycle {cycles}: {us} us\n"));
                }
                if stop_after.is_some_and(|n| cycles >= n) {
                    return Ok(0);
                }
                continue;
            }
            if self.status.expire(now) {
                dirty = true;
            }
            // A pending redraw first drains queued events (zero timeout), so bursts of
            // resizes or keys collapse into one frame (UI-031).
            let ev = if dirty {
                self.term.wait(Some(Duration::ZERO))?
            } else {
                let mut wake = deadline;
                if let Some(u) = self.status.until() {
                    wake = wake.min(u);
                }
                self.term.wait(Some(wake.saturating_duration_since(now)))?
            };
            match ev {
                Event::Timeout => {
                    if dirty {
                        self.draw()?;
                        dirty = false;
                    }
                }
                Event::Quit => return Ok(0),
                Event::Resize(c, r) => {
                    let before = self.needs();
                    self.scr.resize(c, r);
                    self.collect_newly(before);
                    dirty = true;
                }
                Event::Resumed => {
                    self.scr.invalidate();
                    dirty = true;
                }
                Event::Key(k) => match self.on_key(k)? {
                    Flow::None => {}
                    Flow::Redraw => dirty = true,
                    Flow::Refresh => {
                        self.scr.invalidate();
                        force = true;
                        deadline = Instant::now();
                    }
                    Flow::Quit => return Ok(0),
                },
            }
        }
    }

    /// Modules that must be collected for what is visible now (GEN-004, collection rule).
    fn needs(&self) -> [bool; ModuleId::COUNT] {
        use ModuleId as M;
        let ui = &self.w.ui;
        let mut n = [false; ModuleId::COUNT];
        let ql = layout::quicklook_shown(ui);
        let procs = ui.processes && (ui.on(M::Processcount) || ui.on(M::Processlist) || ui.on(M::Programlist));
        let cpu_vis = layout::top_shown(ui, layout::cpu_id(ui));
        n[M::System as usize] = ui.on(M::System);
        n[M::Uptime as usize] = ui.on(M::Uptime);
        n[M::Percpu as usize] = ui.percpu && cpu_vis;
        n[M::Cpu as usize] = cpu_vis || ql || (procs && ui.normalize_cpu);
        // Quicklook reads cpu, mem, memswap and load; processes need the memory total.
        n[M::Mem as usize] = layout::top_shown(ui, M::Mem) || ql || procs;
        n[M::Memswap as usize] = layout::top_shown(ui, M::Memswap) || ql;
        n[M::Load as usize] = layout::top_shown(ui, M::Load) || ql;
        n[M::Quicklook as usize] = ql;
        if layout::sidebar_shown(ui, self.scr.size().0) {
            for &m in &self.order[..self.order_n] {
                n[m as usize] = ui.on(m);
            }
        }
        n[M::Processlist as usize] = procs;
        n
    }

    fn interval(&self, m: ModuleId) -> Duration {
        use ModuleId as M;
        let w = &self.w;
        let r = match m {
            M::System => w.system.refresh(),
            M::Uptime => w.uptime.refresh(),
            M::Now => w.now.refresh(),
            M::Cpu | M::Percpu => w.cpu.refresh(),
            M::Load => w.load.refresh(),
            M::Mem => w.mem.refresh(),
            M::Memswap => w.memswap.refresh(),
            M::Quicklook => w.quicklook.refresh(),
            M::Network => w.network.refresh(),
            M::Diskio => w.diskio.refresh(),
            M::Fs => w.fs.refresh(),
            M::Sensors => w.sensors.refresh(),
            M::Processcount | M::Processlist | M::Programlist => w.processes.refresh(),
            M::Alert => None,
        };
        r.unwrap_or_else(|| self.refresh())
    }

    /// Needed modules whose own interval has passed (GEN-006, UI-024 step 2).
    fn due_mask(&self, now: Instant, force: bool) -> [bool; ModuleId::COUNT] {
        let mut mask = self.needs();
        if !force {
            for m in ModuleId::ALL {
                let i = m as usize;
                if mask[i] {
                    if let Some(t) = self.last[i] {
                        mask[i] = now + DUE_SLACK >= t + self.interval(m);
                    }
                }
            }
        }
        mask
    }

    /// Collect the modules in `mask`, in dependency order.
    fn collect(&mut self, mask: &[bool; ModuleId::COUNT], now: Instant) {
        use ModuleId as M;
        let ctx = Ctx { now, wall: wall_now(), visible_rows: self.visible_rows };
        let on = |m: M| mask[m as usize];
        let sys = &mut self.sys;
        let w = &mut self.w;
        if on(M::System) {
            w.system.update(sys, &ctx, &w.ui, &mut w.alerts);
        }
        if on(M::Uptime) {
            w.uptime.update(sys, &ctx, &w.ui, &mut w.alerts);
        }
        if on(M::Now) {
            w.now.update(sys, &ctx, &w.ui, &mut w.alerts);
        }
        let cpu = on(M::Cpu) || on(M::Percpu);
        if cpu {
            w.cpu.update(sys, &ctx, &w.ui, &mut w.alerts);
        }
        if on(M::Load) {
            w.load.update(sys, &ctx, &w.ui, &mut w.alerts);
        }
        if on(M::Mem) {
            w.mem.update(sys, &ctx, &w.ui, &mut w.alerts);
        }
        if on(M::Memswap) {
            w.memswap.update(sys, &ctx, &w.ui, &mut w.alerts);
        }
        if on(M::Quicklook) {
            w.quicklook.update(sys, &ctx, &w.ui, &mut w.alerts, &w.cpu, &w.mem, &w.memswap, &w.load);
        }
        if on(M::Network) {
            w.network.update(sys, &ctx, &w.ui, &mut w.alerts);
        }
        if on(M::Diskio) {
            w.diskio.update(sys, &ctx, &w.ui, &mut w.alerts);
        }
        if on(M::Fs) {
            w.fs.update(sys, &ctx, &w.ui, &mut w.alerts);
        }
        if on(M::Sensors) {
            w.sensors.update(sys, &ctx, &w.ui, &mut w.alerts);
        }
        if on(M::Processlist) {
            let mem_total = w.mem.total().unwrap_or(0);
            let ncpu = w.cpu.logical_cores();
            w.processes.update(sys, &ctx, &w.ui, &mut w.alerts, mem_total, ncpu);
        }
        for (i, &m) in mask.iter().enumerate() {
            if m {
                self.last[i] = Some(now);
            }
        }
        if cpu {
            self.last[M::Cpu as usize] = Some(now);
            self.last[M::Percpu as usize] = Some(now);
        }
    }

    /// Collect modules that became visible since `before` (UI-026 exception: enabling a
    /// module collects it at once, so its panel never shows stale or empty data).
    fn collect_newly(&mut self, before: [bool; ModuleId::COUNT]) {
        let after = self.needs();
        let mut mask = [false; ModuleId::COUNT];
        let mut any = false;
        for i in 0..ModuleId::COUNT {
            mask[i] = after[i] && !before[i];
            any |= mask[i];
        }
        if any {
            self.collect(&mask, Instant::now());
        }
    }

    fn draw(&mut self) -> io::Result<()> {
        self.scr.clear();
        let (sw, sh) = self.scr.size();
        let mut cursor = None;
        if let Some(scroll) = &mut self.help {
            help::draw(&self.w, &mut self.scr, &mut self.help_l, &mut self.help_r, scroll);
        } else if sw < MIN_W || sh < MIN_H {
            layout::too_small(&mut self.scr, &mut self.text);
            self.status.draw(&mut self.scr);
        } else {
            // `rows` is `processlist::rows_for_height` of the list area (0 = list hidden).
            let rows = layout::draw_main(&self.w, &self.order[..self.order_n], &mut self.panels, &mut self.scr);
            if let Some(p) = &self.popup {
                cursor = popup::draw(&mut self.scr, p, &mut self.popup_panel, self.w.ui.ascii);
            }
            self.status.draw(&mut self.scr);
            self.visible_rows = rows;
            self.w.processes.set_page_rows(rows);
        }
        self.scr.set_cursor(cursor);
        self.scr.flush(&mut self.term, &self.pal)
    }

    fn suspend(&mut self) -> io::Result<()> {
        self.term.suspend()?;
        let (c, r) = self.term.size();
        if (c, r) != self.scr.size() {
            self.scr.resize(c, r);
        } else {
            self.scr.invalidate();
        }
        Ok(())
    }

    /// Re-sort/re-filter the held sample after a key changed sort, filter or view.
    /// `Processes::resort` is a no-op while collection is off.
    fn resort(&mut self) {
        self.w.processes.resort(&self.w.ui, &self.w.alerts);
    }

    fn on_key(&mut self, k: Key) -> io::Result<Flow> {
        // UI-037, UI-102, UI-112: Ctrl-C quits from everywhere; Ctrl-Z suspends (UI-023).
        if k == Key::Ctrl('c') {
            return Ok(Flow::Quit);
        }
        if k == Key::Suspend {
            self.suspend()?;
            return Ok(Flow::Redraw);
        }
        let (sw, sh) = self.scr.size();
        let page = sh.saturating_sub(1).max(1);
        if let Some(s) = self.help {
            // UI-111, UI-112
            self.help = match k {
                Key::Char('h') | Key::Char('q') | Key::Esc => None,
                Key::Up => Some(s.saturating_sub(1)),
                Key::Down => Some(s.saturating_add(1)),
                Key::PageUp => Some(s.saturating_sub(page)),
                Key::PageDown => Some(s.saturating_add(page)),
                Key::Home => Some(0),
                Key::End => Some(u16::MAX),
                _ => return Ok(Flow::None),
            };
            return Ok(Flow::Redraw);
        }
        if sw < MIN_W || sh < MIN_H {
            // UI-071: only quit and suspend work on the "too small" screen.
            return Ok(if matches!(k, Key::Char('q') | Key::Esc) { Flow::Quit } else { Flow::None });
        }
        if let Some(p) = self.popup.take() {
            return Ok(self.popup_key(p, k));
        }
        let Some(b) = keys::lookup(k, self.w.ui.arrow_keys_sort) else {
            return Ok(Flow::None); // UI-048, UI-049
        };
        let before = self.needs();
        let flow = self.action(b.action)?;
        self.collect_newly(before);
        Ok(flow)
    }

    /// Keys while a popup is open (UI-041, UI-103, UI-104).
    fn popup_key(&mut self, p: Popup, k: Key) -> Flow {
        match p {
            Popup::Filter { mut ed, mut error } => match ed.key(k) {
                EditResult::Apply => {
                    // An empty pattern clears the filter (set_filter handles it).
                    match self.w.processes.set_filter(&ed.text) {
                        Ok(()) => self.resort(),
                        Err(e) => {
                            error = e;
                            self.popup = Some(Popup::Filter { ed, error });
                        }
                    }
                }
                EditResult::Cancel => {}
                EditResult::Edited => self.popup = Some(Popup::Filter { ed, error }),
                EditResult::Ignored => {
                    self.popup = Some(Popup::Filter { ed, error });
                    return Flow::None;
                }
            },
            Popup::Kill { sel, content } => {
                let sig = match k {
                    Key::Char('t') | Key::Char('y') => Some(Signal::Term),
                    Key::Char('K') => Some(Signal::Kill),
                    Key::Char('n') | Key::Esc => None,
                    _ => {
                        self.popup = Some(Popup::Kill { sel, content });
                        return Flow::None;
                    }
                };
                if let Some(sig) = sig {
                    let msg = self.w.processes.kill(&mut self.sys, &sel, sig);
                    self.status.set_result(&msg, Instant::now());
                }
            }
        }
        Flow::Redraw
    }

    /// Apply a main-screen action from the key table (01 section 5.2).
    fn action(&mut self, a: Action) -> io::Result<Flow> {
        // PgUp/PgDn move by one page less one row of overlap, at least 1.
        let page = self.visible_rows.saturating_sub(1).max(1) as i32;
        let w = &mut self.w;
        let ui = &mut w.ui;
        match a {
            Action::FilterPopup => {
                let ed = LineEditor::new(w.processes.filter_text());
                self.popup = Some(Popup::Filter { ed, error: String::new() });
            }
            Action::ClearFilter => {
                w.processes.clear_filter();
                self.resort();
            }
            Action::Sort(k) => {
                w.processes.set_sort(k);
                self.resort();
            }
            Action::ReverseSort => {
                w.processes.reverse_sort();
                self.resort();
            }
            Action::SortPrev | Action::SortNext => {
                w.processes.cycle_sort(if a == Action::SortNext { 1 } else { -1 });
                self.resort();
            }
            Action::Up => w.processes.cursor_move(-1),
            Action::Down => w.processes.cursor_move(1),
            Action::PageUp => w.processes.cursor_move(-page),
            Action::PageDown => w.processes.cursor_move(page),
            Action::Home => w.processes.cursor_home(),
            Action::End => w.processes.cursor_end(),
            Action::ScrollLeft => w.processes.scroll_args(-1),
            Action::ScrollRight => w.processes.scroll_args(1),
            Action::ScrollLeft8 => w.processes.scroll_args(-8),
            Action::ScrollRight8 => w.processes.scroll_args(8),
            Action::KillPopup => {
                // UI-105
                if ui.processes {
                    if let Some(sel) = w.processes.selection(ui) {
                        if !sel.targets.is_empty() {
                            self.popup = Some(popup::kill_popup(sel, ui.programs));
                        }
                    }
                }
            }
            Action::ReniceUp | Action::ReniceDown => {
                if ui.processes {
                    let d = if a == Action::ReniceUp { 1 } else { -1 };
                    let msg = w.processes.renice(&mut self.sys, &w.ui, d);
                    self.status.set_result(&msg, Instant::now());
                }
            }
            Action::Pin => w.processes.toggle_pin(ui),
            Action::Programs => {
                ui.programs = !ui.programs;
                self.resort();
            }
            Action::LongNames => {
                ui.long_names = !ui.long_names;
                self.resort();
            }
            Action::Normalize => {
                ui.normalize_cpu = !ui.normalize_cpu;
                self.resort();
            }
            Action::ResetMinMax => w.processes.reset_minmax(),
            Action::Processes => {
                ui.processes = !ui.processes;
                w.processes.set_collecting(ui.processes);
            }
            Action::Percpu => ui.percpu = !ui.percpu,
            Action::Sidebar => ui.left_sidebar = !ui.left_sidebar,
            Action::Quicklook => ui.quicklook = !ui.quicklook,
            Action::FullQuicklook => ui.full_quicklook = !ui.full_quicklook,
            Action::Top => ui.top = !ui.top,
            Action::Sparkline => {
                // SYS-096: refused in ASCII mode or without history.
                if ui.ascii {
                    self.status.set("Sparklines need a UTF-8 terminal", false, Instant::now());
                } else if !ui.history {
                    self.status.set("Sparklines need history (--no-history is set)", false, Instant::now());
                } else {
                    ui.sparkline = !ui.sparkline;
                }
            }
            Action::NetBytes => ui.net_bytes = !ui.net_bytes,
            Action::NetSum => ui.net_sum = !ui.net_sum,
            Action::NetCumulative => ui.net_cumulative = !ui.net_cumulative,
            // IO-049: IOPS and latency exclude each other.
            Action::DiskIops => {
                ui.diskio_mode = if ui.diskio_mode == DiskioMode::Iops { DiskioMode::Bytes } else { DiskioMode::Iops }
            }
            Action::DiskLatency => {
                ui.diskio_mode = if ui.diskio_mode == DiskioMode::Latency { DiskioMode::Bytes } else { DiskioMode::Latency }
            }
            Action::FsFree => ui.fs_free = !ui.fs_free,
            Action::Module(m) => ui.enabled[m as usize] = !ui.enabled[m as usize],
            Action::AlertPanel => ui.alert_panel = !ui.alert_panel,
            Action::ClearWarnings => w.alerts.clear_finished(),
            Action::ClearAllFinished => w.alerts.clear_all_finished(),
            Action::Help => self.help = Some(0),
            Action::Refresh => {
                // UI-045, PROC-030
                w.processes.reset_caches();
                w.cpu.reset_caches();
                w.load.reset_caches();
                w.uptime.reset_caches();
                self.sys.reset_caches();
                return Ok(Flow::Refresh);
            }
            Action::Quit => return Ok(Flow::Quit),
            Action::Suspend => self.suspend()?,
        }
        Ok(Flow::Redraw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sidebar_order() {
        let (o, n) = sidebar_order(&Config::default()).unwrap();
        assert_eq!(&o[..n], &SIDEBAR);
    }
}
