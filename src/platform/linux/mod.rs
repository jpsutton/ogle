//! Linux backend (06-platform-sources.md, Linux parts; 07-performance.md PERF-020..026).
//!
//! System-wide files are opened once and re-read with `pread` at offset 0 into reused
//! buffers (PERF-020). Per-process files are opened with `openat` relative to a kept
//! `/proc` directory fd (PERF-021). See `procs.rs` for the process hot path.

mod devices;
mod procs;
mod sensors;

use std::ffi::CStr;
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::time::{Duration, Instant};

use super::util::{fields, find_key, lines, parse_f64, parse_i64, parse_u64};
use super::*;

/// How long a system-wide buffer counts as "this cycle's read" for a second domain
/// that shares it (PLT-052 /proc/stat, PLT-090 /proc/meminfo).
const SHARE_WINDOW: Duration = Duration::from_millis(500);
/// PLT-041: frequency is cached for 4 s.
const FREQ_CACHE: Duration = Duration::from_secs(4);

/// Read a whole file with positional reads into `buf` (kept at full length, reused).
/// Without `until_eof` a short read counts as end of file, so small procfs/sysfs files
/// cost one read syscall (PERF-018). Returns the number of valid bytes.
pub(crate) fn pread_into(f: &File, buf: &mut Vec<u8>, until_eof: bool) -> io::Result<usize> {
    if buf.len() < 4096 {
        buf.resize(4096, 0);
    }
    let mut off = 0;
    loop {
        if off == buf.len() {
            let n = buf.len() * 2;
            buf.resize(n, 0);
        }
        match f.read_at(&mut buf[off..], off as u64) {
            Ok(0) => break,
            Ok(n) => {
                off += n;
                if !until_eof && off < buf.len() {
                    break;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(off)
}

/// Read a (small, static) file by path into `buf`. C0 / slow-cadence reads only.
pub(crate) fn read_path(path: &str, buf: &mut Vec<u8>) -> Option<usize> {
    let f = File::open(path).ok()?;
    pread_into(&f, buf, true).ok()
}

/// A system-wide file kept open across cycles (PERF-020). On a read error the handle
/// is closed and re-opened on the next call.
pub(crate) struct SysFile {
    path: &'static str,
    file: Option<File>,
}

impl SysFile {
    pub(crate) fn new(path: &'static str) -> Self {
        SysFile { path, file: File::open(path).ok() }
    }
    pub(crate) fn read(&mut self, buf: &mut Vec<u8>) -> Option<usize> {
        if self.file.is_none() {
            self.file = File::open(self.path).ok();
        }
        match pread_into(self.file.as_ref()?, buf, false) {
            Ok(n) => Some(n),
            Err(_) => {
                self.file = None;
                None
            }
        }
    }
}

pub struct Sys {
    clk_tck: f64,
    page_size: u64,
    stat: SysFile,
    stat_buf: Vec<u8>,
    stat_len: usize,
    stat_at: Option<Instant>,
    meminfo: SysFile,
    mem_buf: Vec<u8>,
    mem_len: usize,
    mem_at: Option<Instant>,
    loadavg: SysFile,
    /// ZFS ARC stats, only when the file existed at start-up (PLT-080).
    arcstats: Option<SysFile>,
    small_buf: Vec<u8>,
    /// (boot time, wall-minus-monotonic offset at the time it was read).
    boot: Option<(u64, f64)>,
    topo: Option<CpuTopology>,
    cpu_lines: usize,
    /// The CPU model was already returned by cpu_freq() (or found absent).
    model_given: bool,
    freq_cur: Option<Vec<File>>,
    freq_max: Option<u64>,
    freq_cache: Option<(Instant, CpuFreq)>,
    pid_max: Option<Option<u32>>,
    dev: devices::Devices,
    sens: sensors::Sensors,
    procs: procs::Procs,
}

pub fn new() -> Sys {
    // SAFETY: sysconf has no memory-safety preconditions.
    let (tck, pg) = unsafe { (libc::sysconf(libc::_SC_CLK_TCK), libc::sysconf(libc::_SC_PAGESIZE)) };
    let clk_tck = if tck > 0 { tck as f64 } else { 100.0 };
    let page_size = if pg > 0 { pg as u64 } else { 4096 };
    let arc = "/proc/spl/kstat/zfs/arcstats";
    Sys {
        clk_tck,
        page_size,
        stat: SysFile::new("/proc/stat"),
        stat_buf: Vec::new(),
        stat_len: 0,
        stat_at: None,
        meminfo: SysFile::new("/proc/meminfo"),
        mem_buf: Vec::new(),
        mem_len: 0,
        mem_at: None,
        loadavg: SysFile::new("/proc/loadavg"),
        arcstats: std::path::Path::new(arc).exists().then(|| SysFile::new(arc)),
        small_buf: Vec::new(),
        boot: None,
        topo: None,
        cpu_lines: 0,
        model_given: false,
        freq_cur: None,
        freq_max: None,
        freq_cache: None,
        pid_max: None,
        dev: devices::Devices::new(),
        sens: sensors::Sensors::new(),
        procs: procs::Procs::new(clk_tck, page_size),
    }
}

/// Wall-clock minus monotonic seconds; changes when the system clock jumps.
fn wall_mono_offset() -> f64 {
    let wall = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: ts is a valid, writable timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    wall - (ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9)
}

fn cstr_field(p: &[libc::c_char]) -> String {
    // SAFETY: uname() fills NUL-terminated strings within each array.
    unsafe { CStr::from_ptr(p.as_ptr()) }.to_string_lossy().into_owned()
}

impl Sys {
    /// Read /proc/stat into stat_buf (PLT-051/052).
    fn read_stat(&mut self) -> bool {
        match self.stat.read(&mut self.stat_buf) {
            Some(n) => {
                self.stat_len = n;
                self.stat_at = Some(Instant::now());
                true
            }
            None => false,
        }
    }

    /// Reuse this cycle's /proc/stat read when fresh, else read again.
    fn stat_shared(&mut self) -> bool {
        match self.stat_at {
            Some(t) if t.elapsed() < SHARE_WINDOW => true,
            _ => self.read_stat(),
        }
    }

    /// /proc/meminfo, shared between memory() and swap() in one cycle (PLT-090).
    fn meminfo_shared(&mut self) -> bool {
        if let Some(t) = self.mem_at {
            if t.elapsed() < SHARE_WINDOW {
                return true;
            }
        }
        match self.meminfo.read(&mut self.mem_buf) {
            Some(n) => {
                self.mem_len = n;
                self.mem_at = Some(Instant::now());
                true
            }
            None => false,
        }
    }

    fn mem_kb(&self, key: &[u8]) -> Option<u64> {
        find_key(&self.mem_buf[..self.mem_len], key, b':').and_then(parse_u64).map(|v| v * 1024)
    }

    fn physical_cores() -> Option<u32> {
        // PLT-040: distinct (physical_package_id, core_id) pairs.
        let rd = std::fs::read_dir("/sys/devices/system/cpu").ok()?;
        let mut pairs: Vec<(i64, i64)> = Vec::new();
        let mut lists: Vec<String> = Vec::new();
        let mut buf = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("cpu") || !super::util::all_digits(name[3..].as_bytes()) {
                continue;
            }
            let base = format!("/sys/devices/system/cpu/{name}/topology/");
            let pkg = read_path(&(base.clone() + "physical_package_id"), &mut buf).and_then(|n| parse_i64(&buf[..n]));
            let core = read_path(&(base.clone() + "core_id"), &mut buf).and_then(|n| parse_i64(&buf[..n]));
            if let (Some(p), Some(c)) = (pkg, core) {
                pairs.push((p, c));
            } else if let Some(n) = read_path(&(base.clone() + "core_cpus_list"), &mut buf)
                .or_else(|| read_path(&(base + "thread_siblings_list"), &mut buf))
            {
                lists.push(String::from_utf8_lossy(super::util::trim(&buf[..n])).into_owned());
            }
        }
        let n = if !pairs.is_empty() {
            pairs.sort_unstable();
            pairs.dedup();
            pairs.len()
        } else {
            lists.sort_unstable();
            lists.dedup();
            lists.len()
        };
        (n > 0).then_some(n as u32)
    }

    fn read_cpu_model(&mut self) -> Option<String> {
        let n = read_path("/proc/cpuinfo", &mut self.small_buf)?;
        let buf = &self.small_buf[..n];
        let get = |key: &[u8]| {
            lines(buf).find_map(|l| {
                let p = l.iter().position(|&b| b == b':')?;
                (super::util::trim(&l[..p]) == key).then(|| super::util::trim(&l[p + 1..]))
            })
        };
        // PLT-040. ARM implementer/part lookup table is not implemented.
        let v = get(b"model name").or_else(|| get(b"Model")).or_else(|| get(b"cpu model"))?;
        (!v.is_empty()).then(|| String::from_utf8_lossy(v).into_owned())
    }

    /// Current and maximum frequency (PLT-041); `model` is left None.
    fn read_freq(&mut self) -> CpuFreq {
        if self.freq_cur.is_none() {
            self.open_freq_files();
        }
        // PLT-041: mean of scaling_cur_freq (kHz).
        let (mut sum, mut cnt) = (0u64, 0u64);
        for f in self.freq_cur.as_deref().unwrap_or(&[]) {
            if let Ok(n) = pread_into(f, &mut self.small_buf, false) {
                if let Some(v) = parse_u64(&self.small_buf[..n]) {
                    sum += v;
                    cnt += 1;
                }
            }
        }
        let mut current = (cnt > 0).then(|| sum / cnt * 1000);
        if current.is_none() {
            // Fallback: "cpu MHz" lines of /proc/cpuinfo.
            if let Some(n) = read_path("/proc/cpuinfo", &mut self.small_buf) {
                let (mut s, mut c) = (0.0, 0u32);
                for l in lines(&self.small_buf[..n]) {
                    if l.starts_with(b"cpu MHz") {
                        if let Some(p) = l.iter().position(|&b| b == b':') {
                            if let Some(v) = parse_f64(&l[p + 1..]) {
                                s += v;
                                c += 1;
                            }
                        }
                    }
                }
                current = (c > 0).then(|| (s / c as f64 * 1e6) as u64);
            }
        }
        CpuFreq { model: None, current_hz: current, max_hz: self.freq_max }
    }

    fn open_freq_files(&mut self) {
        let mut cur = Vec::new();
        let mut max: Option<u64> = None;
        let mut buf = Vec::new();
        if let Ok(rd) = std::fs::read_dir("/sys/devices/system/cpu/cpufreq") {
            for e in rd.flatten() {
                let p = e.path();
                if !e.file_name().to_string_lossy().starts_with("policy") {
                    continue;
                }
                if let Ok(f) = File::open(p.join("scaling_cur_freq")) {
                    cur.push(f);
                }
                if let Ok(f) = File::open(p.join("cpuinfo_max_freq")) {
                    if let Ok(n) = pread_into(&f, &mut buf, false) {
                        if let Some(v) = parse_u64(&buf[..n]) {
                            max = Some(max.map_or(v, |m: u64| m.max(v)));
                        }
                    }
                }
            }
        }
        self.freq_max = max.map(|k| k * 1000);
        self.freq_cur = Some(cur);
    }
}

impl Platform for Sys {
    fn system_info(&mut self) -> SystemInfo {
        // PLT-020
        // SAFETY: utsname is plain data; uname fills it.
        let mut u: libc::utsname = unsafe { std::mem::zeroed() };
        // SAFETY: u is a valid writable utsname.
        let ok = unsafe { libc::uname(&mut u) } == 0;
        let mut info = SystemInfo { os_name: "Linux".into(), ..Default::default() };
        if ok {
            info.hostname = cstr_field(&u.nodename);
            info.os_version = cstr_field(&u.release);
            info.arch = cstr_field(&u.machine);
        }
        let mut buf = Vec::new();
        let n = read_path("/etc/os-release", &mut buf).or_else(|| read_path("/usr/lib/os-release", &mut buf));
        if let Some(n) = n {
            let b = &buf[..n];
            let unq = |v: &[u8]| {
                let v = super::util::trim(v);
                let v = v.strip_prefix(b"\"").unwrap_or(v);
                let v = v.strip_suffix(b"\"").unwrap_or(v);
                let v = v.strip_prefix(b"'").unwrap_or(v);
                let v = v.strip_suffix(b"'").unwrap_or(v);
                String::from_utf8_lossy(v).into_owned()
            };
            let name = find_key(b, b"NAME", b'=').map(unq);
            let ver = find_key(b, b"VERSION_ID", b'=').map(unq);
            info.distro = match (name, ver) {
                (Some(n), Some(v)) => Some(n + " " + &v),
                (Some(n), None) => Some(n),
                _ => None,
            };
        }
        info
    }

    fn boot_time(&mut self) -> Option<u64> {
        // PLT-030; re-read when the wall clock jumps by more than 1 s.
        let off = wall_mono_offset();
        if let Some((b, o)) = self.boot {
            if (off - o).abs() <= 1.0 {
                return Some(b);
            }
        }
        if !self.stat_shared() {
            return None;
        }
        let b = find_key(&self.stat_buf[..self.stat_len], b"btime", b' ').and_then(parse_u64)?;
        self.boot = Some((b, off));
        Some(b)
    }

    fn cpu_topology(&mut self) -> CpuTopology {
        if let Some(t) = self.topo {
            return t;
        }
        // SAFETY: sysconf has no memory-safety preconditions.
        let n = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
        let t = CpuTopology { logical: if n > 0 { n as u32 } else { 1 }, physical: Self::physical_cores() };
        self.topo = Some(t);
        t
    }

    fn cpu_freq(&mut self) -> CpuFreq {
        // The model is handed out only on the first call; the caller (quicklook) keeps
        // it. Later results have `model: None`, so the cached value clones without
        // allocating. reset_caches() makes it available again.
        let fresh = matches!(&self.freq_cache, Some((t, _)) if t.elapsed() < FREQ_CACHE);
        if !fresh {
            let f = self.read_freq();
            self.freq_cache = Some((Instant::now(), f));
        }
        let mut f = self.freq_cache.as_ref().map(|c| c.1.clone()).unwrap_or_default();
        if !self.model_given {
            // PLT-040 (C0): /proc/cpuinfo is parsed once, also when no model is found.
            self.model_given = true;
            f.model = self.read_cpu_model();
        }
        f
    }

    fn cpu_times(&mut self, total: &mut CpuTimes, mut per_cpu: Option<&mut Vec<CpuTimes>>) -> bool {
        // PLT-051: one read serves total and per-CPU values.
        if !self.read_stat() {
            return false;
        }
        if let Some(v) = per_cpu.as_deref_mut() {
            v.clear();
        }
        let tck = self.clk_tck;
        let mut found = false;
        let mut cpu_lines = 0;
        for l in lines(&self.stat_buf[..self.stat_len]) {
            if !l.starts_with(b"cpu") {
                continue;
            }
            let mut it = fields(l);
            let name = it.next().unwrap_or(b"");
            let mut v = [None::<u64>; 10];
            for (slot, f) in v.iter_mut().zip(it) {
                *slot = parse_u64(f);
            }
            let s = |i: usize| v[i].map(|x| x as f64 / tck);
            let t = CpuTimes {
                user: s(0).unwrap_or(0.0),
                nice: s(1),
                system: s(2).unwrap_or(0.0),
                idle: s(3).unwrap_or(0.0),
                iowait: s(4),
                irq: s(5),
                softirq: s(6),
                steal: s(7),
                guest: s(8),
                dpc: None,
            };
            if name == b"cpu" {
                *total = t;
                found = true;
            } else if let Some(n) = parse_u64(&name[3..]) {
                cpu_lines += 1;
                // Keyed by N, not by position: offline CPUs leave zeroed gaps.
                if let Some(vec) = per_cpu.as_deref_mut() {
                    let n = n as usize;
                    if vec.len() <= n {
                        vec.resize(n + 1, CpuTimes::default());
                    }
                    vec[n] = t;
                }
            }
        }
        // PLT-040: CPU hotplug invalidates the topology cache.
        if cpu_lines != self.cpu_lines {
            if self.cpu_lines != 0 {
                self.topo = None;
            }
            self.cpu_lines = cpu_lines;
        }
        found
    }

    fn cpu_counters(&mut self) -> CpuCounters {
        // PLT-052/060
        if !self.stat_shared() {
            return CpuCounters::default();
        }
        let b = &self.stat_buf[..self.stat_len];
        CpuCounters {
            ctx_switches: find_key(b, b"ctxt", b' ').and_then(parse_u64),
            interrupts: find_key(b, b"intr", b' ').and_then(parse_u64),
            soft_interrupts: find_key(b, b"softirq", b' ').and_then(parse_u64),
            syscalls: None,
        }
    }

    fn load_avg(&mut self) -> Option<LoadAvg> {
        // PLT-070
        let n = self.loadavg.read(&mut self.small_buf)?;
        let mut it = fields(&self.small_buf[..n]);
        Some(LoadAvg {
            min1: parse_f64(it.next()?)?,
            min5: parse_f64(it.next()?)?,
            min15: parse_f64(it.next()?)?,
            emulated: false,
        })
    }

    fn memory(&mut self) -> Option<Memory> {
        // PLT-080
        if !self.meminfo_shared() {
            return None;
        }
        let total = self.mem_kb(b"MemTotal")?;
        let free = self.mem_kb(b"MemFree");
        let buffers = self.mem_kb(b"Buffers");
        let cached_raw = self.mem_kb(b"Cached");
        let srecl = self.mem_kb(b"SReclaimable");
        let mut cached = cached_raw.map(|c| c + srecl.unwrap_or(0));
        let mut available = self.mem_kb(b"MemAvailable").or_else(|| {
            Some(free? + buffers.unwrap_or(0) + cached_raw.unwrap_or(0) + srecl.unwrap_or(0))
        })?;
        if let Some(arc) = self.arcstats.as_mut() {
            if let Some(n) = arc.read(&mut self.small_buf) {
                let b = &self.small_buf[..n];
                let get = |k: &[u8]| {
                    lines(b).find_map(|l| {
                        let mut it = fields(l);
                        (it.next()? == k).then(|| it.nth(1).and_then(parse_u64)).flatten()
                    })
                };
                if let (Some(size), Some(cmin)) = (get(b"size"), get(b"c_min")) {
                    cached = Some(cached.unwrap_or(0) + size);
                    available += size.saturating_sub(cmin);
                }
            }
        }
        Some(Memory {
            total,
            available: available.min(total),
            free,
            active: self.mem_kb(b"Active"),
            inactive: self.mem_kb(b"Inactive"),
            buffers,
            cached,
            shared: self.mem_kb(b"Shmem"),
            wired: None,
        })
    }

    fn swap(&mut self) -> Option<Swap> {
        // PLT-090
        if !self.meminfo_shared() {
            return None;
        }
        let total = self.mem_kb(b"SwapTotal")?;
        let free = self.mem_kb(b"SwapFree").unwrap_or(0).min(total);
        Some(Swap { total, used: total - free, free })
    }

    fn net_interfaces(&mut self, want_addrs: bool, out: &mut Vec<NetIf>) {
        self.dev.net_interfaces(want_addrs, out)
    }

    fn disk_io(&mut self, out: &mut Vec<DiskIo>) {
        self.dev.disk_io(out)
    }

    fn filesystems(&mut self, out: &mut Vec<Mount>) {
        self.dev.filesystems(out)
    }

    fn fs_usage(mount_point: &str) -> Option<FsUsage> {
        devices::fs_usage(mount_point)
    }

    fn sensors(&mut self, want_temp: bool, want_fan: bool, out: &mut Vec<Sensor>) {
        self.sens.sensors(want_temp, want_fan, out)
    }

    fn batteries(&mut self, out: &mut Vec<Battery>) {
        self.sens.batteries(out)
    }

    fn processes(&mut self, fields: ProcFields, visible: &[u32], out: &mut Vec<Proc>) {
        self.procs.processes(fields, visible, out)
    }

    fn process_cmdline(&mut self, pid: u32, start_time: u64) -> Option<Vec<String>> {
        self.procs.cmdline(pid, start_time)
    }

    fn username(&mut self, uid: u32) -> Option<String> {
        procs::username(uid)
    }

    fn process_extended(&mut self, pid: u32, start_time: u64, want_sockets: bool) -> Option<ProcExtended> {
        self.procs.extended(pid, start_time, want_sockets)
    }

    fn process_signal(&mut self, pid: u32, start_time: u64, sig: Signal) -> Result<(), ActionError> {
        self.procs.signal(pid, start_time, sig)
    }

    fn process_set_priority(&mut self, pid: u32, start_time: u64, delta: i32) -> Result<(), ActionError> {
        self.procs.set_priority(pid, start_time, delta)
    }

    fn pid_max(&mut self) -> Option<u32> {
        // PLT-128 (C0)
        if self.pid_max.is_none() {
            let v = read_path("/proc/sys/kernel/pid_max", &mut self.small_buf)
                .and_then(|n| parse_u64(&self.small_buf[..n]))
                .map(|v| v.min(u32::MAX as u64) as u32);
            self.pid_max = Some(v);
        }
        self.pid_max.flatten()
    }

    fn reset_caches(&mut self) {
        self.topo = None;
        self.model_given = false;
        self.freq_cur = None;
        self.freq_cache = None;
        self.boot = None;
        self.dev.reset();
        self.sens.reset();
        self.procs.reset();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn pread_small() {
        let f = std::fs::File::open("/proc/self/stat").unwrap();
        let mut b = Vec::new();
        let n = super::pread_into(&f, &mut b, false).unwrap();
        assert!(n > 0 && b[..n].contains(&b')'));
    }
}
