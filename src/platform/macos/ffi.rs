//! Raw Mach / libproc / sysctl declarations used by the macOS backend.
//!
//! Declared here instead of taken from `libc` so the backend does not depend on which
//! `libc` 0.2.x release is resolved (several Mach items are deprecated there).

#![allow(non_upper_case_globals, dead_code)]

use libc::{c_char, c_int, c_void};

pub type KernReturn = c_int;
pub type MachPort = u32;

// sysctl MIB components.
pub const CTL_KERN: c_int = 1;
pub const CTL_VM: c_int = 2;
pub const CTL_NET: c_int = 4;
pub const KERN_PROC: c_int = 14;
pub const KERN_PROC_ALL: c_int = 0;
pub const KERN_PROC_PID: c_int = 1;
pub const KERN_PROCARGS2: c_int = 49;
pub const VM_SWAPUSAGE: c_int = 5;
pub const PF_ROUTE: c_int = 17;
pub const NET_RT_IFLIST2: c_int = 6;
pub const RTM_IFINFO2: u8 = 0x12;

// Mach host flavors.
pub const HOST_CPU_LOAD_INFO: c_int = 3;
pub const HOST_CPU_LOAD_INFO_COUNT: u32 = 4;
pub const HOST_VM_INFO64: c_int = 4;
pub const PROCESSOR_CPU_LOAD_INFO: c_int = 2;

// libproc flavors.
pub const PROC_PIDLISTFDS: c_int = 1;
pub const PROC_PIDTASKINFO: c_int = 4;
pub const PROC_PIDFDSOCKETINFO: c_int = 3;
pub const PROX_FDTYPE_SOCKET: u32 = 2;
pub const RUSAGE_INFO_V2: c_int = 2;

// p_stat values (sys/proc.h).
pub const SIDL: i8 = 1;
pub const SSTOP: i8 = 4;
pub const SZOMB: i8 = 5;

/// Kernel `struct timeval` on 64-bit macOS (`time_t` 64-bit, `suseconds_t` 32-bit).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Timeval {
    pub sec: i64,
    pub usec: i32,
}

/// `struct extern_proc` (sys/proc.h). Pointers are kept as `usize`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ExternProc {
    /// Union with `p_st1` (two pointers); same size on 64-bit.
    pub p_starttime: Timeval,
    pub p_vmspace: usize,
    pub p_sigacts: usize,
    pub p_flag: i32,
    pub p_stat: i8,
    pub p_pid: i32,
    pub p_oppid: i32,
    pub p_dupfd: i32,
    pub user_stack: usize,
    pub exit_thread: usize,
    pub p_debugger: i32,
    pub sigwait: i32,
    pub p_estcpu: u32,
    pub p_cpticks: i32,
    pub p_pctcpu: u32,
    pub p_wchan: usize,
    pub p_wmesg: usize,
    pub p_swtime: u32,
    pub p_slptime: u32,
    pub p_realtimer: [Timeval; 2],
    pub p_rtime: Timeval,
    pub p_uticks: u64,
    pub p_sticks: u64,
    pub p_iticks: u64,
    pub p_traceflag: i32,
    pub p_tracep: usize,
    pub p_siglist: i32,
    pub p_textvp: usize,
    pub p_holdcnt: i32,
    pub p_sigmask: u32,
    pub p_sigignore: u32,
    pub p_sigcatch: u32,
    pub p_priority: u8,
    pub p_usrpri: u8,
    pub p_nice: i8,
    pub p_comm: [u8; 17],
    pub p_pgrp: usize,
    pub p_addr: usize,
    pub p_xstat: u16,
    pub p_acflag: u16,
    pub p_ru: usize,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Pcred {
    pub pc_lock: [c_char; 72],
    pub pc_ucred: usize,
    pub p_ruid: u32,
    pub p_svuid: u32,
    pub p_rgid: u32,
    pub p_svgid: u32,
    pub p_refcnt: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Ucred {
    pub cr_ref: i32,
    pub cr_uid: u32,
    pub cr_ngroups: i16,
    pub cr_groups: [u32; 16],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Vmspace {
    pub vm_refcnt: i32,
    pub vm_shm: usize,
    pub vm_sizes: [i32; 5],
    pub vm_taddr: usize,
    pub vm_daddr: usize,
    pub vm_maxsaddr: usize,
}

/// `struct eproc` (sys/sysctl.h).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Eproc {
    pub e_paddr: usize,
    pub e_sess: usize,
    pub e_pcred: Pcred,
    pub e_ucred: Ucred,
    pub e_vm: Vmspace,
    pub e_ppid: i32,
    pub e_pgid: i32,
    pub e_jobc: i16,
    pub e_tdev: i32,
    pub e_tpgid: i32,
    pub e_tsess: usize,
    pub e_wmesg: [c_char; 8],
    pub e_xsize: i32,
    pub e_xrssize: i16,
    pub e_xccount: i16,
    pub e_xswrss: i16,
    pub e_flag: i32,
    pub e_login: [c_char; 12],
    pub e_spare: [i32; 4],
}

/// `struct kinfo_proc` as returned by `KERN_PROC_ALL`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KinfoProc {
    pub kp_proc: ExternProc,
    pub kp_eproc: Eproc,
}

// The kernel ABI size of `struct kinfo_proc` on 64-bit macOS.
#[cfg(target_pointer_width = "64")]
const _: () = assert!(std::mem::size_of::<KinfoProc>() == 648);

/// `struct proc_taskinfo` (sys/proc_info.h).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ProcTaskInfo {
    pub pti_virtual_size: u64,
    pub pti_resident_size: u64,
    pub pti_total_user: u64,
    pub pti_total_system: u64,
    pub pti_threads_user: u64,
    pub pti_threads_system: u64,
    pub pti_policy: i32,
    pub pti_faults: i32,
    pub pti_pageins: i32,
    pub pti_cow_faults: i32,
    pub pti_messages_sent: i32,
    pub pti_messages_received: i32,
    pub pti_syscalls_mach: i32,
    pub pti_syscalls_unix: i32,
    pub pti_csw: i32,
    pub pti_threadnum: i32,
    pub pti_numrunning: i32,
    pub pti_priority: i32,
}

/// `struct rusage_info_v2`; only the trailing disk I/O fields are used.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct RusageInfoV2 {
    pub ri_uuid: [u8; 16],
    pub ri_other: [u64; 16],
    pub ri_diskio_bytesread: u64,
    pub ri_diskio_byteswritten: u64,
}

/// `struct proc_fdinfo`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ProcFdInfo {
    pub proc_fd: i32,
    pub proc_fdtype: u32,
}

/// Byte offsets inside `struct socket_fdinfo` (XNU bsd/sys/proc_info.h): `pfi`
/// (proc_fileinfo, 24 bytes), then `psi` (socket_info, 8-aligned so it starts at 24):
/// soi_stat (vinfo_stat, 136 bytes), soi_so (u64), soi_pcb (u64), soi_type (int),
/// soi_protocol (int), soi_family (int).
const SOCKFD_PSI_OFF: usize = 24;
const SOI_TYPE_OFF: usize = 136 + 8 + 8;
pub const SOCKFD_PROTOCOL_OFF: usize = SOCKFD_PSI_OFF + SOI_TYPE_OFF + 4; // 180
pub const SOCKFD_FAMILY_OFF: usize = SOCKFD_PSI_OFF + SOI_TYPE_OFF + 8; // 184
const _: () = assert!(SOCKFD_PROTOCOL_OFF == 180 && SOCKFD_FAMILY_OFF == 184);
const _: () = assert!(std::mem::size_of::<ProcTaskInfo>() == 96);
const _: () = assert!(std::mem::size_of::<RusageInfoV2>() == 160);
const _: () = assert!(std::mem::size_of::<XswUsage>() == 32);
const _: () = assert!(std::mem::size_of::<Timeval>() == 16);

/// `vm_statistics64_data_t` (mach/vm_statistics.h).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VmStatistics64 {
    pub free_count: u32,
    pub active_count: u32,
    pub inactive_count: u32,
    pub wire_count: u32,
    pub zero_fill_count: u64,
    pub reactivations: u64,
    pub pageins: u64,
    pub pageouts: u64,
    pub faults: u64,
    pub cow_faults: u64,
    pub lookups: u64,
    pub hits: u64,
    pub purges: u64,
    pub purgeable_count: u32,
    pub speculative_count: u32,
    pub decompressions: u64,
    pub compressions: u64,
    pub swapins: u64,
    pub swapouts: u64,
    pub compressor_page_count: u32,
    pub throttled_count: u32,
    pub external_page_count: u32,
    pub internal_page_count: u32,
    pub total_uncompressed_pages_in_compressor: u64,
}

const _: () = assert!(std::mem::size_of::<VmStatistics64>() == 152);
pub const HOST_VM_INFO64_COUNT: u32 = (std::mem::size_of::<VmStatistics64>() / 4) as u32;

/// `struct xsw_usage` (sys/sysctl.h).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct XswUsage {
    pub xsu_total: u64,
    pub xsu_avail: u64,
    pub xsu_used: u64,
    pub xsu_pagesize: u32,
    pub xsu_encrypted: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MachTimebaseInfo {
    pub numer: u32,
    pub denom: u32,
}

extern "C" {
    pub static mach_task_self_: MachPort;
    pub fn mach_host_self() -> MachPort;
    pub fn mach_timebase_info(info: *mut MachTimebaseInfo) -> KernReturn;
    pub fn host_statistics(host: MachPort, flavor: c_int, info: *mut c_int, count: *mut u32) -> KernReturn;
    pub fn host_statistics64(host: MachPort, flavor: c_int, info: *mut c_int, count: *mut u32) -> KernReturn;
    pub fn host_processor_info(
        host: MachPort,
        flavor: c_int,
        out_processor_count: *mut u32,
        out_info: *mut *mut c_int,
        out_info_count: *mut u32,
    ) -> KernReturn;
    pub fn host_page_size(host: MachPort, out: *mut usize) -> KernReturn;
    pub fn vm_deallocate(task: MachPort, address: usize, size: usize) -> KernReturn;

    // libproc (part of libSystem).
    pub fn proc_pidinfo(pid: c_int, flavor: c_int, arg: u64, buffer: *mut c_void, size: c_int) -> c_int;
    pub fn proc_pidfdinfo(pid: c_int, fd: c_int, flavor: c_int, buffer: *mut c_void, size: c_int) -> c_int;
    pub fn proc_name(pid: c_int, buffer: *mut c_void, size: u32) -> c_int;
    pub fn proc_pid_rusage(pid: c_int, flavor: c_int, buffer: *mut c_void) -> c_int;
}
