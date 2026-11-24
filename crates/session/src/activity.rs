//! Activity history: what the user did through DiskRoot on this PC.
//!
//! Deletions already have their own log (`deletions.log`, see `safety`).
//! This adds `activity.log` for the other actions that change files
//! (compress, uncompress) and finished duplicate searches, one JSON object
//! per line, and merges both logs for the Activity page. Local only.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::safety;

/// One action in `activity.log`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Logged {
    /// Unix seconds (UTC).
    pub time: u64,
    /// "compressed", "uncompressed" or "duplicates".
    pub kind: String,
    pub path: String,
    pub files: u64,
    /// Size on disk before and after (duplicates: bytes in extra copies found, 0).
    pub before: u64,
    pub after: u64,
    /// "Fast", "Maximum", "stopped", "3 sets found"…
    pub note: String,
}

/// One row of the Activity page, from either log.
#[derive(Clone, Debug)]
pub struct Entry {
    pub time: u64,
    /// "deleted", "compressed", "uncompressed", "duplicates", or a failed
    /// / cancelled deletion ("not deleted").
    pub kind: String,
    pub path: String,
    pub files: u64,
    /// Bytes freed (deleted) or saved (compressed); for uncompress, the
    /// space given back to the files.
    pub bytes: u64,
    pub before: u64,
    pub after: u64,
    pub note: String,
}

/// `%LOCALAPPDATA%\DiskRoot\activity.log`.
pub fn log_path() -> Option<PathBuf> {
    if cfg!(test) {
        return Some(std::env::temp_dir().join(format!("diskroot-test-activity-{}", std::process::id())).join("activity.log"));
    }
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("DiskRoot").join("activity.log"))
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

pub fn append(e: &Logged) {
    let Some(p) = log_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let (Ok(mut f), Ok(line)) = (OpenOptions::new().create(true).append(true).open(&p), serde_json::to_string(e)) {
        let _ = writeln!(f, "{line}");
    }
}

/// Notes a finished compress or uncompress run (nothing if no file changed
/// and it wasn't stopped).
pub fn log_compress(root: &std::path::Path, r: &crate::compress::Report) {
    if r.changed_files == 0 && !r.cancelled {
        return;
    }
    let level = r.level.map_or("", |l| l.label());
    let note = match (r.cancelled, level) {
        (true, "") => "stopped".to_owned(),
        (true, l) => format!("{l}, stopped"),
        (false, l) => l.to_owned(),
    };
    append(&Logged {
        time: now(),
        kind: if r.undo { "uncompressed" } else { "compressed" }.into(),
        path: root.display().to_string(),
        files: r.changed_files,
        before: r.before,
        after: r.after,
        note,
    });
}

/// "2026-11-10 18:42:07" (UTC, as in deletions.log) -> Unix seconds.
pub fn parse_utc(s: &str) -> Option<u64> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hh, mm, ss) = (n(11..13)?, n(14..16)?, n(17..19)?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Days from civil (Howard Hinnant).
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3600 + mm * 60 + ss).ok()
}

/// Everything from both logs, newest first (at most `limit`).
pub fn read(limit: usize) -> Vec<Entry> {
    let mut v: Vec<Entry> = Vec::new();
    for d in safety::read_log(limit) {
        let Some(time) = parse_utc(&d.time) else { continue };
        let gone = d.outcome == "deleted permanently";
        let ok = d.outcome == "recycled" || gone;
        v.push(Entry {
            time,
            kind: if ok { "deleted".into() } else { "not deleted".into() },
            path: d.path,
            files: u64::from(d.files),
            bytes: if ok { d.bytes } else { 0 },
            before: d.bytes,
            after: 0,
            note: match (ok, gone) {
                (true, false) => format!("to the Recycle Bin · {}", d.category),
                (true, true) => format!("deleted permanently (not in the Recycle Bin) · {}", d.category),
                _ => d.outcome,
            },
        });
    }
    if let Some(text) = log_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        for l in text.lines().filter_map(|l| serde_json::from_str::<Logged>(l).ok()) {
            let bytes = if l.kind == "duplicates" { l.before } else { l.before.abs_diff(l.after) };
            v.push(Entry { time: l.time, kind: l.kind, path: l.path, files: l.files, bytes, before: l.before, after: l.after, note: l.note });
        }
    }
    v.sort_by_key(|e| std::cmp::Reverse(e.time));
    v.truncate(limit);
    v
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct SystemTime {
    year: u16,
    month: u16,
    weekday: u16,
    day: u16,
    hour: u16,
    minute: u16,
    second: u16,
    ms: u16,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn SystemTimeToTzSpecificLocalTime(tz: *const core::ffi::c_void, utc: *const SystemTime, local: *mut SystemTime) -> i32;
}

/// Unix seconds -> local date and time, "14 Nov 2026, 18:42" (Windows'
/// time zone rules, so summer time is right for past dates too).
pub fn local_text(secs: u64) -> String {
    let utc = safety::format_utc_parts(secs);
    let st = SystemTime { year: utc.0, month: utc.1, day: utc.2, hour: utc.3, minute: utc.4, second: utc.5, ..Default::default() };
    let mut out = SystemTime::default();
    // SAFETY: both pointers point to live, correctly laid out SYSTEMTIMEs.
    let ok = unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &st, &mut out) } != 0;
    let t = if ok { out } else { st };
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let month = M.get(usize::from(t.month).saturating_sub(1)).copied().unwrap_or("?");
    format!("{} {month} {}, {:02}:{:02}{}", t.day, t.year, t.hour, t.minute, if ok { "" } else { " UTC" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_log_times() {
        assert_eq!(parse_utc("1970-01-01 00:00:00"), Some(0));
        assert_eq!(parse_utc("2025-11-10 18:40:00"), Some(1_762_800_000));
        assert_eq!(parse_utc("2000-02-29 00:00:00"), Some(951_782_400));
        assert_eq!(parse_utc("garbage"), None);
        for s in [0u64, 951_782_400, 1_762_800_000, 1_800_000_123] {
            assert_eq!(parse_utc(&safety::format_utc(s)), Some(s));
        }
    }

    #[test]
    fn local_text_looks_like_a_date() {
        let t = local_text(1_762_800_000);
        assert!(t.contains("2025") && t.contains(':'), "{t}");
    }

    #[test]
    fn merges_both_logs_newest_first() {
        let _ = std::fs::remove_file(log_path().unwrap());
        append(&Logged { time: 100, kind: "compressed".into(), path: r"C:\a".into(), files: 3, before: 1000, after: 400, note: "Fast".into() });
        append(&Logged { time: 300, kind: "uncompressed".into(), path: r"C:\b".into(), files: 1, before: 10, after: 50, note: String::new() });
        let v = read(10);
        let mine: Vec<&Entry> = v.iter().filter(|e| e.path == r"C:\a" || e.path == r"C:\b").collect();
        assert_eq!(mine.len(), 2);
        assert_eq!(mine[0].kind, "uncompressed");
        assert_eq!(mine[1].bytes, 600, "saved = before - after");
        let _ = std::fs::remove_file(log_path().unwrap());
    }
}
