//! The elevated scan helper, built into `xylem.exe`.
//!
//! When the app needs a fast MFT scan it starts itself through the UAC
//! prompt as `xylem.exe --helper-scan C: --pipe \\.\pipe\xylem-...`. In that
//! mode no window is created: it scans and sends the index through the
//! app's private pipe, then exits.
//!
//! This code runs with admin rights, so it:
//! - only accepts exactly that command line;
//! - opens the drive read-only;
//! - **writes nothing to disk** (no temp file; see `elevate.rs` for why);
//! - connects to the pipe with `SECURITY_IDENTIFICATION`, so even a fake
//!   pipe server could never act with the helper's admin rights.

use std::io::{BufWriter, Write};

use xylem_core::{Index, ScanStats};
use xylem_scan_mft::scan_drive;

/// Exit codes of the helper process.
pub const OK: i32 = 0;
pub const BAD_ARGS: i32 = 2;
pub const SCAN_FAILED: i32 = 3;
pub const WRITE_FAILED: i32 = 4;

/// If the process was started in helper mode, runs the helper and returns
/// its exit code; otherwise returns None and the app starts normally.
pub fn run_if_helper(args: &[String]) -> Option<i32> {
    if args.get(1).map(String::as_str) != Some("--helper-scan") {
        return None;
    }
    let code = match (args.get(2), args.get(3).map(String::as_str), args.get(4), args.len()) {
        (Some(drive), Some("--pipe"), Some(pipe), 5) => run(drive, pipe),
        _ => BAD_ARGS,
    };
    Some(code)
}

fn run(drive: &str, pipe: &str) -> i32 {
    let Some(letter) = drive_letter(drive) else { return BAD_ARGS };
    if check_pipe_name(pipe).is_err() {
        return BAD_ARGS;
    }
    let scan = match scan_drive(letter, xylem_scan_walk::default_threads()) {
        Ok(s) => s,
        Err(_) => return SCAN_FAILED,
    };
    match send_to_pipe(pipe, &scan.index, &scan.stats, "MFT (direct read)") {
        Ok(()) => OK,
        Err(_) => WRITE_FAILED,
    }
}

/// "C:" or "C:\" -> 'C'.
fn drive_letter(s: &str) -> Option<char> {
    let mut c = s.chars();
    let l = c.next().filter(char::is_ascii_alphabetic)?;
    matches!(c.as_str(), ":" | ":\\").then_some(l.to_ascii_uppercase())
}

/// Only a local pipe named like the app makes them:
/// `\\.\pipe\xylem-<digits/hex/dashes>`.
pub fn check_pipe_name(name: &str) -> Result<(), String> {
    let rest = name.strip_prefix(r"\\.\pipe\xylem-").ok_or("not a Xylem pipe")?;
    let ok = !rest.is_empty() && rest.len() <= 80 && rest.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
    if ok { Ok(()) } else { Err("bad pipe name".into()) }
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *mut std::ffi::c_void,
        disposition: u32,
        flags: u32,
        template: *mut std::ffi::c_void,
    ) -> *mut std::ffi::c_void;
}

/// Writes the index into the app's pipe. Opened with SECURITY_SQOS_PRESENT |
/// SECURITY_IDENTIFICATION: the server can see who connected, but can't
/// impersonate (act as) this admin process.
pub fn send_to_pipe(pipe: &str, index: &Index, stats: &ScanStats, mode: &str) -> std::io::Result<()> {
    use std::os::windows::io::FromRawHandle;
    const GENERIC_WRITE: u32 = 0x4000_0000;
    const OPEN_EXISTING: u32 = 3;
    const SECURITY_SQOS_PRESENT: u32 = 0x0010_0000;
    const SECURITY_IDENTIFICATION: u32 = 0x0001_0000;
    check_pipe_name(pipe).map_err(std::io::Error::other)?;
    let w: Vec<u16> = pipe.encode_utf16().chain([0]).collect();
    // SAFETY: NUL-terminated name; returns an owned handle or INVALID.
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            GENERIC_WRITE,
            0,
            std::ptr::null_mut(),
            OPEN_EXISTING,
            SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            std::ptr::null_mut(),
        )
    };
    if h.is_null() || h == -1isize as *mut std::ffi::c_void {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: we own the handle; File closes it.
    let f = unsafe { std::fs::File::from_raw_handle(h) };
    let mut out = BufWriter::with_capacity(1 << 20, f);
    index.save(stats, mode, &mut out)?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn pipe_names_are_restricted() {
        assert!(check_pipe_name(r"\\.\pipe\xylem-123-abc-0f").is_ok());
        assert!(check_pipe_name(r"\\.\pipe\other").is_err());
        assert!(check_pipe_name(r"\\server\pipe\xylem-1").is_err(), "remote");
        assert!(check_pipe_name(r"\\.\pipe\xylem-1\..\x").is_err());
        assert!(check_pipe_name(r"C:\Windows\xylem-1.idx").is_err(), "files are no longer accepted");
        assert!(check_pipe_name(r"\\.\pipe\xylem-").is_err());
    }

    #[test]
    fn only_the_exact_helper_command_line_is_accepted() {
        assert_eq!(run_if_helper(&args(&["xylem.exe"])), None, "normal start");
        assert_eq!(run_if_helper(&args(&["xylem.exe", "C:\\Users"])), None, "open a folder");
        assert_eq!(run_if_helper(&args(&["x", "--helper-scan"])), Some(BAD_ARGS));
        assert_eq!(run_if_helper(&args(&["x", "--helper-scan", "C:", "--out", "f"])), Some(BAD_ARGS));
        assert_eq!(run_if_helper(&args(&["x", "--helper-scan", "C:", "--pipe", "f", "extra"])), Some(BAD_ARGS));
        assert_eq!(run_if_helper(&args(&["x", "--helper-scan", "C:", "--save-index", r"C:\x\xylem-1.idx"])), Some(BAD_ARGS), "old file mode is gone");
        // Wrong drive syntax or a path outside temp: refused before any scan.
        assert_eq!(run_if_helper(&args(&["x", "--helper-scan", "C:\\Users", "--pipe", r"\\.\pipe\xylem-1"])), Some(BAD_ARGS));
        assert_eq!(
            run_if_helper(&args(&["x", "--helper-scan", "C:", "--pipe", r"\\evil\pipe\xylem-1"])),
            Some(BAD_ARGS)
        );
        assert_eq!(drive_letter("d:\\"), Some('D'));
        assert_eq!(drive_letter("1:"), None);
    }
}
