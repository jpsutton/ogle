//! NetBSD backend (06-platform-sources.md, NetBSD notes; best effort, OQ-2).
//! Sensors and batteries are absent (envsys needs proplib ioctls, PLT-133).

use std::mem::{size_of, zeroed};
use std::ptr;
use std::time::{Duration, Instant};

use libc::{c_char, c_int};

use super::bsd_common::{self as bc, Mib, CTL_HW, CTL_KERN, CTL_VM, HW_MODEL, HW_NCPU, MNT_NOWAIT};
use crate::platform::{
    ActionError, Battery, CpuCounters, CpuFreq, CpuTimes, CpuTopology, DiskIo, DiskKind, FsUsage,
    LoadAvg, Memory, Mount, NetIf, Platform, Proc, ProcExtended, ProcFields, Sensor, Signal, Swap,
    SystemInfo,
};

// sys/sysctl.h
const KERN_PROC2: c_int = 47;
const KERN_PROC_ARGS: c_int = 48;
const KERN_CP_TIME: c_int = 51;
const KERN_BOOTTIME: c_int = 83;
const KERN_PROC_ALL: c_int = 0;
const KERN_PROC_PID: c_int = 1;
const KERN_PROC_ARGV: c_int = 1;
const HW_IOSTATS: c_int = 9;
const HW_PHYSMEM64: c_int = 13;
const P_SYSTEM: i32 = 0x200;
const KI_NOCPU: u64 = !0;
const NZERO: i32 = 20;
// uvm/uvm_param.h
const VM_UVMEXP2: c_int = 5;
// sys/iostat.h
const IOSTAT_DISK: i32 = 0;

// Indices into `struct uvmexp_sysctl` viewed as int64_t[] (uvm/uvm_extern.h).
const UVM_PAGESIZE: usize = 0;
const UVM_FREE: usize = 4;
const UVM_ACTIVE: usize = 5;
const UVM_INACTIVE: usize = 6;
const UVM_WIRED: usize = 8;
const UVM_SWPAGES: usize = 17;
const UVM_SWPGINUSE: usize = 18;
const UVM_INTRS: usize = 26;
const UVM_SWTCH: usize = 27;
const UVM_SOFTS: usize = 28;
const UVM_SYSCALLS: usize = 29;
const UVM_FILEPAGES: usize = 72;
const UVM_EXECPAGES: usize = 73;

const FREQ_TTL: Duration = Duration::from_secs(4);

/// `struct io_sysctl` (sys/iostat.h).
#[repr(C)]
#[allow(dead_code)]
#[derive(Clone, Copy)]
struct IoSysctl {
    name: [c_char; 16],
    busy: i32,
    type_: i32,
    xfer: u64,
    seek: u64,
    bytes: u64,
    attachtime_sec: u32,
    attachtime_usec: u32,
    timestamp_sec: u32,
    timestamp_usec: u32,
    time_sec: u32,
    time_usec: u32,
    rxfer: u64,
    rbytes: u64,
    wxfer: u64,
    wbytes: u64,
    wait_sec: u32,
    wait_usec: u32,
    waitsum_sec: u32,
    waitsum_usec: u32,
    busysum_sec: u32,
    busysum_usec: u32,
}

pub struct Sys {
    page: u64,
    hz: f64,
    ncpu: u32,
    freq_mib: Option<Mib>,
    uvm: [i64; 128],
    cp: Vec<u64>,
    procs: Vec<libc::kinfo_proc2>,
    argbuf: Vec<u8>,
    disks: Vec<IoSysctl>,
    fsbuf: Vec<libc::statvfs>,
    freq: Option<(Instant, CpuFreq)>,
    model: Option<Option<String>>,
}

pub fn new() -> Sys {
    let page = bc::sysctl_int(&[CTL_HW, bc::HW_PAGESIZE]).filter(|&p| p > 0).unwrap_or(4096) as u64;
    let ncpu = bc::sysctl_int(&[CTL_HW, HW_NCPU]).filter(|&n| n > 0).unwrap_or(1) as u32;
    Sys {
        page,
        hz: bc::stathz(),
        ncpu,
        // Unverified name (PLT-044); absent when the node does not exist.
        freq_mib: Mib::new(c"machdep.cpu.frequency.current"),
        uvm: [0; 128],
        cp: Vec::new(),
        procs: Vec::new(),
        argbuf: Vec::new(),
        disks: Vec::new(),
        fsbuf: Vec::new(),
        freq: None,
        model: None,
    }
}

impl Sys {
    fn read_uvm(&mut self) -> bool {
        bc::sysctl_pod(&[CTL_VM, VM_UVMEXP2], &mut self.uvm).map_or(false, |n| n >= (UVM_EXECPAGES + 1) * 8)
    }

    fn uvm_page(&self) -> u64 {
        if self.uvm[UVM_PAGESIZE] > 0 { self.uvm[UVM_PAGESIZE] as u64 } else { self.page }
    }

    fn kinfo(&self, pid: u32) -> Option<libc::kinfo_proc2> {
        // SAFETY: kinfo_proc2 is POD.
        let mut kp: libc::kinfo_proc2 = unsafe { zeroed() };
        let sz = size_of::<libc::kinfo_proc2>() as c_int;
        let n = bc::sysctl_pod(&[CTL_KERN, KERN_PROC2, KERN_PROC_PID, pid as c_int, sz, 1], &mut kp)?;
        (n > 0 && kp.p_pid as u32 == pid).then_some(kp)
    }

    fn conv(&self, c: &[u64]) -> CpuTimes {
        // CPUSTATES 5: user, nice, sys, intr, idle (PLT-054).
        let s = |v: u64| v as f64 / self.hz;
        CpuTimes {
            user: s(c[0]),
            nice: Some(s(c[1])),
            system: s(c[2]),
            irq: Some(s(c[3])),
            idle: s(c[4]),
            ..CpuTimes::default()
        }
    }
}

impl Platform for Sys {
    fn system_info(&mut self) -> SystemInfo {
        bc::system_info()
    }

    fn boot_time(&mut self) -> Option<u64> {
        // PLT-031: NetBSD returns a struct timespec.
        let mut ts: libc::timespec = unsafe { zeroed() };
        bc::sysctl_pod(&[CTL_KERN, KERN_BOOTTIME], &mut ts)?;
        (ts.tv_sec > 0).then_some(ts.tv_sec as u64)
    }

    fn cpu_topology(&mut self) -> CpuTopology {
        CpuTopology { logical: self.ncpu, physical: None }
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
        let cur = self
            .freq_mib
            .as_ref()
            .and_then(|m| bc::sysctl_int(m.get()))
            .filter(|&v| v > 0)
            .map(|v| v as u64 * 1_000_000);
        let f = CpuFreq { model: self.model.clone().flatten(), current_hz: cur, max_hz: None };
        self.freq = Some((Instant::now(), f.clone()));
        f
    }

    fn cpu_times(&mut self, total: &mut CpuTimes, per_cpu: Option<&mut Vec<CpuTimes>>) -> bool {
        // PLT-054 NetBSD: kern.cp_time returns per-CPU data when the buffer has room for
        // 5 × ncpu values, else the sum.
        let mib = [CTL_KERN, KERN_CP_TIME];
        if let Some(v) = per_cpu {
            v.clear();
            let want = self.ncpu as usize * 5;
            self.cp.clear();
            self.cp.resize(want, 0);
            let mut cp = std::mem::take(&mut self.cp);
            let got = bc::sysctl_slice(&mib, &mut cp[..]);
            let mut ok = false;
            if got == Some(want * 8) {
                let mut sum = [0u64; 5];
                for c in cp.chunks_exact(5) {
                    v.push(self.conv(c));
                    for k in 0..5 {
                        sum[k] += c[k];
                    }
                }
                *total = self.conv(&sum);
                ok = true;
            }
            self.cp = cp;
            if ok {
                return true;
            }
        }
        let mut t = [0u64; 5];
        if bc::sysctl_pod(&mib, &mut t) != Some(40) {
            return false;
        }
        *total = self.conv(&t);
        true
    }

    fn cpu_counters(&mut self) -> CpuCounters {
        if !self.read_uvm() {
            return CpuCounters::default();
        }
        let g = |i: usize| Some(self.uvm[i].max(0) as u64);
        CpuCounters {
            ctx_switches: g(UVM_SWTCH),
            interrupts: g(UVM_INTRS),
            soft_interrupts: g(UVM_SOFTS),
            syscalls: g(UVM_SYSCALLS),
        }
    }

    fn load_avg(&mut self) -> Option<LoadAvg> {
        bc::load_avg()
    }

    fn memory(&mut self) -> Option<Memory> {
        // PLT-082 NetBSD
        let total = bc::sysctl_u64(&[CTL_HW, HW_PHYSMEM64])?;
        if !self.read_uvm() {
            return None;
        }
        let pg = self.uvm_page();
        let p = |i: usize| self.uvm[i].max(0) as u64 * pg;
        let free = p(UVM_FREE);
        let inactive = p(UVM_INACTIVE);
        Some(Memory {
            total,
            available: (free + inactive).min(total),
            free: Some(free),
            active: Some(p(UVM_ACTIVE)),
            inactive: Some(inactive),
            buffers: None,
            cached: Some(p(UVM_FILEPAGES) + p(UVM_EXECPAGES)),
            shared: None,
            wired: Some(p(UVM_WIRED)),
        })
    }

    fn swap(&mut self) -> Option<Swap> {
        // uvmexp_sysctl.swpages / swpginuse are the kernel's sums over all swap devices,
        // the same figures swapctl(SWAP_STATS) adds up, in pages.
        if !self.read_uvm() {
            return None;
        }
        let pg = self.uvm_page();
        let total = self.uvm[UVM_SWPAGES].max(0) as u64 * pg;
        let used = self.uvm[UVM_SWPGINUSE].max(0) as u64 * pg;
        Some(Swap { total, used: used.min(total), free: total.saturating_sub(used) })
    }

    fn net_interfaces(&mut self, want_addrs: bool, out: &mut Vec<NetIf>) {
        bc::net_interfaces(want_addrs, out);
    }

    fn disk_io(&mut self, out: &mut Vec<DiskIo>) {
        // PLT-112 NetBSD: hw.iostats; per-direction times are absent.
        let mib = [CTL_HW, HW_IOSTATS, size_of::<IoSysctl>() as c_int];
        if !bc::sysctl_table(&mib, None, &mut self.disks) {
            out.clear();
            return;
        }
        let mut n = 0;
        for d in self.disks.iter().filter(|d| d.type_ == IOSTAT_DISK) {
            if n == out.len() {
                out.push(DiskIo::default());
            }
            let e = &mut out[n];
            n += 1;
            let name = bc::cbytes(&d.name);
            bc::set_str(&mut e.name, name);
            // dk(4) wedges are partitions of a parent disk.
            e.kind = if name.starts_with(b"dk") { DiskKind::Partition } else { DiskKind::Disk };
            e.read_count = d.rxfer;
            e.write_count = d.wxfer;
            e.read_bytes = d.rbytes;
            e.write_bytes = d.wbytes;
            e.read_time_ms = None;
            e.write_time_ms = None;
        }
        out.truncate(n);
    }

    fn filesystems(&mut self, out: &mut Vec<Mount>) {
        // PLT-121 NetBSD: getvfsstat(ST_NOWAIT); flags in f_flag (ST_RDONLY, ST_LOCAL).
        self.fsbuf.clear();
        // SAFETY: a null buffer only returns the mount count.
        let cnt = unsafe { libc::getvfsstat(ptr::null_mut(), 0, MNT_NOWAIT) };
        if cnt < 0 {
            out.clear();
            return;
        }
        self.fsbuf.reserve(cnt as usize + 4);
        let bytes = self.fsbuf.capacity() * size_of::<libc::statvfs>();
        // SAFETY: the kernel writes at most `bytes` bytes of statvfs records.
        let got = unsafe { libc::getvfsstat(self.fsbuf.as_mut_ptr(), bytes, MNT_NOWAIT) };
        if got < 0 {
            out.clear();
            return;
        }
        // SAFETY: `got` records were written.
        unsafe { self.fsbuf.set_len((got as usize).min(self.fsbuf.capacity())) };
        let mut n = 0;
        for s in &self.fsbuf {
            bc::put_mount(
                out,
                &mut n,
                bc::cbytes(&s.f_mntfromname),
                bc::cbytes(&s.f_mntonname),
                bc::cbytes(&s.f_fstypename),
                s.f_flag as u64,
            );
        }
        out.truncate(n);
    }

    fn fs_usage(mount_point: &str) -> Option<FsUsage> {
        bc::fs_usage(mount_point)
    }

    fn sensors(&mut self, _want_temp: bool, _want_fan: bool, out: &mut Vec<Sensor>) {
        out.clear(); // PLT-133: absent on NetBSD in phase 1
    }

    fn batteries(&mut self, out: &mut Vec<Battery>) {
        out.clear(); // PLT-133: absent on NetBSD in phase 1
    }

    fn processes(&mut self, _fields: ProcFields, _visible: &[u32], out: &mut Vec<Proc>) {
        // PLT-154 NetBSD: one KERN_PROC2 call; I/O is absent (PLT-153).
        let sz = size_of::<libc::kinfo_proc2>() as c_int;
        if !bc::sysctl_table(&[CTL_KERN, KERN_PROC2, KERN_PROC_ALL, 0, sz, 0], Some(5), &mut self.procs) {
            out.clear();
            return;
        }
        let page = self.page;
        let mut n = 0;
        for kp in &self.procs {
            let comm = bc::cbytes(&kp.p_comm);
            let system = kp.p_flag & P_SYSTEM != 0;
            // PLT-141
            if system && comm == b"idle" {
                continue;
            }
            let p = bc::proc_slot(out, n);
            n += 1;
            p.pid = kp.p_pid as u32;
            p.ppid = Some(kp.p_ppid as u32);
            p.start_time = kp.p_ustart_sec as u64;
            bc::set_str(&mut p.name, comm);
            p.uid = Some(kp.p_uid);
            p.status = bc::netbsdlike_status(kp.p_stat as u8);
            p.nice = Some(kp.p_nice as i32 - NZERO);
            p.priority_class = None;
            p.num_threads = kp.p_nlwps as u32;
            p.cpu_user = bc::tv_secs(kp.p_uutime_sec as i64, kp.p_uutime_usec as i64);
            p.cpu_system = bc::tv_secs(kp.p_ustime_sec as i64, kp.p_ustime_usec as i64);
            p.rss = kp.p_vm_rssize.max(0) as u64 * page;
            p.vms = Some(kp.p_vm_vsize.max(0) as u64 * page);
            p.io_read = None;
            p.io_write = None;
            p.last_cpu = (kp.p_cpuid != KI_NOCPU).then_some(kp.p_cpuid as u32);
            p.is_kernel_thread = system;
        }
        out.truncate(n);
    }

    fn process_cmdline(&mut self, pid: u32, _start_time: u64) -> Option<Vec<String>> {
        // PLT-152: NUL-separated strings.
        if !bc::sysctl_table(&[CTL_KERN, KERN_PROC_ARGS, pid as c_int, KERN_PROC_ARGV], None, &mut self.argbuf) {
            return None;
        }
        bc::split_nul(&self.argbuf)
    }

    fn username(&mut self, uid: u32) -> Option<String> {
        bc::username(uid)
    }

    fn process_extended(&mut self, pid: u32, start_time: u64, _want_sockets: bool) -> Option<ProcExtended> {
        // Section 16: only context switches are available on NetBSD.
        let kp = self.kinfo(pid)?;
        if kp.p_ustart_sec as u64 != start_time {
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
        let cur = self.kinfo(pid).map(|k| k.p_ustart_sec as u64);
        bc::signal(pid, cur, start_time, sig)
    }

    fn process_set_priority(&mut self, pid: u32, start_time: u64, delta: i32) -> Result<(), ActionError> {
        let cur = self.kinfo(pid).map(|k| (k.p_ustart_sec as u64, k.p_nice as i32 - NZERO));
        bc::renice(pid, cur, start_time, delta)
    }

    fn pid_max(&mut self) -> Option<u32> {
        Some(30_000) // PID_MAX in sys/proc.h, PLT-128
    }

    fn reset_caches(&mut self) {
        self.freq = None;
        self.model = None;
        self.hz = bc::stathz();
    }
}
