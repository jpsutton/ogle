//! Process enumeration for Windows (PLT-155..PLT-157, PLT-072, PLT-170..PLT-172).

use std::collections::HashMap;
use std::mem::size_of;
use std::ptr::null_mut;
use std::time::{Duration, Instant};

use windows_sys::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessCommandLineInformation};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, LocalFree, ERROR_ACCESS_DENIED, ERROR_INSUFFICIENT_BUFFER,
    ERROR_INVALID_PARAMETER, FILETIME, HANDLE, STATUS_BUFFER_OVERFLOW, STATUS_BUFFER_TOO_SMALL,
    STATUS_INFO_LENGTH_MISMATCH, UNICODE_STRING,
};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{
    GetLengthSid, GetTokenInformation, LookupAccountSidW, TokenUser, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::ffi::{SystemProcessInformation as Spi, SystemThreadInformation as Sti};
use super::{filetime_to_unix, push_utf16};
use crate::platform::ActionError;

/// A snapshot younger than this is reused instead of re-querying, so `processes`,
/// `load_avg` and `process_extended` share one `NtQuerySystemInformation` call per cycle.
const SNAPSHOT_REUSE: Duration = Duration::from_millis(250);
/// `STILL_ACTIVE` exit code (minwinbase.h).
const STILL_ACTIVE: u32 = 259;

/// Reused `SystemProcessInformation` buffer (PLT-155) plus the emulated load (PLT-072).
pub struct Snapshot {
    /// u64 storage keeps the entries 8-byte aligned.
    buf: Vec<u64>,
    len: usize,
    stamp: Option<Instant>,
    ok: bool,
    load: [f64; 3],
    load_at: Option<Instant>,
}

impl Snapshot {
    pub fn new() -> Self {
        Snapshot { buf: Vec::new(), len: 0, stamp: None, ok: false, load: [0.0; 3], load_at: None }
    }

    /// Refresh the snapshot unless the current one is fresh enough. Returns false when
    /// no usable data exists.
    pub fn ensure(&mut self) -> bool {
        if let Some(t) = self.stamp {
            if t.elapsed() < SNAPSHOT_REUSE {
                return self.ok;
            }
        }
        let now = Instant::now();
        self.stamp = Some(now);
        self.ok = self.query();
        if self.ok {
            // PLT-072: count runnable threads, excluding the idle process (pid 0), whose
            // per-CPU idle threads are always in state Running.
            let mut n = 0u32;
            for (p, threads) in self.iter() {
                if p.unique_process_id == 0 {
                    continue;
                }
                n += threads.iter().filter(|t| matches!(t.thread_state, 1..=3)).count() as u32;
            }
            self.feed_load(n as f64, now);
        }
        self.ok
    }

    fn query(&mut self) -> bool {
        if self.buf.is_empty() {
            self.buf.resize(256 * 1024 / 8, 0);
        }
        for _ in 0..8 {
            let cap = (self.buf.len() * 8) as u32;
            let mut ret = 0u32;
            // SAFETY: the buffer is valid for `cap` bytes and outlives the call.
            let st = unsafe {
                NtQuerySystemInformation(SystemProcessInformation, self.buf.as_mut_ptr().cast(), cap, &mut ret)
            };
            if st == STATUS_INFO_LENGTH_MISMATCH || st == STATUS_BUFFER_TOO_SMALL {
                // Headroom for processes started between the two calls.
                let need = (ret as usize).max(cap as usize) + 64 * 1024;
                self.buf.resize(need / 8 + 1, 0);
                continue;
            }
            if st < 0 {
                return false;
            }
            self.len = (ret as usize).min(cap as usize);
            return true;
        }
        false
    }

    fn feed_load(&mut self, n: f64, now: Instant) {
        match self.load_at {
            // Start from the first sample, not from 0.
            None => self.load = [n; 3],
            Some(t) => {
                let dt = now.duration_since(t).as_secs_f64();
                for (l, period) in self.load.iter_mut().zip([60.0, 300.0, 900.0]) {
                    let e = (-dt / period).exp();
                    *l = *l * e + n * (1.0 - e);
                }
            }
        }
        self.load_at = Some(now);
    }

    pub fn load(&self) -> Option<[f64; 3]> {
        self.load_at.map(|_| self.load)
    }

    pub fn iter(&self) -> Iter<'_> {
        let len = if self.ok { self.len } else { 0 };
        // SAFETY: `len` bytes of `buf` were written by the kernel (len <= capacity in bytes).
        let bytes = unsafe { std::slice::from_raw_parts(self.buf.as_ptr() as *const u8, len) };
        Iter { buf: bytes, off: if len > 0 { Some(0) } else { None } }
    }

    /// Entry for `(pid, start_time)` in the current snapshot.
    pub fn find(&self, pid: u32, start_time: u64) -> Option<(&Spi, &[Sti])> {
        self.iter()
            .find(|(p, _)| p.unique_process_id as u32 == pid && filetime_to_unix(p.create_time) == start_time)
    }
}

pub struct Iter<'a> {
    buf: &'a [u8],
    off: Option<usize>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = (&'a Spi, &'a [Sti]);

    fn next(&mut self) -> Option<Self::Item> {
        let off = self.off?;
        let hdr = size_of::<Spi>();
        if off % 8 != 0 || off + hdr > self.buf.len() {
            self.off = None;
            return None;
        }
        // SAFETY: bounds and 8-byte alignment checked above; the kernel wrote a
        // SYSTEM_PROCESS_INFORMATION record at this offset.
        let p = unsafe { &*(self.buf.as_ptr().add(off) as *const Spi) };
        let n = p.number_of_threads as usize;
        let start = off + hdr;
        let threads: &[Sti] = if start + n * size_of::<Sti>() <= self.buf.len() {
            // SAFETY: the thread records follow the process record, inside the buffer.
            unsafe { std::slice::from_raw_parts(self.buf.as_ptr().add(start) as *const Sti, n) }
        } else {
            &[]
        };
        self.off = match p.next_entry_offset {
            0 => None,
            d => Some(off + d as usize),
        };
        Some((p, threads))
    }
}

/// Image name of a process record as UTF-16.
pub fn image_name(p: &Spi) -> &[u16] {
    unicode(&p.image_name)
}

fn unicode(s: &UNICODE_STRING) -> &[u16] {
    if s.Buffer.is_null() || s.Length == 0 {
        return &[];
    }
    // SAFETY: Buffer points to Length bytes of UTF-16 inside the same kernel-filled buffer.
    unsafe { std::slice::from_raw_parts(s.Buffer, s.Length as usize / 2) }
}

/// PLT-150 Windows rule.
pub fn status(threads: &[Sti]) -> u8 {
    let mut all_suspended = !threads.is_empty();
    for t in threads {
        match t.thread_state {
            1..=3 => return b'R',
            5 if t.wait_reason == 5 => {}
            _ => all_suspended = false,
        }
    }
    if all_suspended {
        b'T'
    } else {
        b'S'
    }
}

/// PLT-155: `BasePriority` to priority class label, nearest class for other values.
pub fn priority_label(base: i32) -> &'static str {
    match base {
        i32::MIN..=4 => "Lo",
        5..=6 => "BN",
        7..=8 => "No",
        9..=11 => "AN",
        12..=18 => "Hi",
        _ => "RT",
    }
}

// ---------------------------------------------------------------------------
// Users: SID <-> small integer index.
//
// The contract carries `uid: Option<u32>`. Windows owners are SIDs, so this backend
// interns every distinct SID into `sids` and reports its index as the uid.
// `username(uid)` resolves the SID at that index. Indexes are stable for the session.

pub struct Users {
    /// (pid, CreateTime) -> (uid, generation last seen). Evicted per PLT-126.
    ids: HashMap<(u32, i64), (Option<u32>, u32)>,
    pub gen: u32,
    /// Interned SIDs, stored as u32 words so the SID stays 4-byte aligned.
    sids: Vec<Vec<u32>>,
    tok: Vec<u64>,
}

impl Users {
    pub fn new() -> Self {
        Users { ids: HashMap::new(), gen: 0, sids: Vec::new(), tok: vec![0; 64] }
    }

    /// Owner index for a process identity; resolved once per identity (PLT-156).
    pub fn uid(&mut self, pid: u32, create: i64) -> Option<u32> {
        let gen = self.gen;
        if let Some(e) = self.ids.get_mut(&(pid, create)) {
            e.1 = gen;
            return e.0;
        }
        let uid = self.resolve(pid);
        self.ids.insert((pid, create), (uid, gen));
        uid
    }

    /// Drop identities not seen in the latest enumeration (PLT-126).
    pub fn evict(&mut self) {
        let gen = self.gen;
        self.ids.retain(|_, e| e.1 == gen);
    }

    pub fn clear_identities(&mut self) {
        self.ids.clear();
    }

    fn resolve(&mut self, pid: u32) -> Option<u32> {
        let h = Handle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
        let mut tok: HANDLE = null_mut();
        // SAFETY: valid process handle; `tok` receives a new handle on success.
        if unsafe { OpenProcessToken(h.0, TOKEN_QUERY, &mut tok) } == 0 {
            return None;
        }
        let tok = Handle(tok);
        let mut ok = false;
        for _ in 0..2 {
            let mut len = 0u32;
            let cap = (self.tok.len() * 8) as u32;
            // SAFETY: buffer valid for `cap` bytes.
            if unsafe { GetTokenInformation(tok.0, TokenUser, self.tok.as_mut_ptr().cast(), cap, &mut len) } != 0 {
                ok = true;
                break;
            }
            // SAFETY: plain thread-local error read.
            if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
                return None;
            }
            self.tok.resize(len as usize / 8 + 1, 0);
        }
        if !ok {
            return None;
        }
        // SAFETY: GetTokenInformation(TokenUser) filled a TOKEN_USER whose Sid points
        // into the same buffer; GetLengthSid reads a valid SID.
        let words = unsafe {
            let tu = &*(self.tok.as_ptr() as *const TOKEN_USER);
            let sid = tu.User.Sid;
            let n = GetLengthSid(sid) as usize;
            let mut w = vec![0u32; n.div_ceil(4)];
            std::ptr::copy_nonoverlapping(sid as *const u8, w.as_mut_ptr() as *mut u8, n);
            w
        };
        if let Some(i) = self.sids.iter().position(|s| *s == words) {
            return Some(i as u32);
        }
        self.sids.push(words);
        Some(self.sids.len() as u32 - 1)
    }

    /// `DOMAIN\user` for an interned SID; the SID string when lookup fails (PLT-127).
    pub fn name(&self, uid: u32) -> Option<String> {
        let sid = self.sids.get(uid as usize)?.as_ptr() as *mut core::ffi::c_void;
        let mut name = [0u16; 256];
        let mut dom = [0u16; 256];
        let (mut nl, mut dl, mut usage) = (256u32, 256u32, 0i32);
        // SAFETY: the SID buffer is valid; output buffers sized as passed.
        let ok = unsafe {
            LookupAccountSidW(std::ptr::null(), sid, name.as_mut_ptr(), &mut nl, dom.as_mut_ptr(), &mut dl, &mut usage)
        };
        let mut s = String::new();
        if ok != 0 {
            if dl > 0 {
                push_utf16(&mut s, &dom[..(dl as usize).min(256)]);
                s.push('\\');
            }
            push_utf16(&mut s, &name[..(nl as usize).min(256)]);
            return Some(s);
        }
        let mut p: *mut u16 = null_mut();
        // SAFETY: valid SID; on success `p` is a LocalAlloc'd NUL-terminated string we free.
        unsafe {
            if ConvertSidToStringSidW(sid, &mut p) == 0 || p.is_null() {
                return None;
            }
            let mut n = 0;
            while *p.add(n) != 0 {
                n += 1;
            }
            push_utf16(&mut s, std::slice::from_raw_parts(p, n));
            LocalFree(p.cast());
        }
        Some(s)
    }
}

// ---------------------------------------------------------------------------
// Handles and identity checks.

/// Owned kernel handle, closed on drop (PLT-156: close every handle after use).
pub struct Handle(pub HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we own the handle.
            unsafe { CloseHandle(self.0) };
        }
    }
}

impl Handle {
    pub fn open(pid: u32, access: u32) -> Result<Handle, ActionError> {
        // SAFETY: plain FFI call.
        let h = unsafe { OpenProcess(access, 0, pid) };
        if h.is_null() {
            Err(last_action_error())
        } else {
            Ok(Handle(h))
        }
    }

    /// PLT-170: the process must be alive and have the expected start time.
    pub fn check_identity(&self, start_time: u64) -> Result<(), ActionError> {
        let z = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        // SAFETY: valid handle and out pointers.
        if unsafe { GetProcessTimes(self.0, &mut c, &mut e, &mut k, &mut u) } == 0 {
            return Err(last_action_error());
        }
        let ct = ((c.dwHighDateTime as i64) << 32) | c.dwLowDateTime as i64;
        if filetime_to_unix(ct) != start_time {
            return Err(ActionError::NoSuchProcess);
        }
        let mut code = 0u32;
        // SAFETY: valid handle and out pointer.
        if unsafe { GetExitCodeProcess(self.0, &mut code) } != 0 && code != STILL_ACTIVE {
            return Err(ActionError::NoSuchProcess);
        }
        Ok(())
    }
}

pub fn last_action_error() -> ActionError {
    // SAFETY: plain thread-local error read.
    match unsafe { GetLastError() } {
        ERROR_ACCESS_DENIED => ActionError::PermissionDenied,
        ERROR_INVALID_PARAMETER => ActionError::NoSuchProcess,
        e => ActionError::Other(e as i32),
    }
}

/// PLT-156: command line through `ProcessCommandLineInformation`.
pub fn cmdline(pid: u32, start_time: u64, buf: &mut Vec<u64>) -> Option<Vec<String>> {
    let h = Handle::open(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok()?;
    h.check_identity(start_time).ok()?;
    if buf.is_empty() {
        buf.resize(512, 0);
    }
    let mut ok = false;
    for _ in 0..3 {
        let cap = (buf.len() * 8) as u32;
        let mut ret = 0u32;
        // SAFETY: buffer valid for `cap` bytes.
        let st = unsafe {
            NtQueryInformationProcess(h.0, ProcessCommandLineInformation, buf.as_mut_ptr().cast(), cap, &mut ret)
        };
        if st == STATUS_INFO_LENGTH_MISMATCH || st == STATUS_BUFFER_TOO_SMALL || st == STATUS_BUFFER_OVERFLOW {
            buf.resize((ret as usize).max(cap as usize * 2) / 8 + 1, 0);
            continue;
        }
        ok = st >= 0;
        break;
    }
    if !ok {
        return None;
    }
    // SAFETY: on success the buffer starts with a UNICODE_STRING pointing into itself.
    let s = unicode(unsafe { &*(buf.as_ptr() as *const UNICODE_STRING) });
    let v = split_cmdline(s);
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// Split a command line with the `CommandLineToArgvW` rules: the program name ends at
/// the closing quote or first whitespace with no escapes; later arguments use the
/// backslash/quote rules (2n backslashes + `"` -> n backslashes and a quote toggle,
/// 2n+1 -> n backslashes and a literal `"`, `""` inside quotes -> literal `"`).
pub fn split_cmdline(s: &[u16]) -> Vec<String> {
    const SP: u16 = b' ' as u16;
    const TAB: u16 = b'\t' as u16;
    const Q: u16 = b'"' as u16;
    const BS: u16 = b'\\' as u16;
    let mut out = Vec::new();
    let mut cur: Vec<u16> = Vec::new();
    let mut i = 0;
    // Program name.
    if s.first() == Some(&Q) {
        i = 1;
        while i < s.len() && s[i] != Q {
            cur.push(s[i]);
            i += 1;
        }
        i += 1;
    } else {
        while i < s.len() && s[i] != SP && s[i] != TAB {
            cur.push(s[i]);
            i += 1;
        }
    }
    if s.is_empty() {
        return out;
    }
    out.push(String::from_utf16_lossy(&cur));
    loop {
        while i < s.len() && (s[i] == SP || s[i] == TAB) {
            i += 1;
        }
        if i >= s.len() {
            break;
        }
        cur.clear();
        let mut quoted = false;
        while i < s.len() {
            let c = s[i];
            if (c == SP || c == TAB) && !quoted {
                break;
            }
            if c == BS {
                let mut n = 0;
                while i < s.len() && s[i] == BS {
                    n += 1;
                    i += 1;
                }
                if i < s.len() && s[i] == Q {
                    cur.extend(std::iter::repeat(BS).take(n / 2));
                    if n % 2 == 1 {
                        cur.push(Q);
                        i += 1;
                    }
                } else {
                    cur.extend(std::iter::repeat(BS).take(n));
                }
                continue;
            }
            if c == Q {
                if quoted && s.get(i + 1) == Some(&Q) {
                    cur.push(Q);
                    i += 2;
                    continue;
                }
                quoted = !quoted;
                i += 1;
                continue;
            }
            cur.push(c);
            i += 1;
        }
        out.push(String::from_utf16_lossy(&cur));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(s: &str) -> Vec<String> {
        let w: Vec<u16> = s.encode_utf16().collect();
        split_cmdline(&w)
    }

    #[test]
    fn cmdline_rules() {
        assert_eq!(split(r#""C:\Program Files\a.exe" -x "y z""#), ["C:\\Program Files\\a.exe", "-x", "y z"]);
        assert_eq!(split(r#"a.exe a\\\"b c\\"d e""#), ["a.exe", "a\\\"b", "c\\d e"]);
        assert_eq!(split(r#"a.exe "x""y""#), ["a.exe", "x\"y"]);
        assert_eq!(split("a.exe  b\\c "), ["a.exe", "b\\c"]);
        assert!(split("").is_empty());
    }

    #[test]
    fn priority_labels() {
        assert_eq!(priority_label(4), "Lo");
        assert_eq!(priority_label(6), "BN");
        assert_eq!(priority_label(8), "No");
        assert_eq!(priority_label(10), "AN");
        assert_eq!(priority_label(13), "Hi");
        assert_eq!(priority_label(24), "RT");
    }
}
