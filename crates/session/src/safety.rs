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
}

impl DeleteWarnings {
    /// True if the user should explicitly tick "I understand" first.
    pub fn needs_ack(&self) -> bool {
        self.cloud || self.git_repos > 0 || self.large
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
