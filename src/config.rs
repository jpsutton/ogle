//! Configuration (spec 01 section 2, GEN-012). TOML subset parser, no dependencies.
//!
//! The public API below is used by every module.
//!
//! Supported TOML subset: comments, `[table]` and `[a.b."quoted"]` headers, bare and
//! quoted keys, dotted keys, strings (basic + literal, common escapes), integers, floats,
//! booleans, arrays (single or multi-line, trailing comma), inline tables. No dates,
//! no multi-line strings, no arrays of tables. Errors carry line numbers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::modules::ModuleId;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Array(Vec<Value>),
    Table(Table),
}

pub type Table = BTreeMap<String, Value>;

#[derive(Debug)]
pub struct ConfigError {
    pub path: Option<PathBuf>,
    pub line: usize,
    pub msg: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.path {
            Some(p) => write!(f, "{}:{}: {}", p.display(), self.line, self.msg),
            None => write!(f, "line {}: {}", self.line, self.msg),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub root: Table,
    /// File actually loaded, if any.
    pub source: Option<PathBuf>,
}

impl Config {
    /// Parse TOML text.
    pub fn parse(text: &str) -> Result<Config, ConfigError> {
        let mut p = Parser { s: text.as_bytes(), src: text, i: 0, line: 1 };
        if text.starts_with('\u{feff}') {
            p.i = 3;
        }
        let mut root = Table::new();
        let mut cur: Vec<String> = Vec::new();
        let mut headers: Vec<Vec<String>> = Vec::new();
        loop {
            p.skip_ws_nl();
            let Some(c) = p.peek() else { break };
            let line = p.line;
            if c == b'[' {
                p.i += 1;
                if p.peek() == Some(b'[') {
                    return p.err("arrays of tables are not supported");
                }
                p.skip_ws();
                let path = p.key_path()?;
                p.skip_ws();
                if p.peek() != Some(b']') {
                    return p.err("expected ']' after table name");
                }
                p.i += 1;
                p.end_line()?;
                if headers.contains(&path) {
                    return Err(err_at(line, format!("table [{}] defined twice", path.join("."))));
                }
                validate(&path, &Value::Table(Table::new())).map_err(|m| err_at(line, m))?;
                table_at(&mut root, &path).map_err(|m| err_at(line, m))?;
                headers.push(path.clone());
                cur = path;
            } else {
                let key = p.key_path()?;
                p.skip_ws();
                if p.peek() != Some(b'=') {
                    return p.err("expected '=' after key");
                }
                p.i += 1;
                p.skip_ws();
                let v = p.value()?;
                p.end_line()?;
                let mut full = cur.clone();
                full.extend(key);
                validate(&full, &v).map_err(|m| err_at(line, m))?;
                insert(&mut root, &full, v).map_err(|m| err_at(line, m))?;
            }
        }
        Ok(Config { root, source: None })
    }

    /// GEN-012 search order: explicit path (error if missing), else
    /// `$XDG_CONFIG_HOME/ogle/config.toml` / `~/.config/ogle/config.toml` (Unix, macOS),
    /// `%APPDATA%\ogle\config.toml` (Windows). Missing default file -> empty Config.
    pub fn load(explicit: Option<&Path>) -> Result<Config, ConfigError> {
        let (path, required) = match explicit {
            Some(p) => (p.to_path_buf(), true),
            None => match default_path() {
                Some(p) => (p, false),
                None => return Ok(Config::default()),
            },
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if !required && e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Config::default())
            }
            Err(e) => {
                let msg = format!("cannot read config file: {e}");
                return Err(ConfigError { path: Some(path), line: 0, msg });
            }
        };
        let text = match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(_) => {
                let msg = "config file is not valid UTF-8".to_string();
                return Err(ConfigError { path: Some(path), line: 0, msg });
            }
        };
        match Config::parse(&text) {
            Ok(mut cfg) => {
                cfg.source = Some(path);
                Ok(cfg)
            }
            Err(mut e) => {
                e.path = Some(path);
                Err(e)
            }
        }
    }

    /// Look up a dotted path of already-split segments, e.g. `&["cpu", "thresholds", "total"]`.
    pub fn get(&self, path: &[&str]) -> Option<&Value> {
        let mut cur = self.root.get(*path.first()?)?;
        for seg in &path[1..] {
            match cur {
                Value::Table(t) => cur = t.get(*seg)?,
                _ => return None,
            }
        }
        Some(cur)
    }

    pub fn table(&self, path: &[&str]) -> Option<&Table> {
        match self.get(path)? {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }

    pub fn bool(&self, path: &[&str]) -> Option<bool> {
        match self.get(path)? {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Int or Float as f64.
    pub fn f64(&self, path: &[&str]) -> Option<f64> {
        match self.get(path)? {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn i64(&self, path: &[&str]) -> Option<i64> {
        match self.get(path)? {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn str(&self, path: &[&str]) -> Option<&str> {
        match self.get(path)? {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Array of strings; non-string elements are skipped.
    pub fn str_list(&self, path: &[&str]) -> Option<Vec<String>> {
        match self.get(path)? {
            Value::Array(a) => Some(
                a.iter()
                    .filter_map(|v| if let Value::Str(s) = v { Some(s.clone()) } else { None })
                    .collect(),
            ),
            _ => None,
        }
    }

    /// Set a value (used by CLI overrides). Creates intermediate tables.
    pub fn set(&mut self, path: &[&str], v: Value) {
        let Some((last, parents)) = path.split_last() else { return };
        let mut t = &mut self.root;
        for seg in parents {
            let e = t.entry((*seg).to_string()).or_insert_with(|| Value::Table(Table::new()));
            if !matches!(e, Value::Table(_)) {
                *e = Value::Table(Table::new());
            }
            t = match e {
                Value::Table(x) => x,
                _ => unreachable!(),
            };
        }
        t.insert((*last).to_string(), v);
    }

    /// Validate known keys (UI-01x). Returns warnings for unknown keys/tables;
    /// the caller prints them to stderr before entering the UI.
    pub fn unknown_key_warnings(&self) -> Vec<String> {
        let at = match &self.source {
            Some(p) => format!("{}: ", p.display()),
            None => String::new(),
        };
        let mut w = Vec::new();
        for (sec, v) in &self.root {
            match v {
                Value::Table(t) if is_section(sec) => {
                    for key in t.keys() {
                        if kind_of(sec, key).is_none() {
                            w.push(format!("ogle: {at}warning: unknown key '{sec}.{key}' ignored"));
                        }
                    }
                }
                Value::Table(_) => w.push(format!("ogle: {at}warning: unknown table [{sec}] ignored")),
                _ => w.push(format!("ogle: {at}warning: unknown key '{sec}' ignored")),
            }
        }
        w
    }

    /// Full default configuration as commented TOML (`--print-default-config`).
    pub fn default_toml() -> &'static str {
        DEFAULT_TOML
    }
}
fn err_at(line: usize, msg: String) -> ConfigError {
    ConfigError { path: None, line, msg }
}

/// Byte-level recursive-descent parser for the TOML subset. Runs once at start-up.
struct Parser<'a> {
    s: &'a [u8],
    src: &'a str,
    i: usize,
    line: usize,
}

impl Parser<'_> {
    fn err<T>(&self, msg: &str) -> Result<T, ConfigError> {
        Err(err_at(self.line, msg.to_string()))
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn rest_starts_with(&self, pat: &[u8]) -> bool {
        self.s[self.i..].starts_with(pat)
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t')) {
            self.i += 1;
        }
    }

    fn skip_comment(&mut self) {
        while !matches!(self.peek(), None | Some(b'\n')) {
            self.i += 1;
        }
    }

    /// Skip blanks, comments and newlines (between lines, inside arrays and inline tables).
    fn skip_ws_nl(&mut self) {
        loop {
            match self.peek() {
                Some(b' ' | b'\t' | b'\r') => self.i += 1,
                Some(b'\n') => {
                    self.i += 1;
                    self.line += 1;
                }
                Some(b'#') => self.skip_comment(),
                _ => break,
            }
        }
    }

    /// After a header or a key/value pair only blanks and a comment may follow.
    fn end_line(&mut self) -> Result<(), ConfigError> {
        self.skip_ws();
        if self.peek() == Some(b'#') {
            self.skip_comment();
        }
        if self.peek() == Some(b'\r') {
            self.i += 1;
        }
        match self.peek() {
            None => Ok(()),
            Some(b'\n') => {
                self.i += 1;
                self.line += 1;
                Ok(())
            }
            Some(_) => self.err("unexpected text after value"),
        }
    }

    fn key_path(&mut self) -> Result<Vec<String>, ConfigError> {
        let mut path = Vec::new();
        loop {
            path.push(self.key()?);
            self.skip_ws();
            if self.peek() != Some(b'.') {
                return Ok(path);
            }
            self.i += 1;
            self.skip_ws();
        }
    }

    fn key(&mut self) -> Result<String, ConfigError> {
        match self.peek() {
            Some(b'"') => self.basic_string(),
            Some(b'\'') => self.literal_string(),
            _ => {
                let start = self.i;
                while matches!(self.peek(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-')) {
                    self.i += 1;
                }
                if start == self.i {
                    return self.err("expected a key");
                }
                Ok(self.src[start..self.i].to_string())
            }
        }
    }

    fn basic_string(&mut self) -> Result<String, ConfigError> {
        self.i += 1;
        if self.rest_starts_with(b"\"\"") {
            return self.err("multi-line strings are not supported");
        }
        let mut out = String::new();
        loop {
            // Stops only at ASCII bytes, so the slice is always on char boundaries.
            let start = self.i;
            while !matches!(self.peek(), None | Some(b'"' | b'\\' | b'\n')) {
                self.i += 1;
            }
            out.push_str(&self.src[start..self.i]);
            match self.peek() {
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    let c = self.s.get(self.i + 1).copied();
                    self.i += 2;
                    match c {
                        Some(b'n') => out.push('\n'),
                        Some(b't') => out.push('\t'),
                        Some(b'r') => out.push('\r'),
                        Some(b'b') => out.push('\u{8}'),
                        Some(b'f') => out.push('\u{c}'),
                        Some(b'e') => out.push('\u{1b}'),
                        Some(b'"') => out.push('"'),
                        Some(b'\\') => out.push('\\'),
                        Some(b'u') => out.push(self.unicode_escape(4)?),
                        Some(b'U') => out.push(self.unicode_escape(8)?),
                        _ => return self.err("invalid escape sequence in string"),
                    }
                }
                _ => return self.err("unterminated string"),
            }
        }
    }

    fn unicode_escape(&mut self, n: usize) -> Result<char, ConfigError> {
        let hex = self.src.get(self.i..self.i + n).unwrap_or("");
        if hex.len() != n || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return self.err("invalid unicode escape in string");
        }
        self.i += n;
        match u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
            Some(c) => Ok(c),
            None => self.err("invalid unicode escape in string"),
        }
    }

    fn literal_string(&mut self) -> Result<String, ConfigError> {
        self.i += 1;
        if self.rest_starts_with(b"''") {
            return self.err("multi-line strings are not supported");
        }
        let start = self.i;
        while !matches!(self.peek(), None | Some(b'\'' | b'\n')) {
            self.i += 1;
        }
        if self.peek() != Some(b'\'') {
            return self.err("unterminated string");
        }
        self.i += 1;
        Ok(self.src[start..self.i - 1].to_string())
    }

    fn value(&mut self) -> Result<Value, ConfigError> {
        match self.peek() {
            Some(b'"') => self.basic_string().map(Value::Str),
            Some(b'\'') => self.literal_string().map(Value::Str),
            Some(b'[') => self.array(),
            Some(b'{') => self.inline_table(),
            Some(b't') if self.rest_starts_with(b"true") => {
                self.i += 4;
                Ok(Value::Bool(true))
            }
            Some(b'f') if self.rest_starts_with(b"false") => {
                self.i += 5;
                Ok(Value::Bool(false))
            }
            Some(_) => self.number(),
            None => self.err("expected a value"),
        }
    }

    fn number(&mut self) -> Result<Value, ConfigError> {
        let start = self.i;
        while matches!(self.peek(), Some(b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z' | b'_' | b'+' | b'-' | b'.')) {
            self.i += 1;
        }
        let tok = &self.src[start..self.i];
        let b = tok.as_bytes();
        let digit_at = |k: usize| b.get(k).is_some_and(|d| d.is_ascii_hexdigit());
        let underscores_ok =
            (0..b.len()).all(|k| b[k] != b'_' || (k > 0 && digit_at(k - 1) && digit_at(k + 1)));
        let bad = || err_at(self.line, format!("invalid value '{tok}' (strings must be quoted)"));
        if tok.is_empty() || !underscores_ok {
            return Err(bad());
        }
        let clean: String = tok.chars().filter(|&c| c != '_').collect();
        let (sign, body) = match clean.as_bytes()[0] {
            b'-' => (-1.0, &clean[1..]),
            b'+' => (1.0, &clean[1..]),
            _ => (1.0, clean.as_str()),
        };
        match body {
            "inf" => return Ok(Value::Float(sign * f64::INFINITY)),
            "nan" => return Ok(Value::Float(f64::NAN)),
            _ => {}
        }
        for (prefix, radix) in [("0x", 16), ("0o", 8), ("0b", 2)] {
            if let Some(digits) = clean.strip_prefix(prefix) {
                let ok = !digits.is_empty() && digits.bytes().all(|d| d.is_ascii_hexdigit());
                return match i64::from_str_radix(digits, radix) {
                    Ok(v) if ok => Ok(Value::Int(v)),
                    _ => Err(bad()),
                };
            }
        }
        if !body.bytes().all(|d| matches!(d, b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')) {
            return Err(bad());
        }
        if body.bytes().any(|d| matches!(d, b'.' | b'e' | b'E')) {
            clean.parse::<f64>().map(Value::Float).map_err(|_| bad())
        } else {
            clean.parse::<i64>().map(Value::Int).map_err(|_| bad())
        }
    }

    fn array(&mut self) -> Result<Value, ConfigError> {
        self.i += 1;
        let mut items = Vec::new();
        loop {
            self.skip_ws_nl();
            if self.peek() == Some(b']') {
                self.i += 1;
                return Ok(Value::Array(items));
            }
            items.push(self.value()?);
            self.skip_ws_nl();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {
                    self.i += 1;
                    return Ok(Value::Array(items));
                }
                _ => return self.err("expected ',' or ']' in array"),
            }
        }
    }

    fn inline_table(&mut self) -> Result<Value, ConfigError> {
        self.i += 1;
        let mut t = Table::new();
        self.skip_ws_nl();
        if self.peek() == Some(b'}') {
            self.i += 1;
            return Ok(Value::Table(t));
        }
        loop {
            self.skip_ws_nl();
            let key = self.key_path()?;
            self.skip_ws();
            if self.peek() != Some(b'=') {
                return self.err("expected '=' after key");
            }
            self.i += 1;
            self.skip_ws();
            let v = self.value()?;
            let line = self.line;
            insert(&mut t, &key, v).map_err(|m| err_at(line, m))?;
            self.skip_ws_nl();
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b'}') => {
                    self.i += 1;
                    return Ok(Value::Table(t));
                }
                _ => return self.err("expected ',' or '}' in inline table"),
            }
        }
    }
}

/// Walk to (creating as needed) the table at `path`.
fn table_at<'t>(mut t: &'t mut Table, path: &[String]) -> Result<&'t mut Table, String> {
    for (n, seg) in path.iter().enumerate() {
        t = match t.entry(seg.clone()).or_insert_with(|| Value::Table(Table::new())) {
            Value::Table(x) => x,
            _ => return Err(format!("'{}' is not a table", path[..=n].join("."))),
        };
    }
    Ok(t)
}

fn insert(root: &mut Table, path: &[String], v: Value) -> Result<(), String> {
    let Some((last, parents)) = path.split_last() else { return Ok(()) };
    let t = table_at(root, parents)?;
    if t.contains_key(last) {
        return Err(format!("duplicate key '{}'", path.join(".")));
    }
    t.insert(last.clone(), v);
    Ok(())
}

fn default_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let appdata = std::env::var_os("APPDATA").filter(|s| !s.is_empty())?;
        Some(PathBuf::from(appdata).join("ogle").join("config.toml"))
    }
    #[cfg(not(windows))]
    {
        let base = match std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()) {
            Some(p) => p,
            None => PathBuf::from(std::env::var_os("HOME").filter(|h| !h.is_empty())?).join(".config"),
        };
        Some(base.join("ogle").join("config.toml"))
    }
}

// ---------------------------------------------------------------------------
// Known keys (GEN-012, UI-010, UI-012..UI-016). One static table; module-common keys
// (`enabled`, `refresh`, `thresholds`) are handled in `kind_of`.

#[derive(Clone, Copy)]
enum Kind {
    Bool,
    /// Integer or float.
    Num,
    Int,
    Str,
    StrList,
    /// String restricted to these values.
    OneOf(&'static [&'static str]),
    /// Array of strings restricted to these values.
    ListOf(&'static [&'static str]),
    /// Table whose values are all strings (`alias`).
    StrMap,
    /// `[<module>.thresholds]`: shape owned by 05-alerts.md, not checked here.
    Thresholds,
    Any,
}

impl Kind {
    fn check(self, v: &Value) -> Result<(), String> {
        let ok = match (self, v) {
            (Kind::Bool, Value::Bool(_)) => true,
            (Kind::Num, Value::Int(_) | Value::Float(_)) => true,
            (Kind::Int, Value::Int(_)) => true,
            (Kind::Str, Value::Str(_)) => true,
            (Kind::StrList, Value::Array(a)) => a.iter().all(|e| matches!(e, Value::Str(_))),
            (Kind::OneOf(opts), Value::Str(s)) => opts.contains(&s.as_str()),
            (Kind::ListOf(opts), Value::Array(a)) => {
                a.iter().all(|e| matches!(e, Value::Str(s) if opts.contains(&s.as_str())))
            }
            (Kind::StrMap, Value::Table(t)) => t.values().all(|e| matches!(e, Value::Str(_))),
            (Kind::Thresholds, Value::Table(_)) => true,
            (Kind::Any, _) => true,
            _ => false,
        };
        if ok {
            return Ok(());
        }
        Err(match self {
            Kind::Bool => "a boolean".to_string(),
            Kind::Num => "a number".to_string(),
            Kind::Int => "an integer".to_string(),
            Kind::Str => "a string".to_string(),
            Kind::StrList => "an array of strings".to_string(),
            Kind::OneOf(o) => format!("one of \"{}\"", o.join("\", \"")),
            Kind::ListOf(o) => format!("an array of names from: {}", o.join(", ")),
            Kind::StrMap => "a table of strings".to_string(),
            Kind::Thresholds => "a table".to_string(),
            Kind::Any => String::new(),
        })
    }
}

/// Process sort keys (PROC-060), shared with the CLI.
pub(crate) const SORT_KEYS: &[&str] = &["auto", "cpu", "mem", "io", "time", "name", "user", "core", "pid"];
const TRISTATE: &[&str] = &["auto", "always", "never"];
const SIDEBAR: &[&str] = &["network", "diskio", "fs", "sensors", "now"]; // UI-016
const QUICKLOOK_LIST: &[&str] = &["cpu", "mem", "swap", "load"]; // SYS-094
const DISKIO_MODES: &[&str] = &["throughput", "iops", "latency"]; // IO-049

#[rustfmt::skip]
static KEYS: &[(&str, &str, Kind)] = &[
    // UI-012
    ("global", "refresh", Kind::Num),
    ("global", "history", Kind::Bool),
    // UI-013
    ("ui", "color", Kind::OneOf(TRISTATE)),
    ("ui", "bold", Kind::Bool),
    ("ui", "background", Kind::Bool),
    ("ui", "unicode", Kind::OneOf(TRISTATE)),
    ("ui", "separator", Kind::Bool),
    ("ui", "left_sidebar", Kind::Bool),
    ("ui", "sidebar_order", Kind::ListOf(SIDEBAR)),
    ("ui", "quicklook", Kind::Bool),
    ("ui", "full_quicklook", Kind::Bool),
    ("ui", "top", Kind::Bool),
    ("ui", "percpu", Kind::Bool),
    ("ui", "sparkline", Kind::Bool),
    ("ui", "normalize_cpu", Kind::Bool),
    ("ui", "arrow_keys_sort", Kind::Bool),
    ("ui", "escape_timeout_ms", Kind::Int),
    // 02-system-modules.md
    ("system", "info_format", Kind::Str),          // SYS-022
    ("now", "format", Kind::Str),                  // SYS-041
    ("percpu", "max_cpu_display", Kind::Int),
    ("mem", "show_available", Kind::Bool),
    ("quicklook", "list", Kind::ListOf(QUICKLOOK_LIST)),
    ("quicklook", "bar_char", Kind::Str),
    // 03-io-modules.md
    ("network", "hide", Kind::StrList),
    ("network", "show", Kind::StrList),
    ("network", "hide_no_up", Kind::Bool),         // IO-028
    ("network", "hide_no_ip", Kind::Bool),         // IO-029
    ("network", "hide_zero", Kind::Bool),          // IO-017
    ("network", "hide_threshold_bytes", Kind::Int),
    ("network", "bytes", Kind::Bool),
    ("network", "sum", Kind::Bool),
    ("network", "cumulative", Kind::Bool),
    ("network", "alias", Kind::StrMap),            // IO-015
    ("diskio", "hide", Kind::StrList),
    ("diskio", "show", Kind::StrList),
    ("diskio", "show_ramfs", Kind::Bool),
    ("diskio", "hide_zero", Kind::Bool),
    ("diskio", "hide_threshold_bytes", Kind::Int),
    ("diskio", "mode", Kind::OneOf(DISKIO_MODES)),
    ("diskio", "alias", Kind::StrMap),
    ("fs", "hide", Kind::StrList),
    ("fs", "show", Kind::StrList),
    ("fs", "allow", Kind::StrList),                // IO-055
    ("fs", "free_space", Kind::Bool),
    ("fs", "alias", Kind::StrMap),
    ("sensors", "hide", Kind::StrList),
    ("sensors", "show", Kind::StrList),
    ("sensors", "fahrenheit", Kind::Bool),
    ("sensors", "alias", Kind::StrMap),
    // 04-processes.md section 15
    ("processlist", "hide_kernel_threads", Kind::Bool),
    ("processlist", "short_name", Kind::Bool),
    ("processlist", "sort_key", Kind::OneOf(SORT_KEYS)),
    ("processlist", "programs", Kind::Bool),
    ("processlist", "focus", Kind::StrList),
    ("processlist", "disable_columns", Kind::StrList),
    ("processlist", "nice_ok", Kind::Any),         // list of int, or "nonzero"
    ("processlist", "nice_careful", Kind::Any),
    ("processlist", "nice_warning", Kind::Any),
    ("processlist", "nice_critical", Kind::Any),
    ("processlist", "status_ok", Kind::StrList),
    ("processlist", "status_careful", Kind::StrList),
    ("processlist", "status_warning", Kind::StrList),
    ("processlist", "status_critical", Kind::StrList),
    // Written by CLI overrides only (`--filter`, `--no-processes`); accepted so that
    // the warning pass does not depend on whether overrides were applied.
    ("processlist", "filter", Kind::Str),
    ("processlist", "collect", Kind::Bool),
    // ALR-041
    ("alert", "max_events", Kind::Int),
    ("alert", "min_duration", Kind::Num),
    ("alert", "min_interval", Kind::Num),
    ("alert", "hysteresis", Kind::Num),
];

fn is_module(sec: &str) -> bool {
    ModuleId::from_name(sec).is_some()
}

fn is_section(sec: &str) -> bool {
    sec == "global" || sec == "ui" || is_module(sec)
}

fn kind_of(sec: &str, key: &str) -> Option<Kind> {
    if is_module(sec) {
        // UI-014 common keys; ALR-011 thresholds sub-table.
        match key {
            "enabled" => return Some(Kind::Bool),
            "refresh" => return Some(Kind::Num),
            "thresholds" => return Some(Kind::Thresholds),
            _ => {}
        }
    }
    KEYS.iter().find(|k| k.0 == sec && k.1 == key).map(|k| k.2)
}

fn check_key(sec: &str, key: &str, v: &Value) -> Result<(), String> {
    match kind_of(sec, key) {
        Some(kind) => kind.check(v).map_err(|exp| format!("wrong type for '{sec}.{key}': expected {exp}")),
        None => Ok(()), // unknown keys only warn (GEN-012)
    }
}

/// UI-010: wrong types of known keys are fatal. `path` is the full dotted path of `v`.
fn validate(path: &[String], v: &Value) -> Result<(), String> {
    match path {
        [sec] if is_section(sec) => match v {
            Value::Table(t) => t.iter().try_for_each(|(k, cv)| check_key(sec, k, cv)),
            _ => Err(format!("'{sec}' must be a table")),
        },
        [sec, key] => check_key(sec, key, v),
        [sec, key, rest @ ..] => match kind_of(sec, key) {
            None | Some(Kind::Thresholds) | Some(Kind::Any) => Ok(()),
            Some(Kind::StrMap) if rest.len() == 1 && matches!(v, Value::Str(_)) => Ok(()),
            Some(Kind::StrMap) if rest.len() == 1 => {
                Err(format!("wrong type for '{}': expected a string", path.join(".")))
            }
            Some(_) => Err(format!("'{sec}.{key}' is not a table")),
        },
        _ => Ok(()),
    }
}

/// `--print-default-config` output. Must parse cleanly and produce no warnings.
const DEFAULT_TOML: &str = r#"# ogle configuration file (TOML).
#
# Location: --config PATH, else $XDG_CONFIG_HOME/ogle/config.toml
# (default ~/.config/ogle/config.toml), or %APPDATA%\ogle\config.toml on Windows.
# Precedence: built-in default < this file < command line.
#
# Every module table accepts `enabled` (bool, default true) and `refresh` (seconds).
# Threshold limits live only in [<module>.thresholds]. A limit set may hold
# careful, warning, critical, log and hysteresis, written module-wide (plain keys),
# per field (`field = { ... }`) or per item (`items."name" = { ... }`).
# Limits are in percent of the stat's maximum.

[global]
refresh = 2.0            # seconds, 0.1 to 3600
history = true           # trend arrows and sparklines

[ui]
color = "auto"           # "auto", "always", "never"
bold = true
background = true
unicode = "auto"         # "auto", "always", "never"
separator = true
left_sidebar = true
sidebar_order = ["network", "diskio", "fs", "sensors", "now"]
quicklook = true
full_quicklook = false
top = true
percpu = false
sparkline = false
normalize_cpu = false
arrow_keys_sort = false
escape_timeout_ms = 30

[system]
# info_format = "{linux_distro} {arch} / {os_name} {os_version}"
refresh = 60

[uptime]
# enabled = true

[now]
# format = "%Y-%m-%d %H:%M:%S %Z"

[cpu]
# refresh = 2            # default: [global] refresh

[cpu.thresholds]
total        = { careful = 65, warning = 75, critical = 85, log = true }
user         = { careful = 50, warning = 70, critical = 90, log = false }
system       = { careful = 50, warning = 70, critical = 90, log = false }
iowait       = { careful = 50, warning = 70, critical = 90, log = true }
steal        = { careful = 50, warning = 70, critical = 90 }
ctx_switches = { careful = 40000, warning = 45000, critical = 50000 }

[percpu]
max_cpu_display = 4

[percpu.thresholds]
user   = { careful = 50, warning = 70, critical = 90 }
system = { careful = 50, warning = 70, critical = 90 }
iowait = { careful = 50, warning = 70, critical = 90 }

[load]
# refresh = 2

[load.thresholds]        # percent of the core count: 70 = 0.7 load per core
careful = 70
warning = 100
critical = 500
log = true

[mem]
show_available = false

[mem.thresholds]
careful = 50
warning = 70
critical = 90
log = true

[memswap]
# refresh = 2

[memswap.thresholds]
careful = 50
warning = 70
critical = 90
log = true

[quicklook]
list = ["cpu", "mem", "load"]    # any of "cpu", "mem", "swap", "load"
bar_char = "|"

[quicklook.thresholds]
cpu  = { careful = 50, warning = 70, critical = 90 }
mem  = { careful = 50, warning = 70, critical = 90 }
swap = { careful = 50, warning = 70, critical = 90 }
load = { careful = 70, warning = 100, critical = 500 }

[network]
# refresh = 2
hide = ["lo", "docker.*", "veth.*"]
show = []
hide_no_up = true
hide_no_ip = true
hide_zero = false
hide_threshold_bytes = 0
bytes = false            # start in bytes mode instead of bits
sum = false
cumulative = false
# alias = { wlp0s20f3 = "WIFI" }

[network.thresholds]
rx = { careful = 70, warning = 80, critical = 90 }   # % of link speed
tx = { careful = 70, warning = 80, critical = 90 }
# items."wlan0".rx = { careful = 4_000_000, warning = 5_000_000, critical = 6_000_000, log = true }  # bits/s

[diskio]
# refresh = 2
hide = ["loop.*", "/dev/loop.*"]
show = []
show_ramfs = false
hide_zero = false
hide_threshold_bytes = 0
mode = "throughput"      # "throughput", "iops", "latency"
# alias = { sda1 = "SystemDisk" }

[diskio.thresholds]
read_latency  = { careful = 10, warning = 20, critical = 50 }   # ms
write_latency = { careful = 10, warning = 20, critical = 50 }
# items."dm-0".read  = { careful = 4_000_000_000, warning = 5_000_000_000, critical = 6_000_000_000 }  # bytes/s

[fs]
refresh = 60
hide = ["/boot.*", ".*/snap.*"]
show = []
allow = []               # e.g. ["nfs", "cifs", "fuse.sshfs"]
free_space = false
# alias = { "/" = "root" }

[fs.thresholds]          # percent of the file system size
careful = 50
warning = 70
critical = 90
# items."/" = { careful = 70, warning = 85, critical = 95 }

[sensors]
refresh = 10
hide = ["unknown.*"]
show = []
fahrenheit = false
# alias = { "package id 0" = "CPU", "composite" = "NVMe" }

[sensors.thresholds]
# temperature = { careful = 45, warning = 65, critical = 80 }   # unset: hardware limits
battery = { careful = 70, warning = 80, critical = 90 }          # applies to 100 - charge
# fan = { careful = 4000, warning = 5000, critical = 6000 }       # RPM

[processcount]
# enabled = true

[processlist]
# refresh = 2
hide_kernel_threads = false
short_name = true
sort_key = "auto"        # auto cpu mem io time name user core pid
programs = false
focus = []
disable_columns = []
nice_ok = []
nice_careful = []
nice_warning = "nonzero"
nice_critical = []
status_ok = ["R", "W", "P", "I"]
status_careful = []
status_warning = []
status_critical = ["Z", "D"]

[processlist.thresholds]
cpu = { careful = 50, warning = 70, critical = 90 }
mem = { careful = 50, warning = 70, critical = 90 }

[programlist]
# enabled = true

[alert]
max_events = 10
min_duration = 6         # seconds
min_interval = 6         # seconds
hysteresis = 0
"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Config {
        Config::parse(s).unwrap_or_else(|e| panic!("{e}"))
    }

    fn err(s: &str) -> ConfigError {
        Config::parse(s).expect_err("expected an error")
    }

    #[test]
    fn scalars_and_tables() {
        let c = parse(
            "# top\n[global]\nrefresh = 1.5 # c\nhistory = false\n\n[cpu]\nenabled=true\nrefresh = 3\n",
        );
        assert_eq!(c.f64(&["global", "refresh"]), Some(1.5));
        assert_eq!(c.bool(&["global", "history"]), Some(false));
        assert_eq!(c.bool(&["cpu", "enabled"]), Some(true));
        assert_eq!(c.f64(&["cpu", "refresh"]), Some(3.0));
    }

    #[test]
    fn strings_keys_numbers() {
        let c = parse(concat!(
            "[network.thresholds]\n",
            "items.\"wlan0\".rx = { careful = 4_000_000, log = true }\n",
            "[x]\n",
            "a = \"t\\tq\\\"\\u00e9\"\n",
            "b = 'C:\\path'\n",
            "'quoted key' = 0x10\n",
            "c = 1e3\n",
            "d = -inf\n",
        ));
        assert_eq!(c.i64(&["network", "thresholds", "items", "wlan0", "rx", "careful"]), Some(4_000_000));
        assert_eq!(c.bool(&["network", "thresholds", "items", "wlan0", "rx", "log"]), Some(true));
        assert_eq!(c.str(&["x", "a"]), Some("t\tq\"\u{e9}"));
        assert_eq!(c.str(&["x", "b"]), Some("C:\\path"));
        assert_eq!(c.f64(&["x", "c"]), Some(1000.0));
        assert_eq!(c.f64(&["x", "d"]), Some(f64::NEG_INFINITY));
        assert_eq!(c.i64(&["x", "quoted key"]), Some(16));
        assert!(Config::parse("[x]\nk = -0x10\n").is_err());
    }

    #[test]
    fn arrays_multiline() {
        let c = parse("[network]\nhide = [\n  \"lo\", # loopback\n  \"veth.*\",\n]\nshow = []\n");
        assert_eq!(c.str_list(&["network", "hide"]), Some(vec!["lo".into(), "veth.*".into()]));
        assert_eq!(c.str_list(&["network", "show"]), Some(vec![]));
    }

    #[test]
    fn errors_have_line_numbers() {
        assert_eq!(err("[ui]\n\npercpu = yes\n").line, 3);
        assert_eq!(err("[a]\nk = 1\nk = 2\n").line, 3);
        assert_eq!(err("[a]\n[a]\n").line, 2);
        assert_eq!(err("a = \"open\n").line, 1);
        assert_eq!(err("a = [1,\n2\n3]\n").line, 3);
        assert_eq!(err("[[a]]\n").line, 1);
        assert_eq!(err("a = 1 b\n").line, 1);
        assert_eq!(err("x = 1\nx.y = 2\n").line, 2);
        assert!(err("a = 2024-01-01\n").msg.contains("invalid value"));
        assert!(err("a = 1__0\n").msg.contains("invalid value"));
    }

    #[test]
    fn wrong_types_are_errors() {
        assert_eq!(err("[ui]\npercpu = 1\n").line, 2);
        assert!(err("[global]\nrefresh = \"2\"\n").msg.contains("global.refresh"));
        assert!(err("ui = 3\n").msg.contains("must be a table"));
        assert!(err("ui = { percpu = 1 }\n").msg.contains("ui.percpu"));
        assert!(err("[ui.percpu]\n").msg.contains("ui.percpu"));
        assert!(err("[ui]\npercpu = true\n[ui.percpu.x]\n").msg.contains("not a table"));
        assert!(err("[ui]\nsidebar_order = [\"cpu\"]\n").msg.contains("sidebar_order"));
        assert!(err("[diskio]\nmode = \"fast\"\n").msg.contains("diskio.mode"));
        assert!(err("[network]\nalias.eth0 = 1\n").msg.contains("network.alias.eth0"));
        // Int is accepted where a number is expected; anything goes under thresholds.
        assert!(Config::parse("[global]\nrefresh = 2\n[cpu.thresholds]\nfoo = { bar = \"x\" }\n").is_ok());
    }

    #[test]
    fn unknown_keys_warn() {
        let c = parse("[cpu]\nenable = true\n[cpu.thresholds]\nx = 1\n[bogus]\nk = 1\n[ui]\npercpu = true\n");
        let w = c.unknown_key_warnings();
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(w[0].contains("'bogus.k'") || w[0].contains("[bogus]"));
        assert!(w.iter().any(|l| l.contains("'cpu.enable'")));
    }

    #[test]
    fn set_creates_and_replaces() {
        let mut c = Config::default();
        c.set(&["processlist", "focus"], Value::Array(vec![Value::Str("a".into())]));
        c.set(&["ui", "percpu"], Value::Bool(true));
        c.set(&["ui", "percpu", "x"], Value::Int(1));
        assert_eq!(c.str_list(&["processlist", "focus"]), Some(vec!["a".into()]));
        assert_eq!(c.i64(&["ui", "percpu", "x"]), Some(1));
    }

    #[test]
    fn default_toml_is_clean() {
        let c = parse(Config::default_toml());
        assert!(c.unknown_key_warnings().is_empty());
        assert_eq!(c.f64(&["global", "refresh"]), Some(2.0));
        assert_eq!(c.i64(&["fs", "refresh"]), Some(60));
        assert_eq!(c.str(&["processlist", "sort_key"]), Some("auto"));
        assert_eq!(c.f64(&["cpu", "thresholds", "total", "critical"]), Some(85.0));
        for m in ModuleId::ALL {
            assert!(c.table(&[m.name()]).is_some(), "missing [{}]", m.name());
        }
    }
}
