//! Command line (spec 01 section 1). Small hand-written GNU-style parser, no dependencies.
//!
//! `parse` turns argv into either `Cmd::Run(Args)` or `Cmd::Exit`. `Exit.text` is the
//! complete message to print (no trailing newline): stdout for code 0, stderr for code 2.
//! `apply_overrides` then writes every CLI choice into the config tree (UI-002), so the
//! rest of the program reads settings from `Config` only.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::config::{Config, Value, SORT_KEYS};
use crate::modules::ModuleId;

#[derive(Clone, Debug, Default)]
pub struct Args {
    pub config: Option<PathBuf>,
    pub no_config: bool,
    pub stop_after: Option<u64>,
    pub log_file: Option<PathBuf>,
    /// Switch options in command-line order.
    sets: Vec<&'static [Set]>,
    refresh: Option<f64>,
    /// Bit per `ModuleId as u32`.
    disable: u32,
    enable: u32,
    sort: Option<&'static str>,
    filter: Option<String>,
    focus: Option<Vec<String>>,
    strftime: Option<String>,
    color: Option<&'static str>,
    diskio_mode: Option<&'static str>,
}

#[derive(Debug)]
pub enum Cmd {
    Run(Args),
    Exit { code: i32, text: String },
}

/// One config assignment made by a switch option.
#[derive(Debug)]
struct Set {
    path: &'static [&'static str],
    v: Cv,
}

#[derive(Clone, Copy, Debug)]
enum Cv {
    B(bool),
    S(&'static str),
}

impl Cv {
    fn value(self) -> Value {
        match self {
            Cv::B(b) => Value::Bool(b),
            Cv::S(s) => Value::Str(s.to_string()),
        }
    }
}

#[derive(Clone, Copy)]
enum Op {
    Help,
    Version,
    ListModules,
    PrintDefault,
    Config,
    NoConfig,
    Refresh,
    StopAfter,
    Disable,
    Enable,
    Sort,
    Filter,
    Focus,
    Strftime,
    Color,
    NoColor,
    DiskioIops,
    DiskioLatency,
    LogFile,
    Switch(&'static [Set]),
}

impl Op {
    fn takes_value(self) -> bool {
        matches!(
            self,
            Op::Config
                | Op::Refresh
                | Op::StopAfter
                | Op::Disable
                | Op::Enable
                | Op::Sort
                | Op::Filter
                | Op::Focus
                | Op::Strftime
                | Op::Color
                | Op::LogFile
        )
    }
}

struct Opt {
    long: &'static str,
    short: Option<char>,
    op: Op,
}

// Plain struct literals (no fn calls) so the nested slices are promoted to 'static.
macro_rules! o {
    ($long:literal, $short:expr, $op:expr) => {
        Opt { long: $long, short: $short, op: $op }
    };
}

macro_rules! sw {
    ($($path:expr => $v:expr),+ $(,)?) => {
        Op::Switch(&[$(Set { path: &$path, v: $v }),+])
    };
}

/// UI-006: the complete phase-1 option set (01 section 1.2).
#[rustfmt::skip]
static OPTS: &[Opt] = &[
    o!("help", Some('h'), Op::Help),
    o!("version", Some('V'), Op::Version),
    o!("config", Some('C'), Op::Config),
    o!("no-config", None, Op::NoConfig),
    o!("print-default-config", None, Op::PrintDefault),
    o!("refresh", Some('t'), Op::Refresh),
    o!("stop-after", None, Op::StopAfter),
    o!("disable", None, Op::Disable),
    o!("enable", None, Op::Enable),
    o!("list-modules", None, Op::ListModules),
    o!("light", None, sw!(
        ["ui", "left_sidebar"] => Cv::B(false),
        ["processcount", "enabled"] => Cv::B(false),
        ["processlist", "enabled"] => Cv::B(false),
        ["programlist", "enabled"] => Cv::B(false),
        ["alert", "enabled"] => Cv::B(false),
    )),
    o!("normalize-cpu", Some('0'), sw!(["ui", "normalize_cpu"] => Cv::B(true))),
    o!("percpu", Some('1'), sw!(["ui", "percpu"] => Cv::B(true))),
    o!("no-left-sidebar", Some('2'), sw!(["ui", "left_sidebar"] => Cv::B(false))),
    o!("no-quicklook", Some('3'), sw!(["ui", "quicklook"] => Cv::B(false))),
    o!("full-quicklook", Some('4'), sw!(["ui", "full_quicklook"] => Cv::B(true))),
    o!("no-top", Some('5'), sw!(["ui", "top"] => Cv::B(false))),
    o!("sparkline", None, sw!(["ui", "sparkline"] => Cv::B(true))),
    o!("no-history", None, sw!(["global", "history"] => Cv::B(false))),
    o!("sort", None, Op::Sort),
    o!("programs", None, sw!(["processlist", "programs"] => Cv::B(true))),
    o!("filter", Some('f'), Op::Filter),
    o!("focus", None, Op::Focus),
    o!("long-names", None, sw!(["processlist", "short_name"] => Cv::B(false))),
    o!("hide-kernel-threads", None, sw!(["processlist", "hide_kernel_threads"] => Cv::B(true))),
    o!("no-processes", None, sw!(["processlist", "collect"] => Cv::B(false))), // PROC-029
    o!("arrow-keys-sort", None, sw!(["ui", "arrow_keys_sort"] => Cv::B(true))),
    o!("byte", Some('b'), sw!(["network", "bytes"] => Cv::B(true))),
    o!("diskio-iops", None, Op::DiskioIops),
    o!("diskio-latency", None, Op::DiskioLatency),
    o!("diskio-show-ramfs", None, sw!(["diskio", "show_ramfs"] => Cv::B(true))),
    o!("fs-free-space", None, sw!(["fs", "free_space"] => Cv::B(true))),
    o!("fahrenheit", None, sw!(["sensors", "fahrenheit"] => Cv::B(true))),
    o!("strftime", None, Op::Strftime),
    o!("color", None, Op::Color),
    o!("no-color", None, Op::NoColor),
    o!("no-bold", None, sw!(["ui", "bold"] => Cv::B(false))),
    o!("no-bg", None, sw!(["ui", "background"] => Cv::B(false))),
    o!("ascii", None, sw!(["ui", "unicode"] => Cv::S("never"))),
    o!("no-separator", None, sw!(["ui", "separator"] => Cv::B(false))),
    o!("log-file", None, Op::LogFile),
];

const HELP: &str = "\
Usage: ogle [OPTION]...
Resource-efficient terminal system monitor.

  -h, --help                  print this help and exit
  -V, --version               print the version and exit
  -C, --config PATH           use this config file
      --no-config             ignore every config file
      --print-default-config  print the default config and exit
      --list-modules          print the module names and exit
  -t, --refresh SECONDS       refresh interval, 0.1 to 3600 (default 2)
      --stop-after N          exit after N cycles
      --disable MODULES       disable modules (comma-separated list, or 'all')
      --enable MODULES        enable modules (applied after --disable)
      --light                 hide the left sidebar, process panels and alerts
  -0, --normalize-cpu         start with per-core CPU normalisation
  -1, --percpu                start in per-CPU mode
  -2, --no-left-sidebar       start with the left sidebar hidden
  -3, --no-quicklook          start with quicklook hidden
  -4, --full-quicklook        start in full-quicklook mode
  -5, --no-top                start with the top row hidden
      --sparkline             draw sparklines instead of bars in quicklook
      --no-history            keep no history (no trend arrows or sparklines)
      --sort KEY              process sort: auto cpu mem io time name user core pid
      --programs              start in program (grouped) view
  -f, --filter PATTERN        initial process filter
      --focus PATTERNS        show only processes matching any pattern
      --long-names            show the full command path
      --hide-kernel-threads   hide kernel threads
      --no-processes          start with process collection off
      --arrow-keys-sort       swap plain and shifted Left/Right
  -b, --byte                  network rates in bytes instead of bits
      --diskio-iops           disk I/O starts in IOPS mode
      --diskio-latency        disk I/O starts in latency mode
      --diskio-show-ramfs     show ram* block devices
      --fs-free-space         show free instead of used file system space
      --fahrenheit            temperatures in degrees Fahrenheit
      --strftime FORMAT       clock format
      --color WHEN            colour mode: auto, always, never
      --no-color              same as --color=never
      --no-bold               never use bold
      --no-bg                 never use background colours
      --ascii                 use ASCII glyphs only
      --no-separator          do not draw separator lines
      --log-file PATH         write a debug log to PATH

Short options without values may be grouped, e.g. -13.";

/// Parse argv (without argv[0]). Usage errors become `Exit { code: 2 }` (UI-003).
pub fn parse<I: Iterator<Item = OsString>>(args: I) -> Cmd {
    match parse_inner(args) {
        Ok(cmd) => cmd,
        Err(msg) => Cmd::Exit {
            code: 2,
            text: format!("ogle: {msg}\nTry 'ogle --help' for more information."),
        },
    }
}

fn parse_inner<I: Iterator<Item = OsString>>(mut it: I) -> Result<Cmd, String> {
    let mut a = Args::default();
    let mut after_dashes = false;
    while let Some(os) = it.next() {
        let Some(arg) = os.to_str() else {
            return Err(format!("invalid argument '{}'", os.to_string_lossy()));
        };
        // ogle takes no operands.
        if after_dashes || arg == "-" || !arg.starts_with('-') {
            return Err(format!("unexpected argument '{arg}'"));
        }
        if arg == "--" {
            after_dashes = true;
            continue;
        }
        if let Some(body) = arg.strip_prefix("--") {
            let (name, inline) = match body.split_once('=') {
                Some((n, v)) => (n, Some(v)),
                None => (body, None),
            };
            let Some(opt) = OPTS.iter().find(|o| o.long == name) else {
                return Err(format!("unrecognized option '--{name}'"));
            };
            let val = if opt.op.takes_value() {
                match inline {
                    Some(v) => Some(OsString::from(v)),
                    None => match it.next() {
                        Some(v) => Some(v),
                        None => return Err(format!("option '--{name}' requires an argument")),
                    },
                }
            } else if inline.is_some() {
                return Err(format!("option '--{name}' doesn't allow an argument"));
            } else {
                None
            };
            if let Some(cmd) = apply(&mut a, opt, val)? {
                return Ok(cmd);
            }
        } else {
            // UI-001: grouped short options; a value option takes the rest of the group
            // or the next argument (`-t1`, `-t 1`, `-1t1`).
            let group = &arg[1..];
            for (pos, ch) in group.char_indices() {
                let Some(opt) = OPTS.iter().find(|o| o.short == Some(ch)) else {
                    return Err(format!("invalid option -- '{ch}'"));
                };
                if opt.op.takes_value() {
                    let rest = &group[pos + ch.len_utf8()..];
                    let val = if !rest.is_empty() {
                        OsString::from(rest)
                    } else {
                        match it.next() {
                            Some(v) => v,
                            None => return Err(format!("option requires an argument -- '{ch}'")),
                        }
                    };
                    if let Some(cmd) = apply(&mut a, opt, Some(val))? {
                        return Ok(cmd);
                    }
                    break;
                }
                if let Some(cmd) = apply(&mut a, opt, None)? {
                    return Ok(cmd);
                }
            }
        }
    }
    Ok(Cmd::Run(a))
}

fn utf8(os: &OsString, name: &str) -> Result<String, String> {
    os.to_str()
        .map(str::to_string)
        .ok_or_else(|| format!("invalid UTF-8 in argument of '--{name}'"))
}

fn exit0(text: String) -> Result<Option<Cmd>, String> {
    Ok(Some(Cmd::Exit { code: 0, text }))
}

fn apply(a: &mut Args, opt: &Opt, val: Option<OsString>) -> Result<Option<Cmd>, String> {
    let name = opt.long;
    // Every value option receives `Some`; non-UTF-8 is accepted only for paths.
    let os = val.unwrap_or_default();
    let text = || utf8(&os, name);
    match opt.op {
        Op::Help => return exit0(HELP.to_string()),
        Op::Version => return exit0(format!("ogle {}", env!("CARGO_PKG_VERSION"))),
        Op::ListModules => {
            let names: Vec<&str> = ModuleId::ALL.iter().map(|m| m.name()).collect();
            return exit0(names.join("\n"));
        }
        Op::PrintDefault => return exit0(Config::default_toml().trim_end().to_string()),
        Op::Config => a.config = Some(PathBuf::from(os.clone())),
        Op::NoConfig => a.no_config = true,
        Op::LogFile => a.log_file = Some(PathBuf::from(os.clone())),
        Op::Refresh => {
            let s = text()?;
            match s.trim().parse::<f64>() {
                Ok(r) if (0.1..=3600.0).contains(&r) => a.refresh = Some(r),
                _ => return Err(format!("invalid refresh interval '{s}' (0.1 to 3600 seconds)")),
            }
        }
        Op::StopAfter => {
            let s = text()?;
            match s.trim().parse::<u64>() {
                Ok(n) if n >= 1 => a.stop_after = Some(n),
                _ => return Err(format!("invalid value '{s}' for '--stop-after' (integer >= 1)")),
            }
        }
        Op::Disable => a.disable |= module_mask(&text()?, true)?,
        Op::Enable => a.enable |= module_mask(&text()?, false)?,
        Op::Sort => {
            let s = text()?;
            let key = s.trim().to_ascii_lowercase();
            match SORT_KEYS.iter().copied().find(|k| *k == key) {
                Some(k) => a.sort = Some(k),
                None => {
                    return Err(format!("invalid sort key '{s}' (valid: {})", SORT_KEYS.join(" ")))
                }
            }
        }
        Op::Filter => a.filter = Some(text()?),
        Op::Focus => {
            // PROC-074; UI-005 spaces around commas.
            let s = text()?;
            let list = a.focus.get_or_insert_with(Vec::new);
            list.extend(s.split(',').map(str::trim).filter(|p| !p.is_empty()).map(str::to_string));
        }
        Op::Strftime => a.strftime = Some(text()?),
        Op::Color => {
            let s = text()?;
            let v = s.trim().to_ascii_lowercase();
            match ["auto", "always", "never"].into_iter().find(|c| *c == v) {
                Some(c) => a.color = Some(c),
                None => return Err(format!("invalid argument '{s}' for '--color' (auto, always, never)")),
            }
        }
        Op::NoColor => a.color = Some("never"),
        Op::DiskioIops | Op::DiskioLatency => {
            // IO-049: the two modes exclude each other.
            let mode = if matches!(opt.op, Op::DiskioIops) { "iops" } else { "latency" };
            if a.diskio_mode.is_some_and(|m| m != mode) {
                return Err("options '--diskio-iops' and '--diskio-latency' exclude each other".into());
            }
            a.diskio_mode = Some(mode);
        }
        Op::Switch(sets) => a.sets.push(sets),
    }
    Ok(None)
}

/// UI-005, UI-008: comma list of module names, case-insensitive; `all` only for --disable.
fn module_mask(list: &str, allow_all: bool) -> Result<u32, String> {
    let mut mask = 0u32;
    for tok in list.split(',').map(str::trim) {
        let lower = tok.to_ascii_lowercase();
        if allow_all && lower == "all" {
            mask |= (1u32 << ModuleId::COUNT) - 1;
            continue;
        }
        match ModuleId::from_name(&lower) {
            Some(m) => mask |= 1u32 << (m as u32),
            None if tok.is_empty() => return Err(format!("empty module name in '{list}'")),
            None => return Err(format!("unknown module '{tok}'")),
        }
    }
    Ok(mask)
}

/// Write every CLI choice into the config tree (UI-002: command line beats the file).
pub fn apply_overrides(args: &Args, cfg: &mut Config) {
    for sets in &args.sets {
        for s in sets.iter() {
            cfg.set(s.path, s.v.value());
        }
    }
    if let Some(r) = args.refresh {
        cfg.set(&["global", "refresh"], Value::Float(r));
    }
    // UI-015: --enable is applied after --disable.
    for (mask, on) in [(args.disable, false), (args.enable, true)] {
        for m in ModuleId::ALL {
            if mask & (1u32 << (m as u32)) != 0 {
                cfg.set(&[m.name(), "enabled"], Value::Bool(on));
            }
        }
    }
    if let Some(k) = args.sort {
        cfg.set(&["processlist", "sort_key"], Value::Str(k.to_string())); // PROC-066
    }
    if let Some(f) = &args.filter {
        cfg.set(&["processlist", "filter"], Value::Str(f.clone())); // PROC-073
    }
    if let Some(list) = &args.focus {
        let arr = list.iter().map(|p| Value::Str(p.clone())).collect();
        cfg.set(&["processlist", "focus"], Value::Array(arr));
    }
    if let Some(f) = &args.strftime {
        cfg.set(&["now", "format"], Value::Str(f.clone())); // SYS-041
    }
    if let Some(c) = args.color {
        cfg.set(&["ui", "color"], Value::Str(c.to_string()));
    }
    if let Some(m) = args.diskio_mode {
        cfg.set(&["diskio", "mode"], Value::Str(m.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: &[&str]) -> Cmd {
        parse(v.iter().map(|s| OsString::from(*s)))
    }

    fn run(v: &[&str]) -> Config {
        match p(v) {
            Cmd::Run(a) => {
                let mut c = Config::default();
                apply_overrides(&a, &mut c);
                c
            }
            Cmd::Exit { text, .. } => panic!("unexpected exit: {text}"),
        }
    }

    fn usage_err(v: &[&str]) -> String {
        match p(v) {
            Cmd::Exit { code: 2, text } => text,
            other => panic!("expected usage error, got {other:?}"),
        }
    }

    #[test]
    fn refresh_forms() {
        for v in [&["-t", "1"][..], &["-t1"][..], &["--refresh=1"][..], &["--refresh", "1"][..]] {
            assert_eq!(run(v).f64(&["global", "refresh"]), Some(1.0));
        }
        assert!(usage_err(&["-t", "0.01"]).contains("refresh"));
        assert!(usage_err(&["--refresh"]).contains("requires an argument"));
    }

    #[test]
    fn grouped_short_flags() {
        let c = run(&["-13", "-b0t5"]);
        assert_eq!(c.bool(&["ui", "percpu"]), Some(true));
        assert_eq!(c.bool(&["ui", "quicklook"]), Some(false));
        assert_eq!(c.bool(&["network", "bytes"]), Some(true));
        assert_eq!(c.bool(&["ui", "normalize_cpu"]), Some(true));
        assert_eq!(c.f64(&["global", "refresh"]), Some(5.0));
    }

    #[test]
    fn modules_enable_disable() {
        let c = run(&["--disable", "all", "--enable", "CPU, mem"]);
        assert_eq!(c.bool(&["cpu", "enabled"]), Some(true));
        assert_eq!(c.bool(&["mem", "enabled"]), Some(true));
        assert_eq!(c.bool(&["alert", "enabled"]), Some(false));
        assert!(usage_err(&["--disable=cpu,bogus"]).contains("'bogus'"));
        assert!(usage_err(&["--enable", "all"]).contains("'all'"));
    }

    #[test]
    fn value_options() {
        let c = run(&["--sort", "MEM", "-f", "^ssh", "--focus", "a , b", "--strftime=%H", "--no-color"]);
        assert_eq!(c.str(&["processlist", "sort_key"]), Some("mem"));
        assert_eq!(c.str(&["processlist", "filter"]), Some("^ssh"));
        assert_eq!(c.str_list(&["processlist", "focus"]), Some(vec!["a".into(), "b".into()]));
        assert_eq!(c.str(&["now", "format"]), Some("%H"));
        assert_eq!(c.str(&["ui", "color"]), Some("never"));
        assert!(usage_err(&["--sort", "rss"]).contains("sort key"));
    }

    #[test]
    fn light_and_switches() {
        let c = run(&["--light", "--ascii", "--fahrenheit", "--long-names", "--no-processes"]);
        assert_eq!(c.bool(&["ui", "left_sidebar"]), Some(false));
        assert_eq!(c.bool(&["processlist", "enabled"]), Some(false));
        assert_eq!(c.bool(&["alert", "enabled"]), Some(false));
        assert_eq!(c.str(&["ui", "unicode"]), Some("never"));
        assert_eq!(c.bool(&["sensors", "fahrenheit"]), Some(true));
        assert_eq!(c.bool(&["processlist", "short_name"]), Some(false));
        assert_eq!(c.bool(&["processlist", "collect"]), Some(false));
        assert!(c.unknown_key_warnings().is_empty());
    }

    #[test]
    fn args_fields() {
        match p(&["-C", "/x.toml", "--no-config", "--stop-after=3", "--log-file", "/l"]) {
            Cmd::Run(a) => {
                assert_eq!(a.config, Some(PathBuf::from("/x.toml")));
                assert!(a.no_config);
                assert_eq!(a.stop_after, Some(3));
                assert_eq!(a.log_file, Some(PathBuf::from("/l")));
            }
            other => panic!("{other:?}"),
        }
        assert!(usage_err(&["--stop-after", "0"]).contains("stop-after"));
    }

    #[test]
    fn exits_and_errors() {
        assert!(matches!(p(&["--help"]), Cmd::Exit { code: 0, .. }));
        match p(&["-V"]) {
            Cmd::Exit { code: 0, text } => assert!(text.starts_with("ogle ")),
            other => panic!("{other:?}"),
        }
        match p(&["--list-modules"]) {
            Cmd::Exit { code: 0, text } => assert_eq!(text.lines().count(), ModuleId::COUNT),
            other => panic!("{other:?}"),
        }
        let e = usage_err(&["--bogus"]);
        assert!(e.starts_with("ogle: unrecognized option '--bogus'\nTry 'ogle --help'"));
        assert!(usage_err(&["-x"]).contains("invalid option -- 'x'"));
        assert!(usage_err(&["stray"]).contains("unexpected argument"));
        assert!(usage_err(&["--percpu=1"]).contains("doesn't allow"));
        assert!(usage_err(&["--diskio-iops", "--diskio-latency"]).contains("exclude"));
        assert!(usage_err(&["--color", "rainbow"]).contains("--color"));
    }
}
