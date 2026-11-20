//! Modules (stats collectors + panels) and the interface shared by every module and the app.
//!
//! Each `modules/<name>.rs` owns its state type and a `render` function:
//!
//! ```ignore
//! impl State {
//!     pub fn new(cfg: &Config, ui: &UiState, alerts: &mut Alerts) -> Self;
//!     /// Collect + derive. Called by the app only when the module is due and needed.
//!     pub fn update(&mut self, sys: &mut Sys, ctx: &Ctx, ui: &UiState, alerts: &mut Alerts);
//!     /// Refresh interval for this module (config `[<module>] refresh`, else default).
//!     pub fn refresh(&self) -> Option<Duration>;   // None = use global refresh
//! }
//! pub fn render(w: &World, area: Area, out: &mut Panel) -> Align;
//! ```
//! Exceptions are documented on each `World` field below. Renderers MUST NOT allocate
//! in steady state (write into `out`, use `fmt::Small`), MUST NOT touch `Sys`.

pub mod alert;
pub mod cpu;
pub mod diskio;
pub mod fs;
pub mod load;
pub mod mem;
pub mod memswap;
pub mod network;
pub mod now;
pub mod percpu;
pub mod processcount;
pub mod processes;
pub mod processlist;
pub mod programlist;
pub mod quicklook;
pub mod sensors;
pub mod system;
pub mod uptime;

use std::time::Instant;

use crate::alerts::Alerts;
use crate::config::Config;

/// Module identifiers (names are the CLI/config names, UI-0xx).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ModuleId {
    System,
    Uptime,
    Now,
    Cpu,
    Percpu,
    Load,
    Mem,
    Memswap,
    Quicklook,
    Network,
    Diskio,
    Fs,
    Sensors,
    Processcount,
    Processlist,
    Programlist,
    Alert,
}

impl ModuleId {
    pub const COUNT: usize = 17;
    pub const ALL: [ModuleId; 17] = [
        ModuleId::System,
        ModuleId::Uptime,
        ModuleId::Now,
        ModuleId::Cpu,
        ModuleId::Percpu,
        ModuleId::Load,
        ModuleId::Mem,
        ModuleId::Memswap,
        ModuleId::Quicklook,
        ModuleId::Network,
        ModuleId::Diskio,
        ModuleId::Fs,
        ModuleId::Sensors,
        ModuleId::Processcount,
        ModuleId::Processlist,
        ModuleId::Programlist,
        ModuleId::Alert,
    ];
    pub fn name(self) -> &'static str {
        match self {
            ModuleId::System => "system",
            ModuleId::Uptime => "uptime",
            ModuleId::Now => "now",
            ModuleId::Cpu => "cpu",
            ModuleId::Percpu => "percpu",
            ModuleId::Load => "load",
            ModuleId::Mem => "mem",
            ModuleId::Memswap => "memswap",
            ModuleId::Quicklook => "quicklook",
            ModuleId::Network => "network",
            ModuleId::Diskio => "diskio",
            ModuleId::Fs => "fs",
            ModuleId::Sensors => "sensors",
            ModuleId::Processcount => "processcount",
            ModuleId::Processlist => "processlist",
            ModuleId::Programlist => "programlist",
            ModuleId::Alert => "alert",
        }
    }
    pub fn from_name(s: &str) -> Option<ModuleId> {
        ModuleId::ALL.iter().copied().find(|m| m.name() == s)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DiskioMode {
    #[default]
    Bytes,
    Iops,
    Latency,
}

/// Interactive display state (01 sections 2 and 5). Owned by the app; read by renderers.
#[derive(Clone, Debug)]
pub struct UiState {
    /// Module enabled flags indexed by `ModuleId as usize` (config `enabled`, CLI, hotkeys).
    pub enabled: [bool; ModuleId::COUNT],
    pub percpu: bool,
    pub left_sidebar: bool,
    pub quicklook: bool,
    pub full_quicklook: bool,
    pub top: bool,
    pub sparkline: bool,
    pub history: bool,
    pub normalize_cpu: bool,
    pub net_bytes: bool,
    pub net_sum: bool,
    pub net_cumulative: bool,
    pub diskio_mode: DiskioMode,
    pub show_ramfs: bool,
    pub fs_free: bool,
    pub fahrenheit: bool,
    pub programs: bool,
    pub long_names: bool,
    pub hide_kernel_threads: bool,
    pub ascii: bool,
    pub separator: bool,
    pub arrow_keys_sort: bool,
    /// Process collection on (key `z`).
    pub processes: bool,
    /// Alert panel shown (key `l`); evaluation continues regardless.
    pub alert_panel: bool,
    /// Global refresh, seconds.
    pub refresh: f64,
}

impl UiState {
    pub fn on(&self, m: ModuleId) -> bool {
        self.enabled[m as usize]
    }
}

/// Per-cycle collection context.
#[derive(Clone, Copy, Debug)]
pub struct Ctx {
    /// Monotonic time of this sample (PLT-004).
    pub now: Instant,
    /// Wall clock, Unix seconds.
    pub wall: f64,
    /// Process rows visible on screen last frame (for lazy per-row reads).
    pub visible_rows: u16,
}

/// Space given to a renderer.
#[derive(Clone, Copy, Debug)]
pub struct Area {
    pub width: u16,
    pub max_height: u16,
    /// Drop `OPTIONAL` spans (narrow top row / narrow process list).
    pub narrow: bool,
}

/// All module states. Field types are fixed by this contract.
pub struct World {
    pub cfg: Config,
    pub ui: UiState,
    pub alerts: Alerts,
    pub system: system::System,
    pub uptime: uptime::Uptime,
    pub now: now::Now,
    /// Also holds the per-CPU sample used by `percpu` and `quicklook` (SYS shared sample).
    /// `update` collects per-CPU times only when `ui.percpu || quicklook needs it`.
    pub cpu: cpu::Cpu,
    pub load: load::Load,
    pub mem: mem::Mem,
    pub memswap: memswap::MemSwap,
    /// `update(&mut self, sys, ctx, ui, alerts, cpu: &cpu::Cpu, mem: &mem::Mem,
    ///        swap: &memswap::MemSwap, load: &load::Load)`; called after those four.
    pub quicklook: quicklook::QuickLook,
    pub network: network::Network,
    pub diskio: diskio::DiskIo,
    pub fs: fs::Fs,
    pub sensors: sensors::Sensors,
    /// Process collection + sort/filter/cursor/pin state. Renderers:
    /// `processcount::render`, `processlist::render`, `programlist::render`.
    /// `update(&mut self, sys, ctx, ui, alerts, mem_total: u64, ncpu: u32)`.
    pub processes: processes::Processes,
}
