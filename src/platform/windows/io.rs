//! Network, disk, file system, battery and socket sources for Windows
//! (PLT-103, PLT-113, PLT-122, PLT-134, section 16 tcp/udp counts).

use std::ptr::{addr_of, null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_BUFFER_OVERFLOW, ERROR_INSUFFICIENT_BUFFER, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GetExtendedTcpTable, GetExtendedUdpTable, GetIfEntry2, GetNumberOfInterfaces,
    GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
    MIB_IF_ROW2, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_TCPTABLE_OWNER_PID, MIB_UDP6ROW_OWNER_PID, MIB_UDP6TABLE_OWNER_PID, MIB_UDPROW_OWNER_PID,
    MIB_UDPTABLE_OWNER_PID, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
};
use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, GetVolumeInformationW,
    FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Ioctl::{DISK_PERFORMANCE, IOCTL_DISK_PERFORMANCE};
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows_sys::Win32::System::WindowsProgramming::{
    DRIVE_FIXED, DRIVE_NO_ROOT_DIR, DRIVE_REMOTE, DRIVE_REMOVABLE,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

use super::ffi::FILE_READ_ONLY_VOLUME;
use super::{push_u32, push_utf16, slot};
use crate::platform::{Battery, BatteryStatus, DiskIo, DiskKind, FsUsage, Mount, NetIf};

const ADAPTER_TTL: Duration = Duration::from_secs(30);
const DISK_TTL: Duration = Duration::from_secs(30);
const VOLUME_TTL: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Network (PLT-103)

#[derive(Default)]
struct Adapter {
    index: u32,
    name: String,
    has_ip: bool,
}

pub struct Net {
    adapters: Vec<Adapter>,
    count: usize,
    at: Option<Instant>,
    /// `GetNumberOfInterfaces` at the last refresh; a change triggers an adapter refresh.
    rows: u32,
    buf: Vec<u64>,
}

impl Net {
    pub fn new() -> Self {
        Net { adapters: Vec::new(), count: 0, at: None, rows: 0, buf: Vec::new() }
    }

    pub fn invalidate(&mut self) {
        self.at = None;
    }

    /// `GetAdaptersAddresses` (C3): interface set, friendly names and address presence.
    fn refresh_adapters(&mut self) {
        self.at = Some(Instant::now());
        self.count = 0;
        if self.buf.is_empty() {
            self.buf.resize(16 * 1024 / 8, 0);
        }
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut ok = false;
        for _ in 0..4 {
            let mut size = (self.buf.len() * 8) as u32;
            // SAFETY: buffer valid for `size` bytes; AF_UNSPEC = 0.
            let r = unsafe { GetAdaptersAddresses(0, flags, null(), self.buf.as_mut_ptr().cast(), &mut size) };
            if r == ERROR_BUFFER_OVERFLOW {
                self.buf.resize(size as usize / 8 + 1, 0);
                continue;
            }
            ok = r == 0;
            break;
        }
        if !ok {
            return;
        }
        let mut p = self.buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !p.is_null() {
            // SAFETY: linked list built by GetAdaptersAddresses inside `buf`.
            let a = unsafe { &*p };
            // SAFETY: both union members share the same leading layout; IfIndex is valid.
            let mut index = unsafe { a.Anonymous1.Anonymous.IfIndex };
            if index == 0 {
                index = a.Ipv6IfIndex;
            }
            let e = slot(&mut self.adapters, self.count);
            self.count += 1;
            e.index = index;
            e.has_ip = !a.FirstUnicastAddress.is_null();
            e.name.clear();
            if !a.FriendlyName.is_null() {
                // SAFETY: NUL-terminated wide string inside `buf`.
                push_utf16(&mut e.name, unsafe { wstr(a.FriendlyName) });
            }
            p = a.Next;
        }
    }

    /// One `GetIfEntry2` per known adapter into a stack row: unlike `GetIfTable2`, this
    /// allocates nothing per cycle. Only adapters listed by `GetAdaptersAddresses` are
    /// queried, which also drops filter-driver pseudo-interfaces.
    pub fn interfaces(&mut self, want_addrs: bool, out: &mut Vec<NetIf>) {
        let mut n = 0u32;
        // SAFETY: valid out pointer. A changed count triggers an adapter refresh.
        let have_n = unsafe { GetNumberOfInterfaces(&mut n) } == 0;
        if self.at.map_or(true, |t| t.elapsed() >= ADAPTER_TTL) || (have_n && n != self.rows) {
            self.rows = n;
            self.refresh_adapters();
        }
        let mut i = 0;
        for a in &self.adapters[..self.count] {
            // SAFETY: MIB_IF_ROW2 is plain data; all-zero is a valid value.
            let mut row: MIB_IF_ROW2 = unsafe { std::mem::zeroed() };
            row.InterfaceIndex = a.index;
            // SAFETY: valid row with InterfaceIndex set; the call fills the rest.
            if unsafe { GetIfEntry2(&mut row) } != 0 {
                continue;
            }
            let e = slot(out, i);
            i += 1;
            e.name.clear();
            e.name.push_str(&a.name);
            e.rx_bytes = row.InOctets;
            e.tx_bytes = row.OutOctets;
            e.is_up = row.OperStatus == IfOperStatusUp;
            let sp = row.ReceiveLinkSpeed.max(row.TransmitLinkSpeed);
            e.speed_bps = if sp == 0 || sp == u64::MAX { None } else { Some(sp) };
            e.has_ip = if want_addrs { a.has_ip } else { true };
        }
        out.truncate(i);
    }
}

/// NUL-terminated wide string as a slice.
///
/// # Safety
/// `p` must point to a valid NUL-terminated UTF-16 string.
unsafe fn wstr<'a>(p: *const u16) -> &'a [u16] {
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    std::slice::from_raw_parts(p, n)
}

// ---------------------------------------------------------------------------
// Disks (PLT-113)

pub struct Disks {
    handles: Vec<(u32, HANDLE)>,
    at: Option<Instant>,
}

impl Disks {
    pub fn new() -> Self {
        Disks { handles: Vec::new(), at: None }
    }

    pub fn invalidate(&mut self) {
        self.at = None;
    }

    fn close_all(&mut self) {
        for (_, h) in self.handles.drain(..) {
            // SAFETY: handles we opened.
            unsafe { CloseHandle(h) };
        }
    }

    /// Probe `\\.\PhysicalDriveN`, stopping 4 indexes past the last miss streak.
    fn probe(&mut self) {
        self.close_all();
        self.at = Some(Instant::now());
        let mut misses = 0;
        let mut n = 0u32;
        while misses <= 4 && n < 64 {
            let mut path = [0u16; 32];
            let prefix = r"\\.\PhysicalDrive";
            let mut len = 0;
            for c in prefix.encode_utf16() {
                path[len] = c;
                len += 1;
            }
            let mut digits = [0u8; 10];
            let mut d = 0;
            let mut v = n;
            loop {
                digits[d] = b'0' + (v % 10) as u8;
                d += 1;
                v /= 10;
                if v == 0 {
                    break;
                }
            }
            while d > 0 {
                d -= 1;
                path[len] = digits[d] as u16;
                len += 1;
            }
            // SAFETY: NUL-terminated path; access 0 queries metadata only (works unprivileged).
            let h = unsafe {
                CreateFileW(path.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, 0, null_mut())
            };
            if h == INVALID_HANDLE_VALUE || h.is_null() {
                misses += 1;
            } else {
                misses = 0;
                self.handles.push((n, h));
            }
            n += 1;
        }
    }

    pub fn read(&mut self, out: &mut Vec<DiskIo>) {
        if self.at.map_or(true, |t| t.elapsed() >= DISK_TTL) {
            self.probe();
        }
        let mut i = 0;
        for &(n, h) in &self.handles {
            // SAFETY: DISK_PERFORMANCE is plain data; zero is a valid value.
            let mut perf: DISK_PERFORMANCE = unsafe { std::mem::zeroed() };
            let mut ret = 0u32;
            // SAFETY: valid handle, output buffer sized as passed, synchronous call.
            let ok = unsafe {
                DeviceIoControl(
                    h,
                    IOCTL_DISK_PERFORMANCE,
                    null(),
                    0,
                    (&mut perf as *mut DISK_PERFORMANCE).cast(),
                    std::mem::size_of::<DISK_PERFORMANCE>() as u32,
                    &mut ret,
                    null_mut(),
                )
            };
            if ok == 0 {
                continue;
            }
            let e = slot(out, i);
            i += 1;
            e.name.clear();
            e.name.push_str("PhysicalDrive");
            push_u32(&mut e.name, n);
            e.kind = DiskKind::Disk;
            e.read_count = perf.ReadCount as u64;
            e.write_count = perf.WriteCount as u64;
            e.read_bytes = perf.BytesRead.max(0) as u64;
            e.write_bytes = perf.BytesWritten.max(0) as u64;
            // 100 ns units -> ms.
            e.read_time_ms = Some(perf.ReadTime.max(0) as u64 / 10_000);
            e.write_time_ms = Some(perf.WriteTime.max(0) as u64 / 10_000);
        }
        out.truncate(i);
    }
}

impl Drop for Disks {
    fn drop(&mut self) {
        self.close_all();
    }
}

// ---------------------------------------------------------------------------
// File systems (PLT-122)

#[derive(Default)]
struct Vol {
    letter: u16,
    ok: bool,
    fs_type: String,
    read_only: bool,
}

pub struct Fs {
    vols: Vec<Vol>,
    at: Option<Instant>,
}

impl Fs {
    pub fn new() -> Self {
        Fs { vols: Vec::new(), at: None }
    }

    pub fn invalidate(&mut self) {
        self.at = None;
    }

    pub fn list(&mut self, out: &mut Vec<Mount>) {
        if self.at.map_or(true, |t| t.elapsed() >= VOLUME_TTL) {
            self.vols.clear();
            self.at = Some(Instant::now());
        }
        let mut buf = [0u16; 128];
        // SAFETY: buffer length passed in characters.
        let n = unsafe { GetLogicalDriveStringsW(buf.len() as u32, buf.as_mut_ptr()) } as usize;
        if n == 0 || n > buf.len() {
            out.clear();
            return;
        }
        let mut i = 0;
        for root in buf[..n].split(|&c| c == 0).filter(|r| !r.is_empty()) {
            let mut z = [0u16; 8];
            let len = root.len().min(7);
            z[..len].copy_from_slice(&root[..len]);
            // SAFETY: NUL-terminated root path.
            let kind = unsafe { GetDriveTypeW(z.as_ptr()) };
            if kind == DRIVE_NO_ROOT_DIR {
                continue;
            }
            let letter = root[0];
            let vi = match self.vols.iter().position(|v| v.letter == letter) {
                Some(p) => p,
                None => {
                    let mut v = Vol { letter, ..Default::default() };
                    let mut name = [0u16; 64];
                    let mut flags = 0u32;
                    if kind == DRIVE_REMOTE {
                        // PLT-010: GetVolumeInformationW can block for seconds on a remote
                        // drive and this runs on the UI thread, so it is skipped here; only
                        // fs_usage (on the fs worker) touches the share.
                        v.ok = true;
                        v.fs_type.push_str("remote");
                    } else {
                        // SAFETY: NUL-terminated root; output buffer sized as passed.
                        v.ok = unsafe {
                            GetVolumeInformationW(z.as_ptr(), null_mut(), 0, null_mut(), null_mut(), &mut flags, name.as_mut_ptr(), name.len() as u32)
                        } != 0;
                    }
                    if v.ok && kind != DRIVE_REMOTE {
                        let end = name.iter().position(|&c| c == 0).unwrap_or(name.len());
                        push_utf16(&mut v.fs_type, &name[..end]);
                        v.read_only = flags & FILE_READ_ONLY_VOLUME != 0;
                    }
                    self.vols.push(v);
                    self.vols.len() - 1
                }
            };
            let v = &self.vols[vi];
            // Drives without media (empty card reader, optical drive) have no volume.
            if !v.ok {
                continue;
            }
            let e = slot(out, i);
            i += 1;
            e.device.clear();
            push_utf16(&mut e.device, root);
            e.mount_point.clear();
            e.mount_point.push_str(&e.device);
            e.fs_type.clear();
            e.fs_type.push_str(&v.fs_type);
            e.is_physical = kind == DRIVE_FIXED || kind == DRIVE_REMOVABLE;
            e.is_network = kind == DRIVE_REMOTE;
            e.read_only = v.read_only;
        }
        out.truncate(i);
    }
}

/// PLT-122 usage. May block on remote drives; callers run it on the fs worker.
pub fn fs_usage(mount_point: &str) -> Option<FsUsage> {
    let mut path = [0u16; 264];
    let mut n = 0;
    for c in mount_point.encode_utf16() {
        if n >= path.len() - 1 {
            return None;
        }
        path[n] = c;
        n += 1;
    }
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: NUL-terminated path, valid out pointers.
    if unsafe { GetDiskFreeSpaceExW(path.as_ptr(), &mut avail, &mut total, &mut free) } == 0 {
        return None;
    }
    Some(FsUsage { total, free, avail })
}

// ---------------------------------------------------------------------------
// Battery (PLT-134)

pub fn batteries(out: &mut Vec<Battery>) {
    // SAFETY: plain data out-struct.
    let mut s: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: valid out pointer.
    let ok = unsafe { GetSystemPowerStatus(&mut s) } != 0;
    // 128 = no system battery; 255 = unknown status / percent.
    if !ok || s.BatteryFlag & 128 != 0 || s.BatteryFlag == 255 || s.BatteryLifePercent == 255 {
        out.clear();
        return;
    }
    let percent = s.BatteryLifePercent as f64;
    let status = if s.BatteryFlag & 8 != 0 {
        BatteryStatus::Charging
    } else {
        match s.ACLineStatus {
            0 => BatteryStatus::Discharging,
            1 if percent >= 100.0 => BatteryStatus::Full,
            1 => BatteryStatus::NotCharging,
            _ => BatteryStatus::Unknown,
        }
    };
    out.truncate(1);
    match out.first_mut() {
        Some(b) => {
            b.percent = percent;
            b.status = status;
        }
        None => out.push(Battery { name: "Battery".to_string(), percent, status }),
    }
}

// ---------------------------------------------------------------------------
// Sockets for the pinned process (section 16)

fn ext_table(buf: &mut Vec<u64>, f: impl Fn(*mut core::ffi::c_void, *mut u32) -> u32) -> bool {
    if buf.is_empty() {
        buf.resize(4096, 0);
    }
    for _ in 0..4 {
        let mut size = (buf.len() * 8) as u32;
        let r = f(buf.as_mut_ptr().cast(), &mut size);
        if r == ERROR_INSUFFICIENT_BUFFER {
            buf.resize(size as usize / 8 + 64, 0);
            continue;
        }
        return r == 0;
    }
    false
}

macro_rules! count_rows {
    ($buf:expr, $tbl:ty, $row:ty, $pid:expr) => {{
        // SAFETY: the buffer holds a table of this type filled by the OS.
        unsafe {
            let t = $buf.as_ptr() as *const $tbl;
            let n = (*t).dwNumEntries as usize;
            let rows = std::slice::from_raw_parts(addr_of!((*t).table) as *const $row, n);
            rows.iter().filter(|r| r.dwOwningPid == $pid).count() as u32
        }
    }};
}

/// TCP and UDP socket counts (IPv4 + IPv6) owned by `pid`.
pub fn socket_counts(buf: &mut Vec<u64>, pid: u32) -> (Option<u32>, Option<u32>) {
    let mut tcp = None;
    let mut udp = None;
    // SAFETY (all four closures): buffer valid for `*size` bytes.
    if ext_table(buf, |p, s| unsafe { GetExtendedTcpTable(p, s, 0, AF_INET as u32, TCP_TABLE_OWNER_PID_ALL, 0) }) {
        tcp = Some(count_rows!(buf, MIB_TCPTABLE_OWNER_PID, MIB_TCPROW_OWNER_PID, pid));
    }
    if ext_table(buf, |p, s| unsafe { GetExtendedTcpTable(p, s, 0, AF_INET6 as u32, TCP_TABLE_OWNER_PID_ALL, 0) }) {
        let n = count_rows!(buf, MIB_TCP6TABLE_OWNER_PID, MIB_TCP6ROW_OWNER_PID, pid);
        tcp = Some(tcp.unwrap_or(0) + n);
    }
    if ext_table(buf, |p, s| unsafe { GetExtendedUdpTable(p, s, 0, AF_INET as u32, UDP_TABLE_OWNER_PID, 0) }) {
        udp = Some(count_rows!(buf, MIB_UDPTABLE_OWNER_PID, MIB_UDPROW_OWNER_PID, pid));
    }
    if ext_table(buf, |p, s| unsafe { GetExtendedUdpTable(p, s, 0, AF_INET6 as u32, UDP_TABLE_OWNER_PID, 0) }) {
        let n = count_rows!(buf, MIB_UDP6TABLE_OWNER_PID, MIB_UDP6ROW_OWNER_PID, pid);
        udp = Some(udp.unwrap_or(0) + n);
    }
    (tcp, udp)
}
