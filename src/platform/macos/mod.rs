//! macOS backend (06-platform-sources.md, macOS parts).

mod ffi;
mod iokit;
mod net;
mod procs;

use std::collections::HashMap;
use std::ffi::CStr;
use std::mem::{size_of, MaybeUninit};
use std::ptr;
use std::time::{Duration, Instant};

use libc::{c_int, c_uint};

use super::{
    ActionError, Battery, CpuCounters, CpuFreq, CpuTimes, CpuTopology, DiskIo, FsUsage, LoadAvg,
    Memory, Mount, NetIf, Platform, Proc, ProcExtended, ProcFields, Sensor, Signal, Swap,
    SystemInfo,
};
use ffi::*;

/// 32-bit Mach tick counter widened to 64 bits (PLT-002).
#[derive(Clone, Copy, Default)]
struct Wide {
    last: u32,
    acc: u64,
    init: bool,
}

impl Wide {
    fn update(&mut self, v: u32) -> u64 {
        if self.init {
            self.acc += v.wrapping_sub(self.last) as u64;
        } else {
            self.acc = v as u64;
            self.init = true;
        }
        self.last = v;
        self.acc
    }
}

pub struct Sys {
    host: MachPort,
    clk_tck: f64,
    page_size: u64,
    mem_total: Option<u64>,
    /// Mach absolute-time ticks per second for process CPU times (PLT-148).
    ticks_per_sec: f64,
    cpu_total_wide: [Wide; 4],
    cpu_wide: Vec<[Wide; 4]>,
    freq: CpuFreq,
    freq_at: Option<Instant>,
    freq_static_done: bool,
    pmgr_max: Option<u64>,
    // Processes.
    proc_buf: Vec<u8>,
    names: HashMap<u32, procs::NameEnt>,
    cycle: u32,
    vis: Vec<u32>,
    args_buf: Vec<u8>,
    fd_buf: Vec<ProcFdInfo>,
    sock_cache: Option<procs::SockCache>,
    // Network.
    net_buf: Vec<u8>,
    ifnames: Vec<(u16, String)>,
    // File systems.
    fs_buf: Vec<libc::statfs>,
    // IOKit (disks, battery).
    io: iokit::IoState,
}

pub fn new() -> Sys {
    // SAFETY: mach_host_self has no preconditions. The port is cached for the whole
    // session so the send right is taken only once.
    let host = unsafe { mach_host_self() };
    // SAFETY: sysconf has no preconditions.
    let tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    let mut pg: usize = 0;
    // SAFETY: valid out pointer.
    if unsafe { host_page_size(host, &mut pg) } != 0 || pg == 0 {
        // SAFETY: sysconf has no preconditions.
        pg = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as usize;
    }
    Sys {
        host,
        clk_tck: if tck > 0 { tck as f64 } else { 100.0 },
        page_size: pg as u64,
        mem_total: sysctl_u64(c"hw.memsize"),
        ticks_per_sec: ticks_per_sec(),
        cpu_total_wide: [Wide::default(); 4],
        cpu_wide: Vec::new(),
        freq: CpuFreq::default(),
        freq_at: None,
        freq_static_done: false,
        pmgr_max: None,
        proc_buf: Vec::new(),
        names: HashMap::new(),
        cycle: 0,
        vis: Vec::new(),
        args_buf: Vec::new(),
        fd_buf: Vec::new(),
        sock_cache: None,
        net_buf: Vec::new(),
        ifnames: Vec::new(),
        fs_buf: Vec::new(),
        io: iokit::IoState::new(),
    }
}

/// Process CPU times come in Mach absolute-time units. `hw.tbfrequency` gives the native
/// tick rate even under Rosetta 2, where `mach_timebase_info` reports 1/1 (PLT-148).
fn ticks_per_sec() -> f64 {
    if let Some(f) = sysctl_u64(c"hw.tbfrequency").filter(|&f| f > 0) {
        return f as f64;
    }
    let mut tb = MachTimebaseInfo::default();
    // SAFETY: valid out pointer.
    if unsafe { mach_timebase_info(&mut tb) } == 0 && tb.numer > 0 && tb.denom > 0 {
        return 1e9 * tb.denom as f64 / tb.numer as f64;
    }
    1e9
}

pub(crate) fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Reads an integer sysctl of 4 or 8 bytes.
pub(crate) fn sysctl_u64(name: &CStr) -> Option<u64> {
    let mut v = [0u8; 8];
    let mut len = v.len();
    // SAFETY: buffer and length describe valid memory.
    let r = unsafe { libc::sysctlbyname(name.as_ptr(), v.as_mut_ptr().cast(), &mut len, ptr::null_mut(), 0) };
    if r != 0 {
        return None;
    }
    match len {
        4 => Some(u32::from_ne_bytes([v[0], v[1], v[2], v[3]]) as u64),
        8 => Some(u64::from_ne_bytes(v)),
        _ => None,
    }
}

/// Reads a short string sysctl (up to 255 bytes).
fn sysctl_string(name: &CStr) -> Option<String> {
    let mut v = [0u8; 256];
    let mut len = v.len();
    // SAFETY: buffer and length describe valid memory.
    let r = unsafe { libc::sysctlbyname(name.as_ptr(), v.as_mut_ptr().cast(), &mut len, ptr::null_mut(), 0) };
    if r != 0 {
        return None;
    }
    let s = trim_nul(&v[..len.min(v.len())]);
    let s = String::from_utf8_lossy(s).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Reads a fixed-size struct through a MIB; the returned size must match exactly.
fn sysctl_struct<T: Copy>(mib: &mut [c_int]) -> Option<T> {
    let mut v = MaybeUninit::<T>::zeroed();
    let mut len = size_of::<T>();
    // SAFETY: buffer and length describe valid memory; T is plain old data.
    let r = unsafe {
        libc::sysctl(mib.as_mut_ptr(), mib.len() as c_uint, v.as_mut_ptr().cast(), &mut len, ptr::null_mut(), 0)
    };
    // SAFETY: the kernel filled all size_of::<T>() bytes (zeroed before anyway).
    (r == 0 && len == size_of::<T>()).then(|| unsafe { v.assume_init() })
}

/// Reads a variable-size MIB into `buf`, growing it on ENOMEM. Returns the byte count.
pub(crate) fn sysctl_vec(mib: &mut [c_int], buf: &mut Vec<u8>) -> Option<usize> {
    for _ in 0..4 {
        if !buf.is_empty() {
            let mut len = buf.len();
            // SAFETY: buffer and length describe valid memory.
            let r = unsafe {
                libc::sysctl(mib.as_mut_ptr(), mib.len() as c_uint, buf.as_mut_ptr().cast(), &mut len, ptr::null_mut(), 0)
            };
            if r == 0 {
                return Some(len);
            }
            if errno() != libc::ENOMEM {
                return None;
            }
        }
        let mut need = 0usize;
        // SAFETY: size probe with a null buffer.
        let r = unsafe {
            libc::sysctl(mib.as_mut_ptr(), mib.len() as c_uint, ptr::null_mut(), &mut need, ptr::null_mut(), 0)
        };
        if r != 0 {
            return None;
        }
        // 10% headroom (PLT-148); double when the table grew past the probe.
        let want = (need + need / 10 + 1024).max(buf.len() * 2);
        buf.resize(want, 0);
    }
    None
}

pub(crate) fn trim_nul(b: &[u8]) -> &[u8] {
    match b.iter().position(|&c| c == 0) {
        Some(i) => &b[..i],
        None => b,
    }
}

/// Bytes of a NUL-terminated `c_char` array, bounded by the array.
pub(crate) fn cchars(a: &[libc::c_char]) -> &[u8] {
    // SAFETY: c_char and u8 have the same size and layout.
    let b = unsafe { std::slice::from_raw_parts(a.as_ptr().cast::<u8>(), a.len()) };
    trim_nul(b)
}

/// Replaces `dst` with `b` (lossy UTF-8) reusing its allocation.
pub(crate) fn set_str(dst: &mut String, b: &[u8]) {
    dst.clear();
    dst.push_str(&String::from_utf8_lossy(b));
}

fn ticks_to_times(t: [u64; 4], tck: f64) -> CpuTimes {
    // CPU_STATE_USER, CPU_STATE_SYSTEM, CPU_STATE_IDLE, CPU_STATE_NICE.
    CpuTimes {
        user: t[0] as f64 / tck,
        system: t[1] as f64 / tck,
        idle: t[2] as f64 / tck,
        nice: Some(t[3] as f64 / tck),
        ..CpuTimes::default()
    }
}

impl Sys {
    /// PLT-053 total via HOST_CPU_LOAD_INFO.
    fn cpu_total(&mut self, total: &mut CpuTimes) -> bool {
        let mut t = [0 as c_int; 4];
        let mut cnt = HOST_CPU_LOAD_INFO_COUNT;
        // SAFETY: buffer holds HOST_CPU_LOAD_INFO_COUNT integers.
        if unsafe { host_statistics(self.host, HOST_CPU_LOAD_INFO, t.as_mut_ptr(), &mut cnt) } != 0 {
            return false;
        }
        let mut acc = [0u64; 4];
        for s in 0..4 {
            acc[s] = self.cpu_total_wide[s].update(t[s] as u32);
        }
        *total = ticks_to_times(acc, self.clk_tck);
        true
    }

    /// PLT-053 per-CPU via host_processor_info; the total is the per-CPU sum.
    fn cpu_per(&mut self, total: &mut CpuTimes, v: &mut Vec<CpuTimes>) -> bool {
        let mut ncpu: u32 = 0;
        let mut info: *mut c_int = ptr::null_mut();
        let mut cnt: u32 = 0;
        // SAFETY: valid out pointers; the kernel allocates `info` in our task.
        let kr = unsafe { host_processor_info(self.host, PROCESSOR_CPU_LOAD_INFO, &mut ncpu, &mut info, &mut cnt) };
        v.clear();
        if kr != 0 || info.is_null() {
            return self.cpu_total(total);
        }
        // SAFETY: the kernel returned `cnt` integers at `info`.
        let ticks = unsafe { std::slice::from_raw_parts(info as *const u32, cnt as usize) };
        let n = (ncpu as usize).min(ticks.len() / 4);
        self.cpu_wide.resize(n, [Wide::default(); 4]);
        let mut sum = [0u64; 4];
        for i in 0..n {
            let mut acc = [0u64; 4];
            for s in 0..4 {
                acc[s] = self.cpu_wide[i][s].update(ticks[i * 4 + s]);
                sum[s] += acc[s];
            }
            v.push(ticks_to_times(acc, self.clk_tck));
        }
        // The buffer MUST be released on every call (PLT-053).
        // SAFETY: address and size are exactly what host_processor_info returned.
        unsafe { vm_deallocate(mach_task_self_, info as usize, cnt as usize * size_of::<c_int>()) };
        *total = ticks_to_times(sum, self.clk_tck);
        true
    }
}

impl Platform for Sys {
    /// PLT-021.
    fn system_info(&mut self) -> SystemInfo {
        let mut host = [0u8; 256];
        // SAFETY: buffer and length are valid.
        let hostname = if unsafe { libc::gethostname(host.as_mut_ptr().cast(), host.len()) } == 0 {
            String::from_utf8_lossy(trim_nul(&host)).into_owned()
        } else {
            String::new()
        };
        let mut u = MaybeUninit::<libc::utsname>::zeroed();
        // SAFETY: valid out pointer.
        let arch = if unsafe { libc::uname(u.as_mut_ptr()) } == 0 {
            // SAFETY: uname succeeded and filled the struct.
            let u = unsafe { u.assume_init() };
            String::from_utf8_lossy(cchars(&u.machine)).into_owned()
        } else {
            String::new()
        };
        SystemInfo {
            hostname,
            os_name: "macOS".to_string(),
            os_version: sysctl_string(c"kern.osproductversion").unwrap_or_default(),
            distro: None,
            arch,
        }
    }

    /// PLT-031.
    fn boot_time(&mut self) -> Option<u64> {
        let mut mib = [CTL_KERN, libc::KERN_BOOTTIME];
        let tv: Timeval = sysctl_struct(&mut mib)?;
        (tv.sec > 0).then_some(tv.sec as u64)
    }

    /// PLT-042.
    fn cpu_topology(&mut self) -> CpuTopology {
        let logical = sysctl_u64(c"hw.logicalcpu").map(|v| v as u32).filter(|&v| v > 0).unwrap_or_else(|| {
            // SAFETY: sysconf has no preconditions.
            unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) }.max(1) as u32
        });
        CpuTopology { logical, physical: sysctl_u64(c"hw.physicalcpu").map(|v| v as u32).filter(|&v| v > 0) }
    }

    /// PLT-042. Cached for 4 s; model and Apple Silicon max read once.
    fn cpu_freq(&mut self) -> CpuFreq {
        if self.freq_at.is_some_and(|t| t.elapsed() < Duration::from_secs(4)) {
            return self.freq.clone();
        }
        if !self.freq_static_done {
            self.freq_static_done = true;
            self.freq.model = sysctl_string(c"machdep.cpu.brand_string");
            if sysctl_u64(c"hw.cpufrequency_max").is_none() {
                // Apple Silicon (or Rosetta on it): best-effort pmgr tables.
                self.pmgr_max = iokit::pmgr_max_hz();
            }
        }
        self.freq.current_hz = sysctl_u64(c"hw.cpufrequency").filter(|&v| v > 0);
        self.freq.max_hz = sysctl_u64(c"hw.cpufrequency_max").filter(|&v| v > 0).or(self.pmgr_max);
        self.freq_at = Some(Instant::now());
        self.freq.clone()
    }

    fn cpu_times(&mut self, total: &mut CpuTimes, per_cpu: Option<&mut Vec<CpuTimes>>) -> bool {
        match per_cpu {
            Some(v) => self.cpu_per(total, v),
            None => self.cpu_total(total),
        }
    }

    /// PLT-061: XNU has no system-wide counters; all absent.
    fn cpu_counters(&mut self) -> CpuCounters {
        CpuCounters::default()
    }

    /// PLT-071.
    fn load_avg(&mut self) -> Option<LoadAvg> {
        let mut l = [0f64; 3];
        // SAFETY: buffer holds 3 doubles.
        if unsafe { libc::getloadavg(l.as_mut_ptr(), 3) } != 3 {
            return None;
        }
        Some(LoadAvg { min1: l[0], min5: l[1], min15: l[2], emulated: false })
    }

    /// PLT-081.
    fn memory(&mut self) -> Option<Memory> {
        let total = self.mem_total?;
        let mut vm = MaybeUninit::<VmStatistics64>::zeroed();
        let mut cnt = HOST_VM_INFO64_COUNT;
        // SAFETY: buffer holds HOST_VM_INFO64_COUNT integers.
        if unsafe { host_statistics64(self.host, HOST_VM_INFO64, vm.as_mut_ptr().cast(), &mut cnt) } != 0 {
            return None;
        }
        // SAFETY: zeroed and (partly) filled by the kernel; all bit patterns are valid.
        let vm = unsafe { vm.assume_init() };
        let pg = self.page_size;
        let free_all = vm.free_count as u64;
        Some(Memory {
            total,
            available: ((vm.inactive_count as u64 + free_all) * pg).min(total),
            free: Some(free_all.saturating_sub(vm.speculative_count as u64) * pg),
            active: Some(vm.active_count as u64 * pg),
            inactive: Some(vm.inactive_count as u64 * pg),
            buffers: None,
            cached: Some(vm.external_page_count as u64 * pg),
            shared: None,
            wired: Some(vm.wire_count as u64 * pg),
        })
    }

    /// PLT-091.
    fn swap(&mut self) -> Option<Swap> {
        let mut mib = [CTL_VM, VM_SWAPUSAGE];
        let x: XswUsage = sysctl_struct(&mut mib)?;
        Some(Swap { total: x.xsu_total, used: x.xsu_used, free: x.xsu_avail })
    }

    fn net_interfaces(&mut self, want_addrs: bool, out: &mut Vec<NetIf>) {
        self.net_impl(want_addrs, out);
    }

    fn disk_io(&mut self, out: &mut Vec<DiskIo>) {
        self.io.disk_io(out);
    }

    fn filesystems(&mut self, out: &mut Vec<Mount>) {
        self.fs_impl(out);
    }

    fn fs_usage(mount_point: &str) -> Option<FsUsage>
    where
        Self: Sized,
    {
        net::fs_usage(mount_point)
    }

    /// PLT-132: temperatures and fans are absent in phase 1 (best effort, not implemented).
    fn sensors(&mut self, _want_temp: bool, _want_fan: bool, out: &mut Vec<Sensor>) {
        out.clear();
    }

    fn batteries(&mut self, out: &mut Vec<Battery>) {
        self.io.batteries(out);
    }

    fn processes(&mut self, fields: ProcFields, visible: &[u32], out: &mut Vec<Proc>) {
        self.procs_impl(fields, visible, out);
    }

    fn process_cmdline(&mut self, pid: u32, start_time: u64) -> Option<Vec<String>> {
        self.cmdline_impl(pid, start_time)
    }

    fn username(&mut self, uid: u32) -> Option<String> {
        procs::username(uid)
    }

    fn process_extended(&mut self, pid: u32, start_time: u64, want_sockets: bool) -> Option<ProcExtended> {
        self.extended_impl(pid, start_time, want_sockets)
    }

    fn process_signal(&mut self, pid: u32, start_time: u64, sig: Signal) -> Result<(), ActionError> {
        self.signal_impl(pid, start_time, sig)
    }

    fn process_set_priority(&mut self, pid: u32, start_time: u64, delta: i32) -> Result<(), ActionError> {
        self.priority_impl(pid, start_time, delta)
    }

    /// PLT-128: PID_MAX in XNU proc_internal.h.
    fn pid_max(&mut self) -> Option<u32> {
        Some(99_999)
    }

    fn reset_caches(&mut self) {
        self.names.clear();
        self.ifnames.clear();
        self.sock_cache = None;
        self.freq_at = None;
        self.io.force_rescan();
    }
}
