//! Linux processes (PLT-140..PLT-147, PLT-160, PLT-170..PLT-173; PERF-021..PERF-026).
//!
//! Hot path: one `getdents64` scan of a kept `/proc` fd into a reused 32 KiB buffer
//! (PERF-025), then per pid exactly one `openat` + `read` of `<pid>/stat` (PERF-021).
//! `fstatat` for the owner UID runs only for new identities; `io` only when requested.
//! Output `Proc` entries and their `name` Strings are reused, so steady state does not
//! allocate.

use std::ffi::CStr;
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use super::super::util::{all_digits, fields, find_key, parse_i64, parse_u64, set_string, slot, write_u32};
use super::super::{ActionError, Proc, ProcExtended, ProcFields, Signal};
use super::pread_into;

const PF_KTHREAD: u64 = 0x0020_0000;
const DENTS_SIZE: usize = 32 * 1024;
const STAT_BUF: usize = 4096;

/// Per-identity data kept between cycles (PERF-021/022).
#[derive(Clone, Copy)]
struct Ident {
    pid: u32,
    start: u64,
    uid: Option<u32>,
    /// `io` open failed with EACCES/EPERM; never retried for this identity.
    io_denied: bool,
}

/// Fields copied out of `/proc/<pid>/stat` (PLT-142).
struct Stat<'a> {
    comm: &'a [u8],
    state: u8,
    ppid: i64,
    flags: u64,
    utime: u64,
    stime: u64,
    nice: i64,
    threads: u64,
    start: u64,
    vsize: u64,
    rss: i64,
    processor: Option<u64>,
}

fn parse_stat(b: &[u8]) -> Option<Stat<'_>> {
    // comm is between the first '(' and the LAST ')' (it may contain both).
    let open = b.iter().position(|&c| c == b'(')?;
    let close = b.iter().rposition(|&c| c == b')')?;
    if close < open {
        return None;
    }
    // f[k] is field k + 3 (field 3 = state).
    let mut f: [&[u8]; 37] = [b""; 37];
    for (s, x) in f.iter_mut().zip(fields(&b[close + 1..])) {
        *s = x;
    }
    Some(Stat {
        comm: &b[open + 1..close],
        state: *f[0].first()?,
        ppid: parse_i64(f[1]).unwrap_or(0),
        flags: parse_u64(f[6]).unwrap_or(0),
        utime: parse_u64(f[11]).unwrap_or(0),
        stime: parse_u64(f[12]).unwrap_or(0),
        nice: parse_i64(f[16]).unwrap_or(0),
        threads: parse_u64(f[17]).unwrap_or(1),
        start: parse_u64(f[19])?,
        vsize: parse_u64(f[20]).unwrap_or(0),
        rss: parse_i64(f[21]).unwrap_or(0),
        processor: parse_u64(f[36]),
    })
}

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Build "<pid><suffix>\0" into `buf`; returns a pointer usable as a C path.
fn pid_path(buf: &mut [u8; 48], pid: u32, suffix: &[u8]) -> *const libc::c_char {
    let n = write_u32(buf, pid);
    buf[n..n + suffix.len()].copy_from_slice(suffix);
    buf[n + suffix.len()] = 0;
    buf.as_ptr() as *const libc::c_char
}

/// openat + one read + close. Errors carry errno.
fn read_at(dirfd: RawFd, path: *const libc::c_char, buf: &mut [u8]) -> Result<usize, i32> {
    // SAFETY: path is NUL-terminated (pid_path); buf is a valid writable slice; the fd
    // is closed before returning.
    unsafe {
        let fd = libc::openat(dirfd, path, libc::O_RDONLY | libc::O_CLOEXEC);
        if fd < 0 {
            return Err(errno());
        }
        let n = libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len());
        let e = errno();
        libc::close(fd);
        if n < 0 {
            Err(e)
        } else {
            Ok(n as usize)
        }
    }
}

/// openat a file and read all of it (for files that may exceed one buffer).
fn read_all_at(dirfd: RawFd, path: *const libc::c_char, buf: &mut Vec<u8>) -> Option<usize> {
    // SAFETY: path is NUL-terminated.
    let fd = unsafe { libc::openat(dirfd, path, libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    // SAFETY: fd is a freshly opened descriptor owned by nobody else.
    let f = unsafe { File::from_raw_fd(fd) };
    pread_into(&f, buf, true).ok()
}

/// Fill `buf` with one getdents64 batch. Returns the byte count; 0 at end or on error.
fn getdents(fd: RawFd, buf: &mut [u8]) -> usize {
    // SAFETY: buf is a valid writable buffer of the given length.
    let n = unsafe { libc::syscall(libc::SYS_getdents64, fd, buf.as_mut_ptr(), buf.len()) };
    if n < 0 {
        0
    } else {
        n as usize
    }
}

/// Parse one linux_dirent64 record at `off`: returns (reclen, name start, name end).
/// Layout: d_ino u64, d_off i64, d_reclen u16, d_type u8, d_name[] (NUL-terminated).
fn dirent(buf: &[u8], off: usize) -> (usize, usize, usize) {
    let reclen = u16::from_ne_bytes([buf[off + 16], buf[off + 17]]) as usize;
    let start = off + 19;
    let end_max = (off + reclen).min(buf.len());
    let end = buf[start.min(end_max)..end_max].iter().position(|&b| b == 0).map_or(end_max, |p| start + p);
    (reclen, start, end)
}

pub(super) struct Procs {
    clk_tck: f64,
    page_size: u64,
    dir: Option<OwnedFd>,
    dents: Vec<u8>,
    buf: Vec<u8>,
    big: Vec<u8>,
    prev: Vec<Ident>,
    cur: Vec<Ident>,
    vis: Vec<u32>,
    inodes: Vec<u64>,
    self_pid: u32,
}

impl Procs {
    pub(super) fn new(clk_tck: f64, page_size: u64) -> Self {
        Procs {
            clk_tck,
            page_size,
            dir: None,
            dents: vec![0; DENTS_SIZE],
            buf: vec![0; STAT_BUF],
            big: Vec::new(),
            prev: Vec::new(),
            cur: Vec::new(),
            vis: Vec::new(),
            inodes: Vec::new(),
            self_pid: std::process::id(),
        }
    }

    pub(super) fn reset(&mut self) {
        // F5: forget cached UIDs and io denials (PLT-143).
        self.prev.clear();
        self.cur.clear();
    }

    fn dir_fd(&mut self) -> Option<RawFd> {
        if self.dir.is_none() {
            // SAFETY: constant NUL-terminated path.
            let fd = unsafe {
                libc::open(c"/proc".as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC)
            };
            if fd < 0 {
                return None;
            }
            // SAFETY: fd is freshly opened and owned only here.
            self.dir = Some(unsafe { OwnedFd::from_raw_fd(fd) });
        }
        self.dir.as_ref().map(|d| d.as_raw_fd())
    }

    pub(super) fn processes(&mut self, want: ProcFields, visible: &[u32], out: &mut Vec<Proc>) {
        let Some(dirfd) = self.dir_fd() else {
            out.clear();
            return;
        };
        self.vis.clear();
        if want.io_visible && !want.io_all {
            self.vis.extend_from_slice(visible);
            self.vis.sort_unstable();
        }
        std::mem::swap(&mut self.prev, &mut self.cur);
        self.cur.clear();
        let mut count = 0usize;
        // PERF-025: rewind the kept directory handle.
        // SAFETY: dirfd is a valid open directory fd.
        unsafe { libc::lseek(dirfd, 0, libc::SEEK_SET) };
        loop {
            let n = getdents(dirfd, &mut self.dents);
            if n == 0 {
                break;
            }
            let mut off = 0;
            while off + 19 <= n {
                let (reclen, s, e) = dirent(&self.dents, off);
                if reclen == 0 {
                    break;
                }
                off += reclen;
                let name = &self.dents[s..e];
                if !all_digits(name) {
                    continue;
                }
                let Some(pid) = parse_u64(name) else { continue };
                self.collect_one(dirfd, pid as u32, want.io_all, out, &mut count);
            }
        }
        out.truncate(count);
        // /proc lists pids in ascending order; keep the lookup table sorted regardless.
        if !self.cur.windows(2).all(|w| w[0].pid <= w[1].pid) {
            self.cur.sort_unstable_by_key(|i| i.pid);
        }
    }

    fn collect_one(&mut self, dirfd: RawFd, pid: u32, io_all: bool, out: &mut Vec<Proc>, count: &mut usize) {
        let mut path = [0u8; 48];
        // PLT-142: the only per-cycle read. Vanished or denied pids are dropped (PLT-006).
        let Ok(len) = read_at(dirfd, pid_path(&mut path, pid, b"/stat"), &mut self.buf) else { return };
        let Some(st) = parse_stat(&self.buf[..len]) else { return };
        let p = slot(out, *count);
        *count += 1;
        p.pid = pid;
        p.ppid = (st.ppid > 0).then_some(st.ppid as u32);
        p.start_time = st.start;
        set_string(&mut p.name, st.comm);
        p.status = st.state; // PLT-150: verbatim
        p.nice = Some(st.nice as i32);
        p.priority_class = None;
        p.num_threads = st.threads as u32;
        p.cpu_user = st.utime as f64 / self.clk_tck;
        p.cpu_system = st.stime as f64 / self.clk_tck;
        p.rss = st.rss.max(0) as u64 * self.page_size;
        p.vms = Some(st.vsize);
        p.last_cpu = st.processor.map(|c| c as u32);
        p.is_kernel_thread = st.flags & PF_KTHREAD != 0;
        p.io_read = None;
        p.io_write = None;
        let start = st.start;

        // PLT-143: owner UID once per identity.
        let mut id = match self.prev.binary_search_by_key(&pid, |i| i.pid) {
            Ok(i) if self.prev[i].start == start => self.prev[i],
            _ => {
                // SAFETY: stat is plain data filled by fstatat; path is NUL-terminated.
                let mut sb: libc::stat = unsafe { std::mem::zeroed() };
                let ok = unsafe { libc::fstatat(dirfd, pid_path(&mut path, pid, b""), &mut sb, 0) } == 0;
                Ident { pid, start, uid: ok.then_some(sb.st_uid), io_denied: false }
            }
        };
        p.uid = id.uid;

        // PLT-146 / PERF-021: io only when requested; denial remembered per identity.
        let want_io = io_all || self.vis.binary_search(&pid).is_ok();
        if want_io && !id.io_denied {
            match read_at(dirfd, pid_path(&mut path, pid, b"/io"), &mut self.buf) {
                Ok(n) => {
                    let b = &self.buf[..n];
                    p.io_read = find_key(b, b"read_bytes", b':').and_then(parse_u64);
                    p.io_write = find_key(b, b"write_bytes", b':').and_then(parse_u64);
                }
                Err(e) if e == libc::EACCES || e == libc::EPERM => id.io_denied = true,
                Err(_) => {}
            }
        }
        self.cur.push(id);
    }

    /// Re-read stat for an identity check (PLT-170). Returns the current nice value.
    fn verify(&mut self, pid: u32, start_time: u64) -> Result<i64, ActionError> {
        let dirfd = self.dir_fd().ok_or(ActionError::Other(libc::ENOENT))?;
        let mut path = [0u8; 48];
        let len = match read_at(dirfd, pid_path(&mut path, pid, b"/stat"), &mut self.buf) {
            Ok(n) => n,
            Err(e) if e == libc::EACCES || e == libc::EPERM => return Err(ActionError::PermissionDenied),
            Err(_) => return Err(ActionError::NoSuchProcess),
        };
        match parse_stat(&self.buf[..len]) {
            Some(st) if st.start == start_time => Ok(st.nice),
            _ => Err(ActionError::NoSuchProcess),
        }
    }

    pub(super) fn cmdline(&mut self, pid: u32, start_time: u64) -> Option<Vec<String>> {
        // PLT-145
        self.verify(pid, start_time).ok()?;
        let dirfd = self.dir_fd()?;
        let mut path = [0u8; 48];
        let n = read_all_at(dirfd, pid_path(&mut path, pid, b"/cmdline"), &mut self.big)?;
        // The pid may have been reused during the read.
        self.verify(pid, start_time).ok()?;
        let mut b = &self.big[..n];
        if let [rest @ .., 0] = b {
            b = rest;
        }
        if b.is_empty() {
            return None; // kernel thread or zombie
        }
        let sep = if b.contains(&0) { 0 } else { b' ' };
        let args = b
            .split(|&c| c == sep)
            .filter(|a| sep == 0 || !a.is_empty())
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect();
        Some(args)
    }

    pub(super) fn extended(&mut self, pid: u32, start_time: u64, want_sockets: bool) -> Option<ProcExtended> {
        // Section 16 / PLT-160. Identity first; mismatch means "gone".
        self.verify(pid, start_time).ok()?;
        let dirfd = self.dir_fd()?;
        let mut path = [0u8; 48];
        let mut x = ProcExtended::default();

        if let Some(n) = read_all_at(dirfd, pid_path(&mut path, pid, b"/status"), &mut self.big) {
            let b = &self.big[..n];
            let kv = |k: &[u8]| find_key(b, k, b':').and_then(parse_u64);
            x.ctx_voluntary = kv(b"voluntary_ctxt_switches");
            x.ctx_involuntary = kv(b"nonvoluntary_ctxt_switches");
            x.swap_bytes = kv(b"VmSwap").map(|v| v * 1024);
            x.shared_bytes = kv(b"RssShmem").map(|v| v * 1024);
        }

        // num_fds: count entries of /proc/<pid>/fd; socket inodes via readlinkat.
        self.inodes.clear();
        // SAFETY: path is NUL-terminated.
        let fdfd = unsafe {
            libc::openat(dirfd, pid_path(&mut path, pid, b"/fd"), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC)
        };
        if fdfd >= 0 {
            // SAFETY: fdfd is freshly opened and owned only here.
            let dir = unsafe { OwnedFd::from_raw_fd(fdfd) };
            let mut fds = 0u32;
            let mut link = [0u8; 64];
            loop {
                let n = getdents(dir.as_raw_fd(), &mut self.dents);
                if n == 0 {
                    break;
                }
                let mut off = 0;
                while off + 19 <= n {
                    let (reclen, s, e) = dirent(&self.dents, off);
                    if reclen == 0 {
                        break;
                    }
                    off += reclen;
                    if self.dents[s..e].first() == Some(&b'.') {
                        continue;
                    }
                    fds += 1;
                    if want_sockets && e < self.dents.len() {
                        // SAFETY: the name at s is NUL-terminated at e inside the buffer;
                        // link is a valid writable buffer.
                        let ln = unsafe {
                            libc::readlinkat(
                                dir.as_raw_fd(),
                                self.dents.as_ptr().add(s) as *const libc::c_char,
                                link.as_mut_ptr() as *mut libc::c_char,
                                link.len(),
                            )
                        };
                        if ln > 0 {
                            let l = &link[..ln as usize];
                            if let Some(rest) = l.strip_prefix(b"socket:[") {
                                if let Some(ino) = parse_u64(rest) {
                                    self.inodes.push(ino);
                                }
                            }
                        }
                    }
                }
            }
            x.num_fds = Some(fds);
            if want_sockets {
                self.inodes.sort_unstable();
                x.tcp = Some(self.count_sockets(&["/proc/net/tcp", "/proc/net/tcp6"]));
                x.udp = Some(self.count_sockets(&["/proc/net/udp", "/proc/net/udp6"]));
            }
        }

        // sched_getaffinity
        // SAFETY: cpu_set_t is plain data; the kernel writes at most its size.
        let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::sched_getaffinity(pid as libc::pid_t, std::mem::size_of::<libc::cpu_set_t>(), &mut set) };
        if rc == 0 {
            // SAFETY: CPU_ISSET only reads the set; c < CPU_SETSIZE.
            let n = (0..libc::CPU_SETSIZE as usize).filter(|&c| unsafe { libc::CPU_ISSET(c, &set) }).count();
            x.affinity_cores = Some(n as u32);
        }

        // ioprio_get(IOPRIO_WHO_PROCESS = 1, pid): class = v >> 13, level = v & 7.
        // SAFETY: raw syscall with integer arguments only.
        let v = unsafe { libc::syscall(libc::SYS_ioprio_get, 1 as libc::c_long, pid as libc::c_long) };
        if v >= 0 {
            let class = match v >> 13 {
                0 => Some("none"),
                1 => Some("realtime"),
                2 => Some("best-effort"),
                3 => Some("idle"),
                _ => None,
            };
            x.ionice = class.map(|c| (c, (v & 7) as u8));
        }
        Some(x)
    }

    /// Count lines of the given /proc/net tables whose inode column (field 10) is one
    /// of `self.inodes` (sorted).
    fn count_sockets(&mut self, files: &[&str]) -> u32 {
        if self.inodes.is_empty() {
            return 0;
        }
        let mut c = 0;
        for path in files {
            let Ok(f) = File::open(path) else { continue };
            let Ok(n) = pread_into(&f, &mut self.big, true) else { continue };
            for l in self.big[..n].split(|&b| b == b'\n').skip(1) {
                if let Some(ino) = fields(l).nth(9).and_then(parse_u64) {
                    if ino != 0 && self.inodes.binary_search(&ino).is_ok() {
                        c += 1;
                    }
                }
            }
        }
        c
    }

    pub(super) fn signal(&mut self, pid: u32, start_time: u64, sig: Signal) -> Result<(), ActionError> {
        if pid == self.self_pid {
            return Err(ActionError::PermissionDenied); // PLT-173
        }
        self.verify(pid, start_time)?; // PLT-170
        let s = match sig {
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        };
        // SAFETY: kill has no memory-safety preconditions.
        if unsafe { libc::kill(pid as libc::pid_t, s) } == 0 {
            Ok(())
        } else {
            Err(map_err(errno()))
        }
    }

    pub(super) fn set_priority(&mut self, pid: u32, start_time: u64, delta: i32) -> Result<(), ActionError> {
        if pid == self.self_pid {
            return Err(ActionError::PermissionDenied); // PLT-173
        }
        let nice = self.verify(pid, start_time)?; // PLT-170
        let new = (nice as i32 + delta).clamp(-20, 19); // PLT-172
        // SAFETY: setpriority has no memory-safety preconditions.
        if unsafe { libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, new) } == 0 {
            Ok(())
        } else {
            Err(map_err(errno()))
        }
    }
}

fn map_err(e: i32) -> ActionError {
    match e {
        libc::ESRCH => ActionError::NoSuchProcess,
        libc::EPERM | libc::EACCES => ActionError::PermissionDenied,
        _ => ActionError::Other(e),
    }
}

/// uid -> user name via getpwuid_r (PLT-127; cached by the caller).
pub(super) fn username(uid: u32) -> Option<String> {
    let mut buf: Vec<libc::c_char> = vec![0; 1024];
    loop {
        // SAFETY: passwd is plain data; buf outlives the call and the use of pw_name.
        let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
        let mut res: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe { libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr(), buf.len(), &mut res) };
        if rc == libc::ERANGE && buf.len() < 1 << 20 {
            let n = buf.len() * 2;
            buf.resize(n, 0);
            continue;
        }
        if rc != 0 || res.is_null() || pw.pw_name.is_null() {
            return None;
        }
        // SAFETY: pw_name points into buf and is NUL-terminated.
        return Some(unsafe { CStr::from_ptr(pw.pw_name) }.to_string_lossy().into_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_parse() {
        let s = b"1234 (a) b) c) S 1 1234 1234 0 -1 4194560 100 0 0 0 7 3 0 0 20 0 2 0 5555 1000000 25 18446744073709551615 0 0 0 0 0 0 0 0 0 0 0 0 17 6 0 0\n";
        let st = parse_stat(s).unwrap();
        assert_eq!(st.comm, b"a) b) c");
        assert_eq!(st.state, b'S');
        assert_eq!(st.ppid, 1);
        assert_eq!(st.utime, 7);
        assert_eq!(st.stime, 3);
        assert_eq!(st.nice, 0);
        assert_eq!(st.threads, 2);
        assert_eq!(st.start, 5555);
        assert_eq!(st.vsize, 1000000);
        assert_eq!(st.rss, 25);
        assert_eq!(st.processor, Some(6));
    }

    #[test]
    fn own_process() {
        let mut p = Procs::new(100.0, 4096);
        let mut out = Vec::new();
        p.processes(ProcFields::default(), &[], &mut out);
        let me = std::process::id();
        let mine = out.iter().find(|x| x.pid == me).expect("self listed");
        assert!(mine.uid.is_some());
        assert!(p.cmdline(me, mine.start_time).is_some());
        assert!(p.extended(me, mine.start_time, true).unwrap().num_fds.is_some());
    }
}
