//! UI-agnostic engine API used by the DiskRoot app (`diskroot.exe`) and kept
//! separate from the UI so it can be tested on its own.
//!
//! A [`Scanned`] holds one scan: the tree, its classification and category
//! totals. Every UI question ("what's inside this folder", "draw this
//! folder", "is this safe to delete") is a method here, so the UI never
//! holds the full tree.

pub mod cachefile;
pub mod compress;
pub mod dupes;
pub mod history;
pub mod similar;
pub mod savings;
pub mod schedule;
pub mod elevate;
pub mod health;
pub mod helper;
pub mod integrity;
pub mod lockdown;
pub mod logs;
pub mod recycle;
pub mod remember;
pub mod safety;
pub mod tools;
pub mod offenders;
pub mod orphans;
pub mod owners;
pub mod power;
pub mod warn;
pub mod watch;
pub mod writes;

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Instant;

use serde::Serialize;
use diskroot_core::classify::{CATEGORIES, CatCrumb, CatKey, CatModel, CatStats, Classification, NCAT, Rules};
use diskroot_core::view::{Children, ViewLimits, ViewNode, ancestors, build_view};
use diskroot_core::{Index, NO_PARENT, ScanStats, flags};
use diskroot_scan_mft::{Mode, Used, scan_auto};

pub use diskroot_core::classify::{Action, Category, RuleInfo, Safety, SubType};
pub use diskroot_core::view::ViewNode as Shape;
pub use diskroot_core::{AGE_BUCKETS, AGE_UNKNOWN, age_bucket, age_text, date_text, today_days};
/// This process's RAM and CPU (for the app's own footprint display).
pub use diskroot_scan_walk::sys as process;

/// Gentle scanning for the folder walk: Windows' background mode (low CPU,
/// disk and memory priority). Measured on C: (E11): another program's 2 GB
/// read took 3.0 s alone, 5.7 s during a normal scan, 3.0 s during a gentle
/// one; the scan took ~8% longer.
pub fn set_gentle(on: bool) {
    diskroot_scan_walk::GENTLE.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// The rules in effect: the built-in ones plus the user's own
/// (`%APPDATA%\DiskRoot\rules.json`). Reloadable while the app runs; each
/// scan keeps the rules it was classified with.
static CURRENT_RULES: LazyLock<RwLock<Arc<Rules>>> = LazyLock::new(|| RwLock::new(Arc::new(load_rules().0)));

/// The rules new scans are classified with.
pub fn rules() -> Arc<Rules> {
    Arc::clone(&CURRENT_RULES.read().unwrap_or_else(std::sync::PoisonError::into_inner))
}

/// Where the user's own rules live.
pub fn user_rules_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("DiskRoot").join("rules.json"))
}

/// What loading the user's rules gave.
#[derive(Clone, Debug, Default)]
pub struct RulesStatus {
    pub path: Option<PathBuf>,
    /// The file exists.
    pub found: bool,
    pub user_rules: usize,
    /// Why the file was ignored (the built-in rules are used alone then).
    pub error: Option<String>,
}

fn load_rules() -> (Rules, RulesStatus) {
    let mut st = RulesStatus { path: user_rules_path(), ..Default::default() };
    let text = st.path.as_ref().and_then(|p| std::fs::read(p).ok());
    let Some(bytes) = text else { return (Rules::builtin(), st) };
    st.found = true;
    let rules = String::from_utf8(bytes)
        .map_err(|_| "the file isn't UTF-8 text".to_owned())
        .and_then(|t| Rules::builtin().with_user_rules(t.trim_start_matches('\u{feff}')));
    match rules {
        Ok(r) => {
            st.user_rules = r.user_rules();
            (r, st)
        }
        Err(e) => {
            st.error = Some(e);
            (Rules::builtin(), st)
        }
    }
}

/// Re-reads the user's rules; later scans (and [`Scanned::reclassify`]) use them.
pub fn reload_rules() -> RulesStatus {
    let (r, st) = load_rules();
    *CURRENT_RULES.write().unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(r);
    st
}

/// Creates an example rules file if there is none, and returns its path.
pub fn create_example_rules() -> Result<PathBuf, String> {
    let p = user_rules_path().ok_or("APPDATA isn't set")?;
    if !p.exists() {
        std::fs::create_dir_all(p.parent().expect("has a parent")).map_err(|e| e.to_string())?;
        std::fs::write(&p, EXAMPLE_RULES).map_err(|e| e.to_string())?;
    }
    Ok(p)
}

/// "This is…" choices for teaching DiskRoot a folder: (menu label, category, sub-type).
pub const TEACH: [(&str, &str, &str); 5] = [
    ("My project", "dev", "My projects"),
    ("A dataset", "ai", "My datasets"),
    ("A game", "apps", "My games"),
    ("Photos and videos I keep", "media", "My photos and videos"),
    ("Backups", "docs", "Backups"),
];

const TAUGHT: &str = "Taught in DiskRoot";

/// The rule pattern for a folder: its path below the drive, anchored
/// ("C:\Users\me\proj" -> "/Users/me/proj"; such a rule matches that path on
/// any drive).
pub fn teach_pattern(path: &str) -> Option<String> {
    let rest = path.get(2..).filter(|_| path.as_bytes().get(1) == Some(&b':'))?;
    let p = rest.replace('\\', "/").trim_end_matches('/').to_owned();
    (p.len() > 1).then_some(p)
}

fn edit_user_rules(p: &Path, f: impl FnOnce(&mut Vec<serde_json::Value>) -> Result<(), String>) -> Result<(), String> {
    if !p.exists() {
        std::fs::create_dir_all(p.parent().ok_or("no folder")?).map_err(|e| e.to_string())?;
        std::fs::write(p, EXAMPLE_RULES).map_err(|e| e.to_string())?;
    }
    let text = std::fs::read_to_string(p).map_err(|e| e.to_string())?;
    let mut v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("your rules file isn't valid JSON ({e}); fix it first (File > Custom rules)"))?;
    let rules = v.get_mut("rules").and_then(|r| r.as_array_mut()).ok_or("your rules file has no \"rules\" list")?;
    f(rules)?;
    let out = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    // The same limits the loader enforces; checked before writing.
    Rules::builtin().with_user_rules(&out).map_err(|e| format!("not saved: {e}"))?;
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, out).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, p).map_err(|e| e.to_string())
}

/// Adds (or replaces) a rule saying what the folder at `path` is. Only a
/// category: user rules never set safety or protection.
pub fn teach(path: &str, choice: usize) -> Result<(), String> {
    teach_at(&user_rules_path().ok_or("APPDATA isn't set")?, path, choice)
}

fn teach_at(file: &Path, path: &str, choice: usize) -> Result<(), String> {
    let (_, cat, sub) = *TEACH.get(choice).ok_or("unknown choice")?;
    let pattern = teach_pattern(path).ok_or("only folders on a drive can be taught")?;
    let today = date_text(today_days());
    edit_user_rules(file, |rules| {
        rules.retain(|r| r["match"].as_str().is_none_or(|m| !m.eq_ignore_ascii_case(&pattern)));
        rules.push(serde_json::json!({ "match": pattern, "kind": "dir", "cat": cat, "sub": sub, "note": format!("{TAUGHT} on {today}.") }));
        Ok(())
    })
}

/// Removes the taught rule for `path`, if there is one. Rules you wrote
/// by hand are left alone.
pub fn forget_taught(path: &str) -> Result<bool, String> {
    forget_taught_at(&user_rules_path().ok_or("APPDATA isn't set")?, path)
}

fn forget_taught_at(file: &Path, path: &str) -> Result<bool, String> {
    let pattern = teach_pattern(path).ok_or("not a folder on a drive")?;
    let mut removed = false;
    edit_user_rules(file, |rules| {
        let before = rules.len();
        rules.retain(|r| !(r["match"].as_str().is_some_and(|m| m.eq_ignore_ascii_case(&pattern)) && r["note"].as_str().is_some_and(|n| n.starts_with(TAUGHT))));
        removed = rules.len() != before;
        Ok(())
    })?;
    Ok(removed)
}

const EXAMPLE_RULES: &str = r#"{
  "_doc": [
    "Your own DiskRoot categories. Edit this file, then use File > Custom rules > Reload.",
    "match: a folder or file path pattern, '/' separated, case-insensitive.",
    "  '/Data/Projects' is anchored at the drive root; 'Projects/*/out' matches the end of any path.",
    "  '*' inside a part matches any text.",
    "kind: dir | file | any (default any).",
    "cat: apps | media | docs | dev | ai | system | temp | downloads | cloud | other.",
    "sub: your own sub-type name, shown in the Category view.",
    "Your rules can't change what DiskRoot protects: they don't apply where a built-in rule",
    "matches, nor inside protected places like C:\\Windows. Safety labels stay 'Unknown'."
  ],
  "rules": [
    { "match": "Projects/*/out", "kind": "dir", "cat": "dev", "sub": "My build output", "note": "Example: rebuilt by my build script." }
  ],
  "extensions": [
    { "ext": ["blend"], "cat": "media", "sub": "3D scenes" }
  ]
}
"#;

pub fn categories() -> &'static [Category] {
    &CATEGORIES
}

/// Items found so far by a running folder-walk scan (0 when idle).
pub fn scan_progress() -> u64 {
    diskroot_scan_walk::PROGRESS.load(std::sync::atomic::Ordering::Relaxed)
}

/// (total, free) bytes of the volume holding `path` (free = free for this user).
pub fn disk_space(path: &str) -> Option<(u64, u64)> {
    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, free_to_caller: *mut u64, total: *mut u64, total_free: *mut u64) -> i32;
    }
    let mut root = path.to_owned();
    if !root.ends_with('\\') {
        root.push('\\');
    }
    let w: Vec<u16> = root.encode_utf16().chain([0]).collect();
    let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
    // SAFETY: NUL-terminated path and three valid out pointers.
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, &mut total_free) } != 0;
    (ok && total > 0).then_some((total, free))
}

/// Drive roots that exist, e.g. `["C:\\", "D:\\"]`.
pub fn drives() -> Vec<String> {
    // GetDriveTypeW answers from the local mount table, so network drives
    // are skipped without touching the network (no SMB traffic at startup).
    ('A'..='Z')
        .map(|c| format!("{c}:\\"))
        .filter(|d| matches!(drive_type(d), 2 | 3 | 5 | 6)) // removable, fixed, CD, RAM disk
        .filter(|d| Path::new(d).exists())
        .collect()
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetDriveTypeW(root: *const u16) -> u32;
}

/// 0 unknown, 1 no root, 2 removable, 3 fixed, 4 remote, 5 CD-ROM, 6 RAM disk.
pub fn drive_type(path: &str) -> u32 {
    let root: String = if path.starts_with(r"\\") { return 4 } else { path.chars().take(2).chain("\\".chars()).collect() };
    let w: Vec<u16> = root.encode_utf16().chain([0]).collect();
    // SAFETY: NUL-terminated buffer that outlives the call.
    unsafe { GetDriveTypeW(w.as_ptr()) }
}

fn mode_text(used: &Used) -> String {
    match used {
        Used::Mft => "MFT (direct read)".into(),
        Used::Walk { fallback_reason: None } => "folder walk".into(),
        Used::Walk { fallback_reason: Some(r) } => format!("folder walk ({r})"),
    }
}

/// Scans a drive or folder in this process (always a folder walk; Fast scan
/// goes through the admin helper).
/// Blocking: call it from a worker thread.
pub fn scan(path: &str) -> Result<Scanned, String> {
    let path = real_path(path)?;
    let threads = diskroot_scan_walk::default_threads();
    // Always a folder walk in this process, even if it was started as admin:
    // reading the raw drive (MFT) happens only in the locked-down helper.
    let (s, used) = scan_auto(Path::new(&path), Mode::Walk, threads).map_err(|e| format!("Scan failed: {e}"))?;
    Ok(Scanned::new(s.index, s.stats, mode_text(&used)))
}

/// What moving from the old name (Xylem) did.
#[derive(Debug, Default)]
pub struct Migrated {
    /// Settings and saved scans were moved to the DiskRoot folders.
    pub moved: bool,
    /// The old scheduled-scan task was removed (it ran the old program).
    pub old_task_removed: bool,
    /// There's a saved schedule but no DiskRoot task yet: ask the user.
    pub schedule_to_restore: bool,
}

/// DiskRoot used to be called Xylem. Once: moves `%APPDATA%\Xylem` and
/// `%LOCALAPPDATA%\Xylem` to the DiskRoot folders (a rename, nothing is
/// copied; skipped if a DiskRoot folder already exists or the old one is
/// in use), and removes the old Xylem scheduled task.
pub fn migrate_from_xylem() -> Migrated {
    let mut m = Migrated::default();
    // Done once per user: a marker in DiskRoot's own folder.
    let marker = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("DiskRoot").join("migrated-from-xylem"));
    if marker.as_ref().is_some_and(|p| p.exists()) {
        return m;
    }
    for var in ["APPDATA", "LOCALAPPDATA"] {
        let Some(base) = std::env::var_os(var).map(PathBuf::from) else { continue };
        let (old, new) = (base.join("Xylem"), base.join("DiskRoot"));
        if old.is_dir() && !new.exists() && std::fs::rename(&old, &new).is_ok() {
            m.moved = true;
        }
    }
    m.old_task_removed = schedule::remove_old_task();
    m.schedule_to_restore = (m.moved || m.old_task_removed) && schedule::load().is_some() && !schedule::installed();
    // Only when nothing is left behind (an old folder in use moves next time).
    let left = ["APPDATA", "LOCALAPPDATA"].iter().any(|v| std::env::var_os(v).is_some_and(|d| PathBuf::from(d).join("Xylem").is_dir()));
    if let Some(p) = marker.filter(|_| !left) {
        let _ = std::fs::create_dir_all(p.parent().expect("has a parent"));
        let _ = std::fs::write(p, b"DiskRoot was called Xylem; its folders were moved here.
");
    }
    m
}

/// Whether this process runs with admin rights (an elevated token). The
/// window isn't meant to: it warns when someone starts it that way.
pub fn is_elevated() -> bool {
    #[link(name = "advapi32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn OpenProcessToken(p: *mut std::ffi::c_void, access: u32, token: *mut *mut std::ffi::c_void) -> i32;
        fn GetTokenInformation(t: *mut std::ffi::c_void, class: u32, info: *mut std::ffi::c_void, len: u32, ret: *mut u32) -> i32;
    }
    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn CloseHandle(h: *mut std::ffi::c_void) -> i32;
    }
    const TOKEN_QUERY: u32 = 0x8;
    const TOKEN_ELEVATION: u32 = 20;
    let mut tok = std::ptr::null_mut();
    // SAFETY: pseudo-handle of this process; out pointer valid; closed below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut tok) } == 0 {
        return false;
    }
    let mut elevated = 0u32;
    let mut ret = 0u32;
    // SAFETY: TOKEN_ELEVATION is one DWORD.
    let ok = unsafe { GetTokenInformation(tok, TOKEN_ELEVATION, (&raw mut elevated).cast(), 4, &mut ret) } != 0;
    unsafe { CloseHandle(tok) };
    ok && elevated != 0
}

/// The real, full path of what the user typed: short 8.3 names
/// (`C:\PROGRA~1`), `subst` drive letters, junctions and symlinks are
/// resolved. Protection rules match on folder names, so they must see the
/// real path, not whatever spelling was typed.
pub fn real_path(path: &str) -> Result<String, String> {
    let p = path.trim();
    // "C:" alone means "the current folder on C:", not the drive.
    let p = if p.len() == 2 && p.ends_with(':') { format!("{p}\\") } else { p.to_owned() };
    let c = std::fs::canonicalize(&p).map_err(|e| format!("Can't open \"{path}\": {e}"))?;
    let s = c.to_string_lossy();
    Ok(if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        s.into_owned()
    })
}

/// The remembered scan of `root` (instant reopen), and when it was made.
/// Blocking: call it from a worker thread.
pub fn reopen(root: &str) -> Option<(Scanned, u64)> {
    let (index, stats, mode, time) = remember::reopen(root)?;
    Some((Scanned::new(index, stats, mode), time))
}

/// Fast MFT scan through the elevated helper (shows the UAC prompt).
/// Blocking: call it from a worker thread.
pub fn scan_admin(drive: &str, cancel: &std::sync::atomic::AtomicBool) -> Result<Scanned, String> {
    // Fast scan runs this very exe with admin rights. A copy in a folder the
    // user can write to (Downloads, a portable zip) could be swapped by any
    // program running as the user, which would then get admin rights from
    // the user's "Yes". So release builds only elevate from Program Files.
    #[cfg(not(debug_assertions))]
    if !integrity::fast_scan_allowed() {
        return Err(integrity::FAST_SCAN_NEEDS_INSTALL.into());
    }
    let drive = real_path(drive)?;
    let (index, stats, mode) = elevate::scan_with_helper(&drive, cancel)?;
    Ok(Scanned::new(index, stats, mode))
}

/// "Where every GB is" for a whole drive.
#[derive(Clone, Debug, Serialize)]
pub struct SpaceReport {
    pub root: String,
    pub total: u64,
    /// What Windows reports as used (size minus free).
    pub used: u64,
    /// Bytes on disk of everything this scan counted.
    pub counted: u64,
    pub fast_scan: bool,
    /// NTFS's own files ($MFT, $LogFile, ...), if the scan could see them.
    pub ntfs_files: Option<u64>,
    /// System Volume Information (restore points, shadow copies), if readable.
    pub restore_points: Option<u64>,
    pub unreadable: u64,
    /// Extra names of hardlinked files (counted once, 0 B each).
    pub hardlink_names: u64,
}

impl SpaceReport {
    /// Used space the scan didn't find in any file it could read (negative:
    /// it counted more than Windows reports as used).
    pub fn not_counted(&self) -> i128 {
        i128::from(self.used) - i128::from(self.counted)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub root: String,
    pub mode: String,
    pub elapsed_ms: f64,
    pub classify_ms: f64,
    pub files: u64,
    pub dirs: u64,
    pub unreadable: u64,
    pub size: u64,
    pub on_disk: u64,
    /// Share of bytes in a real category (not Other).
    pub categorized: f64,
    pub admin_hint: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub id: u32,
    pub name: String,
    pub size: u64,
    pub on_disk: u64,
    pub files: u32,
    pub dir: bool,
    pub link: bool,
    pub cloud: bool,
    /// Colour category (dominant inside folders), own sub-type and rule.
    pub cat: u8,
    pub own_cat: u8,
    pub sub: u16,
    pub rule: u16,
    pub protected: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ListOut {
    /// All children of the folder (the list may be cut at `limit`).
    pub total: usize,
    pub items: Vec<Item>,
    /// Bytes per category inside this folder, biggest first.
    pub breakdown: Vec<(u8, u64)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Crumb {
    pub id: u32,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CatViewOut {
    pub nodes: Vec<ViewNode>,
    pub crumbs: Vec<CatCrumb>,
}

/// One row of the by-category tree: a category, a sub-type, or a place
/// (a real folder/file holding bytes of that sub-type).
#[derive(Clone, Debug, Serialize)]
pub struct CatRow {
    /// "cat:3" / "sub:12" for groups; empty for places.
    pub key: String,
    /// Index node of a place.
    pub node: Option<u32>,
    pub name: String,
    pub bytes: u64,
    pub files: u64,
    pub cat: u8,
    pub sub: u16,
    pub rule: u16,
    pub has_children: bool,
}

/// Everything the details pane and delete dialog show.
#[derive(Clone, Debug, Serialize)]
pub struct ItemInfo {
    pub id: u32,
    pub name: String,
    pub path: String,
    pub dir: bool,
    pub on_disk: u64,
    pub size: u64,
    pub files: u32,
    pub cat: u8,
    pub sub: u16,
    pub rule: u16,
    pub protected: Option<String>,
    /// The folder to open for this item (itself, or its parent for files).
    pub folder: u32,
    pub parent: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeleteOut {
    pub freed: u64,
    pub name: String,
    pub cancelled: bool,
}

/// A row of the "largest files" list.
#[derive(Clone, Debug, Serialize)]
pub struct FileRow {
    pub id: u32,
    pub name: String,
    pub path: String,
    pub on_disk: u64,
    pub cat: u8,
    /// Last-modified day (0 = unknown).
    pub mtime: u16,
}

/// A row of the "file types" list.
#[derive(Clone, Debug, Serialize)]
pub struct TypeRow {
    /// Lowercase extension without the dot ("" = no extension).
    pub ext: String,
    pub bytes: u64,
    pub files: u64,
}

/// Something that's often left behind: see [`Scanned::leftovers`].
#[derive(Clone, Debug)]
pub struct Leftover {
    pub id: u32,
    pub name: String,
    pub path: String,
    pub on_disk: u64,
    /// "Installer", "Archive", "Disk image" or the sub-type name.
    pub kind: String,
    pub note: String,
    /// The Windows tool that removes it (when DiskRoot shouldn't).
    pub action: Option<diskroot_core::classify::Action>,
    /// DiskRoot may move it to the Recycle Bin (not protected).
    pub deletable: bool,
    /// Last-modified day (0 = unknown).
    pub mtime: u16,
    /// A guess ("possibly left over"), never marked for deleting.
    pub hint: bool,
}

/// What [`Scanned::apply_live`] did with one batch of changed paths.
#[derive(Clone, Debug, Default)]
pub struct LiveResult {
    /// Files whose size changed (tree updated).
    pub resized: usize,
    /// Items gone from disk (removed from the tree).
    pub removed: usize,
    /// New items not in the scan: (relative path, bytes). Shown as "press
    /// F5 to include them": inserting them would mean re-sorting the tree.
    pub new: Vec<(String, u64)>,
}

/// One scan, classified, ready to answer UI questions.
#[derive(Clone)]
pub struct Scanned {
    /// Shared so the "remember this scan" thread can save it while the UI
    /// uses it; a delete copies it only if that save is still running.
    index: Arc<Index>,
    children: Arc<Children>,
    cls: Arc<Classification>,
    cats: Arc<CatStats>,
    stats: ScanStats,
    mode: String,
    classify_ms: f64,
    /// Folders deleted through DiskRoot (files come from the live totals).
    removed_dirs: u64,
    /// The rules this scan was classified with.
    rules: Arc<Rules>,
}

impl Scanned {
    pub fn new(mut index: Index, stats: ScanStats, mode: String) -> Self {
        let t = Instant::now();
        let rules = rules();
        index.shrink_to_fit();
        // The child lists don't depend on classification, so build them on
        // a second thread (measured on C:, 1.84M items: 213 -> 168 ms).
        let (children, cls, cats) = std::thread::scope(|s| {
            let children = s.spawn(|| Children::build(&index));
            let cls = Classification::run(&index, &rules);
            let cats = CatStats::compute(&index, &cls, rules.subs.len());
            (children.join().expect("Children::build doesn't panic"), cls, cats)
        });
        let classify_ms = t.elapsed().as_secs_f64() * 1000.0;
        Self { index: Arc::new(index), children: Arc::new(children), cls: Arc::new(cls), cats: Arc::new(cats), stats, mode, classify_ms, removed_dirs: 0, rules }
    }

    /// An online-only cloud placeholder (e.g. OneDrive "Free up space").
    pub fn is_cloud(&self, node: u32) -> bool {
        (node as usize) < self.index.len() && self.index.flags(node) & flags::CLOUD != 0
    }

    /// Last-modified day of a node (folders: newest inside); 0 = unknown.
    pub fn mtime(&self, node: u32) -> u16 {
        if (node as usize) < self.index.len() { self.index.mtime(node) } else { 0 }
    }

    /// Bytes on disk under `node` per age bucket (index = bucket, last =
    /// unknown date). One pass over the subtree's files.
    pub fn age_breakdown(&self, node: u32, today: u16) -> [u64; diskroot_core::AGE_UNKNOWN + 1] {
        let mut out = [0u64; diskroot_core::AGE_UNKNOWN + 1];
        if self.check(node).is_err() {
            return out;
        }
        let idx = &self.index;
        let mut stack = vec![node];
        while let Some(x) = stack.pop() {
            if idx.is_dir(x) {
                stack.extend(self.children.live(idx, x));
            } else {
                out[diskroot_core::age_bucket(today, idx.mtime(x))] += idx.alloc(x);
            }
        }
        out
    }

    /// The safety label for a node: its rule's, with an "apps only" rule
    /// ("Cache") downgraded to "check first" in the user's own files.
    pub fn safety(&self, node: u32) -> Option<Safety> {
        let r = *self.cls.rule.get(node as usize)?;
        let info = self.rules.info.get(r as usize)?;
        diskroot_core::classify::safety_at(info, &self.index.path(self.cls.anchor[node as usize]))
    }

    /// The rules this scan was classified with (for notes and labels).
    pub fn rules(&self) -> &Rules {
        &self.rules
    }

    /// Classifies again with the current rules (after a rules reload).
    pub fn reclassify(&mut self) {
        self.rules = rules();
        self.cls = Arc::new(Classification::run(&self.index, &self.rules));
        self.cats = Arc::new(CatStats::compute(&self.index, &self.cls, self.rules.subs.len()));
    }

    /// "Mark all" for a sub-type ("sub:N"): its places that a rule labelled
    /// "safe to clear" matched directly, minus anything protected. Places
    /// found by file extension are never included: they're the folders
    /// holding such files, and marking one would take the rest with it.
    pub fn markable(&self, key: &str) -> Vec<u32> {
        let Some(CatKey::Sub(s)) = CatKey::parse(key, &self.rules) else { return Vec::new() };
        self.cats.places_by_sub[s as usize]
            .iter()
            .map(|p| p.node)
            .filter(|&n| {
                let r = self.cls.rule[n as usize];
                n != 0
                    && self.index.flags(n) & flags::DELETED == 0
                    && self.cls.matched_here(n)
                    && self.rules.info.get(r as usize).is_some_and(|i| diskroot_core::classify::safety_at(i, &self.index.path(n)) == Some(Safety::Green) && !i.user)
                    && self.cls.protection(&self.index, &self.rules, n).is_none()
            })
            .collect()
    }

    /// Leftovers: Windows upgrade folders (removed through Windows' own
    /// cleanup) and installers, archives and disk images in Downloads
    /// folders. Biggest first.
    pub fn leftovers(&self, limit: usize) -> Vec<Leftover> {
        const KINDS: [(&str, &[&str]); 3] = [
            ("Installer", &["exe", "msi", "msix", "msixbundle", "appx"]),
            ("Archive", &["zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "zst"]),
            ("Disk image", &["iso", "img"]),
        ];
        let idx = &self.index;
        let mut out = Vec::new();
        for id in 1..idx.len() as u32 {
            if idx.flags(id) & flags::DELETED != 0 {
                continue;
            }
            if idx.is_dir(id) {
                let r = self.cls.rule[id as usize];
                if self.cls.matched_here(id)
                    && let Some(info) = self.rules.info.get(r as usize)
                    && info.action == Some(diskroot_core::classify::Action::StorageSettings)
                {
                    out.push(self.leftover(id, self.rules.subs[self.cls.sub[id as usize] as usize].name.clone(), info.note.clone().unwrap_or_default()));
                }
                continue;
            }
            let name = idx.name(id);
            let Some(ext) = name.rsplit_once('.').map(|x| x.1) else { continue };
            let Some((kind, _)) = KINDS.iter().find(|(_, exts)| exts.iter().any(|e| e.eq_ignore_ascii_case(ext))) else { continue };
            // Inside a folder named Downloads (any depth, usually 1-3 up).
            let mut up = idx.parent(id);
            let mut in_downloads = false;
            for _ in 0..32 {
                if up == NO_PARENT {
                    break;
                }
                if idx.name(up).eq_ignore_ascii_case("Downloads") || (up == 0 && idx.name(0).to_ascii_lowercase().ends_with("\\downloads")) {
                    in_downloads = true;
                    break;
                }
                up = idx.parent(up);
            }
            if in_downloads {
                let note = match *kind {
                    "Installer" => "A downloaded installer. Once the program is installed, the installer is usually not needed.",
                    "Archive" => "A downloaded archive. If you've extracted what you need, it can usually go.",
                    _ => "A downloaded disk image. If it's installed or burned, it can usually go.",
                };
                out.push(self.leftover(id, (*kind).to_owned(), note.to_owned()));
            }
        }
        out.sort_unstable_by_key(|l| std::cmp::Reverse(l.on_disk));
        out.truncate(limit);
        out
    }

    /// "Possibly left over" app data: folders directly in AppData\Local,
    /// Roaming, LocalLow or ProgramData that no installed or running
    /// program's name, publisher or folder matches, that hold at least
    /// `min_bytes`, and where nothing changed for `min_days`. Hints only:
    /// never labelled safe, never marked.
    pub fn orphan_hints(&self, programs: &orphans::Programs, min_bytes: u64, min_days: u16) -> Vec<Leftover> {
        let idx = &self.index;
        let today = diskroot_core::today_days();
        let root_is_drive = idx.name(0).len() <= 3 && idx.name(0).ends_with(":\\");
        let mut out = Vec::new();
        for id in 1..idx.len() as u32 {
            if !idx.is_dir(id) || idx.flags(id) & (flags::DELETED | flags::LINK) != 0 || idx.alloc(id) < min_bytes {
                continue;
            }
            let p = idx.parent(id);
            let pname = idx.name(p);
            // The scan root's name is a whole path: compare its last part.
            let last = |n: u32| idx.name(n).trim_end_matches('\\').rsplit('\\').next().unwrap_or("");
            let in_appdata = ["Local", "Roaming", "LocalLow"].iter().any(|x| pname.eq_ignore_ascii_case(x))
                && p != 0
                && last(idx.parent(p)).eq_ignore_ascii_case("AppData");
            let in_programdata = pname.eq_ignore_ascii_case("ProgramData") && p != 0 && idx.parent(p) == 0 && root_is_drive;
            if !(in_appdata || in_programdata) {
                continue;
            }
            let name = idx.name(id);
            let mt = idx.mtime(id);
            if self.cls.matched_here(id)
                || orphans::NEVER.contains(&orphans::norm(name).as_str())
                || mt == 0
                || today.saturating_sub(mt) < min_days
                || programs.matches(name)
                || self.cls.protection(idx, &self.rules, id).is_some()
            {
                continue;
            }
            let note = format!(
                "No installed or running program's name, publisher or folder matches \"{name}\" ({} programs checked), and nothing in it changed for {}. \
It may be left over from an uninstalled program, or belong to one that's installed under another name. It can hold settings or saved games: check before deleting.",
                programs.count,
                diskroot_core::age_text(today, mt).trim_end_matches(" ago"),
            );
            let mut l = self.leftover(id, "Possibly left over (app data)".into(), note);
            l.hint = true;
            out.push(l);
        }
        out.sort_unstable_by_key(|l| std::cmp::Reverse(l.on_disk));
        out
    }

    /// What compressing `node` (or with `undo`, uncompressing it) would
    /// touch, from the scan alone (no file is opened). Refuses protected
    /// places, cloud folders and whole drives; protected places *inside*
    /// are left out, like links and cloud placeholders.
    pub fn compress_plan(&self, node: u32, undo: bool) -> Result<compress::Plan, String> {
        let info = self.info(node)?;
        if let Some(why) = info.protected {
            return Err(why);
        }
        let root = PathBuf::from(&info.path);
        if info.path.trim_end_matches('\\').len() <= 2 {
            return Err("DiskRoot won't compress a whole drive.".into());
        }
        if self.index.flags(node) & flags::CLOUD != 0 || safety::in_cloud_folder(&root) {
            return Err("This is in a OneDrive folder. Compressing would make OneDrive download and upload it again, so DiskRoot leaves cloud folders alone.".into());
        }
        compress::check_volume(&root)?;
        let idx = &self.index;
        let mut plan = compress::Plan { root, ..compress::Plan::default() };
        let mut estimate = 0f64;
        let mut stack = vec![node];
        while let Some(x) = stack.pop() {
            let fl = idx.flags(x);
            if fl & flags::DELETED != 0 {
                continue;
            }
            if idx.is_dir(x) {
                if x != node && self.cls.protected_here(&self.rules, x).is_some() {
                    plan.protected_inside += 1;
                } else if fl & flags::LINK == 0 {
                    stack.extend(self.children.live(idx, x));
                }
                continue;
            }
            let (size, alloc) = (idx.size(x), idx.alloc(x));
            let add = |c: &mut compress::Count| {
                c.0 += 1;
                c.1 += alloc;
            };
            if undo {
                if fl & (flags::CLOUD | flags::LINK | flags::SPARSE) != 0 {
                    add(&mut plan.skipped_already);
                    continue;
                }
                // Windows-compressed files take less space than their size
                // (NTFS-compressed ones are a different thing: left alone).
                let full = size.div_ceil(4096) * 4096;
                if fl & flags::COMPRESSED == 0 && alloc < full {
                    plan.needs_free += full - alloc;
                } else {
                    continue;
                }
            } else {
                match compress::verdict(idx.name(x), fl, size, alloc) {
                    compress::Verdict::Already => {
                        add(&mut plan.skipped_already);
                        continue;
                    }
                    compress::Verdict::Tiny => {
                        add(&mut plan.skipped_tiny);
                        continue;
                    }
                    compress::Verdict::Format => {
                        add(&mut plan.skipped_formats);
                        continue;
                    }
                    compress::Verdict::Pack(e) => estimate += e,
                }
            }
            plan.bytes += alloc;
            plan.files.push(compress::PlanFile { node: x, path: PathBuf::from(idx.path(x)), size, alloc });
        }
        plan.files.sort_unstable_by_key(|f| std::cmp::Reverse(f.alloc));
        plan.estimate_max = estimate as u64;
        Ok(plan)
    }

    /// After compressing: the new size on disk of each changed file, so the
    /// tree and map are right without scanning again.
    pub fn apply_new_alloc(&mut self, changes: &[(u32, u64)]) {
        if changes.is_empty() {
            return;
        }
        let idx = Arc::make_mut(&mut self.index);
        for &(n, alloc) in changes {
            if (n as usize) < idx.len() && !idx.is_dir(n) && idx.flags(n) & flags::DELETED == 0 {
                let size = idx.size(n);
                idx.resize_file(n, size, alloc, 0);
            }
        }
        self.cats = Arc::new(CatStats::compute(&self.index, &self.cls, self.rules.subs.len()));
    }

    /// View > Similar folders: likely copies and folders with similar names.
    pub fn similar_folders(&self) -> similar::Found {
        similar::find(&self.index, &self.children, |x| self.cls.protected_here(&self.rules, x).is_some())
    }

    /// Items for a PowerShell cleanup plan: every node that passes the same
    /// checks as a delete (protection, unusual names), plus the refused ones
    /// and why.
    pub fn plan_items(&self, nodes: &[u32]) -> (Vec<power::PlanItem>, Vec<(String, String)>) {
        let (mut ok, mut refused) = (Vec::new(), Vec::new());
        for &n in nodes {
            match self.prepare_delete(n) {
                Ok(p) => ok.push(power::PlanItem { path: p.path.display().to_string(), dir: p.dir, bytes: p.freed }),
                Err(why) => refused.push((self.index.name(n).to_owned(), why)),
            }
        }
        (ok, refused)
    }

    fn leftover(&self, id: u32, kind: String, note: String) -> Leftover {
        let r = self.cls.rule[id as usize];
        Leftover {
            id,
            name: self.index.name(id).to_owned(),
            path: self.index.path(id),
            on_disk: self.index.alloc(id),
            kind,
            note,
            action: self.rules.info.get(r as usize).and_then(|i| i.action),
            deletable: self.cls.protection(&self.index, &self.rules, id).is_none(),
            mtime: self.index.mtime(id),
            hint: false,
        }
    }

    fn model(&self) -> CatModel<'_> {
        CatModel { idx: &self.index, cls: &self.cls, stats: &self.cats, rules: &self.rules }
    }

    /// Ok if `node` exists and wasn't deleted.
    pub fn check(&self, node: u32) -> Result<(), String> {
        if (node as usize) < self.index.len() && self.index.flags(node) & flags::DELETED == 0 {
            Ok(())
        } else {
            Err(format!("No such item: {node}"))
        }
    }

    /// Adds category data to view nodes of real items.
    fn decorate(&self, nodes: &mut [ViewNode]) {
        for n in nodes.iter_mut().filter(|n| n.id >= 0 && n.key.is_empty()) {
            let i = n.id as usize;
            if n.cat == u8::MAX {
                n.cat = self.cats.dominant[i];
            }
            (n.own_cat, n.sub, n.rule) = (self.cls.cat[i], self.cls.sub[i], self.cls.rule[i]);
        }
    }

    /// A hardlinked file's extra name whose bytes are counted under its
    /// other name (folder walk).
    pub fn counted_elsewhere(&self, node: u32) -> bool {
        (node as usize) < self.index.len() && self.index.flags(node) & diskroot_core::flags::HARDLINK != 0
    }

    /// "What you'd lose if this drive failed": the kinds of data that can't
    /// be downloaded or rebuilt again (documents, photos and videos, your
    /// code projects, your datasets), with bytes and files. Cloud-synced
    /// folders (OneDrive) are a separate category, so they're not in here.
    pub fn at_risk(&self) -> Vec<(&'static str, u64, u64)> {
        let cat = |key: &str| diskroot_core::classify::CATEGORIES.iter().position(|c| c.key == key).map_or((0, 0), |i| (self.cats.cat_total[i], self.cats.cat_files[i]));
        let subs = |names: &[&str]| {
            self.rules.subs.iter().enumerate().filter(|(_, s)| names.contains(&s.name.as_str())).fold((0u64, 0u64), |a, (i, _)| (a.0 + self.cats.sub_total[i], a.1 + self.cats.sub_files[i]))
        };
        let (docs, media) = (cat("docs"), cat("media"));
        let projects = subs(&["Code projects", "My projects"]);
        // Taught photos/videos and backups are already inside Media and Documents.
        let data = subs(&["My datasets"]);
        [("Documents", docs), ("Photos, videos and music", media), ("Your code projects", projects), ("Datasets you marked as yours", data)]
            .into_iter()
            .filter(|(_, (b, _))| *b > 0)
            .map(|(n, (b, f))| (n, b, f))
            .collect()
    }

    /// Folders this scan couldn't read (paths, sorted), at most `limit`.
    pub fn unreadable_folders(&self, limit: usize) -> Vec<String> {
        let idx = &self.index;
        let mut v: Vec<String> = (0..idx.len() as u32)
            .filter(|&n| idx.flags(n) & diskroot_core::flags::UNREADABLE != 0 && idx.flags(n) & diskroot_core::flags::DELETED == 0)
            .filter_map(|n| self.path(n).ok())
            .collect();
        v.sort_by_key(|p| p.to_lowercase());
        v.truncate(limit);
        v
    }

    /// Where the drive's used space is: what this scan counted against what
    /// Windows reports as used, and what a Fast scan could measure. `None`
    /// unless the scan root is a whole drive.
    pub fn space_report(&self) -> Option<SpaceReport> {
        let root = self.index.name(0).to_owned();
        if !(root.len() == 3 && root.ends_with(":\\")) {
            return None;
        }
        let (total, free) = disk_space(&root)?;
        let idx = &self.index;
        let mft = self.mode.starts_with("MFT");
        // Root children: NTFS's own files ($MFT, $LogFile, ...) and System
        // Volume Information, when the scan could see them (Fast scan).
        let (mut ntfs, mut svi) = (0u64, None);
        for &c in self.children.of(0) {
            let name = idx.name(c);
            if idx.flags(c) & diskroot_core::flags::DELETED != 0 {
                continue;
            }
            if name.starts_with('$') && !name.eq_ignore_ascii_case("$Recycle.Bin") && !name.eq_ignore_ascii_case("$WinREAgent") {
                ntfs += idx.alloc(c);
            } else if name.eq_ignore_ascii_case("System Volume Information") && idx.flags(c) & diskroot_core::flags::UNREADABLE == 0 {
                svi = Some(idx.alloc(c));
            }
        }
        let hardlinks = (0..idx.len() as u32).filter(|&n| idx.flags(n) & diskroot_core::flags::HARDLINK != 0).count() as u64;
        Some(SpaceReport {
            root,
            total,
            used: total.saturating_sub(free),
            counted: idx.alloc(0),
            fast_scan: mft,
            ntfs_files: (mft || ntfs > 0).then_some(ntfs),
            restore_points: svi,
            unreadable: self.stats.unreadable,
            hardlink_names: hardlinks,
        })
    }

    pub fn summary(&self) -> Summary {
        let total: u64 = self.cats.cat_total.iter().sum();
        let other = self.cats.cat_total[diskroot_core::classify::CAT_OTHER as usize];
        Summary {
            root: self.index.name(0).to_owned(),
            mode: self.mode.clone(),
            elapsed_ms: self.stats.elapsed.as_secs_f64() * 1000.0,
            classify_ms: self.classify_ms,
            // Live counts, so they stay right after deleting things.
            files: u64::from(self.index.files(0)),
            dirs: self.stats.dirs.saturating_sub(self.removed_dirs),
            unreadable: self.stats.unreadable,
            size: self.index.size(0),
            on_disk: self.index.alloc(0),
            categorized: if total > 0 { (total - other) as f64 / total as f64 } else { 0.0 },
            admin_hint: self.mode.contains("administrator"),
        }
    }

    /// Pruned shapes for drawing folder `node` (sunburst/treemap).
    pub fn view(&self, node: u32, depth: u8) -> Result<Vec<ViewNode>, String> {
        self.check(node)?;
        let lim = ViewLimits { max_depth: depth.clamp(1, 10), ..ViewLimits::default() };
        let mut nodes = build_view(&self.index, &self.children, node, &lim);
        self.decorate(&mut nodes);
        Ok(nodes)
    }

    fn item(&self, c: u32) -> Item {
        let idx = &self.index;
        Item {
            id: c,
            name: idx.name(c).to_owned(),
            size: idx.size(c),
            on_disk: idx.alloc(c),
            files: idx.files(c),
            dir: idx.is_dir(c),
            link: idx.flags(c) & flags::LINK != 0,
            cloud: idx.flags(c) & flags::CLOUD != 0,
            cat: self.cats.dominant[c as usize],
            own_cat: self.cls.cat[c as usize],
            sub: self.cls.sub[c as usize],
            rule: self.cls.rule[c as usize],
            protected: self.cls.protection(idx, &self.rules, c).is_some(),
        }
    }

    /// The node itself as a list row (for the tree root).
    pub fn item_of(&self, node: u32) -> Result<Item, String> {
        self.check(node)?;
        Ok(self.item(node))
    }

    /// Children of folder `node`, biggest first, plus its category breakdown.
    pub fn list(&self, node: u32, limit: usize) -> Result<ListOut, String> {
        self.check(node)?;
        let kids: Vec<u32> = self.children.live(&self.index, node).collect();
        let items = kids.iter().take(limit).map(|&c| self.item(c)).collect();
        Ok(ListOut { total: kids.len(), items, breakdown: self.breakdown(node) })
    }

    /// Bytes per category inside `node`, biggest first.
    pub fn breakdown(&self, node: u32) -> Vec<(u8, u64)> {
        let mut b: Vec<(u8, u64)> = self
            .cats
            .breakdown(node)
            .into_iter()
            .enumerate()
            .filter(|&(_, v)| v > 0)
            .map(|(c, v)| (c as u8, v))
            .collect();
        b.sort_unstable_by_key(|&(_, v)| std::cmp::Reverse(v));
        b
    }

    pub fn crumbs(&self, node: u32) -> Result<Vec<Crumb>, String> {
        self.check(node)?;
        Ok(ancestors(&self.index, node).into_iter().map(|(id, name)| Crumb { id, name }).collect())
    }

    /// The by-category diagram rooted at `key` ("", "cat:3", "sub:12").
    pub fn category_view(&self, key: &str) -> Result<CatViewOut, String> {
        let k = CatKey::parse(key, &self.rules).ok_or("Unknown category")?;
        let m = self.model();
        let mut nodes = m.view(k, 0.002);
        self.decorate(&mut nodes);
        Ok(CatViewOut { nodes, crumbs: m.crumbs(k) })
    }

    /// Bytes and files of a category key ("" = everything).
    pub fn cat_total(&self, key: &str) -> (u64, u64) {
        match CatKey::parse(key, &self.rules) {
            Some(CatKey::Cat(c)) => (self.cats.cat_total[c as usize], self.cats.cat_files[c as usize]),
            Some(CatKey::Sub(s)) => (self.cats.sub_total[s as usize], self.cats.sub_files[s as usize]),
            _ => (self.cats.cat_total.iter().sum(), self.cats.cat_files.iter().sum()),
        }
    }

    /// Rows under a by-category key: "" -> categories, "cat:N" -> sub-types,
    /// "sub:N" -> places (all of them, biggest first).
    pub fn cat_children(&self, key: &str, limit: usize) -> Result<Vec<CatRow>, String> {
        let k = CatKey::parse(key, &self.rules).ok_or("Unknown category")?;
        let st = &self.cats;
        let rows = match k {
            CatKey::All => {
                let mut cats: Vec<u8> = (0..NCAT as u8).filter(|&c| st.cat_total[c as usize] > 0).collect();
                cats.sort_unstable_by_key(|&c| std::cmp::Reverse(st.cat_total[c as usize]));
                cats.into_iter()
                    .map(|c| CatRow {
                        key: CatKey::Cat(c).key(),
                        node: None,
                        name: CATEGORIES[c as usize].name.to_owned(),
                        bytes: st.cat_total[c as usize],
                        files: st.cat_files[c as usize],
                        cat: c,
                        sub: u16::MAX,
                        rule: u16::MAX,
                        has_children: true,
                    })
                    .collect()
            }
            CatKey::Cat(c) => {
                let mut subs: Vec<u16> = (0..self.rules.subs.len() as u16)
                    .filter(|&s| self.rules.sub_cat(s) == c && st.sub_total[s as usize] > 0)
                    .collect();
                subs.sort_unstable_by_key(|&s| std::cmp::Reverse(st.sub_total[s as usize]));
                subs.into_iter()
                    .map(|s| CatRow {
                        key: CatKey::Sub(s).key(),
                        node: None,
                        name: self.rules.subs[s as usize].name.clone(),
                        bytes: st.sub_total[s as usize],
                        files: st.sub_files[s as usize],
                        cat: c,
                        sub: s,
                        rule: u16::MAX,
                        has_children: true,
                    })
                    .collect()
            }
            CatKey::Sub(s) => st.places_by_sub[s as usize]
                .iter()
                .filter(|p| self.index.flags(p.node) & flags::DELETED == 0)
                .take(limit)
                .map(|p| CatRow {
                    key: String::new(),
                    node: Some(p.node),
                    name: self.index.name(p.node).to_owned(),
                    bytes: p.bytes,
                    files: u64::from(p.files),
                    cat: self.rules.sub_cat(s),
                    sub: s,
                    rule: self.cls.rule[p.node as usize],
                    has_children: false,
                })
                .collect(),
        };
        Ok(rows)
    }

    pub fn info(&self, node: u32) -> Result<ItemInfo, String> {
        self.check(node)?;
        let idx = &self.index;
        let i = node as usize;
        let parent = idx.parent(node);
        Ok(ItemInfo {
            id: node,
            name: idx.name(node).to_owned(),
            path: idx.path(node),
            dir: idx.is_dir(node),
            on_disk: idx.alloc(node),
            size: idx.size(node),
            files: idx.files(node),
            cat: self.cls.cat[i],
            sub: self.cls.sub[i],
            rule: self.cls.rule[i],
            protected: self.cls.protection(idx, &self.rules, node),
            folder: if idx.is_dir(node) || parent == NO_PARENT { node } else { parent },
            parent: (parent != NO_PARENT).then_some(parent),
        })
    }

    /// Step 1 of a delete: everything needed to recycle `node`, or why not.
    /// Protection is enforced here, not just in the UI. Also collects the
    /// warnings the confirmation dialog must show (cloud, Git, large).
    pub fn prepare_delete(&self, node: u32) -> Result<PendingDelete, String> {
        let info = self.info(node)?;
        if let Some(why) = info.protected {
            return Err(why);
        }
        if node == 0 {
            return Err("DiskRoot won't delete the whole scanned folder or drive.".into());
        }
        let idx = &self.index;
        let path = PathBuf::from(&info.path);
        if let Some(why) = safety::unusual_name(&path) {
            return Err(format!("DiskRoot won't delete this: {why} Use File Explorer instead."));
        }
        let mut warnings = safety::DeleteWarnings {
            cloud: idx.flags(node) & flags::CLOUD != 0 || safety::in_cloud_folder(&path),
            git_repos: 0,
            large: info.on_disk >= safety::LARGE_BYTES || info.files >= safety::LARGE_FILES,
            no_recycle_bin: matches!(drive_type(&info.path), 2 | 4),
            apps_running: Vec::new(),
        };
        // Programs that use this place (rules name them, e.g. Spotify for
        // its cache): from the rule in effect and rules matched inside.
        let mut apps: Vec<&str> = Vec::new();
        let mut add_apps = |r: u16| {
            if let Some(i) = self.rules.info.get(r as usize) {
                apps.extend(i.app.iter().map(String::as_str));
            }
        };
        add_apps(self.cls.rule[node as usize]);
        // One pass over the subtree: protected places inside (refuse), Git
        // repos and cloud placeholders (warn).
        let mut stack = vec![node];
        while let Some(x) = stack.pop() {
            if x != node
                && let Some(what) = self.cls.protected_here(&self.rules, x)
            {
                return Err(format!("It contains {}, which DiskRoot protects ({what}).", idx.path(x)));
            }
            if x != node && idx.name(x).eq_ignore_ascii_case(".git") {
                warnings.git_repos += 1;
            }
            if x != node && self.cls.matched_here(x) {
                add_apps(self.cls.rule[x as usize]);
            }
            if idx.flags(x) & flags::CLOUD != 0 {
                warnings.cloud = true;
            }
            if idx.is_dir(x) {
                stack.extend(self.children.live(idx, x));
            }
        }
        if idx.name(node).eq_ignore_ascii_case(".git") {
            warnings.git_repos += 1;
        }
        if !apps.is_empty() {
            let running = tools::running_programs();
            let mut seen = std::collections::HashSet::new();
            for a in apps {
                if running.contains(&a.to_lowercase()) && seen.insert(a.to_lowercase()) {
                    warnings.apps_running.push(a.to_owned());
                }
            }
        }
        Ok(PendingDelete {
            node,
            path,
            name: info.name,
            freed: info.on_disk,
            files: info.files,
            dir: info.dir,
            was_link: idx.flags(node) & flags::LINK != 0,
            scan_root: PathBuf::from(idx.name(0)),
            category: CATEGORIES[self.cls.cat[node as usize] as usize].name.to_owned(),
            warnings,
            identity: None,
        })
    }

    /// Step 3 of a delete: after the Recycle Bin move succeeded, drop the
    /// item from the tree and refresh category totals.
    pub fn apply_delete(&mut self, node: u32) {
        let mut stack = vec![node];
        while let Some(x) = stack.pop() {
            if self.index.is_dir(x) && self.index.flags(x) & flags::DELETED == 0 {
                self.removed_dirs += 1;
            }
            stack.extend(self.children.live(&self.index, x));
        }
        Arc::make_mut(&mut self.index).remove_subtree(node, &self.children);
        self.cats = Arc::new(CatStats::compute(&self.index, &self.cls, self.rules.subs.len()));
    }

    /// The `limit` largest files (size on disk), optionally only those whose
    /// name contains `query` (case-insensitive). One pass with a small heap.
    pub fn top_files(&self, limit: usize, query: &str) -> Vec<FileRow> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let q = query.trim().to_lowercase();
        let idx = &self.index;
        let mut heap: BinaryHeap<Reverse<(u64, u32)>> = BinaryHeap::with_capacity(limit + 1);
        for id in 1..idx.len() as u32 {
            if idx.is_dir(id) || idx.flags(id) & flags::DELETED != 0 {
                continue;
            }
            let a = idx.alloc(id);
            if heap.len() == limit && heap.peek().is_some_and(|Reverse((m, _))| a <= *m) {
                continue;
            }
            if !q.is_empty() && !idx.name(id).to_lowercase().contains(&q) {
                continue;
            }
            heap.push(Reverse((a, id)));
            if heap.len() > limit {
                heap.pop();
            }
        }
        let mut v: Vec<(u64, u32)> = heap.into_iter().map(|Reverse(x)| x).collect();
        v.sort_unstable_by_key(|&(a, id)| (Reverse(a), id));
        v.into_iter()
            .map(|(a, id)| FileRow { id, name: idx.name(id).to_owned(), path: idx.path(id), on_disk: a, cat: self.cls.cat[id as usize], mtime: idx.mtime(id) })
            .collect()
    }

    /// Files that could be duplicates: same size (≥ `min_size`), at least
    /// two of them, not cloud placeholders. From the scan only: no file is
    /// opened here. Biggest sizes first.
    pub fn duplicate_candidates(&self, min_size: u64) -> Vec<(u64, Vec<(u32, String)>)> {
        let idx = &self.index;
        let mut by_size: std::collections::HashMap<u64, Vec<u32>> = std::collections::HashMap::new();
        for id in 1..idx.len() as u32 {
            let f = idx.flags(id);
            if idx.is_dir(id) || f & (flags::DELETED | flags::CLOUD | flags::LINK) != 0 || idx.size(id) < min_size {
                continue;
            }
            by_size.entry(idx.size(id)).or_default().push(id);
        }
        let mut v: Vec<(u64, Vec<(u32, String)>)> = by_size
            .into_iter()
            .filter(|(_, ids)| ids.len() > 1)
            .map(|(size, ids)| (size, ids.into_iter().map(|id| (id, idx.path(id))).collect()))
            .collect();
        v.sort_unstable_by_key(|g| std::cmp::Reverse(g.0));
        v
    }

    /// Bytes and file counts per file extension (lowercase, "" = none),
    /// biggest first.
    pub fn file_types(&self, limit: usize) -> Vec<TypeRow> {
        let idx = &self.index;
        let mut m: std::collections::HashMap<String, (u64, u64)> = std::collections::HashMap::new();
        for id in 1..idx.len() as u32 {
            if idx.is_dir(id) || idx.flags(id) & flags::DELETED != 0 {
                continue;
            }
            let name = idx.name(id);
            let ext = match name.rfind('.') {
                Some(i) if i > 0 && name.len() - i <= 12 => &name[i + 1..],
                _ => "",
            };
            let key = if ext.bytes().any(|b| b.is_ascii_uppercase()) { ext.to_ascii_lowercase() } else { ext.to_owned() };
            let e = m.entry(key).or_insert((0, 0));
            e.0 = e.0.saturating_add(idx.alloc(id));
            e.1 += 1;
        }
        let mut v: Vec<TypeRow> = m.into_iter().map(|(ext, (bytes, files))| TypeRow { ext, bytes, files }).collect();
        v.sort_unstable_by_key(|r| std::cmp::Reverse(r.bytes));
        v.truncate(limit);
        v
    }

    /// Writes the whole scan as CSV to `path` (see `report::write_csv`).
    /// Blocking: use a worker thread (via `remember_parts`' index).
    pub fn export_csv(index: &Index, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let mut w = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
        diskroot_core::report::write_csv(index, &mut w)?;
        w.flush()
    }

    /// Heap bytes per data structure (for memory work).
    pub fn memory_report(&self) -> Vec<(&'static str, usize)> {
        let mut v = self.index.heap_columns();
        v.push(("children", self.children.heap_bytes()));
        v.extend(self.cls.heap_bytes());
        v.extend(self.cats.heap_bytes());
        v
    }

    /// What the background "remember" step needs: the shared index, the
    /// scan stats and the mode label.
    pub fn remember_parts(&self) -> (Arc<Index>, ScanStats, String) {
        (Arc::clone(&self.index), self.stats.clone(), self.mode.clone())
    }

    /// Node of a folder given its path relative to the scan root
    /// ("Users\me"), case-insensitive. Used to jump from "what changed".
    pub fn find_rel(&self, rel: &str) -> Option<u32> {
        let mut cur = 0u32;
        for comp in rel.split('\\').filter(|c| !c.is_empty()) {
            cur = self.children.live(&self.index, cur).find(|&c| self.index.name(c).eq_ignore_ascii_case(comp))?;
        }
        Some(cur)
    }

    /// Live updates: re-measures the paths the watcher reported (relative
    /// to the scan root) and patches the tree: changed file sizes, removed
    /// items. New items are only reported. Approximate until the next scan:
    /// the size on disk of a changed file is rounded to 4 KB clusters
    /// (exact for compressed and sparse files).
    pub fn apply_live(&mut self, rel_paths: &[String]) -> LiveResult {
        let root = PathBuf::from(self.index.name(0));
        let mut res = LiveResult::default();
        let (mut resize, mut remove) = (Vec::new(), Vec::new());
        for rel in rel_paths {
            if rel.is_empty() || rel.split('\\').any(|c| c == "..") {
                continue;
            }
            let full = root.join(rel);
            let meta = std::fs::symlink_metadata(&full);
            match (self.find_rel(rel), meta) {
                (Some(n), Err(e)) if e.kind() == std::io::ErrorKind::NotFound && n != 0 => remove.push(n),
                // An extra name of a hardlinked file stays 0 B: its data counts once,
                // under its other name.
                (Some(n), Ok(m)) if m.is_file() && !self.index.is_dir(n) && self.index.flags(n) & (flags::LINK | flags::HARDLINK) == 0 => {
                    let size = m.len();
                    let alloc = live_alloc(&full, size, self.index.flags(n));
                    let mt = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| (d.as_secs() / 86_400).min(u64::from(u16::MAX)) as u16);
                    if size != self.index.size(n) || alloc != self.index.alloc(n) {
                        resize.push((n, size, alloc, mt));
                    }
                }
                (None, Ok(m)) if !m.file_type().is_symlink() => res.new.push((rel.clone(), if m.is_file() { m.len() } else { 0 })),
                _ => {}
            }
        }
        if resize.is_empty() && remove.is_empty() {
            return res;
        }
        for &n in &remove {
            let mut stack = vec![n];
            while let Some(x) = stack.pop() {
                if self.index.is_dir(x) && self.index.flags(x) & flags::DELETED == 0 {
                    self.removed_dirs += 1;
                }
                stack.extend(self.children.live(&self.index, x));
            }
        }
        let idx = Arc::make_mut(&mut self.index);
        for (n, size, alloc, mt) in resize {
            idx.resize_file(n, size, alloc, mt);
            res.resized += 1;
        }
        for n in remove {
            idx.remove_subtree(n, &self.children);
            res.removed += 1;
        }
        self.cats = Arc::new(CatStats::compute(&self.index, &self.cls, self.rules.subs.len()));
        res
    }

    /// Full path of a node (for Explorer, copying, etc.).
    pub fn path(&self, node: u32) -> Result<String, String> {
        self.check(node)?;
        Ok(self.index.path(node))
    }
}

/// Size on disk of a file that changed while watching: asks Windows (exact
/// for NTFS-compressed, sparse and Windows-compressed files), else the size
/// rounded up to 4 KB clusters.
fn live_alloc(path: &Path, size: u64, node_flags: u8) -> u64 {
    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetCompressedFileSizeW(name: *const u16, high: *mut u32) -> u32;
    }
    let rounded = size.div_ceil(4096) * 4096;
    let w: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain([0]).collect();
    let mut high = 0u32;
    // SAFETY: NUL-terminated path and a valid out pointer.
    let low = unsafe { GetCompressedFileSizeW(w.as_ptr(), &mut high) };
    if low != u32::MAX {
        let real = u64::from(high) << 32 | u64::from(low);
        // Plain files report their size here, not clusters: keep the rounding for them.
        if node_flags & (flags::COMPRESSED | flags::SPARSE) != 0 || real < size {
            return real;
        }
    }
    rounded
}

/// A delete that passed the protection check (see [`Scanned::prepare_delete`]).
pub struct PendingDelete {
    pub node: u32,
    pub path: PathBuf,
    pub name: String,
    pub freed: u64,
    pub files: u32,
    pub dir: bool,
    pub was_link: bool,
    pub scan_root: PathBuf,
    pub category: String,
    pub warnings: safety::DeleteWarnings,
    /// File identities of the item and every folder up to the scan root,
    /// taken when the user was asked (see [`PendingDelete::arm`]).
    identity: Option<Vec<(PathBuf, safety::FileId)>>,
}

impl PendingDelete {
    /// Records the on-disk identity of the item and of every folder above
    /// it, up to and including the scan root. Call it when the confirmation
    /// dialog opens; `recycle` then refuses if anything was swapped in the
    /// meantime.
    pub fn arm(&mut self) -> Result<(), String> {
        if let Some(why) = safety::hard_lock(&self.path) {
            return Err(why);
        }
        self.identity = Some(safety::identity_chain(&self.path, &self.scan_root)?);
        Ok(())
    }

    /// Step 2: re-check the item on disk, move it to the Recycle Bin, and
    /// write the result to the local deletion log. Blocking (Windows may
    /// show its own progress or "too big to recycle" prompt): use a worker
    /// thread.
    pub fn recycle(&self) -> Result<DeleteOut, String> {
        let identity = match &self.identity {
            Some(chain) => Ok(chain.clone()),
            None => safety::identity_chain(&self.path, &self.scan_root),
        };
        // The folders above the item stay pinned (can't be renamed or
        // swapped) from the identity check until the move is done.
        let result = identity.and_then(|chain| safety::pin_identity(&chain)).and_then(|pinned| {
            let r = safety::hard_lock(&self.path)
                .map_or(Ok(()), Err)
                .and_then(|()| safety::verify_before_recycle(&self.path, &self.scan_root, self.dir, self.was_link))
                .and_then(|()| recycle::to_recycle_bin(&self.path));
            drop(pinned);
            r
        });
        let cancelled = matches!(result, Ok(recycle::Outcome::Cancelled));
        safety::append_log(&safety::LogEntry {
            time: safety::utc_now(),
            path: self.path.display().to_string(),
            bytes: self.freed,
            files: self.files,
            category: self.category.clone(),
            outcome: match &result {
                Ok(recycle::Outcome::Recycled) => "recycled".into(),
                Ok(recycle::Outcome::Cancelled) => "cancelled".into(),
                Err(e) => format!("refused: {e}"),
            },
        });
        result?;
        Ok(DeleteOut { freed: if cancelled { 0 } else { self.freed }, name: self.name.clone(), cancelled })
    }
}

/// Shows `path` selected in Windows Explorer.
pub fn reveal(path: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    if path.contains('"') {
        return Err("Path contains a quote character".into());
    }
    // Absolute path: a bare "explorer.exe" is searched for in this exe's own
    // folder first, where a planted copy would run instead.
    std::process::Command::new(integrity::windows_dir().join("explorer.exe"))
        .raw_arg(format!("/select,\"{path}\""))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open Explorer: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teaching_writes_a_category_only_rule_and_can_be_undone() {
        let file = std::env::temp_dir().join(format!("diskroot-teach-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&file);
        assert_eq!(teach_pattern(r"C:\Users\me\proj\"), Some("/Users/me/proj".into()));
        assert_eq!(teach_pattern(r"C:\"), None);
        teach_at(&file, r"C:\Users\me\proj", 0).unwrap();
        teach_at(&file, r"C:\Users\me\proj", 1).unwrap(); // replaces, doesn't add
        let text = std::fs::read_to_string(&file).unwrap();
        let rules = Rules::builtin().with_user_rules(&text).unwrap();
        assert_eq!(rules.user_rules(), 2, "the example rule + one taught rule");
        assert!(text.contains("My datasets") && !text.contains("My projects"));
        assert!(!text.contains("safety"), "never a safety label");
        let mut idx = Index::new("C:\\");
        let u = idx.push(0, "Users", 0, 0, diskroot_core::flags::DIR);
        let me = idx.push(u, "me", 0, 0, diskroot_core::flags::DIR);
        let proj = idx.push(me, "proj", 0, 0, diskroot_core::flags::DIR);
        idx.push(proj, "x.dat", 10, 10, 0);
        idx.compute_totals();
        let c = Classification::run(&idx, &rules);
        assert_eq!(rules.subs[c.sub[proj as usize] as usize].name, "My datasets");
        assert!(forget_taught_at(&file, r"C:\Users\me\proj").unwrap());
        assert!(!forget_taught_at(&file, r"C:\Users\me\proj").unwrap(), "already gone");
        assert!(!forget_taught_at(&file, r"C:\Projects\a\out").unwrap(), "hand-written rules stay");
        // A broken file is reported, not overwritten.
        std::fs::write(&file, "{ not json").unwrap();
        assert!(teach_at(&file, r"C:\Users\me\proj", 0).unwrap_err().contains("isn't valid JSON"));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{ not json");
        let _ = std::fs::remove_file(&file);
    }

    fn demo() -> Scanned {
        let mut idx = Index::new("C:\\");
        let users = idx.push(0, "Users", 0, 0, flags::DIR);
        let me = idx.push(users, "me", 0, 0, flags::DIR);
        let proj = idx.push(me, "proj", 0, 0, flags::DIR);
        let nm = idx.push(proj, "node_modules", 0, 0, flags::DIR);
        idx.push(nm, "a.js", 4096, 4096, 0);
        idx.push(me, "movie.mp4", 9000, 9000, 0);
        let win = idx.push(0, "Windows", 0, 0, flags::DIR);
        idx.push(win, "x.dll", 100, 100, 0);
        idx.compute_totals();
        Scanned::new(idx, ScanStats::default(), "test".into())
    }

    #[test]
    fn answers_ui_questions() {
        let s = demo();
        let l = s.list(0, 10).unwrap();
        assert_eq!(l.total, 2);
        assert_eq!(l.items[0].name, "Users");
        assert!(s.list(999, 10).is_err());
        let crumbs = s.crumbs(4).unwrap();
        assert_eq!(crumbs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["C:\\", "Users", "me", "proj", "node_modules"]);
        assert!(s.view(0, 3).unwrap().len() > 3);
        assert!(s.info(7).unwrap().protected.is_some(), "Windows is protected");
        assert!(s.prepare_delete(7).is_err());
        assert!(s.prepare_delete(4).is_ok(), "node_modules can be deleted");
        assert!(s.prepare_delete(0).is_err(), "never the scan root");
    }

    #[test]
    fn category_rows_drill_down() {
        let s = demo();
        let cats = s.cat_children("", 100).unwrap();
        assert!(cats.iter().all(|r| r.key.starts_with("cat:") && r.has_children));
        let total: u64 = cats.iter().map(|r| r.bytes).sum();
        assert_eq!(total, s.summary().on_disk);
        let dev = cats.iter().find(|r| r.name == "Dev stuff").unwrap();
        let subs = s.cat_children(&dev.key, 100).unwrap();
        let places = s.cat_children(&subs[0].key, 100).unwrap();
        assert_eq!(places[0].name, "node_modules");
        assert_eq!(places[0].node, Some(4));
        assert!(s.cat_children("bogus", 1).is_err());
    }

    /// Real disk, read-only: a scan that starts inside Windows must keep
    /// everything in it locked (it once didn't: only the root folder's own
    /// name was checked against the rules, not its parents).
    #[test]
    fn scanning_inside_windows_keeps_it_locked() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        for sub in ["Fonts", "System32\\drivers"] {
            let s = scan(&format!("{windir}\\{sub}")).unwrap();
            let items = s.list(0, 50).unwrap().items;
            assert!(!items.is_empty(), "{sub} has items");
            for it in items {
                assert!(s.prepare_delete(it.id).is_err(), "{sub}\\{} must be locked", it.name);
            }
        }
    }

    #[test]
    fn refuses_folders_that_contain_protected_places() {
        // Deleting "Users" from a scan of C:\ would take profile folders and
        // anything protected inside with it.
        let s = demo();
        assert!(s.prepare_delete(1).is_err() || s.info(1).unwrap().protected.is_some());
        let mut idx = Index::new("C:\\");
        let stuff = idx.push(0, "stuff", 0, 0, flags::DIR);
        let win = idx.push(stuff, "Windows", 0, 0, flags::DIR);
        idx.push(win, "x.dll", 1, 1, 0);
        idx.compute_totals();
        let s = Scanned::new(idx, ScanStats::default(), "test".into());
        // C:\stuff\Windows isn't the real Windows folder, so nothing is locked.
        assert!(s.prepare_delete(stuff).is_ok());
    }

    #[test]
    fn real_path_resolves_short_names() {
        let windir = integrity::windows_dir();
        let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into());
        // C:\PROGRA~1 (8.3 name), if 8.3 names exist on this volume.
        let short = format!("{}PROGRA~1", &pf[..3]);
        if std::path::Path::new(&short).exists() {
            assert_eq!(real_path(&short).unwrap().to_lowercase(), pf.to_lowercase());
        }
        assert_eq!(real_path(&windir.to_string_lossy().to_uppercase()).unwrap().to_lowercase(), windir.to_string_lossy().to_lowercase());
        assert!(real_path("C:").unwrap().ends_with('\\'), "drive letter means the drive root");
        assert!(real_path("Z:\\no\\such\\folder\\diskroot").is_err());
    }

    #[test]
    fn delete_refuses_swapped_items_and_odd_names() {
        let root = std::env::temp_dir().join(format!("diskroot-arm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a").join("f.txt"), b"x").unwrap();
        let s = scan(&root.to_string_lossy()).unwrap();
        let a = s.find_rel("a").unwrap();
        let mut pending = s.prepare_delete(a).unwrap();
        pending.arm().unwrap();
        // Swap folder "a" for another folder with the same name after the
        // user was asked: the delete must be refused, nothing recycled.
        std::fs::rename(root.join("a"), root.join("a-old")).unwrap();
        std::fs::create_dir_all(root.join("a")).unwrap();
        let err = pending.recycle().unwrap_err();
        assert!(err.contains("replaced or moved"), "{err}");
        assert!(root.join("a").exists() && root.join("a-old").exists());

        assert!(safety::unusual_name(std::path::Path::new("C:\\x\\report.")).is_some());
        assert!(safety::unusual_name(std::path::Path::new("C:\\x\\notes \\a")).is_some());
        assert!(safety::unusual_name(std::path::Path::new("C:\\x\\con.txt")).is_some());
        assert!(safety::unusual_name(std::path::Path::new("C:\\x\\console.txt")).is_none());
        assert!(safety::unusual_name(std::path::Path::new(&format!("C:\\{}", "a".repeat(300)))).is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `cargo test --release -p diskroot-session memory_of_a_real_scan -- --ignored --nocapture`
    #[test]
    #[ignore = "scans all of C: (slow); run by hand for memory work"]
    fn memory_of_a_real_scan() {
        let s = scan("C:\\").unwrap();
        let rep = s.memory_report();
        let n = s.summary().files + s.summary().dirs;
        let total: usize = rep.iter().map(|r| r.1).sum();
        for (name, b) in &rep {
            println!("{name:<16} {:>7.1} MB  {:>5.1} B/node", *b as f64 / 1048576.0, *b as f64 / n as f64);
        }
        println!("{:<16} {:>7.1} MB  {:>5.1} B/node  ({n} nodes)", "TOTAL", total as f64 / 1048576.0, total as f64 / n as f64);
    }

    /// The real Fast-scan path, end to end: this (non-elevated) process
    /// creates the pipe and starts the helper through the UAC prompt; the
    /// helper opens C:, locks itself down (exit code 5 if it can't) and
    /// streams the index back. Also checks the cloud-flag fix on a real
    /// drive: only a small share of items may look online-only.
    /// `cargo build --bin diskroot` first, then
    /// `DISKROOT_HELPER=<repo>\target\debug\diskroot.exe cargo test -p diskroot-session real_fast_scan -- --ignored --nocapture`
    #[test]
    #[ignore = "starts the elevated helper (UAC prompt) and scans all of C:"]
    fn real_fast_scan_through_locked_down_helper() {
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let t = Instant::now();
        let s = scan_admin("C:", &cancel).unwrap();
        let n = s.index.len();
        let cloud = (0..n as u32).filter(|&i| s.index.flags(i) & diskroot_core::flags::CLOUD != 0).count();
        println!("fast scan: {n} items, {cloud} cloud, mode {:?}, {:.2} s", s.mode, t.elapsed().as_secs_f64());
        assert!(n > 100_000, "a whole system drive has far more items");
        assert!(s.mode.contains("MFT"), "{}", s.mode);
        assert!(cloud * 20 < n, "{cloud} of {n} items flagged cloud: the EA bit is leaking again");
    }

    /// DEL-06/07 on real files: decoys Windows would silently rewrite
    /// ("report." → "report") must be refused, never hitting the real file.
    #[test]
    fn decoy_names_are_refused_on_real_files() {
        let dir = std::env::temp_dir().join(format!("diskroot-decoys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let verbatim = |name: &str| PathBuf::from(format!(r"\\?\{}\{name}", dir.display()));
        std::fs::write(dir.join("report"), b"the real one").unwrap();
        for decoy in ["report.", "report ", "CON.txt", "nul"] {
            std::fs::write(verbatim(decoy), b"decoy").unwrap();
        }
        let s = scan(&dir.to_string_lossy()).unwrap();
        for decoy in ["report.", "report ", "CON.txt", "nul"] {
            let n = s.find_rel(decoy).unwrap_or_else(|| panic!("scan found {decoy:?}"));
            assert!(s.prepare_delete(n).is_err(), "{decoy:?} must be refused");
        }
        let real = s.find_rel("report").unwrap();
        assert!(s.prepare_delete(real).is_ok());
        assert_eq!(std::fs::read(dir.join("report")).unwrap(), b"the real one");
        for decoy in ["report.", "report ", "CON.txt", "nul"] {
            let _ = std::fs::remove_file(verbatim(decoy));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn largest_files_and_types() {
        let s = demo();
        let top: Vec<String> = s.top_files(2, "").into_iter().map(|r| r.name).collect();
        assert_eq!(top, ["movie.mp4", "a.js"]);
        let js = s.top_files(10, "JS");
        assert_eq!(js.len(), 1, "case-insensitive name search");
        assert!(js[0].path.ends_with(r"node_modules\a.js"));
        let types = s.file_types(10);
        assert_eq!((types[0].ext.as_str(), types[0].bytes, types[0].files), ("mp4", 9000, 1));
        assert_eq!(types.len(), 3);
    }

    /// "Mark all" takes folders a green rule matched (node_modules, a
    /// CACHEDIR.TAG build folder), never the folder holding files found by
    /// extension (Desktop for videos).
    #[test]
    fn mark_all_takes_only_safe_matched_places() {
        let mut idx = Index::new("D:\\");
        let users = idx.push(0, "Users", 0, 0, flags::DIR);
        let me = idx.push(users, "me", 0, 0, flags::DIR);
        let proj = idx.push(me, "proj", 0, 0, flags::DIR);
        let nm = idx.push(proj, "node_modules", 0, 0, flags::DIR);
        idx.push(nm, "x.js", 10, 4096, 0);
        let nested = idx.push(nm, "dep", 0, 0, flags::DIR);
        let nm2 = idx.push(nested, "node_modules", 0, 0, flags::DIR);
        idx.push(nm2, "y.js", 10, 4096, 0);
        let target = idx.push(proj, "target", 0, 0, flags::DIR);
        idx.push(target, "CACHEDIR.TAG", 1, 4096, 0);
        idx.push(target, "app.exe", 10, 4096, 0);
        let desk = idx.push(me, "Desktop", 0, 0, flags::DIR);
        idx.push(desk, "film.mp4", 10, 4096, 0);
        idx.push(desk, "notes.txt", 10, 4096, 0);
        idx.compute_totals();
        let s = Scanned::new(idx, ScanStats::default(), "test".into());
        let key = |name: &str| {
            let i = s.rules().subs.iter().position(|x| x.name == name).unwrap();
            CatKey::Sub(i as u16).key()
        };
        let nm_marks = s.markable(&key("node_modules"));
        assert!(nm_marks.contains(&nm) && nm_marks.contains(&nm2), "{nm_marks:?}");
        assert_eq!(s.markable(&key("Build output")), vec![target]);
        assert!(s.markable(&key("Video")).is_empty(), "Desktop must never be marked for its videos");
        assert!(s.markable("cat:3").is_empty() && s.markable("bogus").is_empty());
        // No program uses node_modules: nothing to warn about.
        assert!(s.prepare_delete(nm).unwrap().warnings.apps_running.is_empty());
    }

    /// Live updates: a grown file moves its folders, a deleted one leaves the
    /// tree, a new one is reported but not inserted.
    #[test]
    fn live_updates_patch_the_tree() {
        let dir = std::env::temp_dir().join(format!("diskroot-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("grow.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(dir.join("gone.bin"), vec![0u8; 3000]).unwrap();
        let mut s = scan(&dir.display().to_string()).unwrap();
        let before = s.summary().on_disk;
        std::fs::write(dir.join("sub").join("grow.bin"), vec![0u8; 100_000]).unwrap();
        std::fs::remove_file(dir.join("gone.bin")).unwrap();
        std::fs::write(dir.join("fresh.bin"), vec![0u8; 7]).unwrap();
        let r = s.apply_live(&["sub\\grow.bin".into(), "gone.bin".into(), "fresh.bin".into(), "..\\x".into(), String::new()]);
        assert_eq!((r.resized, r.removed), (1, 1));
        assert_eq!(r.new, vec![("fresh.bin".to_owned(), 7)]);
        let sub = s.find_rel("sub").unwrap();
        assert_eq!(s.index.alloc(sub), 102_400, "100,000 bytes round up to 25 clusters");
        assert!(s.find_rel("gone.bin").is_none());
        assert_eq!(s.summary().on_disk, before - 4096 - 4096 + 102_400);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The folder walk records real last-modified days: a file written now
    /// is from today, and its folders show it as their newest.
    #[test]
    fn walk_records_real_dates() {
        let dir = std::env::temp_dir().join(format!("diskroot-dates-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("new.txt"), b"x").unwrap();
        let s = scan(&dir.display().to_string()).unwrap();
        let today = diskroot_core::today_days();
        let f = s.find_rel("sub\\new.txt").unwrap();
        for id in [f, s.find_rel("sub").unwrap(), 0] {
            assert!(today.abs_diff(s.index.mtime(id)) <= 1, "node {id}: {} vs today {today}", s.index.mtime(id));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `cargo test -p diskroot-session orphan_hints_on_this_pc -- --ignored --nocapture`
    #[test]
    #[ignore = "reads this PC's AppData and installed programs; prints the hints"]
    fn orphan_hints_on_this_pc() {
        let appdata = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).parent().unwrap().to_path_buf();
        let s = scan(&appdata.display().to_string()).unwrap();
        let p = orphans::Programs::installed();
        let loose = s.orphan_hints(&p, 1 << 20, 0);
        println!("loose (>= 1 MB, any age): {}", loose.len());
        for h in loose.iter().take(15) {
            println!("   {:>10}  {}", diskroot_core::format::bytes(h.on_disk), h.name);
        }
        let hints = s.orphan_hints(&p, 50 << 20, 180);
        println!("{} programs known; {} hints", p.count, hints.len());
        for h in &hints {
            println!("{:>10}  {}  ({})", diskroot_core::format::bytes(h.on_disk), h.path, diskroot_core::age_text(diskroot_core::today_days(), h.mtime));
        }
    }

    /// Hints only for big, long-unchanged app-data folders that no known
    /// program matches; never for Windows' own or recently used ones.
    #[test]
    fn orphan_hints_need_all_the_evidence() {
        let mut idx = Index::new("C:\\");
        let users = idx.push(0, "Users", 0, 0, flags::DIR);
        let me = idx.push(users, "me", 0, 0, flags::DIR);
        let ad = idx.push(me, "AppData", 0, 0, flags::DIR);
        let local = idx.push(ad, "Local", 0, 0, flags::DIR);
        let old = diskroot_core::today_days() - 400;
        let mut folder = |name: &str, days: u16, bytes: u64| {
            let d = idx.push(local, name, 0, 0, flags::DIR);
            let f = idx.push(d, "data.bin", bytes, bytes, 0);
            idx.set_mtime(f, days);
            d
        };
        let gone = folder("OldGameStudio", old, 200 << 20);
        folder("Adobe", old, 200 << 20);
        folder("Microsoft", old, 200 << 20);
        folder("FreshApp", diskroot_core::today_days(), 200 << 20);
        folder("TinyOld", old, 1 << 20);
        let pd = idx.push(0, "ProgramData", 0, 0, flags::DIR);
        let pd_old = idx.push(pd, "RetiredVendor", 0, 0, flags::DIR);
        let f = idx.push(pd_old, "x.bin", 100 << 20, 100 << 20, 0);
        idx.set_mtime(f, old);
        idx.compute_totals();
        let s = Scanned::new(idx, ScanStats::default(), "test".into());
        let mut p = orphans::Programs::default();
        p.add("Adobe Acrobat");
        p.count = 1;
        let hints = s.orphan_hints(&p, 50 << 20, 180);
        let ids: Vec<u32> = hints.iter().map(|h| h.id).collect();
        assert_eq!(ids, vec![gone, pd_old], "{:?}", hints.iter().map(|h| &h.name).collect::<Vec<_>>());
        assert!(hints.iter().all(|h| h.hint && h.note.contains("check before deleting")));
    }

    #[test]
    fn leftovers_lists_upgrade_folders_and_downloaded_installers() {
        let mut idx = Index::new("C:\\");
        let bt = idx.push(0, "$WINDOWS.~BT", 0, 0, flags::DIR);
        idx.push(bt, "x.esd", 0, 5 << 30, 0);
        let users = idx.push(0, "Users", 0, 0, flags::DIR);
        let me = idx.push(users, "me", 0, 0, flags::DIR);
        let dl = idx.push(me, "Downloads", 0, 0, flags::DIR);
        let setup = idx.push(dl, "setup.exe", 0, 100 << 20, 0);
        let sub = idx.push(dl, "old", 0, 0, flags::DIR);
        let iso = idx.push(sub, "ubuntu.iso", 0, 3 << 30, 0);
        idx.push(dl, "photo.jpg", 0, 1 << 20, 0);
        let docs = idx.push(me, "Documents", 0, 0, flags::DIR);
        idx.push(docs, "tool.exe", 0, 1 << 30, 0);
        idx.compute_totals();
        let s = Scanned::new(idx, ScanStats::default(), "test".into());
        let l = s.leftovers(100);
        let ids: Vec<u32> = l.iter().map(|x| x.id).collect();
        assert_eq!(ids, vec![bt, iso, setup], "biggest first; nothing outside Downloads");
        assert!(!l[0].deletable && l[0].action == Some(Action::StorageSettings), "Windows removes it, not DiskRoot");
        assert!(l[1].deletable && l[1].kind == "Disk image" && l[2].kind == "Installer");
    }

    #[test]
    fn delete_warnings_find_git_and_cloud() {
        let mut idx = Index::new("D:\\");
        let code = idx.push(0, "code", 0, 0, flags::DIR);
        let repo = idx.push(code, "app", 0, 0, flags::DIR);
        let git = idx.push(repo, ".git", 0, 0, flags::DIR);
        idx.push(git, "HEAD", 10, 4096, 0);
        let sync = idx.push(0, "synced", 0, 0, flags::DIR);
        idx.push(sync, "photo.jpg", 5000, 0, flags::CLOUD);
        idx.compute_totals();
        let s = Scanned::new(idx, ScanStats::default(), "test".into());
        let w = s.prepare_delete(code).unwrap().warnings;
        assert_eq!((w.git_repos, w.cloud, w.large), (1, false, false));
        assert!(w.needs_ack());
        let w = s.prepare_delete(sync).unwrap().warnings;
        assert_eq!((w.git_repos, w.cloud), (0, true));
        assert!(!s.prepare_delete(git).unwrap().warnings.cloud);
        assert_eq!(s.prepare_delete(git).unwrap().warnings.git_repos, 1);
    }

    #[test]
    fn apply_delete_updates_everything() {
        let mut s = demo();
        let before = s.summary();
        s.apply_delete(4);
        let after = s.summary();
        assert_eq!(before.on_disk - after.on_disk, 4096);
        assert_eq!(after.files, before.files - 1);
        assert!(s.check(4).is_err());
        assert!(s.cat_children("", 100).unwrap().iter().all(|r| r.name != "Dev stuff"));
    }
}
