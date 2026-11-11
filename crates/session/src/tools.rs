//! Starting Windows' own tools for the cleanups Xylem doesn't do itself
//! (DISM, System Protection, Storage settings, compact.exe), and checking
//! which programs are running.
//!
//! Every tool is started by its **absolute path** in System32 (never a bare
//! name that could be found in this exe's folder first), directly, without
//! `cmd.exe`: arguments are never parsed by a shell. Elevated tools go
//! through the normal Windows UAC prompt.

use std::collections::HashSet;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::null_mut;

use crate::integrity::windows_dir;

const SEE_MASK_NOCLOSEPROCESS: u32 = 0x0000_0040;
const SEE_MASK_FLAG_NO_UI: u32 = 0x0000_0400;
const SW_SHOWNORMAL: i32 = 1;
const SW_SHOWMINNOACTIVE: i32 = 7;
const ERROR_CANCELLED: i32 = 1223;
const INFINITE: u32 = u32::MAX;

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

#[repr(C)]
struct ProcessEntry32W {
    size: u32,
    usage: u32,
    pid: u32,
    heap_id: usize,
    module_id: u32,
    threads: u32,
    parent_pid: u32,
    priority: i32,
    flags: u32,
    exe: [u16; 260],
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
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
    fn Process32FirstW(snap: *mut c_void, entry: *mut ProcessEntry32W) -> i32;
    fn Process32NextW(snap: *mut c_void, entry: *mut ProcessEntry32W) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `C:\Windows\System32\<exe>`.
pub fn system32(exe: &str) -> PathBuf {
    windows_dir().join("System32").join(exe)
}

/// Starts `file` (an absolute path or a `ms-settings:` link) with `args`.
/// `admin` asks Windows for admin rights (UAC). With `wait`, blocks until
/// it exits and returns its exit code; call that from a worker thread.
fn shell_execute(file: &str, args: &str, admin: bool, show: i32, wait: bool) -> Result<Option<u32>, String> {
    let (verb, file_w, args_w) = (wide(if admin { "runas" } else { "open" }), wide(file), wide(args));
    let mut info = ShellExecuteInfoW {
        cb_size: size_of::<ShellExecuteInfoW>() as u32,
        f_mask: SEE_MASK_FLAG_NO_UI | if wait { SEE_MASK_NOCLOSEPROCESS } else { 0 },
        hwnd: null_mut(),
        lp_verb: verb.as_ptr(),
        lp_file: file_w.as_ptr(),
        lp_parameters: if args.is_empty() { std::ptr::null() } else { args_w.as_ptr() },
        lp_directory: std::ptr::null(),
        n_show: show,
        h_inst_app: null_mut(),
        lp_id_list: null_mut(),
        lp_class: std::ptr::null(),
        hkey_class: null_mut(),
        dw_hot_key: 0,
        h_icon_or_monitor: null_mut(),
        h_process: null_mut(),
    };
    // SAFETY: `info` and the wide strings it points to outlive the call.
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let e = std::io::Error::last_os_error();
        return Err(if e.raw_os_error() == Some(ERROR_CANCELLED) {
            "Admin permission was declined, so nothing ran.".into()
        } else {
            format!("Couldn't start {file}: {e}")
        });
    }
    if !wait || info.h_process.is_null() {
        return Ok(None);
    }
    let mut code = 1u32;
    // SAFETY: a process handle we own (SEE_MASK_NOCLOSEPROCESS); closed once.
    unsafe {
        WaitForSingleObject(info.h_process, INFINITE);
        GetExitCodeProcess(info.h_process, &mut code);
        CloseHandle(info.h_process);
    }
    Ok(Some(code))
}

/// Windows' supported way to shrink the component store (WinSxS): DISM
/// with /StartComponentCleanup, in its own console window, as admin.
/// Blocks until DISM finishes (minutes): run it on a worker thread.
pub fn dism_component_cleanup() -> Result<String, String> {
    let dism = system32("Dism.exe");
    match shell_execute(&dism.to_string_lossy(), "/Online /Cleanup-Image /StartComponentCleanup", true, SW_SHOWNORMAL, true)? {
        Some(0) => Ok("Windows finished cleaning up the component store. Scan again to see the new size.".into()),
        Some(code) => Err(format!("DISM stopped with code {code} (0x{code:08X}). Nothing was removed by Xylem.")),
        None => Ok("DISM started.".into()),
    }
}

/// Opens System Properties > System Protection (restore points).
pub fn open_system_protection() -> Result<(), String> {
    shell_execute(&system32("SystemPropertiesProtection.exe").to_string_lossy(), "", false, SW_SHOWNORMAL, false).map(|_| ())
}

/// Opens Settings > System > Storage.
pub fn open_storage_settings() -> Result<(), String> {
    shell_execute("ms-settings:storagesense", "", false, SW_SHOWNORMAL, false).map(|_| ())
}

/// Runs a System32 tool without admin rights in a minimized console window
/// and waits for it. Arguments are passed as given (no shell).
pub fn run_system_tool(exe: &str, args: &str) -> Result<u32, String> {
    shell_execute(&system32(exe).to_string_lossy(), args, false, SW_SHOWMINNOACTIVE, true)?.ok_or_else(|| format!("{exe} didn't report back"))
}

/// Runs a System32 tool as admin (UAC prompt) and waits for it.
pub fn run_system_tool_admin(exe: &Path, args: &str) -> Result<u32, String> {
    shell_execute(&exe.to_string_lossy(), args, true, SW_SHOWMINNOACTIVE, true)?.ok_or_else(|| "The tool didn't report back".to_owned())
}

/// Names of running programs, lowercase (e.g. "spotify.exe").
pub fn running_programs() -> HashSet<String> {
    const TH32CS_SNAPPROCESS: u32 = 0x2;
    let mut out = HashSet::new();
    // SAFETY: plain flags; the handle is checked and closed below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap.is_null() || snap == (-1isize as *mut c_void) {
        return out;
    }
    let mut e = ProcessEntry32W { size: size_of::<ProcessEntry32W>() as u32, usage: 0, pid: 0, heap_id: 0, module_id: 0, threads: 0, parent_pid: 0, priority: 0, flags: 0, exe: [0; 260] };
    // SAFETY: `e.size` is set as the API requires; `snap` is valid.
    let mut ok = unsafe { Process32FirstW(snap, &mut e) } != 0;
    while ok {
        let len = e.exe.iter().position(|&c| c == 0).unwrap_or(e.exe.len());
        out.insert(String::from_utf16_lossy(&e.exe[..len]).to_lowercase());
        // SAFETY: as above.
        ok = unsafe { Process32NextW(snap, &mut e) } != 0;
    }
    // SAFETY: we own the snapshot handle.
    unsafe { CloseHandle(snap) };
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn sees_this_test_process() {
        let me = std::env::current_exe().unwrap();
        let name = me.file_name().unwrap().to_string_lossy().to_lowercase();
        let running = super::running_programs();
        assert!(running.contains(&name), "{name} not in {} programs", running.len());
    }

    #[test]
    fn system_tools_are_absolute() {
        let p = super::system32("Dism.exe");
        assert!(p.is_absolute() && p.ends_with(r"System32\Dism.exe"), "{}", p.display());
    }
}
