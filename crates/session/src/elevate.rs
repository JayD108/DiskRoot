//! Starts `xylem.exe --helper-scan` elevated (UAC prompt) to do a fast MFT
//! scan, then loads the index file it writes. The UI never runs as admin.
//! The helper side is in `helper.rs`.

use std::ffi::c_void;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::ptr::null_mut;

use xylem_core::{Index, ScanStats};

const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;
const SW_HIDE: i32 = 0;
const INFINITE: u32 = 0xFFFF_FFFF;
const ERROR_CANCELLED: i32 = 1223;

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
    fn CloseHandle(handle: *mut c_void) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `xylem.exe` next to the app.
///
/// Security: this exe is launched with admin rights, so its location must
/// not be redirectable. The `XYLEM_HELPER` override exists only in debug
/// builds. (An installer that puts the app in Program Files, where normal
/// users can't replace files, plus code signing, closes the remaining gap.)
fn helper_path() -> Result<PathBuf, String> {
    #[cfg(debug_assertions)]
    if let Some(p) = std::env::var_os("XYLEM_HELPER") {
        return Ok(p.into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let p = exe.with_file_name("xylem.exe");
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

pub fn scan_with_helper(drive: &str) -> Result<(Index, ScanStats, String), String> {
    let drive = normalize_drive(drive)?;
    let helper = helper_path()?;
    // Unpredictable name. The helper refuses to overwrite (create_new), so a
    // planted file or link at this path makes the scan fail safely instead
    // of letting an admin process overwrite something.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let out = std::env::temp_dir().join(format!("xylem-{}-{nanos:x}.idx", std::process::id()));
    let params = format!("--helper-scan {drive} --save-index \"{}\"", out.display());

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
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let e = std::io::Error::last_os_error();
        return Err(if e.raw_os_error() == Some(ERROR_CANCELLED) {
            "Admin permission was declined, so the fast scan didn't run.".into()
        } else {
            format!("Could not start the admin helper: {e}")
        });
    }
    let mut code = 1u32;
    if !info.h_process.is_null() {
        // SAFETY: h_process is a valid process handle we own (NOCLOSEPROCESS).
        unsafe {
            WaitForSingleObject(info.h_process, INFINITE);
            GetExitCodeProcess(info.h_process, &mut code);
            CloseHandle(info.h_process);
        }
    }
    let loaded = (|| {
        if code != 0 {
            return Err(match code {
                c if c == crate::helper::SCAN_FAILED as u32 => format!("Fast scan couldn't read {drive}. Is it an NTFS drive?"),
                c if c == crate::helper::WRITE_FAILED as u32 => "Fast scan couldn't write its result file.".into(),
                c => format!("The admin helper failed (exit code {c})."),
            });
        }
        let f = File::open(&out).map_err(|e| format!("Helper produced no result: {e}"))?;
        let len = f.metadata().map_err(|e| e.to_string())?.len();
        let (index, stats, mode) =
            Index::load_checked(&mut BufReader::with_capacity(1 << 20, f), len).map_err(|e| e.to_string())?;
        Ok((index, stats, format!("{mode} via admin helper")))
    })();
    let _ = std::fs::remove_file(&out);
    loaded
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
}
