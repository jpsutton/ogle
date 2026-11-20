//! Platform abstraction (spec 06-platform-sources.md).
//!
//! CONTRACT FOR ALL BACKENDS
//! - Every backend module (`linux`, `macos`, `freebsd`, `openbsd`, `netbsd`, `windows`)
//!   exports `pub struct Sys` that implements [`Platform`] and a `pub fn new() -> Sys`.
//! - `Option` = field may be absent (PLT-001). Never return 0 for "unknown".
//! - Counters are raw cumulative u64 (PLT-002); rates are computed above this layer.
//! - Collection methods fill caller-owned `Vec`s (cleared by the callee) so steady-state
//!   cycles allocate nothing (PERF-0xx). Keep fds/handles/buffers inside `Sys` and reuse.
//! - Per-item permission errors make only that item's fields absent (PLT-005).
//! - No subprocesses, no sockets (PLT-009).
//!
//! Do not change these types or signatures without updating every backend.

use std::time::Duration;

/// Static-ish OS description (PLT-020..PLT-023). Cost class C0.
#[derive(Clone, Debug, Default)]
pub struct SystemInfo {
    pub hostname: String,
    /// "Linux", "macOS", "FreeBSD", "OpenBSD", "NetBSD", "Windows".
    pub os_name: String,
    /// Kernel release or OS product version.
    pub os_version: String,
    /// Linux distribution "NAME VERSION_ID" from /etc/os-release; None elsewhere.
    pub distro: Option<String>,
    /// Machine architecture, e.g. "x86_64", "aarch64".
    pub arch: String,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CpuTopology {
    pub logical: u32,
    pub physical: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct CpuFreq {
    /// Model name, e.g. "AMD Ryzen 9 7950X 16-Core Processor".
    pub model: Option<String>,
    /// Mean current frequency across cores, Hz.
    pub current_hz: Option<u64>,
    /// Maximum frequency, Hz.
    pub max_hz: Option<u64>,
}

/// Cumulative CPU time in seconds for one CPU or the total (PLT-050..PLT-055).
/// Fields the platform lacks are None. `guest`/`guest_nice` are already included
/// in `user`/`nice` on Linux; backends must not double count them in `total()`.
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuTimes {
    pub user: f64,
    pub system: f64,
    pub idle: f64,
    pub nice: Option<f64>,
    pub iowait: Option<f64>,
    pub irq: Option<f64>,
    pub softirq: Option<f64>,
    pub steal: Option<f64>,
    pub guest: Option<f64>,
    /// Windows DPC time.
    pub dpc: Option<f64>,
}

impl CpuTimes {
    /// Sum of all non-overlapping fields (guest excluded, it is inside user).
    pub fn total(&self) -> f64 {
        self.user
            + self.system
            + self.idle
            + self.nice.unwrap_or(0.0)
            + self.iowait.unwrap_or(0.0)
            + self.irq.unwrap_or(0.0)
            + self.softirq.unwrap_or(0.0)
            + self.steal.unwrap_or(0.0)
            + self.dpc.unwrap_or(0.0)
    }
}

/// Cumulative system counters (PLT-060..PLT-061).
#[derive(Clone, Copy, Debug, Default)]
pub struct CpuCounters {
    pub ctx_switches: Option<u64>,
    pub interrupts: Option<u64>,
    pub soft_interrupts: Option<u64>,
    pub syscalls: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LoadAvg {
    pub min1: f64,
    pub min5: f64,
    pub min15: f64,
    /// True when emulated (Windows, PLT-072).
    pub emulated: bool,
}

/// Bytes (PLT-080..PLT-082).
#[derive(Clone, Copy, Debug, Default)]
pub struct Memory {
    pub total: u64,
    pub available: u64,
    pub free: Option<u64>,
    pub active: Option<u64>,
    pub inactive: Option<u64>,
    pub buffers: Option<u64>,
    pub cached: Option<u64>,
    pub shared: Option<u64>,
    pub wired: Option<u64>,
}

/// Bytes. `None` from `swap()` means no swap information at all.
#[derive(Clone, Copy, Debug, Default)]
pub struct Swap {
    pub total: u64,
    pub used: u64,
    pub free: u64,
}

#[derive(Clone, Debug, Default)]
pub struct NetIf {
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub is_up: bool,
    /// Link speed in bits/s (10^6 scaling, PLT-100). None when unknown.
    pub speed_bps: Option<u64>,
    /// True when the interface has at least one non-link-layer address.
    /// Only meaningful when `want_addrs` was true; otherwise true.
    pub has_ip: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiskKind {
    #[default]
    Disk,
    Partition,
    Virtual,
}

#[derive(Clone, Debug, Default)]
pub struct DiskIo {
    pub name: String,
    pub kind: DiskKind,
    pub read_count: u64,
    pub write_count: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
    /// Milliseconds spent on reads/writes (cumulative). None if unavailable.
    pub read_time_ms: Option<u64>,
    pub write_time_ms: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct Mount {
    pub device: String,
    pub mount_point: String,
    pub fs_type: String,
    pub is_physical: bool,
    pub is_network: bool,
    pub read_only: bool,
}

/// Bytes. `avail` = space available to unprivileged users.
#[derive(Clone, Copy, Debug, Default)]
pub struct FsUsage {
    pub total: u64,
    pub free: u64,
    pub avail: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SensorKind {
    Temperature,
    Fan,
}

#[derive(Clone, Debug)]
pub struct Sensor {
    pub kind: SensorKind,
    /// Chip or group name, e.g. "coretemp", "k10temp", "nvme".
    pub chip: String,
    /// Sensor label; may be empty (module synthesizes "chip index").
    pub label: String,
    /// Celsius for temperatures, RPM for fans.
    pub value: f64,
    pub high: Option<f64>,
    pub critical: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatteryStatus {
    Charging,
    Discharging,
    Full,
    NotCharging,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct Battery {
    pub name: String,
    pub percent: f64,
    pub status: BatteryStatus,
}

/// Which per-process fields the caller needs this cycle (PLT-011).
/// Base fields (pid, start_time, name, status, threads, cpu times, rss, vms, nice,
/// uid, last_cpu, kernel-thread flag) are always filled when cheap.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcFields {
    /// Read I/O counters for every process (sort by io).
    pub io_all: bool,
    /// Read I/O counters only for these pids (visible rows). Ignored when io_all.
    pub io_visible: bool,
}

/// One process (PLT-140..PLT-155). Strings that are cached (cmdline, username) are
/// NOT part of this record; use `process_cmdline` / `username` on demand.
#[derive(Clone, Debug, Default)]
pub struct Proc {
    pub pid: u32,
    pub ppid: Option<u32>,
    /// Opaque start-time token; equal values mean same process. Unix seconds when
    /// convertible, else platform ticks. With pid forms the identity (PLT-140).
    pub start_time: u64,
    /// Short name (comm / p_comm / image name). Reuse the String allocation.
    pub name: String,
    pub uid: Option<u32>,
    /// One of R S D Z T I W P K U, or L (FreeBSD SLOCK) (PLT-150). Unknown letters render as-is.
    pub status: u8,
    pub nice: Option<i32>,
    /// Windows priority class label: "RT" "Hi" "AN" "No" "BN" "Lo".
    pub priority_class: Option<&'static str>,
    pub num_threads: u32,
    /// Cumulative CPU seconds.
    pub cpu_user: f64,
    pub cpu_system: f64,
    pub rss: u64,
    pub vms: Option<u64>,
    pub io_read: Option<u64>,
    pub io_write: Option<u64>,
    pub last_cpu: Option<u32>,
    pub is_kernel_thread: bool,
}

/// Pinned-process data (section 16 of 06). Cost class C3, pinned pid only.
#[derive(Clone, Debug, Default)]
pub struct ProcExtended {
    pub affinity_cores: Option<u32>,
    /// Linux ionice: (class name, level).
    pub ionice: Option<(&'static str, u8)>,
    pub ctx_voluntary: Option<u64>,
    pub ctx_involuntary: Option<u64>,
    pub num_fds: Option<u32>,
    pub num_handles: Option<u32>,
    pub swap_bytes: Option<u64>,
    pub shared_bytes: Option<u64>,
    pub tcp: Option<u32>,
    pub udp: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Signal {
    Term,
    Kill,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionError {
    /// Process gone or identity (pid, start_time) no longer matches.
    NoSuchProcess,
    PermissionDenied,
    Other(i32),
}

/// The platform interface (06 section 2). Every backend implements this for `Sys`.
pub trait Platform {
    fn system_info(&mut self) -> SystemInfo;
    /// Wall-clock Unix seconds.
    fn boot_time(&mut self) -> Option<u64>;
    fn cpu_topology(&mut self) -> CpuTopology;
    fn cpu_freq(&mut self) -> CpuFreq;
    /// Fills `total`; when `per_cpu` is Some, clears and fills it, one entry per logical CPU.
    fn cpu_times(&mut self, total: &mut CpuTimes, per_cpu: Option<&mut Vec<CpuTimes>>) -> bool;
    fn cpu_counters(&mut self) -> CpuCounters;
    fn load_avg(&mut self) -> Option<LoadAvg>;
    fn memory(&mut self) -> Option<Memory>;
    fn swap(&mut self) -> Option<Swap>;
    fn net_interfaces(&mut self, want_addrs: bool, out: &mut Vec<NetIf>);
    fn disk_io(&mut self, out: &mut Vec<DiskIo>);
    fn filesystems(&mut self, out: &mut Vec<Mount>);
    /// May block on network file systems (PLT-010). The fs module calls it inline only for
    /// local physical mounts and routes every other mount through its worker thread.
    fn fs_usage(mount_point: &str) -> Option<FsUsage>
    where
        Self: Sized;
    fn sensors(&mut self, want_temp: bool, want_fan: bool, out: &mut Vec<Sensor>);
    fn batteries(&mut self, out: &mut Vec<Battery>);
    /// Enumerate processes. `out` is reused across cycles: the backend may reuse
    /// existing `Proc` entries (and their `name` String) to avoid allocation.
    /// Exclusions per PLT-141.
    fn processes(&mut self, fields: ProcFields, visible: &[u32], out: &mut Vec<Proc>);
    /// Full argv; None if denied/absent. Called once per process identity (cached above).
    fn process_cmdline(&mut self, pid: u32, start_time: u64) -> Option<Vec<String>>;
    /// Resolve uid (or Windows SID index, see backend) to a name. Cached above (PLT-127).
    fn username(&mut self, uid: u32) -> Option<String>;
    fn process_extended(&mut self, pid: u32, start_time: u64, want_sockets: bool) -> Option<ProcExtended>;
    fn process_signal(&mut self, pid: u32, start_time: u64, sig: Signal) -> Result<(), ActionError>;
    fn process_set_priority(&mut self, pid: u32, start_time: u64, delta: i32) -> Result<(), ActionError>;
    fn pid_max(&mut self) -> Option<u32>;
    /// Clock ticks or other info the backend wants refreshed on F5 (UI-045).
    fn reset_caches(&mut self) {}
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{new, Sys};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::{new, Sys};

#[cfg(any(target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
mod bsd_common;

#[cfg(target_os = "freebsd")]
mod freebsd;
#[cfg(target_os = "freebsd")]
pub use freebsd::{new, Sys};

#[cfg(target_os = "openbsd")]
mod openbsd;
#[cfg(target_os = "openbsd")]
pub use openbsd::{new, Sys};

#[cfg(target_os = "netbsd")]
mod netbsd;
#[cfg(target_os = "netbsd")]
pub use netbsd::{new, Sys};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{new, Sys};

/// Shared helpers usable by any backend.
pub mod util;

/// Default timeout for blocking fs queries (PLT-010).
pub const FS_TIMEOUT: Duration = Duration::from_secs(2);
