//! Local FFI declarations for the Windows backend.
//!
//! Two groups live here:
//! - Undocumented NT structures that windows-sys does not define. Layouts are copied from
//!   the phnt headers (`ntexapi.h`, System Informer project) and match the declarations
//!   psutil uses in `psutil/arch/windows/ntextapi.h`.
//! - A few functions whose windows-sys features (`Wdk_System_SystemServices`,
//!   `Win32_System_Diagnostics_Debug`) are not enabled in Cargo.toml, plus
//!   `NtQuerySystemInformationEx`, which windows-sys 0.59 does not bind at all.

#![allow(dead_code)]

use core::ffi::c_void;
use windows_sys::Win32::Foundation::{NTSTATUS, UNICODE_STRING};
use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

/// `SystemPageFileInformation` (phnt `SYSTEM_INFORMATION_CLASS` value 18).
pub const SYSTEM_PAGEFILE_INFORMATION: i32 = 18;
/// `SEM_FAILCRITICALERRORS` (errhandlingapi.h).
pub const SEM_FAILCRITICALERRORS: u32 = 0x0001;
/// `FILE_READ_ONLY_VOLUME` (winnt.h).
pub const FILE_READ_ONLY_VOLUME: u32 = 0x0008_0000;
/// `ALL_PROCESSOR_GROUPS` (winnt.h).
pub const ALL_PROCESSOR_GROUPS: u16 = 0xFFFF;

/// `SYSTEM_PROCESS_INFORMATION` (phnt ntexapi.h), without the trailing
/// `SYSTEM_THREAD_INFORMATION Threads[]` array, which starts at `size_of::<Self>()`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SystemProcessInformation {
    pub next_entry_offset: u32,
    pub number_of_threads: u32,
    pub working_set_private_size: i64,
    pub hard_fault_count: u32,
    pub number_of_threads_high_watermark: u32,
    pub cycle_time: u64,
    pub create_time: i64,
    pub user_time: i64,
    pub kernel_time: i64,
    pub image_name: UNICODE_STRING,
    pub base_priority: i32,
    pub unique_process_id: usize,
    pub inherited_from_unique_process_id: usize,
    pub handle_count: u32,
    pub session_id: u32,
    pub unique_process_key: usize,
    pub peak_virtual_size: usize,
    pub virtual_size: usize,
    pub page_fault_count: u32,
    pub peak_working_set_size: usize,
    pub working_set_size: usize,
    pub quota_peak_paged_pool_usage: usize,
    pub quota_paged_pool_usage: usize,
    pub quota_peak_non_paged_pool_usage: usize,
    pub quota_non_paged_pool_usage: usize,
    pub pagefile_usage: usize,
    pub peak_pagefile_usage: usize,
    pub private_page_count: usize,
    pub read_operation_count: i64,
    pub write_operation_count: i64,
    pub other_operation_count: i64,
    pub read_transfer_count: i64,
    pub write_transfer_count: i64,
    pub other_transfer_count: i64,
}

/// `SYSTEM_THREAD_INFORMATION` (phnt ntexapi.h).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SystemThreadInformation {
    pub kernel_time: i64,
    pub user_time: i64,
    pub create_time: i64,
    pub wait_time: u32,
    pub start_address: usize,
    /// `CLIENT_ID { UniqueProcess, UniqueThread }`.
    pub client_id: [usize; 2],
    pub priority: i32,
    pub base_priority: i32,
    pub context_switches: u32,
    /// `KTHREAD_STATE`: 1 Ready, 2 Running, 3 Standby, 5 Waiting.
    pub thread_state: u32,
    /// `KWAIT_REASON`: 5 Suspended.
    pub wait_reason: u32,
}

/// `SYSTEM_PROCESSOR_PERFORMANCE_INFORMATION` with the members Microsoft hides in
/// `Reserved1`/`Reserved2` named as in phnt.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ProcessorPerformance {
    pub idle_time: i64,
    pub kernel_time: i64,
    pub user_time: i64,
    pub dpc_time: i64,
    pub interrupt_time: i64,
    pub interrupt_count: u32,
}

/// Byte offsets inside `SYSTEM_PERFORMANCE_INFORMATION` (phnt ntexapi.h; the
/// pre-Windows 10 part of the struct is 312 bytes and has not moved since NT 4).
pub const PERF_CONTEXT_SWITCHES_OFFSET: usize = 296;
pub const PERF_SYSTEM_CALLS_OFFSET: usize = 308;
pub const PERF_MIN_SIZE: usize = 312;

/// `SYSTEM_PAGEFILE_INFORMATION` (phnt ntexapi.h). Sizes are in pages.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SystemPageFileInformation {
    pub next_entry_offset: u32,
    pub total_size: u32,
    pub total_in_use: u32,
    pub peak_usage: u32,
    pub page_file_name: UNICODE_STRING,
}

/// `PROCESSOR_POWER_INFORMATION` (documented on Microsoft Learn, missing from WinNT.h).
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ProcessorPowerInformation {
    pub number: u32,
    pub max_mhz: u32,
    pub current_mhz: u32,
    pub mhz_limit: u32,
    pub max_idle_state: u32,
    pub current_idle_state: u32,
}

#[link(name = "ntdll")]
extern "system" {
    pub fn RtlGetVersion(info: *mut OSVERSIONINFOW) -> NTSTATUS;
    pub fn NtQuerySystemInformationEx(
        class: i32,
        input: *const c_void,
        input_len: u32,
        output: *mut c_void,
        output_len: u32,
        return_len: *mut u32,
    ) -> NTSTATUS;
}

#[link(name = "kernel32")]
extern "system" {
    pub fn SetErrorMode(mode: u32) -> u32;
}
