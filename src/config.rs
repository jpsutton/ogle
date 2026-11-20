//! Configuration (spec 01 section 2, GEN-012). TOML subset parser, no dependencies.
//!
//! OWNER: config agent fills the bodies marked `todo!()`. Signatures are a contract used
//! by every module; do not change them.
//!
//! Supported TOML subset: comments, `[table]` and `[a.b."quoted"]` headers, bare and
//! quoted keys, dotted keys, strings (basic + literal, common escapes), integers, floats,
//! booleans, arrays (single or multi-line, trailing comma), inline tables. No dates,
//! no multi-line strings, no arrays of tables. Errors carry line numbers.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
        let _ = text;
        todo!()
    }

    /// GEN-012 search order: explicit path (error if missing), else
    /// `$XDG_CONFIG_HOME/ogle/config.toml` / `~/.config/ogle/config.toml` (Unix, macOS),
    /// `%APPDATA%\ogle\config.toml` (Windows). Missing default file -> empty Config.
    pub fn load(explicit: Option<&Path>) -> Result<Config, ConfigError> {
        let _ = explicit;
        todo!()
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
        let _ = (path, v);
        todo!()
    }

    /// Validate known keys (UI-01x). Returns warnings for unknown keys/tables;
    /// the caller prints them to stderr before entering the UI.
    pub fn unknown_key_warnings(&self) -> Vec<String> {
        todo!()
    }

    /// Full default configuration as commented TOML (`--print-default-config`).
    pub fn default_toml() -> &'static str {
        todo!()
    }
}
