//! Work-stack folder walker.
//!
//! N worker threads pull folders from a shared stack and list them. A single
//! builder thread (the caller) receives each listing, appends the entries to
//! the [`Index`], and pushes the subfolders it finds. Only the builder ever
//! touches the index, so it needs no locks. Parents are always appended
//! before their children, which is the invariant `Index` relies on.
//!
//! Speed tricks:
//! - Each subfolder is opened *relative to its parent's open handle*
//!   (`NtOpenFile` with `RootDirectory`), so Windows never re-parses the full
//!   path from the drive root.
//! - The stack is LIFO (depth-first), so the number of parent handles held
//!   open at any moment stays small.

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
use std::thread;
use std::time::Instant;

use drishti_core::win::{FILE_ATTRIBUTE_REPARSE_POINT, node_flags};
use drishti_core::{Index, Scan, ScanStats, flags};

use crate::ffi::*;

const BUF_BYTES: usize = 64 * 1024;

// Byte offsets inside FILE_FULL_DIR_INFORMATION (x64 and x86 alike).
const OFF_NEXT: usize = 0;
const OFF_END_OF_FILE: usize = 40;
const OFF_ALLOCATION: usize = 48;
const OFF_ATTRIBUTES: usize = 56;
const OFF_NAME_LEN: usize = 60;
/// Holds the reparse tag when the entry is a reparse point.
const OFF_EA_SIZE: usize = 64;
const OFF_NAME: usize = 68;

const BACKSLASH: u16 = b'\\' as u16;

#[derive(Clone, Debug)]
pub struct ScanOptions {
    /// Number of worker threads listing folders.
    pub threads: usize,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self { threads: default_threads() }
    }
}

/// Default worker count: one per CPU core, capped at 8.
///
/// Measured on a 24-core Core Ultra 9 275HX (8 P-cores + 16 E-cores):
/// 6-8 threads was fastest (3.6 s for 1.87M entries), 12 took 4.0 s and 24
/// took 4.9 s. Past ~8 threads, NTFS lock contention and slower E-cores cost
/// more than the extra parallelism gains.
pub fn default_threads() -> usize {
    thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8)
}

struct Task {
    node: u32,
    /// Open handle of the parent folder; `None` for the scan root.
    parent: Option<Arc<OwnedHandle>>,
    /// Folder name relative to `parent`, or the full NT path (`\??\C:\...`)
    /// for the root. UTF-16, no trailing NUL.
    name: Vec<u16>,
}

/// LIFO task stack shared by the workers.
struct Stack {
    state: Mutex<StackState>,
    ready: Condvar,
}

struct StackState {
    tasks: Vec<Task>,
    closed: bool,
}

impl Stack {
    fn new() -> Self {
        Self { state: Mutex::new(StackState { tasks: Vec::new(), closed: false }), ready: Condvar::new() }
    }

    fn lock(&self) -> MutexGuard<'_, StackState> {
        // Workers catch their own panics, so poisoning can't leave bad state.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn push_all(&self, tasks: impl IntoIterator<Item = Task>) {
        let mut s = self.lock();
        let before = s.tasks.len();
        s.tasks.extend(tasks);
        let added = s.tasks.len() - before;
        drop(s);
        match added {
            0 => {}
            1 => self.ready.notify_one(),
            _ => self.ready.notify_all(),
        }
    }

    /// Blocks until a task is available; `None` once the stack is closed.
    fn pop(&self) -> Option<Task> {
        let mut s = self.lock();
        loop {
            if let Some(t) = s.tasks.pop() {
                return Some(t);
            }
            if s.closed {
                return None;
            }
            s = self.ready.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
    }
}

struct Meta {
    name_len: u16,
    size: u64,
    alloc: u64,
    flags: u8,
}

#[derive(Default)]
struct Listing {
    node: u32,
    metas: Vec<Meta>,
    /// All entry names concatenated; `Meta::name_len` splits them.
    names: String,
    /// (index into `metas`, name) for each subfolder to descend into.
    subdirs: Vec<(u32, Vec<u16>)>,
    /// This folder's handle, kept open so subfolders can open relative to it.
    handle: Option<Arc<OwnedHandle>>,
    unreadable: bool,
}

/// Scans a folder or drive root (e.g. `C:\`) and returns the full tree with
/// folder totals already computed.
pub fn scan(root: &Path, opts: &ScanOptions) -> io::Result<Scan> {
    let start = Instant::now();
    let (display, nt_path) = prepare_root(root)?;
    let mut index = Index::new(&display);
    let mut stats = ScanStats::default();
    let threads = opts.threads.max(1);

    let stack = Stack::new();
    let (res_tx, res_rx) = mpsc::channel::<Listing>();

    thread::scope(|s| {
        for _ in 0..threads {
            let stack = &stack;
            let res_tx = res_tx.clone();
            s.spawn(move || {
                let mut buf = vec![0u64; BUF_BYTES / 8];
                let mut units = Vec::with_capacity(256);
                while let Some(task) = stack.pop() {
                    let node = task.node;
                    // A bug in one folder must not hang the whole scan.
                    let listing =
                        panic::catch_unwind(AssertUnwindSafe(|| list_dir(&task, &mut buf, &mut units)))
                            .unwrap_or_else(|_| Listing { node, unreadable: true, ..Default::default() });
                    if res_tx.send(listing).is_err() {
                        break;
                    }
                }
            });
        }
        drop(res_tx);

        stack.push_all([Task { node: 0, parent: None, name: nt_path }]);
        let mut pending = 1usize;
        while pending > 0 {
            let listing = res_rx.recv().expect("workers never exit while tasks are pending");
            pending -= 1;
            if listing.unreadable {
                index.add_flags(listing.node, flags::UNREADABLE);
                stats.unreadable += 1;
            }
            let base = u32::try_from(index.len()).expect("index fits in u32");
            let mut pos = 0;
            for m in &listing.metas {
                let end = pos + m.name_len as usize;
                index.push(listing.node, &listing.names[pos..end], m.size, m.alloc, m.flags);
                pos = end;
                if m.flags & flags::DIR != 0 {
                    stats.dirs += 1;
                } else {
                    stats.files += 1;
                }
            }
            pending += listing.subdirs.len();
            let parent = listing.handle;
            stack.push_all(
                listing
                    .subdirs
                    .into_iter()
                    .map(|(k, name)| Task { node: base + k, parent: parent.clone(), name }),
            );
        }
        stack.close(); // lets the workers' loops end
    });

    index.compute_totals();
    stats.elapsed = start.elapsed();
    Ok(Scan { index, stats })
}

/// Returns (display path, NT path like `\??\C:\` as UTF-16) for the scan root.
fn prepare_root(root: &Path) -> io::Result<(String, Vec<u16>)> {
    // "C:" on its own means "current folder on C:"; users mean the drive root.
    let raw = root.as_os_str();
    let root: PathBuf = match raw.to_str() {
        Some(s) if s.len() == 2 && s.ends_with(':') => PathBuf::from(format!("{s}\\")),
        _ => root.to_path_buf(),
    };
    let abs = std::path::absolute(&root)?;
    if !std::fs::metadata(&abs)?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a folder or drive: {}", abs.display()),
        ));
    }

    let mut wide: Vec<u16> = abs.as_os_str().encode_wide().collect();
    // Drop trailing backslashes, but keep the one in a drive root like "C:\".
    while wide.len() > 3 && wide.last() == Some(&BACKSLASH) {
        wide.pop();
    }
    let display = String::from_utf16_lossy(&wide);

    let verbatim: Vec<u16> = r"\\?\".encode_utf16().collect();
    let unc: Vec<u16> = r"\\".encode_utf16().collect();
    let mut nt = if wide.starts_with(&verbatim) {
        wide
    } else if wide.starts_with(&unc) {
        let mut p: Vec<u16> = r"\\?\UNC\".encode_utf16().collect();
        p.extend_from_slice(&wide[2..]);
        p
    } else {
        let mut p = verbatim;
        p.extend_from_slice(&wide);
        p
    };
    // Win32 verbatim prefix `\\?\` -> NT object-manager prefix `\??\`.
    nt[1] = u16::from(b'?');
    let display = display.strip_prefix(r"\\?\").map(str::to_owned).unwrap_or(display);
    Ok((display, nt))
}

/// Opens a folder for listing, relative to its parent's handle if it has one.
fn open_dir(task: &Task) -> Option<OwnedHandle> {
    let byte_len = u16::try_from(task.name.len() * 2).ok()?;
    let mut name = UnicodeString {
        length: byte_len,
        maximum_length: byte_len,
        buffer: task.name.as_ptr().cast_mut(),
    };
    let mut attrs = ObjectAttributes {
        length: size_of::<ObjectAttributes>() as u32,
        root_directory: task.parent.as_ref().map_or(null_mut(), |h| h.as_raw_handle()),
        object_name: &mut name,
        attributes: OBJ_CASE_INSENSITIVE,
        security_descriptor: null_mut(),
        security_qos: null_mut(),
    };
    let mut raw: Handle = null_mut();
    let mut iosb = IoStatusBlock::default();
    // SAFETY: all pointers refer to live locals; `task.name` and the parent
    // handle outlive the call. NtOpenFile only reads the name buffer.
    let status = unsafe {
        NtOpenFile(
            &mut raw,
            FILE_LIST_DIRECTORY | SYNCHRONIZE,
            &mut attrs,
            &mut iosb,
            FILE_SHARE_ALL,
            FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_FOR_BACKUP_INTENT,
        )
    };
    if status < 0 || raw.is_null() {
        return None;
    }
    // SAFETY: NtOpenFile succeeded, so `raw` is a handle we now own.
    Some(unsafe { OwnedHandle::from_raw_handle(raw) })
}

fn list_dir(task: &Task, buf: &mut [u64], units: &mut Vec<u16>) -> Listing {
    let mut out = Listing { node: task.node, ..Default::default() };
    let Some(handle) = open_dir(task) else {
        out.unreadable = true;
        return out;
    };

    let buf_bytes = buf.len() * 8;
    let mut restart = true;
    loop {
        let mut iosb = IoStatusBlock::default();
        // SAFETY: the handle is open for listing and was opened without
        // FILE_FLAG_OVERLAPPED, so the call is synchronous. The buffer is
        // writable and `buf_bytes` long.
        let status = unsafe {
            NtQueryDirectoryFile(
                handle.as_raw_handle(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut iosb,
                buf.as_mut_ptr().cast(),
                buf_bytes as u32,
                FILE_FULL_DIRECTORY_INFORMATION,
                0,
                null_mut(),
                u8::from(restart),
            )
        } as u32;
        restart = false;

        if status == STATUS_NO_MORE_FILES || status == STATUS_NO_SUCH_FILE {
            break;
        }
        if status != STATUS_SUCCESS {
            out.unreadable = true;
            break;
        }
        let filled = iosb.information.min(buf_bytes);
        // SAFETY: the first `filled` bytes of `buf` are initialized u64 memory.
        let bytes = unsafe { std::slice::from_raw_parts(buf.as_ptr().cast::<u8>(), filled) };
        if !parse_entries(bytes, units, &mut out) {
            out.unreadable = true;
            break;
        }
    }
    if !out.subdirs.is_empty() {
        out.handle = Some(Arc::new(handle));
    }
    out
}

/// Parses one buffer of FILE_FULL_DIR_INFORMATION records. Returns false
/// if the buffer looks malformed.
fn parse_entries(bytes: &[u8], units: &mut Vec<u16>, out: &mut Listing) -> bool {
    let mut off = 0usize;
    loop {
        let Some(e) = bytes.get(off..) else { return false };
        if e.len() < OFF_NAME {
            return e.is_empty() && off == 0; // an empty buffer is fine
        }
        let next = rd_u32(e, OFF_NEXT) as usize;
        let name_bytes = rd_u32(e, OFF_NAME_LEN) as usize;
        let Some(raw_name) = e.get(OFF_NAME..OFF_NAME + name_bytes) else { return false };
        units.clear();
        units.extend(raw_name.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)));

        if !is_dot_entry(units) {
            let attrs = rd_u32(e, OFF_ATTRIBUTES);
            let tag = if attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0 { rd_u32(e, OFF_EA_SIZE) } else { 0 };
            let f = node_flags(attrs, tag);
            let is_dir = f & flags::DIR != 0;
            let (size, alloc) = if is_dir {
                (0, 0)
            } else {
                (rd_u64(e, OFF_END_OF_FILE), rd_u64(e, OFF_ALLOCATION))
            };

            let start = out.names.len();
            out.names.extend(
                char::decode_utf16(units.iter().copied()).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)),
            );
            let Ok(name_len) = u16::try_from(out.names.len() - start) else { return false };

            if is_dir && f & flags::LINK == 0 {
                out.subdirs.push((out.metas.len() as u32, units.clone()));
            }
            out.metas.push(Meta { name_len, size, alloc, flags: f });
        }

        if next == 0 {
            return true;
        }
        off += next;
    }
}

fn is_dot_entry(name: &[u16]) -> bool {
    const DOT: u16 = b'.' as u16;
    matches!(name, [DOT] | [DOT, DOT])
}

fn rd_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

fn rd_u64(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("8 bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use drishti_core::win::FILE_ATTRIBUTE_DIRECTORY;

    #[test]
    fn parses_a_synthetic_buffer() {
        fn record(name: &str, attrs: u32, eof: u64, alloc: u64, last: bool) -> Vec<u8> {
            let units: Vec<u16> = name.encode_utf16().collect();
            let len = (OFF_NAME + units.len() * 2).next_multiple_of(8);
            let mut r = vec![0u8; len];
            let next = if last { 0 } else { len as u32 };
            r[OFF_NEXT..OFF_NEXT + 4].copy_from_slice(&next.to_le_bytes());
            r[OFF_END_OF_FILE..OFF_END_OF_FILE + 8].copy_from_slice(&eof.to_le_bytes());
            r[OFF_ALLOCATION..OFF_ALLOCATION + 8].copy_from_slice(&alloc.to_le_bytes());
            r[OFF_ATTRIBUTES..OFF_ATTRIBUTES + 4].copy_from_slice(&attrs.to_le_bytes());
            r[OFF_NAME_LEN..OFF_NAME_LEN + 4].copy_from_slice(&((units.len() * 2) as u32).to_le_bytes());
            for (i, u) in units.iter().enumerate() {
                r[OFF_NAME + i * 2..OFF_NAME + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
            }
            r
        }
        let mut buf = Vec::new();
        buf.extend(record(".", FILE_ATTRIBUTE_DIRECTORY, 0, 0, false));
        buf.extend(record("..", FILE_ATTRIBUTE_DIRECTORY, 0, 0, false));
        buf.extend(record("नमस्ते.txt", 0, 1234, 4096, false));
        buf.extend(record("sub", FILE_ATTRIBUTE_DIRECTORY, 0, 0, true));

        let mut out = Listing::default();
        assert!(parse_entries(&buf, &mut Vec::new(), &mut out));
        assert_eq!(out.metas.len(), 2);
        assert_eq!(out.names, "नमस्ते.txtsub");
        assert_eq!(out.metas[0].size, 1234);
        assert_eq!(out.metas[0].alloc, 4096);
        assert_eq!(out.subdirs.len(), 1);
        assert_eq!(String::from_utf16_lossy(&out.subdirs[0].1), "sub");

        // Truncated buffer is rejected, not read out of bounds.
        let mut out = Listing::default();
        assert!(!parse_entries(&buf[..buf.len() - 10], &mut Vec::new(), &mut out));
    }
}
