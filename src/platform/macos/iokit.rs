//! IOKit / CoreFoundation sources: disk I/O (PLT-111), battery (PLT-132) and the
//! Apple Silicon max CPU frequency (PLT-042, best effort).

#![allow(non_upper_case_globals, non_snake_case)]

use std::ffi::CStr;
use std::ptr;
use std::time::{Duration, Instant};

use libc::{c_char, c_int, c_void};

use super::set_str;
use crate::platform::{Battery, BatteryStatus, DiskIo, DiskKind};

type CFTypeRef = *const c_void;
type CFIndex = isize;
type IoObject = u32;

const kCFStringEncodingUTF8: u32 = 0x0800_0100;
const kCFNumberSInt64Type: CFIndex = 4;
/// MACH_PORT_NULL selects the default main port.
const kIOMainPortDefault: u32 = 0;
const RESCAN: Duration = Duration::from_secs(30);

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: CFTypeRef;
    fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, enc: u32) -> CFTypeRef;
    fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: CFIndex, enc: u32) -> u8;
    fn CFDictionaryGetValue(d: CFTypeRef, key: CFTypeRef) -> CFTypeRef;
    fn CFDictionarySetValue(d: *mut c_void, key: CFTypeRef, value: CFTypeRef);
    fn CFNumberGetValue(n: CFTypeRef, ty: CFIndex, out: *mut c_void) -> u8;
    fn CFBooleanGetValue(b: CFTypeRef) -> u8;
    fn CFGetTypeID(t: CFTypeRef) -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFBooleanGetTypeID() -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFDataGetTypeID() -> usize;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFDataGetLength(d: CFTypeRef) -> CFIndex;
    fn CFDataGetBytePtr(d: CFTypeRef) -> *const u8;
    fn CFArrayGetCount(a: CFTypeRef) -> CFIndex;
    fn CFArrayGetValueAtIndex(a: CFTypeRef, i: CFIndex) -> CFTypeRef;
    fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> u8;
    fn CFRelease(t: CFTypeRef);
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingServices(main: u32, matching: *mut c_void, it: *mut IoObject) -> c_int;
    fn IOIteratorNext(it: IoObject) -> IoObject;
    fn IOObjectRelease(o: IoObject) -> c_int;
    /// Returns Mach `boolean_t`, which is a C `int` (not CF's one-byte `Boolean`).
    fn IOObjectConformsTo(o: IoObject, class: *const c_char) -> c_int;
    fn IORegistryEntryGetParentEntry(e: IoObject, plane: *const c_char, parent: *mut IoObject) -> c_int;
    fn IORegistryEntryCreateCFProperty(e: IoObject, key: CFTypeRef, alloc: CFTypeRef, opts: u32) -> CFTypeRef;
    fn IORegistryEntryGetName(e: IoObject, name: *mut c_char) -> c_int;
    fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
    fn IOPSCopyPowerSourcesList(blob: CFTypeRef) -> CFTypeRef;
    fn IOPSGetPowerSourceDescription(blob: CFTypeRef, ps: CFTypeRef) -> CFTypeRef;
}

/// Creates a CFString; the caller owns it.
fn cfs(s: &CStr) -> CFTypeRef {
    // SAFETY: valid NUL-terminated string; default allocator.
    unsafe { CFStringCreateWithCString(ptr::null(), s.as_ptr(), kCFStringEncodingUTF8) }
}

fn cf_i64(v: CFTypeRef) -> Option<i64> {
    // SAFETY: type checked before CFNumberGetValue; v is a valid CF object or null.
    unsafe {
        if v.is_null() || CFGetTypeID(v) != CFNumberGetTypeID() {
            return None;
        }
        let mut out: i64 = 0;
        (CFNumberGetValue(v, kCFNumberSInt64Type, (&mut out as *mut i64).cast()) != 0).then_some(out)
    }
}

fn cf_bool(v: CFTypeRef) -> Option<bool> {
    // SAFETY: type checked; v is a valid CF object or null.
    unsafe { (!v.is_null() && CFGetTypeID(v) == CFBooleanGetTypeID()).then(|| CFBooleanGetValue(v) != 0) }
}

/// Copies a CFString into `dst`. Returns false if `v` is not a string.
fn cf_str_into(v: CFTypeRef, dst: &mut String) -> bool {
    let mut buf = [0 as c_char; 256];
    // SAFETY: type checked; buffer size passed.
    let ok = unsafe {
        !v.is_null()
            && CFGetTypeID(v) == CFStringGetTypeID()
            && CFStringGetCString(v, buf.as_mut_ptr(), buf.len() as CFIndex, kCFStringEncodingUTF8) != 0
    };
    if ok {
        set_str(dst, super::cchars(&buf));
    }
    ok
}

/// Immutable CFString keys, created once.
struct Keys {
    whole: CFTypeRef,
    statistics: CFTypeRef,
    bsd_name: CFTypeRef,
    bytes_r: CFTypeRef,
    bytes_w: CFTypeRef,
    ops_r: CFTypeRef,
    ops_w: CFTypeRef,
    time_r: CFTypeRef,
    time_w: CFTypeRef,
    ps_type: CFTypeRef,
    internal_battery: CFTypeRef,
    cur_cap: CFTypeRef,
    max_cap: CFTypeRef,
    is_charging: CFTypeRef,
    is_charged: CFTypeRef,
    ps_state: CFTypeRef,
    battery_power: CFTypeRef,
    ac_power: CFTypeRef,
    name: CFTypeRef,
    is_present: CFTypeRef,
}

impl Keys {
    fn new() -> Keys {
        Keys {
            whole: cfs(c"Whole"),
            statistics: cfs(c"Statistics"),
            bsd_name: cfs(c"BSD Name"),
            bytes_r: cfs(c"Bytes (Read)"),
            bytes_w: cfs(c"Bytes (Write)"),
            ops_r: cfs(c"Operations (Read)"),
            ops_w: cfs(c"Operations (Write)"),
            time_r: cfs(c"Total Time (Read)"),
            time_w: cfs(c"Total Time (Write)"),
            ps_type: cfs(c"Type"),
            internal_battery: cfs(c"InternalBattery"),
            cur_cap: cfs(c"Current Capacity"),
            max_cap: cfs(c"Max Capacity"),
            is_charging: cfs(c"Is Charging"),
            is_charged: cfs(c"Is Charged"),
            ps_state: cfs(c"Power Source State"),
            battery_power: cfs(c"Battery Power"),
            ac_power: cfs(c"AC Power"),
            name: cfs(c"Name"),
            is_present: cfs(c"Is Present"),
        }
    }
}

pub struct IoState {
    keys: Option<Keys>,
    /// Cached IOBlockStorageDriver entries with the BSD name of their whole media.
    drivers: Vec<(IoObject, String)>,
    scanned_at: Option<Instant>,
}

// SAFETY: the only raw pointers are immutable CFStrings, which CoreFoundation documents as
// thread-safe; io_object_t values are Mach port names.
unsafe impl Send for IoState {}

impl Drop for IoState {
    fn drop(&mut self) {
        self.release_drivers();
    }
}

impl IoState {
    pub fn new() -> IoState {
        IoState { keys: None, drivers: Vec::new(), scanned_at: None }
    }

    pub fn force_rescan(&mut self) {
        self.scanned_at = None;
    }

    fn keys(&mut self) -> &Keys {
        self.keys.get_or_insert_with(Keys::new)
    }

    fn release_drivers(&mut self) {
        for (d, _) in self.drivers.drain(..) {
            // SAFETY: each driver object was retained by IORegistryEntryGetParentEntry.
            unsafe { IOObjectRelease(d) };
        }
    }

    /// PLT-111 enumeration (C3): IOMedia with Whole = true, parent IOBlockStorageDriver.
    fn rescan(&mut self) {
        self.release_drivers();
        self.scanned_at = Some(Instant::now());
        let (whole, bsd_name) = {
            let k = self.keys();
            (k.whole, k.bsd_name)
        };
        // SAFETY: standard IOKit enumeration; every returned object is released once.
        unsafe {
            let m = IOServiceMatching(c"IOMedia".as_ptr());
            if m.is_null() {
                return;
            }
            CFDictionarySetValue(m, whole, kCFBooleanTrue);
            let mut it: IoObject = 0;
            // Consumes `m`.
            if IOServiceGetMatchingServices(kIOMainPortDefault, m, &mut it) != 0 {
                return;
            }
            loop {
                let media = IOIteratorNext(it);
                if media == 0 {
                    break;
                }
                let mut parent: IoObject = 0;
                if IORegistryEntryGetParentEntry(media, c"IOService".as_ptr(), &mut parent) == 0 {
                    let mut name = String::new();
                    let v = IORegistryEntryCreateCFProperty(media, bsd_name, ptr::null(), 0);
                    let named = cf_str_into(v, &mut name);
                    if !v.is_null() {
                        CFRelease(v);
                    }
                    if named && IOObjectConformsTo(parent, c"IOBlockStorageDriver".as_ptr()) != 0 {
                        self.drivers.push((parent, name));
                    } else {
                        IOObjectRelease(parent);
                    }
                }
                IOObjectRelease(media);
            }
            IOObjectRelease(it);
        }
    }

    /// PLT-111 per-cycle read (C2 over cached drivers).
    pub fn disk_io(&mut self, out: &mut Vec<DiskIo>) {
        if self.scanned_at.map_or(true, |t| t.elapsed() >= RESCAN) {
            self.rescan();
        }
        let mut failed = false;
        let mut n = 0;
        let k = self.keys.get_or_insert_with(Keys::new);
        for (drv, name) in &self.drivers {
            // SAFETY: drv is a retained registry entry; the property is released below.
            let stats = unsafe { IORegistryEntryCreateCFProperty(*drv, k.statistics, ptr::null(), 0) };
            if stats.is_null() {
                failed = true;
                continue;
            }
            // SAFETY: stats is a valid CF object; type checked before dictionary access.
            let is_dict = unsafe { CFGetTypeID(stats) == CFDictionaryGetTypeID() };
            if is_dict {
                // SAFETY: stats is a CFDictionary; values are borrowed (get rule).
                let get = |key: CFTypeRef| cf_i64(unsafe { CFDictionaryGetValue(stats, key) }).map(|v| v.max(0) as u64);
                if n == out.len() {
                    out.push(DiskIo::default());
                }
                let d = &mut out[n];
                d.name.clear();
                d.name.push_str(name);
                d.kind = DiskKind::Disk;
                d.read_bytes = get(k.bytes_r).unwrap_or(0);
                d.write_bytes = get(k.bytes_w).unwrap_or(0);
                d.read_count = get(k.ops_r).unwrap_or(0);
                d.write_count = get(k.ops_w).unwrap_or(0);
                // Total time is in ns; convert to ms (PLT-003).
                d.read_time_ms = get(k.time_r).map(|v| v / 1_000_000);
                d.write_time_ms = get(k.time_w).map(|v| v / 1_000_000);
                n += 1;
            }
            // SAFETY: created by IORegistryEntryCreateCFProperty (create rule).
            unsafe { CFRelease(stats) };
        }
        out.truncate(n);
        if failed {
            self.scanned_at = None; // re-enumerate next cycle
        }
    }

    /// PLT-132 battery via IOPowerSources. All CF objects are released every call.
    pub fn batteries(&mut self, out: &mut Vec<Battery>) {
        let k = self.keys.get_or_insert_with(Keys::new);
        let mut n = 0;
        // SAFETY: CF copy/get rules followed; blob and list released at the end.
        unsafe {
            let blob = IOPSCopyPowerSourcesInfo();
            if blob.is_null() {
                out.clear();
                return;
            }
            let list = IOPSCopyPowerSourcesList(blob);
            if !list.is_null() {
                for i in 0..CFArrayGetCount(list) {
                    let d = IOPSGetPowerSourceDescription(blob, CFArrayGetValueAtIndex(list, i));
                    if d.is_null() || CFGetTypeID(d) != CFDictionaryGetTypeID() {
                        continue;
                    }
                    let ty = CFDictionaryGetValue(d, k.ps_type);
                    if ty.is_null() || CFEqual(ty, k.internal_battery) == 0 {
                        continue;
                    }
                    if cf_bool(CFDictionaryGetValue(d, k.is_present)) == Some(false) {
                        continue;
                    }
                    let cur = cf_i64(CFDictionaryGetValue(d, k.cur_cap));
                    let max = cf_i64(CFDictionaryGetValue(d, k.max_cap));
                    let (Some(cur), Some(max)) = (cur, max) else { continue };
                    if max <= 0 {
                        continue;
                    }
                    let state = CFDictionaryGetValue(d, k.ps_state);
                    let on_battery = !state.is_null() && CFEqual(state, k.battery_power) != 0;
                    let on_ac = !state.is_null() && CFEqual(state, k.ac_power) != 0;
                    let status = if cf_bool(CFDictionaryGetValue(d, k.is_charging)) == Some(true) {
                        BatteryStatus::Charging
                    } else if cf_bool(CFDictionaryGetValue(d, k.is_charged)) == Some(true) {
                        BatteryStatus::Full
                    } else if on_battery {
                        BatteryStatus::Discharging
                    } else if on_ac {
                        BatteryStatus::NotCharging
                    } else {
                        BatteryStatus::Unknown
                    };
                    let percent = (cur as f64 / max as f64 * 100.0).clamp(0.0, 100.0);
                    if n == out.len() {
                        out.push(Battery { name: String::new(), percent, status });
                    }
                    let b = &mut out[n];
                    if !cf_str_into(CFDictionaryGetValue(d, k.name), &mut b.name) {
                        set_str(&mut b.name, b"Battery");
                    }
                    b.percent = percent;
                    b.status = status;
                    n += 1;
                }
                CFRelease(list);
            }
            CFRelease(blob);
        }
        out.truncate(n);
    }
}

/// PLT-042 Apple Silicon `max_hz`, best effort: the `pmgr` AppleARMIODevice
/// `voltage-states<N>-sram` tables (8-byte entries, 4-byte LE frequency in Hz first).
/// Keys 1 and 5 are the E- and P-cluster tables up to M4; absent if not found.
pub fn pmgr_max_hz() -> Option<u64> {
    let keys = [cfs(c"voltage-states1-sram"), cfs(c"voltage-states5-sram")];
    let mut best: u64 = 0;
    // SAFETY: standard IOKit enumeration; every object and CF value is released once.
    unsafe {
        let m = IOServiceMatching(c"AppleARMIODevice".as_ptr());
        let mut it: IoObject = 0;
        if !m.is_null() && IOServiceGetMatchingServices(kIOMainPortDefault, m, &mut it) == 0 {
            loop {
                let e = IOIteratorNext(it);
                if e == 0 {
                    break;
                }
                let mut name = [0 as c_char; 128];
                if IORegistryEntryGetName(e, name.as_mut_ptr()) == 0 && super::cchars(&name) == b"pmgr" {
                    for &key in &keys {
                        let v = IORegistryEntryCreateCFProperty(e, key, ptr::null(), 0);
                        if v.is_null() {
                            continue;
                        }
                        if CFGetTypeID(v) == CFDataGetTypeID() {
                            let len = CFDataGetLength(v).max(0) as usize;
                            let p = CFDataGetBytePtr(v);
                            if !p.is_null() {
                                let b = std::slice::from_raw_parts(p, len);
                                for c in b.chunks_exact(8) {
                                    let f = u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as u64;
                                    // Ignore implausible entries (> 10 GHz).
                                    if f < 10_000_000_000 {
                                        best = best.max(f);
                                    }
                                }
                            }
                        }
                        CFRelease(v);
                    }
                }
                IOObjectRelease(e);
            }
            IOObjectRelease(it);
        }
        for k in keys {
            if !k.is_null() {
                CFRelease(k);
            }
        }
    }
    (best > 0).then_some(best)
}
