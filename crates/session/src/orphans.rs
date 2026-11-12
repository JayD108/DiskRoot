//! "Possibly left over" hints for app data folders whose program seems to be
//! gone. Only hints: matching folder names to programs is fuzzy, and AppData
//! holds settings and saved games, so nothing here is ever labelled safe or
//! marked automatically. The evidence is shown with each hint.
//!
//! Evidence used: the programs Windows lists as installed (the Uninstall
//! registry keys, for the machine and the current user: names, publishers,
//! install folders), the programs running now, and how long nothing in the
//! folder changed.

use std::collections::HashSet;
use std::ffi::c_void;
use std::ptr::null_mut;

type Hkey = *mut c_void;
const HKEY_CURRENT_USER: Hkey = 0x8000_0001usize as Hkey;
const HKEY_LOCAL_MACHINE: Hkey = 0x8000_0002usize as Hkey;
const KEY_READ: u32 = 0x2_0019;
const RRF_RT_REG_SZ: u32 = 0x2;
const RRF_RT_REG_EXPAND_SZ: u32 = 0x4;
const RRF_NOEXPAND: u32 = 0x1000_0000;

#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegOpenKeyExW(key: Hkey, sub: *const u16, opts: u32, sam: u32, out: *mut Hkey) -> i32;
    fn RegEnumKeyExW(key: Hkey, i: u32, name: *mut u16, len: *mut u32, r: *mut u32, class: *mut u16, class_len: *mut u32, time: *mut c_void) -> i32;
    fn RegGetValueW(key: Hkey, sub: *const u16, value: *const u16, flags: u32, kind: *mut u32, data: *mut c_void, bytes: *mut u32) -> i32;
    fn RegCloseKey(key: Hkey) -> i32;
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Lowercase letters and digits only ("Adobe Systems, Inc." -> "adobesystemsinc").
pub fn norm(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

/// Words that say nothing about which program it is.
const GENERIC: [&str; 22] = [
    "microsoft", "corporation", "corp", "inc", "llc", "ltd", "software", "the", "for", "windows", "update", "runtime", "edition",
    "x64", "x86", "version", "setup", "tools", "app", "apps", "and", "driver",
];

/// What Windows knows about installed programs: normalized names,
/// publishers, install-folder names and distinctive words of them.
#[derive(Default, Debug)]
pub struct Programs {
    /// Normalized full strings (names, publishers, folder names).
    pub strings: Vec<String>,
    /// Normalized distinctive words (≥ 4 letters).
    pub words: HashSet<String>,
    /// How many programs were listed.
    pub count: usize,
}

impl Programs {
    /// Reads the Uninstall keys (64- and 32-bit, machine and user) and
    /// adds the programs running now. Read-only; a few milliseconds.
    pub fn installed() -> Self {
        let mut p = Self::default();
        for (root, path) in [
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
            (HKEY_CURRENT_USER, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        ] {
            p.read_key(root, path);
        }
        for exe in crate::tools::running_programs() {
            p.add(exe.trim_end_matches(".exe"));
        }
        p
    }

    /// Adds one name, publisher or folder name.
    pub fn add(&mut self, s: &str) {
        let n = norm(s);
        if n.len() >= 3 {
            self.strings.push(n);
        }
        for w in s.split(|c: char| !c.is_ascii_alphanumeric()) {
            let w = w.to_ascii_lowercase();
            if w.len() >= 4 && !GENERIC.contains(&w.as_str()) {
                self.words.insert(w);
            }
        }
    }

    fn read_key(&mut self, root: Hkey, path: &str) {
        let mut h: Hkey = null_mut();
        // SAFETY: NUL-terminated path, valid out pointer; closed below.
        if unsafe { RegOpenKeyExW(root, wide(path).as_ptr(), 0, KEY_READ, &mut h) } != 0 {
            return;
        }
        let mut i = 0u32;
        loop {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            // SAFETY: buffer and its length in characters.
            let r = unsafe { RegEnumKeyExW(h, i, name.as_mut_ptr(), &mut len, null_mut(), null_mut(), null_mut(), null_mut()) };
            if r != 0 {
                break;
            }
            i += 1;
            let sub: Vec<u16> = name[..len as usize].iter().copied().chain([0]).collect();
            let mut any = false;
            for value in ["DisplayName", "Publisher", "InstallLocation"] {
                if let Some(v) = get_string(h, &sub, value) {
                    let v = if value == "InstallLocation" {
                        v.trim_end_matches('\\').rsplit('\\').next().unwrap_or("").to_owned()
                    } else {
                        v
                    };
                    any |= value == "DisplayName" && !v.is_empty();
                    self.add(&v);
                }
            }
            if any {
                self.count += 1;
            }
        }
        // SAFETY: opened above.
        unsafe { RegCloseKey(h) };
    }

    /// Whether a folder name looks like it belongs to a known program.
    pub fn matches(&self, folder: &str) -> bool {
        let f = norm(folder);
        if f.len() < 3 {
            return true; // too short to judge: never hint
        }
        self.strings.iter().any(|s| s.contains(&f) || (s.len() >= 4 && f.contains(s.as_str())))
            || folder.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| self.words.contains(&w.to_ascii_lowercase()))
    }
}

/// (InstallLocation, DisplayName) of every program in the Uninstall keys
/// that names its folder. Read-only.
pub fn install_locations() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (root, path) in [
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
    ] {
        let mut h: Hkey = null_mut();
        // SAFETY: NUL-terminated path, valid out pointer; closed below.
        if unsafe { RegOpenKeyExW(root, wide(path).as_ptr(), 0, KEY_READ, &mut h) } != 0 {
            continue;
        }
        let mut i = 0u32;
        loop {
            let mut name = [0u16; 256];
            let mut len = name.len() as u32;
            // SAFETY: buffer and its length in characters.
            if unsafe { RegEnumKeyExW(h, i, name.as_mut_ptr(), &mut len, null_mut(), null_mut(), null_mut(), null_mut()) } != 0 {
                break;
            }
            i += 1;
            let sub: Vec<u16> = name[..len as usize].iter().copied().chain([0]).collect();
            if let (Some(loc), Some(display)) = (get_string(h, &sub, "InstallLocation"), get_string(h, &sub, "DisplayName")) {
                out.push((loc.trim_matches('"').to_owned(), display));
            }
        }
        // SAFETY: opened above.
        unsafe { RegCloseKey(h) };
    }
    out
}

/// A string value under HKEY_CURRENT_USER (e.g. Steam's install path).
pub fn user_string(path: &str, value: &str) -> Option<String> {
    get_string(HKEY_CURRENT_USER, &wide(path), value)
}

fn get_string(key: Hkey, sub: &[u16], value: &str) -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut bytes = (buf.len() * 2) as u32;
    // SAFETY: sub and value are NUL-terminated; buf/bytes describe a valid buffer.
    let r = unsafe {
        RegGetValueW(key, sub.as_ptr(), wide(value).as_ptr(), RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND, null_mut(), buf.as_mut_ptr().cast(), &mut bytes)
    };
    if r != 0 {
        return None;
    }
    let n = (bytes as usize / 2).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..n]);
    let s = s.trim_end_matches('\0').trim();
    (!s.is_empty()).then(|| s.to_owned())
}

/// App-data folders that belong to Windows or are shared, never hinted.
pub const NEVER: [&str; 24] = [
    "microsoft", "packages", "temp", "programs", "comms", "connecteddevicesplatform", "crashdumps", "d3dscache", "publishers",
    "isolatedstorage", "virtualstore", "history", "applicationdata", "temporaryinternetfiles", "squirreltemp", "packagecache",
    "microsoftedge", "windows", "fonts", "ssh", "regid19911020commicrosoft", "softwaredistribution", "ussoclient", "pip",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_programs_by_name_publisher_or_word() {
        let mut p = Programs::default();
        p.add("Adobe Acrobat Reader");
        p.add("JetBrains s.r.o.");
        p.add("Blender Foundation");
        p.add("Microsoft Corporation");
        assert!(p.matches("Adobe"));
        assert!(p.matches("JetBrains"));
        assert!(p.matches("blender"));
        assert!(p.matches("Blender Foundation"));
        assert!(!p.matches("OldGameStudio"));
        assert!(!p.matches("Corporation Tools"), "generic words alone don't match");
        assert!(p.matches("ab"), "too short to judge");
    }

    #[test]
    fn reads_installed_programs() {
        let p = Programs::installed();
        // Every Windows PC lists some programs and runs some.
        assert!(p.count > 0 && !p.strings.is_empty(), "{p:?}");
    }
}
