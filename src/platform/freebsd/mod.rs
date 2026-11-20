//! FreeBSD backend (06-platform-sources.md, FreeBSD rows; tier 1).

use std::fmt::Write as _;
use std::mem::{size_of, zeroed};
use std::ptr;
use std::time::{Duration, Instant};

use libc::{c_char, c_int, c_long};

use super::bsd_common::{self as bc, Mib, CTL_KERN};
use crate::platform::{
    ActionError, Battery, BatteryStatus, CpuCounters, CpuFreq, CpuTimes, CpuTopology, DiskIo,
    DiskKind, FsUsage, LoadAvg, Memory, Mount, NetIf, Platform, Proc, ProcExtended, ProcFields,
    Sensor, SensorKind, Signal, Swap, SystemInfo,
};

// sys/sysctl.h
const KERN_PROC: c_int = 14;
const KERN_PROC_PID: c_int = 1;
const KERN_PROC_ARGS: c_int = 7;
const KERN_PROC_PROC: c_int = 8;
const KERN_PROC_FILEDESC: c_int = 33;
const KERN_BOOTTIME: c_int = 21;
// sys/proc.h
const P_KPROC: c_long = 0x4;
const P_SYSTEM: c_long = 0x200;
// sys/devicestat.h
const DEVSTAT_READ: usize = 1;
const DEVSTAT_WRITE: usize = 2;
const DEVSTAT_TYPE_MASK: u32 = 0xf;
const DEVSTAT_TYPE_DIRECT: u32 = 0;
const DEVSTAT_TYPE_PASS: u32 = 0x100;
// sys/user.h / sys/socket.h / netinet/in.h
const KF_TYPE_SOCKET: i32 = 2;
const IPPROTO_TCP: i32 = 6;
const IPPROTO_UDP: i32 = 17;

const FREQ_TTL: Duration = Duration::from_secs(4);

/// `struct devstat` from sys/devicestat.h (FreeBSD 5+, DEVSTAT_VERSION 6). Declared here
/// because libc models `device_type`/`flags`/`priority` as Rust enums, and the kernel
/// sets bit combinations that are not valid discriminants.
#[repr(C)]
#[allow(dead_code)]
#[derive(Clone, Copy)]
struct Devstat {
    sequence0: u32,
    allocated: c_int,
    start_count: u32,
    end_count: u32,
    busy_from: Bintime,
    dev_links: *const libc::c_void,
    device_number: u32,
    device_name: [c_char; 16],
    unit_number: c_int,
    bytes: [u64; 4],
    operations: [u64; 4],
    duration: [Bintime; 4],
    busy_time: Bintime,
    creation_time: Bintime,
    block_size: u32,
    tag_types: [u64; 3],
    flags: u32,
    device_type: u32,
    priority: u32,
    id: *const libc::c_void,
    sequence1: u32,
}

#[repr(C)]
#[allow(dead_code)]
#[derive(Clone, Copy)]
struct Bintime {
    sec: libc::time_t,
    frac: u64,
}

fn bintime_ms(b: &Bintime) -> u64 {
    (b.sec as u64) * 1000 + (((b.frac >> 32) * 1000) >> 32)
}

/// MIBs resolved once at start-up (C0); absent nodes stay None.
struct Mibs {
    cp_time: Option<Mib>,
    cp_times: Option<Mib>,
    ncpu: Option<Mib>,
    cores: Option<Mib>,
    model: Option<Mib>,
    freq: Option<Mib>,
    freq_levels: Option<Mib>,
    v_swtch: Option<Mib>,
    v_intr: Option<Mib>,
    v_soft: Option<Mib>,
    v_syscall: Option<Mib>,
    physmem: Option<Mib>,
    pagesize: Option<Mib>,
    v_free: Option<Mib>,
    v_active: Option<Mib>,
    v_inactive: Option<Mib>,
    v_wire: Option<Mib>,
    v_laundry: Option<Mib>,
    v_cache: Option<Mib>,
    bufspace: Option<Mib>,
    arc_size: Option<Mib>,
    arc_cmin: Option<Mib>,
    devstat: Option<Mib>,
    pid_max: Option<Mib>,
    osreldate: Option<Mib>,
    batt_life: Option<Mib>,
    batt_state: Option<Mib>,
    acline: Option<Mib>,
}

impl Mibs {
    fn resolve() -> Mibs {
        Mibs {
            cp_time: Mib::new(c"kern.cp_time"),
            cp_times: Mib::new(c"kern.cp_times"),
            ncpu: Mib::new(c"hw.ncpu"),
            cores: Mib::new(c"kern.smp.cores"),
            model: Mib::new(c"hw.model"),
            freq: Mib::new(c"dev.cpu.0.freq"),
            freq_levels: Mib::new(c"dev.cpu.0.freq_levels"),
            v_swtch: Mib::new(c"vm.stats.sys.v_swtch"),
            v_intr: Mib::new(c"vm.stats.sys.v_intr"),
            v_soft: Mib::new(c"vm.stats.sys.v_soft"),
            v_syscall: Mib::new(c"vm.stats.sys.v_syscall"),
            physmem: Mib::new(c"hw.physmem"),
            pagesize: Mib::new(c"hw.pagesize"),
            v_free: Mib::new(c"vm.stats.vm.v_free_count"),
            v_active: Mib::new(c"vm.stats.vm.v_active_count"),
            v_inactive: Mib::new(c"vm.stats.vm.v_inactive_count"),
            v_wire: Mib::new(c"vm.stats.vm.v_wire_count"),
            v_laundry: Mib::new(c"vm.stats.vm.v_laundry_count"),
            v_cache: Mib::new(c"vm.stats.vm.v_cache_count"),
            bufspace: Mib::new(c"vfs.bufspace"),
            arc_size: Mib::new(c"kstat.zfs.misc.arcstats.size"),
            arc_cmin: Mib::new(c"kstat.zfs.misc.arcstats.c_min"),
            devstat: Mib::new(c"kern.devstat.all"),
            pid_max: Mib::new(c"kern.pid_max"),
            osreldate: Mib::new(c"kern.osreldate"),
            batt_life: Mib::new(c"hw.acpi.battery.life"),
            batt_state: Mib::new(c"hw.acpi.battery.state"),
            acline: Mib::new(c"hw.acpi.acline"),
        }
    }
}

fn rd_u64(m: &Option<Mib>) -> Option<u64> {
    m.as_ref().and_then(|m| bc::sysctl_u64(m.get()))
}

fn rd_int(m: &Option<Mib>) -> Option<i64> {
    m.as_ref().and_then(|m| bc::sysctl_int(m.get()))
}

/// deci-Kelvin (sysctl format "IK") to Celsius (PLT-133).
fn dk_to_c(v: i64) -> f64 {
    (v - 2731) as f64 / 10.0
}

struct TempSrc {
    mib: Mib,
    chip: &'static str,
    label: String,
    high: Option<f64>,
    critical: Option<f64>,
}

pub struct Sys {
    m: Mibs,
    page: u64,
    hz: f64,
    /// `v_cache_count` is meaningful only up to FreeBSD 11 (PLT-082).
    has_v_cache: bool,
    cp: Vec<c_long>,
    procs: Vec<libc::kinfo_proc>,
    devbuf: Vec<u8>,
    fdbuf: Vec<u8>,
    argbuf: Vec<u8>,
    fsbuf: Vec<libc::statfs>,
    kvm: *mut libc::kvm_t,
    kvm_tried: bool,
    freq: Option<(Instant, CpuFreq)>,
    max_hz: Option<Option<u64>>,
    model: Option<Option<String>>,
    temps: Option<Vec<TempSrc>>,
}

// SAFETY: the raw pointers (kvm handle, buffers of kinfo_proc) are owned by Sys and only
// used through &mut self; nothing is shared between threads.
unsafe impl Send for Sys {}

pub fn new() -> Sys {
    let m = Mibs::resolve();
    let page = rd_u64(&m.pagesize).filter(|&p| p > 0).unwrap_or(4096);
    let has_v_cache = rd_int(&m.osreldate).map_or(false, |v| v < 1_200_000);
    Sys {
        m,
        page,
        hz: bc::stathz(),
        has_v_cache,
        cp: Vec::new(),
        procs: Vec::new(),
        devbuf: Vec::new(),
        fdbuf: Vec::new(),
        argbuf: Vec::new(),
        fsbuf: Vec::new(),
        kvm: ptr::null_mut(),
        kvm_tried: false,
        freq: None,
        max_hz: None,
        model: None,
        temps: None,
    }
}

impl Drop for Sys {
    fn drop(&mut self) {
        if !self.kvm.is_null() {
            // SAFETY: handle from kvm_open, closed once.
            unsafe { libc::kvm_close(self.kvm) };
        }
    }
}

impl Sys {
    /// One `kinfo_proc` for `pid` (C3, actions and pinned process only).
    fn kinfo(&self, pid: u32) -> Option<libc::kinfo_proc> {
        // SAFETY: kinfo_proc is POD (integers, arrays and raw pointers).
        let mut kp: libc::kinfo_proc = unsafe { zeroed() };
        let n = bc::sysctl_pod(&[CTL_KERN, KERN_PROC, KERN_PROC_PID, pid as c_int], &mut kp)?;
        (n >= size_of::<libc::kinfo_proc>() && kp.ki_pid as u32 == pid).then_some(kp)
    }

    fn cpu_conv(&self, c: &[c_long]) -> CpuTimes {
        // kern.cp_time order: user, nice, sys, intr, idle (PLT-054).
        let t = |i: usize| c[i] as u64 as f64 / self.hz;
        CpuTimes {
            user: t(0),
            nice: Some(t(1)),
            system: t(2),
            irq: Some(t(3)),
            idle: t(4),
            ..CpuTimes::default()
        }
    }

    fn discover_temps(&self) -> Vec<TempSrc> {
        let mut v = Vec::new();
        let ncpu = rd_int(&self.m.ncpu).unwrap_or(1).clamp(1, 1024);
        let mut name = String::new();
        for i in 0..ncpu {
            name.clear();
            let _ = write!(name, "dev.cpu.{i}.temperature");
            let Some(mib) = Mib::named(&name) else { continue };
            name.clear();
            let _ = write!(name, "dev.cpu.{i}.coretemp.tjmax");
            let high = Mib::named(&name).and_then(|m| bc::sysctl_int(m.get())).map(dk_to_c);
            v.push(TempSrc { mib, chip: "coretemp", label: format!("Core {i}"), high, critical: None });
        }
        for i in 0..64 {
            name.clear();
            let _ = write!(name, "hw.acpi.thermal.tz{i}.temperature");
            let Some(mib) = Mib::named(&name) else { break };
            name.clear();
            let _ = write!(name, "hw.acpi.thermal.tz{i}._CRT");
            let critical = Mib::named(&name)
                .and_then(|m| bc::sysctl_int(m.get()))
                .filter(|&v| v > 0)
                .map(dk_to_c);
            v.push(TempSrc { mib, chip: "acpitz", label: format!("tz{i}"), high: None, critical });
        }
        v
    }
}

/// PLT-150 FreeBSD `ki_stat`.
fn status(stat: u8) -> u8 {
    match stat {
        1 => b'I', // SIDL
        2 => b'R', // SRUN
        3 => b'S', // SSLEEP
        4 => b'T', // SSTOP
        5 => b'Z', // SZOMB
        6 => b'W', // SWAIT
        7 => b'L', // SLOCK
        _ => b'S',
    }
}

impl Platform for Sys {
    fn system_info(&mut self) -> SystemInfo {
        bc::system_info()
    }

    fn boot_time(&mut self) -> Option<u64> {
        // PLT-031: struct timeval.
        let mut tv: libc::timeval = unsafe { zeroed() };
        bc::sysctl_pod(&[CTL_KERN, KERN_BOOTTIME], &mut tv)?;
        (tv.tv_sec > 0).then_some(tv.tv_sec as u64)
    }

    fn cpu_topology(&mut self) -> CpuTopology {
        // PLT-043
        let logical = rd_int(&self.m.ncpu).filter(|&n| n > 0).unwrap_or(1) as u32;
        let physical = rd_int(&self.m.cores).filter(|&n| n > 0).map(|n| n as u32);
        CpuTopology { logical, physical }
    }

    fn cpu_freq(&mut self) -> CpuFreq {
        if let Some((t, f)) = &self.freq {
            if t.elapsed() < FREQ_TTL {
                return f.clone();
            }
        }
        if self.model.is_none() {
            self.model = Some(self.m.model.as_ref().and_then(|m| bc::sysctl_string(m.get())));
        }
        if self.max_hz.is_none() {
            // First entry of "MHz/power MHz/power ..." is the highest level.
            let lv = self.m.freq_levels.as_ref().and_then(|m| bc::sysctl_string(m.get()));
            self.max_hz = Some(lv.and_then(|s| {
                let first = s.split(|c: char| c == '/' || c.is_whitespace()).next()?;
                first.parse::<u64>().ok().filter(|&v| v > 0).map(|v| v * 1_000_000)
            }));
        }
        let cur = rd_int(&self.m.freq).filter(|&v| v > 0).map(|v| v as u64 * 1_000_000);
        let f = CpuFreq { model: self.model.clone().flatten(), current_hz: cur, max_hz: self.max_hz.flatten() };
        self.freq = Some((Instant::now(), f.clone()));
        f
    }

    fn cpu_times(&mut self, total: &mut CpuTimes, per_cpu: Option<&mut Vec<CpuTimes>>) -> bool {
        // PLT-054
        let Some(m) = self.m.cp_time else { return false };
        let mut cp = std::mem::take(&mut self.cp);
        let ok = bc::sysctl_table(m.get(), None, &mut cp) && cp.len() >= 5;
        if ok {
            *total = self.cpu_conv(&cp[..5]);
            if let Some(v) = per_cpu {
                v.clear();
                if let Some(pm) = self.m.cp_times {
                    if bc::sysctl_table(pm.get(), None, &mut cp) {
                        for c in cp.chunks_exact(5) {
                            v.push(self.cpu_conv(c));
                        }
                    }
                }
            }
        }
        self.cp = cp;
        ok
    }

    fn cpu_counters(&mut self) -> CpuCounters {
        CpuCounters {
            ctx_switches: rd_u64(&self.m.v_swtch),
            interrupts: rd_u64(&self.m.v_intr),
            soft_interrupts: rd_u64(&self.m.v_soft),
            syscalls: rd_u64(&self.m.v_syscall),
        }
    }

    fn load_avg(&mut self) -> Option<LoadAvg> {
        bc::load_avg()
    }

    fn memory(&mut self) -> Option<Memory> {
        // PLT-082
        let total = rd_u64(&self.m.physmem)?;
        let pg = self.page;
        let free = rd_u64(&self.m.v_free)? * pg;
        let active = rd_u64(&self.m.v_active).map(|v| v * pg);
        let inactive = rd_u64(&self.m.v_inactive).map(|v| v * pg);
        let wired = rd_u64(&self.m.v_wire).map(|v| v * pg);
        let laundry = rd_u64(&self.m.v_laundry).map_or(0, |v| v * pg);
        let mut cached = if self.has_v_cache { rd_u64(&self.m.v_cache).map(|v| v * pg) } else { None };
        let mut available = inactive.unwrap_or(0) + laundry + cached.unwrap_or(0) + free;
        if let Some(size) = rd_u64(&self.m.arc_size) {
            let cmin = rd_u64(&self.m.arc_cmin).unwrap_or(0);
            cached = Some(cached.unwrap_or(0) + size);
            available += size.saturating_sub(cmin);
        }
        Some(Memory {
            total,
            available: available.min(total),
            free: Some(free),
            active,
            inactive,
            buffers: rd_u64(&self.m.bufspace),
            cached,
            shared: None,
            wired,
        })
    }

    fn swap(&mut self) -> Option<Swap> {
        // PLT-092: kvm_getswapinfo on a /dev/null kvm handle works unprivileged.
        if self.kvm.is_null() && !self.kvm_tried {
            self.kvm_tried = true;
            // SAFETY: errstr NULL means kvm reports nothing on stderr (GEN-014).
            self.kvm = unsafe {
                libc::kvm_open(ptr::null(), c"/dev/null".as_ptr(), ptr::null(), libc::O_RDONLY, ptr::null())
            };
        }
        if self.kvm.is_null() {
            return None;
        }
        // SAFETY: kvm_swap is POD; with maxswap 1 only the total is written to entry 0.
        let mut sw: [libc::kvm_swap; 1] = unsafe { zeroed() };
        let n = unsafe { libc::kvm_getswapinfo(self.kvm, sw.as_mut_ptr(), 1, 0) };
        if n < 0 {
            return None;
        }
        if n == 0 {
            return Some(Swap::default());
        }
        let total = sw[0].ksw_total as u64 * self.page;
        let used = sw[0].ksw_used as u64 * self.page;
        Some(Swap { total, used, free: total.saturating_sub(used) })
    }

    fn net_interfaces(&mut self, want_addrs: bool, out: &mut Vec<NetIf>) {
        bc::net_interfaces(want_addrs, out);
    }

    fn disk_io(&mut self, out: &mut Vec<DiskIo>) {
        // PLT-112: raw kern.devstat.all = `long generation` + struct devstat[].
        let Some(m) = self.m.devstat else {
            out.clear();
            return;
        };
        if !bc::sysctl_table(m.get(), None, &mut self.devbuf) {
            out.clear();
            return;
        }
        let hdr = size_of::<c_long>();
        let sz = size_of::<Devstat>();
        let b = &self.devbuf;
        let mut n = 0;
        if b.len() >= hdr && (b.len() - hdr) % sz == 0 {
            for rec in b[hdr..].chunks_exact(sz) {
                // SAFETY: rec holds exactly one devstat record; read unaligned from bytes.
                let d: Devstat = unsafe { ptr::read_unaligned(rec.as_ptr() as *const Devstat) };
                if d.device_type & DEVSTAT_TYPE_MASK != DEVSTAT_TYPE_DIRECT
                    || d.device_type & DEVSTAT_TYPE_PASS != 0
                {
                    continue;
                }
                if n == out.len() {
                    out.push(DiskIo::default());
                }
                let e = &mut out[n];
                n += 1;
                bc::set_str(&mut e.name, bc::cbytes(&d.device_name));
                let _ = write!(e.name, "{}", d.unit_number);
                e.kind = DiskKind::Disk;
                e.read_count = d.operations[DEVSTAT_READ];
                e.write_count = d.operations[DEVSTAT_WRITE];
                e.read_bytes = d.bytes[DEVSTAT_READ];
                e.write_bytes = d.bytes[DEVSTAT_WRITE];
                e.read_time_ms = Some(bintime_ms(&d.duration[DEVSTAT_READ]));
                e.write_time_ms = Some(bintime_ms(&d.duration[DEVSTAT_WRITE]));
            }
        }
        out.truncate(n);
    }

    fn filesystems(&mut self, out: &mut Vec<Mount>) {
        bc::filesystems(&mut self.fsbuf, out);
    }

    fn fs_usage(mount_point: &str) -> Option<FsUsage> {
        bc::fs_usage(mount_point)
    }

    fn sensors(&mut self, want_temp: bool, _want_fan: bool, out: &mut Vec<Sensor>) {
        // PLT-133: temperatures only; fans are absent on FreeBSD.
        if !want_temp {
            out.clear();
            return;
        }
        if self.temps.is_none() {
            self.temps = Some(self.discover_temps());
        }
        let mut n = 0;
        for t in self.temps.as_ref().into_iter().flatten() {
            if let Some(v) = bc::sysctl_int(t.mib.get()) {
                bc::put_sensor(out, &mut n, SensorKind::Temperature, t.chip, &t.label, dk_to_c(v), t.high, t.critical);
            }
        }
        out.truncate(n);
    }

    fn batteries(&mut self, out: &mut Vec<Battery>) {
        // PLT-133: hw.acpi.battery.* report all batteries combined.
        let Some(life) = rd_int(&self.m.batt_life).filter(|&v| (0..=100).contains(&v)) else {
            out.clear();
            return;
        };
        let state = rd_int(&self.m.batt_state);
        let acline = rd_int(&self.m.acline);
        let status = match state {
            Some(s) if s & 2 != 0 => BatteryStatus::Charging,
            Some(s) if s & 1 != 0 => BatteryStatus::Discharging,
            _ => match acline {
                Some(1) if life >= 100 => BatteryStatus::Full,
                Some(1) => BatteryStatus::NotCharging,
                Some(0) => BatteryStatus::Discharging,
                _ => BatteryStatus::Unknown,
            },
        };
        bc::put_battery(out, life as f64, status);
    }

    fn processes(&mut self, _fields: ProcFields, _visible: &[u32], out: &mut Vec<Proc>) {
        // PLT-151: one sysctl for all processes; I/O is absent (PLT-153).
        if !bc::sysctl_table(&[CTL_KERN, KERN_PROC, KERN_PROC_PROC], None, &mut self.procs)
            || self.procs.first().map_or(false, |k| k.ki_structsize as usize != size_of::<libc::kinfo_proc>())
        {
            out.clear();
            return;
        }
        let page = self.page;
        let mut n = 0;
        for kp in &self.procs {
            let comm = bc::cbytes(&kp.ki_comm);
            // PLT-141: drop the kernel idle process.
            if kp.ki_flag & P_SYSTEM != 0 && comm == b"idle" {
                continue;
            }
            let p = bc::proc_slot(out, n);
            n += 1;
            p.pid = kp.ki_pid as u32;
            p.ppid = Some(kp.ki_ppid as u32);
            p.start_time = kp.ki_start.tv_sec as u64;
            bc::set_str(&mut p.name, comm);
            p.uid = Some(kp.ki_uid as u32);
            p.status = status(kp.ki_stat as u8);
            p.nice = Some(kp.ki_nice as i32);
            p.priority_class = None;
            p.num_threads = kp.ki_numthreads.max(0) as u32;
            let ru = &kp.ki_rusage;
            p.cpu_user = bc::tv_secs(ru.ru_utime.tv_sec as i64, ru.ru_utime.tv_usec as i64);
            p.cpu_system = bc::tv_secs(ru.ru_stime.tv_sec as i64, ru.ru_stime.tv_usec as i64);
            p.rss = (kp.ki_rssize.max(0) as u64) * page;
            p.vms = Some(kp.ki_size as u64);
            p.io_read = None;
            p.io_write = None;
            p.last_cpu = (kp.ki_lastcpu >= 0).then_some(kp.ki_lastcpu as u32);
            p.is_kernel_thread = kp.ki_flag & P_KPROC != 0;
        }
        out.truncate(n);
    }

    fn process_cmdline(&mut self, pid: u32, _start_time: u64) -> Option<Vec<String>> {
        // PLT-152
        if !bc::sysctl_table(&[CTL_KERN, KERN_PROC, KERN_PROC_ARGS, pid as c_int], None, &mut self.argbuf) {
            return None;
        }
        bc::split_nul(&self.argbuf)
    }

    fn username(&mut self, uid: u32) -> Option<String> {
        bc::username(uid)
    }

    fn process_extended(&mut self, pid: u32, start_time: u64, want_sockets: bool) -> Option<ProcExtended> {
        // Section 16, FreeBSD column. Identity first.
        let kp = self.kinfo(pid)?;
        if kp.ki_start.tv_sec as u64 != start_time {
            return None;
        }
        let mut x = ProcExtended {
            ctx_voluntary: Some(kp.ki_rusage.ru_nvcsw as u64),
            ctx_involuntary: Some(kp.ki_rusage.ru_nivcsw as u64),
            ..ProcExtended::default()
        };
        // SAFETY: cpuset_t is a plain bit array, filled by the call.
        let mut set: libc::cpuset_t = unsafe { zeroed() };
        let r = unsafe {
            libc::cpuset_getaffinity(
                libc::CPU_LEVEL_WHICH,
                libc::CPU_WHICH_PID,
                pid as libc::id_t,
                size_of::<libc::cpuset_t>(),
                &mut set,
            )
        };
        if r == 0 {
            // SAFETY: counts bits of an initialised set.
            let c = unsafe { libc::CPU_COUNT(&set) };
            x.affinity_cores = (c > 0).then_some(c as u32);
        }
        // KERN_PROC_FILEDESC needs p_candebug: other users' processes fail unless root.
        if bc::sysctl_table(&[CTL_KERN, KERN_PROC, KERN_PROC_FILEDESC, pid as c_int], None, &mut self.fdbuf) {
            let (mut fds, mut tcp, mut udp) = (0u32, 0u32, 0u32);
            let b = &self.fdbuf;
            let rd = |o: usize| i32::from_ne_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
            let mut off = 0;
            // struct kinfo_file: kf_structsize@0, kf_type@4, kf_fd@8; the socket part of
            // the union starts at 32: domain@36, type@40, protocol@44 (sys/user.h).
            while off + 48 <= b.len() {
                let size = rd(off);
                if size <= 0 {
                    break;
                }
                if rd(off + 8) >= 0 {
                    fds += 1;
                    if rd(off + 4) == KF_TYPE_SOCKET {
                        let dom = rd(off + 36);
                        if dom == libc::AF_INET || dom == libc::AF_INET6 {
                            match rd(off + 44) {
                                IPPROTO_TCP => tcp += 1,
                                IPPROTO_UDP => udp += 1,
                                _ => {}
                            }
                        }
                    }
                }
                off += size as usize;
            }
            x.num_fds = Some(fds);
            if want_sockets {
                x.tcp = Some(tcp);
                x.udp = Some(udp);
            }
        }
        Some(x)
    }

    fn process_signal(&mut self, pid: u32, start_time: u64, sig: Signal) -> Result<(), ActionError> {
        let cur = self.kinfo(pid).map(|k| k.ki_start.tv_sec as u64);
        bc::signal(pid, cur, start_time, sig)
    }

    fn process_set_priority(&mut self, pid: u32, start_time: u64, delta: i32) -> Result<(), ActionError> {
        let cur = self.kinfo(pid).map(|k| (k.ki_start.tv_sec as u64, k.ki_nice as i32));
        bc::renice(pid, cur, start_time, delta)
    }

    fn pid_max(&mut self) -> Option<u32> {
        rd_int(&self.m.pid_max).filter(|&v| v > 0).map(|v| v as u32)
    }

    fn reset_caches(&mut self) {
        self.freq = None;
        self.max_hz = None;
        self.model = None;
        self.temps = None;
        self.hz = bc::stathz();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(dk_to_c(3231), 50.0);
        assert_eq!(bintime_ms(&Bintime { sec: 2, frac: 1 << 63 }), 2500);
        assert_eq!(status(2), b'R');
    }
}
