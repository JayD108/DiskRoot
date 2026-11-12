//! Process-level helpers.

use crate::ffi::{GetCurrentProcess, GetProcessTimes, K32GetProcessMemoryInfo, ProcessMemoryCounters};

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

/// This process's memory right now: (working set, peak working set,
/// private bytes), in bytes. Working set = RAM in use; private bytes =
/// memory only this process holds (Task Manager's "Commit size").
pub fn memory() -> Option<(u64, u64, u64)> {
    let mut c = ProcessMemoryCounters { cb: size_of::<ProcessMemoryCounters>() as u32, ..Default::default() };
    // SAFETY: as in `peak_memory_bytes`.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    (ok != 0).then_some((c.working_set_size as u64, c.peak_working_set_size as u64, c.pagefile_usage as u64))
}

/// CPU time this process has used so far (kernel + user), in seconds.
pub fn cpu_seconds() -> Option<f64> {
    let (mut a, mut b, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
    // SAFETY: valid out pointers; pseudo-handle of this process.
    let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut a, &mut b, &mut k, &mut u) };
    (ok != 0).then(|| (k + u) as f64 / 1e7)
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_own_memory_and_cpu() {
        let (ws, peak, private) = super::memory().unwrap();
        assert!(ws > 0 && peak >= ws && private > 0);
        let t0 = super::cpu_seconds().unwrap();
        let mut x = 0u64;
        for i in 0..50_000_000u64 {
            x = x.wrapping_mul(31).wrapping_add(i);
        }
        std::hint::black_box(x);
        assert!(super::cpu_seconds().unwrap() > t0);
    }
}
