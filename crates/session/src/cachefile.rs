//! Xylem's own cache files (remembered scans, snapshots, history):
//! written lz4-compressed, atomically, under one cross-process lock.
//!
//! Why: a remembered scan of C: was ~150 MB of writes per scan (106 MB
//! index + 39 MB snapshot + history); with scheduled scans that adds up on
//! an SSD. lz4 (pure Rust) shrinks the index to ~35% and snapshots to ~16%
//! in ~0.1 s, and reads back at GB/s. Files from older Xylem versions
//! (uncompressed) still load: the lz4 frame magic tells them apart.

use std::ffi::c_void;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// The lz4 frame format's magic number, little-endian.
const LZ4_MAGIC: [u8; 4] = [0x04, 0x22, 0x4D, 0x18];
/// lz4 can't expand data more than ~255x; anything claiming more is damage.
pub const LZ4_MAX_RATIO: u64 = 255;

/// The temporary file for `path`: `<name>.tmp` next to it, so two files
/// never share one (`a.idx` and `a.snap` used to both write `a.tmp`).
fn tmp_of(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

/// Writes `path` compressed, through a temporary file renamed into place,
/// so a crash or a full disk never leaves a half-written file.
pub fn write(path: &Path, f: impl FnOnce(&mut dyn Write) -> io::Result<()>) -> io::Result<()> {
    if let Some(d) = path.parent() {
        fs::create_dir_all(d)?;
    }
    let tmp = tmp_of(path);
    let r = (|| {
        let mut enc = lz4_flex::frame::FrameEncoder::new(BufWriter::with_capacity(1 << 20, File::create(&tmp)?));
        f(&mut enc)?;
        let mut w = enc.finish().map_err(io::Error::other)?;
        w.flush()
    })();
    match r {
        Ok(()) => fs::rename(&tmp, path),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Opens a cache file for reading, compressed or not. Returns the reader
/// and, for a compressed file, the most bytes it can hold uncompressed.
pub fn open(path: &Path) -> io::Result<(Box<dyn Read>, Option<u64>)> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let mut head = [0u8; 4];
    let n = f.read(&mut head)?;
    let f = File::open(path)?; // from the start again
    if n == 4 && head == LZ4_MAGIC {
        let r = BufReader::with_capacity(1 << 20, f);
        Ok((Box::new(lz4_flex::frame::FrameDecoder::new(r)), Some(len.saturating_mul(LZ4_MAX_RATIO))))
    } else {
        Ok((Box::new(BufReader::with_capacity(1 << 20, f)), None))
    }
}

/// True when the reader has nothing left (a cache file must end exactly).
pub fn at_end(r: &mut dyn Read) -> bool {
    let mut b = [0u8; 1];
    matches!(r.read(&mut b), Ok(0))
}

/// Total size on disk of a root's cache files (index, snapshots, history).
pub fn bytes_written_per_save(dir: &Path, key: &str) -> u64 {
    ["idx", "snap", "hist"].iter().filter_map(|e| fs::metadata(dir.join(format!("{key}.{e}"))).ok()).map(|m| m.len()).sum()
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateMutexW(attrs: *mut c_void, initial: i32, name: *const u16) -> *mut c_void;
    fn WaitForSingleObject(h: *mut c_void, ms: u32) -> u32;
    fn ReleaseMutex(h: *mut c_void) -> i32;
    fn CloseHandle(h: *mut c_void) -> i32;
}

/// One writer at a time for the cache, across processes (the app and a
/// scheduled scan can finish at the same moment).
pub struct Lock(*mut c_void);

impl Lock {
    /// Waits up to 2 minutes; `None` if the lock can't be had.
    pub fn take() -> Option<Self> {
        let name: Vec<u16> = "Local\\XylemCacheWrite".encode_utf16().chain([0]).collect();
        // SAFETY: NUL-terminated name; the handle is closed in Drop.
        let h = unsafe { CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
        if h.is_null() {
            return None;
        }
        // WAIT_OBJECT_0 = 0, WAIT_ABANDONED = 0x80 (the other process died: ours now).
        match unsafe { WaitForSingleObject(h, 120_000) } {
            0 | 0x80 => Some(Self(h)),
            _ => {
                unsafe { CloseHandle(h) };
                None
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: we own the mutex (waited on it) and the handle.
        unsafe {
            ReleaseMutex(self.0);
            CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_round_trip_and_old_files_still_open() {
        let dir = std::env::temp_dir().join(format!("xylem-cachefile-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let p = dir.join("a.snap");
        let data: Vec<u8> = (0..200_000u32).flat_map(|i| (i % 1000).to_le_bytes()).collect();
        write(&p, |w| w.write_all(&data)).unwrap();
        assert!(fs::metadata(&p).unwrap().len() < data.len() as u64 / 3, "compressed");
        assert!(!tmp_of(&p).exists());
        let (mut r, max) = open(&p).unwrap();
        let mut back = Vec::new();
        r.read_to_end(&mut back).unwrap();
        assert_eq!(back, data);
        assert!(max.unwrap() >= data.len() as u64);
        // An old, uncompressed file opens as is.
        let old = dir.join("b.snap");
        fs::write(&old, b"XYLMSNP2 plain").unwrap();
        let (mut r, max) = open(&old).unwrap();
        let mut back = Vec::new();
        r.read_to_end(&mut back).unwrap();
        assert_eq!((back.as_slice(), max), (&b"XYLMSNP2 plain"[..], None));
        // Damaged compressed data is an error, never a panic.
        let mut bytes = fs::read(&p).unwrap();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xFF;
        bytes.truncate(bytes.len() - 7);
        fs::write(&p, &bytes).unwrap();
        let (mut r, _) = open(&p).unwrap();
        let mut back = Vec::new();
        assert!(r.read_to_end(&mut back).is_err() || back != data);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_lock_is_exclusive_and_released() {
        let a = Lock::take().expect("lock");
        let t = std::thread::spawn(|| Lock::take().is_some());
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!t.is_finished(), "a second writer waits");
        drop(a);
        assert!(t.join().unwrap(), "and gets it after");
    }
}
