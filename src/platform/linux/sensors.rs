//! Linux temperatures, fans and batteries (PLT-130, PLT-131).
//!
//! hwmon is enumerated at most every 60 s; labels, max and crit are read then (C0).
//! Each cycle only the kept `*_input` handles are re-read with pread (PERF-020).

use std::fs::File;
use std::time::{Duration, Instant};

use super::super::util::{parse_f64, parse_i64, parse_u64, trim};
use super::super::{Battery, BatteryStatus, Sensor, SensorKind};
use super::{pread_into, read_path};

const REENUM: Duration = Duration::from_secs(60);

struct Input {
    kind: SensorKind,
    chip: String,
    label: String,
    high: Option<f64>,
    critical: Option<f64>,
    file: File,
    /// Divisor from raw value to output unit (1000 for millidegrees, 1 for RPM).
    div: f64,
}

pub(super) struct Sensors {
    inputs: Vec<Input>,
    enum_at: Option<Instant>,
    /// A read failed (hot-unplug, ENODEV): re-enumerate on the next call.
    stale: bool,
    bats: Vec<(String, String)>,
    bat_at: Option<Instant>,
    buf: Vec<u8>,
    path: String,
}

/// Sort key for names like "hwmon12" / "thermal_zone3": numeric suffix.
fn num_suffix(s: &str) -> u64 {
    let d = s.trim_start_matches(|c: char| !c.is_ascii_digit());
    d.parse().unwrap_or(u64::MAX)
}

fn sorted_dir(path: &str, prefix: &str) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(path)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| n.starts_with(prefix))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|n| (num_suffix(n), n.clone()));
    v
}

fn read_trimmed(path: &str, buf: &mut Vec<u8>) -> Option<String> {
    let n = read_path(path, buf)?;
    let t = trim(&buf[..n]);
    (!t.is_empty()).then(|| String::from_utf8_lossy(t).into_owned())
}

fn read_num(path: &str, buf: &mut Vec<u8>) -> Option<i64> {
    let n = read_path(path, buf)?;
    parse_i64(&buf[..n])
}

impl Sensors {
    pub(super) fn new() -> Self {
        Sensors {
            inputs: Vec::new(),
            enum_at: None,
            stale: false,
            bats: Vec::new(),
            bat_at: None,
            buf: Vec::new(),
            path: String::new(),
        }
    }

    pub(super) fn reset(&mut self) {
        self.enum_at = None;
        self.bat_at = None;
    }

    fn enumerate(&mut self) {
        self.inputs.clear();
        let buf = &mut self.buf;
        for dir in sorted_dir("/sys/class/hwmon", "hwmon") {
            let base = format!("/sys/class/hwmon/{dir}/");
            let chip = read_trimmed(&(base.clone() + "name"), buf).unwrap_or_else(|| dir.clone());
            // (kind order, index, file prefix)
            let mut found: Vec<(u8, u64, String)> = Vec::new();
            if let Ok(rd) = std::fs::read_dir(&base) {
                for e in rd.flatten() {
                    let Ok(n) = e.file_name().into_string() else { continue };
                    let Some(pre) = n.strip_suffix("_input") else { continue };
                    let k = if pre.starts_with("temp") {
                        0
                    } else if pre.starts_with("fan") {
                        1
                    } else {
                        continue;
                    };
                    found.push((k, num_suffix(pre), pre.to_string()));
                }
            }
            found.sort();
            for (k, _, pre) in found {
                let p = base.clone() + &pre;
                let Ok(file) = File::open(p.clone() + "_input") else { continue };
                let label = read_trimmed(&(p.clone() + "_label"), buf).unwrap_or_default();
                let (kind, div) = if k == 0 { (SensorKind::Temperature, 1000.0) } else { (SensorKind::Fan, 1.0) };
                let (high, critical) = if k == 0 {
                    (
                        read_num(&(p.clone() + "_max"), buf).map(|v| v as f64 / 1000.0),
                        read_num(&(p + "_crit"), buf).map(|v| v as f64 / 1000.0),
                    )
                } else {
                    (None, None)
                };
                self.inputs.push(Input { kind, chip: chip.clone(), label, high, critical, file, div });
            }
        }
        // Fallback when hwmon has no temperature: thermal zones.
        if !self.inputs.iter().any(|i| i.kind == SensorKind::Temperature) {
            for dir in sorted_dir("/sys/class/thermal", "thermal_zone") {
                let base = format!("/sys/class/thermal/{dir}/");
                let Ok(file) = File::open(base.clone() + "temp") else { continue };
                let chip = read_trimmed(&(base + "type"), buf).unwrap_or(dir);
                self.inputs.push(Input {
                    kind: SensorKind::Temperature,
                    chip,
                    label: String::new(),
                    high: None,
                    critical: None,
                    file,
                    div: 1000.0,
                });
            }
        }
        self.enum_at = Some(Instant::now());
        self.stale = false;
    }

    pub(super) fn sensors(&mut self, want_temp: bool, want_fan: bool, out: &mut Vec<Sensor>) {
        if !want_temp && !want_fan {
            out.clear();
            return;
        }
        if self.stale || self.enum_at.map_or(true, |t| t.elapsed() >= REENUM) {
            self.enumerate();
        }
        let mut count = 0;
        for i in &self.inputs {
            let want = match i.kind {
                SensorKind::Temperature => want_temp,
                SensorKind::Fan => want_fan,
            };
            if !want {
                continue;
            }
            // PLT-130: EIO/ENODATA make the sensor absent for this cycle only. Other
            // errors (ENODEV after hot-unplug) trigger re-enumeration.
            let v = match pread_into(&i.file, &mut self.buf, false) {
                Ok(n) => parse_i64(&self.buf[..n]),
                Err(e) => {
                    if !matches!(e.raw_os_error(), Some(libc::EIO) | Some(libc::ENODATA)) {
                        self.stale = true;
                    }
                    None
                }
            };
            let Some(v) = v else { continue };
            let value = v as f64 / i.div;
            if count < out.len() {
                let s = &mut out[count];
                s.kind = i.kind;
                s.chip.clone_from(&i.chip);
                s.label.clone_from(&i.label);
                s.value = value;
                s.high = i.high;
                s.critical = i.critical;
            } else {
                out.push(Sensor {
                    kind: i.kind,
                    chip: i.chip.clone(),
                    label: i.label.clone(),
                    value,
                    high: i.high,
                    critical: i.critical,
                });
            }
            count += 1;
        }
        out.truncate(count);
    }

    fn attr(&mut self, dir: usize, name: &str) -> Option<usize> {
        self.path.clear();
        self.path.push_str(&self.bats[dir].1);
        self.path.push_str(name);
        read_path(&self.path, &mut self.buf)
    }

    fn attr_u64(&mut self, dir: usize, name: &str) -> Option<u64> {
        let n = self.attr(dir, name)?;
        parse_u64(&self.buf[..n])
    }

    pub(super) fn batteries(&mut self, out: &mut Vec<Battery>) {
        // PLT-131
        if self.bat_at.map_or(true, |t| t.elapsed() >= REENUM) {
            self.bats.clear();
            for name in sorted_dir("/sys/class/power_supply", "") {
                let base = format!("/sys/class/power_supply/{name}/");
                if read_trimmed(&(base.clone() + "type"), &mut self.buf).as_deref() != Some("Battery") {
                    continue;
                }
                if read_trimmed(&(base.clone() + "scope"), &mut self.buf).as_deref() == Some("Device") {
                    continue;
                }
                self.bats.push((name, base));
            }
            self.bat_at = Some(Instant::now());
        }
        let mut count = 0;
        for b in 0..self.bats.len() {
            let pct = match self.attr(b, "capacity").and_then(|n| parse_f64(&self.buf[..n])) {
                Some(p) => Some(p),
                None => {
                    let ratio = |now: Option<u64>, full: Option<u64>| match (now, full) {
                        (Some(n), Some(f)) if f > 0 => Some(n as f64 / f as f64 * 100.0),
                        _ => None,
                    };
                    let e = ratio(self.attr_u64(b, "energy_now"), self.attr_u64(b, "energy_full"));
                    e.or_else(|| ratio(self.attr_u64(b, "charge_now"), self.attr_u64(b, "charge_full")))
                }
            };
            let Some(pct) = pct else { continue };
            let status = match self.attr(b, "status").map(|n| trim(&self.buf[..n])) {
                Some(b"Charging") => BatteryStatus::Charging,
                Some(b"Discharging") => BatteryStatus::Discharging,
                Some(b"Full") => BatteryStatus::Full,
                Some(b"Not charging") => BatteryStatus::NotCharging,
                _ => BatteryStatus::Unknown,
            };
            let percent = pct.clamp(0.0, 100.0);
            if count < out.len() {
                let o = &mut out[count];
                o.name.clone_from(&self.bats[b].0);
                o.percent = percent;
                o.status = status;
            } else {
                out.push(Battery { name: self.bats[b].0.clone(), percent, status });
            }
            count += 1;
        }
        out.truncate(count);
    }
}
