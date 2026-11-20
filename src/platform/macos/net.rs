//! Network interfaces (PLT-101) and file systems (PLT-121) on macOS.

use std::mem::{size_of, MaybeUninit};
use std::ptr;

use libc::c_int;

use super::ffi::{CTL_NET, NET_RT_IFLIST2, PF_ROUTE, RTM_IFINFO2};
use super::{cchars, set_str, sysctl_vec, trim_nul, Sys};
use crate::platform::{FsUsage, Mount, NetIf};

// Byte offsets in `struct if_msghdr2` (net/if.h); `if_data64` starts at 32.
const IFM_FLAGS: usize = 8;
const IFM_INDEX: usize = 12;
const IFI_BAUDRATE: usize = 32 + 16;
const IFI_IBYTES: usize = 32 + 64;
const IFI_OBYTES: usize = 32 + 72;
const IFM2_MIN_LEN: usize = IFI_OBYTES + 8;

fn rd_u16(b: &[u8], o: usize) -> u16 {
    u16::from_ne_bytes([b[o], b[o + 1]])
}
fn rd_i32(b: &[u8], o: usize) -> i32 {
    i32::from_ne_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn rd_u64(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_ne_bytes(a)
}

/// Types never counted as physical storage (PLT-121).
const VIRTUAL_FS: [&[u8]; 5] = [b"devfs", b"autofs", b"nullfs", b"tmpfs", b"fdescfs"];

/// PLT-121 usage from the cached statfs figures. Units are `f_bsize` on macOS.
pub fn fs_usage(mount_point: &str) -> Option<FsUsage> {
    // NUL-terminated copy on the stack (MAXPATHLEN = 1024): no per-call allocation.
    let src = mount_point.as_bytes();
    let mut path = [0u8; 1025];
    if src.len() >= path.len() || src.contains(&0) {
        return None;
    }
    path[..src.len()].copy_from_slice(src);
    let mut st = MaybeUninit::<libc::statfs>::zeroed();
    // SAFETY: NUL-terminated path buffer and valid out pointer.
    if unsafe { libc::statfs(path.as_ptr().cast(), st.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: statfs succeeded.
    let st = unsafe { st.assume_init() };
    let bs = st.f_bsize as u64;
    Some(FsUsage { total: st.f_blocks * bs, free: st.f_bfree * bs, avail: st.f_bavail * bs })
}

impl Sys {
    /// Cached `if_indextoname` (C0 per index).
    fn ifname(&mut self, index: u16) -> Option<usize> {
        if let Some(i) = self.ifnames.iter().position(|(ix, _)| *ix == index) {
            return Some(i);
        }
        let mut buf = [0 as libc::c_char; 16];
        // SAFETY: buffer has IF_NAMESIZE (16) bytes as required.
        let r = unsafe { libc::if_indextoname(index as libc::c_uint, buf.as_mut_ptr()) };
        if r.is_null() {
            return None;
        }
        let name = String::from_utf8_lossy(cchars(&buf)).into_owned();
        self.ifnames.push((index, name));
        Some(self.ifnames.len() - 1)
    }

    /// PLT-101: one NET_RT_IFLIST2 sysctl for counters, flags and speed.
    pub(super) fn net_impl(&mut self, want_addrs: bool, out: &mut Vec<NetIf>) {
        let mut mib = [CTL_NET, PF_ROUTE, 0, 0, NET_RT_IFLIST2, 0];
        let mut buf = std::mem::take(&mut self.net_buf);
        let Some(len) = sysctl_vec(&mut mib, &mut buf) else {
            self.net_buf = buf;
            out.clear();
            return;
        };
        let data = &buf[..len.min(buf.len())];
        let mut off = 0;
        let mut n = 0;
        while off + 4 <= data.len() {
            let msglen = rd_u16(data, off) as usize;
            if msglen == 0 || off + msglen > data.len() {
                break;
            }
            let rec = &data[off..off + msglen];
            off += msglen;
            if rec[3] != RTM_IFINFO2 || msglen < IFM2_MIN_LEN {
                continue;
            }
            let flags = rd_i32(rec, IFM_FLAGS);
            let Some(ni) = self.ifname(rd_u16(rec, IFM_INDEX)) else { continue };
            if n == out.len() {
                out.push(NetIf::default());
            }
            let e = &mut out[n];
            e.name.clear();
            e.name.push_str(&self.ifnames[ni].1);
            e.rx_bytes = rd_u64(rec, IFI_IBYTES);
            e.tx_bytes = rd_u64(rec, IFI_OBYTES);
            e.is_up = flags & libc::IFF_UP != 0 && flags & libc::IFF_RUNNING != 0;
            let baud = rd_u64(rec, IFI_BAUDRATE);
            e.speed_bps = (baud > 0).then_some(baud);
            e.has_ip = !want_addrs;
            n += 1;
        }
        out.truncate(n);
        self.net_buf = buf;
        if want_addrs {
            mark_addrs(out);
        }
    }

    /// PLT-121: getfsstat(MNT_NOWAIT) into a reused buffer.
    pub(super) fn fs_impl(&mut self, out: &mut Vec<Mount>) {
        // SAFETY: a null buffer asks for the mount count.
        let count = unsafe { libc::getfsstat(ptr::null_mut(), 0, libc::MNT_NOWAIT) };
        if count <= 0 {
            out.clear();
            return;
        }
        self.fs_buf.clear();
        self.fs_buf.reserve(count as usize + 4);
        let cap = self.fs_buf.capacity();
        let bytes = (cap * size_of::<libc::statfs>()).min(c_int::MAX as usize) as c_int;
        // SAFETY: the buffer has room for `cap` statfs entries.
        let got = unsafe { libc::getfsstat(self.fs_buf.as_mut_ptr(), bytes, libc::MNT_NOWAIT) };
        if got <= 0 {
            out.clear();
            return;
        }
        // SAFETY: the kernel initialised `got` (<= cap) entries.
        unsafe { self.fs_buf.set_len((got as usize).min(cap)) };
        let mut n = 0;
        for st in &self.fs_buf {
            let flags = st.f_flags;
            let fstype = cchars(&st.f_fstypename);
            let on = cchars(&st.f_mntonname);
            let local = flags & libc::MNT_LOCAL as u32 != 0;
            // PLT-121: hidden (nobrowse) mounts and APFS system volumes are not user-facing.
            let hidden = flags & libc::MNT_DONTBROWSE as u32 != 0 || on.starts_with(b"/System/Volumes/");
            if n == out.len() {
                out.push(Mount::default());
            }
            let m = &mut out[n];
            set_str(&mut m.device, cchars(&st.f_mntfromname));
            set_str(&mut m.mount_point, on);
            set_str(&mut m.fs_type, fstype);
            m.read_only = flags & libc::MNT_RDONLY as u32 != 0;
            m.is_network = !local;
            m.is_physical = local && !hidden && !VIRTUAL_FS.iter().any(|t| *t == fstype);
            n += 1;
        }
        out.truncate(n);
    }
}

/// `has_ip` from getifaddrs: any AF_INET/AF_INET6 entry for the name (PLT-101).
fn mark_addrs(out: &mut [NetIf]) {
    let mut ifa: *mut libc::ifaddrs = ptr::null_mut();
    // SAFETY: valid out pointer; freed below.
    if unsafe { libc::getifaddrs(&mut ifa) } != 0 {
        return;
    }
    let mut cur = ifa;
    while !cur.is_null() {
        // SAFETY: `cur` is a node of the list returned by getifaddrs.
        let a = unsafe { &*cur };
        cur = a.ifa_next;
        if a.ifa_addr.is_null() || a.ifa_name.is_null() {
            continue;
        }
        // SAFETY: ifa_addr is non-null and points to a sockaddr.
        let fam = unsafe { (*a.ifa_addr).sa_family } as c_int;
        if fam != libc::AF_INET && fam != libc::AF_INET6 {
            continue;
        }
        // SAFETY: ifa_name is a NUL-terminated string owned by the list.
        let name = unsafe { std::ffi::CStr::from_ptr(a.ifa_name) }.to_bytes();
        let name = trim_nul(name);
        if let Some(e) = out.iter_mut().find(|e| e.name.as_bytes() == name) {
            e.has_ip = true;
        }
    }
    // SAFETY: list from getifaddrs, freed once.
    unsafe { libc::freeifaddrs(ifa) };
}
