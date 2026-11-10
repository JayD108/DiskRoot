//! Parallel user-mode folder scanner for Windows. Needs no admin rights.
//!
//! Each worker thread lists one folder at a time with `NtQueryDirectoryFile`
//! and a 64 KB buffer. A single call returns hundreds of entries together
//! with their sizes, size on disk and attributes, so no extra call per file
//! is needed.
//!
//! This is the fallback scanner. The raw-MFT scanner (Phase 2) is the fast
//! path when running as admin on NTFS.

#[cfg(not(windows))]
compile_error!("xylem-scan-walk only supports Windows");

mod ffi;
pub mod sys;
mod walker;

pub use xylem_core::{Scan, ScanStats};
pub use walker::{PROGRESS, ScanOptions, default_threads, scan};
