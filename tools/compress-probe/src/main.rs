//! compress-probe: measures compression on COPIES of real files.
//!
//! The source files are only read (copied into the scratch folder); every
//! experiment runs on the copy, which is deleted at the end.
//!
//! For each source file (copied into a scratch folder first; the original
//! is only read):
//! - every WOF algorithm (XPRESS4K/8K/16K, LZX): single-thread time, size on
//!   disk, data identical afterwards (FNV-64), undo;
//! - in-memory baselines a future non-transparent engine could use: zstd
//!   1/3/9 (19 on a 64 MB slice), lz4;
//! - a sampled predictor: N chunks of 16 KB spread over the file, each
//!   compressed alone with XPRESS-Huffman (what WOF XPRESS16K does per
//!   chunk), vs the real XPRESS16K result; and its time.
//!
//! Usage: cargo run --release -- <scratch dir on an NTFS drive> <file>...
//! Prints one row per file. Needs no admin. See CONTRIBUTING.md.

use std::ffi::c_void;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

type Handle = *mut c_void;
const INVALID: Handle = -1isize as Handle;
const GENERIC_READ: u32 = 0x8000_0000;
const FILE_SHARE_READ: u32 = 1;
const OPEN_EXISTING: u32 = 3;
const FSCTL_SET_EXTERNAL_BACKING: u32 = 0x0009_030C;
const FSCTL_DELETE_EXTERNAL_BACKING: u32 = 0x0009_0314;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateFileW(name: *const u16, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, tmpl: Handle) -> Handle;
    fn DeviceIoControl(h: Handle, code: u32, inb: *const c_void, inl: u32, outb: *mut c_void, outl: u32, ret: *mut u32, ov: *mut c_void) -> i32;
    fn CloseHandle(h: Handle) -> i32;
    fn GetLastError() -> u32;
    fn GetCompressedFileSizeW(name: *const u16, hi: *mut u32) -> u32;
}
#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    fn RtlGetCompressionWorkSpaceSize(fmt: u16, ws: *mut u32, frag: *mut u32) -> i32;
    fn RtlCompressBuffer(fmt: u16, src: *const u8, srcl: u32, dst: *mut u8, dstl: u32, chunk: u32, finl: *mut u32, ws: *mut c_void) -> i32;
}

#[repr(C)]
struct SetBacking {
    wof_version: u32,
    wof_provider: u32,
    file_version: u32,
    algorithm: u32,
    flags: u32,
}

fn wide(p: &Path) -> Vec<u16> {
    p.as_os_str().encode_wide().chain([0]).collect()
}

fn ioctl(p: &Path, code: u32, algorithm: Option<u32>) -> Result<(), u32> {
    let w = wide(p);
    // SAFETY: NUL-terminated path; handle checked.
    let h = unsafe { CreateFileW(w.as_ptr(), GENERIC_READ, FILE_SHARE_READ, std::ptr::null_mut(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
    if h == INVALID {
        return Err(unsafe { GetLastError() });
    }
    let mut ret = 0u32;
    let ok = match algorithm {
        Some(a) => {
            let inb = SetBacking { wof_version: 1, wof_provider: 2, file_version: 1, algorithm: a, flags: 0 };
            // SAFETY: valid handle and input buffer.
            unsafe { DeviceIoControl(h, code, (&raw const inb).cast(), size_of::<SetBacking>() as u32, std::ptr::null_mut(), 0, &mut ret, std::ptr::null_mut()) }
        }
        // SAFETY: valid handle, no buffers.
        None => unsafe { DeviceIoControl(h, code, std::ptr::null(), 0, std::ptr::null_mut(), 0, &mut ret, std::ptr::null_mut()) },
    };
    let err = if ok == 0 { Some(unsafe { GetLastError() }) } else { None };
    unsafe { CloseHandle(h) };
    err.map_or(Ok(()), Err)
}

fn on_disk(p: &Path) -> u64 {
    let w = wide(p);
    let mut hi = 0u32;
    // SAFETY: NUL-terminated path.
    let lo = unsafe { GetCompressedFileSizeW(w.as_ptr(), &mut hi) };
    (u64::from(hi) << 32) | u64::from(lo)
}

fn fnv(p: &Path) -> u64 {
    let mut f = fs::File::open(p).unwrap();
    let mut buf = vec![0u8; 1 << 20];
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    loop {
        let n = f.read(&mut buf).unwrap();
        if n == 0 {
            return h;
        }
        for &b in &buf[..n] {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// Predicted XPRESS16K saving from `n` 16 KB chunks spread evenly, each
/// compressed alone with XPRESS-Huffman and rounded to 4 KB like WOF stores
/// it (a chunk that doesn't shrink is stored raw).
fn predict(p: &Path, n: u64) -> f64 {
    let fmt: u16 = 4; // COMPRESSION_FORMAT_XPRESS_HUFF, standard engine
    let (mut ws, mut frag) = (0u32, 0u32);
    unsafe { RtlGetCompressionWorkSpaceSize(fmt, &mut ws, &mut frag) };
    let mut work = vec![0u8; ws as usize];
    let len = fs::metadata(p).unwrap().len();
    let mut f = fs::File::open(p).unwrap();
    let chunks = len.div_ceil(16384).max(1);
    let n = n.min(chunks);
    let (mut raw, mut packed) = (0u64, 0u64);
    let mut src = vec![0u8; 16384];
    let mut dst = vec![0u8; 16384 + 4096];
    for i in 0..n {
        let c = i * chunks / n;
        f.seek(SeekFrom::Start(c * 16384)).unwrap();
        let got = f.read(&mut src).unwrap();
        if got == 0 {
            continue;
        }
        let mut out = 0u32;
        // SAFETY: buffers sized as declared; workspace from the size query.
        let st = unsafe { RtlCompressBuffer(fmt, src.as_ptr(), got as u32, dst.as_mut_ptr(), dst.len() as u32, 4096, &mut out, work.as_mut_ptr().cast()) };
        // WOF stores compressed chunks back to back (no per-chunk rounding);
        // a chunk that doesn't shrink is stored raw.
        raw += got as u64;
        packed += if st == 0 && (out as usize) < got { u64::from(out) } else { got as u64 };
    }
    if raw == 0 { 0.0 } else { 1.0 - packed as f64 / raw as f64 }
}

fn pct(saved: u64, of: u64) -> f64 {
    if of == 0 { 0.0 } else { 100.0 * saved as f64 / of as f64 }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: compress-probe <scratch dir on an NTFS drive> <file>...");
        std::process::exit(2);
    }
    let scratch = PathBuf::from(&args[0]);
    for f in &args[1..] {
        assert!(!Path::new(f).starts_with(&scratch), "source files must be outside the scratch folder");
    }
    fs::create_dir_all(&scratch).unwrap();
    println!("compress-probe ({} files). WOF single-thread; MB/s of input.", args.len() - 1);
    println!(
        "{:<12} {:>7} | {:>14} {:>14} {:>14} {:>14} | {:>12} {:>12} {:>12} {:>12} {:>10} | {:>8} {:>8} {:>8}",
        "type", "MB", "XPRESS4K", "XPRESS8K", "XPRESS16K", "LZX", "zstd1", "zstd3", "zstd9", "zstd19*", "lz4", "pred16", "pred64", "pred256"
    );
    for src in &args[1..] {
        let src = Path::new(src);
        let ext = src.extension().map_or("?".into(), |e| e.to_string_lossy().to_lowercase());
        let copy = scratch.join(format!("sample.{ext}"));
        fs::copy(src, &copy).unwrap();
        let len = fs::metadata(&copy).unwrap().len();
        let full = len.div_ceil(4096) * 4096;
        let h0 = fnv(&copy);
        let mb = len as f64 / 1e6;
        let mut cells = Vec::new();
        let mut x16 = 0.0;
        for (alg, _name) in [(0u32, "XPRESS4K"), (2, "XPRESS8K"), (3, "XPRESS16K"), (1, "LZX")] {
            let t = Instant::now();
            let r = ioctl(&copy, FSCTL_SET_EXTERNAL_BACKING, Some(alg));
            let secs = t.elapsed().as_secs_f64();
            let d = on_disk(&copy);
            let same = fnv(&copy) == h0;
            let saved = pct(full.saturating_sub(d), full);
            if alg == 3 {
                x16 = saved;
            }
            cells.push(match r {
                Ok(()) => format!("{saved:5.1}% {:>4.0}MB/s{}", mb / secs, if same { "" } else { "!" }),
                Err(344) => "won't shrink".into(),
                Err(e) => format!("err {e}"),
            });
            let _ = ioctl(&copy, FSCTL_DELETE_EXTERNAL_BACKING, None);
            assert!(on_disk(&copy) >= len.min(full.saturating_sub(4096)), "undo restored full size");
        }
        assert_eq!(fnv(&copy), h0, "data identical after all runs");
        let data = fs::read(&copy).unwrap();
        for lvl in [1, 3, 9] {
            let t = Instant::now();
            let out = zstd::bulk::compress(&data, lvl).unwrap();
            let secs = t.elapsed().as_secs_f64();
            cells.push(format!("{:4.1}% {:>4.0}", pct(len - out.len() as u64, len), mb / secs));
        }
        let slice = &data[..data.len().min(64 << 20)];
        let t = Instant::now();
        let out = zstd::bulk::compress(slice, 19).unwrap();
        cells.push(format!("{:4.1}% {:>4.1}", pct(slice.len() as u64 - out.len() as u64, slice.len() as u64), slice.len() as f64 / 1e6 / t.elapsed().as_secs_f64()));
        let t = Instant::now();
        let out = lz4_flex::compress(&data);
        cells.push(format!("{:4.1}% {:>3.0}", pct(len.saturating_sub(out.len() as u64), len), mb / t.elapsed().as_secs_f64()));
        drop(data);
        let mut preds = Vec::new();
        for n in [16u64, 64, 256] {
            let t = Instant::now();
            let p = predict(&copy, n) * 100.0;
            preds.push(format!("{p:4.1}/{:.0}ms", t.elapsed().as_secs_f64() * 1000.0));
        }
        println!(
            "{:<12} {:>7.0} | {:>14} {:>14} {:>14} {:>14} | {:>12} {:>12} {:>12} {:>12} {:>10} | {:>8} {:>8} {:>8}   (real X16 {x16:.1})",
            ext, mb, cells[0], cells[1], cells[2], cells[3], cells[4], cells[5], cells[6], cells[7], cells[8], preds[0], preds[1], preds[2]
        );
        fs::remove_file(&copy).unwrap();
    }
    let _ = fs::remove_dir(&scratch);
    println!("* zstd19 on the first 64 MB only. '!' = data differed (never expected).");
}
