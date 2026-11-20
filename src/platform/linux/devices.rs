//! Linux network interfaces, disk I/O and file systems (PLT-100, PLT-110, PLT-120).

use std::ffi::{CStr, CString};
use std::fs::File;
use std::os::fd::AsRawFd;

use super::super::util::{fields, lines, parse_hex, parse_i64, parse_u64, set_string, slot, trim};
use super::super::{DiskIo, DiskKind, FsUsage, Mount, NetIf};
use super::{pread_into, read_path, SysFile};

/// PLT-100 speed cache: re-read only for new interfaces or when `is_up` changes.
struct Speed {
    name: String,
    up: bool,
    speed: Option<u64>,
    seen: bool,
}

pub(super) struct Devices {
    net_dev: SysFile,
    diskstats: SysFile,
    buf: Vec<u8>,
    path: String,
    speeds: Vec<Speed>,
    /// PLT-110: kind computed once per disk name.
    kinds: Vec<(String, DiskKind)>,
    mounts_fd: Option<File>,
    mounts: Vec<Mount>,
    mounts_valid: bool,
    /// Types from /proc/filesystems without `nodev` (C0).
    physical_fs: Option<Vec<String>>,
}

const NETWORK_FS: &[&str] = &["nfs", "nfs4", "cifs", "smb3", "smbfs", "fuse.sshfs", "9p", "afs", "ceph"];
const VIRTUAL_DISKS: &[&str] = &["loop", "ram", "zram", "dm-", "md"];

impl Devices {
    pub(super) fn new() -> Self {
        Devices {
            net_dev: SysFile::new("/proc/net/dev"),
            diskstats: SysFile::new("/proc/diskstats"),
            buf: Vec::new(),
            path: String::new(),
            speeds: Vec::new(),
            kinds: Vec::new(),
            mounts_fd: None,
            mounts: Vec::new(),
            mounts_valid: false,
            physical_fs: None,
        }
    }

    pub(super) fn reset(&mut self) {
        self.speeds.clear();
        self.kinds.clear();
        self.mounts_valid = false;
        self.physical_fs = None;
    }

    pub(super) fn net_interfaces(&mut self, want_addrs: bool, out: &mut Vec<NetIf>) {
        // PLT-100 counters: /proc/net/dev, columns 1 and 9 after the colon.
        let Some(n) = self.net_dev.read(&mut self.buf) else {
            out.clear();
            return;
        };
        let mut count = 0;
        for l in lines(&self.buf[..n]).skip(2) {
            let Some(c) = l.iter().position(|&b| b == b':') else { continue };
            let mut it = fields(&l[c + 1..]);
            let rx = it.next().and_then(parse_u64);
            let tx = it.nth(7).and_then(parse_u64);
            let (Some(rx), Some(tx)) = (rx, tx) else { continue };
            let e = slot(out, count);
            count += 1;
            set_string(&mut e.name, trim(&l[..c]));
            e.rx_bytes = rx;
            e.tx_bytes = tx;
            e.is_up = false;
            e.has_ip = !want_addrs;
            e.speed_bps = None;
        }
        out.truncate(count);

        // is_up and addresses: one getifaddrs() call.
        let mut ifa: *mut libc::ifaddrs = std::ptr::null_mut();
        // SAFETY: ifa receives a list we free below.
        if unsafe { libc::getifaddrs(&mut ifa) } == 0 {
            let mut p = ifa;
            while !p.is_null() {
                // SAFETY: p is a node of the list returned by getifaddrs.
                let a = unsafe { &*p };
                p = a.ifa_next;
                if a.ifa_name.is_null() || a.ifa_addr.is_null() {
                    continue;
                }
                // SAFETY: ifa_name is a NUL-terminated string owned by the list.
                let name = unsafe { CStr::from_ptr(a.ifa_name) }.to_bytes();
                let Some(e) = out.iter_mut().find(|e| e.name.as_bytes() == name) else { continue };
                // SAFETY: ifa_addr is non-null and points to a sockaddr.
                let fam = unsafe { (*a.ifa_addr).sa_family } as libc::c_int;
                if fam == libc::AF_PACKET {
                    let up = libc::IFF_UP as libc::c_uint | libc::IFF_RUNNING as libc::c_uint;
                    e.is_up = a.ifa_flags & up == up;
                } else if want_addrs && (fam == libc::AF_INET || fam == libc::AF_INET6) {
                    e.has_ip = true;
                }
            }
            // SAFETY: ifa came from a successful getifaddrs.
            unsafe { libc::freeifaddrs(ifa) };
        } else {
            // Fallback: /sys/class/net/<if>/flags (IFF_UP 0x1, IFF_RUNNING 0x40).
            for e in out.iter_mut() {
                self.path.clear();
                self.path.push_str("/sys/class/net/");
                self.path.push_str(&e.name);
                self.path.push_str("/flags");
                if let Some(n) = read_path(&self.path, &mut self.buf) {
                    e.is_up = parse_hex(&self.buf[..n]).is_some_and(|f| f & 0x41 == 0x41);
                }
            }
        }

        // speed_bps (C2, only on change).
        for s in self.speeds.iter_mut() {
            s.seen = false;
        }
        for e in out.iter_mut() {
            let i = match self.speeds.iter().position(|s| s.name == e.name) {
                Some(i) if self.speeds[i].up == e.is_up => i,
                found => {
                    self.path.clear();
                    self.path.push_str("/sys/class/net/");
                    self.path.push_str(&e.name);
                    self.path.push_str("/speed");
                    // EINVAL when down, -1 for virtual interfaces: absent.
                    let speed = read_path(&self.path, &mut self.buf)
                        .and_then(|n| parse_i64(&self.buf[..n]))
                        .filter(|&v| v > 0)
                        .map(|v| v as u64 * 1_000_000);
                    match found {
                        Some(i) => {
                            self.speeds[i].up = e.is_up;
                            self.speeds[i].speed = speed;
                            i
                        }
                        None => {
                            self.speeds.push(Speed { name: e.name.clone(), up: e.is_up, speed, seen: false });
                            self.speeds.len() - 1
                        }
                    }
                }
            };
            self.speeds[i].seen = true;
            e.speed_bps = self.speeds[i].speed;
        }
        self.speeds.retain(|s| s.seen);
    }

    pub(super) fn disk_io(&mut self, out: &mut Vec<DiskIo>) {
        // PLT-110
        let Some(n) = self.diskstats.read(&mut self.buf) else {
            out.clear();
            return;
        };
        let mut count = 0;
        for l in lines(&self.buf[..n]) {
            let mut f: [&[u8]; 11] = [b""; 11];
            for (s, x) in f.iter_mut().zip(fields(l)) {
                *s = x;
            }
            let name = f[2];
            let (Some(rd), Some(rs), Some(wr), Some(ws)) =
                (parse_u64(f[3]), parse_u64(f[5]), parse_u64(f[7]), parse_u64(f[9]))
            else {
                continue;
            };
            let kind = disk_kind(&mut self.kinds, &mut self.path, name);
            let d = slot(out, count);
            count += 1;
            set_string(&mut d.name, name);
            d.kind = kind;
            d.read_count = rd;
            d.write_count = wr;
            d.read_bytes = rs * 512;
            d.write_bytes = ws * 512;
            d.read_time_ms = parse_u64(f[6]);
            d.write_time_ms = parse_u64(f[10]);
        }
        out.truncate(count);
    }

    pub(super) fn filesystems(&mut self, out: &mut Vec<Mount>) {
        // PLT-120: re-parse only when poll() reports POLLPRI on the mounts fd.
        if self.mounts_fd.is_none() {
            self.mounts_fd = File::open("/proc/self/mounts").ok();
            self.mounts_valid = false;
        }
        if let Some(f) = &self.mounts_fd {
            let mut p = libc::pollfd { fd: f.as_raw_fd(), events: libc::POLLPRI, revents: 0 };
            // SAFETY: p is a valid pollfd; timeout 0 never blocks.
            let r = unsafe { libc::poll(&mut p, 1, 0) };
            if r > 0 && p.revents & (libc::POLLPRI | libc::POLLERR) != 0 {
                self.mounts_valid = false;
            }
        }
        if !self.mounts_valid {
            self.parse_mounts();
        }
        for (i, m) in self.mounts.iter().enumerate() {
            let o = slot(out, i);
            o.device.clone_from(&m.device);
            o.mount_point.clone_from(&m.mount_point);
            o.fs_type.clone_from(&m.fs_type);
            o.is_physical = m.is_physical;
            o.is_network = m.is_network;
            o.read_only = m.read_only;
        }
        out.truncate(self.mounts.len());
    }

    fn parse_mounts(&mut self) {
        if self.physical_fs.is_none() {
            let mut v = Vec::new();
            if let Some(n) = read_path("/proc/filesystems", &mut self.buf) {
                for l in lines(&self.buf[..n]) {
                    if !l.starts_with(b"nodev") {
                        let t = trim(l);
                        if !t.is_empty() {
                            v.push(String::from_utf8_lossy(t).into_owned());
                        }
                    }
                }
            }
            self.physical_fs = Some(v);
        }
        let Some(f) = &self.mounts_fd else {
            self.mounts.clear();
            return;
        };
        let Ok(n) = pread_into(f, &mut self.buf, true) else {
            self.mounts_fd = None;
            return;
        };
        let phys = self.physical_fs.as_deref().unwrap_or(&[]);
        let mut count = 0;
        for l in lines(&self.buf[..n]) {
            let mut it = fields(l);
            let (Some(dev), Some(mp), Some(ty), Some(opts)) = (it.next(), it.next(), it.next(), it.next()) else {
                continue;
            };
            let m = slot(&mut self.mounts, count);
            count += 1;
            unescape(&mut m.device, dev);
            unescape(&mut m.mount_point, mp);
            unescape(&mut m.fs_type, ty);
            m.read_only = opts.split(|&b| b == b',').any(|o| o == b"ro");
            m.is_network = NETWORK_FS.contains(&m.fs_type.as_str());
            m.is_physical = m.fs_type == "zfs" || phys.iter().any(|p| *p == m.fs_type);
        }
        self.mounts.truncate(count);
        self.mounts_valid = true;
    }
}

/// PLT-110 kind, cached per name.
fn disk_kind(cache: &mut Vec<(String, DiskKind)>, path: &mut String, name: &[u8]) -> DiskKind {
    if let Some((_, k)) = cache.iter().find(|(n, _)| n.as_bytes() == name) {
        return *k;
    }
    let s = String::from_utf8_lossy(name).into_owned();
    // Virtual devices are checked first: loop/dm/md also appear under /sys/block.
    let kind = if VIRTUAL_DISKS.iter().any(|p| s.starts_with(p)) {
        DiskKind::Virtual
    } else {
        path.clear();
        path.push_str("/sys/block/");
        path.push_str(&s);
        if std::path::Path::new(path.as_str()).exists() {
            DiskKind::Disk
        } else {
            // Not a whole disk: /sys/class/block/<name>/partition exists for partitions,
            // and diskstats lists nothing else.
            DiskKind::Partition
        }
    };
    cache.push((s, kind));
    kind
}

/// Decode `\ooo` octal escapes used in /proc/self/mounts (`\040`, `\011`, `\012`, `\134`).
fn unescape(out: &mut String, b: &[u8]) {
    out.clear();
    let mut i = 0;
    let mut tmp: [u8; 4] = [0; 4];
    let mut start = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            super::super::util::push_bytes_lossy(out, &b[start..i]);
            let v = (b[i + 1] - b'0') as u32 * 64 + (b[i + 2] - b'0') as u32 * 8 + (b[i + 3] - b'0') as u32;
            tmp[0] = v as u8;
            super::super::util::push_bytes_lossy(out, &tmp[..1]);
            i += 4;
            start = i;
        } else {
            i += 1;
        }
    }
    super::super::util::push_bytes_lossy(out, &b[start..]);
}

/// PLT-120 usage via statvfs. May block; callers run it on the fs worker (PLT-010).
pub(super) fn fs_usage(mount_point: &str) -> Option<FsUsage> {
    let c = CString::new(mount_point).ok()?;
    // SAFETY: statvfs is plain data; c is NUL-terminated.
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let fr = if s.f_frsize > 0 { s.f_frsize as u64 } else { s.f_bsize as u64 };
    Some(FsUsage {
        total: s.f_blocks as u64 * fr,
        free: s.f_bfree as u64 * fr,
        avail: s.f_bavail as u64 * fr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mount_escapes() {
        let mut s = String::new();
        unescape(&mut s, b"/mnt/my\\040disk\\134x");
        assert_eq!(s, "/mnt/my disk\\x");
    }

    #[test]
    fn root_usage() {
        let u = fs_usage("/").unwrap();
        assert!(u.total >= u.free && u.free >= u.avail);
    }
}
