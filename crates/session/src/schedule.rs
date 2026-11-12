//! Scheduled scans: a normal (no-admin) folder walk that Windows' Task
//! Scheduler starts every N minutes (at least 30) for the signed-in user.
//!
//! - **No admin, ever:** the task is created by the user for the user
//!   (`LeastPrivilege`); it runs `diskroot.exe --scheduled-scan` without a
//!   window. Fast scan (admin) stays something you start in the window.
//! - **Nothing stays running:** each run scans, saves the result exactly
//!   like the app does (instant reopen, "what changed"), records one line in
//!   a small log (time, duration, peak RAM, change), and exits. Idle RAM: 0.
//! - **Gentle:** the whole run is in Windows' background mode (low CPU, disk
//!   and memory priority); not on battery unless allowed; one run at a time.
//! - **Read-only** like every scan; it writes only DiskRoot's own files in
//!   `%APPDATA%\DiskRoot` and `%LOCALAPPDATA%\DiskRoot`.

use std::ffi::c_void;
use std::fs;
use std::io::Write;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use diskroot_scan_mft::{Mode, scan_auto};

use crate::remember;

/// Shortest allowed interval between automatic scans.
pub const MIN_MINUTES: u32 = 30;
/// The intervals offered in the app.
pub const CHOICES: [u32; 7] = [30, 60, 120, 240, 360, 720, 1440];
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const PROCESS_MODE_BACKGROUND_BEGIN: u32 = 0x0010_0000;
const LOG_KEEP: usize = 500;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateMutexW(attrs: *mut c_void, initial: i32, name: *const u16) -> *mut c_void;
    fn GetLastError() -> u32;
    fn CloseHandle(h: *mut c_void) -> i32;
    fn GetCurrentProcess() -> *mut c_void;
    fn SetPriorityClass(p: *mut c_void, class: u32) -> i32;
}

/// What to scan and when. Saved in `%APPDATA%\DiskRoot\schedule.json`; the
/// Windows task only says "run diskroot.exe --scheduled-scan every N minutes".
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Schedule {
    pub every_min: u32,
    /// Drives or folders, e.g. "C:\\".
    pub roots: Vec<String>,
    /// Also run on battery power.
    pub on_battery: bool,
    /// Only when the PC has been idle for a few minutes.
    pub only_idle: bool,
    /// Warn when a scanned drive has less than this many GB free (0 = off).
    pub warn_free_gb: u32,
    /// Warn when a scanned root grew by more than this many GB in a day (0 = off).
    pub warn_grow_gb: u32,
    /// Once a week, a notification with growth, writes per day and drive health.
    pub weekly_report: bool,
    /// Warn when a program keeps writing at least this many GB a day while
    /// the PC is idle (0 = off).
    pub warn_idle_gb_day: u32,
}

impl Default for Schedule {
    fn default() -> Self {
        Self { every_min: 120, roots: vec!["C:\\".into()], on_battery: false, only_idle: false, warn_free_gb: 10, warn_grow_gb: 0, weekly_report: false, warn_idle_gb_day: 10 }
    }
}

/// One background run, as logged.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Run {
    /// Unix seconds when the scan finished.
    pub time: u64,
    pub root: String,
    pub secs: f64,
    /// Peak memory the run needed (peak private bytes), bytes.
    pub peak_ram: u64,
    pub files: u64,
    /// Change in size on disk since the previous remembered scan.
    pub delta: Option<i128>,
    pub error: Option<String>,
    /// A warning shown after this run (low space, fast growth).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
    /// Top writing programs in the 30 s before the scan (first root of a run only).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writers: Vec<crate::writes::Writer>,
    /// No key or mouse input during that sample (for 5+ minutes).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idle: bool,
}

fn roaming() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("DiskRoot"))
}
fn local() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("DiskRoot"))
}
fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The saved schedule (what the task scans), if any.
pub fn load() -> Option<Schedule> {
    parse(&fs::read_to_string(roaming()?.join("schedule.json")).ok()?)
}

fn parse(text: &str) -> Option<Schedule> {
    // Notepad and PowerShell may add a byte-order mark: accept it.
    let mut s: Schedule = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    s.every_min = s.every_min.max(MIN_MINUTES);
    s.roots.retain(|r| !r.trim().is_empty());
    Some(s)
}

/// Saves what to scan (used by the task and by "Scan once now").
pub fn save(s: &Schedule) -> Result<(), String> {
    let dir = roaming().ok_or("APPDATA isn't set")?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    fs::write(dir.join("schedule.json"), text).map_err(|e| e.to_string())
}

/// The Windows task's name, one per user (task names are machine-wide).
pub fn task_name() -> String {
    let user: String = std::env::var("USERNAME").unwrap_or_default().chars().filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.')).collect();
    if user.is_empty() { "DiskRoot scheduled scan".into() } else { format!("DiskRoot scheduled scan ({user})") }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// The Task Scheduler definition. Runs as the signed-in user, least
/// privilege, one instance at a time, below-normal priority, at most 1 h.
pub fn task_xml(s: &Schedule, exe: &Path) -> String {
    let every = s.every_min.max(MIN_MINUTES);
    let battery = if s.on_battery { "false" } else { "true" };
    let idle = if s.only_idle {
        "<RunOnlyIfIdle>true</RunOnlyIfIdle><IdleSettings><Duration>PT5M</Duration><WaitTimeout>PT1H</WaitTimeout><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>"
    } else {
        "<RunOnlyIfIdle>false</RunOnlyIfIdle>"
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>DiskRoot: a normal, read-only scan every {every} minutes (no admin). Turn it off in DiskRoot: View &gt; Scheduled scans.</Description></RegistrationInfo>
  <Triggers><TimeTrigger><StartBoundary>2026-01-01T00:00:00</StartBoundary><Enabled>true</Enabled><Repetition><Interval>PT{every}M</Interval><StopAtDurationEnd>false</StopAtDurationEnd></Repetition></TimeTrigger></Triggers>
  <Principals><Principal id="Author"><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>{battery}</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>{battery}</StopIfGoingOnBatteries><StartWhenAvailable>true</StartWhenAvailable>{idle}<ExecutionTimeLimit>PT1H</ExecutionTimeLimit><Priority>7</Priority></Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command><Arguments>--scheduled-scan</Arguments></Exec></Actions>
</Task>
"#,
        exe = xml_escape(&exe.display().to_string()),
    )
}

fn schtasks(args: &[&str]) -> Result<String, String> {
    let out = Command::new(crate::tools::system32("schtasks.exe")).args(args).creation_flags(CREATE_NO_WINDOW).output().map_err(|e| format!("Couldn't start schtasks: {e}"))?;
    let text = String::from_utf8_lossy(if out.status.success() { &out.stdout } else { &out.stderr }).trim().to_owned();
    if out.status.success() { Ok(text) } else { Err(text) }
}

/// Saves the schedule and creates (or replaces) the user's task for `exe`.
pub fn install(s: &Schedule, exe: &Path) -> Result<(), String> {
    if s.roots.is_empty() {
        return Err("Pick at least one drive or folder to scan.".into());
    }
    let s = Schedule { every_min: s.every_min.max(MIN_MINUTES), ..s.clone() };
    save(&s)?;
    let dir = local().ok_or("LOCALAPPDATA isn't set")?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // Task Scheduler wants the XML as UTF-16 with a byte-order mark.
    let xml_path = dir.join("scheduled-scan-task.xml");
    let mut bytes = vec![0xFF, 0xFE];
    bytes.extend(task_xml(&s, exe).encode_utf16().flat_map(u16::to_le_bytes));
    fs::write(&xml_path, bytes).map_err(|e| e.to_string())?;
    let r = schtasks(&["/create", "/tn", &task_name(), "/xml", &xml_path.display().to_string(), "/f"]);
    let _ = fs::remove_file(&xml_path);
    r.map(|_| ())
}

/// The task the previous version (before the rename) made.
fn old_task_name() -> String {
    task_name().replacen("DiskRoot", crate::PREVIOUS_NAME, 1)
}

/// Removes the previous version's task (it runs the old program). Returns whether
/// there was one.
pub fn remove_old_task() -> bool {
    schtasks(&["/query", "/tn", &old_task_name()]).is_ok() && schtasks(&["/delete", "/tn", &old_task_name(), "/f"]).is_ok()
}

/// Whether the user's task exists.
pub fn installed() -> bool {
    schtasks(&["/query", "/tn", &task_name()]).is_ok()
}

/// Removes the user's task (the saved schedule and log stay).
pub fn remove() -> Result<(), String> {
    if !installed() {
        return Ok(());
    }
    schtasks(&["/delete", "/tn", &task_name(), "/f"]).map(|_| ())
}

/// Starts one background run now (no window), like the task would.
pub fn run_now(exe: &Path) -> Result<(), String> {
    Command::new(exe).arg("--scheduled-scan").creation_flags(CREATE_NO_WINDOW).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// A suggested interval and why. Two signals: what a scan costs (keep
/// background scanning under ~0.1% of the PC's time) and how fast the
/// drive changes (no point scanning hourly if little changes in a day).
/// `secs`: duration of a normal scan; `bytes_per_day`: average change.
/// SSD writes DiskRoot allows itself for background scans: about 1 GB a day.
const WRITE_BUDGET_PER_DAY: f64 = 1024.0 * 1024.0 * 1024.0;

/// `saved`: bytes one run writes to save its result (the compressed
/// remembered scan, snapshot and history). Background scans keep DiskRoot's
/// own SSD writes under ~1 GB a day.
pub fn recommend(secs: Option<f64>, bytes_per_day: Option<f64>, saved: Option<u64>) -> (u32, String) {
    let snap = |m: f64| *CHOICES.iter().find(|&&c| f64::from(c) >= m).unwrap_or(&1440);
    let Some(secs) = secs.filter(|s| *s > 0.0) else {
        return (120, "Every 2 hours is a good start; DiskRoot refines this after a few scans.".into());
    };
    // 0.1% duty cycle: a 7 s scan every ~2 h.
    let by_cost = snap((secs * 1000.0 / 60.0).max(f64::from(MIN_MINUTES)));
    let gb = 1024.0 * 1024.0 * 1024.0;
    let (by_change, why) = match bytes_per_day {
        Some(b) if b >= 20.0 * gb => (60, "it changes a lot (over 20 GB a day)"),
        Some(b) if b >= 2.0 * gb => (240, "it changes a few GB a day"),
        Some(_) => (720, "it changes little from day to day"),
        None => (by_cost, ""),
    };
    let by_writes = saved.map_or(MIN_MINUTES, |b| snap((b as f64 * 1440.0 / WRITE_BUDGET_PER_DAY).max(f64::from(MIN_MINUTES))));
    let m = by_cost.max(by_change).max(by_writes);
    let every = |m: u32| if m < 60 { format!("{m} minutes") } else if m == 60 { "hour".into() } else if m < 1440 { format!("{} hours", m / 60) } else { "day".into() };
    let mut reason = format!("A scan takes about {secs:.0} s, so every {} keeps background scanning under 0.1% of your PC's time", every(m));
    if !why.is_empty() {
        reason.push_str(&format!(", and {why}"));
    }
    reason.push('.');
    if let Some(b) = saved.filter(|&b| b > 0) {
        let per_day = b as f64 * 1440.0 / f64::from(m);
        reason.push_str(&format!(
            " Each run saves about {} on this PC (about {} of SSD writes a day at this pace).",
            diskroot_core::format::bytes(b),
            diskroot_core::format::bytes(per_day as u64)
        ));
    }
    (m, reason)
}

fn log_path() -> Option<PathBuf> {
    Some(local()?.join("scheduled-scans.jsonl"))
}

/// Logged runs, oldest first (at most the last 500).
pub fn runs() -> Vec<Run> {
    let Some(text) = log_path().and_then(|p| fs::read_to_string(p).ok()) else { return Vec::new() };
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

fn append_run(r: &Run) {
    let Some(p) = log_path() else { return };
    if let Some(d) = p.parent() {
        let _ = fs::create_dir_all(d);
    }
    let mut all = runs();
    all.push(r.clone());
    if all.len() > LOG_KEEP {
        all.drain(..all.len() - LOG_KEEP);
    }
    let text: String = all.iter().filter_map(|r| serde_json::to_string(r).ok()).map(|l| l + "\n").collect();
    let tmp = p.with_extension("tmp");
    if fs::File::create(&tmp).and_then(|mut f| f.write_all(text.as_bytes())).is_ok() {
        let _ = fs::rename(&tmp, &p);
    }
}

/// `diskroot.exe --scheduled-scan`: one background run, then exit.
pub fn run_if_scheduled(args: &[String]) -> Option<i32> {
    (args.get(1).map(String::as_str) == Some("--scheduled-scan")).then(run_scheduled)
}

fn run_scheduled() -> i32 {
    let _ = crate::migrate_previous();
    // One run at a time per user (a slow scan must not pile up).
    let name: Vec<u16> = "Local\\DiskRootScheduledScan".encode_utf16().chain([0]).collect();
    // SAFETY: NUL-terminated name; the handle is closed at the end.
    let mutex = unsafe { CreateMutexW(std::ptr::null_mut(), 1, name.as_ptr()) };
    if mutex.is_null() || unsafe { GetLastError() } == 183 {
        if !mutex.is_null() {
            unsafe { CloseHandle(mutex) };
        }
        return 0; // ERROR_ALREADY_EXISTS: another run is going
    }
    // SAFETY: pseudo-handle of this process. Background mode: low CPU, disk
    // and memory priority for the whole run.
    unsafe { SetPriorityClass(GetCurrentProcess(), PROCESS_MODE_BACKGROUND_BEGIN) };
    crate::set_gentle(true);
    let Some(s) = load() else {
        unsafe { CloseHandle(mutex) };
        return 2;
    };
    // Drive health: one small daily note (a few storage queries, no admin).
    crate::health::record_daily(&crate::health::drives());
    // Who writes while nobody is looking: 30 s, before DiskRoot itself reads anything.
    let (mut writers, idle) = crate::writes::sample_for(30, 5);
    let mut idle_warning = crate::warn::after_idle_sample(&runs(), &writers, idle, now_secs(), s.warn_idle_gb_day);
    for root in &s.roots {
        let t = Instant::now();
        let r = scan_and_remember(root);
        // Peak committed memory: background mode trims the working set, so its
        // peak would understate what the scan needed.
        let peak_ram = crate::process::peak_private().unwrap_or(0);
        let secs = t.elapsed().as_secs_f64();
        let (files, delta, error) = match r {
            Ok((f, d)) => (f, d, None),
            Err(e) => (0, None, Some(e)),
        };
        let warning = if error.is_none() { crate::warn::after_scan(root, s.warn_free_gb, s.warn_grow_gb, now_secs()) } else { None };
        let warning = match (idle_warning.take(), warning) {
            (Some(a), Some(b)) => Some(format!("{a}
{b}")),
            (a, b) => a.or(b),
        };
        append_run(&Run { time: now_secs(), root: root.clone(), secs, peak_ram, files, delta, error, warning, writers: std::mem::take(&mut writers), idle });
    }
    if s.weekly_report {
        let _ = crate::warn::weekly(&s.roots, now_secs());
    }
    unsafe { CloseHandle(mutex) };
    0
}

/// A normal walk of `root`, saved like the app saves its scans. Keeps the
/// "last seen" baseline so the app can say what changed while you were away.
fn scan_and_remember(root: &str) -> Result<(u64, Option<i128>), String> {
    let path = crate::real_path(root)?;
    let (scan, _) = scan_auto(Path::new(&path), Mode::Walk, diskroot_scan_walk::default_threads()).map_err(|e| format!("Scan failed: {e}"))?;
    let files = scan.stats.files;
    let idx = Arc::new(scan.index);
    remember::keep_seen(idx.name(0));
    let changes = remember::remember(&idx, &scan.stats, "folder walk").map_err(|e| format!("Couldn't save the scan: {e}"))?;
    Ok((files, changes.map(|c| i128::from(c.after_total) - i128::from(c.before_total))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_definition_is_least_privilege_and_escaped() {
        let s = Schedule { every_min: 10, roots: vec!["C:\\".into()], on_battery: false, only_idle: true, ..Schedule::default() };
        let x = task_xml(&s, Path::new(r"C:\Program Files\X & Y\diskroot.exe"));
        assert!(x.contains("<Interval>PT30M</Interval>"), "never more often than every 30 minutes");
        assert!(x.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(x.contains("<DisallowStartIfOnBatteries>true</DisallowStartIfOnBatteries>"));
        assert!(x.contains("<RunOnlyIfIdle>true</RunOnlyIfIdle>"));
        assert!(x.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"));
        assert!(x.contains(r"<Command>C:\Program Files\X &amp; Y\diskroot.exe</Command>"));
        assert!(x.contains("<Arguments>--scheduled-scan</Arguments>"));
        assert!(!x.contains("HighestAvailable"));
    }

    #[test]
    fn recommendations_respect_cost_change_and_the_minimum() {
        assert_eq!(recommend(Some(7.0), None, None).0, 120, "7 s scan: ~2 h for 0.1%");
        assert_eq!(recommend(Some(1.0), None, None).0, 30, "never below 30 minutes");
        assert_eq!(recommend(Some(60.0), None, None).0, 1440, "slow scans: daily");
        let gb = 1024.0 * 1024.0 * 1024.0;
        assert_eq!(recommend(Some(7.0), Some(0.3 * gb), None).0, 720, "little change: twice a day");
        assert_eq!(recommend(Some(7.0), Some(30.0 * gb), None).0, 120, "lots of change, but cost still decides the floor");
        assert_eq!(recommend(Some(1.0), Some(30.0 * gb), None).0, 60);
        assert_eq!(recommend(None, None, None).0, 120);
        assert!(recommend(Some(7.0), None, None).1.contains("7 s"));
        // Writes: a 45 MB save every 30 min would be ~2 GB a day; capped near 1 GB.
        let (m, why) = recommend(Some(1.0), Some(30.0 * gb), Some(45 << 20));
        assert_eq!(m, 120, "{why}");
        assert!(why.contains("Each run saves about 45.0 MB") && why.contains("540 MB of SSD writes a day"), "{why}");
        assert_eq!(recommend(Some(1.0), Some(30.0 * gb), Some(1 << 20)).0, 60, "tiny saves don't slow it down");
    }

    #[test]
    fn schedule_text_with_a_byte_order_mark_parses() {
        let text = "\u{feff}{\"every_min\": 5, \"roots\": [\"D:\\\\\", \" \"]}";
        let s = parse(text).expect("parses");
        assert_eq!(s.every_min, MIN_MINUTES, "clamped to 30");
        assert_eq!(s.roots, vec!["D:\\".to_string()], "blank roots dropped");
        assert!(parse("not json").is_none());
    }

    /// Creates the real per-user task (pointing at this test binary, never
    /// run), checks Windows has it with least privilege, and removes it.
    /// `cargo test -p diskroot-session real_task -- --ignored --test-threads=1`
    #[test]
    #[ignore = "creates and deletes a real scheduled task for this user"]
    fn real_task_install_and_remove() {
        let dir = std::env::temp_dir().join(format!("diskroot-sched-real-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY (test only, run with --test-threads=1): nothing else reads these meanwhile.
        unsafe {
            std::env::set_var("APPDATA", dir.join("r"));
            std::env::set_var("LOCALAPPDATA", dir.join("l"));
        }
        let exe = std::env::current_exe().unwrap();
        let s = Schedule { every_min: 240, roots: vec!["C:\\".into()], on_battery: false, only_idle: false, ..Schedule::default() };
        install(&s, &exe).expect("created without admin");
        let was_installed = installed();
        let q = schtasks(&["/query", "/tn", &task_name(), "/xml"]).unwrap_or_default();
        let loaded = load();
        let xml_left = dir.join("l").join("DiskRoot").join("scheduled-scan-task.xml").exists();
        remove().unwrap(); // before the checks, so a failure never leaves a task behind
        assert!(was_installed);
        // Windows omits RunLevel when it's the default (least privilege).
        assert!(!q.contains("HighestAvailable") && q.contains("PT4H") && q.contains("--scheduled-scan"), "{q}");
        assert_eq!(loaded.unwrap(), s);
        assert!(!xml_left, "temp XML removed");
        assert!(!installed());
        remove().expect("removing twice is fine");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn task_name_is_per_user_and_safe() {
        let n = task_name();
        assert!(n.starts_with("DiskRoot scheduled scan"));
        assert!(!n.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']));
    }
}
