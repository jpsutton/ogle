//! Process collection, sort, filter, cursor, pin, kill, renice (04-processes.md).
//!
//! The public methods are used by the app's key dispatch (01 key table) and by the
//! alerts engine.
//!
//! Data layout (07-performance.md): the platform fills `procs` (reused every cycle). A
//! parallel `der` vector holds derived per-cycle values and the slot of the per-identity
//! state in `slab`. `map` maps (pid, start_time) to that slot. Views (`rows`, `prows`)
//! are index vectors; records are never moved. Display strings are built only by the
//! renderers, only for visible rows.

use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use regex_lite::Regex;

use crate::alerts::{Alerts, AutoSort, Limits, TopProcesses};
use crate::config::{Config, Value};
use crate::fmt::Small;
use crate::modules::{Ctx, UiState};
use crate::platform::{ActionError, Platform, Proc, ProcExtended, ProcFields, Signal, Sys};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Auto,
    Cpu,
    Mem,
    Io,
    Time,
    Name,
    User,
    Core,
    Pid,
}

impl SortKey {
    /// CLI/config names: auto cpu mem io time name user core pid.
    pub fn from_name(s: &str) -> Option<SortKey> {
        Some(match s {
            "auto" => SortKey::Auto,
            "cpu" => SortKey::Cpu,
            "mem" => SortKey::Mem,
            "io" => SortKey::Io,
            "time" => SortKey::Time,
            "name" => SortKey::Name,
            "user" => SortKey::User,
            "core" => SortKey::Core,
            "pid" => SortKey::Pid,
            _ => return None,
        })
    }
}

/// Identity + display name of the selected row (or program group).
#[derive(Clone, Debug)]
pub struct Selection {
    /// One entry per process; several in program view.
    pub targets: Vec<(u32, u64)>,
    pub name: String,
}

// ---------------------------------------------------------------------------
// Private data types (shared with the processcount/processlist/programlist renderers).

/// Rate/percent value meaning "denied" (`?`). NaN means unknown (`-`).
pub(super) const DENIED: f64 = -1.0;

/// Per-process I/O is absent on the BSDs (PLT-153, PROC-052).
pub(super) const HAS_IO: bool = !cfg!(any(target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"));

/// Column bits for `[processlist] disable_columns` (PROC-054).
pub(super) mod col {
    pub const VIRT: u16 = 1;
    pub const RES: u16 = 2;
    pub const TIME: u16 = 4;
    pub const THR: u16 = 8;
    pub const NI: u16 = 16;
    pub const S: u16 = 32;
    pub const IOR: u16 = 64;
    pub const IOW: u16 = 128;
    pub const CORE: u16 = 256;
}

/// Sort cycle order (PROC-065, UI-047).
const CYCLE: [SortKey; 8] =
    [SortKey::Cpu, SortKey::Mem, SortKey::Io, SortKey::Time, SortKey::Name, SortKey::User, SortKey::Core, SortKey::Pid];

/// Socket counts of the pinned process are re-read at most this often (PROC-123).
const SOCKET_PERIOD: Duration = Duration::from_secs(10);

/// Rows whose strings are fetched when the app has not yet reported a row count.
const DEFAULT_VISIBLE: usize = 60;

/// Per-identity state (PROC-001): previous counters and cached strings.
pub(super) struct Entry {
    gen: u32,
    name_hash: u32,
    /// Previous cumulative CPU seconds; NaN = no previous sample.
    prev_cpu: f64,
    io_prev: (u64, u64),
    /// Time of `io_prev` in seconds since `t0`; NaN = no previous sample.
    io_t: f64,
    /// argv joined with single spaces, control characters replaced (PROC-046 step 5).
    /// None = not fetched yet; Some("") = empty cmdline (PROC-023).
    pub(super) cmd: Option<Box<str>>,
    /// Byte length of argv[0] inside `cmd`.
    pub(super) arg0: u32,
    /// PLT-144: byte range in `cmd` of the derived long name; (0, 0) = use `Proc::name`.
    dname: (u32, u32),
}

impl Entry {
    fn new(gen: u32) -> Entry {
        Entry { gen, name_hash: 0, prev_cpu: f64::NAN, io_prev: (0, 0), io_t: f64::NAN, cmd: None, arg0: 0, dname: (0, 0) }
    }
    fn cmd_str(&self) -> &str {
        self.cmd.as_deref().unwrap_or("")
    }
}

/// Display name: the comm, or the PLT-144 long name derived from the cached cmdline.
pub(super) fn disp_name<'a>(p: &'a Proc, e: &'a Entry) -> &'a str {
    match (&e.cmd, e.dname) {
        (Some(c), (a, b)) if b > a => c.get(a as usize..b as usize).unwrap_or(&p.name),
        _ => &p.name,
    }
}

/// Derived per-cycle values, parallel to `procs`.
#[derive(Clone, Copy)]
pub(super) struct Der {
    pub(super) slot: u32,
    /// Raw (non-normalised) CPU%; NaN unknown.
    pub(super) cpu: f64,
    pub(super) mem: f64,
    /// Bytes/s; NaN unknown, DENIED denied.
    pub(super) ior: f64,
    pub(super) iow: f64,
}

/// Session-wide uid -> name cache (PROC-024).
#[derive(Default)]
pub(super) struct Users {
    map: HashMap<u32, u32>,
    names: Vec<Box<str>>,
}

impl Users {
    fn resolve(&mut self, sys: &mut Sys, uid: u32) {
        if self.map.contains_key(&uid) {
            return;
        }
        let n = sys.username(uid).unwrap_or_else(|| uid.to_string());
        self.map.insert(uid, self.names.len() as u32);
        self.names.push(n.into_boxed_str());
    }
    pub(super) fn by_uid(&self, uid: u32) -> Option<&str> {
        self.map.get(&uid).map(|&i| &*self.names[i as usize])
    }
    fn clear(&mut self) {
        self.map.clear();
        self.names.clear();
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum FKey {
    Any,
    Name,
    Cmdline,
    User,
    Pid,
    Status,
}

/// One parsed `[key:]pattern` (PROC-070..072).
pub(super) struct Filter {
    pub(super) text: String,
    pub(super) key: FKey,
    pstart: usize,
    re: Regex,
}

impl Filter {
    fn parse(text: &str) -> Result<Filter, String> {
        let (key, pstart) = match text.find(':') {
            Some(i) => match &text[..i] {
                "name" => (FKey::Name, i + 1),
                "cmdline" => (FKey::Cmdline, i + 1),
                "username" | "user" => (FKey::User, i + 1),
                "pid" => (FKey::Pid, i + 1),
                "status" => (FKey::Status, i + 1),
                _ => (FKey::Any, 0),
            },
            None => (FKey::Any, 0),
        };
        let pat = &text[pstart..];
        // PROC-072: smart case, unanchored search, linear-time engine.
        let re = if smart_insensitive(pat) { Regex::new(&format!("(?i){pat}")) } else { Regex::new(pat) };
        let re = re.map_err(|e| format!("Invalid pattern: {e}").replace('\n', " "))?;
        Ok(Filter { text: text.to_string(), key, pstart, re })
    }
    pub(super) fn pattern(&self) -> &str {
        &self.text[self.pstart..]
    }
    pub(super) fn key_name(&self) -> Option<&'static str> {
        Some(match self.key {
            FKey::Any => return None,
            FKey::Name => "name",
            FKey::Cmdline => "cmdline",
            FKey::User => "username",
            FKey::Pid => "pid",
            FKey::Status => "status",
        })
    }
    fn proc_match(&self, p: &Proc, e: &Entry, users: &Users) -> bool {
        match self.key {
            FKey::Any => self.re.is_match(disp_name(p, e)) || self.re.is_match(e.cmd_str()), // PROC-071
            FKey::Name => self.re.is_match(disp_name(p, e)),
            FKey::Cmdline => self.re.is_match(e.cmd_str()),
            FKey::User => self.re.is_match(p.uid.and_then(|u| users.by_uid(u)).unwrap_or("")),
            FKey::Pid => {
                let mut b = [0u8; 10];
                self.re.is_match(fmt_u32(p.pid, &mut b))
            }
            FKey::Status => {
                let b = [p.status];
                std::str::from_utf8(&b).map_or(false, |s| self.re.is_match(s))
            }
        }
    }
}

/// Smart case: insensitive unless the pattern has an uppercase letter that is not part
/// of an escape such as `\D` or `\S`.
fn smart_insensitive(pat: &str) -> bool {
    let mut prev_bs = false;
    for c in pat.chars() {
        if c.is_uppercase() && !prev_bs {
            return false;
        }
        prev_bs = c == '\\' && !prev_bs;
    }
    true
}

fn fmt_u32(mut v: u32, b: &mut [u8; 10]) -> &str {
    let mut i = b.len();
    loop {
        i -= 1;
        b[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    std::str::from_utf8(&b[i..]).unwrap_or("")
}

fn proc_ok(filter: Option<&Filter>, focus: &[Filter], p: &Proc, e: &Entry, users: &Users) -> bool {
    if let Some(f) = filter {
        if !f.proc_match(p, e, users) {
            return false;
        }
    }
    // PROC-074: focus entries are ORed, then ANDed with the filter.
    focus.is_empty() || focus.iter().any(|f| f.proc_match(p, e, users))
}

/// Common value of a program's members, or mixed (`_`, PROC-131).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Agree<T> {
    Same(T),
    Mixed,
}

impl<T: PartialEq + Copy> Agree<T> {
    fn merge(&mut self, v: T) {
        if let Agree::Same(x) = *self {
            if x != v {
                *self = Agree::Mixed;
            }
        }
    }
}

/// One program row (PROC-131). Members are `grp[first..first + n]`.
pub(super) struct Prog {
    pub(super) name: String,
    pub(super) first: u32,
    pub(super) n: u32,
    pub(super) cpu: f64,
    pub(super) mem: f64,
    pub(super) rss: u64,
    pub(super) vms: Option<u64>,
    pub(super) thr: u32,
    pub(super) time: f64,
    pub(super) ior: f64,
    pub(super) iow: f64,
    pub(super) uid: Agree<Option<u32>>,
    pub(super) nice: Agree<Option<i32>>,
    pub(super) class: Agree<Option<&'static str>>,
    /// Common status letter or `_`.
    pub(super) status: u8,
    /// Member (proc index) with the highest CPU%.
    pub(super) top: u32,
}

impl Prog {
    fn blank() -> Prog {
        Prog {
            name: String::new(),
            first: 0,
            n: 0,
            cpu: f64::NAN,
            mem: f64::NAN,
            rss: 0,
            vms: None,
            thr: 0,
            time: 0.0,
            ior: f64::NAN,
            iow: f64::NAN,
            uid: Agree::Mixed,
            nice: Agree::Mixed,
            class: Agree::Mixed,
            status: b'_',
            top: 0,
        }
    }
}

/// Sum of known values: NaN stays unknown, DENIED only when nothing is known.
fn add_known(acc: &mut f64, v: f64) {
    if v.is_nan() {
        return;
    }
    if v < 0.0 {
        if acc.is_nan() {
            *acc = DENIED;
        }
    } else if acc.is_nan() || *acc < 0.0 {
        *acc = v;
    } else {
        *acc += v;
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct Counts {
    pub(super) total: u32,
    pub(super) thr: u64,
    pub(super) run: u32,
    pub(super) slp: u32,
}

/// Filter summary (PROC-080). Columns: CPU% (raw), MEM%, VIRT, RES, R/s, W/s. NaN = `-`.
pub(super) struct Summary {
    pub(super) cur: [f64; 6],
    pub(super) min: [f64; 6],
    pub(super) max: [f64; 6],
}

impl Summary {
    fn new() -> Summary {
        Summary { cur: [f64::NAN; 6], min: [f64::NAN; 6], max: [f64::NAN; 6] }
    }
    fn reset(&mut self) {
        *self = Summary::new();
    }
}

#[derive(Clone, Copy)]
pub(super) struct Stat3 {
    pub(super) min: f64,
    pub(super) max: f64,
    pub(super) sum: f64,
    pub(super) n: u32,
}

impl Stat3 {
    fn new() -> Stat3 {
        Stat3 { min: f64::INFINITY, max: f64::NEG_INFINITY, sum: 0.0, n: 0 }
    }
    fn push(&mut self, v: f64) {
        if v.is_nan() {
            return;
        }
        self.min = self.min.min(v);
        self.max = self.max.max(v);
        self.sum += v;
        self.n += 1;
    }
}

/// Pinned process (section 13).
pub(super) struct Pin {
    pub(super) pid: u32,
    pub(super) start: u64,
    pub(super) name: String,
    pub(super) exited: bool,
    pub(super) ext: Option<ProcExtended>,
    pub(super) cpu: Stat3,
    pub(super) mem: Stat3,
    pub(super) threads: u32,
    pub(super) rss: u64,
    pub(super) vms: Option<u64>,
    last_sock: Option<Instant>,
    sock: (Option<u32>, Option<u32>),
    ctx_prev: Option<(u64, u64, Instant)>,
    /// Voluntary / involuntary context switches per second.
    pub(super) ctx_rate: Option<(f64, f64)>,
}

/// One `nice_*` list (PROC-041).
pub(super) struct NiceList {
    nonzero: bool,
    vals: Vec<i64>,
    labels: Vec<String>,
}

impl NiceList {
    fn parse(v: Option<&Value>, default_nonzero: bool) -> NiceList {
        let mut l = NiceList { nonzero: false, vals: Vec::new(), labels: Vec::new() };
        match v {
            None => l.nonzero = default_nonzero,
            Some(Value::Str(s)) if s == "nonzero" => l.nonzero = true,
            Some(Value::Int(i)) => l.vals.push(*i),
            Some(Value::Array(a)) => {
                for x in a {
                    match x {
                        Value::Int(i) => l.vals.push(*i),
                        Value::Str(s) if s == "nonzero" => l.nonzero = true,
                        Value::Str(s) => l.labels.push(s.clone()),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        l
    }
    pub(super) fn matches(&self, nice: Option<i32>, class: Option<&str>) -> bool {
        if let Some(c) = class {
            return self.labels.iter().any(|l| l == c) || (self.nonzero && c != "No");
        }
        match nice {
            Some(n) => (self.nonzero && n != 0) || self.vals.contains(&(n as i64)),
            None => false,
        }
    }
}

fn parse_status(v: Option<&Value>, default: &[u8]) -> u128 {
    let mut m = 0u128;
    let mut set = |b: u8| {
        if b < 128 {
            m |= 1u128 << b;
        }
    };
    match v {
        None => default.iter().for_each(|&b| set(b)),
        Some(Value::Array(a)) => {
            for x in a {
                if let Value::Str(s) = x {
                    if let Some(&b) = s.as_bytes().first() {
                        set(b);
                    }
                }
            }
        }
        Some(Value::Str(s)) => s.bytes().filter(|b| b.is_ascii_alphabetic()).for_each(set),
        _ => {}
    }
    m
}

fn name_hash(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in s.as_bytes() {
        h = (h ^ b as u32).wrapping_mul(0x0100_0193);
    }
    h
}

/// Join argv with single spaces, replacing control characters (PROC-046 step 5).
fn join_argv(v: &[String]) -> (String, u32) {
    if v.iter().all(|a| a.is_empty()) {
        return (String::new(), 0);
    }
    let mut s = String::with_capacity(v.iter().map(|a| a.len() + 1).sum());
    let mut arg0 = 0;
    for (k, a) in v.iter().enumerate() {
        if k > 0 {
            s.push(' ');
        }
        s.extend(a.chars().map(|c| if c.is_control() { ' ' } else { c }));
        if k == 0 {
            arg0 = s.len() as u32;
        }
    }
    (s, arg0)
}

fn ensure_cmd(sys: &mut Sys, e: &mut Entry, p: &Proc) {
    if e.cmd.is_some() {
        return;
    }
    let (s, a0) = match sys.process_cmdline(p.pid, p.start_time) {
        Some(v) => join_argv(&v),
        None => (String::new(), 0),
    };
    // PLT-144: comm is cut at 15 bytes; use basename(argv[0]) when it extends the comm.
    e.dname = (0, 0);
    if p.name.len() == 15 {
        let a = &s[..a0 as usize];
        let b = a.rfind(|c: char| c == '/' || (cfg!(windows) && c == '\\')).map_or(0, |k| k + 1);
        if a.len() > b + 15 && a[b..].starts_with(p.name.as_str()) {
            e.dname = (b as u32, a0);
        }
    }
    e.cmd = Some(s.into_boxed_str());
    e.arg0 = a0;
}

/// Compare floats with NaN ("absent/denied") last in either direction (PROC-060).
fn cmp_f(a: f64, b: f64, desc: bool) -> Ordering {
    match (a.is_nan(), b.is_nan()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        _ => dir(a.partial_cmp(&b).unwrap_or(Ordering::Equal), desc),
    }
}

fn dir(o: Ordering, desc: bool) -> Ordering {
    if desc {
        o.reverse()
    } else {
        o
    }
}

fn ci_cmp(a: &str, b: &str) -> Ordering {
    a.bytes().map(|c| c.to_ascii_lowercase()).cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

/// Username order, uid as fallback / tie-break; absent uid last.
fn user_cmp(a: Option<u32>, b: Option<u32>, users: &Users, desc: bool) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, _) => Ordering::Greater,
        (_, None) => Ordering::Less,
        (Some(x), Some(y)) => {
            let o = match (users.by_uid(x), users.by_uid(y)) {
                (Some(m), Some(n)) => m.cmp(n).then(x.cmp(&y)),
                _ => x.cmp(&y),
            };
            dir(o, desc)
        }
    }
}

/// I/O sort value: unknown counts as 0, denied/absent is NaN (sorts last).
fn io_val(r: f64, w: f64) -> f64 {
    if !HAS_IO || r < 0.0 || w < 0.0 {
        return f64::NAN;
    }
    nan0(r) + nan0(w)
}

fn nan0(v: f64) -> f64 {
    if v.is_nan() {
        0.0
    } else {
        v
    }
}

fn proc_skey(k: SortKey, p: &Proc, d: &Der) -> f64 {
    match k {
        SortKey::Auto | SortKey::Cpu => nan0(d.cpu),
        SortKey::Mem => p.rss as f64,
        SortKey::Io => io_val(d.ior, d.iow),
        SortKey::Time => p.cpu_user + p.cpu_system,
        SortKey::Core => p.last_cpu.map_or(f64::NAN, |c| c as f64),
        SortKey::Pid => p.pid as f64,
        SortKey::Name | SortKey::User => 0.0,
    }
}

fn prog_skey(k: SortKey, g: &Prog) -> f64 {
    match k {
        SortKey::Auto | SortKey::Cpu => nan0(g.cpu),
        SortKey::Mem => g.rss as f64,
        SortKey::Io => io_val(g.ior, g.iow),
        SortKey::Time => g.time,
        SortKey::Pid => g.n as f64,
        SortKey::Core | SortKey::Name | SortKey::User => f64::NAN,
    }
}

/// PROC-033.
pub(super) fn human_sort(k: SortKey) -> &'static str {
    match k {
        SortKey::Auto | SortKey::Cpu => "CPU consumption",
        SortKey::Mem => "memory consumption",
        SortKey::Io => "disk IO",
        SortKey::Time => "process time",
        SortKey::User => "user name",
        SortKey::Name => "process name",
        SortKey::Pid => "process ID",
        SortKey::Core => "CPU core number",
    }
}

/// TIME+ (PROC-045).
pub(super) fn time_plus(t: f64) -> Small {
    let t = if t.is_finite() && t > 0.0 { t as u64 } else { 0 };
    let (h, m, s) = (t / 3600, (t % 3600) / 60, t % 60);
    if h >= 100 {
        crate::small!("{h}h")
    } else if h >= 1 {
        crate::small!("{h}h{m}:{s:02}")
    } else {
        crate::small!("{m}:{s:02}")
    }
}

fn digits(mut v: u32) -> u8 {
    let mut d = 1;
    while v >= 10 {
        v /= 10;
        d += 1;
    }
    d
}

fn action_error(verb: &str, pid: u32, e: ActionError) -> String {
    match e {
        ActionError::NoSuchProcess => format!("Process {pid} has already exited"),
        ActionError::PermissionDenied => {
            if verb == "kill" {
                format!("Cannot kill {pid}: Operation not permitted")
            } else {
                format!("Cannot renice {pid}: Permission denied")
            }
        }
        ActionError::Other(code) => {
            let m = std::io::Error::from_raw_os_error(code).to_string();
            let m = m.split(" (os error").next().unwrap_or("");
            format!("Cannot {verb} {pid}: {m}")
        }
    }
}

// ---------------------------------------------------------------------------

pub struct Processes {
    refresh: Option<Duration>,
    on: bool,
    pub(super) procs: Vec<Proc>,
    pub(super) der: Vec<Der>,
    pub(super) slab: Vec<Entry>,
    free: Vec<u32>,
    map: HashMap<(u32, u64), u32>,
    gen: u32,
    t0: Instant,
    last_sample: Option<Instant>,
    pub(super) users: Users,
    pub(super) ncpu: u32,
    pid_max: Option<Option<u32>>,
    pub(super) pid_width: usize,
    hide: bool,

    // Views.
    pub(super) counts: Counts,
    /// Filtered, sorted process indices (process mode). Base list in programs mode.
    pub(super) rows: Vec<u32>,
    skey: Vec<f64>,
    pub(super) progs: Vec<Prog>,
    pub(super) nprogs: usize,
    /// Member proc indices grouped by name; `Prog::first/n` index into it.
    pub(super) grp: Vec<u32>,
    /// Filtered, sorted program indices (programs mode).
    pub(super) prows: Vec<u32>,
    vis: Vec<u32>,

    // Sort.
    pub(super) sort: SortKey,
    /// Effective key (auto resolved, PROC-062).
    pub(super) eff: SortKey,
    pub(super) reverse: bool,

    // Cursor (PROC-090..093).
    pub(super) cursor: usize,
    scroll: usize,
    sel: Option<(u32, u64)>,
    sel_name: String,
    psel: bool,
    page_rows: u16,
    pub(super) arg_scroll: usize,
    pub(super) programs: bool,
    long_names: bool,

    // Filter (section 8, 9).
    pub(super) filter: Option<Filter>,
    pub(super) focus: Vec<Filter>,
    pub(super) summary: Summary,
    /// Display positions of the MAX cells (ALR-040); usize::MAX = none.
    pub(super) max_cpu: usize,
    pub(super) max_mem: usize,

    // Config.
    pub(super) cpu_lim: Option<Limits>,
    pub(super) mem_lim: Option<Limits>,
    /// ok, careful, warning, critical.
    pub(super) nice_lists: [NiceList; 4],
    pub(super) status_lists: [u128; 4],
    pub(super) disabled: u16,

    pub(super) pin: Option<Pin>,
    /// Whether the last render showed the optional columns (PROC-051). Written by the
    /// renderer, read by `update` to decide whether to read I/O counters (PROC-026).
    pub(super) wide: Cell<bool>,
}

impl Processes {
    pub fn new(cfg: &Config, ui: &UiState, alerts: &mut Alerts) -> Self {
        let _ = alerts; // processlist stats are decoration only (PROC-041, ALR-007).
        let pl = |k: &'static str| -> [&'static str; 2] { ["processlist", k] };
        let refresh = cfg.f64(&pl("refresh")).filter(|r| *r > 0.0).map(Duration::from_secs_f64);
        let sort = cfg.str(&pl("sort_key")).and_then(SortKey::from_name).unwrap_or(SortKey::Auto);
        let filter = cfg.str(&pl("filter")).filter(|s| !s.is_empty()).and_then(|s| Filter::parse(s).ok());
        let focus = cfg
            .str_list(&pl("focus"))
            .unwrap_or_default()
            .iter()
            .filter(|s| !s.is_empty())
            .filter_map(|s| Filter::parse(s).ok())
            .collect();
        let mut disabled = 0;
        for c in cfg.str_list(&pl("disable_columns")).unwrap_or_default() {
            disabled |= match c.as_str() {
                "virt" => col::VIRT,
                "res" => col::RES,
                "time" => col::TIME,
                "thr" => col::THR,
                "ni" => col::NI,
                "s" => col::S,
                "ior" => col::IOR,
                "iow" => col::IOW,
                "core" => col::CORE,
                _ => 0,
            };
        }
        let def = Some(Limits::new(50.0, 70.0, 90.0, false));
        let nice = |k: &'static str, d: bool| NiceList::parse(cfg.get(&pl(k)), d);
        let st = |k: &'static str, d: &[u8]| parse_status(cfg.get(&pl(k)), d);
        Processes {
            refresh,
            on: ui.processes,
            procs: Vec::new(),
            der: Vec::new(),
            slab: Vec::new(),
            free: Vec::new(),
            map: HashMap::new(),
            gen: 0,
            t0: Instant::now(),
            last_sample: None,
            users: Users::default(),
            ncpu: 1,
            pid_max: None,
            pid_width: 5,
            hide: ui.hide_kernel_threads,
            counts: Counts::default(),
            rows: Vec::new(),
            skey: Vec::new(),
            progs: Vec::new(),
            nprogs: 0,
            grp: Vec::new(),
            prows: Vec::new(),
            vis: Vec::new(),
            sort,
            eff: if sort == SortKey::Auto { SortKey::Cpu } else { sort },
            reverse: false,
            cursor: 0,
            scroll: 0,
            sel: None,
            sel_name: String::new(),
            psel: false,
            page_rows: 0,
            arg_scroll: 0,
            programs: ui.programs,
            long_names: ui.long_names,
            filter,
            focus,
            summary: Summary::new(),
            max_cpu: usize::MAX,
            max_mem: usize::MAX,
            cpu_lim: Alerts::resolve(cfg, "processlist", None, Some("cpu"), def),
            mem_lim: Alerts::resolve(cfg, "processlist", None, Some("mem"), def),
            nice_lists: [
                nice("nice_ok", false),
                nice("nice_careful", false),
                nice("nice_warning", true),
                nice("nice_critical", false),
            ],
            status_lists: [
                st("status_ok", b"RWPI"),
                st("status_careful", b""),
                st("status_warning", b""),
                st("status_critical", b"ZD"),
            ],
            disabled,
            pin: None,
            wide: Cell::new(true),
        }
    }

    pub fn refresh(&self) -> Option<Duration> {
        self.refresh
    }

    /// Collect + derive (PROC-022..). `mem_total` for MEM%, `ncpu` for normalisation.
    pub fn update(&mut self, sys: &mut Sys, ctx: &Ctx, ui: &UiState, alerts: &mut Alerts, mem_total: u64, ncpu: u32) {
        if !ui.processes {
            if self.on {
                self.set_collecting(false);
            }
            return;
        }
        if !self.on {
            self.set_collecting(true);
        }
        self.ncpu = ncpu.max(1);
        self.check_mode(ui);
        self.eff = self.effective(alerts);
        if self.pid_max.is_none() {
            self.pid_max = Some(sys.pid_max());
        }

        // PROC-026 / PROC-051: I/O only for visible rows while R/s or W/s are shown, or
        // for all processes while sorting by io.
        let io_cols = col::IOR | col::IOW;
        let show_io = HAS_IO && self.wide.get() && self.disabled & io_cols != io_cols;
        let io_all = HAS_IO && self.eff == SortKey::Io;
        let n = match ctx.visible_rows as usize {
            0 if self.page_rows > 0 => self.page_rows as usize,
            0 => DEFAULT_VISIBLE,
            v => v,
        };
        self.fill_vis(n, show_io && !io_all);
        let fields = ProcFields { io_all, io_visible: show_io };
        sys.processes(fields, &self.vis, &mut self.procs);

        // PROC-001 / PROC-013: derive per identity.
        let now_s = ctx.now.saturating_duration_since(self.t0).as_secs_f64();
        let dt = self.last_sample.map_or(0.0, |t| ctx.now.saturating_duration_since(t).as_secs_f64());
        self.gen = self.gen.wrapping_add(1);
        let gen = self.gen;
        let pin_key = self.pin.as_ref().filter(|p| !p.exited).map(|p| (p.pid, p.start));
        let mut pin_idx = None;
        let mut maxpid = 0;
        self.der.clear();
        for (i, p) in self.procs.iter().enumerate() {
            maxpid = maxpid.max(p.pid);
            let key = (p.pid, p.start_time);
            if pin_key == Some(key) {
                pin_idx = Some(i);
            }
            let (slot, fresh) = match self.map.get(&key) {
                Some(&s) => (s, false),
                None => {
                    let s = match self.free.pop() {
                        Some(s) => {
                            self.slab[s as usize] = Entry::new(gen);
                            s
                        }
                        None => {
                            self.slab.push(Entry::new(gen));
                            (self.slab.len() - 1) as u32
                        }
                    };
                    self.map.insert(key, s);
                    (s, true)
                }
            };
            let e = &mut self.slab[slot as usize];
            e.gen = gen;
            // PROC-023: re-read cmdline when the name changes (exec without new pid).
            let h = name_hash(&p.name);
            if !fresh && e.name_hash != h {
                e.cmd = None;
            }
            e.name_hash = h;
            let tot = p.cpu_user + p.cpu_system;
            let cpu = if !e.prev_cpu.is_nan() && dt > 0.0 && tot >= e.prev_cpu {
                (tot - e.prev_cpu) / dt * 100.0
            } else {
                f64::NAN
            };
            e.prev_cpu = tot;
            let (ior, iow) = match (p.io_read, p.io_write) {
                (Some(r), Some(w)) => {
                    let out = if !e.io_t.is_nan() && now_s > e.io_t && r >= e.io_prev.0 && w >= e.io_prev.1 {
                        let d = now_s - e.io_t;
                        ((r - e.io_prev.0) as f64 / d, (w - e.io_prev.1) as f64 / d)
                    } else {
                        (f64::NAN, f64::NAN)
                    };
                    e.io_prev = (r, w);
                    e.io_t = now_s;
                    out
                }
                _ => {
                    e.io_t = f64::NAN;
                    let asked = io_all || (show_io && self.vis.binary_search(&p.pid).is_ok());
                    if asked && HAS_IO {
                        (DENIED, DENIED)
                    } else {
                        (f64::NAN, f64::NAN)
                    }
                }
            };
            let mem = if mem_total > 0 { p.rss as f64 * 100.0 / mem_total as f64 } else { f64::NAN };
            self.der.push(Der { slot, cpu, mem, ior, iow });
        }

        // PROC-028: reap state of identities not seen this cycle.
        if self.map.len() > self.procs.len() {
            let (slab, free) = (&mut self.slab, &mut self.free);
            self.map.retain(|_, s| {
                let e = &mut slab[*s as usize];
                if e.gen == gen {
                    true
                } else {
                    e.cmd = None;
                    free.push(*s);
                    false
                }
            });
        }
        self.last_sample = Some(ctx.now);
        let pw = match self.pid_max {
            Some(Some(m)) => digits(m),
            _ => digits(maxpid),
        };
        self.pid_width = pw.clamp(5, 7) as usize;

        // PROC-025: strings for all processes only when filter, focus or sort needs them.
        let (cmd_all, user_all) = self.string_needs();
        if cmd_all || user_all {
            let hide = ui.hide_kernel_threads;
            for i in 0..self.procs.len() {
                let p = &self.procs[i];
                if hide && p.is_kernel_thread {
                    continue; // PROC-021
                }
                if cmd_all {
                    let slot = self.der[i].slot as usize;
                    ensure_cmd(sys, &mut self.slab[slot], p);
                }
                if let (true, Some(u)) = (user_all, p.uid) {
                    self.users.resolve(sys, u);
                }
            }
        }

        self.rebuild(ui);
        self.fetch_window(sys, n);
        self.update_pin(sys, ctx, pin_idx);
    }

    /// Re-sort/re-filter the held sample without collecting (after key presses).
    pub fn resort(&mut self, ui: &UiState, alerts: &Alerts) {
        if !self.on {
            return;
        }
        self.eff = self.effective(alerts);
        self.rebuild(ui);
    }

    /// Turn collection off/on (key `z`): off frees large buffers' contents (not capacity).
    pub fn set_collecting(&mut self, on: bool) {
        self.on = on;
        // PROC-029: re-enabling makes the next sample a first sample.
        self.last_sample = None;
        if on {
            return;
        }
        self.procs.clear();
        self.der.clear();
        self.slab.clear();
        self.free.clear();
        self.map.clear();
        self.rows.clear();
        self.skey.clear();
        self.grp.clear();
        self.prows.clear();
        self.vis.clear();
        self.nprogs = 0;
        self.counts = Counts::default();
        self.pin = None;
        self.max_cpu = usize::MAX;
        self.max_mem = usize::MAX;
    }

    // ---- sort (keys a c m i t p u o P r, Shift-Left/Right) ----
    pub fn set_sort(&mut self, key: SortKey) {
        if key == self.sort {
            return;
        }
        // PROC-063 / PROC-064 / PROC-093: new key resets reverse, cursor and arg scroll.
        self.sort = key;
        if key != SortKey::Auto {
            self.eff = key;
        }
        self.reverse = false;
        self.reset_cursor();
    }

    pub fn reverse_sort(&mut self) {
        self.reverse = !self.reverse;
    }

    /// Cycle through non-auto keys: dir = +1 / -1 (UI-047).
    pub fn cycle_sort(&mut self, dir: i32) {
        let cur = CYCLE.iter().position(|&k| k == self.eff).unwrap_or(0) as i32;
        let n = CYCLE.len() as i32;
        let next = CYCLE[(cur + dir.signum()).rem_euclid(n) as usize];
        self.set_sort(next);
    }

    // ---- cursor (Up/Down/PgUp/PgDn/Home/End) ----
    pub fn cursor_move(&mut self, delta: i32) {
        let len = self.cur_len();
        if len == 0 {
            return;
        }
        self.cursor = (self.cursor as i64 + delta as i64).clamp(0, len as i64 - 1) as usize;
        self.take_sel();
        self.fix_scroll();
    }
    pub fn cursor_home(&mut self) {
        self.cursor_move(i32::MIN);
    }
    pub fn cursor_end(&mut self) {
        self.cursor_move(i32::MAX);
    }
    /// Rows the list showed last frame (set by the renderer's caller) for paging.
    pub fn set_page_rows(&mut self, rows: u16) {
        self.page_rows = rows;
        self.fix_scroll();
    }
    /// Left/Right (1) and `<`/`>` (8) argument scroll.
    pub fn scroll_args(&mut self, delta: i32) {
        self.arg_scroll = (self.arg_scroll as i64 + delta as i64).clamp(0, 1 << 16) as usize;
    }

    // ---- filter (Enter, E, M) ----
    /// Current filter text (for the popup prefill).
    pub fn filter_text(&self) -> &str {
        self.filter.as_ref().map_or("", |f| f.text.as_str())
    }
    /// Err(message) on invalid pattern; popup stays open (PROC-07x).
    pub fn set_filter(&mut self, text: &str) -> Result<(), String> {
        if text.is_empty() {
            self.clear_filter();
            return Ok(());
        }
        let f = Filter::parse(text)?;
        self.filter = Some(f);
        self.summary.reset(); // PROC-081
        self.arg_scroll = 0;
        Ok(())
    }
    pub fn clear_filter(&mut self) {
        if self.filter.take().is_some() {
            self.summary.reset();
            self.arg_scroll = 0;
        }
    }
    pub fn reset_minmax(&mut self) {
        if self.filter.is_none() && self.focus.is_empty() {
            return;
        }
        let s = &mut self.summary;
        s.min = s.cur;
        s.max = s.cur;
    }

    // ---- actions ----
    pub fn selection(&self, ui: &UiState) -> Option<Selection> {
        if !self.on || ui.programs != self.programs {
            return None;
        }
        if self.programs {
            let g = &self.progs[*self.prows.get(self.cursor)? as usize];
            let m = &self.grp[g.first as usize..(g.first + g.n) as usize];
            let targets = m.iter().map(|&i| (self.procs[i as usize].pid, self.procs[i as usize].start_time)).collect();
            Some(Selection { targets, name: g.name.clone() })
        } else {
            let i = *self.rows.get(self.cursor)? as usize;
            let p = &self.procs[i];
            let name = disp_name(p, &self.slab[self.der[i].slot as usize]).to_string();
            Some(Selection { targets: vec![(p.pid, p.start_time)], name })
        }
    }
    /// `e`: pin selection, or unpin when the selection is the pinned process.
    pub fn toggle_pin(&mut self, ui: &UiState) {
        if ui.programs || self.programs {
            return; // PROC-120
        }
        if self.pin.as_ref().map_or(false, |p| p.exited) {
            self.pin = None;
            return;
        }
        let Some(&i) = self.rows.get(self.cursor) else { return };
        let p = &self.procs[i as usize];
        let name = disp_name(p, &self.slab[self.der[i as usize].slot as usize]).to_string();
        if self.pin.as_ref().map_or(false, |x| x.pid == p.pid && x.start == p.start_time) {
            self.pin = None; // PROC-121: frees the extended state.
            return;
        }
        self.pin = Some(Pin {
            pid: p.pid,
            start: p.start_time,
            name,
            exited: false,
            ext: None,
            cpu: Stat3::new(),
            mem: Stat3::new(),
            threads: p.num_threads,
            rss: p.rss,
            vms: p.vms,
            last_sock: None,
            sock: (None, None),
            ctx_prev: None,
            ctx_rate: None,
        });
    }
    /// Returns the status-line message (UI-114).
    pub fn kill(&mut self, sys: &mut Sys, sel: &Selection, sig: Signal) -> String {
        let me = std::process::id();
        let sname = if cfg!(windows) {
            "terminate"
        } else if sig == Signal::Kill {
            "SIGKILL"
        } else {
            "SIGTERM"
        };
        let mut first_err: Option<String> = None;
        let mut sent = 0;
        for &(pid, st) in &sel.targets {
            // PROC-103: never signal ogle itself; the backend checks the start time.
            let r = if pid == me {
                Err("Refusing to kill ogle itself".to_string())
            } else {
                sys.process_signal(pid, st, sig).map_err(|e| action_error("kill", pid, e))
            };
            match r {
                Ok(()) => sent += 1,
                Err(m) => {
                    first_err.get_or_insert(m);
                }
            }
        }
        if let Some(m) = first_err {
            return m;
        }
        match sel.targets.as_slice() {
            [(pid, _)] => format!("Sent {sname} to {} (pid {pid})", sel.name),
            _ => format!("Sent {sname} to {} ({sent} processes)", sel.name),
        }
    }
    /// `+` (delta 1) / `-` (delta -1). Returns the status-line message.
    pub fn renice(&mut self, sys: &mut Sys, ui: &UiState, delta: i32) -> String {
        let Some(sel) = self.selection(ui) else { return String::new() };
        let mut first_err: Option<String> = None;
        let me = std::process::id();
        for &(pid, st) in &sel.targets {
            // PROC-112 / PROC-113: each member from its own base, identity checked.
            if pid == me {
                first_err.get_or_insert_with(|| "Refusing to renice ogle itself".to_string());
            } else if let Err(e) = sys.process_set_priority(pid, st, delta) {
                first_err.get_or_insert_with(|| action_error("renice", pid, e));
            }
        }
        if let Some(m) = first_err {
            return m;
        }
        let what = if delta > 0 { "lowered" } else { "raised" };
        match sel.targets.as_slice() {
            [(pid, _)] => format!("Priority of {} (pid {pid}) {what}", sel.name),
            t => format!("Priority of {} ({} processes) {what}", sel.name, t.len()),
        }
    }
    /// F5 / Ctrl-R: drop cmdline and username caches (PROC-030).
    pub fn reset_caches(&mut self) {
        self.users.clear();
        for e in &mut self.slab {
            e.cmd = None;
        }
    }

    // ---- private ----

    fn effective(&self, alerts: &Alerts) -> SortKey {
        match self.sort {
            // PROC-062 / ALR-025.
            SortKey::Auto => match alerts.auto_sort() {
                AutoSort::Cpu => SortKey::Cpu,
                AutoSort::Mem => SortKey::Mem,
                AutoSort::Io => SortKey::Io,
            },
            k => k,
        }
    }

    /// Effective direction (PROC-060, PROC-064).
    pub(super) fn desc(&self) -> bool {
        let d = match self.eff {
            SortKey::Auto | SortKey::Cpu | SortKey::Mem | SortKey::Io | SortKey::Time => true,
            SortKey::Pid => self.programs,
            SortKey::Name | SortKey::User | SortKey::Core => false,
        };
        d != self.reverse
    }

    fn check_mode(&mut self, ui: &UiState) {
        if ui.programs != self.programs {
            // PROC-130: cursor resets, pin released.
            self.programs = ui.programs;
            self.reset_cursor();
            self.pin = None;
            // The old program view may index procs from an older sample; drop it until
            // `rebuild` regroups.
            self.prows.clear();
        }
        if ui.long_names != self.long_names {
            self.long_names = ui.long_names;
            self.arg_scroll = 0;
        }
    }

    fn reset_cursor(&mut self) {
        self.cursor = 0;
        self.scroll = 0;
        self.sel = None;
        self.psel = false;
        self.arg_scroll = 0;
    }

    fn string_needs(&self) -> (bool, bool) {
        let mut cmd = false;
        let mut user = self.eff == SortKey::User;
        for f in self.filter.iter().chain(self.focus.iter()) {
            match f.key {
                FKey::Any | FKey::Cmdline => cmd = true,
                FKey::User => user = true,
                _ => {}
            }
        }
        (cmd, user)
    }

    pub(super) fn cur_len(&self) -> usize {
        if self.programs {
            self.prows.len()
        } else {
            self.rows.len()
        }
    }

    /// First visible list position for a window of `n` rows, keeping the cursor visible.
    pub(super) fn window(&self, n: usize) -> usize {
        let len = self.cur_len();
        if n == 0 || len == 0 {
            return 0;
        }
        let mut s = self.scroll.min(len.saturating_sub(n));
        if self.cursor < s {
            s = self.cursor;
        } else if self.cursor >= s + n {
            s = self.cursor + 1 - n;
        }
        s
    }

    fn fix_scroll(&mut self) {
        self.scroll = self.window(self.page_rows.max(1) as usize);
    }

    /// Store the identity of the row under the cursor.
    fn take_sel(&mut self) {
        if self.programs {
            if let Some(&j) = self.prows.get(self.cursor) {
                self.sel_name.clear();
                self.sel_name.push_str(&self.progs[j as usize].name);
                self.psel = true;
            }
        } else if let Some(&i) = self.rows.get(self.cursor) {
            let p = &self.procs[i as usize];
            self.sel = Some((p.pid, p.start_time));
        }
    }

    /// PROC-090 / PROC-091: keep the selected identity, else the same index, clamped.
    fn follow(&mut self) {
        let len = self.cur_len();
        if len == 0 {
            self.cursor = 0;
            self.scroll = 0;
            self.sel = None;
            self.psel = false;
            return;
        }
        let pos = if self.programs {
            if self.psel {
                self.prows.iter().position(|&j| self.progs[j as usize].name == self.sel_name)
            } else {
                None
            }
        } else {
            self.sel.and_then(|k| {
                self.rows.iter().position(|&i| {
                    let p = &self.procs[i as usize];
                    (p.pid, p.start_time) == k
                })
            })
        };
        match pos {
            Some(p) => self.cursor = p,
            None => {
                self.cursor = self.cursor.min(len - 1);
                self.take_sel();
            }
        }
        self.fix_scroll();
    }

    /// Pids whose I/O is read this cycle (visible rows of the current view).
    fn fill_vis(&mut self, n: usize, want: bool) {
        self.vis.clear();
        if !want || self.der.len() != self.procs.len() {
            return;
        }
        let s = self.window(n);
        if self.programs {
            let end = (s + n).min(self.prows.len());
            for &j in &self.prows[s..end] {
                let g = &self.progs[j as usize];
                for &i in &self.grp[g.first as usize..(g.first + g.n) as usize] {
                    self.vis.push(self.procs[i as usize].pid);
                }
            }
        } else {
            let end = (s + n).min(self.rows.len());
            for &i in &self.rows[s..end] {
                self.vis.push(self.procs[i as usize].pid);
            }
        }
        // Sorted so `update` can binary-search it (a program window may hold most pids).
        self.vis.sort_unstable();
    }

    /// PROC-025: cmdline and username for the rows now visible.
    fn fetch_window(&mut self, sys: &mut Sys, n: usize) {
        let s = self.window(n);
        if self.programs {
            let end = (s + n).min(self.prows.len());
            for k in s..end {
                let g = &self.progs[self.prows[k] as usize];
                if let Agree::Same(Some(u)) = g.uid {
                    self.users.resolve(sys, u);
                }
                if self.long_names {
                    let i = g.top as usize;
                    let slot = self.der[i].slot as usize;
                    ensure_cmd(sys, &mut self.slab[slot], &self.procs[i]);
                }
            }
        } else {
            let end = (s + n).min(self.rows.len());
            for k in s..end {
                let i = self.rows[k] as usize;
                let slot = self.der[i].slot as usize;
                let p = &self.procs[i];
                ensure_cmd(sys, &mut self.slab[slot], p);
                if let Some(u) = p.uid {
                    self.users.resolve(sys, u);
                }
            }
        }
    }

    /// Counts, filter, grouping, sort, cursor, MAX and summary from the held sample.
    fn rebuild(&mut self, ui: &UiState) {
        self.check_mode(ui);
        let hide = ui.hide_kernel_threads;
        self.hide = hide;
        let n = self.procs.len().min(self.der.len());
        let (procs, der, slab, users) = (&self.procs, &self.der, &self.slab, &self.users);
        let (filter, focus) = (self.filter.as_ref(), &self.focus[..]);
        let programs = self.programs;
        let mut c = Counts::default();
        self.rows.clear();
        for i in 0..n {
            let p = &procs[i];
            if hide && p.is_kernel_thread {
                continue; // PROC-021
            }
            // PROC-032: counts before filter and focus.
            c.total += 1;
            c.thr += p.num_threads as u64;
            match p.status {
                b'R' => c.run += 1,
                b'S' | b'I' => c.slp += 1,
                _ => {}
            }
            if programs || proc_ok(filter, focus, p, &slab[der[i].slot as usize], users) {
                self.rows.push(i as u32);
            }
        }
        self.counts = c;
        if programs {
            self.build_programs();
        } else {
            self.sort_rows();
        }
        self.follow();
        self.finish();
    }

    fn sort_rows(&mut self) {
        let desc = self.desc();
        let eff = self.eff;
        let (procs, der, slab, users) = (&self.procs, &self.der, &self.slab, &self.users);
        self.skey.clear();
        if !matches!(eff, SortKey::Name | SortKey::User) {
            self.skey.extend(procs.iter().zip(der.iter()).map(|(p, d)| proc_skey(eff, p, d)));
        }
        let skey = &self.skey;
        self.rows.sort_unstable_by(|&a, &b| {
            let (pa, pb) = (&procs[a as usize], &procs[b as usize]);
            let o = match eff {
                SortKey::Name => {
                    let na = disp_name(pa, &slab[der[a as usize].slot as usize]);
                    let nb = disp_name(pb, &slab[der[b as usize].slot as usize]);
                    dir(ci_cmp(na, nb), desc)
                }
                SortKey::User => user_cmp(pa.uid, pb.uid, users, desc),
                _ => cmp_f(skey[a as usize], skey[b as usize], desc),
            };
            // PROC-061 tie-breaks: rss descending, then pid ascending.
            o.then(pb.rss.cmp(&pa.rss)).then(pa.pid.cmp(&pb.pid))
        });
    }

    /// PROC-131..133. `rows` holds the base list on entry.
    fn build_programs(&mut self) {
        let (procs, der, slab, users) = (&self.procs, &self.der, &self.slab, &self.users);
        self.grp.clear();
        self.grp.extend_from_slice(&self.rows);
        self.grp.sort_unstable_by(|&a, &b| procs[a as usize].name.cmp(&procs[b as usize].name));
        let grp = &self.grp;
        let mut np = 0;
        let mut k = 0;
        while k < grp.len() {
            let first = k;
            let name = &procs[grp[k] as usize].name;
            while k < grp.len() && procs[grp[k] as usize].name == *name {
                k += 1;
            }
            if np == self.progs.len() {
                self.progs.push(Prog::blank());
            }
            let g = &mut self.progs[np];
            np += 1;
            let p0 = &procs[grp[first] as usize];
            g.name.clear();
            g.name.push_str(name);
            g.first = first as u32;
            g.n = (k - first) as u32;
            g.cpu = f64::NAN;
            g.mem = f64::NAN;
            g.rss = 0;
            g.vms = None;
            g.thr = 0;
            g.time = 0.0;
            g.ior = f64::NAN;
            g.iow = f64::NAN;
            g.uid = Agree::Same(p0.uid);
            g.nice = Agree::Same(p0.nice);
            g.class = Agree::Same(p0.priority_class);
            g.status = p0.status;
            g.top = grp[first];
            let mut top_cpu = f64::NEG_INFINITY;
            for &i in &grp[first..k] {
                let (p, d) = (&procs[i as usize], &der[i as usize]);
                add_known(&mut g.cpu, d.cpu);
                add_known(&mut g.mem, d.mem);
                add_known(&mut g.ior, d.ior);
                add_known(&mut g.iow, d.iow);
                g.rss += p.rss;
                if let Some(v) = p.vms {
                    g.vms = Some(g.vms.unwrap_or(0) + v);
                }
                g.thr += p.num_threads;
                g.time += p.cpu_user + p.cpu_system;
                g.uid.merge(p.uid);
                g.nice.merge(p.nice);
                g.class.merge(p.priority_class);
                if p.status != g.status {
                    g.status = b'_';
                }
                if nan0(d.cpu) > top_cpu {
                    top_cpu = nan0(d.cpu);
                    g.top = i;
                }
            }
        }
        self.nprogs = np;

        // PROC-075: name matches the program name, other keys any member.
        let (filter, focus) = (self.filter.as_ref(), &self.focus[..]);
        let progs = &self.progs;
        let pmatch = |f: &Filter, g: &Prog| -> bool {
            let m = &grp[g.first as usize..(g.first + g.n) as usize];
            let any = |f: &Filter| m.iter().any(|&i| f.proc_match(&procs[i as usize], &slab[der[i as usize].slot as usize], users));
            match f.key {
                FKey::Name => f.re.is_match(&g.name),
                FKey::Any => {
                    f.re.is_match(&g.name)
                        || m.iter().any(|&i| f.re.is_match(slab[der[i as usize].slot as usize].cmd_str()))
                }
                _ => any(f),
            }
        };
        self.prows.clear();
        for j in 0..np {
            let g = &progs[j];
            if filter.map_or(true, |f| pmatch(f, g)) && (focus.is_empty() || focus.iter().any(|f| pmatch(f, g))) {
                self.prows.push(j as u32);
            }
        }
        let desc = self.desc();
        let eff = self.eff;
        self.prows.sort_unstable_by(|&a, &b| {
            let (x, y) = (&progs[a as usize], &progs[b as usize]);
            let o = match eff {
                SortKey::Name => dir(ci_cmp(&x.name, &y.name), desc),
                SortKey::User => {
                    let u = |g: &Prog| if let Agree::Same(u) = g.uid { u } else { None };
                    user_cmp(u(x), u(y), users, desc)
                }
                _ => cmp_f(prog_skey(eff, x), prog_skey(eff, y), desc),
            };
            o.then(y.rss.cmp(&x.rss)).then_with(|| x.name.cmp(&y.name))
        });
    }

    /// MAX positions (ALR-040) and the filter summary (PROC-080/081).
    fn finish(&mut self) {
        let mut best = (f64::NEG_INFINITY, usize::MAX, f64::NEG_INFINITY, usize::MAX);
        let active = self.filter.is_some() || !self.focus.is_empty();
        let mut sum = [f64::NAN; 6];
        let len = self.cur_len();
        for k in 0..len {
            let (cpu, mem, vms, rss, ior, iow) = if self.programs {
                let g = &self.progs[self.prows[k] as usize];
                (g.cpu, g.mem, g.vms, g.rss, g.ior, g.iow)
            } else {
                let i = self.rows[k] as usize;
                let (p, d) = (&self.procs[i], &self.der[i]);
                (d.cpu, d.mem, p.vms, p.rss, d.ior, d.iow)
            };
            if cpu > best.0 {
                best.0 = cpu;
                best.1 = k;
            }
            if mem > best.2 {
                best.2 = mem;
                best.3 = k;
            }
            if active {
                add_known(&mut sum[0], cpu);
                add_known(&mut sum[1], mem);
                add_known(&mut sum[2], vms.map_or(f64::NAN, |v| v as f64));
                add_known(&mut sum[3], rss as f64);
                add_known(&mut sum[4], ior);
                add_known(&mut sum[5], iow);
            }
        }
        self.max_cpu = if best.0 > 0.0 { best.1 } else { usize::MAX };
        self.max_mem = if best.2 > 0.0 { best.3 } else { usize::MAX };
        if !active {
            return;
        }
        let s = &mut self.summary;
        for c in 0..6 {
            let v = if sum[c] < 0.0 { f64::NAN } else { sum[c] };
            s.cur[c] = v;
            if !v.is_nan() {
                s.min[c] = if s.min[c].is_nan() { v } else { s.min[c].min(v) };
                s.max[c] = if s.max[c].is_nan() { v } else { s.max[c].max(v) };
            }
        }
    }

    /// PROC-121..123: extended fields for the pinned process only.
    fn update_pin(&mut self, sys: &mut Sys, ctx: &Ctx, idx: Option<usize>) {
        let Some(pin) = self.pin.as_mut() else { return };
        if pin.exited {
            return;
        }
        let Some(i) = idx else {
            pin.exited = true;
            pin.ext = None;
            pin.ctx_prev = None;
            pin.ctx_rate = None;
            return;
        };
        let (p, d) = (&self.procs[i], &self.der[i]);
        pin.cpu.push(d.cpu);
        pin.mem.push(d.mem);
        pin.threads = p.num_threads;
        pin.rss = p.rss;
        pin.vms = p.vms;
        let want_sock = pin.last_sock.map_or(true, |t| ctx.now.saturating_duration_since(t) >= SOCKET_PERIOD);
        pin.ext = sys.process_extended(pin.pid, pin.start, want_sock).map(|mut x| {
            if want_sock {
                pin.last_sock = Some(ctx.now);
                pin.sock = (x.tcp, x.udp);
            } else {
                x.tcp = pin.sock.0;
                x.udp = pin.sock.1;
            }
            x
        });
        let cs = pin.ext.as_ref().and_then(|x| Some((x.ctx_voluntary?, x.ctx_involuntary?)));
        pin.ctx_rate = None;
        if let Some((v, iv)) = cs {
            if let Some((pv, piv, t)) = pin.ctx_prev {
                let dt = ctx.now.saturating_duration_since(t).as_secs_f64();
                if dt > 0.0 && v >= pv && iv >= piv {
                    pin.ctx_rate = Some(((v - pv) as f64 / dt, (iv - piv) as f64 / dt));
                }
            }
            pin.ctx_prev = Some((v, iv, ctx.now));
        }
    }
}

impl TopProcesses for Processes {
    /// ALR-021: names from the unfiltered list, descending by `key`.
    fn top_names(&self, key: AutoSort, n: usize, out: &mut Vec<String>) {
        if !self.on || n == 0 {
            out.clear();
            return;
        }
        const CAP: usize = 16;
        let n = n.min(CAP);
        let mut top = [(0.0f64, 0u32); CAP];
        let mut cnt = 0;
        for (i, (p, d)) in self.procs.iter().zip(self.der.iter()).enumerate() {
            if self.hide && p.is_kernel_thread {
                continue;
            }
            let v = match key {
                AutoSort::Cpu => nan0(d.cpu),
                AutoSort::Mem => p.rss as f64,
                AutoSort::Io => io_val(d.ior, d.iow),
            };
            if v.is_nan() {
                continue;
            }
            let pos = top[..cnt].iter().position(|t| v > t.0).unwrap_or(cnt);
            if pos >= n {
                continue;
            }
            let last = cnt.min(n - 1);
            top.copy_within(pos..last, pos + 1);
            top[pos] = (v, i as u32);
            cnt = (cnt + 1).min(n);
        }
        // Reuse the caller's String slots; allocate only when the list grows.
        for (k, t) in top[..cnt].iter().enumerate() {
            let i = t.1 as usize;
            let name = disp_name(&self.procs[i], &self.slab[self.der[i].slot as usize]);
            if k < out.len() {
                out[k].clear();
                out[k].push_str(name);
            } else {
                out.push(name.to_string());
            }
        }
        out.truncate(cnt);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_format() {
        assert_eq!(time_plus(42.9).as_str(), "0:42");
        assert_eq!(time_plus(723.0).as_str(), "12:03");
        assert_eq!(time_plus(3.0 * 3600.0 + 7.0 * 60.0 + 5.0).as_str(), "3h7:05");
        assert_eq!(time_plus(100.0 * 3600.0 + 5.0).as_str(), "100h");
    }

    #[test]
    fn filter_parse() {
        let f = Filter::parse("name:^nginx$").unwrap();
        assert_eq!(f.key, FKey::Name);
        assert_eq!(f.pattern(), "^nginx$");
        let f = Filter::parse("^/usr/lib/x86_64:").unwrap();
        assert_eq!(f.key, FKey::Any);
        assert!(Filter::parse("(").is_err());
        assert!(Filter::parse("python").unwrap().re.is_match("Python3"));
        assert!(!Filter::parse("Python").unwrap().re.is_match("python3"));
        assert!(Filter::parse(r"\d").unwrap().re.is_match("A1"));
        assert!(smart_insensitive(r"a\Sb"));
    }

    #[test]
    fn argv_join() {
        let v = vec!["/bin/sh".to_string(), "-c".to_string(), "a\nb".to_string()];
        let (s, a0) = join_argv(&v);
        assert_eq!(s, "/bin/sh -c a b");
        assert_eq!(a0, 7);
        assert_eq!(join_argv(&[String::new()]).0, "");
    }

    #[test]
    fn pid_fmt() {
        let mut b = [0u8; 10];
        assert_eq!(fmt_u32(0, &mut b), "0");
        assert_eq!(fmt_u32(4294967295, &mut b), "4294967295");
    }
}
