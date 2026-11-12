//! Starts `diskroot.exe --helper-scan` elevated (UAC prompt) to do a fast MFT
//! scan, and receives the result through a private named pipe. The UI never
//! runs as admin. The helper side is in `helper.rs`.
//!
//! Why a pipe and not a file: the elevated helper then writes nothing to
//! disk at all. An earlier version wrote `%TEMP%\diskroot-*.idx`, and the
//! user's TEMP is under the user's control (it can be redirected or swapped
//! for a junction), which is a poor place for an admin process to create
//! files.
//!
//! Pipe safety:
//! - The app creates the pipe with a random name *before* launching the
//!   helper, with `FILE_FLAG_FIRST_PIPE_INSTANCE` (nobody can have created it
//!   first) and `PIPE_REJECT_REMOTE_CLIENTS`. Its default DACL lets only the
//!   creating user, administrators and SYSTEM write to it.
//! - The helper connects with `SECURITY_IDENTIFICATION`, so the pipe server
//!   can never impersonate the helper's admin token.
//! - Whatever arrives is validated by the same length-checked, fuzz-tested
//!   loader as before.

use std::ffi::c_void;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use diskroot_core::{Index, ScanStats};

const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;
const SW_HIDE: i32 = 0;
const ERROR_CANCELLED: i32 = 1223;
const WAIT_TIMEOUT: u32 = 0x102;
const PIPE_ACCESS_INBOUND: u32 = 0x1;
const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x0008_0000;
const PIPE_TYPE_BYTE: u32 = 0;
const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x8;
const ERROR_PIPE_CONNECTED: i32 = 535;
const GENERIC_WRITE: u32 = 0x4000_0000;
const OPEN_EXISTING: u32 = 3;
/// A whole-drive scan normally takes seconds; this only stops a helper
/// that hangs (for example on a crafted drive).
pub const HELPER_TIME_LIMIT: Duration = Duration::from_secs(10 * 60);

#[repr(C)]
struct ShellExecuteInfoW {
    cb_size: u32,
    f_mask: u32,
    hwnd: *mut c_void,
    lp_verb: *const u16,
    lp_file: *const u16,
    lp_parameters: *const u16,
    lp_directory: *const u16,
    n_show: i32,
    h_inst_app: *mut c_void,
    lp_id_list: *mut c_void,
    lp_class: *const u16,
    hkey_class: *mut c_void,
    dw_hot_key: u32,
    h_icon_or_monitor: *mut c_void,
    h_process: *mut c_void,
}

#[link(name = "shell32", kind = "raw-dylib")]
unsafe extern "system" {
    fn ShellExecuteExW(info: *mut ShellExecuteInfoW) -> i32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn WaitForSingleObject(handle: *mut c_void, ms: u32) -> u32;
    fn GetExitCodeProcess(handle: *mut c_void, code: *mut u32) -> i32;
    fn TerminateProcess(handle: *mut c_void, code: u32) -> i32;
    fn CloseHandle(handle: *mut c_void) -> i32;
    fn CreateNamedPipeW(
        name: *const u16,
        open_mode: u32,
        pipe_mode: u32,
        max_instances: u32,
        out_size: u32,
        in_size: u32,
        timeout_ms: u32,
        security: *mut c_void,
    ) -> *mut c_void;
    fn ConnectNamedPipe(pipe: *mut c_void, overlapped: *mut c_void) -> i32;
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut c_void,
        disposition: u32,
        flags: u32,
        template: *mut c_void,
    ) -> *mut c_void;
}

const INVALID_HANDLE: *mut c_void = -1isize as *mut c_void;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// An owned Win32 handle (closed on drop). Raw handles are just numbers, so
/// sending one to the reader thread is fine.
struct Handle(*mut c_void);
unsafe impl Send for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: we own this handle and close it exactly once.
        unsafe { CloseHandle(self.0) };
    }
}

/// `diskroot.exe` next to the app.
///
/// Security: this exe is launched with admin rights, so its location must
/// not be redirectable. The `DISKROOT_HELPER` override exists only in debug
/// builds, and release builds refuse to elevate outside Program Files
/// (see `integrity::fast_scan_allowed`).
fn helper_path() -> Result<std::path::PathBuf, String> {
    #[cfg(debug_assertions)]
    if let Some(p) = std::env::var_os("DISKROOT_HELPER") {
        return Ok(p.into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let p = exe.with_file_name("diskroot.exe");
    if p.exists() { Ok(p) } else { Err(format!("Helper not found: {}", p.display())) }
}

/// Accepts "C", "C:" or "C:\" and returns "C:".
fn normalize_drive(drive: &str) -> Result<String, String> {
    let letter = drive.chars().next().filter(char::is_ascii_alphabetic);
    let rest = drive.get(1..).unwrap_or("");
    match (letter, rest) {
        (Some(l), "" | ":" | ":\\" | ":/") => Ok(format!("{}:", l.to_ascii_uppercase())),
        _ => Err(format!("Fast scan works on whole drives only, not \"{drive}\"")),
    }
}

/// A fresh, unguessable pipe name: `\\.\pipe\diskroot-<pid>-<nanos>-<random>`.
pub fn new_pipe_name() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    // RandomState is seeded from the OS random generator.
    let rnd = {
        use std::hash::{BuildHasher, Hasher};
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(nanos);
        h.finish()
    };
    format!(r"\\.\pipe\diskroot-{}-{nanos:x}-{rnd:016x}", std::process::id())
}

/// Creates the pipe (first and only instance) and returns its handle.
fn create_pipe(name: &str) -> Result<Handle, String> {
    let w = wide(name);
    // SAFETY: NUL-terminated name; default security (creator, admins, SYSTEM).
    let h = unsafe {
        CreateNamedPipeW(
            w.as_ptr(),
            PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            0,
            1 << 20,
            0,
            null_mut(),
        )
    };
    if h == INVALID_HANDLE || h.is_null() {
        return Err(format!("Couldn't set up the connection to the admin helper: {}", std::io::Error::last_os_error()));
    }
    Ok(Handle(h))
}

/// Reads everything the helper sends (it connects, writes the index, exits).
fn read_pipe(pipe: Handle) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    use std::os::windows::io::FromRawHandle;
    // SAFETY: valid pipe handle owned by `pipe`.
    // ERROR_PIPE_CONNECTED: the helper connected before we started waiting.
    // ERROR_NO_DATA: it even finished writing and closed; what it wrote is
    // still in the pipe, so read it.
    let ok = unsafe { ConnectNamedPipe(pipe.0, null_mut()) } != 0
        || matches!(std::io::Error::last_os_error().raw_os_error(), Some(ERROR_PIPE_CONNECTED | 232));
    if !ok {
        return Err(std::io::Error::last_os_error());
    }
    let raw = pipe.0;
    std::mem::forget(pipe); // ownership moves to the File below
    // SAFETY: we own the handle and hand it to File, which closes it.
    let mut f = unsafe { std::fs::File::from_raw_handle(raw) };
    let mut buf = Vec::new();
    match f.read_to_end(&mut buf) {
        Ok(_) => Ok(buf),
        // ERROR_BROKEN_PIPE (109) = the writer closed: that's the normal end.
        Err(e) if e.raw_os_error() == Some(109) => Ok(buf),
        Err(e) => Err(e),
    }
}

/// Unblocks a reader still waiting in ConnectNamedPipe (helper never came).
fn wake_reader(name: &str) {
    let w = wide(name);
    // SAFETY: plain CreateFileW on our own pipe name; handle closed at once.
    let h = unsafe { CreateFileW(w.as_ptr(), GENERIC_WRITE, 0, null_mut(), OPEN_EXISTING, 0, null_mut()) };
    if h != INVALID_HANDLE && !h.is_null() {
        // SAFETY: valid handle we just opened.
        unsafe { CloseHandle(h) };
    }
}

pub fn scan_with_helper(drive: &str, cancel: &AtomicBool) -> Result<(Index, ScanStats, String), String> {
    let drive = normalize_drive(drive)?;
    let helper = helper_path()?;
    let pipe_name = new_pipe_name();
    let pipe = create_pipe(&pipe_name)?;
    let reader = std::thread::spawn(move || read_pipe(pipe));

    let params = format!("--helper-scan {drive} --pipe {pipe_name}");
    let (verb, file, args) = (wide("runas"), wide(&helper.to_string_lossy()), wide(&params));
    let mut info = ShellExecuteInfoW {
        cb_size: size_of::<ShellExecuteInfoW>() as u32,
        f_mask: SEE_MASK_NOCLOSEPROCESS,
        hwnd: null_mut(),
        lp_verb: verb.as_ptr(),
        lp_file: file.as_ptr(),
        lp_parameters: args.as_ptr(),
        lp_directory: std::ptr::null(),
        n_show: SW_HIDE,
        h_inst_app: null_mut(),
        lp_id_list: null_mut(),
        lp_class: std::ptr::null(),
        hkey_class: null_mut(),
        dw_hot_key: 0,
        h_icon_or_monitor: null_mut(),
        h_process: null_mut(),
    };
    // SAFETY: `info` and the wide strings it points to live across the call.
    let launched = unsafe { ShellExecuteExW(&mut info) } != 0;
    let launch_err = std::io::Error::last_os_error();
    if !launched {
        wake_reader(&pipe_name);
        let _ = reader.join();
        return Err(if launch_err.raw_os_error() == Some(ERROR_CANCELLED) {
            "Admin permission was declined, so the fast scan didn't run.".into()
        } else {
            format!("Could not start the admin helper: {launch_err}")
        });
    }

    // Wait for the helper in short slices so Cancel and the time limit work.
    let mut code = 1u32;
    let proc = (!info.h_process.is_null()).then_some(Handle(info.h_process));
    if let Some(p) = &proc {
        let started = Instant::now();
        loop {
            // SAFETY: valid process handle owned by `proc`.
            let w = unsafe { WaitForSingleObject(p.0, 200) };
            if w != WAIT_TIMEOUT {
                // SAFETY: as above.
                unsafe { GetExitCodeProcess(p.0, &mut code) };
                break;
            }
            let stop = if cancel.load(Ordering::Relaxed) {
                Some("Fast scan was cancelled.".to_owned())
            } else if started.elapsed() > HELPER_TIME_LIMIT {
                Some(format!("The admin helper took longer than {} minutes, so it was stopped.", HELPER_TIME_LIMIT.as_secs() / 60))
            } else {
                None
            };
            if let Some(msg) = stop {
                // SAFETY: as above. May fail if Windows didn't give us
                // terminate rights on the elevated process; we stop waiting
                // either way, and the helper only reads.
                unsafe { TerminateProcess(p.0, 1) };
                wake_reader(&pipe_name);
                drop(reader);
                return Err(msg);
            }
        }
    }
    drop(proc);
    wake_reader(&pipe_name);
    let data = reader.join().map_err(|_| "The connection to the admin helper failed.".to_owned())?;
    if code != 0 {
        return Err(match code {
            c if c == crate::helper::SCAN_FAILED as u32 => format!("Fast scan couldn't read {drive}. Is it an NTFS drive?"),
            c if c == crate::helper::WRITE_FAILED as u32 => "Fast scan couldn't send its result back to the app.".into(),
            c if c == crate::helper::LOCKDOWN_FAILED as u32 => {
                "The admin helper couldn't drop its admin rights before reading the drive, so it stopped for safety.".into()
            }
            c => format!("The admin helper failed (exit code {c})."),
        });
    }
    let data = data.map_err(|e| format!("Couldn't read the admin helper's result: {e}"))?;
    let (index, stats, mode) =
        Index::load_checked(&mut data.as_slice(), data.len() as u64).map_err(|e| format!("The admin helper's result was invalid: {e}"))?;
    Ok((index, stats, format!("{mode} via admin helper")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_drives() {
        assert_eq!(normalize_drive("c").unwrap(), "C:");
        assert_eq!(normalize_drive("D:\\").unwrap(), "D:");
        assert!(normalize_drive("C:\\Users").is_err());
        assert!(normalize_drive("").is_err());
    }

    #[test]
    fn pipe_names_are_unique_and_valid() {
        let a = new_pipe_name();
        let b = new_pipe_name();
        assert_ne!(a, b);
        assert!(crate::helper::check_pipe_name(&a).is_ok(), "{a}");
    }

    /// The whole handoff without elevation: a stand-in "helper" writes an
    /// index into the pipe; the reader gets exactly that, and a second
    /// pipe with the same name can't be created (no squatting).
    #[test]
    fn pipe_round_trip_and_first_instance() {
        let name = new_pipe_name();
        let pipe = create_pipe(&name).unwrap();
        assert!(create_pipe(&name).is_err(), "first-instance pipe can't be duplicated");
        let reader = std::thread::spawn(move || read_pipe(pipe));
        let mut idx = Index::new("C:\\");
        idx.push(0, "a.bin", 10, 4096, 0);
        idx.compute_totals();
        crate::helper::send_to_pipe(&name, &idx, &ScanStats::default(), "MFT (direct read)").unwrap();
        let data = reader.join().unwrap().unwrap();
        let (back, _, mode) = Index::load_checked(&mut data.as_slice(), data.len() as u64).unwrap();
        assert_eq!((back.len(), back.alloc(0), mode.as_str()), (2, 4096, "MFT (direct read)"));
    }
}
