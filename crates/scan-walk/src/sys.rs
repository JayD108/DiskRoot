//! Process-level helpers.

use crate::ffi::{GetCurrentProcess, K32GetProcessMemoryInfo, ProcessMemoryCounters};

/// Peak working set (peak RAM) of this process, in bytes.
pub fn peak_memory_bytes() -> Option<u64> {
    let mut c = ProcessMemoryCounters {
        cb: size_of::<ProcessMemoryCounters>() as u32,
        ..Default::default()
    };
    // SAFETY: `c` is a correctly sized, writable PROCESS_MEMORY_COUNTERS and
    // GetCurrentProcess returns a pseudo-handle that needs no closing.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    (ok != 0).then_some(c.peak_working_set_size as u64)
}
