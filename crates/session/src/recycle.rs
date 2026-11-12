//! Moves a file or folder to the Recycle Bin with the Windows shell.
//!
//! Uses SHFileOperationW with FOF_ALLOWUNDO (Recycle Bin, never a permanent
//! delete) and FOF_WANTNUKEWARNING: if something can't go to the Recycle Bin
//! (too big, network drive...), Windows itself asks before deleting it
//! permanently. DiskRoot has already asked the user, so the normal
//! "are you sure" prompt is skipped (FOF_NOCONFIRMATION).

use std::ffi::c_void;
use std::path::Path;

const FO_DELETE: u32 = 3;
const FOF_NOCONFIRMATION: u16 = 0x0010;
const FOF_ALLOWUNDO: u16 = 0x0040;
const FOF_WANTNUKEWARNING: u16 = 0x4000;

#[repr(C)]
struct ShFileOpStructW {
    hwnd: *mut c_void,
    w_func: u32,
    p_from: *const u16,
    p_to: *const u16,
    f_flags: u16,
    f_any_operations_aborted: i32,
    h_name_mappings: *mut c_void,
    lpsz_progress_title: *const u16,
}

#[link(name = "shell32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SHFileOperationW(op: *mut ShFileOpStructW) -> i32;
}

pub enum Outcome {
    Recycled,
    Cancelled,
}

pub fn to_recycle_bin(path: &Path) -> Result<Outcome, String> {
    let text = path.to_str().ok_or("Path is not valid Unicode")?;
    // The shell treats '*' and '?' as wildcards. Windows file names can't
    // contain them, but refuse anyway rather than risk a wider delete.
    if text.contains(['*', '?']) || !path.is_absolute() {
        return Err("Refusing an unusual path".into());
    }
    if std::fs::symlink_metadata(path).is_err() {
        return Err("It no longer exists. Rescan to refresh.".into());
    }
    // pFrom is a list of paths, each NUL-terminated, ending with an extra NUL.
    let from: Vec<u16> = text.encode_utf16().chain([0, 0]).collect();
    let mut op = ShFileOpStructW {
        hwnd: std::ptr::null_mut(),
        w_func: FO_DELETE,
        p_from: from.as_ptr(),
        p_to: std::ptr::null(),
        f_flags: FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_WANTNUKEWARNING,
        f_any_operations_aborted: 0,
        h_name_mappings: std::ptr::null_mut(),
        lpsz_progress_title: std::ptr::null(),
    };
    // SAFETY: `op` and the double-NUL-terminated `from` buffer stay alive
    // for the whole (synchronous) call.
    let code = unsafe { SHFileOperationW(&mut op) };
    if op.f_any_operations_aborted != 0 {
        return Ok(Outcome::Cancelled);
    }
    if code != 0 {
        return Err(format!("Windows couldn't move it to the Recycle Bin (error 0x{code:X}). It may be in use, or the path may be too long."));
    }
    if std::fs::symlink_metadata(path).is_ok() {
        return Err("Windows reported success but it is still there (partly in use?).".into());
    }
    Ok(Outcome::Recycled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_bad_paths() {
        assert!(to_recycle_bin(Path::new("relative\\x")).is_err());
        assert!(to_recycle_bin(Path::new("C:\\a*b")).is_err());
        assert!(to_recycle_bin(Path::new("C:\\definitely-not-here-diskroot-test")).is_err());
    }

    /// Really moves a temp file to the Recycle Bin, so it's opt-in:
    /// cargo test -p diskroot-app -- --ignored recycles_a_temp_file
    #[test]
    #[ignore = "puts a small test file in the user's Recycle Bin"]
    fn recycles_a_temp_file() {
        let p = std::env::temp_dir().join(format!("diskroot-recycle-test-{}.txt", std::process::id()));
        std::fs::write(&p, b"diskroot test file - safe to delete").unwrap();
        assert!(matches!(to_recycle_bin(&p), Ok(Outcome::Recycled)));
        assert!(!p.exists());
    }
}
