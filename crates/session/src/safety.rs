//! Safety around deleting: re-checks right before the Recycle Bin move,
//! warnings worth a second look (cloud-synced, Git repositories, very
//! large), and a local deletion log.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// Things the delete dialog should point out before the user confirms.
#[derive(Clone, Debug, Default, Serialize)]
pub struct DeleteWarnings {
    /// Inside a cloud-synced folder (OneDrive): deleting it here also
    /// deletes it in the cloud and on the user's other devices.
    pub cloud: bool,
    /// Number of Git repositories (`.git` folders) inside; unpushed work in
    /// them would be lost.
    pub git_repos: u32,
    /// Big enough that a slip would hurt (≥ 10 GB or ≥ 50,000 files).
    pub large: bool,
    /// On a USB stick or network share: Windows may have no Recycle Bin
    /// there and delete it permanently (after its own prompt).
    pub no_recycle_bin: bool,
}

impl DeleteWarnings {
    /// True if the user should explicitly tick "I understand" first.
    pub fn needs_ack(&self) -> bool {
        self.cloud || self.git_repos > 0 || self.large || self.no_recycle_bin
    }
}

/// True if `path` is inside a OneDrive folder (personal or work). Windows
/// sets these variables for every signed-in OneDrive account.
pub fn in_cloud_folder(path: &Path) -> bool {
    let p = path.to_string_lossy().to_lowercase();
    ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|d| d.to_string_lossy().trim_end_matches('\\').to_lowercase())
        .filter(|d| d.len() > 3)
        .any(|d| p == d || p.starts_with(&(d + "\\")))
}

pub const LARGE_BYTES: u64 = 10 << 30;
pub const LARGE_FILES: u32 = 50_000;

/// Re-checks `path` against what the scan saw, right before deleting:
/// - it still exists and is still a folder/file as scanned;
/// - it didn't become a link (symlink/junction) since the scan;
/// - no folder between the scan root and it became a link since the scan
///   (that would redirect the delete somewhere else).
pub fn verify_before_recycle(path: &Path, scan_root: &Path, expect_dir: bool, was_link: bool) -> Result<(), String> {
    let changed = |what: &str| Err(format!("{what} since the scan, so Xylem won't delete it. Rescan and try again."));
    let meta = std::fs::symlink_metadata(path).map_err(|_| "It no longer exists. Rescan to refresh.".to_owned())?;
    let is_link = meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 && meta.file_type().is_symlink();
    if is_link != was_link {
        return changed("It was replaced by a link (or the link was replaced)");
    }
    if !is_link && meta.is_dir() != expect_dir {
        return changed(if expect_dir { "It changed from a folder to a file" } else { "It changed from a file to a folder" });
    }
    let mut cur = path.parent();
    while let Some(dir) = cur {
        if dir == scan_root || !dir.starts_with(scan_root) {
            break;
        }
        match std::fs::symlink_metadata(dir) {
            Ok(m) if m.file_type().is_symlink() => return changed(&format!("The folder {} became a link", dir.display())),
            Ok(_) => {}
            Err(_) => return changed(&format!("The folder {} disappeared", dir.display())),
        }
        cur = dir.parent();
    }
    Ok(())
}

/// Names Windows silently rewrites or treats as devices. A file created as
/// `report.` (trailing dot) is a *different* file from `report`, but the
/// Recycle Bin call would delete `report`. Xylem refuses such names (and
/// over-long paths) instead of risking the wrong item.
pub fn unusual_name(path: &Path) -> Option<String> {
    const DEVICES: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2",
        "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if path.as_os_str().encode_wide_len() >= 260 {
        return Some("Its path is longer than Windows' 260-character limit for the Recycle Bin.".into());
    }
    for c in path.components() {
        let std::path::Component::Normal(name) = c else { continue };
        let n = name.to_string_lossy();
        if n.ends_with('.') || n.ends_with(' ') {
            return Some(format!("\"{n}\" ends with a dot or space, which Windows would silently change."));
        }
        let stem = n.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
        if DEVICES.contains(&stem.as_str()) {
            return Some(format!("\"{n}\" is a reserved device name in Windows."));
        }
    }
    None
}

trait WideLen {
    fn encode_wide_len(&self) -> usize;
}
impl WideLen for std::ffi::OsStr {
    fn encode_wide_len(&self) -> usize {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().count()
    }
}

/// Identity of a file or folder on disk: volume serial number + file index.
/// Survives renames; a different item at the same path has another id.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FileId {
    volume: u32,
    index: u64,
}

#[repr(C)]
#[derive(Default)]
struct ByHandleFileInformation {
    attributes: u32,
    times: [u32; 6],
    volume_serial: u32,
    size_high: u32,
    size_low: u32,
    links: u32,
    index_high: u32,
    index_low: u32,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetFileInformationByHandle(h: *mut std::ffi::c_void, info: *mut ByHandleFileInformation) -> i32;
}

const FILE_READ_ATTRIBUTES: u32 = 0x80;
/// Listing access. Needed on pinned folders: Windows only enforces share
/// modes for handles with data or DELETE access, so a metadata-only handle
/// would NOT stop a rename (measured in R&D: the parent could still be
/// renamed while "pinned").
const FILE_LIST_DIRECTORY: u32 = 0x1;
const SHARE_ALL: u32 = 0x7;
/// Read + write sharing, but no FILE_SHARE_DELETE: while we hold the handle
/// nobody can rename or delete the folder.
const SHARE_NO_DELETE: u32 = 0x3;

/// Opens the item itself (never what a link points to).
fn open_item(path: &Path, access: u32, share: u32) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    OpenOptions::new()
        .access_mode(access)
        .share_mode(share)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

fn id_of(f: &std::fs::File) -> std::io::Result<FileId> {
    use std::os::windows::io::AsRawHandle;
    let mut info = ByHandleFileInformation::default();
    // SAFETY: valid open handle and a correctly sized output struct.
    if unsafe { GetFileInformationByHandle(f.as_raw_handle(), &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(FileId { volume: info.volume_serial, index: u64::from(info.index_high) << 32 | u64::from(info.index_low) })
}

/// Opens the item itself (never what a link points to) with no data access.
pub fn file_id(path: &Path) -> std::io::Result<FileId> {
    id_of(&open_item(path, FILE_READ_ATTRIBUTES, SHARE_ALL)?)
}

/// The identities of `path` and of every folder above it, up to and
/// including `scan_root`. Taken when the delete dialog opens and compared
/// right before deleting: if anything on the way was swapped (a folder
/// replaced, a junction slipped in, even the scan root itself), the ids
/// differ and the delete is refused.
pub fn identity_chain(path: &Path, scan_root: &Path) -> Result<Vec<(PathBuf, FileId)>, String> {
    let mut out = Vec::new();
    let mut cur = Some(path);
    while let Some(p) = cur {
        out.push((p.to_path_buf(), file_id(p).map_err(|_| format!("{} no longer exists. Rescan to refresh.", p.display()))?));
        if p == scan_root || !p.starts_with(scan_root) {
            break;
        }
        cur = p.parent();
    }
    Ok(out)
}

/// Hard locks that don't depend on how a path is spelled (UNC `\\host\C$`,
/// `\\?\`, `subst`, 8.3 names, junctions): the item and every folder above
/// it, up to the drive root, are compared by file identity with the real
/// Windows and Program Files folders (locked with everything inside), and
/// with ProgramData and the Users folder (locked themselves).
pub fn hard_lock(path: &Path) -> Option<String> {
    let id = |p: &Path| file_id(p).ok();
    let tree: Vec<(PathBuf, FileId)> = std::iter::once(crate::integrity::windows_dir())
        .chain(["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"].iter().filter_map(std::env::var_os).map(PathBuf::from))
        .filter_map(|p| id(&p).map(|i| (p, i)))
        .collect();
    let own: Vec<(PathBuf, FileId)> = ["ProgramData", "PUBLIC", "USERPROFILE"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .flat_map(|p| {
            // For PUBLIC / USERPROFILE also lock their parent, the Users folder.
            let parent = p.parent().map(Path::to_path_buf);
            [Some(p), parent]
        })
        .flatten()
        .filter_map(|p| id(&p).map(|i| (p, i)))
        .collect();
    let item = id(path)?;
    if let Some((p, _)) = own.iter().chain(&tree).find(|(_, i)| *i == item) {
        return Some(format!("This is {} itself, which Xylem never deletes.", p.display()));
    }
    let mut cur = path.parent();
    while let Some(dir) = cur {
        if let Some(d) = id(dir)
            && let Some((p, _)) = tree.iter().find(|(_, i)| *i == d)
        {
            return Some(format!("It's inside {}, which Xylem never deletes from.", p.display()));
        }
        cur = dir.parent();
    }
    None
}

/// Every name of a hardlinked file (including `path` itself), as full paths.
/// Empty if the file has a single name or can't be queried. Asks Windows
/// live (FindFirstFileNameW), so it works for any scan mode and needs no
/// admin rights.
pub fn hardlink_names(path: &Path) -> Vec<String> {
    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn FindFirstFileNameW(name: *const u16, flags: u32, len: *mut u32, link: *mut u16) -> *mut std::ffi::c_void;
        fn FindNextFileNameW(find: *mut std::ffi::c_void, len: *mut u32, link: *mut u16) -> i32;
        fn FindClose(find: *mut std::ffi::c_void) -> i32;
    }
    let s = path.to_string_lossy();
    let Some(drive) = s.get(..2).filter(|d| d.ends_with(':')) else { return Vec::new() };
    let w: Vec<u16> = s.encode_utf16().chain([0]).collect();
    let mut buf = vec![0u16; 32_768];
    let mut out = Vec::new();
    let mut len = buf.len() as u32;
    // SAFETY: NUL-terminated path, buffer and its length passed together;
    // the find handle is closed below.
    let h = unsafe { FindFirstFileNameW(w.as_ptr(), 0, &mut len, buf.as_mut_ptr()) };
    if h.is_null() || h == -1isize as *mut std::ffi::c_void {
        return out;
    }
    loop {
        let n = (len as usize).saturating_sub(1).min(buf.len()); // len includes the NUL
        out.push(format!("{drive}{}", String::from_utf16_lossy(&buf[..n])));
        len = buf.len() as u32;
        // SAFETY: as above.
        if unsafe { FindNextFileNameW(h, &mut len, buf.as_mut_ptr()) } == 0 || out.len() >= 64 {
            break;
        }
    }
    // SAFETY: valid find handle from FindFirstFileNameW.
    unsafe { FindClose(h) };
    if out.len() < 2 { Vec::new() } else { out }
}

/// Makes a name safe to show: control characters, bidirectional overrides
/// (U+202E can make `fdp.exe` look like `exe.pdf`) and zero-width characters
/// are shown as `<U+XXXX>`, so the dialog and log show what's really there.
pub fn visible(name: &str) -> String {
    name.chars()
        .map(|c| {
            let hidden = c.is_control()
                || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}');
            if hidden { format!("<U+{:04X}>", c as u32) } else { c.to_string() }
        })
        .collect()
}

/// Folders held open by [`pin_identity`]; dropping it releases them.
pub struct Pinned {
    _held: Vec<std::fs::File>,
}

/// Re-checks the identity chain taken at confirm time **on open handles**,
/// and keeps every folder above the item open without FILE_SHARE_DELETE.
/// While the returned guard lives, none of those folders can be renamed,
/// deleted or replaced (e.g. by a junction), so the path the Recycle Bin
/// call resolves is exactly the one that was checked: no gap between check
/// and use. `chain[0]` (the item) is checked but not held, because the
/// shell has to move it. Measured: ~0.1 ms for a 4-level chain.
pub fn pin_identity(chain: &[(PathBuf, FileId)]) -> Result<Pinned, String> {
    const ERROR_ACCESS_DENIED: i32 = 5;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    let replaced = |p: &Path| {
        format!("{} was replaced or moved after you confirmed, so Xylem won't delete anything. Rescan and try again.", p.display())
    };
    let mut held = Vec::with_capacity(chain.len());
    // Top down: once a folder is pinned, nothing below it can be re-pointed
    // by renaming it.
    for (i, (p, want)) in chain.iter().enumerate().rev() {
        let f = if i == 0 {
            open_item(p, FILE_READ_ATTRIBUTES, SHARE_ALL)
        } else {
            match open_item(p, FILE_READ_ATTRIBUTES | FILE_LIST_DIRECTORY, SHARE_NO_DELETE) {
                // A folder the user may traverse but not list (rare): check
                // it like before, without holding it.
                Err(e) if e.raw_os_error() == Some(ERROR_ACCESS_DENIED) => open_item(p, FILE_READ_ATTRIBUTES, SHARE_ALL),
                other => other,
            }
        };
        let f = f.map_err(|e| {
            if e.raw_os_error() == Some(ERROR_SHARING_VIOLATION) {
                format!("{} is being renamed or deleted right now, so Xylem won't delete anything. Try again.", p.display())
            } else {
                replaced(p)
            }
        })?;
        if id_of(&f).ok() != Some(*want) {
            return Err(replaced(p));
        }
        if i > 0 {
            held.push(f);
        }
    }
    Ok(Pinned { _held: held })
}

/// One line of the deletion log.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogEntry {
    /// UTC, "2026-11-10 18:42:07".
    pub time: String,
    pub path: String,
    pub bytes: u64,
    pub files: u32,
    pub category: String,
    /// "recycled", "cancelled" or "failed: …".
    pub outcome: String,
}

/// `%LOCALAPPDATA%\Xylem\deletions.log` (one JSON object per line).
pub fn log_path() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Xylem").join("deletions.log"))
}

pub fn append_log(entry: &LogEntry) {
    let Some(p) = log_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let (Ok(mut f), Ok(line)) = (OpenOptions::new().create(true).append(true).open(&p), serde_json::to_string(entry)) {
        let _ = writeln!(f, "{line}");
    }
}

/// Most recent entries first (at most `limit`). Bad lines are skipped.
pub fn read_log(limit: usize) -> Vec<LogEntry> {
    let Some(text) = log_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return Vec::new() };
    let mut v: Vec<LogEntry> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    v.reverse();
    v.truncate(limit);
    v
}

/// "YYYY-MM-DD HH:MM:SS" in UTC, without pulling in a date library.
pub fn utc_now() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    format_utc(secs)
}

fn format_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant), valid for the proleptic Gregorian calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_utc_dates() {
        assert_eq!(format_utc(0), "1970-01-01 00:00:00");
        assert_eq!(format_utc(1_762_800_000), "2025-11-10 18:40:00");
        assert_eq!(format_utc(951_782_400), "2000-02-29 00:00:00", "leap day");
    }

    #[test]
    fn lists_hardlink_names() {
        let dir = std::env::temp_dir().join(format!("xylem-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        assert!(hardlink_names(&dir.join("a.txt")).is_empty(), "single name");
        std::fs::hard_link(dir.join("a.txt"), dir.join("sub").join("b.txt")).unwrap();
        let names = hardlink_names(&dir.join("a.txt"));
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names.iter().any(|n| n.to_lowercase().ends_with(r"sub\b.txt")), "{names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hard_lock_ignores_path_spelling() {
        let win = crate::integrity::windows_dir();
        assert!(hard_lock(&win.join("Fonts")).is_some(), "inside Windows");
        assert!(hard_lock(&win).is_some(), "Windows itself");
        // Same folder through a subst drive letter (no admin needed).
        let subst = win.join("System32").join("subst.exe");
        if let Some(l) = ('P'..='Y').rev().find(|c| !Path::new(&format!("{c}:\\")).exists()) {
            let drive = format!("{l}:");
            if std::process::Command::new(&subst).arg(&drive).arg(&win).status().is_ok_and(|s| s.success()) {
                let r = hard_lock(Path::new(&format!("{drive}\\Fonts")));
                let _ = std::process::Command::new(&subst).args([drive.as_str(), "/D"]).status();
                assert!(r.is_some(), "subst {drive} -> Windows must stay locked");
            }
        }
        assert!(hard_lock(&std::env::temp_dir()).is_none(), "temp is not locked");
        assert_eq!(visible("invoice\u{202E}fdp.exe"), "invoice<U+202E>fdp.exe");
        assert_eq!(visible("a\tb"), "a<U+0009>b");
    }

    /// The folders above an item stay locked against rename while pinned,
    /// the item itself stays movable, and a swapped folder is refused.
    #[test]
    fn pinned_chain_blocks_swaps() {
        let root = std::env::temp_dir().join(format!("xylem-pin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a").join("b")).unwrap();
        let item = root.join("a").join("b").join("f.txt");
        std::fs::write(&item, b"x").unwrap();
        let chain = identity_chain(&item, &root).unwrap();

        let pinned = pin_identity(&chain).unwrap();
        assert!(std::fs::rename(root.join("a"), root.join("a2")).is_err(), "grandparent locked");
        assert!(std::fs::rename(root.join("a").join("b"), root.join("a").join("b2")).is_err(), "parent locked");
        std::fs::rename(&item, root.join("a").join("b").join("g.txt")).unwrap();
        std::fs::rename(root.join("a").join("b").join("g.txt"), &item).unwrap();
        drop(pinned);

        // After release: swap folder b for a new folder of the same name.
        std::fs::rename(root.join("a").join("b"), root.join("a").join("b-old")).unwrap();
        std::fs::create_dir(root.join("a").join("b")).unwrap();
        std::fs::write(&item, b"other").unwrap();
        let err = pin_identity(&chain).err().unwrap();
        assert!(err.contains("replaced or moved"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn re_check_catches_changes() {
        let root = std::env::temp_dir().join(format!("xylem-safety-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a").join("b")).unwrap();
        std::fs::write(root.join("a").join("f.txt"), b"x").unwrap();

        assert!(verify_before_recycle(&root.join("a").join("b"), &root, true, false).is_ok());
        assert!(verify_before_recycle(&root.join("a").join("f.txt"), &root, false, false).is_ok());
        assert!(verify_before_recycle(&root.join("a").join("f.txt"), &root, true, false).is_err(), "type changed");
        assert!(verify_before_recycle(&root.join("gone"), &root, true, false).is_err(), "missing");

        // Replace folder "a" with a junction to somewhere else: deleting
        // a\b must now be refused, because it would hit the junction target.
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(elsewhere.join("b")).unwrap();
        std::fs::rename(root.join("a"), root.join("a-old")).unwrap();
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(root.join("a"))
            .arg(&elsewhere)
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "mklink /J");
        let err = verify_before_recycle(&root.join("a").join("b"), &root, true, false).unwrap_err();
        assert!(err.contains("became a link"), "{err}");

        let _ = std::fs::remove_dir(root.join("a"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
