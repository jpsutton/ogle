//! Processes on macOS (06 sections 15.3, 16, 17).

use std::collections::hash_map::Entry;
use std::mem::{size_of, MaybeUninit};
use std::ptr;
use std::time::{Duration, Instant};

use libc::{c_int, c_void};

use super::ffi::*;
use super::{errno, set_str, sysctl_u64, sysctl_vec, trim_nul, Sys};
use crate::platform::{ActionError, Proc, ProcExtended, ProcFields, Signal};

const KINFO_SIZE: usize = size_of::<KinfoProc>();
/// Socket counts are refreshed at most every 10 s for the pinned process (PLT-160).
const SOCK_TTL: Duration = Duration::from_secs(10);

/// Per-identity name cache entry (PLT-149, PLT-125).
pub struct NameEnt {
    start: u64,
    comm: [u8; 17],
    name: String,
    seen: u32,
}

pub struct SockCache {
    pid: u32,
    start: u64,
    at: Instant,
    tcp: Option<u32>,
    udp: Option<u32>,
}

/// Reads PROC_PIDTASKINFO. Err carries errno.
fn task_info(pid: c_int) -> Result<ProcTaskInfo, i32> {
    let mut ti = MaybeUninit::<ProcTaskInfo>::zeroed();
    let sz = size_of::<ProcTaskInfo>() as c_int;
    // SAFETY: buffer of `sz` bytes.
    let r = unsafe { proc_pidinfo(pid, PROC_PIDTASKINFO, 0, ti.as_mut_ptr().cast(), sz) };
    if r == sz {
        // SAFETY: fully written by the kernel.
        Ok(unsafe { ti.assume_init() })
    } else {
        // A short read is treated like a denial; errno is only meaningful when r <= 0.
        let e = if r <= 0 { errno() } else { 0 };
        Err(if e == 0 { libc::EPERM } else { e })
    }
}

/// `proc_name` (up to 32 bytes) falling back to `p_comm` (PLT-149).
fn resolve_name(pid: c_int, comm: &[u8], dst: &mut String) {
    let mut buf = [0u8; 64];
    // SAFETY: buffer and size are valid.
    let r = unsafe { proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if r > 0 {
        set_str(dst, trim_nul(&buf[..(r as usize).min(buf.len())]));
    } else {
        set_str(dst, comm);
    }
}

/// PLT-150 status letter.
fn status(p_stat: i8, numrunning: Option<i32>) -> u8 {
    match p_stat {
        SZOMB => b'Z',
        SSTOP => b'T',
        SIDL => b'I',
        _ => match numrunning {
            Some(n) if n > 0 => b'R',
            _ => b'S',
        },
    }
}

fn map_errno(e: i32) -> ActionError {
    match e {
        libc::ESRCH => ActionError::NoSuchProcess,
        libc::EPERM | libc::EACCES => ActionError::PermissionDenied,
        e => ActionError::Other(e),
    }
}

/// Parses a KERN_PROCARGS2 buffer: argc, exec path, NUL padding, argv strings.
fn parse_procargs2(b: &[u8]) -> Option<Vec<String>> {
    if b.len() < 4 {
        return None;
    }
    let argc = i32::from_ne_bytes([b[0], b[1], b[2], b[3]]);
    if argc <= 0 {
        return None;
    }
    let mut i = 4;
    while i < b.len() && b[i] != 0 {
        i += 1;
    }
    while i < b.len() && b[i] == 0 {
        i += 1;
    }
    let mut v = Vec::with_capacity((argc as usize).min(256));
    while v.len() < argc as usize && i < b.len() {
        let s = i;
        while i < b.len() && b[i] != 0 {
            i += 1;
        }
        v.push(String::from_utf8_lossy(&b[s..i]).into_owned());
        i += 1;
    }
    (!v.is_empty()).then_some(v)
}

/// PLT-127 (cached above).
pub fn username(uid: u32) -> Option<String> {
    let mut pw = MaybeUninit::<libc::passwd>::zeroed();
    let mut buf = [0 as libc::c_char; 2048];
    let mut res: *mut libc::passwd = ptr::null_mut();
    // SAFETY: all pointers are valid for the given sizes.
    let r = unsafe { libc::getpwuid_r(uid, pw.as_mut_ptr(), buf.as_mut_ptr(), buf.len(), &mut res) };
    if r != 0 || res.is_null() {
        return None;
    }
    // SAFETY: on success `res` points to `pw` whose strings live in `buf`.
    let name = unsafe { (*res).pw_name };
    if name.is_null() {
        return None;
    }
    // SAFETY: pw_name is a NUL-terminated string inside `buf`.
    let s = unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned();
    (!s.is_empty()).then_some(s)
}

impl Sys {
    /// One kinfo_proc for `pid`: (start_time, nice). PLT-170 identity check.
    fn kinfo_of(&mut self, pid: u32) -> Option<(u64, i32)> {
        let mut mib = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid as c_int];
        let mut kp = MaybeUninit::<KinfoProc>::zeroed();
        let mut len = KINFO_SIZE;
        // SAFETY: buffer holds one kinfo_proc.
        let r = unsafe { libc::sysctl(mib.as_mut_ptr(), 4, kp.as_mut_ptr().cast(), &mut len, ptr::null_mut(), 0) };
        if r != 0 || len != KINFO_SIZE {
            return None;
        }
        // SAFETY: fully written; plain old data.
        let kp = unsafe { kp.assume_init() };
        (kp.kp_proc.p_pid == pid as i32)
            .then(|| (kp.kp_proc.p_starttime.sec.max(0) as u64, kp.kp_proc.p_nice as i32))
    }

    /// PLT-148, PLT-149, PLT-150.
    pub(super) fn procs_impl(&mut self, fields: ProcFields, visible: &[u32], out: &mut Vec<Proc>) {
        let mut mib = [CTL_KERN, KERN_PROC, KERN_PROC_ALL];
        let Some(len) = sysctl_vec(&mut mib, &mut self.proc_buf) else {
            out.clear();
            return;
        };
        self.cycle = self.cycle.wrapping_add(1);
        let cycle = self.cycle;
        self.vis.clear();
        if fields.io_visible && !fields.io_all {
            self.vis.extend_from_slice(visible);
            self.vis.sort_unstable();
        }
        let tps = self.ticks_per_sec;
        let mut n = 0;
        for k in 0..len / KINFO_SIZE {
            // SAFETY: entry k lies inside the `len` bytes the kernel wrote; read_unaligned
            // because the Vec<u8> buffer has alignment 1.
            let kp: KinfoProc =
                unsafe { ptr::read_unaligned(self.proc_buf.as_ptr().add(k * KINFO_SIZE).cast::<KinfoProc>()) };
            let pid = kp.kp_proc.p_pid;
            if pid <= 0 {
                continue; // PLT-141: kernel_task (pid 0) is excluded.
            }
            let start = kp.kp_proc.p_starttime.sec.max(0) as u64;
            let comm = kp.kp_proc.p_comm;
            let p_stat = kp.kp_proc.p_stat;

            // C2: one proc_pidinfo per process. Zombies have no task to query.
            let ti = if p_stat == SZOMB { Err(libc::ESRCH) } else { task_info(pid) };
            if let Err(e) = ti {
                if e == libc::ESRCH && p_stat != SZOMB {
                    continue; // PLT-006: vanished.
                }
            }

            if n == out.len() {
                out.push(Proc::default());
            }
            let p = &mut out[n];
            p.pid = pid as u32;
            p.ppid = Some(kp.kp_eproc.e_ppid.max(0) as u32);
            p.start_time = start;
            p.uid = Some(kp.kp_eproc.e_ucred.cr_uid);
            p.nice = Some(kp.kp_proc.p_nice as i32);
            p.priority_class = None;
            p.last_cpu = None;
            p.is_kernel_thread = false;

            // Name: proc_name for new identities, re-read when p_comm changes (PLT-125).
            let ent = match self.names.entry(pid as u32) {
                Entry::Occupied(o) => {
                    let e = o.into_mut();
                    if e.start != start || e.comm != comm {
                        e.start = start;
                        e.comm = comm;
                        resolve_name(pid, trim_nul(&comm), &mut e.name);
                    }
                    e
                }
                Entry::Vacant(v) => {
                    let mut name = String::new();
                    resolve_name(pid, trim_nul(&comm), &mut name);
                    v.insert(NameEnt { start, comm, name, seen: cycle })
                }
            };
            ent.seen = cycle;
            p.name.clear();
            p.name.push_str(&ent.name);

            match ti {
                Ok(t) => {
                    p.cpu_user = t.pti_total_user as f64 / tps;
                    p.cpu_system = t.pti_total_system as f64 / tps;
                    p.rss = t.pti_resident_size;
                    p.vms = Some(t.pti_virtual_size);
                    p.num_threads = t.pti_threadnum.max(0) as u32;
                    p.status = status(p_stat, Some(t.pti_numrunning));
                }
                Err(_) => {
                    // Contract deviation (PLT-001 says "never 0 for unknown"): macOS denies
                    // PROC_PIDTASKINFO for other users' processes unless we are root (EPERM),
                    // and zombies have no task at all. `Proc` has no Option for cpu times,
                    // rss or threads, so they are reported as 0; `vms` is the only one of
                    // these fields that can say "unknown", so it is None. Callers should
                    // treat rss == 0 with vms == None as "not readable", not "uses nothing".
                    p.cpu_user = 0.0;
                    p.cpu_system = 0.0;
                    p.rss = 0;
                    p.vms = None;
                    p.num_threads = 0;
                    p.status = status(p_stat, None);
                }
            }

            // PLT-149 I/O: only when requested (PLT-011) and only if task info was allowed.
            p.io_read = None;
            p.io_write = None;
            let want_io = fields.io_all || (fields.io_visible && self.vis.binary_search(&(pid as u32)).is_ok());
            if want_io && ti.is_ok() {
                let mut ri = MaybeUninit::<RusageInfoV2>::zeroed();
                // SAFETY: buffer is a rusage_info_v2.
                if unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V2, ri.as_mut_ptr().cast()) } == 0 {
                    // SAFETY: filled by the kernel.
                    let ri = unsafe { ri.assume_init() };
                    p.io_read = Some(ri.ri_diskio_bytesread);
                    p.io_write = Some(ri.ri_diskio_byteswritten);
                }
            }
            n += 1;
        }
        out.truncate(n);
        // PLT-126: drop identities not seen this cycle.
        self.names.retain(|_, e| e.seen == cycle);
    }

    /// PLT-149 cmdline via KERN_PROCARGS2.
    pub(super) fn cmdline_impl(&mut self, pid: u32, start_time: u64) -> Option<Vec<String>> {
        if pid == 0 || self.kinfo_of(pid)?.0 != start_time {
            return None;
        }
        // Start small (RSS budget, PERF-001); grow to kern.argmax only when an argv
        // does not fit, which is rare.
        if self.args_buf.is_empty() {
            self.args_buf.resize(64 * 1024, 0);
        }
        let mut mib = [CTL_KERN, KERN_PROCARGS2, pid as c_int];
        for _ in 0..2 {
            let mut len = self.args_buf.len();
            // SAFETY: buffer and length describe valid memory.
            let r = unsafe {
                libc::sysctl(mib.as_mut_ptr(), 3, self.args_buf.as_mut_ptr().cast(), &mut len, ptr::null_mut(), 0)
            };
            let err = if r != 0 { errno() } else { 0 };
            // Grow on ENOMEM or on a result that filled the whole buffer (maybe truncated).
            // Denials (EINVAL/EPERM for other users) never grow the buffer.
            let full = if r == 0 { len >= self.args_buf.len() } else { err == libc::ENOMEM };
            if full {
                let max = sysctl_u64(c"kern.argmax").unwrap_or(1 << 20).clamp(64 * 1024, 4 << 20) as usize;
                if self.args_buf.len() < max {
                    self.args_buf.resize(max, 0);
                    continue;
                }
            }
            if r != 0 {
                return None;
            }
            return parse_procargs2(&self.args_buf[..len.min(self.args_buf.len())]);
        }
        None
    }

    /// Section 16, macOS column.
    pub(super) fn extended_impl(&mut self, pid: u32, start_time: u64, want_sockets: bool) -> Option<ProcExtended> {
        if self.kinfo_of(pid)?.0 != start_time {
            return None;
        }
        let ipid = pid as c_int;
        let mut e = ProcExtended::default();
        if let Ok(t) = task_info(ipid) {
            // Total switches only; reported as voluntary, involuntary absent.
            e.ctx_voluntary = Some(t.pti_csw as u32 as u64);
        }
        // num_fds: PROC_PIDLISTFDS size / sizeof(proc_fdinfo) (same UID or root).
        let fd_sz = size_of::<ProcFdInfo>();
        // SAFETY: size probe with a null buffer.
        let est = unsafe { proc_pidinfo(ipid, PROC_PIDLISTFDS, 0, ptr::null_mut(), 0) };
        let mut nfds = None;
        if est > 0 {
            let cap = est as usize / fd_sz + 32;
            if self.fd_buf.len() < cap {
                self.fd_buf.resize(cap, ProcFdInfo::default());
            }
            let bytes = (self.fd_buf.len() * fd_sz) as c_int;
            // SAFETY: buffer holds `bytes` bytes.
            let got = unsafe { proc_pidinfo(ipid, PROC_PIDLISTFDS, 0, self.fd_buf.as_mut_ptr().cast(), bytes) };
            if got >= 0 {
                nfds = Some(got as usize / fd_sz);
            }
        }
        e.num_fds = nfds.map(|n| n as u32);

        if want_sockets {
            let fresh = self
                .sock_cache
                .as_ref()
                .is_some_and(|c| c.pid == pid && c.start == start_time && c.at.elapsed() < SOCK_TTL);
            if !fresh {
                let (tcp, udp) = match nfds {
                    Some(n) => self.count_sockets(ipid, n),
                    None => (None, None),
                };
                self.sock_cache = Some(SockCache { pid, start: start_time, at: Instant::now(), tcp, udp });
            }
            if let Some(c) = &self.sock_cache {
                e.tcp = c.tcp;
                e.udp = c.udp;
            }
        }
        Some(e)
    }

    /// PROC_PIDFDSOCKETINFO per socket fd; classify by family and protocol.
    fn count_sockets(&mut self, pid: c_int, nfds: usize) -> (Option<u32>, Option<u32>) {
        let (mut tcp, mut udp, mut ok) = (0u32, 0u32, true);
        // Larger than struct socket_fdinfo (792 bytes); 8-aligned.
        let mut buf = [0u64; 128];
        for i in 0..nfds.min(self.fd_buf.len()) {
            let fd = self.fd_buf[i];
            if fd.proc_fdtype != PROX_FDTYPE_SOCKET {
                continue;
            }
            // SAFETY: buffer of 1024 bytes.
            let r = unsafe {
                proc_pidfdinfo(pid, fd.proc_fd, PROC_PIDFDSOCKETINFO, buf.as_mut_ptr() as *mut c_void, 1024)
            };
            if r <= 0 {
                if errno() == libc::EPERM {
                    ok = false;
                    break;
                }
                continue; // fd closed meanwhile
            }
            if (r as usize) < SOCKFD_FAMILY_OFF + 4 {
                continue;
            }
            // SAFETY: u64 buffer viewed as bytes, within its 1024-byte size.
            let b = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), 1024) };
            let rd = |o: usize| i32::from_ne_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
            let fam = rd(SOCKFD_FAMILY_OFF);
            if fam != libc::AF_INET && fam != libc::AF_INET6 {
                continue;
            }
            match rd(SOCKFD_PROTOCOL_OFF) {
                libc::IPPROTO_TCP => tcp += 1,
                libc::IPPROTO_UDP => udp += 1,
                _ => {}
            }
        }
        if ok {
            (Some(tcp), Some(udp))
        } else {
            (None, None)
        }
    }

    /// PLT-170, PLT-171, PLT-173.
    pub(super) fn signal_impl(&mut self, pid: u32, start_time: u64, sig: Signal) -> Result<(), ActionError> {
        if pid == std::process::id() || pid == 0 {
            return Err(ActionError::PermissionDenied);
        }
        match self.kinfo_of(pid) {
            Some((s, _)) if s == start_time => {}
            _ => return Err(ActionError::NoSuchProcess),
        }
        let s = match sig {
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        };
        // SAFETY: plain syscall.
        if unsafe { libc::kill(pid as libc::pid_t, s) } == 0 {
            Ok(())
        } else {
            Err(map_errno(errno()))
        }
    }

    /// PLT-170, PLT-172, PLT-173.
    pub(super) fn priority_impl(&mut self, pid: u32, start_time: u64, delta: i32) -> Result<(), ActionError> {
        if pid == std::process::id() || pid == 0 {
            return Err(ActionError::PermissionDenied);
        }
        let nice = match self.kinfo_of(pid) {
            Some((s, nice)) if s == start_time => nice,
            _ => return Err(ActionError::NoSuchProcess),
        };
        let new = (nice + delta).clamp(-20, 19);
        // SAFETY: plain syscall.
        if unsafe { libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, new) } == 0 {
            Ok(())
        } else {
            Err(map_errno(errno()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procargs2() {
        let mut b = 2i32.to_ne_bytes().to_vec();
        b.extend_from_slice(b"/bin/ls\0\0\0\0ls\0-la\0PATH=/bin\0");
        assert_eq!(parse_procargs2(&b).unwrap(), vec!["ls".to_string(), "-la".to_string()]);
        assert!(parse_procargs2(&0i32.to_ne_bytes()).is_none());
        assert!(parse_procargs2(&[1, 0]).is_none());
    }

    #[test]
    fn status_letters() {
        assert_eq!(status(SZOMB, None), b'Z');
        assert_eq!(status(2, Some(1)), b'R');
        assert_eq!(status(2, Some(0)), b'S');
        assert_eq!(status(2, None), b'S');
    }
}
