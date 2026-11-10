//! "Can this copy of Xylem be trusted to elevate?"
//!
//! - **Install location:** the admin helper is an exe launched with admin
//!   rights. If it lives in a folder normal users can write to, malware
//!   running as the user could swap it before the UAC prompt. Program Files
//!   is admin-only, so an installed copy is protected.
//! - **Signature:** checks the Authenticode signature with Windows'
//!   WinVerifyTrust. Revocation checks are off and URL retrieval is cache-
//!   only, so this check never touches the network.

use std::ffi::c_void;
use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct Integrity {
    pub exe: String,
    /// Running from Program Files (only admins can modify it).
    pub protected_location: bool,
    pub signature: SignatureState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum SignatureState {
    /// Valid Authenticode signature, trusted by Windows.
    Signed,
    /// No signature (typical for builds from source).
    Unsigned,
    /// A signature that Windows does not trust (tampered, expired, unknown).
    Invalid,
}

pub fn check_current_exe() -> Integrity {
    let exe = std::env::current_exe().unwrap_or_default();
    Integrity {
        exe: exe.display().to_string(),
        protected_location: is_protected_location(&exe),
        signature: signature(&exe),
    }
}

fn program_files_dirs() -> Vec<PathBuf> {
    ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect()
}

pub fn is_protected_location(exe: &Path) -> bool {
    let Ok(exe) = std::fs::canonicalize(exe) else { return false };
    let low = |p: &Path| p.to_string_lossy().to_lowercase();
    let e = low(&exe);
    program_files_dirs().iter().any(|pf| e.starts_with(&(low(pf) + "\\")))
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SetDefaultDllDirectories(flags: u32) -> i32;
}

/// Call first thing in `main`: any DLL loaded later (by name) comes only
/// from System32, never from the exe's folder or the current directory.
/// Static imports are covered by the `/DEPENDENTLOADFLAG:0x800` link flag.
pub fn harden_dll_search() -> bool {
    const LOAD_LIBRARY_SEARCH_SYSTEM32: u32 = 0x0000_0800;
    // SAFETY: plain flag argument, no pointers.
    unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) != 0 }
}

#[repr(C)]
struct Guid {
    d1: u32,
    d2: u16,
    d3: u16,
    d4: [u8; 8],
}

/// WINTRUST_ACTION_GENERIC_VERIFY_V2
const GENERIC_VERIFY_V2: Guid =
    Guid { d1: 0x00aa_c56b, d2: 0xcd44, d3: 0x11d0, d4: [0x8c, 0xc2, 0x00, 0xc0, 0x4f, 0xc2, 0x95, 0xee] };

#[repr(C)]
struct WintrustFileInfo {
    cb_struct: u32,
    file_path: *const u16,
    h_file: *mut c_void,
    known_subject: *const Guid,
}

#[repr(C)]
struct WintrustData {
    cb_struct: u32,
    policy_callback_data: *mut c_void,
    sip_client_data: *mut c_void,
    ui_choice: u32,
    revocation_checks: u32,
    union_choice: u32,
    file: *mut WintrustFileInfo,
    state_action: u32,
    state_data: *mut c_void,
    url_reference: *mut u16,
    prov_flags: u32,
    ui_context: u32,
    signature_settings: *mut c_void,
}

const WTD_UI_NONE: u32 = 2;
const WTD_REVOKE_NONE: u32 = 0;
const WTD_CHOICE_FILE: u32 = 1;
const WTD_STATEACTION_VERIFY: u32 = 1;
const WTD_STATEACTION_CLOSE: u32 = 2;
const WTD_CACHE_ONLY_URL_RETRIEVAL: u32 = 0x0000_1000;
const TRUST_E_NOSIGNATURE: u32 = 0x800B_0100;
const TRUST_E_SUBJECT_FORM_UNKNOWN: u32 = 0x800B_0003;
const TRUST_E_PROVIDER_UNKNOWN: u32 = 0x800B_0001;

#[link(name = "wintrust", kind = "raw-dylib")]
unsafe extern "system" {
    fn WinVerifyTrust(hwnd: *mut c_void, action: *const Guid, data: *mut c_void) -> i32;
}

pub fn signature(path: &Path) -> SignatureState {
    let wide: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain([0]).collect();
    let mut file = WintrustFileInfo {
        cb_struct: size_of::<WintrustFileInfo>() as u32,
        file_path: wide.as_ptr(),
        h_file: std::ptr::null_mut(),
        known_subject: std::ptr::null(),
    };
    let mut data = WintrustData {
        cb_struct: size_of::<WintrustData>() as u32,
        policy_callback_data: std::ptr::null_mut(),
        sip_client_data: std::ptr::null_mut(),
        ui_choice: WTD_UI_NONE,
        revocation_checks: WTD_REVOKE_NONE,
        union_choice: WTD_CHOICE_FILE,
        file: &mut file,
        state_action: WTD_STATEACTION_VERIFY,
        state_data: std::ptr::null_mut(),
        url_reference: std::ptr::null_mut(),
        prov_flags: WTD_CACHE_ONLY_URL_RETRIEVAL,
        ui_context: 0,
        signature_settings: std::ptr::null_mut(),
    };
    // SAFETY: the structs are correctly sized WINTRUST_* layouts and the
    // path buffer outlives both calls; the CLOSE call frees provider state.
    let result = unsafe {
        let r = WinVerifyTrust(std::ptr::null_mut(), &GENERIC_VERIFY_V2, (&mut data as *mut WintrustData).cast());
        data.state_action = WTD_STATEACTION_CLOSE;
        WinVerifyTrust(std::ptr::null_mut(), &GENERIC_VERIFY_V2, (&mut data as *mut WintrustData).cast());
        r as u32
    };
    match result {
        0 => SignatureState::Signed,
        TRUST_E_NOSIGNATURE | TRUST_E_SUBJECT_FORM_UNKNOWN | TRUST_E_PROVIDER_UNKNOWN => SignatureState::Unsigned,
        _ => SignatureState::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_own_binaries_are_signed_and_ours_are_not_yet() {
        // Windows' own exes are mostly catalog-signed (this check only reads
        // embedded signatures, which is what Xylem's release exe will carry),
        // so test against Edge, which has an embedded signature.
        if let Some(pf) = std::env::var_os("ProgramFiles(x86)") {
            let edge = PathBuf::from(pf).join("Microsoft\\Edge\\Application\\msedge.exe");
            if edge.exists() {
                assert_eq!(signature(&edge), SignatureState::Signed);
            }
        }
        // This test binary is unsigned.
        let me = std::env::current_exe().unwrap();
        assert_eq!(signature(&me), SignatureState::Unsigned);
        assert!(!is_protected_location(&me), "test binaries run from target/");
    }

    #[test]
    fn program_files_counts_as_protected() {
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            let p = PathBuf::from(pf).join("Common Files");
            if p.exists() {
                assert!(is_protected_location(&p));
            }
        }
        assert!(!is_protected_location(&std::env::temp_dir()));
    }
}
