//! Drops the elevated helper's power once it has opened what it needs.
//!
//! The helper needs admin rights for exactly one thing: opening the raw
//! volume. So it opens the volume and the result pipe first, then calls
//! [`lockdown`], and only then reads and parses the MFT. Handles that are
//! already open keep the access they were granted; every *new* open is
//! checked against the reduced token. A bug in the parser (the code that
//! handles drive data a crafted disk could control) can then no longer use
//! admin rights.
//!
//! Steps:
//! 1. **Remove every privilege** from the token (removed privileges can't
//!    be re-enabled, unlike disabled ones).
//! 2. **Lower the integrity level to Low.** Windows' "no write up" rule then
//!    blocks writing to files, registry keys and processes at Medium or
//!    above (almost everything), even though the token is an admin token.
//! 3. **No child processes:** the process joins a job that allows only one
//!    active process (itself).
//! 4. **Process mitigations:** no dynamic code, no legacy extension points,
//!    no DLLs from remote or low-integrity locations.
//!
//! Steps 1 and 2 are required: if either fails, the helper doesn't scan.
//! Steps 3 and 4 are extra layers and are best-effort.
//!
//! Measured (R&D, non-elevated and elevated): after lockdown, writing
//! through the pre-opened handle works; creating a file in TEMP fails with
//! ERROR_ACCESS_DENIED; starting `cmd.exe` fails (1816); cost is a few
//! microseconds.

use std::ffi::c_void;
use std::ptr::null_mut;

type Handle = *mut c_void;

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
    fn GetTokenInformation(token: Handle, class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
    fn AdjustTokenPrivileges(token: Handle, disable_all: i32, new: *const c_void, len: u32, prev: *mut c_void, ret: *mut u32) -> i32;
    fn SetTokenInformation(token: Handle, class: u32, info: *const c_void, len: u32) -> i32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetCurrentProcess() -> Handle;
    fn CloseHandle(h: Handle) -> i32;
    fn GetLastError() -> u32;
    fn CreateJobObjectW(sa: *mut c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(job: Handle, class: u32, info: *const c_void, len: u32) -> i32;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn SetProcessMitigationPolicy(policy: u32, buf: *const c_void, len: usize) -> i32;
}

const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_ADJUST_PRIVILEGES: u32 = 0x0020;
const TOKEN_ADJUST_DEFAULT: u32 = 0x0080;
const TOKEN_PRIVILEGES_CLASS: u32 = 3;
const TOKEN_INTEGRITY_LEVEL_CLASS: u32 = 25;
const SE_PRIVILEGE_REMOVED: u32 = 0x4;
const SE_GROUP_INTEGRITY: u32 = 0x20;

/// What [`lockdown`] achieved. `Err` holds the Win32 error code.
#[derive(Debug)]
pub struct Report {
    pub privileges_removed: Result<usize, u32>,
    pub low_integrity: Result<(), u32>,
    pub no_child_processes: Result<(), u32>,
    pub mitigations: [(&'static str, Result<(), u32>); 3],
}

impl Report {
    /// The two steps the helper can't do without.
    pub fn essential_ok(&self) -> bool {
        self.privileges_removed.is_ok() && self.low_integrity.is_ok()
    }
}

fn last_error() -> u32 {
    // SAFETY: no preconditions.
    unsafe { GetLastError() }
}

/// Locks down the current process (see the module docs). Irreversible.
pub fn lockdown() -> Report {
    let mut tok: Handle = null_mut();
    let access = TOKEN_QUERY | TOKEN_ADJUST_PRIVILEGES | TOKEN_ADJUST_DEFAULT;
    // SAFETY: current-process pseudo-handle; `tok` is a valid out-pointer.
    let (privileges_removed, low_integrity) = if unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut tok) } == 0 {
        let e = last_error();
        (Err(e), Err(e))
    } else {
        let r = (remove_privileges(tok), set_low_integrity(tok));
        // SAFETY: we own the token handle and close it once.
        unsafe { CloseHandle(tok) };
        r
    };
    Report { privileges_removed, low_integrity, no_child_processes: no_child_processes(), mitigations: mitigations() }
}

fn remove_privileges(tok: Handle) -> Result<usize, u32> {
    let mut need = 0u32;
    // SAFETY: size query with a null buffer.
    unsafe { GetTokenInformation(tok, TOKEN_PRIVILEGES_CLASS, null_mut(), 0, &mut need) };
    if !(4..=64 * 1024).contains(&need) {
        return Err(last_error());
    }
    // u64 storage keeps the LUIDs aligned.
    let mut buf = vec![0u64; (need as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `need` writable bytes.
    if unsafe { GetTokenInformation(tok, TOKEN_PRIVILEGES_CLASS, buf.as_mut_ptr().cast(), need, &mut need) } == 0 {
        return Err(last_error());
    }
    // SAFETY: the first `need` bytes of `buf` are initialised by the call.
    let bytes: &mut [u8] = unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr().cast(), need as usize) };
    // TOKEN_PRIVILEGES: u32 count, then count x { LUID (8 bytes), u32 attributes }.
    let count = u32::from_le_bytes(bytes[0..4].try_into().expect("4 bytes")) as usize;
    if 4 + count * 12 > bytes.len() {
        return Err(0);
    }
    if count == 0 {
        return Ok(0);
    }
    for i in 0..count {
        let at = 4 + i * 12 + 8;
        bytes[at..at + 4].copy_from_slice(&SE_PRIVILEGE_REMOVED.to_le_bytes());
    }
    // SAFETY: a well-formed TOKEN_PRIVILEGES of `need` bytes.
    let ok = unsafe { AdjustTokenPrivileges(tok, 0, buf.as_ptr().cast(), need, null_mut(), null_mut()) };
    // AdjustTokenPrivileges "succeeds" with ERROR_NOT_ALL_ASSIGNED on partial success.
    match (ok, last_error()) {
        (0, e) => Err(e),
        (_, 0) => Ok(count),
        (_, e) => Err(e),
    }
}

fn set_low_integrity(tok: Handle) -> Result<(), u32> {
    // SID S-1-16-4096 (Mandatory Label\Low Mandatory Level).
    let sid: [u8; 12] = [1, 1, 0, 0, 0, 0, 0, 16, 0x00, 0x10, 0x00, 0x00];
    #[repr(C)]
    struct TokenMandatoryLabel {
        sid: *const u8,
        attributes: u32,
    }
    let label = TokenMandatoryLabel { sid: sid.as_ptr(), attributes: SE_GROUP_INTEGRITY };
    let len = (size_of::<TokenMandatoryLabel>() + sid.len()) as u32;
    // SAFETY: TOKEN_MANDATORY_LABEL pointing at a valid SID that outlives the call.
    if unsafe { SetTokenInformation(tok, TOKEN_INTEGRITY_LEVEL_CLASS, (&raw const label).cast(), len) } == 0 {
        Err(last_error())
    } else {
        Ok(())
    }
}

fn no_child_processes() -> Result<(), u32> {
    // JOBOBJECT_EXTENDED_LIMIT_INFORMATION (x64, 144 bytes): LimitFlags at
    // offset 16, ActiveProcessLimit at offset 40.
    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: u32 = 9;
    const JOB_OBJECT_LIMIT_ACTIVE_PROCESS: u32 = 0x8;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;
    #[cfg(not(target_pointer_width = "64"))]
    return Err(0);
    let mut info = [0u8; 144];
    info[16..20].copy_from_slice(&(JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE).to_le_bytes());
    info[40..44].copy_from_slice(&1u32.to_le_bytes());
    // SAFETY: unnamed job. The handle is deliberately never closed: the job
    // must live as long as the process.
    let job = unsafe { CreateJobObjectW(null_mut(), std::ptr::null()) };
    if job.is_null() {
        return Err(last_error());
    }
    // SAFETY: valid job handle and a correctly sized buffer.
    if unsafe { SetInformationJobObject(job, JOB_OBJECT_EXTENDED_LIMIT_INFORMATION, info.as_ptr().cast(), 144) } == 0 {
        return Err(last_error());
    }
    // SAFETY: current-process pseudo-handle.
    if unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) } == 0 {
        return Err(last_error());
    }
    Ok(())
}

fn mitigations() -> [(&'static str, Result<(), u32>); 3] {
    const DYNAMIC_CODE: u32 = 2;
    const EXTENSION_POINTS: u32 = 6;
    const IMAGE_LOAD: u32 = 10;
    let set = |policy: u32, flags: u32| {
        // SAFETY: each of these policy structs is a single u32 of flag bits.
        if unsafe { SetProcessMitigationPolicy(policy, (&raw const flags).cast(), 4) } == 0 { Err(last_error()) } else { Ok(()) }
    };
    [
        ("no dynamic code", set(DYNAMIC_CODE, 0x1)),
        ("no extension points", set(EXTENSION_POINTS, 0x1)),
        // NoRemoteImages | NoLowMandatoryLabelImages | PreferSystem32Images
        ("image load restrictions", set(IMAGE_LOAD, 0x1 | 0x2 | 0x4)),
    ]
}

#[cfg(test)]
mod tests {
    /// Locks down a child copy of the test binary (lockdown is
    /// irreversible, so never in the test runner itself) and checks what it
    /// can still do.
    #[test]
    fn locked_down_process_cannot_write_or_spawn() {
        if std::env::var_os("XYLEM_LOCKDOWN_CHILD").is_some() {
            return; // handled by `child_side`
        }
        let dir = std::env::temp_dir().join(format!("xylem-lockdown-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = std::env::current_exe().unwrap();
        let out = std::process::Command::new(exe)
            .args(["--exact", "lockdown::tests::child_side", "--nocapture", "--test-threads=1"])
            .env("XYLEM_LOCKDOWN_CHILD", &dir)
            .output()
            .unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "child failed:\n{text}\n{}", String::from_utf8_lossy(&out.stderr));
        assert!(text.contains("LOCKDOWN-CHILD-OK"), "{text}");
    }

    #[test]
    fn child_side() {
        let Some(dir) = std::env::var_os("XYLEM_LOCKDOWN_CHILD").map(std::path::PathBuf::from) else { return };
        use std::io::Write;
        let pre = dir.join("pre.tmp");
        let post = dir.join("post.tmp");
        let mut held = std::fs::File::create(&pre).unwrap();
        let r = super::lockdown();
        assert!(r.essential_ok(), "{r:?}");
        assert!(r.no_child_processes.is_ok(), "{r:?}");
        held.write_all(b"pre-opened handles keep working").unwrap();
        assert!(std::fs::File::create(&post).is_err(), "new files must be refused at Low integrity");
        assert!(std::process::Command::new("cmd.exe").args(["/c", "exit"]).status().is_err(), "no child processes");
        drop(held);
        // We can't delete our own file any more (that's the point); the
        // parent test removes the folder.
        println!("LOCKDOWN-CHILD-OK");
    }
}
