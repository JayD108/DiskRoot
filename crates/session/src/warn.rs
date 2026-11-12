//! Early warnings after a scheduled scan: low free space, or a drive or
//! folder that grew fast. Quiet on purpose:
//! - only after background scans (in the window you see it anyway);
//! - at most one per reason per day (`%LOCALAPPDATA%\Xylem\warnings.json`);
//! - shown as one Windows notification from the exiting scan. Its icon
//!   goes away with it: nothing stays in the tray, nothing keeps running.

use std::collections::HashMap;
use std::ffi::c_void;
use std::fs;
use std::path::PathBuf;

use crate::history::{self, History};

const GB: u64 = 1 << 30;
const DAY: u64 = 86_400;
/// "Once a day", with slack so a daily scan that runs a bit early still warns.
const QUIET: u64 = 23 * 3600;

/// One warning: a stable key (for the once-a-day rule) and its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    pub key: String,
    pub text: String,
}

/// Which warnings apply after a scan of `root`. `vol` = (size, free) of its
/// volume; `hist` = its size history (for growth over the last day).
/// `free_gb` / `grow_gb`: thresholds, 0 = off.
pub fn check(root: &str, vol: Option<(u64, u64)>, hist: Option<&History>, free_gb: u32, grow_gb: u32) -> Vec<Warning> {
    let mut out = Vec::new();
    let drive = root.get(..2).unwrap_or(root).to_uppercase();
    if free_gb > 0
        && let Some((_, free)) = vol
        && free < u64::from(free_gb) * GB
    {
        out.push(Warning {
            key: format!("free:{drive}"),
            text: format!("{drive} has only {} free. Open Xylem to see what takes the space.", xylem_core::format::bytes(free)),
        });
    }
    if grow_gb > 0
        && let Some(h) = hist
        && let Some(last) = h.points.last()
        && let Some(base) = h.points.iter().rev().find(|p| p.time.saturating_add(DAY) <= last.time && last.time.saturating_sub(p.time) <= 3 * DAY)
    {
        let grew = last.total.saturating_sub(base.total);
        if grew > u64::from(grow_gb) * GB {
            let top = h
                .growers(DAY, 10)
                .and_then(|c| c.items.into_iter().find(|i| i.delta() > 0 && !i.path.is_empty()))
                .map(|c| format!(", most of it in {}", c.path))
                .unwrap_or_default();
            out.push(Warning {
                key: format!("grow:{}", crate::remember::key(root)),
                text: format!("{} grew by {} in the last day{top}.", root.trim_end_matches('\\'), xylem_core::format::bytes(grew)),
            });
        }
    }
    out
}

fn state_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Xylem").join("warnings.json"))
}

/// Keeps only the warnings not shown in the last day, and remembers them
/// as shown at `now`.
pub fn due(found: Vec<Warning>, last: &mut HashMap<String, u64>, now: u64) -> Vec<Warning> {
    // The weekly report: once a week (with an hour of slack), others once a day.
    let quiet = |key: &str| if key.starts_with("weekly") { 7 * DAY - 3600 } else { QUIET };
    let due: Vec<Warning> = found.into_iter().filter(|w| last.get(&w.key).is_none_or(|&t| now.saturating_sub(t) >= quiet(&w.key) || t > now)).collect();
    for w in &due {
        last.insert(w.key.clone(), now);
    }
    // Forget reasons not seen for a month.
    last.retain(|_, t| now.saturating_sub(*t) < 30 * DAY);
    due
}

/// After a scheduled scan of `root`: check, apply the once-a-day rule,
/// notify. Returns what was shown (for the run log).
pub fn after_scan(root: &str, free_gb: u32, grow_gb: u32, now: u64) -> Option<String> {
    let hist = (grow_gb > 0).then(|| history::load(root)).flatten();
    let found = check(root, crate::disk_space(root), hist.as_ref(), free_gb, grow_gb);
    show_due(found, now)
}

/// The weekly drive report's text: growth of each scanned root over the
/// last week, writes per day and health of each NVMe drive.
pub fn weekly_text(growth: &[(String, i128)], drives: &[(String, Option<f64>, String)]) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for (root, d) in growth {
        let sign = if *d >= 0 { "+" } else { "−" };
        parts.push(format!("{} {sign}{} this week", root.trim_end_matches('\\'), xylem_core::format::bytes(d.unsigned_abs().min(u128::from(u64::MAX)) as u64)));
    }
    for (letters, per_day, verdict) in drives {
        let w = per_day.map(|b| format!(", about {} written a day", xylem_core::format::bytes(b as u64))).unwrap_or_default();
        parts.push(format!("drive {letters} ({}){w}", verdict.trim_end_matches('.')));
    }
    (!parts.is_empty()).then(|| format!("Weekly report: {}.", parts.join(" · ")))
}

/// After a scheduled run, if the weekly report is on: at most once a week.
pub fn weekly(roots: &[String], now: u64) -> Option<String> {
    let growth: Vec<(String, i128)> = roots
        .iter()
        .filter_map(|r| {
            let h = history::load(r)?;
            let g = h.growers(7 * DAY, 1)?;
            Some((r.clone(), i128::from(g.after_total) - i128::from(g.before_total)))
        })
        .collect();
    let drives = crate::health::drives();
    let snaps = crate::health::snaps();
    let info: Vec<(String, Option<f64>, String)> = drives
        .iter()
        .filter(|d| d.nvme.is_some())
        .map(|d| (if d.letters.is_empty() { d.model.clone() } else { d.letters.join(" ") }, crate::health::trend(&snaps, d).and_then(|t| t.per_day), d.verdict().1))
        .collect();
    let text = weekly_text(&growth, &info)?;
    show_due(vec![Warning { key: "weekly".into(), text }], now)
}

/// Applies the once-a-day / once-a-week rule, notifies, returns what was shown.
fn show_due(found: Vec<Warning>, now: u64) -> Option<String> {
    if found.is_empty() {
        return None;
    }
    let path = state_path()?;
    let mut last: HashMap<String, u64> = fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let shown = due(found, &mut last, now);
    if let Some(d) = path.parent() {
        let _ = fs::create_dir_all(d);
    }
    let _ = fs::write(&path, serde_json::to_string(&last).unwrap_or_default());
    if shown.is_empty() {
        return None;
    }
    let text = shown.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join("\n");
    let _ = notify("Xylem", &text);
    Some(text)
}

// ---------------------------------------------------------------------------
// One Windows notification (Shell_NotifyIcon balloon; Windows 10/11 show it
// as a normal notification). A hidden window owns the icon for ~10 s.
// ---------------------------------------------------------------------------

#[repr(C)]
struct NotifyIconData {
    size: u32,
    wnd: *mut c_void,
    id: u32,
    flags: u32,
    callback: u32,
    icon: *mut c_void,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    timeout_or_version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid: [u8; 16],
    balloon_icon: *mut c_void,
}

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateWindowExW(ex: u32, class: *const u16, name: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: *mut c_void, menu: *mut c_void, inst: *mut c_void, param: *mut c_void) -> *mut c_void;
    fn DestroyWindow(w: *mut c_void) -> i32;
    fn LoadIconW(inst: *mut c_void, name: *const u16) -> *mut c_void;
}
#[link(name = "shell32", kind = "raw-dylib")]
unsafe extern "system" {
    fn Shell_NotifyIconW(msg: u32, data: *mut NotifyIconData) -> i32;
}
#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
}

fn fill<const N: usize>(dst: &mut [u16; N], s: &str) {
    for (d, c) in dst.iter_mut().zip(s.encode_utf16().take(N - 1)) {
        *d = c;
    }
}

/// Shows one notification and waits ~10 s so Windows can show it, then
/// removes the icon. Only called from a scheduled scan that is exiting.
pub fn notify(title: &str, text: &str) -> Result<(), String> {
    const NIM_ADD: u32 = 0;
    const NIM_DELETE: u32 = 2;
    const NIF_ICON: u32 = 0x2;
    const NIF_TIP: u32 = 0x4;
    const NIF_INFO: u32 = 0x10;
    const NIIF_WARNING: u32 = 0x2;
    const NIIF_LARGE_ICON: u32 = 0x20;
    let class: Vec<u16> = "STATIC".encode_utf16().chain([0]).collect();
    let name: Vec<u16> = "Xylem".encode_utf16().chain([0]).collect();
    // SAFETY: a plain hidden STATIC window (never shown), destroyed below.
    let wnd = unsafe { CreateWindowExW(0, class.as_ptr(), name.as_ptr(), 0, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
    if wnd.is_null() {
        return Err("couldn't create the notification window".into());
    }
    // The exe's own icon (resource 1), else Windows' warning icon.
    // SAFETY: MAKEINTRESOURCE-style ids (a small integer as the pointer)
    // are valid "names" for LoadIconW.
    let res = |id: usize| std::ptr::without_provenance::<u16>(id);
    let mut icon = unsafe { LoadIconW(GetModuleHandleW(std::ptr::null()), res(1)) };
    if icon.is_null() {
        icon = unsafe { LoadIconW(std::ptr::null_mut(), res(32515)) }; // IDI_WARNING
    }
    let mut d = NotifyIconData {
        size: size_of::<NotifyIconData>() as u32,
        wnd,
        id: 1,
        flags: NIF_ICON | NIF_TIP | NIF_INFO,
        callback: 0,
        icon,
        tip: [0; 128],
        state: 0,
        state_mask: 0,
        info: [0; 256],
        timeout_or_version: 10_000,
        info_title: [0; 64],
        info_flags: NIIF_WARNING | NIIF_LARGE_ICON,
        guid: [0; 16],
        balloon_icon: std::ptr::null_mut(),
    };
    fill(&mut d.tip, "Xylem");
    fill(&mut d.info, text);
    fill(&mut d.info_title, title);
    // SAFETY: `d` is a fully initialised NOTIFYICONDATAW with its size set.
    let ok = unsafe { Shell_NotifyIconW(NIM_ADD, &mut d) } != 0;
    if ok {
        std::thread::sleep(std::time::Duration::from_secs(10));
        // SAFETY: same struct, same window and id.
        unsafe { Shell_NotifyIconW(NIM_DELETE, &mut d) };
    }
    unsafe { DestroyWindow(wnd) };
    if ok { Ok(()) } else { Err("Windows didn't accept the notification".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remember::{SnapDir, Snapshot};

    fn hist(sizes: &[(u64, u64, &str, u64)]) -> History {
        // (time, root total, folder, folder size)
        let mut h = History { root: "C:\\".into(), ..History::default() };
        for &(t, total, dir, b) in sizes {
            h.add(
                &Snapshot {
                    time: t,
                    root: "C:\\".into(),
                    mode: "folder walk".into(),
                    dirs: vec![SnapDir { path: String::new(), bytes: total, files: 1 }, SnapDir { path: dir.into(), bytes: b, files: 1 }],
                },
                None,
            );
        }
        h
    }

    #[test]
    fn low_free_space_warns_once_a_day() {
        let w = check("C:\\", Some((500 * GB, 7 * GB)), None, 10, 0);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].key, "free:C:");
        assert!(w[0].text.starts_with("C: has only 7.00 GB free"), "{}", w[0].text);
        assert!(check("C:\\", Some((500 * GB, 70 * GB)), None, 10, 0).is_empty());
        assert!(check("C:\\", Some((500 * GB, 1)), None, 0, 0).is_empty(), "0 = off");
        assert!(check("C:\\", None, None, 10, 0).is_empty(), "unknown space: quiet");
        let mut last = HashMap::new();
        assert_eq!(due(w.clone(), &mut last, 1_000_000).len(), 1);
        assert!(due(w.clone(), &mut last, 1_000_000 + 3600).is_empty(), "an hour later: quiet");
        assert_eq!(due(w.clone(), &mut last, 1_000_000 + DAY).len(), 1, "a day later: again");
        assert_eq!(due(w, &mut last, 10), vec![check("C:\\", Some((500 * GB, 7 * GB)), None, 10, 0)[0].clone()], "clock went back: not stuck");
    }

    #[test]
    fn fast_growth_warns_with_the_folder() {
        let h = hist(&[(0, 100 * GB, "Users\\me\\Downloads", GB), (DAY / 2, 110 * GB, "Users\\me\\Downloads", 11 * GB), (DAY + 60, 130 * GB, "Users\\me\\Downloads", 31 * GB)]);
        let w = check("C:\\", None, Some(&h), 0, 20);
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(w[0].text.contains("grew by 30.0 GB in the last day, most of it in Users\\me\\Downloads"), "{}", w[0].text);
        assert!(check("C:\\", None, Some(&h), 0, 50).is_empty(), "under the threshold");
        assert!(check("C:\\", None, Some(&h), 0, 0).is_empty(), "off");
        // Less than a day of history: no growth warning yet.
        let short = hist(&[(0, 100 * GB, "a", GB), (3600, 150 * GB, "a", 51 * GB)]);
        assert!(check("C:\\", None, Some(&short), 0, 5).is_empty());
        // The only older scan is a week old: not "in the last day".
        let old = hist(&[(0, 100 * GB, "a", GB), (7 * DAY, 150 * GB, "a", 51 * GB)]);
        assert!(check("C:\\", None, Some(&old), 0, 5).is_empty());
    }

    #[test]
    fn weekly_report_reads_plainly_and_comes_once_a_week() {
        let t = weekly_text(&[("C:\\".into(), i128::from(12 * GB))], &[("C:".into(), Some(38.0 * GB as f64), "Good: 1% of rated life used.".into())]).unwrap();
        assert_eq!(t, "Weekly report: C: +12.0 GB this week · drive C: (Good: 1% of rated life used), about 38.0 GB written a day.");
        assert!(weekly_text(&[], &[]).is_none());
        let w = vec![Warning { key: "weekly".into(), text: t }];
        let mut last = HashMap::new();
        assert_eq!(due(w.clone(), &mut last, 1_000_000).len(), 1);
        assert!(due(w.clone(), &mut last, 1_000_000 + 3 * DAY).is_empty(), "3 days later: quiet");
        assert_eq!(due(w, &mut last, 1_000_000 + 7 * DAY).len(), 1, "a week later: again");
    }

    /// Shows one real notification. Run by hand: cargo test -- --ignored real_notification
    #[test]
    #[ignore = "shows a real Windows notification"]
    fn real_notification() {
        notify("Xylem", "Test notification from Xylem's tests. Nothing is wrong.").unwrap();
    }
}
