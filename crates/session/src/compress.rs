//! Xylem's compression engine: fast, transparent compression of a folder.
//!
//! It uses Windows' own file compression (WOF, the same format `compact.exe
//! /EXE` writes): files stay where they are, every program opens them as
//! usual, and Windows decompresses on read. Xylem doesn't invent a format;
//! it drives Windows directly instead of through compact.exe, which is what
//! makes it fast:
//!
//! - **Many files at once** (one worker per CPU core, minus two, max 16),
//!   biggest files first. compact.exe does one file at a time.
//! - **Skips what won't shrink** before spending time on it: known compressed
//!   formats (photos, video, archives), files whose first 64 KB look random
//!   (byte entropy > 7.95 bits/byte), single-cluster files, and files that
//!   are already compressed, sparse, encrypted, links or cloud placeholders.
//!   Windows also refuses a file that wouldn't get smaller (error 344).
//! - **Real progress and Cancel** (between files).
//!
//! Safety, measured in the R&D probe (`xylem-rnd`, E9): the bytes read back
//! are identical, timestamps don't change, undo restores normal storage.
//! Each file is opened for reading only, without write or delete sharing,
//! so nothing can change or rename it meanwhile; a file another program is
//! writing is skipped ("in use"). Windows still checks that the user may
//! change the file (read-only files give "access denied"), so no rights are
//! gained. Every file's real location is checked to be inside the chosen
//! folder, so a folder swapped for a junction can't redirect the work.
//!
//! Levels: **Fast** = XPRESS16K (~450 MB/s on 16 threads, saves 70-95% of
//! what Maximum saves), **Maximum** = LZX (~125 MB/s, the most savings).

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

type Handle = *mut c_void;
const INVALID: Handle = -1isize as Handle;

const GENERIC_READ: u32 = 0x8000_0000;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_ALL: u32 = 7;
const OPEN_EXISTING: u32 = 3;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;

const FSCTL_SET_EXTERNAL_BACKING: u32 = 0x0009_030C;
const FSCTL_DELETE_EXTERNAL_BACKING: u32 = 0x0009_0314;
const FILE_ATTRIBUTE_TAG_INFO: u32 = 9;
const FILE_STANDARD_INFO: u32 = 1;
const IO_REPARSE_TAG_WOF: u32 = 0x8000_0017;

const ATTR_DIRECTORY: u32 = 0x10;
const ATTR_SPARSE: u32 = 0x200;
const ATTR_REPARSE: u32 = 0x400;
const ATTR_COMPRESSED: u32 = 0x800;
const ATTR_OFFLINE: u32 = 0x1000;
const ATTR_ENCRYPTED: u32 = 0x4000;
const ATTR_RECALL_ON_OPEN: u32 = 0x4_0000;
const ATTR_RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;

const ERROR_ACCESS_DENIED: u32 = 5;
const ERROR_SHARING_VIOLATION: u32 = 32;
const ERROR_LOCK_VIOLATION: u32 = 33;
const ERROR_OBJECT_NOT_EXTERNALLY_BACKED: u32 = 342;
const ERROR_COMPRESSION_NOT_BENEFICIAL: u32 = 344;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateFileW(name: *const u16, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, tmpl: Handle) -> Handle;
    fn DeviceIoControl(h: Handle, code: u32, inb: *const c_void, inl: u32, outb: *mut c_void, outl: u32, ret: *mut u32, ov: *mut c_void) -> i32;
    fn ReadFile(h: Handle, buf: *mut u8, len: u32, read: *mut u32, ov: *mut c_void) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn GetLastError() -> u32;
    fn GetCompressedFileSizeW(name: *const u16, hi: *mut u32) -> u32;
    fn GetFileInformationByHandleEx(h: Handle, class: u32, info: *mut c_void, len: u32) -> i32;
    fn GetFinalPathNameByHandleW(h: Handle, buf: *mut u16, len: u32, flags: u32) -> u32;
    fn GetVolumeInformationW(root: *const u16, name: *mut u16, nlen: u32, serial: *mut u32, maxc: *mut u32, fsflags: *mut u32, fs: *mut u16, fslen: u32) -> i32;
    fn GetDiskFreeSpaceExW(dir: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
    fn GetCurrentThread() -> Handle;
    fn SetThreadPriority(h: Handle, prio: i32) -> i32;
}

/// How hard to compress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// XPRESS16K: several times faster than Maximum, usually 70-95% of its savings.
    Fast,
    /// LZX: the most savings, slower. Best for things you rarely change.
    Max,
}

impl Level {
    fn algorithm(self) -> u32 {
        match self {
            Level::Fast => 3, // FILE_PROVIDER_COMPRESSION_XPRESS16K
            Level::Max => 1,  // FILE_PROVIDER_COMPRESSION_LZX
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Level::Fast => "Fast",
            Level::Max => "Maximum",
        }
    }
}

/// What XPRESS16K saves compared with LZX. Measured on 12 real file types
/// (R&D E10, 12 Nov 2026): 0.71-0.95, median 0.86 (HDF5 0.36 is the outlier,
/// where both save little); earlier mixed set (E9): 0.75.
const FAST_SHARE_OF_MAX: f64 = 0.85;

/// One file the engine will work on.
#[derive(Clone, Debug)]
pub struct PlanFile {
    pub node: u32,
    pub path: PathBuf,
    pub size: u64,
    /// Size on disk at scan time.
    pub alloc: u64,
}

/// (files, bytes on disk) left out of a plan for one reason.
pub type Count = (u64, u64);

/// What compressing (or uncompressing) a folder would touch, built from the
/// scan without opening any file.
#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub root: PathBuf,
    /// Biggest first, so one huge file doesn't start last.
    pub files: Vec<PlanFile>,
    /// Size on disk of `files`.
    pub bytes: u64,
    /// Rough saving with Maximum; Fast saves 70-95% of it.
    pub estimate_max: u64,
    /// Photos, videos, archives and other formats that are already compressed.
    pub skipped_formats: Count,
    /// Already compressed (by Windows or NTFS), sparse, or a cloud placeholder / link.
    pub skipped_already: Count,
    /// One cluster or less: nothing to save.
    pub skipped_tiny: Count,
    /// Protected places inside the folder (e.g. a Windows folder): left alone.
    pub protected_inside: u64,
    /// Uncompress only: space the files need back once restored.
    pub needs_free: u64,
}

impl Plan {
    pub fn estimate(&self, level: Level) -> u64 {
        match level {
            Level::Max => self.estimate_max,
            Level::Fast => fast_estimate(self.estimate_max),
        }
    }
}

/// Live counters the window reads while the engine works.
#[derive(Debug, Default)]
pub struct Progress {
    /// Bytes (size on disk at scan time) of the files handled so far.
    pub done: AtomicU64,
    pub files_done: AtomicU64,
    /// Space saved so far (or, for undo, given back to the files).
    pub saved: AtomicU64,
    /// Set by Cancel: workers stop before their next file.
    pub cancel: AtomicBool,
}

/// What happened, for the summary and to update the scan's sizes.
#[derive(Debug, Default)]
pub struct Report {
    pub undo: bool,
    pub level: Option<Level>,
    /// Files compressed (or, for undo, restored).
    pub changed_files: u64,
    /// Size on disk of those files before and after.
    pub before: u64,
    pub after: u64,
    /// Looked random or Windows said it wouldn't get smaller.
    pub wont_shrink: Count,
    /// Another program had the file open for writing.
    pub in_use: Count,
    /// Windows refused (read-only file, no permission).
    pub denied: Count,
    /// Already compressed, sparse, a link or placeholder, or outside the folder.
    pub skipped: Count,
    /// Anything else, with the first error's text.
    pub failed: Count,
    pub first_error: Option<String>,
    pub cancelled: bool,
    /// (node, new size on disk) for every changed file.
    pub new_alloc: Vec<(u32, u64)>,
    pub secs: f64,
}

impl Report {
    pub fn saved(&self) -> u64 {
        self.before.saturating_sub(self.after)
    }
}

/// Already-compressed formats: compressing them saves ~nothing.
pub fn incompressible(ext: &str) -> bool {
    const NONE: &[&str] = &[
        "jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "avif", "jxl", "mp4", "mkv", "mov", "avi", "webm", "m4v", "wmv", "mpg", "mpeg",
        "mp3", "m4a", "aac", "flac", "ogg", "opus", "wma", "zip", "7z", "rar", "gz", "tgz", "xz", "txz", "bz2", "tbz2", "zst", "lz4", "br",
        "lzma", "cab", "jar", "apk", "whl", "nupkg", "msi", "msix", "appx", "vsix", "crx", "xpi", "epub", "docx", "xlsx", "pptx", "odt",
        "ods", "odp", "pdf", "iso", "dmg", "wim", "esd", "vhdx", "vhd", "gguf",
    ];
    let e = ext.to_ascii_lowercase();
    NONE.contains(&e.as_str())
}

/// Rough share LZX saves for files of this extension (estimates only; the
/// real result depends on the data). Measured on real data where noted
/// (11 Nov 2026, 5,519 files of a 140 GB ML dataset, and R&D E9).
pub fn ratio(ext: &str) -> f64 {
    if incompressible(ext) {
        return 0.0;
    }
    match ext.to_ascii_lowercase().as_str() {
        // Model weights: measured .pt 6.9% (31.6 GB).
        "pt" | "pth" | "ckpt" | "safetensors" | "onnx" | "h5" | "hdf5" | "keras" | "tflite" | "pb" | "bin" => 0.07,
        // Float arrays: measured .npy 12.7% (118.5 GB) and 16.9% (E10), .npz 13.9% (np.savez stores them uncompressed).
        "npy" | "npz" | "mat" | "tfrecord" | "arrow" | "feather" => 0.14,
        // EEG recordings (16-bit samples): measured .edf 37.8% (LZX), 32.6% (Fast).
        "edf" | "bdf" => 0.38,
        // Columnar files are compressed inside: measured .parquet 11.8% (2 GB) and
        // 8.1% (E10, LZX); small ones saved more (40%), but big ones hold the bytes.
        "parquet" | "orc" | "avro" => 0.1,
        "exe" | "dll" | "sys" | "node" | "pyd" | "so" => 0.4,
        _ => 0.5,
    }
}

/// What the engine would do with one file, decided from the scan alone
/// (used by the Compress dialog and by Ways to save space, so they agree).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    /// Already compressed (by Windows or NTFS), sparse, a cloud placeholder or a link.
    Already,
    /// One cluster or less: nothing to save.
    Tiny,
    /// Photos, videos, archives and other packed formats.
    Format,
    /// Worth packing; rough saving with Maximum, in bytes.
    Pack(f64),
}

pub fn verdict(name: &str, fl: u8, size: u64, alloc: u64) -> Verdict {
    use xylem_core::flags;
    if fl & (flags::CLOUD | flags::LINK | flags::SPARSE | flags::COMPRESSED) != 0 {
        return Verdict::Already;
    }
    // Windows-compressed files take less space than their size.
    let full = size.div_ceil(4096) * 4096;
    if alloc < full / 10 * 9 {
        return Verdict::Already;
    }
    if alloc <= 4096 {
        return Verdict::Tiny;
    }
    let ext = name.rsplit_once('.').map_or("", |e| e.1);
    if incompressible(ext) {
        return Verdict::Format;
    }
    Verdict::Pack(alloc as f64 * ratio(ext))
}

/// Fast's rough saving from Maximum's.
pub fn fast_estimate(max: u64) -> u64 {
    (max as f64 * FAST_SHARE_OF_MAX) as u64
}

/// Worker threads: every core but two (the window and the rest of the PC
/// stay responsive), at least 1, at most 16 (more doesn't help: disk-bound).
pub fn default_threads() -> usize {
    std::thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(2).clamp(1, 16)
}

fn wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain([0]).collect()
}

fn win_err(code: u32) -> String {
    std::io::Error::from_raw_os_error(code as i32).to_string()
}

/// Checks the drive can hold Windows-compressed files (NTFS). Others
/// (FAT32, exFAT, network drives, ReFS) would fail on every file.
pub fn check_volume(path: &Path) -> Result<(), String> {
    let s = path.to_string_lossy();
    let root = if s.len() >= 2 && s.as_bytes()[1] == b':' { format!("{}\\", &s[..2]) } else { return Err("Only folders on a local drive (like C:) can be compressed.".into()) };
    let w: Vec<u16> = root.encode_utf16().chain([0]).collect();
    let mut fs = [0u16; 32];
    // SAFETY: NUL-terminated root, valid output buffer of the given length.
    let ok = unsafe { GetVolumeInformationW(w.as_ptr(), std::ptr::null_mut(), 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), fs.as_mut_ptr(), fs.len() as u32) };
    if ok == 0 {
        return Err(format!("Couldn't read drive {root}: {}", win_err(unsafe { GetLastError() })));
    }
    let name = String::from_utf16_lossy(&fs[..fs.iter().position(|&c| c == 0).unwrap_or(fs.len())]);
    if name.eq_ignore_ascii_case("NTFS") {
        Ok(())
    } else {
        Err(format!("Drive {root} uses {name}. Windows' file compression works only on NTFS drives."))
    }
}

/// Free space on the drive holding `path`.
pub fn free_space(path: &Path) -> Option<u64> {
    let w = wide(path);
    let mut avail = 0u64;
    // SAFETY: NUL-terminated path; out pointers valid (others may be null).
    (unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut avail, std::ptr::null_mut(), std::ptr::null_mut()) } != 0).then_some(avail)
}

/// Where a handle really points ("c:\users\me\x", lowercase, no `\\?\`).
fn final_path(h: Handle) -> Option<String> {
    let mut buf = vec![0u16; 1024];
    loop {
        // SAFETY: valid handle and buffer of the given length.
        let n = unsafe { GetFinalPathNameByHandleW(h, buf.as_mut_ptr(), buf.len() as u32, 0) } as usize;
        if n == 0 {
            return None;
        }
        if n < buf.len() {
            let s = String::from_utf16_lossy(&buf[..n]);
            let s = s.strip_prefix(r"\\?\UNC\").map(|r| format!(r"\\{r}")).unwrap_or_else(|| s.strip_prefix(r"\\?\").unwrap_or(&s).to_owned());
            return Some(s.to_lowercase());
        }
        buf.resize(n + 1, 0);
    }
}

/// The real location of the chosen folder, to check every file against.
fn root_prefix(root: &Path) -> Result<String, String> {
    let w = wide(root);
    // SAFETY: NUL-terminated path; the handle is checked and closed.
    let h = unsafe { CreateFileW(w.as_ptr(), FILE_READ_ATTRIBUTES, FILE_SHARE_ALL, std::ptr::null_mut(), OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, std::ptr::null_mut()) };
    if h == INVALID {
        return Err(format!("Couldn't open {}: {}", root.display(), win_err(unsafe { GetLastError() })));
    }
    let p = final_path(h);
    unsafe { CloseHandle(h) };
    let mut p = p.ok_or("Couldn't find where the folder really is.")?;
    if !p.ends_with('\\') {
        p.push('\\');
    }
    Ok(p)
}

/// Shannon entropy (bits per byte) of a buffer. Random or already
/// compressed data is close to 8.
fn entropy(b: &[u8]) -> f64 {
    if b.is_empty() {
        return 0.0;
    }
    let mut c = [0u32; 256];
    for &x in b {
        c[x as usize] += 1;
    }
    let n = b.len() as f64;
    c.iter().filter(|&&k| k > 0).map(|&k| {
        let q = f64::from(k) / n;
        -q * q.log2()
    }).sum()
}
const RANDOM_LOOKING: f64 = 7.95;

fn on_disk(path: &Path) -> Option<u64> {
    let w = wide(path);
    let mut hi = 0u32;
    // SAFETY: NUL-terminated path; valid out pointer.
    let lo = unsafe { GetCompressedFileSizeW(w.as_ptr(), &mut hi) };
    if lo == u32::MAX && unsafe { GetLastError() } != 0 {
        return None;
    }
    Some((u64::from(hi) << 32) | u64::from(lo))
}

enum Outcome {
    Changed(u64),
    WontShrink,
    InUse,
    Denied,
    Skipped,
    Failed(String),
}

struct OpenFile(Handle);
impl Drop for OpenFile {
    fn drop(&mut self) {
        // SAFETY: a handle we opened and own.
        unsafe { CloseHandle(self.0) };
    }
}

/// Opens one file for the engine: read-only, nobody else may write, rename
/// or delete it meanwhile; checks it's a plain file inside the folder.
fn open_checked(f: &PlanFile, prefix: &str, undo: bool) -> Result<OpenFile, Outcome> {
    let w = wide(&f.path);
    // SAFETY: NUL-terminated path; the handle is owned by OpenFile.
    let h = unsafe { CreateFileW(w.as_ptr(), GENERIC_READ, FILE_SHARE_READ, std::ptr::null_mut(), OPEN_EXISTING, FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_SEQUENTIAL_SCAN, std::ptr::null_mut()) };
    if h == INVALID {
        return Err(match unsafe { GetLastError() } {
            ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION => Outcome::InUse,
            ERROR_ACCESS_DENIED => Outcome::Denied,
            2 | 3 => Outcome::Skipped, // gone since the scan
            e => Outcome::Failed(win_err(e)),
        });
    }
    let file = OpenFile(h);
    let mut tag = [0u32; 2]; // FILE_ATTRIBUTE_TAG_INFO: attributes, reparse tag
    // SAFETY: valid handle; buffer is exactly FILE_ATTRIBUTE_TAG_INFO.
    if unsafe { GetFileInformationByHandleEx(h, FILE_ATTRIBUTE_TAG_INFO, tag.as_mut_ptr().cast(), 8) } == 0 {
        return Err(Outcome::Failed(win_err(unsafe { GetLastError() })));
    }
    let (attrs, rtag) = (tag[0], tag[1]);
    let wof = attrs & ATTR_REPARSE != 0 && rtag == IO_REPARSE_TAG_WOF;
    let other_reparse = attrs & ATTR_REPARSE != 0 && !wof;
    let special = ATTR_DIRECTORY | ATTR_OFFLINE | ATTR_ENCRYPTED | ATTR_RECALL_ON_OPEN | ATTR_RECALL_ON_DATA_ACCESS;
    if attrs & special != 0 || other_reparse {
        return Err(Outcome::Skipped);
    }
    // Windows hides its own compression marker from programs, so "is it
    // already compressed?" comes from the scan (size on disk) and, for
    // undo, from Windows' answer (342 = not compressed: skipped).
    if !undo && (wof || attrs & (ATTR_COMPRESSED | ATTR_SPARSE) != 0) {
        return Err(Outcome::Skipped);
    }
    // A file with other names (hardlinks) shares its data with them, and
    // they may live outside the chosen folder (even in Windows): leave it.
    if !undo {
        // FILE_STANDARD_INFO: AllocationSize, EndOfFile (i64), NumberOfLinks (u32), 2 BOOLEANs.
        let mut st = [0u64; 3];
        // SAFETY: valid handle; 24-byte buffer holds FILE_STANDARD_INFO (22 bytes + padding).
        if unsafe { GetFileInformationByHandleEx(h, FILE_STANDARD_INFO, st.as_mut_ptr().cast(), 24) } == 0 {
            return Err(Outcome::Failed(win_err(unsafe { GetLastError() })));
        }
        if (st[2] & 0xFFFF_FFFF) > 1 {
            return Err(Outcome::Skipped);
        }
    }
    match final_path(h) {
        Some(p) if p.starts_with(prefix) => Ok(file),
        _ => Err(Outcome::Skipped), // not where the scan saw it (junction?): leave it
    }
}

fn compress_one(f: &PlanFile, prefix: &str, algorithm: u32) -> Outcome {
    let file = match open_checked(f, prefix, false) {
        Ok(h) => h,
        Err(o) => return o,
    };
    // Random-looking start: almost certainly won't shrink, don't spend the time.
    if f.size >= 64 * 1024 {
        let mut buf = vec![0u8; 64 * 1024];
        let mut got = 0u32;
        // SAFETY: valid handle and buffer.
        if unsafe { ReadFile(file.0, buf.as_mut_ptr(), buf.len() as u32, &mut got, std::ptr::null_mut()) } != 0 && entropy(&buf[..got as usize]) > RANDOM_LOOKING {
            return Outcome::WontShrink;
        }
    }
    // WOF_EXTERNAL_INFO { version 1, provider FILE } + FILE_PROVIDER_EXTERNAL_INFO_V1 { version 1, algorithm, flags 0 }
    let inb: [u32; 5] = [1, 2, 1, algorithm, 0];
    let mut ret = 0u32;
    // SAFETY: valid handle; input buffer of the declared size; no output.
    let ok = unsafe { DeviceIoControl(file.0, FSCTL_SET_EXTERNAL_BACKING, inb.as_ptr().cast(), 20, std::ptr::null_mut(), 0, &mut ret, std::ptr::null_mut()) };
    if ok == 0 {
        return match unsafe { GetLastError() } {
            ERROR_COMPRESSION_NOT_BENEFICIAL => Outcome::WontShrink,
            ERROR_ACCESS_DENIED => Outcome::Denied,
            ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION => Outcome::InUse,
            e => Outcome::Failed(win_err(e)),
        };
    }
    Outcome::Changed(on_disk(&f.path).unwrap_or(f.alloc))
}

fn uncompress_one(f: &PlanFile, prefix: &str) -> Outcome {
    let file = match open_checked(f, prefix, true) {
        Ok(h) => h,
        Err(o) => return o,
    };
    let mut ret = 0u32;
    // SAFETY: valid handle; this control code takes no buffers.
    let ok = unsafe { DeviceIoControl(file.0, FSCTL_DELETE_EXTERNAL_BACKING, std::ptr::null(), 0, std::ptr::null_mut(), 0, &mut ret, std::ptr::null_mut()) };
    if ok == 0 {
        return match unsafe { GetLastError() } {
            ERROR_OBJECT_NOT_EXTERNALLY_BACKED => Outcome::Skipped,
            ERROR_ACCESS_DENIED => Outcome::Denied,
            ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION => Outcome::InUse,
            e => Outcome::Failed(win_err(e)),
        };
    }
    Outcome::Changed(on_disk(&f.path).unwrap_or(f.size))
}

/// Compresses (`level` = Some) or uncompresses (None) every file in `plan`,
/// on `threads` workers. Blocks until done or cancelled: call it from a
/// worker thread and watch `progress`.
pub fn run(plan: &Plan, level: Option<Level>, threads: usize, progress: &Progress) -> Report {
    let t0 = Instant::now();
    let mut report = Report { undo: level.is_none(), level, ..Report::default() };
    if let Err(e) = check_volume(&plan.root) {
        report.first_error = Some(e);
        report.failed = (plan.files.len() as u64, plan.bytes);
        return report;
    }
    let prefix = match root_prefix(&plan.root) {
        Ok(p) => p,
        Err(e) => {
            report.first_error = Some(e);
            report.failed = (plan.files.len() as u64, plan.bytes);
            return report;
        }
    };
    let next = AtomicUsize::new(0);
    let shared = Mutex::new(report);
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                // Below normal: the window and other programs come first.
                // SAFETY: pseudo-handle of the current thread.
                unsafe { SetThreadPriority(GetCurrentThread(), -1) };
                let mut local = Report::default();
                loop {
                    if progress.cancel.load(Ordering::Relaxed) {
                        local.cancelled = true;
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(f) = plan.files.get(i) else { break };
                    let out = match level {
                        Some(l) => compress_one(f, &prefix, l.algorithm()),
                        None => uncompress_one(f, &prefix),
                    };
                    let one = (1, f.alloc);
                    let add = |c: &mut Count| {
                        c.0 += one.0;
                        c.1 += one.1;
                    };
                    match out {
                        Outcome::Changed(after) => {
                            local.changed_files += 1;
                            local.before += f.alloc;
                            local.after += after;
                            local.new_alloc.push((f.node, after));
                            let delta = if level.is_some() { f.alloc.saturating_sub(after) } else { after.saturating_sub(f.alloc) };
                            progress.saved.fetch_add(delta, Ordering::Relaxed);
                        }
                        Outcome::WontShrink => add(&mut local.wont_shrink),
                        Outcome::InUse => add(&mut local.in_use),
                        Outcome::Denied => add(&mut local.denied),
                        Outcome::Skipped => add(&mut local.skipped),
                        Outcome::Failed(e) => {
                            add(&mut local.failed);
                            local.first_error.get_or_insert_with(|| format!("{}: {e}", f.path.display()));
                        }
                    }
                    progress.done.fetch_add(f.alloc, Ordering::Relaxed);
                    progress.files_done.fetch_add(1, Ordering::Relaxed);
                }
                let mut r = shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                merge(&mut r, local);
            });
        }
    });
    let mut report = shared.into_inner().unwrap_or_else(std::sync::PoisonError::into_inner);
    report.secs = t0.elapsed().as_secs_f64();
    report
}

fn merge(r: &mut Report, l: Report) {
    r.changed_files += l.changed_files;
    r.before += l.before;
    r.after += l.after;
    for (a, b) in [(&mut r.wont_shrink, l.wont_shrink), (&mut r.in_use, l.in_use), (&mut r.denied, l.denied), (&mut r.skipped, l.skipped), (&mut r.failed, l.failed)] {
        a.0 += b.0;
        a.1 += b.1;
    }
    if r.first_error.is_none() {
        r.first_error = l.first_error;
    }
    r.cancelled |= l.cancelled;
    r.new_alloc.extend(l.new_alloc);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fnv(p: &Path) -> u64 {
        std::fs::read(p).unwrap().iter().fold(0xcbf2_9ce4_8422_2325_u64, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
    }

    /// A temp folder on the system drive with compressible, random and tiny files.
    fn fixture(tag: &str) -> (PathBuf, Vec<PlanFile>) {
        let dir = std::env::temp_dir().join(format!("xylem-compress-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let text: Vec<u8> = (0..4_000_000u32).flat_map(|i| format!("line {} of a very repetitive log\n", i % 977).into_bytes()).take(4 << 20).collect();
        std::fs::write(dir.join("log.txt"), &text).unwrap();
        std::fs::write(dir.join("sub").join("log2.txt"), &text[..1 << 20]).unwrap();
        let mut rng = 0x9e37_79b9_7f4a_7c15_u64;
        let random: Vec<u8> = (0..(2 << 20))
            .map(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                (rng >> 24) as u8
            })
            .collect();
        std::fs::write(dir.join("noise.bin"), &random).unwrap();
        std::fs::write(dir.join("tiny.txt"), b"hello").unwrap();
        let mut files = Vec::new();
        for (i, rel) in ["log.txt", "sub\\log2.txt", "noise.bin"].iter().enumerate() {
            let p = dir.join(rel);
            let m = std::fs::metadata(&p).unwrap();
            files.push(PlanFile { node: i as u32 + 1, path: p.clone(), size: m.len(), alloc: on_disk(&p).unwrap() });
        }
        (dir, files)
    }

    fn plan(dir: &Path, files: Vec<PlanFile>) -> Plan {
        let bytes = files.iter().map(|f| f.alloc).sum();
        Plan { root: dir.to_path_buf(), files, bytes, ..Plan::default() }
    }

    #[test]
    fn compresses_keeps_data_and_dates_and_undoes() {
        let (dir, files) = fixture("roundtrip");
        if check_volume(&dir).is_err() {
            eprintln!("temp folder isn't on NTFS: skipped");
            return;
        }
        let before: Vec<(u64, std::time::SystemTime)> = files.iter().map(|f| (fnv(&f.path), std::fs::metadata(&f.path).unwrap().modified().unwrap())).collect();
        let p = plan(&dir, files.clone());
        let progress = Progress::default();
        let r = run(&p, Some(Level::Fast), 4, &progress);
        assert_eq!(r.first_error, None);
        assert_eq!(r.changed_files, 2, "{r:?}");
        assert_eq!(r.wont_shrink.0, 1, "random data is left alone: {r:?}");
        assert!(r.after * 4 < r.before, "the logs shrink a lot: {r:?}");
        assert_eq!(progress.files_done.load(Ordering::Relaxed), 3);
        for (f, (h, t)) in files.iter().zip(&before) {
            assert_eq!(fnv(&f.path), *h, "same bytes: {}", f.path.display());
            assert_eq!(std::fs::metadata(&f.path).unwrap().modified().unwrap(), *t, "same date");
        }
        // Undo: only the compressed ones change, and they're full size again.
        let mut up = p.clone();
        for f in &mut up.files {
            f.alloc = on_disk(&f.path).unwrap();
        }
        let u = run(&up, None, 2, &Progress::default());
        assert_eq!(u.changed_files, 2, "{u:?}");
        assert_eq!(u.skipped.0, 1, "noise.bin wasn't compressed");
        for (f, (h, _)) in files.iter().zip(&before) {
            assert_eq!(fnv(&f.path), *h);
            assert!(on_disk(&f.path).unwrap() >= f.size, "restored: {}", f.path.display());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_in_use_are_skipped_and_cancel_stops() {
        let (dir, files) = fixture("inuse");
        if check_volume(&dir).is_err() {
            return;
        }
        let p = plan(&dir, files.clone());
        {
            let mut w = std::fs::OpenOptions::new().append(true).open(&files[0].path).unwrap();
            w.write_all(b"").unwrap();
            let r = run(&p, Some(Level::Fast), 1, &Progress::default());
            assert_eq!(r.in_use.0, 1, "{r:?}");
        }
        let pr = Progress::default();
        pr.cancel.store(true, Ordering::Relaxed);
        let r = run(&p, Some(Level::Max), 2, &pr);
        assert!(r.cancelled);
        assert_eq!(pr.files_done.load(Ordering::Relaxed), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hardlinked_files_are_left_alone() {
        let (dir, files) = fixture("hardlink");
        if check_volume(&dir).is_err() {
            return;
        }
        // A second name for log.txt, as if it lived somewhere else.
        std::fs::hard_link(&files[0].path, dir.join("other-name.txt")).unwrap();
        let h = fnv(&files[0].path);
        let r = run(&plan(&dir, vec![files[0].clone()]), Some(Level::Fast), 1, &Progress::default());
        assert_eq!((r.changed_files, r.skipped.0), (0, 1), "{r:?}");
        assert_eq!(on_disk(&files[0].path).unwrap(), files[0].alloc, "not compressed");
        assert_eq!(fnv(&files[0].path), h);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn files_outside_the_folder_are_refused() {
        let (dir, files) = fixture("outside");
        if check_volume(&dir).is_err() {
            return;
        }
        // The plan says "inside sub\", but the file really lives one level up.
        let mut p = plan(&dir.join("sub"), vec![files[0].clone()]);
        p.root = dir.join("sub");
        let r = run(&p, Some(Level::Fast), 1, &Progress::default());
        assert_eq!((r.changed_files, r.skipped.0), (0, 1), "{r:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn helpers() {
        assert!(entropy(&[7u8; 4096]) < 0.01);
        let all: Vec<u8> = (0..=255u8).cycle().take(256 * 64).collect();
        assert!((entropy(&all) - 8.0).abs() < 1e-9);
        // npz: np.savez stores arrays uncompressed (measured 13.9% saved); np.savez_compressed ones look random and the entropy check skips them.
        assert!(incompressible("JPG") && incompressible("gguf") && !incompressible("npz") && !incompressible("npy"));
        assert_eq!(ratio("zip"), 0.0);
        assert!((1..=16).contains(&default_threads()));
        let p = Plan { estimate_max: 1000, ..Plan::default() };
        assert_eq!(p.estimate(Level::Fast), 850);
        assert!(check_volume(Path::new(r"\\server\share")).is_err());
    }
}
