//! Windows backend (06-platform-sources.md, Windows parts; Windows 10 1809 and later).
//!
//! Process owners: the contract's `Proc::uid` is a `u32`, but Windows owners are SIDs.
//! This backend interns each distinct owner SID into a small table inside `Sys` and
//! reports the table index as `uid`. `username(uid)` resolves the SID at that index with
//! `LookupAccountSidW` (`DOMAIN\user`, or the SID string when lookup fails). Indexes are
//! stable for the whole session and are meaningless outside this process.

mod ffi;
mod io;
mod procs;

use std::fmt::Write as _;
use std::mem::{size_of, zeroed};
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemPerformanceInformation, SystemProcessorPerformanceInformation,
    SystemTimeOfDayInformation,
};
use windows_sys::Win32::Foundation::{FILETIME, STATUS_BUFFER_TOO_SMALL, STATUS_INFO_LENGTH_MISMATCH};
use windows_sys::Win32::System::Power::{CallNtPowerInformation, ProcessorInformation};
use windows_sys::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
use windows_sys::Win32::System::SystemInformation::{
    ComputerNameDnsHostname, GetComputerNameExW, GetLogicalProcessorInformationEx, GetNativeSystemInfo,
    GetSystemTimeAsFileTime, GetTickCount64, GlobalMemoryStatusEx, RelationProcessorCore,
    IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64, IMAGE_FILE_MACHINE_I386, MEMORYSTATUSEX,
    OSVERSIONINFOW, SYSTEM_INFO, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
};
use windows_sys::Win32::System::Threading::{
    GetActiveProcessorCount, GetActiveProcessorGroupCount, GetCurrentProcess, GetCurrentProcessId,
    GetPriorityClass, GetProcessAffinityMask, GetProcessGroupAffinity, GetSystemTimes, IsWow64Process2,
    SetPriorityClass, TerminateProcess, ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS,
    HIGH_PRIORITY_CLASS, IDLE_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SET_INFORMATION, PROCESS_TERMINATE, REALTIME_PRIORITY_CLASS,
};

use self::ffi::ProcessorPerformance;
use self::procs::Handle;
use crate::platform::{
    ActionError, Battery, CpuCounters, CpuFreq, CpuTimes, CpuTopology, DiskIo, FsUsage, LoadAvg, Memory,
    Mount, NetIf, Platform, Proc, ProcExtended, ProcFields, Sensor, Signal, Swap, SystemInfo,
};

/// Per-CPU data younger than this is shared between `cpu_times` and `cpu_counters`.
const PPI_REUSE: Duration = Duration::from_millis(250);
/// Glances caches CPU frequency for 4 s.
const FREQ_TTL: Duration = Duration::from_secs(4);
/// Seconds between 1601-01-01 and 1970-01-01.
const EPOCH_DIFF_SECS: u64 = 11_644_473_600;

/// FILETIME (100 ns since 1601) to Unix seconds.
pub(crate) fn filetime_to_unix(ft: i64) -> u64 {
    (ft.max(0) as u64 / 10_000_000).saturating_sub(EPOCH_DIFF_SECS)
}

fn ft64(ft: &FILETIME) -> i64 {
    ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
}

/// Append UTF-16 text to `dst` without an intermediate allocation.
pub(crate) fn push_utf16(dst: &mut String, src: &[u16]) {
    dst.extend(char::decode_utf16(src.iter().copied()).map(|c| c.unwrap_or('\u{FFFD}')));
}

/// Append a decimal number without `format!`.
pub(crate) fn push_u32(dst: &mut String, mut v: u32) {
    let mut d = [0u8; 10];
    let mut n = 0;
    loop {
        d[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    while n > 0 {
        n -= 1;
        dst.push(d[n] as char);
    }
}

/// Element `i` of a reused output vector, appending a default one when needed.
pub(crate) fn slot<T: Default>(v: &mut Vec<T>, i: usize) -> &mut T {
    if i >= v.len() {
        v.push(T::default());
    }
    &mut v[i]
}

/// NUL-terminated UTF-16 copy of `s` (start-up / slow-cadence paths only).
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `HKLM\<key>\<value>` as a string (REG_SZ), trimmed of trailing NULs.
fn reg_sz(key: &[u16], value: &str, out: &mut String) -> bool {
    let value = wide(value);
    let mut buf = [0u16; 512];
    let mut cb = (buf.len() * 2) as u32;
    // SAFETY: NUL-terminated key/value names; buffer size passed in bytes.
    let r = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr().cast(), &mut cb)
    };
    if r != 0 {
        return false;
    }
    let mut n = (cb as usize / 2).min(buf.len());
    while n > 0 && buf[n - 1] == 0 {
        n -= 1;
    }
    push_utf16(out, &buf[..n]);
    true
}

/// 32-bit kernel counter widened to 64 bits (PLT-002); assumes fewer than 2^32 events
/// between two reads.
#[derive(Clone, Copy, Default)]
struct Wrap32 {
    last: u32,
    acc: u64,
    init: bool,
}

impl Wrap32 {
    fn update(&mut self, raw: u32) -> u64 {
        if self.init {
            self.acc += raw.wrapping_sub(self.last) as u64;
        } else {
            self.acc = raw as u64;
            self.init = true;
        }
        self.last = raw;
        self.acc
    }
}

pub struct Sys {
    snap: procs::Snapshot,
    users: procs::Users,
    cmd_buf: Vec<u64>,
    sock_buf: Vec<u64>,
    pf_buf: Vec<u64>,
    ppi: Vec<ProcessorPerformance>,
    ppi_n: usize,
    ppi_at: Option<Instant>,
    groups: u16,
    topo: Option<CpuTopology>,
    model: Option<Option<String>>,
    power: Vec<ffi::ProcessorPowerInformation>,
    freq: Option<(Instant, Option<u64>, Option<u64>)>,
    ctx: Wrap32,
    syscalls: Wrap32,
    intr: Vec<Wrap32>,
    page_size: u64,
    self_pid: u32,
    net: io::Net,
    disks: io::Disks,
    fs: io::Fs,
}

// SAFETY: the raw handles and pointers inside `Sys` (disk handles, buffers) are owned
// exclusively by it and are valid on any thread.
unsafe impl Send for Sys {}

pub fn new() -> Sys {
    // SAFETY: plain FFI calls; SYSTEM_INFO is plain data.
    let (page_size, self_pid) = unsafe {
        // PLT-122: no "No disk" dialogs for empty removable drives.
        ffi::SetErrorMode(ffi::SEM_FAILCRITICALERRORS);
        let mut si: SYSTEM_INFO = zeroed();
        GetNativeSystemInfo(&mut si);
        (if si.dwPageSize == 0 { 4096 } else { si.dwPageSize as u64 }, GetCurrentProcessId())
    };
    Sys {
        snap: procs::Snapshot::new(),
        users: procs::Users::new(),
        cmd_buf: Vec::new(),
        sock_buf: Vec::new(),
        pf_buf: Vec::new(),
        ppi: Vec::new(),
        ppi_n: 0,
        ppi_at: None,
        groups: 1,
        topo: None,
        model: None,
        power: Vec::new(),
        freq: None,
        ctx: Wrap32::default(),
        syscalls: Wrap32::default(),
        intr: Vec::new(),
        page_size,
        self_pid,
        net: io::Net::new(),
        disks: io::Disks::new(),
        fs: io::Fs::new(),
    }
}

impl Sys {
    /// PLT-055: per-CPU `SystemProcessorPerformanceInformation`, one call per processor
    /// group (`NtQuerySystemInformationEx`) on machines with more than 64 CPUs.
    fn read_ppi(&mut self) -> bool {
        if let Some(t) = self.ppi_at {
            if t.elapsed() < PPI_REUSE {
                return self.ppi_n > 0;
            }
        }
        self.ppi_at = Some(Instant::now());
        if self.ppi.is_empty() {
            // SAFETY: plain FFI calls.
            let (n, g) = unsafe { (GetActiveProcessorCount(ffi::ALL_PROCESSOR_GROUPS), GetActiveProcessorGroupCount()) };
            self.ppi.resize(n.max(1) as usize, ProcessorPerformance::default());
            self.groups = g.max(1);
        }
        let esz = size_of::<ProcessorPerformance>();
        let mut filled = 0usize;
        if self.groups <= 1 {
            let mut ret = 0u32;
            // SAFETY: buffer valid for the length passed.
            let st = unsafe {
                NtQuerySystemInformation(
                    SystemProcessorPerformanceInformation,
                    self.ppi.as_mut_ptr().cast(),
                    (self.ppi.len() * esz) as u32,
                    &mut ret,
                )
            };
            if st >= 0 {
                filled = ret as usize / esz;
            }
        } else {
            for g in 0..self.groups {
                let rest = self.ppi.len() - filled;
                if rest == 0 {
                    break;
                }
                let mut ret = 0u32;
                // SAFETY: input is a USHORT group number; output slice valid for `rest` entries.
                let st = unsafe {
                    ffi::NtQuerySystemInformationEx(
                        SystemProcessorPerformanceInformation,
                        (&g as *const u16).cast(),
                        2,
                        self.ppi.as_mut_ptr().add(filled).cast(),
                        (rest * esz) as u32,
                        &mut ret,
                    )
                };
                if st < 0 {
                    filled = 0;
                    break;
                }
                filled += ret as usize / esz;
            }
        }
        self.ppi_n = filled.min(self.ppi.len());
        self.ppi_n > 0
    }

    fn arch() -> String {
        let (mut pm, mut nm) = (0u16, 0u16);
        // SAFETY: pseudo-handle of the current process, valid out pointers.
        let ok = unsafe { IsWow64Process2(GetCurrentProcess(), &mut pm, &mut nm) } != 0;
        let from_native = if ok {
            match nm {
                IMAGE_FILE_MACHINE_AMD64 => Some("x86_64"),
                IMAGE_FILE_MACHINE_ARM64 => Some("aarch64"),
                IMAGE_FILE_MACHINE_I386 => Some("x86"),
                0x01c4 => Some("arm"), // IMAGE_FILE_MACHINE_ARMNT
                _ => None,
            }
        } else {
            None
        };
        // Fallback only: reports x64 for an x64 build emulated on ARM64.
        let name = from_native.unwrap_or_else(|| {
            // SAFETY: SYSTEM_INFO is plain data filled by the call.
            let arch = unsafe {
                let mut si: SYSTEM_INFO = zeroed();
                GetNativeSystemInfo(&mut si);
                si.Anonymous.Anonymous.wProcessorArchitecture
            };
            match arch {
                9 => "x86_64",
                12 => "aarch64",
                0 => "x86",
                5 => "arm",
                _ => "unknown",
            }
        });
        name.to_string()
    }
}

impl Platform for Sys {
    /// PLT-023.
    fn system_info(&mut self) -> SystemInfo {
        let mut info = SystemInfo { os_name: "Windows".to_string(), ..Default::default() };
        let mut buf = [0u16; 256];
        let mut n = buf.len() as u32;
        // SAFETY: buffer size passed in characters.
        if unsafe { GetComputerNameExW(ComputerNameDnsHostname, buf.as_mut_ptr(), &mut n) } != 0 {
            push_utf16(&mut info.hostname, &buf[..(n as usize).min(buf.len())]);
        }
        // SAFETY: OSVERSIONINFOW is plain data; size field set before the call.
        let mut v: OSVERSIONINFOW = unsafe { zeroed() };
        v.dwOSVersionInfoSize = size_of::<OSVERSIONINFOW>() as u32;
        // SAFETY: valid out pointer with its size field set.
        let have_v = unsafe { ffi::RtlGetVersion(&mut v) } >= 0;
        let key = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
        let mut product = String::new();
        reg_sz(&key, "ProductName", &mut product);
        // The registry ProductName still says "Windows 10" on Windows 11.
        if have_v && v.dwBuildNumber >= 22000 && product.contains("Windows 10") {
            product = product.replacen("Windows 10", "Windows 11", 1);
        }
        // DisplayVersion exists from 20H2; ReleaseId before that (frozen at 2009 later).
        let mut release = String::new();
        if !reg_sz(&key, "DisplayVersion", &mut release) {
            reg_sz(&key, "ReleaseId", &mut release);
        }
        let s = &mut info.os_version;
        let prod = product.strip_prefix("Windows ").unwrap_or(&product).trim();
        for part in [prod, release.trim()] {
            if !part.is_empty() {
                if !s.is_empty() {
                    s.push(' ');
                }
                s.push_str(part);
            }
        }
        if have_v {
            if !s.is_empty() {
                s.push(' ');
            }
            let _ = write!(s, "{}.{}.{}", v.dwMajorVersion, v.dwMinorVersion, v.dwBuildNumber);
        }
        info.arch = Self::arch();
        info
    }

    /// PLT-032.
    fn boot_time(&mut self) -> Option<u64> {
        // SYSTEM_TIMEOFDAY_INFORMATION: BootTime, CurrentTime, TimeZoneBias, TimeZoneId,
        // Reserved, BootTimeBias, SleepTimeBias (48 bytes, phnt ntexapi.h).
        let mut tod = [0i64; 6];
        let mut ret = 0u32;
        // SAFETY: 48-byte buffer passed with its size.
        let st = unsafe { NtQuerySystemInformation(SystemTimeOfDayInformation, tod.as_mut_ptr().cast(), 48, &mut ret) };
        if st >= 0 && tod[0] > 0 {
            return Some(filetime_to_unix(tod[0]));
        }
        // SAFETY: plain FFI calls with a valid out pointer.
        let (now, up_ms) = unsafe {
            let mut ft = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
            GetSystemTimeAsFileTime(&mut ft);
            (ft64(&ft), GetTickCount64())
        };
        Some(filetime_to_unix(now - (up_ms as i64) * 10_000))
    }

    /// PLT-045.
    fn cpu_topology(&mut self) -> CpuTopology {
        if let Some(t) = self.topo {
            return t;
        }
        let mut len = 0u32;
        let (mut logical, mut cores) = (0u32, 0u32);
        // SAFETY: first call only reports the needed length.
        unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, null_mut(), &mut len) };
        let mut buf = vec![0u64; len as usize / 8 + 1];
        // SAFETY: buffer valid for `len` bytes.
        let ok = len > 0
            && unsafe { GetLogicalProcessorInformationEx(RelationProcessorCore, buf.as_mut_ptr().cast(), &mut len) } != 0;
        if ok {
            let base = buf.as_ptr() as *const u8;
            let mut off = 0usize;
            while off + size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX>() <= len as usize {
                // SAFETY: records are laid out back to back inside `len` bytes; each
                // starts on an 8-byte boundary.
                let rec = unsafe { &*(base.add(off) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX) };
                if rec.Relationship == RelationProcessorCore {
                    cores += 1;
                    // SAFETY: Processor is the active union member for this relationship;
                    // GroupMask has GroupCount entries.
                    unsafe {
                        let pr = &rec.Anonymous.Processor;
                        let masks = std::ptr::addr_of!(pr.GroupMask) as *const windows_sys::Win32::System::SystemInformation::GROUP_AFFINITY;
                        for i in 0..pr.GroupCount as usize {
                            logical += (*masks.add(i)).Mask.count_ones();
                        }
                    }
                }
                if rec.Size == 0 {
                    break;
                }
                off += rec.Size as usize;
            }
        }
        let t = if ok && logical > 0 {
            CpuTopology { logical, physical: Some(cores) }
        } else {
            // SAFETY: plain FFI call.
            let n = unsafe { GetActiveProcessorCount(ffi::ALL_PROCESSOR_GROUPS) };
            CpuTopology { logical: n.max(1), physical: None }
        };
        self.topo = Some(t);
        t
    }

    /// PLT-045 model and frequency.
    fn cpu_freq(&mut self) -> CpuFreq {
        if self.model.is_none() {
            let key = wide(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0");
            let mut s = String::new();
            let ok = reg_sz(&key, "ProcessorNameString", &mut s);
            let t = s.trim();
            self.model = Some(if ok && !t.is_empty() { Some(t.to_string()) } else { None });
        }
        let fresh = matches!(self.freq, Some((t, _, _)) if t.elapsed() < FREQ_TTL);
        if !fresh {
            let n = self.cpu_topology().logical.max(1) as usize;
            self.power.resize(n, Default::default());
            let bytes = (n * size_of::<ffi::ProcessorPowerInformation>()) as u32;
            // SAFETY: output buffer holds one PROCESSOR_POWER_INFORMATION per logical CPU.
            let st = unsafe { CallNtPowerInformation(ProcessorInformation, std::ptr::null(), 0, self.power.as_mut_ptr().cast(), bytes) };
            let (cur, max) = if st >= 0 {
                let sum: u64 = self.power.iter().map(|p| p.current_mhz as u64).sum();
                let max = self.power.iter().map(|p| p.max_mhz as u64).max().unwrap_or(0);
                let cur = sum / n as u64;
                ((cur > 0).then_some(cur * 1_000_000), (max > 0).then_some(max * 1_000_000))
            } else {
                (None, None)
            };
            self.freq = Some((Instant::now(), cur, max));
        }
        let (_, current_hz, max_hz) = self.freq.unwrap_or((Instant::now(), None, None));
        CpuFreq { model: self.model.clone().flatten(), current_hz, max_hz }
    }

    /// PLT-055. Interrupt time is reported in `irq` (the contract has no separate
    /// `interrupt` field); `total()` then equals user + kernel.
    fn cpu_times(&mut self, total: &mut CpuTimes, per_cpu: Option<&mut Vec<CpuTimes>>) -> bool {
        let mut per = per_cpu;
        if let Some(v) = per.as_mut() {
            v.clear();
        }
        if self.read_ppi() {
            *total = CpuTimes { dpc: Some(0.0), irq: Some(0.0), ..Default::default() };
            for p in &self.ppi[..self.ppi_n] {
                let sys = (p.kernel_time - p.idle_time - p.dpc_time - p.interrupt_time).max(0);
                let c = CpuTimes {
                    user: p.user_time as f64 / 1e7,
                    system: sys as f64 / 1e7,
                    idle: p.idle_time as f64 / 1e7,
                    dpc: Some(p.dpc_time as f64 / 1e7),
                    irq: Some(p.interrupt_time as f64 / 1e7),
                    ..Default::default()
                };
                total.user += c.user;
                total.system += c.system;
                total.idle += c.idle;
                total.dpc = Some(total.dpc.unwrap_or(0.0) + c.dpc.unwrap_or(0.0));
                total.irq = Some(total.irq.unwrap_or(0.0) + c.irq.unwrap_or(0.0));
                if let Some(v) = per.as_mut() {
                    v.push(c);
                }
            }
            return true;
        }
        // Fallback: GetSystemTimes (kernel includes idle).
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut idle, mut kernel, mut user) = (z, z, z);
        // SAFETY: valid out pointers.
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return false;
        }
        let (i, k, u) = (ft64(&idle), ft64(&kernel), ft64(&user));
        *total = CpuTimes {
            user: u as f64 / 1e7,
            system: (k - i).max(0) as f64 / 1e7,
            idle: i as f64 / 1e7,
            ..Default::default()
        };
        true
    }

    /// Section 7 Windows row.
    fn cpu_counters(&mut self) -> CpuCounters {
        let mut c = CpuCounters::default();
        let mut buf = [0u64; 128];
        let mut ret = 0u32;
        // SAFETY: 1024-byte buffer passed with its size.
        let st = unsafe {
            NtQuerySystemInformation(SystemPerformanceInformation, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut ret)
        };
        if st >= 0 && ret as usize >= ffi::PERF_MIN_SIZE {
            let b = buf.as_ptr() as *const u8;
            // SAFETY: offsets lie inside the 312 bytes the kernel filled.
            let (cs, sc) = unsafe {
                (
                    (b.add(ffi::PERF_CONTEXT_SWITCHES_OFFSET) as *const u32).read_unaligned(),
                    (b.add(ffi::PERF_SYSTEM_CALLS_OFFSET) as *const u32).read_unaligned(),
                )
            };
            c.ctx_switches = Some(self.ctx.update(cs));
            c.syscalls = Some(self.syscalls.update(sc));
        }
        if self.read_ppi() {
            self.intr.resize(self.ppi_n, Wrap32::default());
            let mut sum = 0u64;
            for (w, p) in self.intr.iter_mut().zip(&self.ppi[..self.ppi_n]) {
                sum += w.update(p.interrupt_count);
            }
            c.interrupts = Some(sum);
        }
        c
    }

    /// PLT-072: emulated from the runnable-thread count of the process snapshot.
    fn load_avg(&mut self) -> Option<LoadAvg> {
        self.snap.ensure();
        let [min1, min5, min15] = self.snap.load()?;
        Some(LoadAvg { min1, min5, min15, emulated: true })
    }

    /// PLT-083.
    fn memory(&mut self) -> Option<Memory> {
        // SAFETY: plain data structs with their size fields set before each call.
        unsafe {
            let mut m: MEMORYSTATUSEX = zeroed();
            m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
            if GlobalMemoryStatusEx(&mut m) == 0 {
                return None;
            }
            let mut pi: PERFORMANCE_INFORMATION = zeroed();
            let cb = size_of::<PERFORMANCE_INFORMATION>() as u32;
            pi.cb = cb;
            let cached = if GetPerformanceInfo(&mut pi, cb) != 0 {
                Some(pi.SystemCache as u64 * pi.PageSize as u64)
            } else {
                None
            };
            Some(Memory {
                total: m.ullTotalPhys,
                available: m.ullAvailPhys,
                free: Some(m.ullAvailPhys),
                cached,
                ..Default::default()
            })
        }
    }

    /// PLT-093.
    fn swap(&mut self) -> Option<Swap> {
        if self.pf_buf.is_empty() {
            self.pf_buf.resize(512, 0);
        }
        let mut len = 0usize;
        let mut ok = false;
        for _ in 0..4 {
            let cap = (self.pf_buf.len() * 8) as u32;
            let mut ret = 0u32;
            // SAFETY: buffer valid for `cap` bytes.
            let st = unsafe {
                NtQuerySystemInformation(ffi::SYSTEM_PAGEFILE_INFORMATION, self.pf_buf.as_mut_ptr().cast(), cap, &mut ret)
            };
            if st == STATUS_INFO_LENGTH_MISMATCH || st == STATUS_BUFFER_TOO_SMALL {
                self.pf_buf.resize((ret as usize).max(cap as usize * 2) / 8 + 1, 0);
                continue;
            }
            ok = st >= 0;
            len = (ret as usize).min(cap as usize);
            break;
        }
        if !ok {
            return None;
        }
        let (mut total, mut used) = (0u64, 0u64);
        let base = self.pf_buf.as_ptr() as *const u8;
        let mut off = 0usize;
        while off + size_of::<ffi::SystemPageFileInformation>() <= len {
            // SAFETY: record inside the filled part of the 8-byte aligned buffer.
            let e = unsafe { &*(base.add(off) as *const ffi::SystemPageFileInformation) };
            total += e.total_size as u64;
            used += e.total_in_use as u64;
            if e.next_entry_offset == 0 {
                break;
            }
            off += e.next_entry_offset as usize;
        }
        let (total, used) = (total * self.page_size, used * self.page_size);
        Some(Swap { total, used, free: total.saturating_sub(used) })
    }

    fn net_interfaces(&mut self, want_addrs: bool, out: &mut Vec<NetIf>) {
        self.net.interfaces(want_addrs, out);
    }

    fn disk_io(&mut self, out: &mut Vec<DiskIo>) {
        self.disks.read(out);
    }

    fn filesystems(&mut self, out: &mut Vec<Mount>) {
        self.fs.list(out);
    }

    fn fs_usage(mount_point: &str) -> Option<FsUsage> {
        io::fs_usage(mount_point)
    }

    /// PLT-134: temperatures and fans are absent on Windows.
    fn sensors(&mut self, _want_temp: bool, _want_fan: bool, out: &mut Vec<Sensor>) {
        out.clear();
    }

    fn batteries(&mut self, out: &mut Vec<Battery>) {
        io::batteries(out);
    }

    /// PLT-155: one `SystemProcessInformation` call; every field comes from it. I/O
    /// transfer counts are part of the same record, so they are always filled.
    fn processes(&mut self, _fields: ProcFields, _visible: &[u32], out: &mut Vec<Proc>) {
        if !self.snap.ensure() {
            out.clear();
            return;
        }
        let Sys { snap, users, .. } = self;
        users.gen = users.gen.wrapping_add(1);
        let mut i = 0;
        for (p, threads) in snap.iter() {
            let pid = p.unique_process_id as u32;
            // PLT-141: drop the System Idle Process.
            if pid == 0 {
                continue;
            }
            let r = slot(out, i);
            i += 1;
            r.pid = pid;
            r.ppid = Some(p.inherited_from_unique_process_id as u32);
            r.start_time = filetime_to_unix(p.create_time);
            r.name.clear();
            push_utf16(&mut r.name, procs::image_name(p));
            r.uid = users.uid(pid, p.create_time);
            r.status = procs::status(threads);
            // PLT-157: priority class instead of nice.
            r.nice = None;
            r.priority_class = Some(procs::priority_label(p.base_priority));
            r.num_threads = p.number_of_threads;
            r.cpu_user = p.user_time as f64 / 1e7;
            r.cpu_system = p.kernel_time as f64 / 1e7;
            r.rss = p.working_set_size as u64;
            r.vms = Some(p.pagefile_usage as u64);
            r.io_read = Some(p.read_transfer_count.max(0) as u64);
            r.io_write = Some(p.write_transfer_count.max(0) as u64);
            r.last_cpu = None;
            r.is_kernel_thread = false;
        }
        out.truncate(i);
        users.evict();
    }

    fn process_cmdline(&mut self, pid: u32, start_time: u64) -> Option<Vec<String>> {
        procs::cmdline(pid, start_time, &mut self.cmd_buf)
    }

    fn username(&mut self, uid: u32) -> Option<String> {
        self.users.name(uid)
    }

    /// Section 16, Windows column.
    fn process_extended(&mut self, pid: u32, start_time: u64, want_sockets: bool) -> Option<ProcExtended> {
        if !self.snap.ensure() {
            return None;
        }
        let mut x = ProcExtended::default();
        {
            // Identity check against the current enumeration (gone -> None).
            let (p, threads) = self.snap.find(pid, start_time)?;
            x.num_handles = Some(p.handle_count);
            // Total context switches only, reported as voluntary.
            x.ctx_voluntary = Some(threads.iter().map(|t| t.context_switches as u64).sum());
        }
        if let Ok(h) = Handle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION) {
            let mut groups = [0u16; 4];
            let mut count = groups.len() as u16;
            // SAFETY: valid handle; array length passed in `count`.
            let single = unsafe { GetProcessGroupAffinity(h.0, &mut count, groups.as_mut_ptr()) } != 0 && count <= 1;
            // The mask covers the current group only; multi-group processes stay absent.
            if single {
                let (mut pm, mut sm) = (0usize, 0usize);
                // SAFETY: valid handle and out pointers.
                if unsafe { GetProcessAffinityMask(h.0, &mut pm, &mut sm) } != 0 {
                    x.affinity_cores = Some(pm.count_ones());
                }
            }
        }
        if want_sockets {
            let (tcp, udp) = io::socket_counts(&mut self.sock_buf, pid);
            x.tcp = tcp;
            x.udp = udp;
        }
        Some(x)
    }

    /// PLT-171: both signals map to TerminateProcess(handle, 1) after the identity check.
    fn process_signal(&mut self, pid: u32, start_time: u64, _sig: Signal) -> Result<(), ActionError> {
        // PLT-173.
        if pid == self.self_pid {
            return Err(ActionError::PermissionDenied);
        }
        let h = Handle::open(pid, PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION)?;
        h.check_identity(start_time)?;
        // SAFETY: valid handle with PROCESS_TERMINATE; does not wait for exit.
        if unsafe { TerminateProcess(h.0, 1) } == 0 {
            return Err(procs::last_action_error());
        }
        Ok(())
    }

    /// PLT-172: one step along Idle <-> BelowNormal <-> Normal <-> AboveNormal <-> High.
    /// `delta > 0` lowers the priority. Never steps into Realtime.
    fn process_set_priority(&mut self, pid: u32, start_time: u64, delta: i32) -> Result<(), ActionError> {
        const LADDER: [u32; 5] = [
            IDLE_PRIORITY_CLASS,
            BELOW_NORMAL_PRIORITY_CLASS,
            NORMAL_PRIORITY_CLASS,
            ABOVE_NORMAL_PRIORITY_CLASS,
            HIGH_PRIORITY_CLASS,
        ];
        if pid == self.self_pid {
            return Err(ActionError::PermissionDenied);
        }
        let h = Handle::open(pid, PROCESS_SET_INFORMATION | PROCESS_QUERY_LIMITED_INFORMATION)?;
        h.check_identity(start_time)?;
        // SAFETY: valid handle.
        let cur = unsafe { GetPriorityClass(h.0) };
        if cur == 0 {
            return Err(procs::last_action_error());
        }
        let idx = if cur == REALTIME_PRIORITY_CLASS {
            5
        } else {
            LADDER.iter().position(|&c| c == cur).unwrap_or(2) as i32
        };
        if delta == 0 || (idx == 5 && delta < 0) {
            return Ok(());
        }
        let new = (idx - delta.signum()).clamp(0, 4);
        if new == idx {
            return Ok(());
        }
        // SAFETY: valid handle with PROCESS_SET_INFORMATION.
        if unsafe { SetPriorityClass(h.0, LADDER[new as usize]) } == 0 {
            return Err(procs::last_action_error());
        }
        Ok(())
    }

    /// PLT-128: absent on Windows.
    fn pid_max(&mut self) -> Option<u32> {
        None
    }

    fn reset_caches(&mut self) {
        self.users.clear_identities();
        self.topo = None;
        self.model = None;
        self.freq = None;
        self.net.invalidate();
        self.disks.invalidate();
        self.fs.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filetime_conversion() {
        // 2000-01-01T00:00:00Z.
        assert_eq!(filetime_to_unix(125_911_584_000_000_000), 946_684_800);
        assert_eq!(filetime_to_unix(0), 0);
    }

    #[test]
    fn wrap32_widens() {
        let mut w = Wrap32::default();
        assert_eq!(w.update(u32::MAX - 1), (u32::MAX - 1) as u64);
        assert_eq!(w.update(3), u32::MAX as u64 + 4);
    }

    #[test]
    fn push_number() {
        let mut s = String::new();
        push_u32(&mut s, 0);
        push_u32(&mut s, 1203);
        assert_eq!(s, "01203");
    }
}
