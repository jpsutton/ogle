//! Helpers shared by the FreeBSD, OpenBSD and NetBSD backends (06-platform-sources.md).
//!
//! Everything here is allocation-free in steady state: callers pass reusable buffers and
//! output vectors whose entries (and their `String`s) are overwritten in place.

// Each BSD backend uses a different subset of these helpers.
#![allow(dead_code)]

use std::ffi::{CStr, CString};
use std::mem::{size_of, zeroed};
use std::ptr;

use libc::{c_char, c_int, c_void};

use super::{
    ActionError, Battery, BatteryStatus, FsUsage, Mount, NetIf, Proc, Sensor, SensorKind, Signal,
    SystemInfo,
};

// net/if.h, identical on all three BSDs.
const IFF_UP: u32 = 0x1;
const IFF_RUNNING: u32 = 0x40;
// sys/socket.h, identical on all three BSDs.
const AF_LINK: c_int = 18;
// sys/mount.h (MNT_*) and sys/statvfs.h (ST_*), identical values on all three BSDs.
pub const MNT_RDONLY: u64 = 0x1;
pub const MNT_LOCAL: u64 = 0x1000;
pub const MNT_NOWAIT: c_int = 2;

// sys/sysctl.h top-level and kern.* numbers shared by the three BSDs.
pub const CTL_KERN: c_int = 1;
pub const CTL_VM: c_int = 2;
pub const CTL_HW: c_int = 6;
pub const KERN_CLOCKRATE: c_int = 12;
pub const HW_MODEL: c_int = 2;
pub const HW_NCPU: c_int = 3;
pub const HW_PAGESIZE: c_int = 7;

pub fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Reads a fixed-size value. `T` must be plain old data (valid for any bit pattern).
/// Returns the number of bytes the kernel wrote.
pub fn sysctl_pod<T: Copy>(mib: &[c_int], out: &mut T) -> Option<usize> {
    let mut len = size_of::<T>();
    // SAFETY: `out` is a valid, writable T of `len` bytes; T is POD so any bytes are valid.
    let r = unsafe {
        libc::sysctl(
            mib.as_ptr(),
            mib.len() as _,
            out as *mut T as *mut c_void,
            &mut len,
            ptr::null_mut(),
            0,
        )
    };
    (r == 0).then_some(len)
}

/// Reads into a slice of plain-old-data values. Returns the number of bytes written.
pub fn sysctl_slice<T: Copy>(mib: &[c_int], out: &mut [T]) -> Option<usize> {
    let mut len = std::mem::size_of_val(out);
    // SAFETY: `out` is valid for `len` writable bytes; T is POD.
    let r = unsafe {
        libc::sysctl(
            mib.as_ptr(),
            mib.len() as _,
            out.as_mut_ptr() as *mut c_void,
            &mut len,
            ptr::null_mut(),
            0,
        )
    };
    (r == 0).then_some(len)
}

/// Reads an integer sysctl of 4 or 8 bytes (signed).
pub fn sysctl_int(mib: &[c_int]) -> Option<i64> {
    let mut b = [0u8; 8];
    match sysctl_pod(mib, &mut b)? {
        4 => Some(i32::from_ne_bytes([b[0], b[1], b[2], b[3]]) as i64),
        8 => Some(i64::from_ne_bytes(b)),
        _ => None,
    }
}

/// Reads an unsigned counter sysctl of 4 or 8 bytes. FreeBSD `vm.stats.sys.*` are 64-bit
/// on current releases and 32-bit on old ones; an 8-byte buffer serves both.
pub fn sysctl_u64(mib: &[c_int]) -> Option<u64> {
    let mut b = [0u8; 8];
    match sysctl_pod(mib, &mut b)? {
        4 => Some(u32::from_ne_bytes([b[0], b[1], b[2], b[3]]) as u64),
        8 => Some(u64::from_ne_bytes(b)),
        _ => None,
    }
}

/// Reads a variable-size table of `T` into `v` (cleared first, capacity reused).
/// Tries the current capacity first; on ENOMEM (or no capacity) probes the size, adds
/// 10 % headroom and retries. `count_idx`, when set, is the MIB slot that carries the
/// element count (OpenBSD/NetBSD `KERN_PROC`, NetBSD `hw.iostats` style MIBs).
/// `T` must be plain old data.
pub fn sysctl_table<T: Copy>(mib: &[c_int], count_idx: Option<usize>, v: &mut Vec<T>) -> bool {
    let sz = size_of::<T>().max(1);
    let mut m = [0 as c_int; 12];
    if mib.len() > m.len() {
        return false;
    }
    m[..mib.len()].copy_from_slice(mib);
    let m = &mut m[..mib.len()];
    v.clear();
    for _ in 0..6 {
        if v.capacity() > 0 {
            if let Some(i) = count_idx {
                m[i] = v.capacity().min(c_int::MAX as usize) as c_int;
            }
            let mut len = v.capacity() * sz;
            // SAFETY: the kernel writes at most `len` bytes into the vector's spare capacity.
            let r = unsafe {
                libc::sysctl(
                    m.as_ptr(),
                    m.len() as _,
                    v.as_mut_ptr() as *mut c_void,
                    &mut len,
                    ptr::null_mut(),
                    0,
                )
            };
            if r == 0 {
                // SAFETY: `len / sz` elements were initialised by the kernel; T is POD.
                unsafe { v.set_len((len / sz).min(v.capacity())) };
                return true;
            }
            if errno() != libc::ENOMEM {
                return false;
            }
        }
        let mut need: usize = 0;
        // SAFETY: size probe with a null buffer.
        let r = unsafe {
            libc::sysctl(m.as_ptr(), m.len() as _, ptr::null_mut(), &mut need, ptr::null_mut(), 0)
        };
        let want_bytes = if r == 0 && need > 0 {
            need + need / 10 + 256
        } else {
            // Some nodes (OpenBSD KERN_PROC_ARGS) do not report a size: grow geometrically.
            (v.capacity() * sz * 2).max(8192)
        };
        let want = want_bytes / sz + 1;
        if want > (64 << 20) / sz {
            return false;
        }
        v.reserve(want.max(v.capacity() + 1));
    }
    false
}

/// Reads a string sysctl.
pub fn sysctl_string(mib: &[c_int]) -> Option<String> {
    let mut v: Vec<u8> = Vec::new();
    if !sysctl_table(mib, None, &mut v) {
        return None;
    }
    let s = cbytes_u8(&v);
    let s = String::from_utf8_lossy(s).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// MIB resolved once from a name (FreeBSD and NetBSD have `sysctlnametomib`).
#[cfg(not(target_os = "openbsd"))]
#[derive(Clone, Copy)]
pub struct Mib {
    v: [c_int; 12],
    n: usize,
}

#[cfg(not(target_os = "openbsd"))]
impl Mib {
    pub fn new(name: &CStr) -> Option<Mib> {
        let mut m = Mib { v: [0; 12], n: 12 };
        // SAFETY: `v` has room for `n` entries; name is NUL-terminated.
        let r = unsafe { libc::sysctlnametomib(name.as_ptr(), m.v.as_mut_ptr(), &mut m.n) };
        (r == 0 && m.n > 0 && m.n <= 12).then_some(m)
    }

    /// Resolves a name built at run time (C0 discovery paths only).
    pub fn named(name: &str) -> Option<Mib> {
        let c = CString::new(name).ok()?;
        Mib::new(&c)
    }

    pub fn get(&self) -> &[c_int] {
        &self.v[..self.n]
    }
}

/// Bytes of a C char array up to the first NUL.
pub fn cbytes(s: &[c_char]) -> &[u8] {
    // SAFETY: c_char and u8 have the same size and alignment.
    let b = unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, s.len()) };
    cbytes_u8(b)
}

pub fn cbytes_u8(b: &[u8]) -> &[u8] {
    match b.iter().position(|&c| c == 0) {
        Some(i) => &b[..i],
        None => b,
    }
}

/// Overwrites `dst` with `b`, keeping its allocation. Invalid UTF-8 bytes become '?'.
pub fn set_str(dst: &mut String, b: &[u8]) {
    dst.clear();
    match std::str::from_utf8(b) {
        Ok(s) => dst.push_str(s),
        Err(_) => {
            for &c in b {
                dst.push(if c.is_ascii() { c as char } else { '?' });
            }
        }
    }
}

pub fn cstring(s: &[c_char]) -> String {
    let mut out = String::new();
    set_str(&mut out, cbytes(s));
    out
}

/// PLT-022.
pub fn system_info() -> SystemInfo {
    // SAFETY: utsname is POD; uname fills it.
    let mut u: libc::utsname = unsafe { zeroed() };
    let ok = unsafe { libc::uname(&mut u) } == 0;
    let mut host = [0 as c_char; 256];
    // SAFETY: buffer of 256 bytes; the last byte stays NUL.
    let hok = unsafe { libc::gethostname(host.as_mut_ptr(), host.len() - 1) } == 0;
    SystemInfo {
        hostname: if hok { cstring(&host) } else if ok { cstring(&u.nodename) } else { String::new() },
        os_name: if ok { cstring(&u.sysname) } else { String::new() },
        os_version: if ok { cstring(&u.release) } else { String::new() },
        distro: None,
        arch: if ok { cstring(&u.machine) } else { String::new() },
    }
}

/// PLT-071.
pub fn load_avg() -> Option<super::LoadAvg> {
    let mut l = [0f64; 3];
    // SAFETY: room for 3 doubles.
    let n = unsafe { libc::getloadavg(l.as_mut_ptr(), 3) };
    (n == 3).then_some(super::LoadAvg { min1: l[0], min5: l[1], min15: l[2], emulated: false })
}

/// Statistics clock rate from `kern.clockrate`. The struct layout differs (OpenBSD 7 has
/// no tickadj/spare slot), so the returned length picks the `stathz` slot.
pub fn stathz() -> f64 {
    let mut ci = [0 as c_int; 5];
    let hz = match sysctl_pod(&[CTL_KERN, KERN_CLOCKRATE], &mut ci) {
        Some(16) => ci[2],
        Some(n) if n >= 20 => ci[3],
        _ => 0,
    };
    let hz = if hz > 0 { hz } else { ci[0] };
    if hz > 0 { hz as f64 } else { 100.0 }
}

/// PLT-127: one getpwuid_r lookup; caching happens above.
pub fn username(uid: u32) -> Option<String> {
    let mut pw: libc::passwd = unsafe { zeroed() };
    let mut res: *mut libc::passwd = ptr::null_mut();
    let mut buf = [0 as c_char; 2048];
    // SAFETY: all pointers are valid for the call; the result points into `buf`/`pw`.
    let r = unsafe { libc::getpwuid_r(uid as libc::uid_t, &mut pw, buf.as_mut_ptr(), buf.len(), &mut res) };
    if r != 0 || res.is_null() || pw.pw_name.is_null() {
        return None;
    }
    // SAFETY: pw_name is a NUL-terminated string inside `buf`.
    let name = unsafe { CStr::from_ptr(pw.pw_name) };
    Some(String::from_utf8_lossy(name.to_bytes()).into_owned())
}

/// PLT-102: one getifaddrs() walk gives counters, flags, speed and addresses.
pub fn net_interfaces(want_addrs: bool, out: &mut Vec<NetIf>) {
    let mut head: *mut libc::ifaddrs = ptr::null_mut();
    // SAFETY: getifaddrs allocates a list that is released below with freeifaddrs.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        out.clear();
        return;
    }
    let mut n = 0;
    let mut p = head;
    while !p.is_null() {
        // SAFETY: p is a node of the list returned by getifaddrs.
        let ifa = unsafe { &*p };
        p = ifa.ifa_next;
        if ifa.ifa_addr.is_null() || ifa.ifa_name.is_null() || ifa.ifa_data.is_null() {
            continue;
        }
        // SAFETY: ifa_addr is non-null.
        if unsafe { (*ifa.ifa_addr).sa_family } as c_int != AF_LINK {
            continue;
        }
        // SAFETY: AF_LINK entries carry a `struct if_data` in ifa_data (64-bit counters
        // on FreeBSD 11+, OpenBSD and NetBSD).
        let d: libc::if_data = unsafe { ptr::read_unaligned(ifa.ifa_data as *const libc::if_data) };
        if n == out.len() {
            out.push(NetIf::default());
        }
        let e = &mut out[n];
        n += 1;
        // SAFETY: ifa_name is a NUL-terminated string.
        set_str(&mut e.name, unsafe { CStr::from_ptr(ifa.ifa_name) }.to_bytes());
        e.rx_bytes = d.ifi_ibytes as u64;
        e.tx_bytes = d.ifi_obytes as u64;
        e.is_up = ifa.ifa_flags & (IFF_UP | IFF_RUNNING) == (IFF_UP | IFF_RUNNING);
        let baud = d.ifi_baudrate as u64;
        e.speed_bps = (baud > 0).then_some(baud);
        e.has_ip = !want_addrs;
    }
    out.truncate(n);
    if want_addrs {
        let mut p = head;
        while !p.is_null() {
            // SAFETY: as above.
            let ifa = unsafe { &*p };
            p = ifa.ifa_next;
            if ifa.ifa_addr.is_null() || ifa.ifa_name.is_null() {
                continue;
            }
            let fam = unsafe { (*ifa.ifa_addr).sa_family } as c_int;
            if fam != libc::AF_INET && fam != libc::AF_INET6 {
                continue;
            }
            let name = unsafe { CStr::from_ptr(ifa.ifa_name) }.to_bytes();
            if let Some(e) = out.iter_mut().find(|e| e.name.as_bytes() == name) {
                e.has_ip = true;
            }
        }
    }
    // SAFETY: head came from getifaddrs.
    unsafe { libc::freeifaddrs(head) };
}

/// PLT-121 mount record, reusing `out[*n]` when present.
pub fn put_mount(out: &mut Vec<Mount>, n: &mut usize, dev: &[u8], mnt: &[u8], ty: &[u8], flags: u64) {
    if *n == out.len() {
        out.push(Mount::default());
    }
    let m = &mut out[*n];
    *n += 1;
    set_str(&mut m.device, dev);
    set_str(&mut m.mount_point, mnt);
    set_str(&mut m.fs_type, ty);
    let local = flags & MNT_LOCAL != 0;
    m.is_network = !local;
    m.is_physical =
        local && !matches!(ty, b"devfs" | b"autofs" | b"nullfs" | b"tmpfs" | b"fdescfs");
    m.read_only = flags & MNT_RDONLY != 0;
}

/// PLT-121 (FreeBSD/OpenBSD): one getfsstat(MNT_NOWAIT) call into a reused buffer.
#[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
pub fn filesystems(buf: &mut Vec<libc::statfs>, out: &mut Vec<Mount>) {
    buf.clear();
    // SAFETY: a null buffer only returns the mount count.
    let cnt = unsafe { libc::getfsstat(ptr::null_mut(), 0, MNT_NOWAIT) };
    if cnt < 0 {
        out.clear();
        return;
    }
    buf.reserve(cnt as usize + 4);
    let bytes = buf.capacity() * size_of::<libc::statfs>();
    // SAFETY: the kernel writes at most `bytes` bytes (capacity) of statfs records.
    let got = unsafe { libc::getfsstat(buf.as_mut_ptr(), bytes as _, MNT_NOWAIT) };
    if got < 0 {
        out.clear();
        return;
    }
    // SAFETY: `got` records were written.
    unsafe { buf.set_len((got as usize).min(buf.capacity())) };
    let mut n = 0;
    for s in buf.iter() {
        put_mount(
            out,
            &mut n,
            cbytes(&s.f_mntfromname),
            cbytes(&s.f_mntonname),
            cbytes(&s.f_fstypename),
            s.f_flags as u64,
        );
    }
    out.truncate(n);
}

/// Usage of one mount (runs on the fs worker, PLT-010). Blocks are in `f_frsize` units.
pub fn fs_usage(path: &str) -> Option<FsUsage> {
    let c = CString::new(path).ok()?;
    // SAFETY: statvfs is POD and filled by the call.
    let mut s: libc::statvfs = unsafe { zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let fr = if s.f_frsize as u64 > 0 { s.f_frsize as u64 } else { s.f_bsize as u64 };
    Some(FsUsage {
        total: s.f_blocks as u64 * fr,
        free: s.f_bfree as u64 * fr,
        avail: s.f_bavail as u64 * fr,
    })
}

/// Returns the slot `n` of `out`, appending a default record when needed.
pub fn proc_slot(out: &mut Vec<Proc>, n: usize) -> &mut Proc {
    if n == out.len() {
        out.push(Proc::default());
    }
    &mut out[n]
}

/// Sensor record, reusing `out[*n]` and its strings when present.
#[allow(clippy::too_many_arguments)]
pub fn put_sensor(
    out: &mut Vec<Sensor>,
    n: &mut usize,
    kind: SensorKind,
    chip: &str,
    label: &str,
    value: f64,
    high: Option<f64>,
    critical: Option<f64>,
) {
    if *n < out.len() {
        let s = &mut out[*n];
        s.kind = kind;
        s.chip.clear();
        s.chip.push_str(chip);
        s.label.clear();
        s.label.push_str(label);
        s.value = value;
        s.high = high;
        s.critical = critical;
    } else {
        out.push(Sensor { kind, chip: chip.to_string(), label: label.to_string(), value, high, critical });
    }
    *n += 1;
}

/// Single combined battery record named "Battery".
pub fn put_battery(out: &mut Vec<Battery>, percent: f64, status: BatteryStatus) {
    out.truncate(1);
    if out.is_empty() {
        out.push(Battery { name: "Battery".to_string(), percent, status });
    } else {
        out[0].percent = percent;
        out[0].status = status;
    }
}

/// NUL-separated argv to strings; None when empty (kernel thread, zombie, denied).
pub fn split_nul(b: &[u8]) -> Option<Vec<String>> {
    let mut end = b.len();
    while end > 0 && b[end - 1] == 0 {
        end -= 1;
    }
    let b = &b[..end];
    if b.is_empty() {
        return None;
    }
    Some(b.split(|&c| c == 0).map(|s| String::from_utf8_lossy(s).into_owned()).collect())
}

pub fn tv_secs(sec: i64, usec: i64) -> f64 {
    sec as f64 + usec as f64 / 1e6
}

/// PLT-150 for OpenBSD/NetBSD `p_stat` (SIDL 1, SRUN 2, SSLEEP 3, SSTOP 4, SZOMB 5,
/// SDEAD 6, SONPROC 7; NetBSD LSSUSPENDED 8).
pub fn netbsdlike_status(stat: u8) -> u8 {
    match stat {
        1 => b'I',
        2 | 7 => b'R',
        3 => b'S',
        4 | 8 => b'T',
        5 | 6 => b'Z',
        _ => b'S',
    }
}

fn map_err(e: c_int) -> ActionError {
    match e {
        libc::ESRCH => ActionError::NoSuchProcess,
        libc::EPERM | libc::EACCES => ActionError::PermissionDenied,
        e => ActionError::Other(e),
    }
}

/// PLT-170 / PLT-173: refuse our own pid and pid 0 (kill(0) would hit the whole
/// process group), and require the start time read just now to match.
fn check_identity(pid: u32, current: Option<u64>, start_time: u64) -> Result<(), ActionError> {
    if pid == 0 || pid == std::process::id() {
        return Err(ActionError::PermissionDenied);
    }
    match current {
        Some(s) if s == start_time => Ok(()),
        _ => Err(ActionError::NoSuchProcess),
    }
}

/// PLT-171. `current` is the start time re-read immediately before acting.
pub fn signal(pid: u32, current: Option<u64>, start_time: u64, sig: Signal) -> Result<(), ActionError> {
    check_identity(pid, current, start_time)?;
    let s = match sig {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    // SAFETY: plain syscall; pid is > 0 and verified.
    if unsafe { libc::kill(pid as libc::pid_t, s) } == 0 {
        Ok(())
    } else {
        Err(map_err(errno()))
    }
}

/// PLT-172. `current` is (start time, nice) re-read immediately before acting.
pub fn renice(pid: u32, current: Option<(u64, i32)>, start_time: u64, delta: i32) -> Result<(), ActionError> {
    check_identity(pid, current.map(|c| c.0), start_time)?;
    let nice = current.map(|c| c.1).unwrap_or(0).saturating_add(delta).clamp(-20, 19);
    // SAFETY: plain syscall; pid is > 0 and verified.
    if unsafe { libc::setpriority(libc::PRIO_PROCESS, pid as _, nice) } == 0 {
        Ok(())
    } else {
        Err(map_err(errno()))
    }
}

/// PLT-002 widening of a 32-bit counter to 64 bits.
#[derive(Clone, Copy, Default)]
pub struct Wide {
    last: u32,
    base: u64,
    init: bool,
}

impl Wide {
    pub fn update(&mut self, v: u32) -> u64 {
        if self.init && v < self.last {
            if self.last > 1 << 31 {
                self.base += 1 << 32;
            } else {
                self.base = 0; // counter reset
            }
        }
        self.last = v;
        self.init = true;
        self.base + v as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_argv() {
        assert_eq!(split_nul(b"ls\0-l\0\0").unwrap(), vec!["ls", "-l"]);
        assert!(split_nul(b"\0\0").is_none());
    }

    #[test]
    fn widen() {
        let mut w = Wide::default();
        assert_eq!(w.update(u32::MAX - 1), (u32::MAX - 1) as u64);
        assert_eq!(w.update(3), (1u64 << 32) + 3);
        assert_eq!(w.update(1), 1); // small drop: reset
    }

    #[test]
    fn status() {
        assert_eq!(netbsdlike_status(7), b'R');
        assert_eq!(netbsdlike_status(6), b'Z');
    }
}
