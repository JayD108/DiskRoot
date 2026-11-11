//! Live updates: watches the scanned folder (or drive) for changes while the
//! app is open, using `ReadDirectoryChangesW`. No admin rights, no polling of
//! the disk: Windows reports what changed, and the app re-measures only those
//! paths ([`crate::Scanned::apply_live`]).
//!
//! Changes are batched: at most one batch per second reaches the UI. If
//! Windows reports that its buffer overflowed (too many changes at once),
//! the UI is told so and suggests a rescan instead of guessing.

use std::collections::HashSet;
use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

const FILE_LIST_DIRECTORY: u32 = 0x1;
const SHARE_ALL: u32 = 0x7;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
const NOTIFY: u32 = 0x1 | 0x2 | 0x8 | 0x10; // file name, dir name, size, last write
const WAIT_OBJECT_0: u32 = 0;
const INVALID_HANDLE: *mut c_void = -1isize as *mut c_void;
/// One batch per second at most.
pub const BATCH: Duration = Duration::from_secs(1);

#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: *mut c_void,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateFileW(name: *const u16, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, template: *mut c_void) -> *mut c_void;
    fn ReadDirectoryChangesW(
        dir: *mut c_void,
        buf: *mut c_void,
        len: u32,
        subtree: i32,
        filter: u32,
        returned: *mut u32,
        ov: *mut Overlapped,
        routine: *mut c_void,
    ) -> i32;
    fn GetOverlappedResult(h: *mut c_void, ov: *mut Overlapped, n: *mut u32, wait: i32) -> i32;
    fn CancelIoEx(h: *mut c_void, ov: *mut Overlapped) -> i32;
    fn CreateEventW(sa: *mut c_void, manual: i32, initial: i32, name: *const u16) -> *mut c_void;
    fn ResetEvent(h: *mut c_void) -> i32;
    fn WaitForSingleObject(h: *mut c_void, ms: u32) -> u32;
    fn CloseHandle(h: *mut c_void) -> i32;
}

/// What the watcher reports.
#[derive(Debug)]
pub enum Event {
    /// Paths (relative to the watched root, `\`-separated) that changed.
    Changed(Vec<String>),
    /// Too many changes at once: Windows dropped some. Rescan to be exact.
    Overflow,
    /// The watch stopped (e.g. the folder was removed or the drive ejected).
    Stopped(String),
}

/// A running watch; dropping it stops the thread.
pub struct Watcher {
    stop: Arc<AtomicBool>,
    pub rx: Receiver<Event>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

struct Owned(*mut c_void);
// SAFETY: plain kernel handles, used by one thread at a time.
unsafe impl Send for Owned {}
impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: we own the handle and close it once.
        unsafe { CloseHandle(self.0) };
    }
}

impl Watcher {
    /// Starts watching `root` and everything below it.
    pub fn start(root: &str) -> Result<Self, String> {
        let wide: Vec<u16> = root.encode_utf16().chain([0]).collect();
        // SAFETY: NUL-terminated path; the handle is checked below.
        let h = unsafe {
            CreateFileW(wide.as_ptr(), FILE_LIST_DIRECTORY, SHARE_ALL, null_mut(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED, null_mut())
        };
        if h == INVALID_HANDLE || h.is_null() {
            return Err(format!("Can't watch {root}: {}", std::io::Error::last_os_error()));
        }
        let dir = Owned(h);
        // SAFETY: unnamed manual-reset event.
        let ev = unsafe { CreateEventW(null_mut(), 1, 0, std::ptr::null()) };
        if ev.is_null() {
            return Err("Can't create an event".into());
        }
        let ev = Owned(ev);
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let s2 = Arc::clone(&stop);
        std::thread::Builder::new()
            .name("xylem-watch".into())
            .spawn(move || run(&dir, &ev, &s2, &tx))
            .map_err(|e| e.to_string())?;
        Ok(Self { stop, rx })
    }
}

fn run(dir: &Owned, ev: &Owned, stop: &AtomicBool, tx: &Sender<Event>) {
    // DWORD-aligned buffer, 64 KB (the limit for network shares).
    let mut buf = vec![0u64; 8192];
    let len = (buf.len() * 8) as u32;
    let mut pending: HashSet<String> = HashSet::new();
    let mut overflow = false;
    let mut last_send = Instant::now();
    loop {
        let mut ov = Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, event: ev.0 };
        // SAFETY: valid handles; `buf` and `ov` outlive the I/O, which is
        // always completed or cancelled (and waited for) before reuse.
        unsafe { ResetEvent(ev.0) };
        let ok = unsafe { ReadDirectoryChangesW(dir.0, buf.as_mut_ptr().cast(), len, 1, NOTIFY, null_mut(), &mut ov, null_mut()) };
        if ok == 0 {
            let _ = tx.send(Event::Stopped(std::io::Error::last_os_error().to_string()));
            return;
        }
        // Wait in short slices so stop and the batch timer are honoured.
        loop {
            // SAFETY: valid event handle.
            if unsafe { WaitForSingleObject(ev.0, 250) } == WAIT_OBJECT_0 {
                break;
            }
            if stop.load(Ordering::Relaxed) {
                let mut n = 0;
                // SAFETY: cancel our own pending read, then wait for it to
                // finish so `buf`/`ov` aren't used after we return.
                unsafe {
                    CancelIoEx(dir.0, &mut ov);
                    GetOverlappedResult(dir.0, &mut ov, &mut n, 1);
                }
                return;
            }
            flush(&mut pending, &mut overflow, &mut last_send, tx);
        }
        let mut n = 0u32;
        // SAFETY: the read completed (event signalled).
        if unsafe { GetOverlappedResult(dir.0, &mut ov, &mut n, 0) } == 0 {
            let _ = tx.send(Event::Stopped(std::io::Error::last_os_error().to_string()));
            return;
        }
        if n == 0 {
            overflow = true;
        } else {
            // SAFETY: the first `n` bytes were written by the read.
            let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), n as usize) };
            parse(bytes, &mut pending);
        }
        if stop.load(Ordering::Relaxed) {
            return;
        }
        flush(&mut pending, &mut overflow, &mut last_send, tx);
    }
}

fn flush(pending: &mut HashSet<String>, overflow: &mut bool, last: &mut Instant, tx: &Sender<Event>) {
    if last.elapsed() < BATCH || (pending.is_empty() && !*overflow) {
        return;
    }
    if std::mem::take(overflow) {
        pending.clear();
        let _ = tx.send(Event::Overflow);
    } else {
        let _ = tx.send(Event::Changed(pending.drain().collect()));
    }
    *last = Instant::now();
}

/// FILE_NOTIFY_INFORMATION records: next offset, action, name length
/// (bytes), name (UTF-16).
fn parse(bytes: &[u8], out: &mut HashSet<String>) {
    let mut off = 0usize;
    while let Some(rec) = bytes.get(off..) {
        let rd = |at: usize| rec.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")));
        let (Some(next), Some(name_len)) = (rd(0), rd(8)) else { return };
        let Some(raw) = rec.get(12..12 + name_len as usize) else { return };
        let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        out.insert(String::from_utf16_lossy(&units));
        if next == 0 {
            return;
        }
        off += next as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_notification_records() {
        let rec = |next: u32, action: u32, name: &str| {
            let u: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
            let mut v = Vec::new();
            v.extend_from_slice(&next.to_le_bytes());
            v.extend_from_slice(&action.to_le_bytes());
            v.extend_from_slice(&(u.len() as u32).to_le_bytes());
            v.extend_from_slice(&u);
            while v.len() % 4 != 0 {
                v.push(0);
            }
            v
        };
        let a = rec(0, 3, "sub\\a.txt");
        let mut buf = rec(0, 1, "x");
        let first_len = buf.len() as u32;
        buf[0..4].copy_from_slice(&first_len.to_le_bytes());
        buf.extend_from_slice(&a);
        let mut set = HashSet::new();
        parse(&buf, &mut set);
        assert!(set.contains("x") && set.contains("sub\\a.txt"), "{set:?}");
        // Truncated or garbage input never panics.
        parse(&buf[..7], &mut set);
        parse(&[0xff; 40], &mut set);
    }

    #[test]
    fn reports_changes_in_a_real_folder() {
        let dir = std::env::temp_dir().join(format!("xylem-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let w = Watcher::start(&dir.display().to_string()).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        std::fs::write(dir.join("sub").join("new.bin"), vec![0u8; 5000]).unwrap();
        let got = w.rx.recv_timeout(Duration::from_secs(5)).unwrap();
        match got {
            Event::Changed(p) => assert!(p.iter().any(|x| x.eq_ignore_ascii_case("sub\\new.bin")), "{p:?}"),
            other => panic!("{other:?}"),
        }
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
