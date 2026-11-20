//! OpenBSD backend (06-platform-sources.md, OpenBSD notes; best effort, OQ-2).

use std::mem::{size_of, zeroed};
use std::time::{Duration, Instant};

use libc::{c_char, c_int};

use super::bsd_common::{self as bc, Wide, CTL_HW, CTL_KERN, CTL_VM, HW_MODEL, HW_NCPU};
use crate::platform::{
    ActionError, Battery, BatteryStatus, CpuCounters, CpuFreq, CpuTimes, CpuTopology, DiskIo,
    DiskKind, FsUsage, LoadAvg, Memory, Mount, NetIf, Platform, Proc, ProcExtended, ProcFields,
    Sensor, SensorKind, Signal, Swap, SystemInfo,
};

// sys/sysctl.h
const KERN_BOOTTIME: c_int = 21;
const KERN_PROC_ARGS: c_int = 55;
const KERN_PROC: c_int = 66;
const KERN_CPTIME2: c_int = 71;
const KERN_CPUSTATS: c_int = 85;
const KERN_PROC_PID: c_int = 1;
const KERN_PROC_KTHREAD: c_int = 7; // all processes, kernel threads included
const KERN_PROC_ARGV: c_int = 1;
const HW_DISKSTATS: c_int = 9;
const HW_SENSORS: c_int = 11;
const HW_CPUSPEED: c_int = 12;
const HW_PHYSMEM64: c_int = 19;
const HW_NCPUFOUND: c_int = 21;
const HW_NCPUONLINE: c_int = 25;
// uvm/uvmexp.h
const VM_UVMEXP: c_int = 4;
// sys/mount.h
const CTL_VFS: c_int = 10;
const VFS_GENERIC: c_int = 0;
const VFS_BCACHESTAT: c_int = 3;
// sys/proc.h
const P_SYSTEM: i32 = 0x200;
const PS_SYSTEM: u32 = 0x10000;
const NZERO: i32 = 20;
// sys/sysctl.h KI_NOCPU
const KI_NOCPU: u64 = !0;
// sys/sched.h
const CPUSTATS_ONLINE: u64 = 0x1;
// sys/sensors.h
const SENSOR_TEMP: c_int = 0;
const SENSOR_FANRPM: c_int = 1;
const SENSOR_FINVALID: c_int = 0x1;
const SENSOR_FUNKNOWN: c_int = 0x2;
// machine/apmvar.h: _IOR('A', 3, struct apm_power_info) with a 32-byte struct.
const APM_IOC_GETPOWER: libc::c_ulong = 0x4020_4103;
const APM_BATT_CHARGING: u8 = 3;
const APM_BATTERY_ABSENT: u8 = 4;
const APM_AC_OFF: u8 = 0;
const APM_AC_ON: u8 = 1;

// Indices into `struct uvmexp` viewed as int[] (uvm/uvmexp.h, OpenBSD 7.x layout).
const UVM_PAGESIZE: usize = 0;
const UVM_FREE: usize = 4;
const UVM_ACTIVE: usize = 5;
const UVM_INACTIVE: usize = 6;
const UVM_WIRED: usize = 8;
const UVM_SWPAGES: usize = 26;
const UVM_SWPGINUSE: usize = 27;
const UVM_INTRS: usize = 35;
const UVM_SWTCH: usize = 36;
const UVM_SOFTS: usize = 37;
const UVM_SYSCALLS: usize = 38;

const FREQ_TTL: Duration = Duration::from_secs(4);

/// `struct cpustats` (sys/sched.h).
#[repr(C)]
#[allow(dead_code)]
#[derive(Clone, Copy)]
struct CpuStats {
    cs_time: [u64; 6],
    cs_flags: u64,
}

/// `struct diskstats` (sys/disk.h).
#[repr(C)]
#[allow(dead_code)]
#[derive(Clone, Copy)]
struct DiskStats {
    ds_name: [c_char; 16],
    ds_busy: c_int,
    ds_rxfer: u64,
    ds_wxfer: u64,
    ds_seek: u64,
    ds_rbytes: u64,
    ds_wbytes: u64,
    ds_attachtime: libc::timeval,
    ds_timestamp: libc::timeval,
    ds_time: libc::timeval,
}

/// `struct sensordev` (sys/sensors.h). `maxnumt` is sized generously; the kernel copies
/// its own (smaller) struct and only the first entries are read.
#[repr(C)]
#[allow(dead_code)]
#[derive(Clone, Copy)]
struct SensorDev {
    num: c_int,
    xname: [c_char; 16],
    maxnumt: [c_int; 32],
    sensors_count: c_int,
}

/// `struct sensor` (sys/sensors.h).
#[repr(C)]
#[allow(dead_code)]
#[derive(Clone, Copy)]
struct SensorRec {
    desc: [c_char; 32],
    tv: libc::timeval,
    value: i64,
    type_: c_int,
    status: c_int,
    numt: c_int,
    flags: c_int,
}

/// `struct apm_power_info` (machine/apmvar.h).
#[repr(C)]
#[allow(dead_code)]
#[derive(Clone, Copy, Default)]
struct ApmPowerInfo {
    battery_state: u8,
    ac_state: u8,
    battery_life: u8,
    spare1: u8,
    minutes_left: u32,
    spare2: [u32; 6],
}

struct SensorSrc {
    mib: [c_int; 5],
    kind: SensorKind,
    chip: String,
    label: String,
}

pub struct Sys {
    page: u64,
    hz: f64,
    /// None until probed; Some(true) when `kern.cpustats` works (OpenBSD 6.4+).
    use_cpustats: Option<bool>,
    /// Room for `struct uvmexp` with headroom: sysctl_rdstruct fails with ENOMEM when
    /// the buffer is smaller than the kernel struct.
    uvm: [c_int; 256],
    procs: Vec<libc::kinfo_proc>,
    argbuf: Vec<u8>,
    disks: Vec<DiskStats>,
    fsbuf: Vec<libc::statfs>,
    ctx: Wide,
    intr: Wide,
    soft: Wide,
    sysc: Wide,
    freq: Option<(Instant, CpuFreq)>,
    model: Option<Option<String>>,
    sensors: Option<Vec<SensorSrc>>,
    apm_fd: c_int,
    apm_tried: bool,
}

pub fn new() -> Sys {
    let page = bc::sysctl_int(&[CTL_HW, bc::HW_PAGESIZE]).filter(|&p| p > 0).unwrap_or(4096) as u64;
    Sys {
        page,
        hz: bc::stathz(),
        use_cpustats: None,
        uvm: [0; 256],
        procs: Vec::new(),
        argbuf: Vec::new(),
        disks: Vec::new(),
        fsbuf: Vec::new(),
        ctx: Wide::default(),
        intr: Wide::default(),
        soft: Wide::default(),
        sysc: Wide::default(),
        freq: None,
        model: None,
        sensors: None,
        apm_fd: -1,
        apm_tried: false,
    }
}

impl Drop for Sys {
    fn drop(&mut self) {
        if self.apm_fd >= 0 {
            // SAFETY: fd opened by us, closed once.
            unsafe { libc::close(self.apm_fd) };
        }
    }
}

/// PLT-145-style argv for OpenBSD: an array of `char *` (NULL-terminated) followed by
/// the strings. The pointers are not dereferenced; the strings are read in order.
fn parse_argv(b: &[u8]) -> Option<Vec<String>> {
    let psz = size_of::<usize>();
    let mut n = 0;
    loop {
        let o = n * psz;
        if o + psz > b.len() {
            return None;
        }
        if b[o..o + psz].iter().all(|&c| c == 0) {
            break;
        }
        n += 1;
    }
    if n == 0 {
        return None;
    }
    let strs = &b[(n + 1) * psz..];
    let v: Vec<String> =
        strs.split(|&c| c == 0).take(n).map(|s| String::from_utf8_lossy(s).into_owned()).collect();
    (!v.is_empty()).then_some(v)
}

impl Sys {
    fn read_uvm(&mut self) -> bool {
        bc::sysctl_pod(&[CTL_VM, VM_UVMEXP], &mut self.uvm).map_or(false, |n| n > UVM_SYSCALLS * 4)
    }

    fn kinfo(&self, pid: u32) -> Option<libc::kinfo_proc> {
        // SAFETY: kinfo_proc is POD.
        let mut kp: libc::kinfo_proc = unsafe { zeroed() };
        let sz = size_of::<libc::kinfo_proc>() as c_int;
        let n = bc::sysctl_pod(&[CTL_KERN, KERN_PROC, KERN_PROC_PID, pid as c_int, sz, 1], &mut kp)?;
        (n > 0 && kp.p_pid as u32 == pid).then_some(kp)
    }

    fn ncpu_found(&self) -> u32 {
        bc::sysctl_int(&[CTL_HW, HW_NCPUFOUND])
            .or_else(|| bc::sysctl_int(&[CTL_HW, HW_NCPU]))
            .filter(|&n| n > 0)
            .unwrap_or(1) as u32
    }

    /// Per-CPU raw ticks for CPU `i`; None when the CPU is offline or unreadable.
    fn cpu_ticks(&mut self, i: u32) -> Option<[u64; 6]> {
        if self.use_cpustats != Some(false) {
            // SAFETY: CpuStats is POD.
            let mut cs: CpuStats = unsafe { zeroed() };
            match bc::sysctl_pod(&[CTL_KERN, KERN_CPUSTATS, i as c_int], &mut cs) {
                Some(_) => {
                    self.use_cpustats = Some(true);
                    return (cs.cs_flags & CPUSTATS_ONLINE != 0).then_some(cs.cs_time);
                }
                // Older kernels without kern.cpustats: fall back to kern.cp_time2.
                None if self.use_cpustats.is_none() => self.use_cpustats = Some(false),
                None => return None,
            }
        }
        let mut t = [0u64; 6];
        bc::sysctl_pod(&[CTL_KERN, KERN_CPTIME2, i as c_int], &mut t)?;
        Some(t)
    }

    fn conv(&self, t: &[u64; 6]) -> CpuTimes {
        // CPUSTATES 6: user, nice, sys, spin, intr, idle; spin is added to system (PLT-054).
        let s = |v: u64| v as f64 / self.hz;
        CpuTimes {
            user: s(t[0]),
            nice: Some(s(t[1])),
            system: s(t[2] + t[3]),
            irq: Some(s(t[4])),
            idle: s(t[5]),
            ..CpuTimes::default()
        }
    }

    fn discover_sensors(&self) -> Vec<SensorSrc> {
        let mut v = Vec::new();
        for dev in 0..1024 {
            // SAFETY: SensorDev is POD.
            let mut sd: SensorDev = unsafe { zeroed() };
            if bc::sysctl_pod(&[CTL_HW, HW_SENSORS, dev], &mut sd).is_none() {
                match bc::errno() {
                    libc::ENXIO => continue, // hole in the device numbering
                    _ => break,              // ENOENT: no more devices
                }
            }
            let chip = bc::cstring(&sd.xname);
            for (ty, kind) in [(SENSOR_TEMP, SensorKind::Temperature), (SENSOR_FANRPM, SensorKind::Fan)] {
                for n in 0..sd.maxnumt[ty as usize].max(0) {
                    let mib = [CTL_HW, HW_SENSORS, dev, ty, n];
                    // SAFETY: SensorRec is POD.
                    let mut s: SensorRec = unsafe { zeroed() };
                    if bc::sysctl_pod(&mib, &mut s).is_none() || s.flags & SENSOR_FINVALID != 0 {
                        continue;
                    }
                    let mut label = bc::cstring(&s.desc);
                    if label.is_empty() {
                        label = format!("{}{}", if ty == SENSOR_TEMP { "temp" } else { "fan" }, n);
                    }
                    v.push(SensorSrc { mib, kind, chip: chip.clone(), label });
                }
            }
        }
        v
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
        // PLT-044
        let logical = bc::sysctl_int(&[CTL_HW, HW_NCPUONLINE])
            .or_else(|| bc::sysctl_int(&[CTL_HW, HW_NCPU]))
            .filter(|&n| n > 0)
            .unwrap_or(1) as u32;
        CpuTopology { logical, physical: None }
    }

    fn cpu_freq(&mut self) -> CpuFreq {
        if let Some((t, f)) = &self.freq {
            if t.elapsed() < FREQ_TTL {
                return f.clone();
            }
        }
        if self.model.is_none() {
            self.model = Some(bc::sysctl_string(&[CTL_HW, HW_MODEL]));
        }
        let cur = bc::sysctl_int(&[CTL_HW, HW_CPUSPEED]).filter(|&v| v > 0).map(|v| v as u64 * 1_000_000);
        let f = CpuFreq { model: self.model.clone().flatten(), current_hz: cur, max_hz: None };
        self.freq = Some((Instant::now(), f.clone()));
        f
    }

    fn cpu_times(&mut self, total: &mut CpuTimes, mut per_cpu: Option<&mut Vec<CpuTimes>>) -> bool {
        // PLT-054: kern.cp_time is an average, so the total is the sum over online CPUs.
        if let Some(v) = per_cpu.as_deref_mut() {
            v.clear();
        }
        let mut sum = [0u64; 6];
        let mut any = false;
        for i in 0..self.ncpu_found() {
            let Some(t) = self.cpu_ticks(i) else { continue };
            any = true;
            for k in 0..6 {
                sum[k] += t[k];
            }
            if let Some(v) = per_cpu.as_deref_mut() {
                v.push(self.conv(&t));
            }
        }
        if any {
            *total = self.conv(&sum);
        }
        any
    }

    fn cpu_counters(&mut self) -> CpuCounters {
        // vm.uvmexp: 32-bit counters widened per PLT-002.
        if !self.read_uvm() {
            return CpuCounters::default();
        }
        let u = &self.uvm;
        CpuCounters {
            ctx_switches: Some(self.ctx.update(u[UVM_SWTCH] as u32)),
            interrupts: Some(self.intr.update(u[UVM_INTRS] as u32)),
            soft_interrupts: Some(self.soft.update(u[UVM_SOFTS] as u32)),
            syscalls: Some(self.sysc.update(u[UVM_SYSCALLS] as u32)),
        }
    }

    fn load_avg(&mut self) -> Option<LoadAvg> {
        bc::load_avg()
    }

    fn memory(&mut self) -> Option<Memory> {
        // PLT-082 OpenBSD
        let total = bc::sysctl_u64(&[CTL_HW, HW_PHYSMEM64])?;
        if !self.read_uvm() {
            return None;
        }
        let pg = if self.uvm[UVM_PAGESIZE] > 0 { self.uvm[UVM_PAGESIZE] as u64 } else { self.page };
        let p = |i: usize| self.uvm[i].max(0) as u64 * pg;
        let free = p(UVM_FREE);
        let inactive = p(UVM_INACTIVE);
        // struct bcachestats: numbufs, numbufpages, ... (int64_t each, sys/mount.h). The
        // buffer must be at least the kernel struct size (about 20 fields), else ENOMEM.
        let mut bc_stats = [0i64; 64];
        let buffers = bc::sysctl_pod(&[CTL_VFS, VFS_GENERIC, VFS_BCACHESTAT], &mut bc_stats)
            .filter(|&n| n >= 16)
            .map(|_| bc_stats[1].max(0) as u64 * pg);
        Some(Memory {
            total,
            available: (free + inactive).min(total),
            free: Some(free),
            active: Some(p(UVM_ACTIVE)),
            inactive: Some(inactive),
            buffers,
            cached: None,
            shared: None,
            wired: Some(p(UVM_WIRED)),
        })
    }

    fn swap(&mut self) -> Option<Swap> {
        // uvmexp.swpages / swpginuse are the kernel's sums over all swap devices, the
        // same figures swapctl(SWAP_STATS) adds up, in pages.
        if !self.read_uvm() {
            return None;
        }
        let pg = if self.uvm[UVM_PAGESIZE] > 0 { self.uvm[UVM_PAGESIZE] as u64 } else { self.page };
        let total = self.uvm[UVM_SWPAGES].max(0) as u64 * pg;
        let used = self.uvm[UVM_SWPGINUSE].max(0) as u64 * pg;
        Some(Swap { total, used: used.min(total), free: total.saturating_sub(used) })
    }

    fn net_interfaces(&mut self, want_addrs: bool, out: &mut Vec<NetIf>) {
        bc::net_interfaces(want_addrs, out);
    }

    fn disk_io(&mut self, out: &mut Vec<DiskIo>) {
        // PLT-112 OpenBSD: hw.diskstats; per-direction times are absent.
        if !bc::sysctl_table(&[CTL_HW, HW_DISKSTATS], None, &mut self.disks) {
            out.clear();
            return;
        }
        let mut n = 0;
        for d in &self.disks {
            if n == out.len() {
                out.push(DiskIo::default());
            }
            let e = &mut out[n];
            n += 1;
            bc::set_str(&mut e.name, bc::cbytes(&d.ds_name));
            e.kind = DiskKind::Disk;
            e.read_count = d.ds_rxfer;
            e.write_count = d.ds_wxfer;
            e.read_bytes = d.ds_rbytes;
            e.write_bytes = d.ds_wbytes;
            e.read_time_ms = None;
            e.write_time_ms = None;
        }
        out.truncate(n);
    }

    fn filesystems(&mut self, out: &mut Vec<Mount>) {
        bc::filesystems(&mut self.fsbuf, out);
    }

    fn fs_usage(mount_point: &str) -> Option<FsUsage> {
        bc::fs_usage(mount_point)
    }

    fn sensors(&mut self, want_temp: bool, want_fan: bool, out: &mut Vec<Sensor>) {
        // PLT-133 OpenBSD: hw.sensors. Discovery walks every device (C2), done once.
        if !want_temp && !want_fan {
            out.clear();
            return;
        }
        if self.sensors.is_none() {
            self.sensors = Some(self.discover_sensors());
        }
        let mut n = 0;
        for s in self.sensors.as_ref().into_iter().flatten() {
            let want = match s.kind {
                SensorKind::Temperature => want_temp,
                SensorKind::Fan => want_fan,
            };
            if !want {
                continue;
            }
            // SAFETY: SensorRec is POD.
            let mut r: SensorRec = unsafe { zeroed() };
            if bc::sysctl_pod(&s.mib, &mut r).is_none() || r.flags & (SENSOR_FINVALID | SENSOR_FUNKNOWN) != 0 {
                continue;
            }
            let value = match s.kind {
                SensorKind::Temperature => (r.value - 273_150_000) as f64 / 1e6, // µK
                SensorKind::Fan => r.value as f64,
            };
            bc::put_sensor(out, &mut n, s.kind, &s.chip, &s.label, value, None, None);
        }
        out.truncate(n);
    }

    fn batteries(&mut self, out: &mut Vec<Battery>) {
        // PLT-133 OpenBSD: apm(4) APM_IOC_GETPOWER on /dev/apm, opened read-only once.
        if self.apm_fd < 0 && !self.apm_tried {
            self.apm_tried = true;
            // SAFETY: path is NUL-terminated.
            self.apm_fd = unsafe { libc::open(c"/dev/apm".as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
        }
        if self.apm_fd < 0 {
            out.clear();
            return;
        }
        let mut pi = ApmPowerInfo::default();
        // SAFETY: the ioctl writes one apm_power_info into `pi`.
        let r = unsafe { libc::ioctl(self.apm_fd, APM_IOC_GETPOWER, &mut pi as *mut ApmPowerInfo) };
        if r != 0 || pi.battery_state == APM_BATTERY_ABSENT || pi.battery_life > 100 {
            out.clear();
            return;
        }
        let status = if pi.battery_state == APM_BATT_CHARGING {
            BatteryStatus::Charging
        } else if pi.ac_state == APM_AC_OFF {
            BatteryStatus::Discharging
        } else if pi.ac_state == APM_AC_ON {
            if pi.battery_life >= 100 { BatteryStatus::Full } else { BatteryStatus::NotCharging }
        } else {
            BatteryStatus::Unknown
        };
        bc::put_battery(out, pi.battery_life as f64, status);
    }

    fn processes(&mut self, _fields: ProcFields, _visible: &[u32], out: &mut Vec<Proc>) {
        // PLT-154: one KERN_PROC call; threads and I/O are absent.
        let sz = size_of::<libc::kinfo_proc>() as c_int;
        if !bc::sysctl_table(&[CTL_KERN, KERN_PROC, KERN_PROC_KTHREAD, 0, sz, 0], Some(5), &mut self.procs) {
            out.clear();
            return;
        }
        let page = self.page;
        let mut n = 0;
        for kp in &self.procs {
            let comm = bc::cbytes(&kp.p_comm);
            let system = kp.p_flag & P_SYSTEM != 0 || kp.p_psflags & PS_SYSTEM != 0;
            // PLT-141: drop the kernel idle threads.
            if system && comm.starts_with(b"idle") {
                continue;
            }
            let p = bc::proc_slot(out, n);
            n += 1;
            p.pid = kp.p_pid as u32;
            p.ppid = Some(kp.p_ppid as u32);
            p.start_time = kp.p_ustart_sec;
            bc::set_str(&mut p.name, comm);
            p.uid = Some(kp.p_uid);
            p.status = bc::netbsdlike_status(kp.p_stat as u8);
            p.nice = Some(kp.p_nice as i32 - NZERO);
            p.priority_class = None;
            p.num_threads = 0; // absent in phase 1 (PLT-154)
            p.cpu_user = bc::tv_secs(kp.p_uutime_sec as i64, kp.p_uutime_usec as i64);
            p.cpu_system = bc::tv_secs(kp.p_ustime_sec as i64, kp.p_ustime_usec as i64);
            p.rss = kp.p_vm_rssize.max(0) as u64 * page;
            let pages = kp.p_vm_tsize.max(0) as u64 + kp.p_vm_dsize.max(0) as u64 + kp.p_vm_ssize.max(0) as u64;
            p.vms = Some(pages * page);
            p.io_read = None;
            p.io_write = None;
            p.last_cpu = (kp.p_cpuid != KI_NOCPU).then_some(kp.p_cpuid as u32);
            p.is_kernel_thread = system;
        }
        out.truncate(n);
    }

    fn process_cmdline(&mut self, pid: u32, _start_time: u64) -> Option<Vec<String>> {
        // PLT-152
        if !bc::sysctl_table(&[CTL_KERN, KERN_PROC_ARGS, pid as c_int, KERN_PROC_ARGV], None, &mut self.argbuf) {
            return None;
        }
        parse_argv(&self.argbuf)
    }

    fn username(&mut self, uid: u32) -> Option<String> {
        bc::username(uid)
    }

    fn process_extended(&mut self, pid: u32, start_time: u64, _want_sockets: bool) -> Option<ProcExtended> {
        // Section 16: only context switches are available on OpenBSD.
        let kp = self.kinfo(pid)?;
        if kp.p_ustart_sec != start_time {
            return None;
        }
        let valid = kp.p_uvalid != 0;
        Some(ProcExtended {
            ctx_voluntary: valid.then_some(kp.p_uru_nvcsw),
            ctx_involuntary: valid.then_some(kp.p_uru_nivcsw),
            ..ProcExtended::default()
        })
    }

    fn process_signal(&mut self, pid: u32, start_time: u64, sig: Signal) -> Result<(), ActionError> {
        let cur = self.kinfo(pid).map(|k| k.p_ustart_sec);
        bc::signal(pid, cur, start_time, sig)
    }

    fn process_set_priority(&mut self, pid: u32, start_time: u64, delta: i32) -> Result<(), ActionError> {
        let cur = self.kinfo(pid).map(|k| (k.p_ustart_sec, k.p_nice as i32 - NZERO));
        bc::renice(pid, cur, start_time, delta)
    }

    fn pid_max(&mut self) -> Option<u32> {
        Some(99_999) // PID_MAX, PLT-128
    }

    fn reset_caches(&mut self) {
        self.freq = None;
        self.model = None;
        self.sensors = None;
        self.hz = bc::stathz();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_layout() {
        let psz = size_of::<usize>();
        let mut b = vec![1u8; psz]; // one non-null pointer
        b.extend(std::iter::repeat(0u8).take(psz)); // terminating NULL
        b.extend_from_slice(b"sh\0");
        assert_eq!(parse_argv(&b).unwrap(), vec!["sh"]);
        assert!(parse_argv(&vec![0u8; psz]).is_none());
    }
}
