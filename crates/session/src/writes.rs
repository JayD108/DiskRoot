//! Who's writing: per-process and per-drive write rates, without admin.
//!
//! - **Processes:** one `NtQuerySystemInformation(SystemProcessInformation)`
//!   call lists every process with its `WriteTransferCount` (bytes written
//!   since it started). No process is opened. This counts *all* writes the
//!   process makes (files, pipes, some network), so the app calls it
//!   "writes" and shows the drive's own counter next to it.
//! - **Drives:** `IOCTL_DISK_PERFORMANCE` on a disk opened with no access
//!   rights: bytes written to the disk since boot.
//! - **Idle:** `GetLastInputInfo`: no key or mouse input for 5 minutes.
//!
//! Sampling happens only while the "Who's writing" view is open (every
//! 2 s) and for 30 s at the start of a scheduled scan. Read-only.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// No input for this long = idle.
pub const IDLE_SECS: u64 = 300;

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> i32;
}
#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLastInputInfo(info: *mut [u32; 2]) -> i32;
}
#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetTickCount() -> u32;
    fn GetCurrentProcessId() -> u32;
}

/// One process at one moment: bytes written since it started.
#[derive(Clone, Debug, PartialEq)]
pub struct Proc {
    pub pid: usize,
    pub name: String,
    pub written: u64,
}

const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
// SYSTEM_PROCESS_INFORMATION (x64) field offsets.
const OFF_NEXT: usize = 0;
const OFF_NAME_LEN: usize = 56;
const OFF_NAME_PTR: usize = 64;
const OFF_PID: usize = 80;
const OFF_WRITE_BYTES: usize = 240;
const ENTRY_MIN: usize = 256;

fn rd<const N: usize>(b: &[u8], o: usize) -> Option<[u8; N]> {
    b.get(o..o + N)?.try_into().ok()
}

/// Parses a SystemProcessInformation buffer. Every offset and the name
/// pointer are checked against the buffer; a bad entry ends the walk.
pub fn parse(buf: &[u8]) -> Vec<Proc> {
    let base = buf.as_ptr() as usize;
    let mut out = Vec::new();
    let mut off = 0usize;
    while off.checked_add(ENTRY_MIN).is_some_and(|e| e <= buf.len()) {
        let e = &buf[off..];
        let (Some(next), Some(nlen), Some(nptr), Some(pid), Some(w)) =
            (rd::<4>(e, OFF_NEXT), rd::<2>(e, OFF_NAME_LEN), rd::<8>(e, OFF_NAME_PTR), rd::<8>(e, OFF_PID), rd::<8>(e, OFF_WRITE_BYTES))
        else {
            break;
        };
        let (next, nlen, nptr) = (u32::from_le_bytes(next) as usize, u16::from_le_bytes(nlen) as usize, usize::from_le_bytes(nptr));
        // The name lives inside the same buffer: read it from there, never
        // through the raw pointer.
        let name = if nptr == 0 {
            "System (kernel)".to_owned()
        } else {
            match nptr.checked_sub(base) {
                Some(at) if at.checked_add(nlen).is_some_and(|end| end <= buf.len()) && nlen % 2 == 0 => {
                    let units: Vec<u16> = buf[at..at + nlen].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
                    String::from_utf16_lossy(&units)
                }
                _ => break,
            }
        };
        out.push(Proc { pid: usize::from_le_bytes(pid), name, written: u64::from_le_bytes(w) });
        if next == 0 {
            break;
        }
        off = match off.checked_add(next) {
            Some(o) => o,
            None => break,
        };
    }
    out
}

/// Every process now, with bytes written since it started.
pub fn processes() -> Vec<Proc> {
    let mut len: u32 = 1 << 20;
    for _ in 0..6 {
        let mut buf = vec![0u8; len as usize];
        let mut ret = 0u32;
        // SAFETY: the buffer is valid for `len` bytes; class 5 = SystemProcessInformation.
        let st = unsafe { NtQuerySystemInformation(5, buf.as_mut_ptr(), len, &mut ret) };
        if st == STATUS_INFO_LENGTH_MISMATCH {
            len = ret.max(len).saturating_mul(2);
            continue;
        }
        if st != 0 {
            return Vec::new();
        }
        buf.truncate((ret as usize).min(buf.len()));
        return parse(&buf);
    }
    Vec::new()
}

/// Seconds since the last key or mouse input in this session.
pub fn idle_secs() -> u64 {
    let mut info = [8u32, 0];
    // SAFETY: LASTINPUTINFO is { cbSize, dwTime }.
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return 0;
    }
    u64::from(unsafe { GetTickCount() }.wrapping_sub(info[1])) / 1000
}

/// Bytes written to physical disk `n` since boot (no admin).
pub fn disk_written(n: u32) -> Option<u64> {
    crate::health::disk_written(n)
}

/// Totals for one program (all its processes together).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Totals {
    pub name: String,
    /// Bytes per second over the last sample.
    pub now: f64,
    /// Bytes written while you used the PC / while it was idle.
    pub active: u64,
    pub idle: u64,
}

/// Keeps the previous sample and adds up what each program wrote.
#[derive(Default)]
pub struct Sampler {
    prev: HashMap<(usize, String), u64>,
    disk_prev: HashMap<u32, u64>,
    pub totals: HashMap<String, Totals>,
    /// (disk number, bytes per second over the last sample).
    pub disks: Vec<(u32, f64)>,
    pub disk_totals: HashMap<u32, u64>,
    pub seconds: f64,
    pub idle_seconds: f64,
    last: Option<std::time::Instant>,
    me: usize,
}

impl Sampler {
    pub fn new(disks: &[u32]) -> Self {
        let mut s = Self { me: unsafe { GetCurrentProcessId() } as usize, ..Self::default() };
        for &d in disks {
            if let Some(w) = disk_written(d) {
                s.disk_prev.insert(d, w);
            }
        }
        s.add(&processes(), 0.0, false);
        s.last = Some(std::time::Instant::now());
        s
    }

    /// Takes a sample now.
    pub fn tick(&mut self) {
        let dt = self.last.map_or(0.0, |t| t.elapsed().as_secs_f64());
        self.last = Some(std::time::Instant::now());
        let idle = idle_secs() >= IDLE_SECS;
        self.add(&processes(), dt, idle);
        let mut disks = Vec::new();
        let keys: Vec<u32> = self.disk_prev.keys().copied().collect();
        for d in keys {
            if let Some(w) = disk_written(d) {
                let before = self.disk_prev.insert(d, w).unwrap_or(w);
                let delta = w.saturating_sub(before);
                *self.disk_totals.entry(d).or_default() += delta;
                disks.push((d, if dt > 0.0 { delta as f64 / dt } else { 0.0 }));
            }
        }
        disks.sort_unstable_by_key(|x| x.0);
        self.disks = disks;
    }

    /// Adds one sample of processes taken `dt` seconds after the last.
    pub fn add(&mut self, procs: &[Proc], dt: f64, idle: bool) {
        let mut seen: HashMap<(usize, String), u64> = HashMap::with_capacity(procs.len());
        let mut rate: HashMap<String, f64> = HashMap::new();
        for p in procs {
            if p.pid == self.me || p.pid == 0 {
                continue;
            }
            let key = (p.pid, p.name.clone());
            // A new process (or a reused pid) starts from its current total.
            if let Some(&before) = self.prev.get(&key)
                && dt > 0.0
            {
                let d = p.written.saturating_sub(before);
                let t = self.totals.entry(p.name.clone()).or_insert_with(|| Totals { name: p.name.clone(), ..Totals::default() });
                if idle {
                    t.idle += d;
                } else {
                    t.active += d;
                }
                *rate.entry(p.name.clone()).or_default() += d as f64 / dt;
            }
            seen.insert(key, p.written);
        }
        for t in self.totals.values_mut() {
            t.now = rate.get(&t.name).copied().unwrap_or(0.0);
        }
        self.prev = seen;
        self.seconds += dt;
        if idle {
            self.idle_seconds += dt;
        }
    }

    /// Programs, biggest writers (active + idle) first.
    pub fn top(&self, n: usize) -> Vec<Totals> {
        let mut v: Vec<Totals> = self.totals.values().filter(|t| t.active + t.idle > 0).cloned().collect();
        v.sort_by(|a, b| (b.active + b.idle).cmp(&(a.active + a.idle)).then_with(|| a.name.cmp(&b.name)));
        v.truncate(n);
        v
    }
}

/// One program's writes during a background sample (for the run log).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Writer {
    pub name: String,
    /// Bytes per second over the sample.
    pub rate: f64,
}

/// A short sample (scheduled scans): the top writers over `secs` seconds,
/// and whether the PC was idle the whole time.
pub fn sample_for(secs: u64, n: usize) -> (Vec<Writer>, bool) {
    let idle_before = idle_secs() >= IDLE_SECS;
    let mut s = Sampler::new(&[]);
    std::thread::sleep(std::time::Duration::from_secs(secs));
    s.tick();
    let idle = idle_before && idle_secs() >= IDLE_SECS + secs;
    let total = s.seconds.max(1.0);
    let top = s.top(n).into_iter().map(|t| Writer { rate: (t.active + t.idle) as f64 / total, name: t.name }).collect();
    (top, idle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn totals_split_active_and_idle_and_group_by_program() {
        let mut s = Sampler::default();
        let p = |pid, name: &str, w| Proc { pid, name: name.into(), written: w };
        s.add(&[p(10, "a.exe", 100), p(11, "a.exe", 0), p(12, "b.exe", 5)], 0.0, false);
        s.add(&[p(10, "a.exe", 1100), p(11, "a.exe", 1000), p(12, "b.exe", 5)], 2.0, false);
        s.add(&[p(10, "a.exe", 3100), p(12, "b.exe", 405), p(13, "c.exe", 9999)], 2.0, true);
        let top = s.top(10);
        assert_eq!(top.len(), 2, "c.exe is new (no delta yet); b.exe wrote only while idle: {top:?}");
        assert_eq!((top[0].name.as_str(), top[0].active, top[0].idle), ("a.exe", 2000, 2000));
        assert_eq!((top[1].name.as_str(), top[1].active, top[1].idle), ("b.exe", 0, 400));
        assert!((top[0].now - 1000.0).abs() < 1e-9, "{}", top[0].now);
        assert!((s.seconds, s.idle_seconds) == (4.0, 2.0));
        // A reused pid with a smaller counter doesn't go negative.
        s.add(&[p(10, "a.exe", 50)], 2.0, false);
        assert_eq!(s.top(1)[0].active, 2000);
    }

    #[test]
    fn parser_checks_every_offset() {
        assert!(parse(&[]).is_empty());
        assert!(parse(&[0u8; 100]).is_empty());
        // A name pointer outside the buffer ends the walk instead of reading it.
        let mut b = vec![0u8; 512];
        b[OFF_NAME_LEN..OFF_NAME_LEN + 2].copy_from_slice(&8u16.to_le_bytes());
        b[OFF_NAME_PTR..OFF_NAME_PTR + 8].copy_from_slice(&0x10usize.to_le_bytes());
        assert!(parse(&b).is_empty());
        // Random buffers never panic.
        let mut rng = 0x1357_9bdf_2468_ace0_u64;
        for _ in 0..5000 {
            let b: Vec<u8> = (0..1024)
                .map(|_| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    rng as u8
                })
                .collect();
            let _ = parse(&b);
        }
    }

    #[test]
    fn real_processes_and_disk_counter() {
        let ps = processes();
        assert!(ps.len() > 10, "{}", ps.len());
        assert!(ps.iter().any(|p| p.name.eq_ignore_ascii_case("explorer.exe") || p.name.eq_ignore_ascii_case("svchost.exe")));
        let me = unsafe { GetCurrentProcessId() } as usize;
        assert!(ps.iter().any(|p| p.pid == me));
        if let Some(n) = crate::health::disk_of("C:\\") {
            assert!(disk_written(n).is_some_and(|w| w > 0));
        }
    }
}
