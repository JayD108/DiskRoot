//! Minimal hand-written Win32 / NT declarations. `raw-dylib` means no
//! import libraries are needed at link time.

use std::ffi::c_void;

pub type Handle = *mut c_void;

// NtOpenFile
pub const FILE_LIST_DIRECTORY: u32 = 0x0000_0001;
pub const SYNCHRONIZE: u32 = 0x0010_0000;
pub const FILE_SHARE_ALL: u32 = 0x1 | 0x2 | 0x4; // read | write | delete
pub const FILE_DIRECTORY_FILE: u32 = 0x0000_0001;
pub const FILE_SYNCHRONOUS_IO_NONALERT: u32 = 0x0000_0020;
pub const FILE_OPEN_FOR_BACKUP_INTENT: u32 = 0x0000_4000;
pub const OBJ_CASE_INSENSITIVE: u32 = 0x0000_0040;

#[repr(C)]
pub struct UnicodeString {
    /// In bytes, without a terminating NUL.
    pub length: u16,
    pub maximum_length: u16,
    pub buffer: *mut u16,
}

#[repr(C)]
pub struct ObjectAttributes {
    pub length: u32,
    pub root_directory: Handle,
    pub object_name: *mut UnicodeString,
    pub attributes: u32,
    pub security_descriptor: *mut c_void,
    pub security_qos: *mut c_void,
}

// NtQueryDirectoryFile
/// FILE_FULL_DIR_INFORMATION: no short (8.3) names, which are costly to look up.
pub const FILE_FULL_DIRECTORY_INFORMATION: u32 = 2;
pub const STATUS_SUCCESS: u32 = 0;
pub const STATUS_NO_MORE_FILES: u32 = 0x8000_0006;
pub const STATUS_NO_SUCH_FILE: u32 = 0xC000_000F;

#[repr(C)]
#[derive(Default)]
pub struct IoStatusBlock {
    pub status: usize,
    pub information: usize,
}

#[repr(C)]
#[derive(Default)]
pub struct ProcessMemoryCounters {
    pub cb: u32,
    pub page_fault_count: u32,
    pub peak_working_set_size: usize,
    pub working_set_size: usize,
    pub quota_peak_paged_pool_usage: usize,
    pub quota_paged_pool_usage: usize,
    pub quota_peak_non_paged_pool_usage: usize,
    pub quota_non_paged_pool_usage: usize,
    pub pagefile_usage: usize,
    pub peak_pagefile_usage: usize,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn GetCurrentProcess() -> Handle;
    pub fn K32GetProcessMemoryInfo(
        process: Handle,
        counters: *mut ProcessMemoryCounters,
        cb: u32,
    ) -> i32;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    pub fn NtOpenFile(
        handle: *mut Handle,
        access: u32,
        object_attributes: *mut ObjectAttributes,
        io_status: *mut IoStatusBlock,
        share: u32,
        open_options: u32,
    ) -> i32;
    pub fn NtQueryDirectoryFile(
        handle: Handle,
        event: Handle,
        apc_routine: *mut c_void,
        apc_context: *mut c_void,
        io_status: *mut IoStatusBlock,
        buffer: *mut c_void,
        length: u32,
        class: u32,
        return_single_entry: u8,
        file_name: *mut c_void,
        restart_scan: u8,
    ) -> i32;
}
