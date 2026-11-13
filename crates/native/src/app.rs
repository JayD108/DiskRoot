//! The native window: menu bar, toolbar, tree-table, map, details pane,
//! status bar ("classic utility, modern finish"). All data comes from `diskroot_session`; scans and deletes
//! run on worker threads so the window never freezes.

use std::collections::{BTreeSet, HashSet};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Frame, Key, Layout, Margin, Modal, Rect, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, TextEdit, Ui, ViewportCommand, pos2, vec2,
};
use diskroot_session::compress::{self, Level};
use diskroot_session::integrity::{Integrity, SignatureState};
use diskroot_session::similar;
use diskroot_session::savings::{self, TipAction};
use diskroot_session::schedule::{self, Schedule};
use diskroot_session::health::{self, Level as Health};
use diskroot_session::history::{self, Forecast, History};
use diskroot_session::remember::{self, Changes, PrevSizes};
use diskroot_session::safety::{DeleteWarnings, LogEntry, visible};
use diskroot_session::{Action, DeleteOut, FileRow, ItemInfo, Leftover, PendingDelete, RulesStatus, Safety, Scanned, Shape, Summary, TypeRow};

use crate::map::{self, Hit, Kind, Layout as MapLayout, PaintCtx};
use crate::settings::{Alert, Settings};
use crate::theme::{self, Theme, Tokens};
use crate::{fmt_bytes, fmt_count, fmt_files, fmt_pct};

const ROW_H: f32 = 24.0;
const LIST_LIMIT: usize = 2000;

/// A row in the tree: a real file/folder, or a by-category group.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key_ {
    Node(u32),
    Cat(String),
}

struct Row {
    key: Key_,
    depth: u16,
    name: String,
    bytes: u64,
    files: u64,
    parent_bytes: u64,
    cat: u8,
    cat_label: String,
    expandable: bool,
    dir: bool,
    /// "+ N more" placeholder rows.
    more: usize,
    tag: &'static str,
    /// Last-modified day (folders: newest inside); 0 = unknown / a group.
    mtime: u16,
}

enum JobKind {
    Scan(String),
    Fast(String),
    Delete(String),
    Reopen(String),
    Export(String),
    DeleteMany(usize),
    /// A Windows tool DiskRoot started and waits for (e.g. DISM).
    Tool(String),
    /// DiskRoot's compression engine: title, live counters, total bytes, total files.
    Compress(String, Arc<compress::Progress>, u64, u64),
}

/// A remembered scan opened at startup: the scan, when it was made, the
/// "what changed" between the last two scans, and how long opening took.
struct Reopened {
    scanned: Scanned,
    time: u64,
    changes: Option<Changes>,
    prev: Option<PrevSizes>,
    hist: Option<History>,
    took: Duration,
}

enum Done {
    Scanned(Box<Result<Scanned, String>>),
    Deleted(u32, Result<DeleteOut, String>),
    Reopened(Box<Option<Reopened>>),
    Exported(Result<std::path::PathBuf, String>),
    DeletedMany(Vec<(u32, Result<DeleteOut, String>)>),
    Tool(Result<String, String>),
    Compressed(Box<compress::Report>),
}

struct Job {
    kind: JobKind,
    started: Instant,
    rx: Receiver<Done>,
    /// Set by the Cancel button (Fast scan): stops waiting for the helper.
    cancel: Arc<AtomicBool>,
}

/// Ctrl+K: every command and every folder or file, one search.
#[derive(Default)]
struct Palette {
    query: String,
    sel: usize,
    /// Name matches in the scan for `hits_for` (folders and files, biggest first).
    hits: Vec<(FileRow, bool)>,
    hits_for: String,
    focused: bool,
}

/// Something the command palette (and View menu) can do.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cmd {
    Scan,
    FastScan,
    Up,
    Home,
    SaveSpace,
    Changes,
    Growth,
    Space,
    Writers,
    Health,
    Schedule,
    Dupes,
    Files,
    Similar,
    Leftovers,
    Sunburst,
    Treemap,
    ModeSize,
    ModeGrowth,
    ModeReclaim,
    Animations,
    Tilt,
    Look,
    ByLocation,
    ByCategory,
    ColorCategory,
    ColorAge,
    Focus,
    ToggleList,
    ToggleInspector,
    Light,
    Dark,
    Export,
    Rules,
    Alerts,
    DeletionHistory,
    Memory,
    FreeMemory,
    Shortcuts,
    Security,
    About,
}

/// What was picked in the palette.
enum Pick {
    Cmd(Cmd),
    Node(u32),
}

enum Dialog {
    About,
    /// First launch: what DiskRoot does and doesn't do.
    Welcome,
    Security(Box<Integrity>),
    History(Vec<LogEntry>),
    Changes(Box<Changes>),
    Files(Box<FilesView>),
    Dupes(Box<DupesView>),
    Message(String, String),
    ConfirmDelete(Box<Confirm>),
    ConfirmMany(Box<ConfirmMany>),
    /// File > Custom rules: where the file is and whether it loaded.
    Rules,
    /// View > Size alerts.
    Alerts,
    /// Compress (or uncompress) a folder with DiskRoot's engine.
    Compress(Box<CompressAsk>),
    /// View > Similar folders.
    Similar(Box<similar::Found>),
    /// Compacting a WSL/Docker virtual disk: (path, size, in use).
    Vhdx(String, u64, bool),
    /// View > Leftovers.
    Leftovers(Vec<Leftover>),
    /// View > Ways to save space.
    Savings(Box<SavingsView>),
    /// DiskRoot's own memory and CPU, and freeing memory.
    Memory,
    /// View > Scheduled scans.
    Schedule(Box<ScheduleView>),
    /// View > Size history.
    Growth(Box<GrowthView>),
    /// View > Drive health.
    Health(Box<HealthView>),
    /// View > Who's writing.
    Writers(Box<WritersView>),
    /// Home: everything important on one screen.
    Home(Box<HomeView>),
    /// View > Where every GB is.
    Space(Box<diskroot_session::SpaceReport>),
    /// Folders the scan couldn't read: (scan root, paths).
    Unreadable(Box<(String, Vec<String>)>),
    /// Explains a Windows tool before starting it.
    Tool(Action),
}

/// Ways to save space: what could be compressed (tab 0) and other ways (tab 1).
struct SavingsView {
    tab: u8,
    /// "Not changed for at least" this many days.
    min_days: u16,
    sugg: savings::Suggestions,
    tips: Vec<savings::Tip>,
    /// A recompute for a new `min_days`, running on a worker thread.
    pending: Option<Receiver<savings::Suggestions>>,
}

/// The Scheduled scans dialog: the schedule being edited, whether the
/// Windows task exists, the recommendation, and recent runs.
struct ScheduleView {
    s: Schedule,
    on: bool,
    installed: bool,
    rec: (u32, String),
    runs: Vec<schedule::Run>,
    msg: Option<String>,
}

/// View > Size history: the root's size (or the drive's used space) over
/// time, the forecast, and the top growers per period.
struct GrowthView {
    root: String,
    /// Drive root: the chart shows the drive's used space.
    drive: bool,
    chart: Vec<(u64, u64)>,
    forecast: Option<Forecast>,
    /// Last day / week / month.
    tab: usize,
    growers: [Option<Changes>; 3],
    points: usize,
    first: u64,
    disk: u64,
    /// Log files that grew in each of the last 3+ scans.
    logs: Vec<diskroot_session::logs::Growing>,
}

/// (kind of data, bytes, files) that a failed drive would take with it.
type AtRisk = (&'static str, u64, u64);

/// View > Drive health: each drive with its trend, and Windows' TRIM setting.
struct HealthView {
    drives: Vec<(health::Drive, Option<health::Trend>)>,
    trim: Option<bool>,
    /// The scan on screen, if it's a whole drive: (its letter "C:", what
    /// can't be downloaded or rebuilt again: (kind, bytes, files)).
    at_risk: Option<(String, Vec<AtRisk>)>,
}

impl HealthView {
    /// Reads the drives now (a few storage queries), saves today's note,
    /// and works out each drive's trend from the notes.
    fn read() -> Self {
        let drives = health::drives();
        health::record_daily(&drives);
        let snaps = health::snaps();
        Self { drives: drives.into_iter().map(|d| {
            let t = health::trend(&snaps, &d);
            (d, t)
        }).collect(), trim: health::trim_enabled(), at_risk: None }
    }
}

/// View > Who's writing: a live sample (every 2 s while open), what
/// background scans saw while the PC was idle, and folder hotspots.
struct WritersView {
    sampler: diskroot_session::writes::Sampler,
    last: Instant,
    /// (disk number, its letters).
    disks: Vec<(u32, String)>,
    /// From scheduled runs in the last 7 days: (program, average bytes/s
    /// while idle, in how many idle samples it showed up), and how many
    /// idle samples there were.
    away: Vec<(String, f64, usize)>,
    away_runs: usize,
    /// Settings that write a lot, as plain advice.
    advice: Vec<String>,
}

/// Advice about Windows settings that write a lot (read-only checks).
fn write_advice() -> Vec<String> {
    let mut out = Vec::new();
    let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    if let Ok(m) = std::fs::metadata(format!("{sys}\\hiberfil.sys")) {
        out.push(format!(
            "Hibernation: {sys}\\hiberfil.sys is {}. Each hibernate writes up to that much (with Fast Startup, also at shut down). If you never hibernate, \"powercfg /h off\" (as admin) removes it and those writes.",
            fmt_bytes(m.len())
        ));
    }
    if let Ok(m) = std::fs::metadata(format!("{sys}\\pagefile.sys")) {
        out.push(format!("Page file: {sys}\\pagefile.sys is {}. Windows writes to it when memory runs short; more RAM or closing heavy programs means fewer writes. Removing it is not recommended.", fmt_bytes(m.len())));
    }
    out.push("Browsers and chat apps write their caches all the time; it's normal, and usually a few GB a day at most.".into());
    out
}

impl WritersView {
    fn open() -> Self {
        let mut disks: Vec<(u32, String)> = Vec::new();
        for d in diskroot_session::drives() {
            if let Some(n) = health::disk_of(&d) {
                let l = d.trim_end_matches('\\').to_owned();
                match disks.iter_mut().find(|x| x.0 == n) {
                    Some(x) => {
                        x.1.push(' ');
                        x.1.push_str(&l);
                    }
                    None => disks.push((n, l)),
                }
            }
        }
        let week = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()).saturating_sub(7 * 86_400);
        let idle_runs: Vec<schedule::Run> = schedule::runs().into_iter().filter(|r| r.time >= week && r.idle && !r.writers.is_empty()).collect();
        let mut sum: std::collections::HashMap<String, (f64, usize)> = std::collections::HashMap::new();
        for r in &idle_runs {
            for w in &r.writers {
                let e = sum.entry(w.name.clone()).or_default();
                e.0 += w.rate;
                e.1 += 1;
            }
        }
        let n = idle_runs.len().max(1) as f64;
        let mut away: Vec<(String, f64, usize)> = sum.into_iter().map(|(k, (r, c))| (k, r / n, c)).collect();
        away.sort_by(|a, b| b.1.total_cmp(&a.1));
        away.truncate(8);
        Self { sampler: diskroot_session::writes::Sampler::new(&disks.iter().map(|d| d.0).collect::<Vec<_>>()), last: Instant::now(), disks, away, away_runs: idle_runs.len(), advice: write_advice() }
    }
}

/// Home: the drive at a glance. Built on a worker thread (Save space's
/// tips take ~0.2 s on a whole drive).
struct HomeView {
    root: String,
    /// (total, free) of the volume.
    space: Option<(u64, u64)>,
    nearly_full: Option<String>,
    forecast: Option<String>,
    /// Drive health verdict for this drive, if it reports one.
    health: Option<(health::Level, String)>,
    /// Top 3 ways to get space back: (title, bytes).
    tips: Vec<(String, Option<u64>)>,
    /// What Fast compression could save on old data (rough).
    compress: Option<u64>,
    /// The biggest items under the root: (node, name, bytes).
    biggest: Vec<(u32, String, u64)>,
}

impl HomeView {
    fn build(s: &Scanned) -> Self {
        let root = s.summary().root;
        let space = diskroot_session::disk_space(&root);
        let is_drive = root.len() == 3 && root.ends_with(":\\");
        let nearly_full = space.and_then(|(t, f)| health::nearly_full(&root, t, f, health::is_ssd(&root) == Some(true)));
        let forecast = if is_drive { history::load(&root).map(|h| history::forecast(&h.points).text(root.trim_end_matches('\\'))) } else { None };
        let health = if is_drive {
            let n = health::disk_of(&root);
            health::drives().into_iter().find(|d| Some(d.number) == n).map(|d| d.verdict()).filter(|v| v.0 != health::Level::Unknown)
        } else {
            None
        };
        let mut tips: Vec<(String, Option<u64>)> = s.space_tips().into_iter().map(|t| (t.title, t.bytes)).collect();
        tips.sort_by_key(|t| std::cmp::Reverse(t.1.unwrap_or(0)));
        tips.truncate(3);
        let sugg = s.compress_suggestions(180, SAVINGS_MIN, 200);
        let compress = (sugg.not_possible.is_none() && !sugg.items.is_empty()).then(|| sugg.total(Level::Fast));
        let biggest = s.list(0, 5).map(|l| l.items.into_iter().map(|i| (i.id, i.name, i.on_disk)).collect()).unwrap_or_default();
        Self { root, space, nearly_full, forecast, health, tips, compress, biggest }
    }
}

/// Smallest rough saving (Maximum) a compress suggestion must reach.
const SAVINGS_MIN: u64 = 100 << 20;

/// What a click in Ways to save space asks for (done after the dialog is drawn).
enum SavingsGo {
    Compress(u32, Level),
    Vhdx(u32),
    Open(TipAction),
    Mark(Vec<u32>),
}

/// The Compress dialog: what would be packed (and unpacked), and the level.
struct CompressAsk {
    path: String,
    plan: compress::Plan,
    undo: compress::Plan,
    level: Level,
}

/// Deleting several marked items at once: each one prepared and armed on
/// its own (all the usual checks), plus the ones DiskRoot refused and why.
struct ConfirmMany {
    items: Vec<PendingDelete>,
    refused: Vec<(String, String)>,
    bytes: u64,
    warnings: DeleteWarnings,
    ack: bool,
}

/// Result of the background "remember" step: what changed, and the previous
/// scan's folder sizes for the tree's change column.
type Remembered = (Result<Option<Changes>, String>, Option<PrevSizes>, Option<History>);

/// The drive strip at the bottom of the window.
struct DriveBar {
    root: String,
    /// Volume size and free space (None if Windows didn't say).
    space: Option<(u64, u64)>,
    scanned: u64,
    /// The 5 biggest top-level items: (node, name, bytes on disk, colour category).
    top: Vec<(u32, String, u64, u8)>,
    /// "C: is 95% full. SSDs get slower...": an SSD under 10% free.
    nearly_full: Option<String>,
}

/// A running duplicate search: shared progress, the cancel flag, and where
/// the result arrives.
type DupesRun = (Arc<diskroot_session::dupes::Progress>, Arc<AtomicBool>, Receiver<Vec<diskroot_session::dupes::DupGroup>>);

/// The duplicate finder window: intro, running (progress), or results.
struct DupesView {
    min_mb: u64,
    running: Option<DupesRun>,
    groups: Option<Vec<diskroot_session::dupes::DupGroup>>,
}

/// "Largest files & file types": the search text and current results.
struct FilesView {
    query: String,
    top: Vec<FileRow>,
    types: Vec<TypeRow>,
}

/// The delete confirmation: the item, the warnings to show, and whether
/// the user ticked "I understand" (required when there are warnings).
struct Confirm {
    info: ItemInfo,
    warnings: DeleteWarnings,
    ack: bool,
    /// The delete prepared and "armed" (file identities recorded) when the
    /// dialog opened. Confirming runs exactly this, so anything swapped on
    /// disk while the dialog was open is caught. None for protected items.
    pending: Option<PendingDelete>,
}

/// The map's current root.
#[derive(Clone, PartialEq, Eq)]
enum MapRoot {
    Node(u32),
    Cat(String),
}

struct MapState {
    root: MapRoot,
    nodes: Vec<Shape>,
    kids: Vec<Vec<usize>>,
    cached: Option<(Rect, Kind, MapLayout)>,
    up: Option<Key_>,
    /// Bytes per age bucket under the map root (age view, folder roots).
    ages: Option<[u64; diskroot_session::AGE_UNKNOWN + 1]>,
    /// Path from the top to the map root, for the breadcrumbs: (key, name).
    crumbs: Vec<(Key_, String)>,
    /// Map mode these colours were made for (settings.map_mode).
    mode: u8,
    /// When this map replaced another root (for the short transition).
    born: Option<Instant>,
    /// Growth / reclaimable: colour, legend code (200+ / 210+) and tooltip
    /// line per node. Empty in size mode.
    over: Vec<Color32>,
    codes: Vec<u8>,
    notes: Vec<String>,
}

pub struct DiskRootApp {
    settings: Settings,
    path: String,
    drives: Vec<String>,
    scanned: Option<Scanned>,
    summary: Option<Summary>,
    job: Option<Job>,
    expanded: HashSet<Key_>,
    selected: Option<Key_>,
    filter: String,
    rows: Option<Vec<Row>>,
    map: Option<MapState>,
    hover_tree: Option<Key_>,
    hover_map: Option<usize>,
    dialog: Option<Dialog>,
    status: Option<(String, Instant)>,
    /// Pop-up notices: (text, since).
    toasts: Vec<(String, Instant)>,
    /// DISKROOT_TEST_PALETTE: open the palette with these words once a scan
    /// is shown (screenshot tests can't type).
    test_palette: Option<String>,
    title: String,
    tree_focus: bool,
    /// A folder was passed on the command line while the first-run notice
    /// was showing: scan it once the notice is closed.
    scan_after_welcome: bool,
    /// Background "remember this scan" step (saves the cache, computes
    /// what changed). Separate from `job`: the scan is already on screen.
    remember_rx: Option<Receiver<Remembered>>,
    /// What changed between the previous scan and the current one.
    changes: Option<Changes>,
    /// Set when the tree on screen is a remembered scan (its scan time).
    remembered_at: Option<u64>,
    /// Bottom strip: how full the drive is, and the 5 biggest items.
    drive_bar: Option<DriveBar>,
    /// Folder sizes from the previous scan (for the tree's change column).
    prev_sizes: Option<PrevSizes>,
    /// Items marked with Ctrl+click for deleting several at once.
    marked: BTreeSet<u32>,
    /// Hardlink names of the selected file: (node, all its names).
    links_cache: Option<(u32, Vec<String>)>,
    /// Size history of the scanned root (folders of 1 MB or more).
    hist: Option<Arc<History>>,
    /// For waking the window from other threads (live updates).
    egui_ctx: egui::Context,
    /// Drive letter -> is it an SSD (for the nearly-full hint).
    ssd_cache: std::collections::HashMap<String, Option<bool>>,
    /// Which program or game folders belong to ("Belongs to"); loaded once
    /// on a background thread at start.
    owners: Option<Arc<diskroot_session::owners::Owners>>,
    owners_rx: Option<Receiver<diskroot_session::owners::Owners>>,
    /// Save space being worked out on a worker thread.
    savings_rx: Option<Receiver<SavingsView>>,
    /// Home being worked out on a worker thread.
    home_rx: Option<Receiver<HomeView>>,
    /// Live updates: how often each folder changed (relative path) since `hot_since`.
    hot: std::collections::HashMap<String, u32>,
    hot_since: Instant,
    /// The selected folder's size at each history point: (node, series).
    hist_cache: Option<(u32, Vec<(u64, u64)>)>,
    /// The DiskRoot logo (start screen, About), uploaded once.
    logo: egui::TextureHandle,
    scroll_to_selected: bool,
    /// Focus mode (F): only the map; the list, Inspector and drive strip hide.
    focus: bool,
    /// The command palette (Ctrl+K), when open.
    palette: Option<Palette>,
    /// Save space badge: the biggest single way to get space back (bytes, what).
    badge: Option<(u64, String)>,
    badge_rx: Option<Receiver<Option<(u64, String)>>>,
    /// The scan changed (new scan, delete, compress): work the badge out again.
    badge_dirty: bool,
    /// Map "Reclaimable" mode: safe / check-first bytes per folder (~10 ms
    /// for a whole drive; made when first needed, dropped when the scan changes).
    reclaim: Option<Arc<std::collections::HashMap<u32, [u64; 2]>>>,
    /// The root of the last map built, to tell a new place from a recolour.
    last_map_root: Option<MapRoot>,
    /// Speed guard: frame intervals (ms) measured during a transition.
    anim_frames: Vec<f32>,
    last_frame: Option<(Instant, bool)>,
    /// DISKROOT_TEST_SLOW_FRAME=<ms>: make every frame that slow (tests the guard).
    test_slow: Option<u64>,
    /// The user's own rules: loaded or not, and why.
    rules_status: RulesStatus,
    /// Live updates: the running watch on the scan root.
    watcher: Option<diskroot_session::watch::Watcher>,
    /// New items seen since the scan (relative path -> bytes): not in the
    /// tree until the next scan.
    live_new: std::collections::HashMap<String, u64>,
    /// Windows reported more changes than it could list: rescan to be exact.
    live_overflow: bool,
    /// Flash the taskbar button (a size alert fired).
    attention: bool,
    /// DiskRoot's own RAM and CPU, sampled at most every 2 s.
    footprint: Footprint,
    /// When the window was minimized (for "free memory when minimized").
    minimized_since: Option<Instant>,
    /// The scan was freed from memory and can be reopened: its root.
    freed: Option<String>,
}

/// This process's memory and CPU, for the status bar and the Memory dialog.
#[derive(Clone, Copy, Default)]
struct Footprint {
    at: Option<Instant>,
    cpu_secs: f64,
    /// Working set (RAM in use), peak working set, private bytes.
    ram: u64,
    peak: u64,
    private: u64,
    /// Average over the last sample interval, % of the whole CPU.
    cpu_pct: f64,
}

impl Footprint {
    fn sample(&mut self) {
        let now = Instant::now();
        if self.at.is_some_and(|t| now.duration_since(t) < Duration::from_secs(2)) {
            return;
        }
        let cpu = diskroot_session::process::cpu_seconds().unwrap_or(0.0);
        if let Some(t) = self.at {
            let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
            let wall = now.duration_since(t).as_secs_f64().max(0.001);
            self.cpu_pct = ((cpu - self.cpu_secs) / wall / cores * 100.0).clamp(0.0, 100.0);
        }
        if let Some((ram, peak, private)) = diskroot_session::process::memory() {
            (self.ram, self.peak, self.private) = (ram, peak, private);
        }
        (self.at, self.cpu_secs) = (Some(now), cpu);
    }
}

impl DiskRootApp {
    pub fn new(cc: &eframe::CreationContext<'_>, start: Option<String>) -> Self {
        Self::with_context(&cc.egui_ctx, start)
    }

    /// The app on any driver (eframe/OpenGL or the CPU renderer).
    pub fn with_context(ctx: &egui::Context, start: Option<String>) -> Self {
        theme::install_fonts(ctx);
        // Upgrading from the previous version: move its settings and saved scans first.
        let migrated = diskroot_session::migrate_previous();
        let settings = Settings::load();
        theme::set_classic(settings.classic);
        theme::apply(ctx, if settings.dark { Theme::Dark } else { Theme::Light });
        let mut app = Self {
            path: start.clone().unwrap_or_else(|| settings.last_path.clone()),
            settings,
            drives: diskroot_session::drives(),
            scanned: None,
            summary: None,
            job: None,
            expanded: HashSet::new(),
            selected: None,
            filter: String::new(),
            rows: None,
            map: None,
            hover_tree: None,
            hover_map: None,
            dialog: None,
            status: None,
            title: String::new(),
            tree_focus: true,
            footprint: Footprint::default(),
            minimized_since: None,
            freed: None,
            scan_after_welcome: false,
            remember_rx: None,
            changes: None,
            remembered_at: None,
            drive_bar: None,
            prev_sizes: None,
            marked: BTreeSet::new(),
            links_cache: None,
            hist: None,
            egui_ctx: ctx.clone(),
            ssd_cache: std::collections::HashMap::new(),
            owners: None,
            savings_rx: None,
            home_rx: None,
            focus: false,
            toasts: Vec::new(),
            test_palette: std::env::var("DISKROOT_TEST_PALETTE").ok(),
            palette: None,
            badge: None,
            badge_rx: None,
            badge_dirty: false,
            reclaim: None,
            last_map_root: None,
            anim_frames: Vec::new(),
            last_frame: None,
            test_slow: std::env::var("DISKROOT_TEST_SLOW_FRAME").ok().and_then(|v| v.parse().ok()),
            owners_rx: {
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(diskroot_session::owners::Owners::load());
                });
                Some(rx)
            },
            hot: std::collections::HashMap::new(),
            hot_since: Instant::now(),
            hist_cache: None,
            logo: ctx.load_texture(
                "diskroot-logo",
                egui::ColorImage::from_rgba_unmultiplied([128, 128], include_bytes!("../assets/logo-128.rgba")),
                egui::TextureOptions::LINEAR,
            ),
            scroll_to_selected: false,
            rules_status: diskroot_session::reload_rules(),
            watcher: None,
            live_new: std::collections::HashMap::new(),
            live_overflow: false,
            attention: false,
        };
        if let Some(e) = &app.rules_status.error {
            app.say(format!("Your custom rules weren't loaded ({e}). File > Custom rules shows where they are."));
        }
        if !app.settings.seen_notice {
            app.dialog = Some(Dialog::Welcome);
            app.scan_after_welcome = start.is_some();
        } else if start.is_some() {
            app.start_scan(false);
        } else if app.settings.remember_scans {
            app.start_reopen();
        }
        if migrated.moved {
            app.say("Welcome to DiskRoot: your settings and saved scans from the previous version came along.");
        }
        if migrated.schedule_to_restore && app.dialog.is_none() {
            app.dialog = Some(Dialog::Message(
                "Scheduled scans".into(),
                "Your schedule from the previous version is saved, but its scheduled task ran the old program, so it was removed. To keep scanning automatically, open View → Scheduled scans and press Save.".into(),
            ));
        }
        // Started with "Run as administrator": say so. Everything still works
        // (scans in this window stay folder walks; Fast scan still uses the
        // locked-down helper), but deleting and the rest run with admin rights.
        if diskroot_session::is_elevated() && app.dialog.is_none() {
            app.dialog = Some(Dialog::Message(
                "DiskRoot is running as administrator".into(),
                "DiskRoot doesn't need this, and it's safer without it: started this way, everything this window does (deleting, Compress, Windows' tools) runs with admin rights. Scans here stay normal folder walks, and Fast scan still goes through its locked-down helper.\n\nClose DiskRoot and start it normally (not with \"Run as administrator\") next time.".into(),
            ));
        }
        app
    }

    fn tokens(&self) -> Tokens {
        theme::tokens(if self.settings.dark { Theme::Dark } else { Theme::Light })
    }

    fn kind(&self) -> Kind {
        if self.settings.treemap { Kind::Treemap } else { Kind::Sunburst }
    }

    /// A message in the status bar that also pops up as a notice for a few
    /// seconds (outcomes: deleted, exported, errors…).
    fn say(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        self.toasts.retain(|t| t.0 != msg);
        self.toasts.push((msg.clone(), Instant::now()));
        if self.toasts.len() > 3 {
            self.toasts.remove(0);
        }
        self.status = Some((msg, Instant::now()));
    }

    /// Status bar only (progress and routine messages).
    fn say_quiet(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    /// Notices in the bottom-right corner; click one to dismiss it. No
    /// animation: one repaint when the oldest one expires.
    fn toasts_ui(&mut self, ctx: &egui::Context) {
        const SHOW: Duration = Duration::from_secs(6);
        self.toasts.retain(|t| t.1.elapsed() < SHOW);
        if self.toasts.is_empty() {
            return;
        }
        if let Some(oldest) = self.toasts.iter().map(|t| t.1).min() {
            ctx.request_repaint_after(SHOW.saturating_sub(oldest.elapsed()) + Duration::from_millis(20));
        }
        let t = self.tokens();
        let mut dismiss = None;
        egui::Area::new(egui::Id::unique("diskroot-toasts"))
            .order(egui::Order::Foreground)
            .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -72.0))
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                for (i, (msg, _)) in self.toasts.iter().enumerate() {
                    let resp = Frame::new()
                        .fill(t.panel)
                        .stroke(Stroke::new(1.0, t.line_strong))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(12, 8))
                        .shadow(egui::epaint::Shadow { offset: [0, 2], blur: 8, spread: 0, color: Color32::from_black_alpha(40) })
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(msg).color(t.text)).wrap());
                        })
                        .response
                        .interact(Sense::click())
                        .on_hover_text("Click to dismiss");
                    if resp.clicked() {
                        dismiss = Some(i);
                    }
                    ui.add_space(6.0);
                }
            });
        if let Some(i) = dismiss {
            self.toasts.remove(i);
        }
    }

    fn invalidate(&mut self) {
        self.rows = None;
        self.map = None;
    }

    /// Drops the scan from memory. It stays saved (saved again first if a
    /// delete or compress made the saved copy stale), so it reopens fast.
    fn free_memory(&mut self) {
        let Some(s) = self.scanned.take() else { return };
        let root = s.summary().root;
        if self.settings.remember_scans && !remember::has_index(&root) {
            let (idx, stats, mode) = s.remember_parts();
            if let Err(e) = remember::save_index(&idx, &stats, &mode) {
                self.scanned = Some(s);
                self.say(format!("Couldn't save the scan, so it stays in memory: {e}"));
                return;
            }
        }
        drop(s);
        self.watcher = None;
        self.summary = None;
        self.rows = None;
        self.map = None;
        self.drive_bar = None;
        self.changes = None;
        self.prev_sizes = None;
        self.selected = None;
        self.marked.clear();
        self.links_cache = None;
        self.set_hist(None);
        self.badge = None;
        self.badge_rx = None;
        self.focus = false;
        self.freed = self.settings.remember_scans.then(|| root.clone());
        self.footprint.at = None; // show the new number right away
        self.say(if self.settings.remember_scans { format!("Memory freed. Your scan of {root} is saved: open it from the start screen.") } else { "Memory freed.".to_owned() });
    }

    /// "Free memory when minimized for 10 minutes".
    fn check_minimized(&mut self, ctx: &egui::Context) {
        let minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        if !minimized {
            self.minimized_since = None;
            return;
        }
        let since = *self.minimized_since.get_or_insert_with(Instant::now);
        if self.settings.free_when_minimized && self.scanned.is_some() && self.job.is_none() {
            if since.elapsed() >= Duration::from_secs(600) {
                self.free_memory();
            } else {
                ctx.request_repaint_after(Duration::from_secs(60));
            }
        }
    }

    // ------------------------------------------------------------------
    // Jobs
    // ------------------------------------------------------------------

    fn start_scan(&mut self, fast: bool) {
        if self.job.is_some() {
            return;
        }
        let path = self.path.trim().to_owned();
        if path.is_empty() {
            self.dialog = Some(Dialog::Message("Nothing to scan".into(), "Type a drive like C:\\ or a folder path.".into()));
            return;
        }
        let (tx, rx) = mpsc::channel();
        let p = path.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let c = Arc::clone(&cancel);
        diskroot_session::set_gentle(self.settings.gentle_scan);
        std::thread::spawn(move || {
            let r = if fast { diskroot_session::scan_admin(&p, &c) } else { diskroot_session::scan(&p) };
            let _ = tx.send(Done::Scanned(Box::new(r)));
        });
        self.job = Some(Job { kind: if fast { JobKind::Fast(path) } else { JobKind::Scan(path) }, started: Instant::now(), rx, cancel });
    }

    /// Instant reopen: load the remembered scan of the last path.
    fn start_reopen(&mut self) {
        let path = self.path.trim().to_owned();
        if path.is_empty() || self.job.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let t = Instant::now();
            let r = diskroot_session::reopen(&p).map(|(scanned, time)| Reopened {
                scanned,
                time,
                changes: remember::last_changes(&p),
                prev: remember::previous_sizes(&p),
                hist: history::load(&p),
                took: t.elapsed(),
            });
            let _ = tx.send(Done::Reopened(Box::new(r)));
        });
        self.job = Some(Job { kind: JobKind::Reopen(path), started: Instant::now(), rx, cancel: Arc::default() });
    }

    /// After a fresh scan: save it for instant reopen and work out what
    /// changed, on a background thread (the scan is already on screen).
    fn start_remember(&mut self) {
        let Some(s) = &self.scanned else { return };
        if !self.settings.remember_scans {
            return;
        }
        let (idx, stats, mode) = s.remember_parts();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let r = remember::remember(&idx, &stats, &mode).map_err(|e| e.to_string());
            let prev = remember::previous_sizes(idx.name(0));
            let hist = history::load(idx.name(0));
            health::record_daily(&health::drives());
            let _ = tx.send((r, prev, hist));
        });
        self.remember_rx = Some(rx);
    }

    fn poll_remember(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.remember_rx else { return };
        match rx.try_recv() {
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(200)),
            Err(TryRecvError::Disconnected) => self.remember_rx = None,
            Ok((r, prev, hist)) => {
                self.remember_rx = None;
                self.prev_sizes = prev;
                // Growth colours on the map use these: draw it again.
                if self.settings.map_mode == 1 {
                    self.map = None;
                }
                self.set_hist(hist);
                self.rows = None;
                match r {
                    Ok(Some(ch)) => {
                        let msg = changes_summary(&ch);
                        self.changes = Some(ch);
                        self.say(format!("{msg} · View → What changed"));
                    }
                    Ok(None) => {}
                    // Not fatal: the scan itself is fine, it just won't reopen instantly.
                    Err(e) => self.say(format!("Couldn't remember this scan: {e}")),
                }
            }
        }
    }

    /// Runs the delete the user confirmed (prepared and armed in `ask_delete`).
    fn start_delete(&mut self, pending: PendingDelete) {
        let (tx, rx) = mpsc::channel();
        let (node, name) = (pending.node, pending.name.clone());
        std::thread::spawn(move || {
            let r = pending.recycle();
            let _ = tx.send(Done::Deleted(node, r));
        });
        self.job = Some(Job { kind: JobKind::Delete(name), started: Instant::now(), rx, cancel: Arc::default() });
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        match job.rx.try_recv() {
            Err(TryRecvError::Empty) => {
                // Every frame (vsync) while a job runs, so the progress bar
                // moves smoothly; nothing else redraws while idle.
                ctx.request_repaint();
            }
            Err(TryRecvError::Disconnected) => {
                self.job = None;
                self.dialog = Some(Dialog::Message("Something went wrong".into(), "The background task stopped unexpectedly.".into()));
            }
            Ok(Done::Scanned(r)) => self.finish_scan(*r),
            Ok(Done::Deleted(node, r)) => self.finish_delete(node, r),
            Ok(Done::Reopened(r)) => self.finish_reopen(*r),
            Ok(Done::DeletedMany(results)) => self.finish_delete_many(results),
            Ok(Done::Compressed(r)) => {
                self.job = None;
                self.finish_compress(*r);
            }
            Ok(Done::Tool(r)) => {
                self.job = None;
                self.dialog = Some(match r {
                    Ok(msg) => Dialog::Message("Done".into(), msg),
                    Err(e) => Dialog::Message("The Windows tool didn't finish".into(), e),
                });
            }
            Ok(Done::Exported(r)) => {
                self.job = None;
                match r {
                    Ok(p) => {
                        self.say(format!("Exported to {}", p.display()));
                        let _ = diskroot_session::reveal(&p.display().to_string());
                    }
                    Err(e) => self.dialog = Some(Dialog::Message("Couldn't export".into(), e)),
                }
            }
        }
    }

    fn finish_scan(&mut self, r: Result<Scanned, String>) {
        self.job = None;
        match r {
            Ok(s) => {
                let sum = s.summary();
                self.say_quiet(format!(
                    "Scanned {} in {:.2} s · {}",
                    sum.root,
                    sum.elapsed_ms / 1000.0,
                    sum.mode
                ));
                // The scan's own root path (long form, e.g. not `MRITTU~1`), so the
                // remembered scan is found again under the same name next time.
                self.path = sum.root.clone();
                self.settings.last_path = sum.root.clone();
                self.settings.save();
                remember::clear_seen(&sum.root);
                self.show_scan(s, sum);
                self.remembered_at = None;
                self.changes = None;
                self.prev_sizes = None;
                self.set_hist(None);
                self.start_remember();
            }
            Err(e) => self.dialog = Some(Dialog::Message("Scan didn't finish".into(), e)),
        }
    }

    fn refresh_drive_bar(&mut self) {
        self.drive_bar = self.scanned.as_ref().map(|s| {
            let sum = s.summary();
            let top = s.list(0, 5).map(|l| l.items.into_iter().map(|i| (i.id, i.name, i.on_disk, i.cat)).collect()).unwrap_or_default();
            let space = diskroot_session::disk_space(&sum.root);
            // Asked once per drive letter, not at every refresh (live updates).
            let letter = sum.root.get(..1).unwrap_or("").to_uppercase();
            let ssd = *self.ssd_cache.entry(letter).or_insert_with(|| health::is_ssd(&sum.root));
            let nearly_full = space.and_then(|(total, free)| health::nearly_full(&sum.root, total, free, ssd == Some(true)));
            DriveBar { space, root: sum.root, scanned: sum.on_disk, top, nearly_full }
        });
    }

    fn show_scan(&mut self, s: Scanned, sum: Summary) {
        self.freed = None;
        self.marked.clear();
        self.summary = Some(sum);
        self.scanned = Some(s);
        self.expanded.clear();
        self.expanded.insert(self.root_key());
        self.selected = Some(self.root_key());
        self.invalidate();
        self.refresh_drive_bar();
        self.start_watch();
        self.check_alerts();
        self.badge_dirty = true;
    }

    /// Live updates: (re)starts watching the scan root.
    fn start_watch(&mut self) {
        self.watcher = None;
        self.hot.clear();
        self.hot_since = Instant::now();
        self.live_new.clear();
        self.live_overflow = false;
        if !self.settings.live_updates {
            return;
        }
        if let Some(sum) = &self.summary {
            let ctx = self.egui_ctx.clone();
            match diskroot_session::watch::Watcher::start_with_wake(&sum.root, Box::new(move || ctx.request_repaint())) {
                Ok(w) => self.watcher = Some(w),
                Err(e) => self.say(format!("Live updates are off for this scan: {e}")),
            }
        }
    }

    /// Applies what the watcher reported (at most once a second).
    fn poll_watch(&mut self, ctx: &egui::Context) {
        use diskroot_session::watch::Event;
        let Some(w) = &self.watcher else { return };
        let _ = ctx; // the watcher wakes the window when something changed
        if self.job.is_some() {
            return;
        }
        let mut paths: Vec<String> = Vec::new();
        let mut stopped = None;
        while let Ok(ev) = w.rx.try_recv() {
            match ev {
                Event::Changed(p) => paths.extend(p),
                Event::Overflow => self.live_overflow = true,
                Event::Stopped(e) => stopped = Some(e),
            }
        }
        if let Some(e) = stopped {
            self.watcher = None;
            self.say(format!("Live updates stopped: {e}"));
        }
        if paths.is_empty() {
            return;
        }
        // Folder hotspots (Who's writing): count changes per folder.
        if self.hot.len() < 50_000 {
            for p in &paths {
                let folder = p.rfind('\\').map_or("", |i| &p[..i]);
                *self.hot.entry(folder.to_owned()).or_default() += 1;
            }
        }
        let root_key = self.root_key();
        let Some(s) = &mut self.scanned else { return };
        for p in &paths {
            self.live_new.remove(p);
        }
        let r = s.apply_live(&paths);
        self.live_new.extend(r.new);
        if r.resized + r.removed > 0 {
            self.summary = Some(s.summary());
            if let Some(Key_::Node(n)) = self.selected
                && s.check(n).is_err()
            {
                self.selected = Some(root_key);
            }
            self.marked.retain(|&n| s.check(n).is_ok());
            self.invalidate();
            self.refresh_drive_bar();
            self.check_alerts();
        }
    }

    /// Size alerts: fires (once, then re-arms at the new size) for folders
    /// in the current scan that grew by their threshold.
    fn check_alerts(&mut self) {
        let Some(s) = &self.scanned else { return };
        let root = s.summary().root;
        let mut fired = Vec::new();
        for a in &mut self.settings.alerts {
            let Some(rel) = rel_of(&root, &a.path) else { continue };
            let Some(n) = s.find_rel(&rel) else { continue };
            let now = s.info(n).map(|i| i.on_disk).unwrap_or(0);
            if now >= a.baseline.saturating_add(a.grow) {
                fired.push(format!("{} grew by {} (now {})", a.path, fmt_bytes(now - a.baseline), fmt_bytes(now)));
                a.baseline = now;
            } else if now < a.baseline {
                // Shrank: measure growth from the new, smaller size.
                a.baseline = now;
            }
        }
        if !fired.is_empty() {
            self.settings.save();
            if self.dialog.is_none() {
                self.dialog = Some(Dialog::Message("Size alert".into(), fired.join("\n")));
            } else {
                self.say(fired.join(" · "));
            }
            self.attention = true;
        }
    }
    fn finish_reopen(&mut self, r: Option<Reopened>) {
        self.job = None;
        let Some(r) = r else { return }; // nothing remembered: start empty, as before
        let sum = r.scanned.summary();
        let mut msg = format!(
            "Opened the remembered scan of {} from {} in {:.2} s. Press F5 for a fresh scan.",
            sum.root,
            remember::age_text(r.time),
            r.took.as_secs_f64()
        );
        if r.changes.is_some() {
            msg.push_str(" · View → What changed");
        }
        // Background scans since the user last looked: say what changed.
        let mut changes = r.changes;
        let mut warned: Vec<String> = Vec::new();
        if let Some(away) = remember::away_changes(&sum.root) {
            for w in schedule::runs().iter().filter(|x| x.time > away.before_time).filter_map(|x| x.warning.clone()) {
                if !warned.contains(&w) {
                    warned.push(w);
                }
            }
            let n = schedule::runs().iter().filter(|x| x.error.is_none() && x.time > away.before_time && x.root.eq_ignore_ascii_case(&sum.root)).count();
            let d = i128::from(away.after_total) - i128::from(away.before_total);
            let top = away.items.first().map(|c| format!(" ({} {}{})", if c.path.is_empty() { visible(&sum.root) } else { visible(&c.path) }, if c.delta() >= 0 { "+" } else { "−" }, fmt_bytes(c.delta().unsigned_abs() as u64))).unwrap_or_default();
            msg = format!(
                "{} background scan{} since you last looked: {} {}{}{} · View → What changed",
                n.max(1),
                if n == 1 { "" } else { "s" },
                sum.root,
                if d >= 0 { "+" } else { "−" },
                fmt_bytes(d.unsigned_abs() as u64),
                top
            );
            changes = Some(away);
            remember::clear_seen(&sum.root);
        }
        self.say(msg);
        self.show_scan(r.scanned, sum);
        if !warned.is_empty() && self.dialog.is_none() {
            self.dialog = Some(Dialog::Message("While you were away".into(), warned.join("
")));
        }
        if self.settings.home_at_start && self.dialog.is_none() {
            self.open_home();
        }
        self.remembered_at = Some(r.time);
        self.changes = changes;
        self.prev_sizes = r.prev;
        // Growth colours on the map use these: draw it again.
        if self.settings.map_mode == 1 {
            self.map = None;
        }
        self.set_hist(r.hist);
    }

    fn set_hist(&mut self, h: Option<History>) {
        self.hist = h.filter(|h| !h.points.is_empty()).map(Arc::new);
        self.hist_cache = None;
    }

    fn finish_delete(&mut self, node: u32, r: Result<DeleteOut, String>) {
        self.job = None;
        self.badge_dirty = true;
        match r {
            Ok(out) if out.cancelled => self.say("Cancelled. Nothing was deleted."),
            Ok(out) => {
                if let Some(s) = &mut self.scanned {
                    remember::forget_index(&s.summary().root);
                    let parent = s.info(node).ok().and_then(|i| i.parent);
                    s.apply_delete(node);
                    self.summary = Some(s.summary());
                    self.drive_bar = None; // refreshed below
                    if self.selected == Some(Key_::Node(node)) {
                        self.selected = parent.map(Key_::Node);
                    }
                }
                self.invalidate();
                self.refresh_drive_bar();
                self.say(format!("Moved \"{}\" to the Recycle Bin · {} freed", out.name, fmt_bytes(out.freed)));
            }
            Err(e) => self.dialog = Some(Dialog::Message("Couldn't move it to the Recycle Bin".into(), e)),
        }
    }

    // ------------------------------------------------------------------
    // Tree rows
    // ------------------------------------------------------------------

    fn root_key(&self) -> Key_ {
        if self.settings.by_category { Key_::Cat(String::new()) } else { Key_::Node(0) }
    }

    fn cat_label(&self, own_cat: u8, sub: u16) -> String {
        let c = diskroot_session::categories().get(own_cat as usize).map_or("", |c| c.name);
        match self.scanned.as_ref().and_then(|s| s.rules().subs.get(sub as usize)) {
            Some(s) if s.name != c => format!("{c} › {}", s.name),
            _ => c.to_owned(),
        }
    }

    fn build_rows(&self) -> Vec<Row> {
        let Some(s) = &self.scanned else { return Vec::new() };
        let mut out = Vec::new();
        let root = self.root_key();
        match &root {
            Key_::Node(n) => {
                if let Ok(it) = s.item_of(*n) {
                    out.push(Row {
                        key: root.clone(),
                        depth: 0,
                        name: it.name,
                        bytes: it.on_disk,
                        files: u64::from(it.files),
                        parent_bytes: it.on_disk,
                        cat: it.cat,
                        cat_label: String::new(),
                        expandable: true,
                        dir: true,
                        more: 0,
                        tag: "",
                        mtime: s.mtime(*n),
                    });
                    self.push_node_children(s, *n, it.on_disk, 1, &mut out);
                }
            }
            Key_::Cat(_) => {
                let (bytes, files) = s.cat_total("");
                let sum = self.summary.as_ref().map_or(String::new(), |x| x.root.clone());
                out.push(Row {
                    key: root.clone(),
                    depth: 0,
                    name: format!("{sum} by category"),
                    bytes,
                    files,
                    parent_bytes: bytes,
                    cat: u8::MAX,
                    cat_label: String::new(),
                    expandable: true,
                    dir: true,
                    more: 0,
                    tag: "",
                    mtime: 0,
                });
                self.push_cat_children(s, "", bytes, 1, &mut out);
            }
        }
        if !self.filter.trim().is_empty() {
            let f = self.filter.trim().to_lowercase();
            out.retain(|r| r.depth == 0 || r.name.to_lowercase().contains(&f));
        }
        out
    }

    fn push_node_children(&self, s: &Scanned, node: u32, parent_bytes: u64, depth: u16, out: &mut Vec<Row>) {
        if !self.expanded.contains(&Key_::Node(node)) {
            return;
        }
        let Ok(list) = s.list(node, LIST_LIMIT) else { return };
        for it in &list.items {
            let key = Key_::Node(it.id);
            out.push(Row {
                key,
                depth,
                name: it.name.clone(),
                bytes: it.on_disk,
                files: u64::from(it.files),
                parent_bytes,
                cat: it.cat,
                cat_label: self.cat_label(it.own_cat, it.sub),
                expandable: it.dir && it.files > 0,
                dir: it.dir,
                more: 0,
                tag: if it.link { "link" } else if it.cloud { "cloud" } else { "" },
                mtime: s.mtime(it.id),
            });
            if it.dir {
                self.push_node_children(s, it.id, it.on_disk, depth + 1, out);
            }
        }
        if list.total > list.items.len() {
            out.push(Row {
                key: Key_::Cat(format!("more:{node}")),
                depth,
                name: format!("+ {} smaller items", fmt_count((list.total - list.items.len()) as u64)),
                bytes: 0,
                files: 0,
                parent_bytes,
                cat: u8::MAX,
                cat_label: String::new(),
                expandable: false,
                dir: false,
                more: list.total - list.items.len(),
                tag: "",
                mtime: 0,
            });
        }
    }

    fn push_cat_children(&self, s: &Scanned, key: &str, parent_bytes: u64, depth: u16, out: &mut Vec<Row>) {
        if !self.expanded.contains(&Key_::Cat(key.to_owned())) {
            return;
        }
        let Ok(rows) = s.cat_children(key, LIST_LIMIT) else { return };
        for r in rows {
            match r.node {
                None => {
                    out.push(Row {
                        key: Key_::Cat(r.key.clone()),
                        depth,
                        name: r.name.clone(),
                        bytes: r.bytes,
                        files: r.files,
                        parent_bytes,
                        cat: r.cat,
                        cat_label: String::new(),
                        expandable: true,
                        dir: true,
                        more: 0,
                        tag: "",
                        mtime: 0,
                    });
                    self.push_cat_children(s, &r.key, r.bytes, depth + 1, out);
                }
                Some(n) => {
                    let dir = s.info(n).map(|i| i.dir).unwrap_or(false);
                    out.push(Row {
                        key: Key_::Node(n),
                        depth,
                        name: r.name.clone(),
                        bytes: r.bytes,
                        files: r.files,
                        parent_bytes,
                        cat: r.cat,
                        cat_label: self.cat_label(r.cat, r.sub),
                        expandable: dir,
                        dir,
                        more: 0,
                        tag: "",
                        mtime: s.mtime(n),
                    });
                    if dir {
                        let total = s.item_of(n).map(|i| i.on_disk).unwrap_or(r.bytes);
                        self.push_node_children(s, n, total, depth + 1, out);
                    }
                }
            }
        }
    }

    fn toggle(&mut self, key: &Key_) {
        if !self.expanded.remove(key) {
            self.expanded.insert(key.clone());
        }
        self.rows = None;
    }

    fn select(&mut self, key: Key_) {
        if self.selected.as_ref() != Some(&key) {
            self.selected = Some(key);
            self.map = None;
            self.scroll_to_selected = true;
        }
    }

    /// Selects a node and expands everything above it so it's visible.
    fn reveal_node(&mut self, id: u32, cat: Option<(u8, u16)>) {
        if let Some(s) = &self.scanned {
            if self.settings.by_category {
                self.expanded.insert(Key_::Cat(String::new()));
                if let Some((c, sub)) = cat {
                    self.expanded.insert(Key_::Cat(format!("cat:{c}")));
                    self.expanded.insert(Key_::Cat(format!("sub:{sub}")));
                }
            } else if let Ok(chain) = s.crumbs(id) {
                for c in chain.iter().take(chain.len().saturating_sub(1)) {
                    self.expanded.insert(Key_::Node(c.id));
                }
            }
        }
        self.rows = None;
        self.select(Key_::Node(id));
    }

    fn reveal_cat(&mut self, key: &str, cat: u8) {
        self.expanded.insert(Key_::Cat(String::new()));
        if key.starts_with("sub:") {
            self.expanded.insert(Key_::Cat(format!("cat:{cat}")));
        }
        self.rows = None;
        self.select(Key_::Cat(key.to_owned()));
    }

    // ------------------------------------------------------------------
    // Map model
    // ------------------------------------------------------------------

    fn map_root(&self) -> Option<MapRoot> {
        let s = self.scanned.as_ref()?;
        Some(match &self.selected {
            Some(Key_::Node(n)) => MapRoot::Node(s.info(*n).map(|i| i.folder).unwrap_or(0)),
            Some(Key_::Cat(k)) if !k.starts_with("more:") => MapRoot::Cat(k.clone()),
            _ => {
                if self.settings.by_category { MapRoot::Cat(String::new()) } else { MapRoot::Node(0) }
            }
        })
    }

    fn ensure_map(&mut self) {
        let Some(root) = self.map_root() else {
            self.map = None;
            return;
        };
        if self.map.as_ref().is_some_and(|m| m.root == root && m.mode == self.settings.map_mode) {
            return;
        }
        let Some(s) = &self.scanned else { return };
        let (nodes, up, crumbs) = match &root {
            MapRoot::Node(n) => {
                let nodes = s.view(*n, 3).unwrap_or_default();
                let up = s.info(*n).ok().and_then(|i| i.parent).map(Key_::Node);
                let crumbs = s.crumbs(*n).map(|c| c.into_iter().map(|c| (Key_::Node(c.id), c.name)).collect()).unwrap_or_default();
                (nodes, up, crumbs)
            }
            MapRoot::Cat(k) => match s.category_view(k) {
                Ok(v) => {
                    let up = (v.crumbs.len() > 1).then(|| Key_::Cat(v.crumbs[v.crumbs.len() - 2].key.clone()));
                    let crumbs = v.crumbs.into_iter().map(|c| (Key_::Cat(c.key), c.name)).collect();
                    (v.nodes, up, crumbs)
                }
                Err(_) => (Vec::new(), None, Vec::new()),
            },
        };
        let kids = map::children(&nodes);
        let ages = match (&root, self.settings.color_by_age) {
            (MapRoot::Node(n), true) => Some(s.age_breakdown(*n, diskroot_session::today_days())),
            _ => None,
        };
        let (over, codes, notes) = self.mode_colors(&nodes);
        // A new place (not just new colours): a short transition.
        let born = (self.settings.animations && self.last_map_root.as_ref().is_some_and(|r| *r != root)).then(Instant::now);
        self.last_map_root = Some(root.clone());
        self.map = Some(MapState { root, nodes, kids, cached: None, up, ages, crumbs, mode: self.settings.map_mode, born, over, codes, notes });
    }

    /// Index of a view node matching a tree key.
    fn map_index(&self, key: &Key_) -> Option<usize> {
        let m = self.map.as_ref()?;
        m.nodes.iter().position(|n| match key {
            Key_::Node(id) => n.key.is_empty() && n.id == i64::from(*id),
            Key_::Cat(k) => !n.key.is_empty() && &n.key == k,
        })
    }

    // ------------------------------------------------------------------
    // UI pieces
    // ------------------------------------------------------------------

    fn menu_bar(&mut self, ui: &mut Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.add(egui::Button::new("Scan").shortcut_text("F5")).clicked() {
                    self.start_scan(false);
                    ui.close();
                }
                if ui.add(egui::Button::new("Fast scan (MFT, admin)").shortcut_text("Ctrl+F5")).clicked() {
                    self.start_scan(true);
                    ui.close();
                }
                ui.separator();
                let mut keep = self.settings.remember_scans;
                if ui
                    .checkbox(&mut keep, "Remember scans")
                    .on_hover_text("Saves each scan on this PC (names and sizes only) so it reopens instantly and DiskRoot can show what changed.")
                    .changed()
                {
                    self.settings.remember_scans = keep;
                    self.settings.save();
                }
                if ui.button("Forget remembered scans").clicked() {
                    match remember::forget_all() {
                        Ok(n) => self.say(format!("Forgot all remembered scans ({n} files deleted).")),
                        Err(e) => self.say(format!("Couldn't delete the remembered scans: {e}")),
                    }
                    self.changes = None;
                    ui.close();
                }
                if ui.add_enabled(self.scanned.is_some() && self.job.is_none(), egui::Button::new("Export to CSV")).on_hover_text("Every file and folder of this scan, saved in Documents\\DiskRoot").clicked() {
                    self.start_export();
                    ui.close();
                }
                if ui
                    .add_enabled(!self.marked.is_empty(), egui::Button::new("Export marked as PowerShell script…"))
                    .on_hover_text("A script that moves the marked items to the Recycle Bin, re-checking each one. DiskRoot only writes it; you review and run it.")
                    .clicked()
                {
                    self.export_plan();
                    ui.close();
                }
                if ui.button("Custom rules…").on_hover_text("Your own categories for folders and file types").clicked() {
                    self.dialog = Some(Dialog::Rules);
                    ui.close();
                }
                ui.separator();
                if ui.button("Deletion history…").clicked() {
                    self.open_history();
                    ui.close();
                }
                ui.separator();
                if ui.button("Exit").clicked() {
                    ui.ctx().send_viewport_cmd(ViewportCommand::Close);
                }
            });
            ui.menu_button("View", |ui| {
                if ui.add_enabled(self.changes.is_some(), egui::Button::new("What changed since the previous scan…")).clicked() {
                    if let Some(c) = &self.changes {
                        self.dialog = Some(Dialog::Changes(Box::new(c.clone())));
                    }
                    ui.close();
                }
                if ui
                    .add_enabled(self.scanned.is_some(), egui::Button::new("Ways to save space…"))
                    .on_hover_text("What could be compressed (big, not changed for a while), and other ways to get space back")
                    .clicked()
                {
                    self.open_savings();
                    ui.close();
                }
                if ui
                    .add_enabled(self.summary.is_some(), egui::Button::new("Size history…"))
                    .on_hover_text("How this drive or folder grew over time, what grew the most, and when the drive would be full")
                    .clicked()
                {
                    self.open_growth();
                    ui.close();
                }
                if ui
                    .add_enabled(self.scanned.is_some(), egui::Button::new("Where every GB is…"))
                    .on_hover_text("The drive's used space against what this scan counted, and what the rest is")
                    .clicked()
                {
                    self.open_space();
                    ui.close();
                }
                if ui.button("Who's writing…").on_hover_text("Which programs write to your drives right now and while the PC is idle, and which folders change most (no admin)").clicked() {
                    self.dialog = Some(Dialog::Writers(Box::new(WritersView::open())));
                    ui.close();
                }
                if ui.button("Drive health…").on_hover_text("How worn your SSDs are, how much was written to them, temperature and errors (the drives' own reports; no admin)").clicked() {
                    self.open_health();
                    ui.close();
                }
                if ui.button("Scheduled scans…").on_hover_text("Let DiskRoot scan by itself every few hours (no admin, no window), so it can show what changed").clicked() {
                    self.open_schedule();
                    ui.close();
                }
                if ui.add_enabled(self.scanned.is_some(), egui::Button::new("Find duplicate files…")).clicked() {
                    self.open_dupes();
                    ui.close();
                }
                if ui.add_enabled(self.scanned.is_some(), egui::Button::new("Largest files & file types…")).clicked() {
                    self.open_files();
                    ui.close();
                }
                if ui
                    .add_enabled(self.scanned.is_some(), egui::Button::new("Similar folders…"))
                    .on_hover_text("Folders that look like copies of each other, and folders with similar names (project, project_v2, project - Copy)")
                    .clicked()
                {
                    self.open_similar();
                    ui.close();
                }
                if ui
                    .add_enabled(self.scanned.is_some(), egui::Button::new("Leftovers…"))
                    .on_hover_text("Old installers, archives and disk images in Downloads, and Windows upgrade leftovers")
                    .clicked()
                {
                    self.open_leftovers();
                    ui.close();
                }
                ui.separator();
                if ui.add(egui::Button::new("Command palette…").shortcut_text("Ctrl+K")).on_hover_text("Every command and every folder, one search").clicked() {
                    self.palette = Some(Palette::default());
                    ui.close();
                }
                let mut list = !self.settings.hide_list;
                if ui.add(egui::Checkbox::new(&mut list, "Folder list")).on_hover_text("Show or minimize the list on the left (Ctrl+B)").changed() {
                    self.run_cmd(ui.ctx(), Cmd::ToggleList);
                }
                let mut insp = !self.settings.hide_inspector;
                if ui.add(egui::Checkbox::new(&mut insp, "Inspector")).on_hover_text("Show or minimize the details at the bottom (Ctrl+J)").changed() {
                    self.run_cmd(ui.ctx(), Cmd::ToggleInspector);
                }
                let mut tilt = self.settings.map_25d;
                if ui.checkbox(&mut tilt, "Tilted map (2.5D)").on_hover_text("Boxes and rings stand up by size; 2D is the default").changed() {
                    self.run_cmd(ui.ctx(), Cmd::Tilt);
                }
                let mut anim = self.settings.animations;
                if ui.checkbox(&mut anim, "Smooth transitions").on_hover_text("A short movement when the map opens a folder or goes up").changed() {
                    self.settings.animations = anim;
                    self.settings.save();
                }
                let mut focus = self.focus;
                if ui.add_enabled(self.scanned.is_some(), egui::Checkbox::new(&mut focus, "Focus on the map")).on_hover_text("Only the map (F; Esc to leave)").changed() {
                    self.run_cmd(ui.ctx(), Cmd::Focus);
                }
                ui.separator();
                let mut changed = false;
                changed |= ui.radio_value(&mut self.settings.treemap, false, "Sunburst map").clicked();
                changed |= ui.radio_value(&mut self.settings.treemap, true, "Treemap").clicked();
                let mut live = self.settings.live_updates;
                if ui.checkbox(&mut live, "Live updates").on_hover_text("Watch the scanned folder while DiskRoot is open and update sizes as files change").changed() {
                    self.settings.live_updates = live;
                    self.settings.save();
                    if live { self.start_watch() } else { self.watcher = None }
                }
                if ui.add(egui::Button::new(format!("Size alerts ({})…", self.settings.alerts.len()))).clicked() {
                    self.dialog = Some(Dialog::Alerts);
                    ui.close();
                }
                ui.separator();
                let age = self.settings.color_by_age;
                ui.radio_value(&mut self.settings.color_by_age, false, "Colour by category");
                ui.radio_value(&mut self.settings.color_by_age, true, "Colour by age (last modified)");
                if age != self.settings.color_by_age {
                    self.settings.save();
                    self.map = None;
                    self.rows = None;
                }
                ui.separator();
                let before = self.settings.by_category;
                ui.radio_value(&mut self.settings.by_category, false, "Group by location");
                ui.radio_value(&mut self.settings.by_category, true, "Group by category");
                if before != self.settings.by_category {
                    self.regroup();
                }
                ui.separator();
                let classic = self.settings.classic;
                ui.radio_value(&mut self.settings.classic, false, "Studio look");
                ui.radio_value(&mut self.settings.classic, true, "Classic look");
                if classic != self.settings.classic {
                    theme::set_classic(self.settings.classic);
                    theme::apply(ui.ctx(), if self.settings.dark { Theme::Dark } else { Theme::Light });
                    self.map = None;
                    self.rows = None;
                    changed = true;
                }
                ui.separator();
                let dark = self.settings.dark;
                ui.radio_value(&mut self.settings.dark, false, "Light theme");
                ui.radio_value(&mut self.settings.dark, true, "Dark theme");
                if dark != self.settings.dark {
                    theme::apply(ui.ctx(), if self.settings.dark { Theme::Dark } else { Theme::Light });
                    changed = true;
                }
                if changed {
                    self.settings.save();
                    if let Some(m) = &mut self.map {
                        m.cached = None;
                    }
                }
            });
            ui.menu_button("Help", |ui| {
                if ui.button("Security & privacy").clicked() {
                    self.open_security();
                    ui.close();
                }
                if ui.button("Memory and CPU…").clicked() {
                    self.dialog = Some(Dialog::Memory);
                    ui.close();
                }
                if ui.button("Keyboard shortcuts").clicked() {
                    self.dialog = Some(Dialog::Message(
                        "Keyboard shortcuts".into(),
                        [
                            "Ctrl+K · Command palette: every command and every folder, one search",
                            "F · Focus on the map (Esc to leave)",
                            "Ctrl+B · Show or minimize the folder list",
                            "Ctrl+J · Show or minimize the Inspector",
                            "F5 · Scan (no admin)",
                            "Ctrl+F5 · Fast scan (admin, whole NTFS drives)",
                            "Ctrl+H · Home: the drive at a glance",
                            "Ctrl+F · Filter by name",
                            "↑ / ↓ · Move in the tree",
                            "→ / ← · Open / close a folder (← on a closed one goes to its parent)",
                            "Enter · Open or close the selected folder",
                            "Backspace · Up one folder",
                            "Space or Ctrl+click · Mark / unmark (to move several to the Recycle Bin at once)",
                            "Delete · Move the selected or marked items to the Recycle Bin (always asks first)",
                            "Esc · Close a window",
                        ]
                        .join("
"),
                    ));
                    ui.close();
                }
                if ui.button("About DiskRoot").clicked() {
                    self.dialog = Some(Dialog::About);
                    ui.close();
                }
            });
            // Right: DiskRoot's own footprint, Free memory, Focus, Security.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let t = self.tokens();
                if shield_button(ui, &t).on_hover_text("Security & privacy: what DiskRoot does with your disk").clicked() {
                    self.open_security();
                }
                if ui
                    .add(egui::Button::new(RichText::new("Commands").size(11.5)).shortcut_text(RichText::new("Ctrl+K").size(11.0)).small())
                    .on_hover_text("Every command and every folder, one search (Ctrl+K)")
                    .clicked()
                {
                    self.palette = Some(Palette::default());
                }
                if ui
                    .add_enabled(self.scanned.is_some(), egui::Button::new(RichText::new("Free memory").size(11.5)).small())
                    .on_hover_text("Drop the scan from memory. It stays saved on this PC and reopens in about a second.")
                    .clicked()
                {
                    self.free_memory();
                }
                self.footprint.sample();
                let f = self.footprint;
                let r = ui
                    .add(egui::Label::new(RichText::new(format!("●  RAM {} · CPU {:.0}%", fmt_bytes(f.ram), f.cpu_pct)).color(t.muted).size(11.5)).sense(Sense::click()))
                    .on_hover_text(format!(
                        "DiskRoot's own use of this PC.\nRAM in use: {} (peak {}, private {}).\nCPU: {:.1}% of the whole processor.\nClick to see what uses the memory.",
                        fmt_bytes(f.ram),
                        fmt_bytes(f.peak),
                        fmt_bytes(f.private),
                        f.cpu_pct
                    ))
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if r.clicked() {
                    self.dialog = Some(Dialog::Memory);
                }
                if self.scanned.is_some() && ui.selectable_label(self.focus, RichText::new("Focus").size(11.5)).on_hover_text("Only the map (F; Esc to leave)").clicked() {
                    self.focus = !self.focus;
                }
            });
        });
    }

    fn regroup(&mut self) {
        self.marked.clear();
        self.settings.save();
        self.expanded.clear();
        let root = self.root_key();
        self.expanded.insert(root.clone());
        self.selected = Some(root);
        self.invalidate();
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let t = self.tokens();
        ui.horizontal_centered(|ui| {
            // Narrow window (down to the 960 px minimum): shorter labels and
            // spacing, so everything incl. the filter box still fits.
            let compact = ui.available_width() < 1180.0;
            if compact {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.spacing_mut().button_padding.x = 5.0;
            }
            ui.add(TextEdit::singleline(&mut self.path).desired_width(if compact { 100.0 } else { 160.0 }).hint_text("C:\\ or a folder"));
            ui.menu_button("▾", |ui| {
                for d in self.drives.clone() {
                    if ui.button(&d).clicked() {
                        self.path = d;
                        ui.close();
                    }
                }
            });
            let busy = self.job.is_some();
            if ui.add_enabled(!busy, egui::Button::new("▶  Scan")).on_hover_text("Scan this drive or folder (F5)").clicked() {
                self.start_scan(false);
            }
            let fast = egui::Button::new(RichText::new(if compact { "⚡ Fast scan" } else { "⚡  Fast scan" }).color(t.accent_ink)).fill(t.accent);
            if ui
                .add_enabled(!busy, fast)
                .on_hover_text("Reads the drive's Master File Table, read-only. Windows asks for admin permission. (Ctrl+F5)")
                .clicked()
            {
                self.start_scan(true);
            }
            ui.separator();
            let up = self.map.as_ref().and_then(|m| m.up.clone());
            if ui.add_enabled(up.is_some(), egui::Button::new("↑")).on_hover_text("Up one level (Backspace)").clicked()
                && let Some(k) = up {
                    self.go_to(k);
                }
            ui.separator();
            let mut tm = self.settings.treemap;
            ui.selectable_value(&mut tm, false, if compact { "Sunburst" } else { "◎ Sunburst" });
            ui.selectable_value(&mut tm, true, if compact { "Treemap" } else { "▦ Treemap" });
            if tm != self.settings.treemap {
                self.settings.treemap = tm;
                self.settings.save();
                if let Some(m) = &mut self.map {
                    m.cached = None;
                }
            }
            ui.separator();
            let mut bc = self.settings.by_category;
            ui.selectable_value(&mut bc, false, "Location");
            ui.selectable_value(&mut bc, true, "Category");
            if bc != self.settings.by_category {
                self.settings.by_category = bc;
                self.regroup();
            }
            ui.separator();
            let home_key = ui.input(|i| i.modifiers.ctrl && i.key_pressed(Key::H));
            if (ui.add_enabled(self.scanned.is_some(), egui::Button::new("Home")).on_hover_text("Everything important on one screen (Ctrl+H)").clicked() || home_key) && self.scanned.is_some() {
                self.open_home();
            }
            let mut job = egui::text::LayoutJob::default();
            let body = ui.style().text_styles.get(&egui::TextStyle::Button).cloned().unwrap_or_else(|| theme::regular(13.0));
            job.append("Save space", 0.0, egui::TextFormat { font_id: body, color: t.text, valign: Align::Center, ..Default::default() });
            let tip = match &self.badge {
                Some((b, what)) => {
                    job.append(&format!(" {} ", fmt_bytes(*b)), 7.0, egui::TextFormat { font_id: theme::semibold(11.5), color: t.accent_ink, background: t.accent, valign: Align::Center, ..Default::default() });
                    format!("Ways to save space. The biggest single one: {what}, {}. Click for all of them.", fmt_bytes(*b))
                }
                None => "Ways to save space: what could be compressed, and other ways to get space back".to_owned(),
            };
            if ui.add_enabled(self.scanned.is_some(), egui::Button::new(job)).on_hover_text(tip).clicked() {
                self.open_savings();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Whatever room is left (the window can be narrow).
                let w = (ui.available_width() - 4.0).clamp(90.0, 220.0);
                let r = ui.add(TextEdit::singleline(&mut self.filter).desired_width(w).hint_text("Filter by name  (Ctrl+F)"));
                if ui.input(|i| i.modifiers.ctrl && i.key_pressed(Key::F)) {
                    r.request_focus();
                }
                if r.changed() {
                    self.rows = None;
                }
            });
        });
    }

    fn go_to(&mut self, key: Key_) {
        match &key {
            Key_::Node(n) => self.reveal_node(*n, None),
            Key_::Cat(k) => {
                let cat = k.strip_prefix("cat:").and_then(|c| c.parse().ok()).unwrap_or(0);
                self.reveal_cat(&k.clone(), cat);
            }
        }
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let t = self.tokens();
        ui.horizontal_centered(|ui| {
            let msg = match &self.status {
                Some((m, at)) if at.elapsed() < Duration::from_secs(8) => m.clone(),
                _ if !self.marked.is_empty() => format!("{} marked (Ctrl+click) · press Delete to move them to the Recycle Bin", self.marked.len()),
                _ => "Ready".into(),
            };
            // Right-hand items first; the message gets the space that's
            // left and is shortened (full text on hover) instead of overlapping.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let muted = |s: String| RichText::new(s).color(t.muted).size(11.5);
                ui.label(RichText::new("●  Read-only · no network").color(t.accent).size(11.5))
                    .on_hover_text("Scanning only reads. DiskRoot has no network code: it never connects anywhere.");
                if self.watcher.is_some() {
                    ui.separator();
                    if self.live_overflow {
                        ui.label(RichText::new("Many changes · press F5").color(theme::WARN).size(11.5))
                            .on_hover_text("Windows reported more changes than it could list. Scan again for exact sizes.");
                    } else if !self.live_new.is_empty() {
                        let bytes: u64 = self.live_new.values().sum();
                        ui.label(RichText::new(format!("{} new since the scan ({}) · F5 to include", fmt_count(self.live_new.len() as u64), fmt_bytes(bytes))).color(theme::WARN).size(11.5))
                            .on_hover_text("New files and folders appeared. Sizes of existing items update live; new ones join the tree at the next scan.");
                    } else {
                        ui.label(RichText::new("Live").color(t.accent).size(11.5))
                            .on_hover_text("Watching for changes: sizes update as files change or are deleted (no admin needed).");
                    }
                }
                if let Some(s) = &self.summary {
                    ui.separator();
                    ui.label(muted(format!("{}% classified", (s.categorized * 100.0).round())));
                    ui.separator();
                    let mode = if s.mode.starts_with("MFT") { "MFT" } else { "folder walk" };
                    ui.label(muted(format!("{mode} · {:.2} s", s.elapsed_ms / 1000.0)));
                    ui.separator();
                    ui.label(muted(format!("{} {}", fmt_count(s.dirs), if s.dirs == 1 { "folder" } else { "folders" })));
                    ui.separator();
                    ui.label(muted(fmt_files(s.files)));
                    if s.unreadable > 0 {
                        ui.separator();
                        if ui
                            .add(egui::Label::new(RichText::new(format!("{} unreadable", fmt_count(s.unreadable))).color(theme::cat_color(6)).size(11.5)).sense(Sense::click()))
                            .on_hover_text("Some folders couldn't be read. Click to see which. Fast scan (admin) sees everything.")
                            .clicked()
                        {
                            self.open_unreadable();
                        }
                    }
                }
                ui.separator();
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.add(egui::Label::new(msg).truncate());
                });
            });
        });
    }

    /// "C:\  1.54 TB used of 1.86 TB (83%) · 320 GB free" and a bar split
    /// into the 5 biggest items, everything else, and free space.
    fn drive_strip(&mut self, ui: &mut Ui) {
        let t = self.tokens();
        let Some(b) = &self.drive_bar else { return };
        let is_drive = b.root.len() == 3 && b.root.ends_with(":\\");
        let (capacity, text) = match b.space {
            Some((total, free)) if is_drive => {
                let used = total.saturating_sub(free);
                (total, format!("{}  {} used of {} ({:.0}%) · {} free", b.root, fmt_bytes(used), fmt_bytes(total), used as f64 * 100.0 / total as f64, fmt_bytes(free)))
            }
            Some((total, _)) => (b.scanned.max(1), format!("{}  {} · {:.1}% of the drive ({})", b.root, fmt_bytes(b.scanned), b.scanned as f64 * 100.0 / total as f64, fmt_bytes(total))),
            None => (b.scanned.max(1), format!("{}  {}", b.root, fmt_bytes(b.scanned))),
        };
        let mut click: Option<u32> = None;
        let mut open_space = false;
        ui.horizontal_centered(|ui| {
            let full = b.nearly_full.as_ref().filter(|_| is_drive);
            let l = ui.add(egui::Label::new(RichText::new(text).font(theme::semibold(12.5)).color(if full.is_some() { theme::WARN } else { t.text })).sense(Sense::click()));
            if is_drive && l.clicked() {
                open_space = true;
            }
            let l = if is_drive { l.on_hover_text("Click: where every GB is") } else { l };
            if let Some(w) = full {
                l.on_hover_text(w.as_str());
                ui.label(RichText::new("⚠ nearly full").color(theme::WARN).size(12.0)).on_hover_text(w.as_str());
            }
            ui.add_space(10.0);
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 16.0), Sense::click());
            let p = ui.painter_at(r);
            p.rect_filled(r, CornerRadius::same(4), t.head);
            let mut x = r.min.x;
            let px = |bytes: u64| (bytes as f64 / capacity as f64 * r.width() as f64) as f32;
            let mut segs: Vec<(Rect, String, u64, Option<u32>, Color32)> = Vec::new();
            for (k, (id, name, bytes, cat)) in b.top.iter().enumerate() {
                let w = px(*bytes);
                if w < 1.0 {
                    continue;
                }
                // Same-category neighbours get alternating shades so each stands apart.
                let c = theme::cat_color(*cat);
                let c = if k % 2 == 1 { c.gamma_multiply(0.78) } else { c };
                segs.push((Rect::from_min_size(pos2(x, r.min.y), vec2(w, r.height())), name.clone(), *bytes, Some(*id), c));
                x += w;
            }
            let rest = b.scanned.saturating_sub(b.top.iter().map(|x| x.2).sum());
            if rest > 0 {
                let w = px(rest);
                segs.push((Rect::from_min_size(pos2(x, r.min.y), vec2(w, r.height())), "Everything else".into(), rest, None, t.line_strong));
                x += w;
            }
            if let (Some((total, free)), true) = (b.space, is_drive) {
                let hidden = total.saturating_sub(free).saturating_sub(b.scanned);
                if hidden > (total / 200) {
                    let w = px(hidden);
                    segs.push((Rect::from_min_size(pos2(x, r.min.y), vec2(w, r.height())), "Not visible to this scan (system, other users; Fast scan sees it)".into(), hidden, None, t.faint));
                }
            }
            for (rect, name, bytes, _, color) in &segs {
                p.rect_filled(rect.shrink2(vec2(0.5, 0.0)), CornerRadius::same(3), *color);
                let pct = *bytes as f64 * 100.0 / capacity as f64;
                let label = format!("{name} {pct:.0}%");
                let g = p.layout_no_wrap(label, theme::regular(11.0), Color32::WHITE);
                if g.size().x + 8.0 < rect.width() {
                    p.galley(rect.center() - g.size() / 2.0, g, Color32::WHITE);
                }
            }
            if let Some(pos) = resp.hover_pos()
                && let Some((_, name, bytes, id, _)) = segs.iter().find(|s| s.0.contains(pos))
            {
                let pct = *bytes as f64 * 100.0 / capacity as f64;
                let more = if id.is_some() { "\nClick to open it" } else { "" };
                resp.clone().on_hover_text_at_pointer(format!("{}\n{} · {pct:.1}%{more}", visible(name), fmt_bytes(*bytes)));
                if resp.clicked() {
                    click = *id;
                }
            } else if resp.hovered() {
                resp.on_hover_text_at_pointer("Free space");
            }
        });
        if let Some(id) = click {
            if self.settings.by_category {
                self.settings.by_category = false;
                self.regroup();
            }
            self.go_to(Key_::Node(id));
        }
        if open_space {
            self.open_space();
        }
    }

    /// Before the first scan: one card per drive (used / free bar), click
    /// to scan it.
    fn start_screen(&mut self, ui: &mut Ui, t: &Tokens) {
        ui.add_space(28.0);
        let mut pick: Option<String> = None;
        let mut reopen_freed: Option<String> = None;
        ui.vertical_centered(|ui| {
            ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(72.0, 72.0)));
            ui.add_space(8.0);
            ui.label(RichText::new("See what's filling your drive").font(theme::semibold(16.0)));
            ui.add_space(4.0);
            ui.label(RichText::new("Pick a drive to scan. Fast scan (admin) reads a whole NTFS drive in seconds.").color(t.muted));
            ui.add_space(14.0);
            if let Some(root) = self.freed.clone() {
                let b = egui::Button::new(RichText::new(format!("Open your saved scan of {}", visible(&root))).color(t.accent_ink)).fill(t.accent).min_size(vec2(260.0, 30.0));
                if ui.add(b).on_hover_text("Freed from memory earlier; reopens in about a second").clicked() {
                    reopen_freed = Some(root);
                }
                ui.add_space(12.0);
            }
            for d in &self.drives {
                let space = diskroot_session::disk_space(d);
                let (rect, resp) = ui.allocate_exact_size(vec2(420.0, 54.0), Sense::click());
                let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("Scan {d}")));
                let p = ui.painter();
                p.rect_filled(rect, CornerRadius::same(6), if resp.hovered() { t.hover } else { t.field });
                p.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, t.line), StrokeKind::Inside);
                p.text(pos2(rect.min.x + 14.0, rect.min.y + 16.0), Align2::LEFT_CENTER, d, theme::semibold(14.0), t.text);
                if let Some((total, free)) = space {
                    let used = total.saturating_sub(free);
                    let txt = format!("{} free of {}", fmt_bytes(free), fmt_bytes(total));
                    p.text(pos2(rect.max.x - 14.0, rect.min.y + 16.0), Align2::RIGHT_CENTER, txt, theme::regular(12.0), t.muted);
                    let bar = Rect::from_min_size(pos2(rect.min.x + 14.0, rect.min.y + 32.0), vec2(rect.width() - 28.0, 9.0));
                    p.rect_filled(bar, CornerRadius::same(4), t.head);
                    let frac = used as f32 / total as f32;
                    let col = if frac > 0.9 { theme::cat_color(5) } else if frac > 0.75 { theme::WARN } else { t.accent };
                    p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac, bar.height())), CornerRadius::same(4), col);
                }
                if resp.clicked() {
                    pick = Some(d.clone());
                }
                ui.add_space(8.0);
            }
        });
        if let Some(d) = pick {
            self.path = d;
            self.freed = None;
            self.start_scan(false);
        }
        if let Some(root) = reopen_freed {
            self.path = root;
            self.freed = None;
            self.start_reopen();
        }
    }

    fn pane_frame(t: &Tokens) -> Frame {
        Frame::new()
            .fill(t.panel)
            .stroke(Stroke::new(1.0, t.line_strong))
            .corner_radius(CornerRadius::same(t.radius))
            .inner_margin(Margin::ZERO)
    }

    fn tree_pane(&mut self, ui: &mut Ui) {
        let t = self.tokens();
        if self.rows.is_none() {
            self.rows = Some(self.build_rows());
        }
        Self::pane_frame(&t).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            let full = ui.available_width();
            let cols = Columns::new(full);
            // Header
            let (hr, _) = ui.allocate_exact_size(vec2(full, 26.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(hr, CornerRadius { nw: t.radius, ne: t.radius, sw: 0, se: 0 }, t.head);
            p.line_segment([hr.left_bottom(), hr.right_bottom()], Stroke::new(1.0, t.line));
            let f = theme::regular(11.5);
            p.text(pos2(hr.min.x + 10.0, hr.center().y), Align2::LEFT_CENTER, "Name", f.clone(), t.muted);
            p.text(pos2(hr.min.x + cols.size_r, hr.center().y), Align2::RIGHT_CENTER, "Size on disk", f.clone(), t.muted);
            p.text(pos2(hr.min.x + cols.bar_x, hr.center().y), Align2::LEFT_CENTER, "% of parent", f.clone(), t.muted);
            if cols.wide {
                p.text(pos2(hr.min.x + cols.files_r, hr.center().y), Align2::RIGHT_CENTER, "Files", f.clone(), t.muted);
                let head = if self.prev_sizes.is_some() { "Change · Category" } else { "Category" };
                p.text(pos2(hr.min.x + cols.cat_x, hr.center().y), Align2::LEFT_CENTER, head, f, t.muted);
            }

            if self.scanned.is_none() {
                self.start_screen(ui, &t);
                return;
            }

            let rows = self.rows.take().unwrap_or_default();
            let today = self.settings.color_by_age.then(diskroot_session::today_days);
            let row_color = |r: &Row| match (today, &r.key) {
                (Some(d), Key_::Node(_)) => theme::age_color(diskroot_session::age_bucket(d, r.mtime)),
                (Some(_), _) => t.faint,
                (None, _) if r.cat == u8::MAX => t.faint,
                (None, _) => theme::cat_color(r.cat),
            };
            let sel_idx = self.selected.as_ref().and_then(|k| rows.iter().position(|r| &r.key == k));
            let mut clicked: Option<(usize, bool)> = None; // (row, on twisty)
            let mut toggled: Option<usize> = None;
            let mut ctx_action: Option<(usize, &'static str)> = None;
            let mut hovered: Option<Key_> = None;
            let scroll_to = if self.scroll_to_selected { sel_idx } else { None };
            ui.spacing_mut().item_spacing.y = 0.0;
            // Bring the selected row to the middle by setting this list's own
            // offset (a "scroll to me" request can be picked up by another
            // scrolling area, e.g. the Inspector's).
            let mut area = ScrollArea::vertical().auto_shrink([false, false]);
            if let Some(i) = scroll_to {
                let view_h = ui.available_height();
                area = area.vertical_scroll_offset((i as f32 * ROW_H + ROW_H / 2.0 - view_h / 2.0).max(0.0));
            }
            area.show_rows(ui, ROW_H, rows.len(), |ui, range| {
                for i in range {
                    let r = &rows[i];
                    let (rect, resp) = ui.allocate_exact_size(vec2(full, ROW_H), Sense::click());
                    let p = ui.painter_at(rect);
                    let selected = sel_idx == Some(i);
                    // Accessible name + state for screen readers (and UI Automation).
                    let label = r.name.clone();
                    resp.widget_info(|| egui::WidgetInfo::selected(egui::Role::TreeItem, true, selected, &label));
                    let marked = matches!(r.key, Key_::Node(n) if self.marked.contains(&n));
                    if marked {
                        p.rect_filled(rect, CornerRadius::ZERO, t.sel);
                        p.rect_filled(Rect::from_min_size(rect.min, vec2(4.0, ROW_H)), CornerRadius::ZERO, theme::WARN);
                    } else if selected {
                        p.rect_filled(rect, CornerRadius::ZERO, t.sel);
                        p.rect_filled(Rect::from_min_size(rect.min, vec2(2.0, ROW_H)), CornerRadius::ZERO, t.accent);
                    } else if resp.hovered() || self.hover_map.is_some_and(|h| self.map_index(&r.key) == Some(h)) {
                        p.rect_filled(rect, CornerRadius::ZERO, t.hover);
                    }
                    if resp.hovered() {
                        hovered = Some(r.key.clone());
                    }
                    let y = rect.center().y;
                    let x0 = rect.min.x + 8.0 + f32::from(r.depth) * 16.0;
                    // Twisty
                    let tw = Rect::from_center_size(pos2(x0 + 5.0, y), vec2(14.0, ROW_H));
                    if r.expandable {
                        let open = self.expanded.contains(&r.key);
                        let c = t.muted;
                        let pts = if open {
                            vec![pos2(x0 + 1.5, y - 2.5), pos2(x0 + 8.5, y - 2.5), pos2(x0 + 5.0, y + 2.5)]
                        } else {
                            vec![pos2(x0 + 3.0, y - 4.0), pos2(x0 + 8.0, y), pos2(x0 + 3.0, y + 4.0)]
                        };
                        p.add(egui::Shape::convex_polygon(pts, c, Stroke::NONE));
                    }
                    let mut x = x0 + 14.0;
                    if r.more == 0 {
                        // Category (or age) swatch
                        let col = row_color(r);
                        p.rect_filled(Rect::from_center_size(pos2(x + 4.5, y), vec2(9.0, 9.0)), CornerRadius::same(2), col);
                        x += 15.0;
                        // Folder / file glyph
                        let g = Rect::from_center_size(pos2(x + 6.0, y), vec2(13.0, 11.0));
                        if r.dir {
                            p.rect_stroke(g, CornerRadius::same(2), Stroke::new(1.0, t.muted), StrokeKind::Inside);
                            p.line_segment([pos2(g.min.x, g.min.y + 3.0), pos2(g.max.x, g.min.y + 3.0)], Stroke::new(1.0, t.muted));
                        } else {
                            let f = Rect::from_center_size(pos2(x + 6.0, y), vec2(9.0, 12.0));
                            p.rect_stroke(f, CornerRadius::same(1), Stroke::new(1.0, t.muted), StrokeKind::Inside);
                        }
                        x += 18.0;
                    }
                    let name_w = (rect.min.x + cols.size_r - 70.0 - x).max(30.0);
                    let name_col = if r.more > 0 { t.muted } else { t.text };
                    let shown = map::fit(&p, &r.name, 13.0, name_w).unwrap_or_default();
                    let nr = p.text(pos2(x, y), Align2::LEFT_CENTER, shown, theme::regular(13.0), name_col);
                    if !r.tag.is_empty() {
                        p.text(pos2(nr.max.x + 6.0, y), Align2::LEFT_CENTER, r.tag, theme::regular(10.5), t.faint);
                    }
                    if r.more == 0 {
                        p.text(pos2(rect.min.x + cols.size_r, y), Align2::RIGHT_CENTER, fmt_bytes(r.bytes), theme::num(12.5), t.text);
                        // % bar
                        let track = Rect::from_min_size(pos2(rect.min.x + cols.bar_x, y - 4.0), vec2(cols.bar_w, 8.0));
                        p.rect_filled(track, CornerRadius::same(2), t.head);
                        p.rect_stroke(track, CornerRadius::same(2), Stroke::new(1.0, t.line), StrokeKind::Inside);
                        let frac = if r.parent_bytes > 0 { r.bytes as f32 / r.parent_bytes as f32 } else { 0.0 };
                        let col = row_color(r);
                        p.rect_filled(
                            Rect::from_min_size(track.min, vec2(track.width() * frac.clamp(0.0, 1.0), 8.0)),
                            CornerRadius::same(2),
                            col,
                        );
                        p.text(
                            pos2(track.max.x + 44.0, y),
                            Align2::RIGHT_CENTER,
                            fmt_pct(r.bytes, r.parent_bytes),
                            theme::regular(11.5),
                            t.muted,
                        );
                        if cols.wide {
                            p.text(pos2(rect.min.x + cols.files_r, y), Align2::RIGHT_CENTER, fmt_count(r.files), theme::regular(12.0), t.muted);
                            // Change since the previous scan (folders, ≥ 1 MB).
                            let mut cat_x = rect.min.x + cols.cat_x;
                            if let (Some(prev), Some(s), Key_::Node(n)) = (&self.prev_sizes, &self.scanned, &r.key)
                                && r.dir
                                && let (Ok(full), Some(sum)) = (s.path(*n), &self.summary)
                            {
                                let rel = full.strip_prefix(sum.root.as_str()).unwrap_or(&full).trim_start_matches('\\');
                                let before = prev.get(&remember::path_key(rel)).copied().unwrap_or(0);
                                let d = i128::from(r.bytes) - i128::from(before);
                                if d.unsigned_abs() >= 1 << 20 {
                                    let col = if d > 0 { theme::WARN } else { t.accent };
                                    p.text(pos2(cat_x, y), Align2::LEFT_CENTER, fmt_delta(d), theme::semibold(12.0), col);
                                    cat_x += 74.0;
                                }
                            }
                            if let Some(c) = map::fit(&p, &r.cat_label, 12.0, rect.max.x - cat_x - 8.0) {
                                p.text(pos2(cat_x, y), Align2::LEFT_CENTER, c, theme::regular(12.0), t.muted);
                            }
                        }
                    }
                    if resp.clicked() {
                        let on_twisty = resp.interact_pointer_pos().is_some_and(|q| tw.contains(q));
                        clicked = Some((i, on_twisty));
                    }
                    if resp.double_clicked() && r.expandable {
                        toggled = Some(i);
                    }
                    if let Key_::Cat(k) = &r.key
                        && k.starts_with("sub:")
                    {
                        resp.context_menu(|ui| {
                            if ui.button("Mark all that are safe to clear").clicked() {
                                ctx_action = Some((i, "mark-all"));
                                ui.close();
                            }
                        });
                    }
                    if r.more == 0
                        && let Key_::Node(_) = r.key {
                            resp.context_menu(|ui| {
                                if ui.button("Show in Explorer").clicked() {
                                    ctx_action = Some((i, "reveal"));
                                    ui.close();
                                }
                                if ui.button("Move to Recycle Bin…").clicked() {
                                    ctx_action = Some((i, "delete"));
                                    ui.close();
                                }
                                if r.dir && ui.add_enabled(self.job.is_none(), egui::Button::new("Compress…")).clicked() {
                                    ctx_action = Some((i, "compress"));
                                    ui.close();
                                }
                                if r.dir {
                                    ui.menu_button("Alert me if it grows by", |ui| {
                                        for (label, act) in [("1 GB", "alert-1"), ("5 GB", "alert-5"), ("10 GB", "alert-10"), ("50 GB", "alert-50")] {
                                            if ui.button(label).clicked() {
                                                ctx_action = Some((i, act));
                                                ui.close();
                                            }
                                        }
                                    });
                                    ui.menu_button("This is…", |ui| {
                                        ui.label(RichText::new("Teach DiskRoot what this folder is (a category in your own rules; never a safety label)").color(t.faint).size(11.5));
                                        const ACTS: [&str; 5] = ["teach-0", "teach-1", "teach-2", "teach-3", "teach-4"];
                                        for (k, (label, _, _)) in diskroot_session::TEACH.iter().enumerate() {
                                            if ui.button(*label).clicked() {
                                                ctx_action = Some((i, ACTS[k]));
                                                ui.close();
                                            }
                                        }
                                        ui.separator();
                                        if ui.button("Forget what I taught here").clicked() {
                                            ctx_action = Some((i, "untaught"));
                                            ui.close();
                                        }
                                    });
                                }
                            });
                        }
                }
            });
            self.scroll_to_selected = false;
            self.hover_tree = hovered;

            // Apply interactions after drawing.
            if let Some((i, twisty)) = clicked {
                self.tree_focus = true;
                let ctrl = ui.input(|inp| inp.modifiers.ctrl);
                if twisty && rows[i].expandable {
                    toggled = Some(i);
                } else if ctrl && let Key_::Node(n) = rows[i].key {
                    // Ctrl+click: mark / unmark for deleting several at once.
                    if self.marked.is_empty()
                        && let Some(Key_::Node(cur)) = self.selected
                        && cur != 0
                    {
                        self.marked.insert(cur);
                    }
                    if n != 0 && !self.marked.remove(&n) {
                        self.marked.insert(n);
                    }
                    self.select(rows[i].key.clone());
                } else if rows[i].more == 0 {
                    self.marked.clear();
                    self.select(rows[i].key.clone());
                }
            }
            let mut rebuild = false;
            if let Some(i) = toggled {
                let k = rows[i].key.clone();
                if !self.expanded.remove(&k) {
                    self.expanded.insert(k);
                }
                rebuild = true;
            }
            if let Some((i, "mark-all")) = ctx_action
                && let Key_::Cat(k) = &rows[i].key
            {
                self.mark_all(&k.clone());
            } else if let Some((i, act)) = ctx_action
                && let Key_::Node(n) = rows[i].key {
                    self.select(Key_::Node(n));
                    match act {
                        "reveal" => self.reveal_in_explorer(n),
                        "compress" => self.ask_compress(n),
                        a if a.starts_with("alert-") => {
                            let gb: u64 = a[6..].parse().unwrap_or(1);
                            self.add_alert(n, gb << 30);
                        }
                        a if a.starts_with("teach-") || a == "untaught" => {
                            let Some(path) = self.scanned.as_ref().and_then(|s| s.info(n).ok()).map(|i| i.path) else { return };
                            let r = match a.strip_prefix("teach-").and_then(|k| k.parse::<usize>().ok()) {
                                Some(k) => diskroot_session::teach(&path, k).map(|()| format!("Taught: {} is \"{}\". It's a rule in your own rules file (File > Custom rules).", visible(&path), diskroot_session::TEACH[k].2)),
                                None => diskroot_session::forget_taught(&path).map(|gone| if gone { format!("Forgot what you taught about {}.", visible(&path)) } else { "Nothing taught here.".into() }),
                            };
                            match r {
                                Ok(msg) => {
                                    self.reload_rules();
                                    let wanted = a.strip_prefix("teach-").and_then(|k| k.parse::<usize>().ok()).map(|k| diskroot_session::TEACH[k].2);
                                    let now = self.scanned.as_ref().and_then(|s| s.info(n).ok().and_then(|i| s.rules().subs.get(i.sub as usize).map(|x| x.name.clone())));
                                    match (wanted, now) {
                                        (Some(w), Some(now)) if now != w => self.say(format!("Saved, but a built-in rule already classifies this folder as \"{now}\", and built-in rules come first. Your rule applies elsewhere it matches.")),
                                        _ => self.say(msg),
                                    }
                                }
                                Err(e) => self.dialog = Some(Dialog::Message("Couldn't save your rule".into(), e)),
                            }
                        }
                        _ => self.ask_delete(n),
                    }
                }
            self.rows = if rebuild { None } else { Some(rows) };
        });
    }

    /// "Mark all" on a sub-type: every place a "safe to clear" rule matched
    /// directly (never folders found by file extension, never protected).
    fn mark_all(&mut self, key: &str) {
        let Some(s) = &self.scanned else { return };
        let ids = s.markable(key);
        if ids.is_empty() {
            self.say("Nothing here is labelled \"safe to clear\" by DiskRoot's rules, so nothing was marked.");
            return;
        }
        let bytes: u64 = ids.iter().filter_map(|&n| s.info(n).ok()).map(|i| i.on_disk).sum();
        let n = ids.len();
        self.marked.extend(ids);
        self.rows = None;
        self.say(format!("{n} places marked ({}) · press Delete to review and move them to the Recycle Bin", fmt_bytes(bytes)));
    }

    /// Starts a Windows tool for a place. DISM is explained first; the
    /// settings pages just open.
    fn start_tool(&mut self, a: Action, confirmed: bool) {
        let r = match a {
            Action::SystemProtection => diskroot_session::tools::open_system_protection(),
            Action::StorageSettings => diskroot_session::tools::open_storage_settings(),
            Action::DismCleanup if !confirmed => {
                self.dialog = Some(Dialog::Tool(a));
                return;
            }
            Action::DismCleanup => {
                if self.job.is_some() {
                    return;
                }
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let _ = tx.send(Done::Tool(diskroot_session::tools::dism_component_cleanup()));
                });
                self.job = Some(Job { kind: JobKind::Tool("Windows component cleanup (DISM)".into()), started: Instant::now(), rx, cancel: Arc::default() });
                return;
            }
        };
        if let Err(e) = r {
            self.dialog = Some(Dialog::Message("Couldn't open it".into(), e));
        }
    }

    /// Opens the Compress dialog for a folder, or says why it can't be done.
    fn ask_compress(&mut self, n: u32) {
        self.ask_compress_at(n, Level::Fast);
    }

    /// The Compress dialog with `level` preselected.
    fn ask_compress_at(&mut self, n: u32, level: Level) {
        let Some(s) = &self.scanned else { return };
        let path = s.info(n).map(|i| i.path).unwrap_or_default();
        self.dialog = Some(match (s.compress_plan(n, false), s.compress_plan(n, true)) {
            (Ok(plan), Ok(undo)) => Dialog::Compress(Box::new(CompressAsk { path, plan, undo, level })),
            (Err(e), _) | (_, Err(e)) => Dialog::Message("DiskRoot won't compress this".into(), e),
        });
    }

    /// View > Ways to save space (from the scan alone, well under a second on a full drive).
    /// Save space: worked out on a worker thread (about 0.2 s on a whole
    /// 1.7M-file drive), shown when ready.
    fn open_savings(&mut self) {
        if self.savings_rx.is_some() {
            return;
        }
        if let Some(s) = &self.scanned {
            let s = s.clone(); // cheap: shared parts
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let min_days = 180;
                let sugg = s.compress_suggestions(min_days, SAVINGS_MIN, 200);
                let tips = s.space_tips();
                let _ = tx.send(SavingsView { tab: 0, min_days, sugg, tips, pending: None });
            });
            self.savings_rx = Some(rx);
            self.say_quiet("Finding ways to save space…");
        }
    }

    fn open_home(&mut self) {
        if self.home_rx.is_some() {
            return;
        }
        if let Some(s) = &self.scanned {
            let s = s.clone();
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(HomeView::build(&s));
            });
            self.home_rx = Some(rx);
        }
    }

    fn poll_home(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.home_rx else { return };
        match rx.try_recv() {
            Ok(v) => {
                self.home_rx = None;
                if self.dialog.is_none() {
                    self.dialog = Some(Dialog::Home(Box::new(v)));
                }
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(50)),
            Err(TryRecvError::Disconnected) => self.home_rx = None,
        }
    }

    fn poll_savings(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.savings_rx {
            match rx.try_recv() {
                Ok(v) => {
                    self.savings_rx = None;
                    if self.dialog.is_none() {
                        self.dialog = Some(Dialog::Savings(Box::new(v)));
                    }
                    self.say_quiet("Ready.");
                }
                Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(50)),
                Err(TryRecvError::Disconnected) => self.savings_rx = None,
            }
        }
        if let Some(Dialog::Savings(v)) = &mut self.dialog
            && let Some(rx) = &v.pending
        {
            match rx.try_recv() {
                Ok(sugg) => {
                    v.sugg = sugg;
                    v.pending = None;
                }
                Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(50)),
                Err(TryRecvError::Disconnected) => v.pending = None,
            }
        }
    }

    fn open_health(&mut self) {
        let mut v = HealthView::read();
        v.at_risk = self.scanned.as_ref().and_then(|s| {
            let root = s.summary().root;
            (root.len() == 3 && root.ends_with(":\\")).then(|| (root.trim_end_matches('\\').to_owned(), s.at_risk()))
        });
        self.dialog = Some(Dialog::Health(Box::new(v)));
    }

    fn open_unreadable(&mut self) {
        if let Some(sc) = &self.scanned {
            let root = sc.summary().root;
            self.dialog = Some(Dialog::Unreadable(Box::new((root, sc.unreadable_folders(2000)))));
        }
    }

    /// View > Where every GB is.
    fn open_space(&mut self) {
        match self.scanned.as_ref().and_then(|s| s.space_report()) {
            Some(r) => self.dialog = Some(Dialog::Space(Box::new(r))),
            None => self.dialog = Some(Dialog::Message("Where every GB is".into(), "This works for a scan of a whole drive (like C:\\), so the drive's used space can be compared with what the scan found.".into())),
        }
    }

    /// View > Size history.
    fn open_growth(&mut self) {
        let Some(sum) = &self.summary else { return };
        let root = sum.root.clone();
        let h = self.hist.clone().or_else(|| history::load(&root).map(Arc::new));
        let Some(h) = h.filter(|h| !h.points.is_empty()) else {
            self.dialog = Some(Dialog::Message(
                "No size history yet".into(),
                format!(
                    "DiskRoot keeps a small size history of every scan it remembers (folders of 1 MB or more, on this PC only). Scan {} again later, or turn on View → Scheduled scans, and the history starts filling.{}",
                    visible(&root),
                    if self.settings.remember_scans { "" } else { "\n\nRemembering scans is off (File menu), so no history is kept." }
                ),
            ));
            return;
        };
        let drive = root.len() <= 3 && h.points.iter().any(|p| p.used().is_some());
        let chart = if drive { h.points.iter().filter_map(|p| Some((p.time, p.used()?))).collect() } else { h.series("") };
        let growers = [86_400, 7 * 86_400, 30 * 86_400].map(|secs| h.growers(secs, 30));
        self.dialog = Some(Dialog::Growth(Box::new(GrowthView {
            drive,
            chart,
            forecast: drive.then(|| history::forecast(&h.points)),
            tab: 1,
            growers,
            points: h.points.len(),
            first: h.points[0].time,
            disk: history::disk_usage(),
            logs: diskroot_session::logs::growing(&diskroot_session::logs::load(&root)),
            root,
        })));
    }

    /// View > Scheduled scans: the saved schedule (or a default for the
    /// current drive), whether the task exists, and a recommendation.
    fn open_schedule(&mut self) {
        let installed = schedule::installed();
        let mut s = schedule::load().unwrap_or_default();
        if schedule::load().is_none()
            && let Some(sum) = &self.summary
        {
            s.roots = vec![sum.root.clone()];
        }
        let runs = schedule::runs();
        let rec = Self::recommendation(self.summary.as_ref(), &runs);
        if !installed && schedule::load().is_none() {
            s.every_min = rec.0;
        }
        self.dialog = Some(Dialog::Schedule(Box::new(ScheduleView { s, on: installed, installed, rec, runs, msg: None })));
    }

    /// Scan time from the latest normal scan (or background runs) and the
    /// average daily change from the background runs.
    fn recommendation(sum: Option<&Summary>, runs: &[schedule::Run]) -> (u32, String) {
        let ok: Vec<&schedule::Run> = runs.iter().filter(|r| r.error.is_none()).collect();
        let secs = sum.filter(|s| !s.mode.starts_with("MFT")).map(|s| s.elapsed_ms / 1000.0).or_else(|| (!ok.is_empty()).then(|| ok.iter().map(|r| r.secs).sum::<f64>() / ok.len() as f64));
        let per_day = match (ok.first(), ok.last()) {
            (Some(a), Some(b)) if b.time > a.time + 86_400 => {
                let moved: f64 = ok.iter().filter_map(|r| r.delta).map(|d| d.unsigned_abs() as f64).sum();
                Some(moved / ((b.time - a.time) as f64 / 86_400.0))
            }
            _ => None,
        };
        // What one save of this root writes (measured from its cache files).
        let saved = sum.and_then(|s| remember::cache_dir().map(|d| diskroot_session::cachefile::bytes_written_per_save(&d, &remember::key(&s.root)))).filter(|&b| b > 0);
        schedule::recommend(secs, per_day, saved)
    }

    fn open_similar(&mut self) {
        if let Some(s) = &self.scanned {
            self.dialog = Some(Dialog::Similar(Box::new(s.similar_folders())));
        }
    }

    fn open_leftovers(&mut self) {
        if let Some(s) = &self.scanned {
            let mut list = s.leftovers(300);
            list.extend(s.orphan_hints(&diskroot_session::orphans::Programs::installed(), 50 << 20, 90));
            list.sort_by_key(|l| std::cmp::Reverse(l.on_disk));
            self.dialog = Some(Dialog::Leftovers(list));
        }
    }

    fn open_dupes(&mut self) {
        self.dialog = Some(Dialog::Dupes(Box::new(DupesView { min_mb: 10, running: None, groups: None })));
    }

    /// The Compact dialog for a WSL/Docker virtual disk.
    fn ask_vhdx(&mut self, n: u32) {
        if let Some(s) = &self.scanned
            && let Ok(i) = s.info(n)
        {
            let busy = diskroot_session::power::in_use(std::path::Path::new(&i.path));
            self.dialog = Some(Dialog::Vhdx(i.path, i.on_disk, busy));
        }
    }

    /// Runs the compression engine on a worker thread; the progress dialog
    /// reads its live counters.
    fn start_compress(&mut self, plan: compress::Plan, level: Option<Level>) {
        if self.job.is_some() {
            return;
        }
        let progress = Arc::new(compress::Progress::default());
        let (tx, rx) = mpsc::channel();
        let (total, files) = (plan.bytes, plan.files.len() as u64);
        let title = match level {
            Some(l) => format!("Compressing ({})", l.label()),
            None => "Uncompressing".to_owned(),
        };
        let pr = Arc::clone(&progress);
        std::thread::spawn(move || {
            let r = compress::run(&plan, level, compress::default_threads(), &pr);
            let _ = tx.send(Done::Compressed(Box::new(r)));
        });
        self.job = Some(Job { kind: JobKind::Compress(title, progress, total, files), started: Instant::now(), rx, cancel: Arc::default() });
    }

    /// After compressing: new sizes into the tree, and a plain summary.
    fn finish_compress(&mut self, r: compress::Report) {
        self.badge_dirty = true;
        if let Some(s) = &mut self.scanned
            && !r.new_alloc.is_empty()
        {
            // The remembered copy of this scan no longer matches the disk.
            remember::forget_index(&s.summary().root);
            s.apply_new_alloc(&r.new_alloc);
            self.summary = Some(s.summary());
        }
        // Free space changed: the drive strip and totals at the bottom too.
        self.refresh_drive_bar();
        self.rows = None;
        self.map = None;
        let n = |c: compress::Count| fmt_files(c.0);
        let mut lines = Vec::new();
        if r.undo {
            lines.push(format!("{} restored to normal storage: {} → {} on disk.", fmt_files(r.changed_files), fmt_bytes(r.before), fmt_bytes(r.after)));
        } else {
            lines.push(format!(
                "{} packed: {} → {} on disk, {} saved, in {:.0} s.",
                fmt_files(r.changed_files),
                fmt_bytes(r.before),
                fmt_bytes(r.after),
                fmt_bytes(r.saved()),
                r.secs
            ));
        }
        if r.wont_shrink.0 > 0 {
            lines.push(format!("{} left as it was: it wouldn't get smaller.", n(r.wont_shrink)));
        }
        if r.in_use.0 > 0 {
            lines.push(format!("{} skipped: in use by another program.", n(r.in_use)));
        }
        if r.denied.0 > 0 {
            lines.push(format!("{} skipped: Windows doesn't allow changing them (read-only or no permission).", n(r.denied)));
        }
        if r.skipped.0 > 0 {
            lines.push(format!("{} skipped: already in that state, changed since the scan, not where the scan saw them, or hardlinked (sharing data with another name).", n(r.skipped)));
        }
        if r.failed.0 > 0 {
            lines.push(format!("{} failed: {}", n(r.failed), r.first_error.clone().unwrap_or_default()));
        }
        if r.cancelled {
            lines.push("Stopped. Files already done stay that way (each file is all or nothing); the rest are untouched. Run it again to continue.".into());
        }
        lines.push("The tree already shows the new sizes.".into());
        let title = match (r.undo, r.cancelled) {
            (false, false) => "Compressed",
            (false, true) => "Compression stopped",
            (true, false) => "Uncompressed",
            (true, true) => "Uncompressing stopped",
        };
        self.dialog = Some(Dialog::Message(title.into(), lines.join("\n\n")));
    }

    /// Runs a blocking tool call on a worker thread behind the progress dialog.
    fn run_tool_job(&mut self, name: String, f: impl FnOnce() -> Result<String, String> + Send + 'static) {
        if self.job.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(Done::Tool(f()));
        });
        self.job = Some(Job { kind: JobKind::Tool(name), started: Instant::now(), rx, cancel: Arc::default() });
    }

    /// File > Export marked as PowerShell script: Documents\DiskRoot\cleanup <time>.ps1
    /// (UTF-8 with BOM, so Windows PowerShell 5.1 reads non-English names right).
    fn export_plan(&mut self) {
        let Some(s) = &self.scanned else { return };
        let nodes: Vec<u32> = self.marked.iter().copied().collect();
        let (items, refused) = s.plan_items(&nodes);
        if items.is_empty() {
            self.dialog = Some(Dialog::Message("Nothing to export".into(), "None of the marked items can be deleted by DiskRoot (protected or unusual names).".into()));
            return;
        }
        let stamp = diskroot_session::safety::utc_now();
        let script = diskroot_session::power::powershell_plan(&items, &s.summary().root, &format!("{stamp} UTC"));
        let docs = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).unwrap_or_default().join("Documents").join("DiskRoot");
        let path = docs.join(format!("cleanup {}.ps1", stamp.replace([':', ' '], "-")));
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(script.as_bytes());
        match std::fs::create_dir_all(&docs).and_then(|()| std::fs::write(&path, bytes)) {
            Ok(()) => {
                let mut msg = format!(
                    "{} items written to\n{}\n\nOpen it in a text editor and read it. Run it with -Run to recycle them; without -Run it only shows what it would do. DiskRoot doesn't run it.",
                    items.len(),
                    path.display()
                );
                if !refused.is_empty() {
                    msg += &format!("\n\nLeft out ({}): {}", refused.len(), refused.iter().map(|r| r.0.as_str()).collect::<Vec<_>>().join(", "));
                }
                let _ = diskroot_session::reveal(&path.display().to_string());
                self.dialog = Some(Dialog::Message("Cleanup script saved".into(), msg));
            }
            Err(e) => self.dialog = Some(Dialog::Message("Couldn't save the script".into(), e.to_string())),
        }
    }

    fn add_alert(&mut self, n: u32, grow: u64) {
        let Some(s) = &self.scanned else { return };
        let Ok(info) = s.info(n) else { return };
        self.settings.alerts.retain(|a| !a.path.eq_ignore_ascii_case(&info.path));
        self.settings.alerts.push(Alert { path: info.path.clone(), baseline: info.on_disk, grow });
        self.settings.save();
        self.say(format!("Alert set: DiskRoot tells you when {} grows by {} (while it's open, or at the next scan)", info.name, fmt_bytes(grow)));
    }

    fn reload_rules(&mut self) {
        self.rules_status = diskroot_session::reload_rules();
        if let Some(s) = &mut self.scanned {
            s.reclassify();
            self.summary = Some(s.summary());
        }
        self.marked.clear();
        self.invalidate();
        self.refresh_drive_bar();
        match &self.rules_status.error {
            Some(e) => self.say(format!("Your rules weren't loaded: {e}")),
            None => self.say(format!("Rules reloaded · {} of your own", self.rules_status.user_rules)),
        }
    }

    fn reveal_in_explorer(&mut self, n: u32) {
        if let Some(s) = &self.scanned
            && let Err(e) = s.path(n).and_then(|p| diskroot_session::reveal(&p)) {
                self.dialog = Some(Dialog::Message("Couldn't open Explorer".into(), e));
            }
    }

    fn ask_delete(&mut self, n: u32) {
        // Anything marked (Ctrl+click, Space, "Mark extra copies"): Delete
        // means "the marked items", whatever row is selected.
        if !self.marked.is_empty() && (self.marked.len() > 1 || self.marked.contains(&n) || n == 0) {
            self.ask_delete_many();
            return;
        }
        let Some(s) = &self.scanned else { return };
        // Protected items still open the dialog (it explains why). Others are
        // prepared and armed now: warnings computed, file identities taken.
        let prepared = s.prepare_delete(n).and_then(|mut p| p.arm().map(|()| p));
        match (s.info(n), prepared) {
            (Ok(_), _) if n == 0 => {
                self.dialog = Some(Dialog::Message("Can't delete this".into(), "DiskRoot won't delete the whole scanned folder or drive.".into()));
            }
            (Ok(info), _) if info.protected.is_some() => {
                self.dialog = Some(Dialog::ConfirmDelete(Box::new(Confirm { info, warnings: DeleteWarnings::default(), ack: false, pending: None })));
            }
            (Ok(info), Ok(p)) => {
                let warnings = p.warnings.clone();
                self.dialog = Some(Dialog::ConfirmDelete(Box::new(Confirm { info, warnings, ack: false, pending: Some(p) })));
            }
            (Ok(_), Err(e)) | (Err(e), _) => self.dialog = Some(Dialog::Message("Can't delete this".into(), e)),
        }
    }

    /// Prepares every marked item (skipping ones inside another marked
    /// folder), arms each, and asks once.
    fn ask_delete_many(&mut self) {
        let Some(s) = &self.scanned else { return };
        let marked: Vec<u32> = self.marked.iter().copied().collect();
        let inside_other = |n: u32| {
            s.crumbs(n).map(|c| c.iter().rev().skip(1).any(|a| self.marked.contains(&a.id))).unwrap_or(false)
        };
        let (mut items, mut refused) = (Vec::new(), Vec::new());
        let mut warnings = DeleteWarnings::default();
        for n in marked.into_iter().filter(|&n| !inside_other(n)) {
            let name = s.info(n).map(|i| i.name).unwrap_or_default();
            match s.prepare_delete(n).and_then(|mut p| p.arm().map(|()| p)) {
                Ok(p) => {
                    let w = &p.warnings;
                    warnings.cloud |= w.cloud;
                    warnings.git_repos += w.git_repos;
                    warnings.large |= w.large;
                    warnings.no_recycle_bin |= w.no_recycle_bin;
                    for a in &w.apps_running {
                        if !warnings.apps_running.contains(a) {
                            warnings.apps_running.push(a.clone());
                        }
                    }
                    items.push(p);
                }
                Err(why) => refused.push((name, why)),
            }
        }
        let bytes: u64 = items.iter().map(|p| p.freed).sum();
        let files: u64 = items.iter().map(|p| u64::from(p.files)).sum();
        warnings.large |= bytes >= diskroot_session::safety::LARGE_BYTES || files >= u64::from(diskroot_session::safety::LARGE_FILES);
        self.dialog = Some(Dialog::ConfirmMany(Box::new(ConfirmMany { items, refused, bytes, warnings, ack: false })));
    }

    fn start_delete_many(&mut self, items: Vec<PendingDelete>) {
        let (tx, rx) = mpsc::channel();
        let n = items.len();
        std::thread::spawn(move || {
            let results = items.into_iter().map(|p| (p.node, p.recycle())).collect();
            let _ = tx.send(Done::DeletedMany(results));
        });
        self.job = Some(Job { kind: JobKind::DeleteMany(n), started: Instant::now(), rx, cancel: Arc::default() });
    }

    fn finish_delete_many(&mut self, results: Vec<(u32, Result<DeleteOut, String>)>) {
        self.job = None;
        self.badge_dirty = true;
        self.marked.clear();
        let (mut done, mut freed, mut failed) = (0, 0u64, Vec::new());
        if let Some(s) = &mut self.scanned {
            remember::forget_index(&s.summary().root);
            for (node, r) in results {
                match r {
                    Ok(out) if !out.cancelled => {
                        s.apply_delete(node);
                        done += 1;
                        freed += out.freed;
                    }
                    Ok(_) => {}
                    Err(e) => failed.push(e),
                }
            }
            self.summary = Some(s.summary());
        }
        self.invalidate();
        self.refresh_drive_bar();
        self.selected = Some(self.root_key());
        if failed.is_empty() {
            self.say(format!("Moved {done} items to the Recycle Bin · {} freed", fmt_bytes(freed)));
        } else {
            self.dialog = Some(Dialog::Message(
                format!("Moved {done} items; {} couldn't be moved", failed.len()),
                failed.join("\n\n"),
            ));
        }
    }

    fn open_security(&mut self) {
        self.dialog = Some(Dialog::Security(Box::new(diskroot_session::integrity::check_current_exe())));
    }

    /// Writes the scan to Documents\DiskRoot\<root> <date>.csv on a worker thread.
    fn start_export(&mut self) {
        let Some(s) = &self.scanned else { return };
        let (idx, _, _) = s.remember_parts();
        let root = s.summary().root;
        let docs = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).unwrap_or_default().join("Documents").join("DiskRoot");
        let safe: String = root.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
        let stamp = diskroot_session::safety::utc_now().replace([':', ' '], "-");
        let path = docs.join(format!("{} {stamp}.csv", safe.trim_matches('_')));
        let (tx, rx) = mpsc::channel();
        let shown = path.display().to_string();
        std::thread::spawn(move || {
            let r = std::fs::create_dir_all(&docs)
                .and_then(|()| Scanned::export_csv(&idx, &path))
                .map(|()| path)
                .map_err(|e| e.to_string());
            let _ = tx.send(Done::Exported(r));
        });
        self.job = Some(Job { kind: JobKind::Export(shown), started: Instant::now(), rx, cancel: Arc::default() });
    }

    fn open_files(&mut self) {
        let Some(s) = &self.scanned else { return };
        let view = FilesView { query: String::new(), top: s.top_files(100, ""), types: s.file_types(40) };
        self.dialog = Some(Dialog::Files(Box::new(view)));
    }

    fn open_history(&mut self) {
        self.dialog = Some(Dialog::History(diskroot_session::safety::read_log(200)));
    }

    fn map_pane(&mut self, ui: &mut Ui) {
        let t = self.tokens();
        self.ensure_map();
        let mut crumb_go: Option<Key_> = None;
        let mut mode_go: Option<u8> = None;
        let mut tilt_go = false;
        Self::pane_frame(&t).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            let full = ui.available_width();
            // Title strip
            let (hr, _) = ui.allocate_exact_size(vec2(full, 26.0), Sense::hover());
            let p = &ui.painter().clone(); // a handle, so the strip can add widgets below
            p.rect_filled(hr, CornerRadius { nw: t.radius, ne: t.radius, sw: 0, se: 0 }, t.head);
            p.line_segment([hr.left_bottom(), hr.right_bottom()], Stroke::new(1.0, t.line));
            let Some(m) = &self.map else {
                p.text(pos2(hr.min.x + 10.0, hr.center().y), Align2::LEFT_CENTER, "Map", theme::semibold(12.0), t.text);
                return;
            };
            if m.nodes.is_empty() {
                return;
            }
            let root = &m.nodes[0];
            // Breadcrumbs: every level above the map root can be clicked.
            // When they don't fit, the first levels give way to "…".
            let crumbs: Vec<(Key_, String)> = if m.crumbs.is_empty() { vec![(Key_::Node(0), root.name.clone())] } else { m.crumbs.clone() };
            let sep = "  ›  ";
            let width = |text: &str, last: bool| p.layout_no_wrap(text.to_owned(), if last { theme::semibold(12.0) } else { theme::regular(12.0) }, t.text).size().x;
            let sep_w = width(sep, false);
            // Room left of the switches (2.5D, Size, Growth, Reclaimable ≈ 340 px);
            // the size text only shows when there's room besides.
            let budget = full - 350.0;
            let mut first = 0;
            while first + 1 < crumbs.len() {
                let total: f32 = crumbs[first..].iter().enumerate().map(|(i, c)| width(&visible(&c.1), first + i + 1 == crumbs.len()) + sep_w).sum::<f32>() + if first > 0 { width("…", false) + sep_w } else { 0.0 };
                if total <= budget {
                    break;
                }
                first += 1;
            }
            let mut x = hr.min.x + 10.0;
            if first > 0 {
                p.text(pos2(x, hr.center().y), Align2::LEFT_CENTER, "…", theme::regular(12.0), t.faint);
                x += width("…", false);
                p.text(pos2(x, hr.center().y), Align2::LEFT_CENTER, sep, theme::regular(12.0), t.faint);
                x += sep_w;
            }
            for (i, (key, name)) in crumbs.iter().enumerate().skip(first) {
                let last = i + 1 == crumbs.len();
                let shown = visible(name);
                // The last level gets what's left before the mode switch (~300 px).
                let text = if last { map::fit(p, &shown, 12.0, (hr.max.x - 350.0 - x).max(40.0)).unwrap_or_default() } else { shown.clone() };
                let w = width(&text, last);
                if last {
                    p.text(pos2(x, hr.center().y), Align2::LEFT_CENTER, text, theme::semibold(12.0), t.text);
                    x += w;
                } else {
                    let r = Rect::from_min_size(pos2(x - 2.0, hr.min.y + 3.0), vec2(w + 4.0, hr.height() - 6.0));
                    let resp = ui.interact(r, ui.scope_id().with(("crumb", i)), Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Link, true, format!("Go to {shown}")));
                    let hovered = resp.hovered();
                    if hovered {
                        p.rect_filled(r, CornerRadius::same(3), t.hover);
                    }
                    p.text(pos2(x, hr.center().y), Align2::LEFT_CENTER, text, theme::regular(12.0), if hovered { t.text } else { t.muted });
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        crumb_go = Some(key.clone());
                    }
                    x += w;
                    p.text(pos2(x, hr.center().y), Align2::LEFT_CENTER, sep, theme::regular(12.0), t.faint);
                    x += sep_w;
                }
            }
            // Right: map colours (Size / Growth / Reclaimable), then the size
            // when there's room.
            let crumbs_end = x;
            let mut mode = self.settings.map_mode;
            let mut strip = ui.new_child(egui::UiBuilder::new().max_rect(hr.shrink2(vec2(6.0, 2.0))).layout(Layout::right_to_left(Align::Center)));
            for (v, label, tip) in [
                (2u8, "Reclaimable", "Colour space you could get back: safe to clear (green), and caches or Windows leftovers to check first (amber)"),
                (1, "Growth", "Colour what grew or shrank since the previous scan"),
                (0, "Size", "Colour by category"),
            ] {
                if strip.selectable_label(mode == v, RichText::new(label).size(11.5)).on_hover_text(tip).clicked() {
                    mode = v;
                }
            }
            if mode != self.settings.map_mode {
                mode_go = Some(mode);
            }
            strip.separator();
            if strip.selectable_label(self.settings.map_25d, RichText::new("2.5D").size(11.5)).on_hover_text("Tilted map: boxes and rings stand up by size (2D is the default)").clicked() {
                tilt_go = true;
            }
            let size_text = format!("{} · {}", fmt_bytes(root.size), fmt_files(u64::from(root.files)));
            let size_w = width(&size_text, false) + 16.0;
            if strip.min_rect().min.x - size_w > crumbs_end + 12.0 {
                p.text(pos2(strip.min_rect().min.x - 10.0, hr.center().y), Align2::RIGHT_CENTER, size_text, theme::regular(11.5), t.muted);
            }

            // Legend (bottom)
            let legend = self.legend_items();
            let rows = legend.len().div_ceil(2) as f32;
            let legend_h = if legend.is_empty() { 0.0 } else { rows * 20.0 + 14.0 };
            let avail = ui.available_size();
            let (map_rect, resp) = ui.allocate_exact_size(vec2(full, (avail.y - legend_h).max(80.0)), Sense::click());
            // Screen readers: the map is a picture; say what it shows and where the words are.
            let root = self.summary.as_ref().map(|s| visible(&s.root)).unwrap_or_default();
            map_resp_info(&resp, format!("{} map of {root}. The biggest items are listed below it and in the tree.", if self.settings.treemap { "Treemap" } else { "Sunburst" }));

            let kind = self.kind();
            let tree_hover = self.hover_tree.as_ref().and_then(|k| self.map_index(k));
            let selected = self.selected.as_ref().and_then(|k| self.map_index(k)).filter(|&i| i != 0);
            {
                let m = self.map.as_mut().expect("map exists");
                // Tilted sunburst: the picture is half as tall, so lay the rings
                // out in a taller area to use the width.
                let lay_rect = if self.settings.map_25d && kind == Kind::Sunburst {
                    Rect::from_center_size(map_rect.center(), vec2(map_rect.width(), (map_rect.height() / 0.6).min(map_rect.width())))
                } else {
                    map_rect
                };
                let stale = !matches!(&m.cached, Some((r, k, _)) if *r == lay_rect && *k == kind);
                if stale {
                    m.cached = Some((lay_rect, kind, map::layout(kind, &m.nodes, &m.kids, lay_rect)));
                }
            }
            let m = self.map.as_ref().expect("map exists");
            let layout = &m.cached.as_ref().expect("layout").2;
            let tilt = self.settings.map_25d.then(|| map::Tilt::new(layout, &m.nodes, map_rect));
            let hit = resp.hover_pos().and_then(|q| match &tilt {
                Some(tl) => map::hit_25d(layout, tl, &m.nodes, q),
                None => map::hit(layout, q),
            });
            let hover_idx = match hit {
                Some(Hit::Node(i)) => Some(i),
                _ => None,
            };
            self.hover_map = hover_idx;
            let value = fmt_bytes(m.nodes[0].size);
            let pc = PaintCtx {
                nodes: &m.nodes,
                hover: hover_idx.or(tree_hover),
                selected,
                t: &t,
                center_title: &m.nodes[0].name,
                center_value: &value,
                can_go_up: m.up.is_some(),
                today: self.settings.color_by_age.then(diskroot_session::today_days),
                over: &m.over,
                grow: match m.born {
                    // Ease out over 220 ms; repaint only while it runs.
                    Some(b) if b.elapsed() < Duration::from_millis(220) => {
                        ui.ctx().request_repaint();
                        let x = b.elapsed().as_secs_f32() / 0.22;
                        1.0 - (1.0 - x).powi(3)
                    }
                    _ => 1.0,
                },
            };
            match &tilt {
                Some(tl) => map::paint_25d(&ui.painter_at(map_rect), layout, tl, &pc),
                None => map::paint(&ui.painter_at(map_rect), layout, &pc),
            }
            let mode_hint = match m.mode {
                1 if self.prev_sizes.is_none() => Some("Growth needs a previous scan of this place: it shows from the next scan on (File → Remember scans)"),
                1 => Some("Colours: change since the previous scan"),
                2 => Some("Colours: space you could get back"),
                _ => None,
            };
            // Hints only beside the sunburst, where there's room for them (the
            // treemap fills its area; the legend says the same).
            let room = match layout {
                _ if tilt.is_some() => 0.0,
                MapLayout::Sun { center, arcs, .. } => center.x - arcs.iter().map(|a| a.ro).fold(0.0, f32::max) - map_rect.min.x - 16.0,
                MapLayout::Tree { .. } => 0.0,
            };
            let fits = |h: &str| ui.painter().layout_no_wrap(h.to_owned(), theme::regular(11.0), t.faint).size().x <= room;
            if kind == Kind::Treemap || !mode_hint.is_none_or(fits) {
                // no room
            } else if let Some(h) = mode_hint {
                ui.painter_at(map_rect).text(map_rect.left_top() + vec2(10.0, 8.0), Align2::LEFT_TOP, h, theme::regular(11.0), if m.mode == 1 && self.prev_sizes.is_none() { theme::WARN } else { t.faint });
            } else if kind == Kind::Sunburst {
                let hint = if m.up.is_some() { "Click a piece to open it · the centre goes up · Ctrl+K: everything" } else { "Click a piece to open it · Ctrl+K: everything" };
                let hint = if fits(hint) { hint } else if fits("Click a piece to open it") { "Click a piece to open it" } else { "" };
                ui.painter_at(map_rect).text(map_rect.left_top() + vec2(10.0, 8.0), Align2::LEFT_TOP, hint, theme::regular(11.0), t.faint);
            }


            // Tooltip + clicks
            let mut go: Option<Key_> = None;
            let mut sel: Option<(Option<u32>, String, u8, u16)> = None;
            match hit {
                Some(Hit::Pith) => {
                    if let Some(up) = &m.up {
                        let resp = resp.on_hover_text("Up one level");
                        if resp.clicked() {
                            go = Some(up.clone());
                        }
                    }
                }
                Some(Hit::Node(i)) => {
                    let n = &m.nodes[i];
                    let root_size = m.nodes[0].size;
                    let label = self.cat_label(if n.own_cat == u8::MAX { n.cat } else { n.own_cat }, n.sub);
                    let resp = resp.on_hover_ui_at_pointer(|ui| {
                        ui.label(RichText::new(&n.name).font(theme::semibold(13.0)));
                        ui.label(RichText::new(format!("{}  ·  {} of {}", fmt_bytes(n.size), fmt_pct(n.size, root_size), m.nodes[0].name)).color(t.muted));
                        if let Some(sum) = &self.summary
                            && sum.on_disk > root_size
                        {
                            ui.label(RichText::new(format!("{} of all of {}", fmt_pct(n.size, sum.on_disk), sum.root)).color(t.muted));
                        }
                        if n.merged > 0 {
                            ui.label(RichText::new(format!("{} items too small to draw", fmt_count(u64::from(n.merged)))).color(t.muted));
                        } else if !label.is_empty() {
                            ui.label(RichText::new(label).color(t.muted));
                        }
                        if n.mtime > 0 {
                            let what = if n.dir { "Newest inside" } else { "Modified" };
                            ui.label(RichText::new(format!("{what}: {}", diskroot_session::age_text(diskroot_session::today_days(), n.mtime))).color(t.muted));
                        }
                        if !n.hint.is_empty() {
                            ui.label(RichText::new(&n.hint).color(t.faint).size(11.5));
                        }
                        if let Some(note) = m.notes.get(i).filter(|x| !x.is_empty()) {
                            ui.label(RichText::new(note).color(t.text).size(12.0));
                        }
                    });
                    if resp.clicked() && n.merged == 0 {
                        let id = (n.id >= 0).then_some(n.id as u32);
                        sel = Some((id, n.key.clone(), n.cat, n.sub));
                    }
                }
                None => {}
            }
            if let Some(k) = go {
                self.go_to(k);
            }
            if let Some((id, key, cat, sub)) = sel {
                match id {
                    Some(id) if key.is_empty() => self.reveal_node(id, Some((cat, sub))),
                    _ if !key.is_empty() => self.reveal_cat(&key, cat),
                    _ => {}
                }
            }

            // Legend
            if !legend.is_empty() {
                ui.add_space(4.0);
                let lp = ui.painter();
                let top = ui.cursor().min.y + 4.0;
                let col_w = (full - 24.0) / 2.0;
                let mut clicked_leg: Option<Key_> = None;
                for (k, (key, name, bytes, total, cat)) in legend.iter().enumerate() {
                    let cx = ui.min_rect().min.x + 12.0 + (k % 2) as f32 * (col_w + 0.0);
                    let cy = top + (k / 2) as f32 * 20.0 + 10.0;
                    let r = Rect::from_min_size(pos2(cx, cy - 9.0), vec2(col_w - 8.0, 18.0));
                    let resp = ui.interact(r, ui.scope_id().with(("leg", k)), Sense::click());
                    let leg_label = format!("{name}: {} ({})", fmt_bytes(*bytes), fmt_pct(*bytes, *total));
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &leg_label));
                    if resp.hovered() {
                        lp.rect_filled(r, CornerRadius::same(3), t.hover);
                    }
                    lp.rect_filled(Rect::from_center_size(pos2(cx + 6.0, cy), vec2(10.0, 10.0)), CornerRadius::same(2), theme::swatch(*cat));
                    // The name gives way to the number when the column is narrow.
                    let value = format!("{} · {}", fmt_bytes(*bytes), fmt_pct(*bytes, *total));
                    let value_w = lp.layout_no_wrap(value.clone(), theme::regular(11.5), t.muted).size().x;
                    if let Some(shown) = map::fit(lp, name, 12.0, (col_w - 12.0 - value_w - 17.0 - 10.0).max(20.0)) {
                        lp.text(pos2(cx + 17.0, cy), Align2::LEFT_CENTER, shown, theme::regular(12.0), t.text);
                    }
                    lp.text(pos2(cx + col_w - 12.0, cy), Align2::RIGHT_CENTER, value, theme::regular(11.5), t.muted);
                    if resp.clicked() {
                        clicked_leg = Some(key.clone());
                    }
                }
                ui.allocate_space(vec2(full, legend_h - 4.0));
                if let Some(k) = clicked_leg
                    && let Key_::Cat(key) = &k
                    && !key.starts_with("age:")
                    && !key.starts_with("mode:") {
                        if !self.settings.by_category {
                            self.settings.by_category = true;
                            self.regroup();
                        }
                        let cat = key.strip_prefix("cat:").and_then(|c| c.parse().ok()).unwrap_or(0);
                        self.reveal_cat(&key.clone(), cat);
                    }
            }
        });
        if let Some(k) = crumb_go {
            self.go_to(k);
        }
        if tilt_go {
            self.run_cmd(&ui.ctx().clone(), Cmd::Tilt);
        }
        if let Some(v) = mode_go
            && v != self.settings.map_mode
        {
            self.settings.map_mode = v;
            self.settings.save();
            self.map = None;
        }
    }

    /// Growth / reclaimable colouring of the map's nodes: (colour, legend
    /// code, tooltip line) per node; all empty in size mode.
    fn mode_colors(&mut self, nodes: &[Shape]) -> (Vec<Color32>, Vec<u8>, Vec<String>) {
        let mode = self.settings.map_mode;
        let Some(s) = &self.scanned else { return Default::default() };
        if mode == 0 || nodes.is_empty() {
            return Default::default();
        }
        let mut over = Vec::with_capacity(nodes.len());
        let mut codes = Vec::with_capacity(nodes.len());
        let mut notes = Vec::with_capacity(nodes.len());
        if mode == 1 {
            // Growth since the previous scan: folders of 1 MB or more are remembered.
            let root = self.summary.as_ref().map(|x| x.root.clone()).unwrap_or_default();
            for n in nodes {
                let real = n.key.is_empty() && n.id >= 0;
                let (code, note) = match (&self.prev_sizes, real && n.dir) {
                    (None, _) => (5, "No previous scan of this place to compare with".to_owned()),
                    (_, false) => (5, if n.key.is_empty() { "Files aren't tracked between scans (folders are)".to_owned() } else { "Growth is shown in the Location view".to_owned() }),
                    (Some(prev), true) => {
                        let full = s.path(n.id as u32).unwrap_or_default();
                        let rel = full.strip_prefix(root.as_str()).unwrap_or(&full).trim_start_matches('\\');
                        match prev.get(&remember::path_key(rel)).copied() {
                            None if n.size >= 1 << 20 => (4, "New since the previous scan (or under 1 MB then)".to_owned()),
                            None => (5, "Under 1 MB: not tracked between scans".to_owned()),
                            Some(before) => {
                                let d = i128::from(n.size) - i128::from(before);
                                let small = d.unsigned_abs() < (1u128 << 20).max(u128::from(before) / 100);
                                let code = if small {
                                    2
                                } else if d < 0 {
                                    3
                                } else if d >= 1 << 30 || d >= i128::from(before) / 4 {
                                    0
                                } else {
                                    1
                                };
                                (code, if small { "About the same as at the previous scan".to_owned() } else { format!("{} since the previous scan", fmt_delta(d)) })
                            }
                        }
                    }
                };
                over.push(theme::GROWTH[code as usize]);
                codes.push(200 + code);
                notes.push(note);
            }
        } else {
            if self.reclaim.is_none() {
                self.reclaim = Some(Arc::new(s.reclaimable()));
            }
            let table = self.reclaim.clone().unwrap_or_default();
            for (i, n) in nodes.iter().enumerate() {
                let parent = (n.parent >= 0).then_some(n.parent as usize).filter(|&p| p < i);
                // Inside a place that is reclaimable as a whole: same colour.
                if let Some(p) = parent
                    && codes[p] != 212
                    && over[p] == theme::RECLAIM[(codes[p] - 210) as usize]
                {
                    over.push(over[p]);
                    codes.push(codes[p]);
                    notes.push(notes[p].clone());
                    continue;
                }
                let (g, y) = if n.key.is_empty() && n.id >= 0 {
                    let v = table.get(&(n.id as u32)).copied().unwrap_or_default();
                    (v[0], v[1])
                } else {
                    // Category view: the rule behind a sub-type or place.
                    // (Same idea as Scanned::reclaimable: "check first" counts only
                    // for caches and Windows leftovers, not personal folders.)
                    let rule = s.rules().info.get(n.rule as usize);
                    let cache_like = n.cat == 6 || rule.is_some_and(|r| r.action.is_some());
                    match rule.and_then(|r| r.safety) {
                        Some(Safety::Green) if !rule.is_some_and(|r| r.user) => (n.size, 0),
                        Some(Safety::Yellow) if cache_like => (0, n.size),
                        _ => (0, 0),
                    }
                };
                let size = n.size.max(1);
                if g + y == 0 {
                    over.push(theme::RECLAIM[2]);
                    codes.push(212);
                    notes.push("Nothing here is labelled safe to clear or check first".to_owned());
                    continue;
                }
                let slot = usize::from(y > g);
                let frac = ((g + y) as f32 / size as f32).min(1.0);
                // Whole place reclaimable: full colour (children inherit it);
                // partly: the more it holds, the stronger the colour.
                let c = if frac >= 0.99 { theme::RECLAIM[slot] } else { lerp(theme::RECLAIM[2], theme::RECLAIM[slot], 0.3 + 0.6 * frac) };
                over.push(c);
                codes.push(210 + slot as u8);
                let mut note = Vec::new();
                if g > 0 {
                    note.push(format!("Safe to clear: {}", fmt_bytes(g)));
                }
                if y > 0 {
                    note.push(format!("Check first: {}", fmt_bytes(y)));
                }
                notes.push(note.join(" · "));
            }
        }
        (over, codes, notes)
    }

    /// (key, name, bytes, total, colour) entries under the map.
    fn legend_items(&self) -> Vec<(Key_, String, u64, u64, u8)> {
        let (Some(s), Some(m)) = (&self.scanned, &self.map) else { return Vec::new() };
        let cats = diskroot_session::categories();
        let mut out = Vec::new();
        if !m.codes.is_empty() {
            // Growth / reclaimable: the map root's parts by colour.
            let root_size = m.nodes.first().map_or(0, |n| n.size);
            let mut by: Vec<(u8, u64)> = Vec::new();
            if m.mode == 2
                && let (MapRoot::Node(n), Some(table)) = (&m.root, &self.reclaim)
            {
                let v = table.get(n).copied().unwrap_or_default();
                by = vec![(210, v[0]), (211, v[1]), (212, root_size.saturating_sub(v[0] + v[1]))];
            } else {
                for (i, n) in m.nodes.iter().enumerate().skip(1) {
                    if n.parent == 0 {
                        match by.iter_mut().find(|x| x.0 == m.codes[i]) {
                            Some(x) => x.1 += n.size,
                            None => by.push((m.codes[i], n.size)),
                        }
                    }
                }
                by.sort_by_key(|x| x.0);
            }
            let no_data = if self.prev_sizes.is_none() { "No previous scan yet" } else { "Not tracked (files, small folders)" };
            let names = ["Grew a lot", "Grew", "About the same", "Shrank", "New (or under 1 MB before)", no_data];
            let rnames = ["Safe to clear", "Check first", "Everything else"];
            for (code, bytes) in by.into_iter().filter(|x| x.1 > 0) {
                let name = if code >= 210 { rnames[(code - 210) as usize] } else { names[(code - 200) as usize] };
                out.push((Key_::Cat(format!("mode:{code}")), name.to_owned(), bytes, root_size, code));
            }
            return out;
        }
        if self.settings.color_by_age {
            if let Some(ages) = &m.ages {
                let total: u64 = ages.iter().sum();
                for (b, &v) in ages.iter().enumerate().filter(|x| *x.1 > 0) {
                    let name = diskroot_session::AGE_BUCKETS.get(b).map_or("Date unknown", |x| x.1);
                    out.push((Key_::Cat(format!("age:{b}")), name.to_owned(), v, total, 100 + b as u8));
                }
            }
            return out;
        }
        match &m.root {
            MapRoot::Node(n) => {
                let b = s.breakdown(*n);
                let total: u64 = b.iter().map(|x| x.1).sum();
                for (c, v) in b.into_iter().take(10) {
                    out.push((Key_::Cat(format!("cat:{c}")), cats[c as usize].name.to_owned(), v, total, c));
                }
            }
            MapRoot::Cat(k) if k.is_empty() || k.starts_with("cat:") => {
                if let Ok(rows) = s.cat_children(k, 10) {
                    let total: u64 = rows.iter().map(|r| r.bytes).sum();
                    for r in rows {
                        out.push((Key_::Cat(r.key), r.name, r.bytes, total, r.cat));
                    }
                }
            }
            MapRoot::Cat(_) => {}
        }
        out
    }

    fn details_pane(&mut self, ui: &mut Ui) {
        // Hardlink names of the selected file: asked once per selection.
        if let (Some(Key_::Node(n)), Some(s)) = (&self.selected, &self.scanned)
            && self.links_cache.as_ref().is_none_or(|c| c.0 != *n)
        {
            let names = match s.info(*n) {
                Ok(i) if !i.dir => diskroot_session::safety::hardlink_names(std::path::Path::new(&i.path)),
                _ => Vec::new(),
            };
            self.links_cache = Some((*n, names));
        }
        if let (Some(Key_::Node(n)), Some(s), Some(h)) = (&self.selected, &self.scanned, &self.hist)
            && self.hist_cache.as_ref().is_none_or(|c| c.0 != *n)
        {
            let series = match s.info(*n) {
                Ok(i) if i.dir => rel_of(&s.summary().root, &i.path).map(|rel| h.series(&rel)).unwrap_or_default(),
                _ => Vec::new(),
            };
            self.hist_cache = Some((*n, series));
        }
        let t = self.tokens();
        Self::pane_frame(&t).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            let Some(s) = &self.scanned else {
                ui.label(RichText::new("Select a folder or file to see what it is and whether it's safe to remove.").color(t.faint));
                return;
            };
            let mut action: Option<(&'static str, u32)> = None;
            let mut tool_action: Option<Action> = None;
            let mut mark_key: Option<String> = None;
            if !self.marked.is_empty() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{} items marked", self.marked.len())).font(theme::semibold(13.0)).color(theme::WARN));
                    if ui.add_enabled(self.job.is_none(), egui::Button::new(format!("Move {} marked to Recycle Bin…", self.marked.len()))).clicked() {
                        action = Some(("delete-marked", 0));
                    }
                    if ui.button("Clear marks").clicked() {
                        action = Some(("clear-marks", 0));
                    }
                });
                ui.add_space(4.0);
            }
            match &self.selected {
                Some(Key_::Node(n)) => {
                    let Ok(info) = s.info(*n) else { return };
                    let rule = s.rules().info.get(info.rule as usize);
                    let tool = rule.and_then(|r| r.action);
                    let (label, color, note) = match s.safety(*n) {
                        None if rule.is_some_and(|r| r.user) => (
                            "Your rule",
                            t.muted,
                            Some(rule.and_then(|r| r.note.clone()).unwrap_or_else(|| "Classified by your own rule. Check what it is before removing it.".into())),
                        ),
                        Some(Safety::Green) => ("Safe to clear", theme::cat_color(2), rule.and_then(|r| r.note.clone())),
                        Some(Safety::Yellow) if rule.is_some_and(|r| r.safety == Some(Safety::Green)) => (
                            "Check first",
                            theme::cat_color(6),
                            Some("A cache folder inside your own files, not an app's. It's probably rebuildable, but check what made it first: rebuilding a project's cache can take long.".into()),
                        ),
                        Some(Safety::Yellow) => ("Check first", theme::cat_color(6), rule.and_then(|r| r.note.clone())),
                        Some(Safety::Red) => ("Don't delete by hand", theme::cat_color(5), rule.and_then(|r| r.note.clone())),
                        None => ("Unknown", t.muted, Some("No rule for this yet. Check what it is before removing it.".into())),
                    };
                    let parent_size = info.parent.and_then(|p| s.info(p).ok()).map(|p| p.on_disk);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            // Leave room for the buttons on the right (170 px + gap);
                            // long names and paths wrap instead of running under them.
                            ui.set_max_width((ui.available_width() - 190.0).max(220.0));
                            egui::Grid::new("props").num_columns(2).spacing([14.0, 1.0]).show(ui, |ui| {
                                let key = |ui: &mut Ui, s: &str| {
                                    ui.label(RichText::new(s).color(t.muted));
                                };
                                key(ui, "Name");
                                ui.add(egui::Label::new(RichText::new(&info.name).font(theme::semibold(13.0))).wrap());
                                ui.end_row();
                                key(ui, "Location");
                                ui.add(egui::Label::new(&info.path).wrap());
                                ui.end_row();
                                key(ui, "Size on disk");
                                let mut size = format!("{} · {}", fmt_bytes(info.on_disk), fmt_files(u64::from(info.files)));
                                if let Some(ps) = parent_size {
                                    size += &format!(" · {} of its folder", fmt_pct(info.on_disk, ps));
                                }
                                if s.counted_elsewhere(*n) {
                                    size += " · another name of a hardlinked file: its bytes are counted under the other name";
                                }
                                let links = self.links_cache.as_ref().filter(|c| c.0 == *n && c.1.len() > 1);
                                if let Some((_, names)) = links {
                                    ui.horizontal(|ui| {
                                        ui.label(size);
                                        let others = names.iter().filter(|x| !x.eq_ignore_ascii_case(&info.path)).map(|x| visible(x)).collect::<Vec<_>>().join("\n");
                                        ui.label(RichText::new(format!("· {} names (hardlinks)", names.len())).color(theme::WARN))
                                            .on_hover_text(format!("This file is stored once but has {} names. Deleting this one frees nothing while the others exist.\n\nOther names:\n{others}", names.len()));
                                    });
                                } else {
                                    ui.label(size);
                                }
                                ui.end_row();
                                let mt = s.mtime(*n);
                                if mt > 0 {
                                    key(ui, if info.dir { "Newest inside" } else { "Modified" });
                                    ui.label(format!("{} · {}", diskroot_session::date_text(mt), diskroot_session::age_text(diskroot_session::today_days(), mt)));
                                    ui.end_row();
                                }
                                if let Some((_, series)) = self.hist_cache.as_ref().filter(|c| c.0 == *n && c.1.len() >= 2) {
                                    key(ui, "Size history");
                                    ui.horizontal(|ui| {
                                        size_chart(ui, &t, series, vec2(220.0, 22.0), false);
                                        let last = series[series.len() - 1];
                                        let week = series.iter().rev().find(|p| p.0.saturating_add(7 * 86_400) <= last.0).unwrap_or(&series[0]);
                                        let d = i128::from(last.1) - i128::from(week.1);
                                        let span = remember::gap_text(last.0.saturating_sub(week.0));
                                        let text = if d == 0 { format!("no change in {span}") } else { format!("{} in {span}", fmt_delta(d)) };
                                        ui.label(RichText::new(text).color(if d > 0 { theme::WARN } else { t.muted }))
                                            .on_hover_text("From the scans DiskRoot remembered (folders of 1 MB or more). View → Size history for more.");
                                    });
                                    ui.end_row();
                                } else if info.dir {
                                    key(ui, "Size history");
                                    let how = if self.settings.remember_scans {
                                        "None yet: it builds up with every scan (or turn on View → Scheduled scans)."
                                    } else {
                                        "None: turn on File → Remember scans, and it builds up with every scan."
                                    };
                                    ui.label(RichText::new(how).color(t.faint));
                                    ui.end_row();
                                }
                                if let Some(owner) = self.owners.as_ref().and_then(|o| o.of(&info.path)) {
                                    key(ui, "Belongs to");
                                    ui.label(owner).on_hover_text("From Windows' list of installed programs, Steam's game manifests or the Epic Games Launcher (read on this PC, nothing online)");
                                    ui.end_row();
                                }
                                key(ui, "Category");
                                ui.horizontal(|ui| {
                                    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                                    ui.painter().rect_filled(r, CornerRadius::same(2), theme::cat_color(info.cat));
                                    ui.label(self.cat_label(info.cat, info.sub));
                                });
                                ui.end_row();
                                key(ui, "Safe to delete?");
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(RichText::new(label).color(color).font(theme::semibold(13.0)));
                                    if let Some(n) = &note {
                                        ui.label(RichText::new(n).color(t.muted));
                                    }
                                });
                                ui.end_row();
                            });
                        });
                        ui.with_layout(Layout::top_down(Align::Max), |ui| {
                            if ui.add_sized([170.0, 28.0], egui::Button::new("Show in Explorer")).clicked() {
                                action = Some(("reveal", *n));
                            }
                            let can = info.protected.is_none() && self.job.is_none() && info.parent.is_some();
                            let btn = ui.add_enabled(can, egui::Button::new("Move to Recycle Bin…").min_size(vec2(170.0, 28.0)));
                            let btn = match &info.protected {
                                Some(why) => btn.on_disabled_hover_text(why),
                                None => btn,
                            };
                            if btn.clicked() {
                                action = Some(("delete", *n));
                            }
                            if let Some(a) = tool {
                                let (text, hint) = match a {
                                    Action::DismCleanup => ("Clean up with Windows…", "Runs Windows' DISM component cleanup (admin). DiskRoot itself deletes nothing here."),
                                    Action::SystemProtection => ("System Protection…", "Opens Windows' System Protection settings to manage restore points."),
                                    Action::StorageSettings => ("Open Storage settings…", "Windows removes these: Settings > System > Storage > Temporary files."),
                                };
                                if ui.add_sized([170.0, 28.0], egui::Button::new(text)).on_hover_text(hint).clicked() {
                                    tool_action = Some(a);
                                }
                            }
                            let cloud = s.is_cloud(*n) || diskroot_session::safety::in_cloud_folder(std::path::Path::new(&info.path));
                            if info.dir
                                && info.protected.is_none()
                                && info.parent.is_some()
                                && !cloud
                                && self.job.is_none()
                                && ui.add_sized([170.0, 28.0], egui::Button::new("Compress…")).on_hover_text("DiskRoot's fast compression (Windows' own file compression, several files at once): same files, less space. Undoable.").clicked()
                            {
                                action = Some(("compress", *n));
                            }
                            if !info.dir
                                && info.name.to_ascii_lowercase().ends_with(".vhdx")
                                && self.job.is_none()
                                && ui.add_sized([170.0, 28.0], egui::Button::new("Compact virtual disk…")).on_hover_text("Give back space a WSL or Docker disk no longer uses (Windows' diskpart, admin)").clicked()
                            {
                                action = Some(("vhdx", *n));
                            }
                        });
                    });
                }
                Some(Key_::Cat(k)) => {
                    let (bytes, files) = s.cat_total(k);
                    let name = self.rows.as_ref().and_then(|rs| rs.iter().find(|r| &r.key == self.selected.as_ref().unwrap())).map(|r| r.name.clone()).unwrap_or_default();
                    ui.label(RichText::new(name).font(theme::semibold(14.0)));
                    ui.label(format!("{} on disk · {}", fmt_bytes(bytes), fmt_files(files)));
                    ui.label(
                        RichText::new("Expand it to see the places holding these bytes. Select a place to see what it is and whether it's safe to remove.")
                            .color(t.muted),
                    );
                    if k.starts_with("sub:") {
                        let ids = s.markable(k);
                        if !ids.is_empty() {
                            let bytes: u64 = ids.iter().filter_map(|&n| s.info(n).ok()).map(|i| i.on_disk).sum();
                            ui.add_space(4.0);
                            if ui
                                .add_enabled(self.job.is_none(), egui::Button::new(format!("Mark all {} that are safe to clear ({})", ids.len(), fmt_bytes(bytes))))
                                .on_hover_text("Marks every place here that DiskRoot's rules label \"safe to clear\". Press Delete to review them in one confirmation.")
                                .clicked()
                            {
                                mark_key = Some(k.clone());
                            }
                        }
                    }
                }
                None => {}
            }
            if let Some(a) = tool_action {
                self.start_tool(a, false);
            }
            if let Some(k) = mark_key {
                self.mark_all(&k);
            }
            if let Some((a, n)) = action {
                match a {
                    "reveal" => self.reveal_in_explorer(n),
                    "compress" => self.ask_compress(n),
                    "vhdx" => self.ask_vhdx(n),
                    "delete-marked" => self.ask_delete_many(),
                    "clear-marks" => {
                        self.marked.clear();
                        self.rows = None;
                    }
                    _ => self.ask_delete(n),
                }
            }
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let t = self.tokens();
        // Progress while a job runs.
        if let Some(job) = &self.job {
            let secs = job.started.elapsed().as_secs_f32();
            let (title, text) = match &job.kind {
                JobKind::Scan(p) => (format!("Scanning {p}"), "Walking folders (no admin needed)…".to_owned()),
                JobKind::Fast(p) => (
                    format!("Fast scan of {p}"),
                    "Approve the Windows prompt. DiskRoot then reads the drive's Master File Table, read-only.".to_owned(),
                ),
                JobKind::Delete(n) => (format!("Moving \"{n}\" to the Recycle Bin"), "Windows is moving the files…".to_owned()),
                JobKind::Reopen(p) => (format!("Opening the remembered scan of {p}"), "Loading the last scan from this PC…".to_owned()),
                JobKind::Export(p) => ("Exporting to CSV".to_owned(), format!("Writing {p}…")),
                JobKind::DeleteMany(n) => (format!("Moving {n} items to the Recycle Bin"), "Each item is re-checked on disk first…".to_owned()),
                JobKind::Compress(title, ..) => (
                    title.clone(),
                    format!("Windows' own file compression, {} files at a time. You can keep using the PC; files stay usable throughout.", compress::default_threads()),
                ),
                JobKind::Tool(name) => (
                    name.clone(),
                    "Approve the Windows prompt. The tool runs in its own window and can take several minutes; DiskRoot waits until it's done.".to_owned(),
                ),
            };
            Modal::new(egui::Id::unique("diskroot-job")).show(ctx, |ui| {
                // Fixed size from the first frame, so the dialog doesn't jump.
                ui.set_width(420.0);
                ui.set_min_height(96.0);
                ui.label(RichText::new(title).font(theme::semibold(14.0)));
                ui.add_space(6.0);
                ui.label(RichText::new(text).color(t.muted));
                ui.add_space(10.0);
                // Indeterminate bar: a sap pulse gliding back and forth
                // (eased at both ends, so it never jumps).
                let (r, _) = ui.allocate_exact_size(vec2(420.0, 8.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(r, CornerRadius::same(4), t.head);
                if let JobKind::Compress(_, pr, total, _) = &job.kind {
                    // Real progress: bytes handled out of the plan's total.
                    let done = pr.done.load(Ordering::Relaxed);
                    let frac = if *total == 0 { 1.0 } else { (done as f32 / *total as f32).clamp(0.0, 1.0) };
                    p.rect_filled(Rect::from_min_size(r.min, vec2(r.width() * frac, r.height())), CornerRadius::same(4), t.accent);
                } else {
                    let w = r.width() * 0.28;
                    let k = (1.0 - (secs * 2.2).cos()) / 2.0; // 0 → 1 → 0, smooth
                    let x = r.min.x + k * (r.width() - w);
                    p.rect_filled(Rect::from_min_size(pos2(x, r.min.y), vec2(w, r.height())), CornerRadius::same(4), t.accent);
                }
                if let JobKind::Compress(title, pr, total, files) = &job.kind {
                    let done = pr.done.load(Ordering::Relaxed);
                    let speed = done as f64 / f64::from(secs.max(0.1));
                    let left = if speed > 0.0 { (total.saturating_sub(done)) as f64 / speed } else { 0.0 };
                    let saved = pr.saved.load(Ordering::Relaxed);
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!(
                        "{} of {} · {} of {} files · {}/s · about {} left",
                        fmt_bytes(done),
                        fmt_bytes(*total),
                        fmt_count(pr.files_done.load(Ordering::Relaxed)),
                        fmt_count(*files),
                        fmt_bytes(speed as u64),
                        if left >= 90.0 { format!("{:.0} min", left / 60.0) } else { format!("{left:.0} s") }
                    )).color(t.muted));
                    let what = if title.starts_with("Uncompressing") { "given back to the files" } else { "saved" };
                    ui.label(RichText::new(format!("{} {what} so far", fmt_bytes(saved))).color(t.accent));
                }
                if matches!(job.kind, JobKind::Scan(_)) {
                    let found = diskroot_session::scan_progress();
                    ui.add_space(4.0);
                    ui.label(RichText::new(if found > 1 { format!("{} items found", fmt_count(found)) } else { "Starting…".to_owned() }).color(t.muted));
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{secs:.0} s")).color(t.faint).size(11.5));
                    if matches!(job.kind, JobKind::Fast(_)) {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let stopping = job.cancel.load(Ordering::Relaxed);
                            if ui.add_enabled(!stopping, egui::Button::new(if stopping { "Cancelling…" } else { "Cancel" })).clicked() {
                                job.cancel.store(true, Ordering::Relaxed);
                            }
                        });
                    }
                    if let JobKind::Compress(_, pr, ..) = &job.kind {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let stopping = pr.cancel.load(Ordering::Relaxed);
                            let label = if stopping { "Stopping after the current files…" } else { "Stop" };
                            if ui.add_enabled(!stopping, egui::Button::new(label)).on_hover_text("Files already done stay compressed; the rest are left as they are.").clicked() {
                                pr.cancel.store(true, Ordering::Relaxed);
                            }
                        });
                    }
                });
            });
            return;
        }
        let Some(d) = &mut self.dialog else { return };
        let mut close = false;
        let mut confirm: Option<u32> = None;
        let mut action: Option<&'static str> = None;
        let mut jump: Option<String> = None;
        let wide = matches!(d, Dialog::History(_) | Dialog::Changes(_) | Dialog::Files(_) | Dialog::ConfirmMany(_) | Dialog::Dupes(_) | Dialog::Leftovers(_) | Dialog::Similar(_) | Dialog::Savings(_) | Dialog::Schedule(_) | Dialog::Growth(_) | Dialog::Health(_) | Dialog::Writers(_) | Dialog::Space(_) | Dialog::Unreadable(_) | Dialog::Home(_));
        let mut leftover_marks: Option<Vec<u32>> = None;
        let mut tool_start: Option<(Action, bool)> = None;
        let mut dupes_start = false;
        let mut dupes_mark: Option<Vec<u32>> = None;
        let mut requery: Option<String> = None;
        let mut jump_node: Option<u32> = None;
        let mut savings_go: Option<SavingsGo> = None;
        // One window id per dialog kind, so a dialog never inherits another
        // one's remembered size.
        let kind = std::mem::discriminant(&*d);
        let resp = Modal::new(egui::Id::unique("diskroot-dialog").with(kind)).show(ctx, |ui| {
            ui.set_width(if wide { 720.0 } else { 500.0 });
            match d {
                Dialog::Schedule(v) => {
                    ui.label(RichText::new("Scheduled scans").font(theme::semibold(16.0)));
                    ui.add(egui::Label::new(RichText::new("DiskRoot can scan by itself and remember the result, so when you open it you see what changed. Each run is a normal scan (no admin, no window) at background priority; it saves the result and exits, so nothing stays in memory between scans. Fast scan stays something you start here.").color(t.muted)).wrap());
                    ui.add_space(6.0);
                    ui.checkbox(&mut v.on, RichText::new("Scan automatically").font(theme::semibold(13.5)));
                    ui.add_enabled_ui(v.on, |ui| {
                        let label = |m: u32| {
                            let base = if m < 60 { format!("{m} minutes") } else if m == 60 { "1 hour".into() } else if m < 1440 { format!("{} hours", m / 60) } else { "1 day".into() };
                            if m == v.rec.0 { format!("{base} (recommended)") } else { base }
                        };
                        ui.horizontal(|ui| {
                            ui.label("Every");
                            egui::ComboBox::from_id_salt("sched-every").selected_text(label(v.s.every_min)).show_ui(ui, |ui| {
                                for m in schedule::CHOICES {
                                    ui.selectable_value(&mut v.s.every_min, m, label(m));
                                }
                            });
                        });
                        ui.add(egui::Label::new(RichText::new(v.rec.1.as_str()).color(t.faint).size(12.0)).wrap());
                        ui.add_space(4.0);
                        ui.label("What to scan:");
                        let mut remove: Option<usize> = None;
                        for (i, r) in v.s.roots.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(visible(r)).monospace());
                                if v.s.roots.len() > 1 && ui.small_button("Remove").clicked() {
                                    remove = Some(i);
                                }
                            });
                        }
                        if let Some(i) = remove {
                            v.s.roots.remove(i);
                        }
                        if let Some(sum) = &self.summary
                            && !v.s.roots.iter().any(|r| r.eq_ignore_ascii_case(&sum.root))
                            && ui.small_button(format!("Add {}", visible(&sum.root))).clicked()
                        {
                            v.s.roots.push(sum.root.clone());
                        }
                        ui.checkbox(&mut v.s.on_battery, "Also when the laptop runs on battery");
                        ui.checkbox(&mut v.s.only_idle, "Only when the PC has been idle for 5 minutes");
                        ui.add_space(4.0);
                        ui.label("Warn me with a Windows notification (after a background scan, at most once a day):");
                        let gb = |n: u32| if n == 0 { "never".to_owned() } else { format!("{n} GB") };
                        ui.horizontal(|ui| {
                            ui.label("when a drive has less free than");
                            egui::ComboBox::from_id_salt("warn-free").selected_text(gb(v.s.warn_free_gb)).show_ui(ui, |ui| {
                                for n in [0, 5, 10, 20, 50, 100] {
                                    ui.selectable_value(&mut v.s.warn_free_gb, n, gb(n));
                                }
                            });
                        });
                        ui.horizontal(|ui| {
                            ui.label("when a program keeps writing while the PC is idle, more than");
                            let day = |n: u32| if n == 0 { "never".to_owned() } else { format!("{n} GB a day") };
                            egui::ComboBox::from_id_salt("warn-idle").selected_text(day(v.s.warn_idle_gb_day)).show_ui(ui, |ui| {
                                for n in [0, 5, 10, 20, 50] {
                                    ui.selectable_value(&mut v.s.warn_idle_gb_day, n, day(n));
                                }
                            });
                        });
                        ui.checkbox(&mut v.s.weekly_report, "Once a week: a short report (growth, writes per day, drive health)");
                        ui.horizontal(|ui| {
                            ui.label("when what's scanned grows in a day by more than");
                            egui::ComboBox::from_id_salt("warn-grow").selected_text(gb(v.s.warn_grow_gb)).show_ui(ui, |ui| {
                                for n in [0, 5, 10, 20, 50, 100] {
                                    ui.selectable_value(&mut v.s.warn_grow_gb, n, gb(n));
                                }
                            });
                        });
                    });
                    ui.add_space(6.0);
                    // A portable copy can be moved or deleted; the task would then fail
                    // silently, and no uninstaller removes it.
                    let installed_copy = std::env::current_exe().ok().is_some_and(|e| {
                        let pf = std::env::var("ProgramFiles").unwrap_or_default().to_lowercase();
                        !pf.is_empty() && e.to_string_lossy().to_lowercase().starts_with(&pf)
                    });
                    if !installed_copy {
                        ui.add(egui::Label::new(RichText::new("This is a portable copy: the scheduled task runs this very file. If you move or delete it, turn scheduled scans off here first (no uninstaller removes the task for you).").color(theme::WARN).size(12.0)).wrap());
                    }
                    let state = if v.installed { "On: a Windows scheduled task runs it for your account." } else { "Off." };
                    ui.label(RichText::new(state).color(if v.installed { t.accent } else { t.muted }));
                    if let Some(m) = &v.msg {
                        ui.add(egui::Label::new(RichText::new(m.as_str()).color(theme::WARN)).wrap());
                    }
                    if !v.runs.is_empty() {
                        ui.add_space(4.0);
                        ui.label(RichText::new("Recent background scans").font(theme::semibold(13.0)));
                        egui::Grid::new("sched-runs").num_columns(5).striped(true).spacing([12.0, 3.0]).show(ui, |ui| {
                            for r in v.runs.iter().rev().take(6) {
                                ui.label(RichText::new(remember::age_text(r.time)).color(t.muted));
                                ui.label(visible(&r.root));
                                match &r.error {
                                    Some(e) => {
                                        ui.label(RichText::new("failed").color(theme::WARN)).on_hover_text(e.as_str());
                                        ui.label("");
                                        ui.label("");
                                    }
                                    None => {
                                        ui.label(format!("{:.1} s", r.secs));
                                        ui.label(RichText::new(format!("peak {} memory", fmt_bytes(r.peak_ram))).color(t.muted));
                                        let d = r.delta.map_or_else(String::new, |d| format!("{}{}", if d >= 0 { "+" } else { "−" }, fmt_bytes(d.unsigned_abs() as u64)));
                                        ui.label(d);
                                    }
                                }
                                ui.end_row();
                            }
                        });
                    }
                }
                Dialog::Home(h) => {
                    ui.label(RichText::new(format!("{} at a glance", visible(&h.root))).font(theme::semibold(17.0)));
                    ui.add_space(4.0);
                    if let Some((total, free)) = h.space {
                        let used = total.saturating_sub(free);
                        ui.label(RichText::new(format!("{} used of {} · {} free", fmt_bytes(used), fmt_bytes(total), fmt_bytes(free))).font(theme::semibold(13.5)));
                        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
                        let p = ui.painter_at(bar);
                        p.rect_filled(bar, CornerRadius::same(5), t.head);
                        let frac = (used as f64 / total.max(1) as f64) as f32;
                        p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac, bar.height())), CornerRadius::same(5), if h.nearly_full.is_some() { theme::WARN } else { t.accent });
                    }
                    for line in [h.nearly_full.as_ref(), h.forecast.as_ref()].into_iter().flatten() {
                        ui.add(egui::Label::new(RichText::new(line.as_str()).color(t.muted)).wrap());
                    }
                    if let Some((level, text)) = &h.health {
                        let col = match level {
                            Health::Good => t.accent,
                            Health::Watch => theme::WARN,
                            _ => theme::cat_color(5),
                        };
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Drive health:").color(t.muted));
                            if ui.add(egui::Label::new(RichText::new(text.as_str()).color(col)).sense(Sense::click())).on_hover_text("Open Drive health").clicked() {
                                action = Some("health-refresh");
                            }
                        });
                    }
                    ui.add_space(10.0);
                    ui.columns(3, |cols| {
                        cols[0].label(RichText::new("What changed").font(theme::semibold(13.5)));
                        match &self.changes {
                            Some(ch) if !ch.items.is_empty() => {
                                for c in ch.items.iter().take(3) {
                                    let name = if c.path.is_empty() { visible(&h.root) } else { visible(&c.path) };
                                    let r = cols[0].add(egui::Label::new(format!("{} {}", fmt_delta(c.delta()), name)).truncate().sense(Sense::click()));
                                    if r.on_hover_text("Show it in the tree").clicked() {
                                        jump = Some(c.path.clone());
                                    }
                                }
                                if ch.hardlink_switch() {
                                    plain_note(&mut cols[0], RichText::new("Partly because hardlinked files now count once.").color(theme::WARN).size(12.0));
                                }
                                if cols[0].small_button("All changes…").clicked() {
                                    action = Some("home-changes");
                                }
                            }
                            Some(_) => {
                                plain_note(&mut cols[0], RichText::new("Nothing changed by more than 1 MB since the previous scan.").color(t.faint));
                            }
                            None => {
                                plain_note(&mut cols[0], RichText::new("No earlier scan to compare with yet. Scan again later, or turn on scheduled scans.").color(t.faint));
                            }
                        }
                        cols[1].label(RichText::new("Get space back").font(theme::semibold(13.5)));
                        if let Some(b) = h.compress {
                            cols[1].label(format!("Compress old data: about {}", fmt_bytes(b)));
                        }
                        for (title, bytes) in &h.tips {
                            let b = bytes.map(|b| format!(" · {}", fmt_bytes(b))).unwrap_or_default();
                            cols[1].add(egui::Label::new(format!("{title}{b}")).truncate());
                        }
                        if cols[1].small_button("Save space…").clicked() {
                            action = Some("home-savings");
                        }
                        cols[2].label(RichText::new("Biggest here").font(theme::semibold(13.5)));
                        for (id, name, bytes) in &h.biggest {
                            let r = cols[2].add(egui::Label::new(format!("{} {}", fmt_bytes(*bytes), name)).truncate().sense(Sense::click()));
                            if r.on_hover_text("Show it in the tree").clicked() {
                                jump_node = Some(*id);
                            }
                        }
                    });
                }
                Dialog::Space(r) => {
                    let drive = r.root.trim_end_matches('\\').to_owned();
                    ui.label(RichText::new(format!("Where the space on {drive} is")).font(theme::semibold(16.0)));
                    ui.add_space(4.0);
                    let gap = r.not_counted();
                    let mut rows: Vec<(String, String)> = vec![
                        ("Used, as Windows reports it".into(), format!("{} of {}", fmt_bytes(r.used), fmt_bytes(r.total))),
                        ("Counted by this scan".into(), format!("{} (every file it could read, on disk)", fmt_bytes(r.counted))),
                    ];
                    if let Some(n) = r.ntfs_files {
                        rows.push(("  incl. NTFS's own files".into(), format!("{} ($MFT, $LogFile, ...: the file system's bookkeeping)", fmt_bytes(n))));
                    }
                    if let Some(v) = r.restore_points {
                        rows.push(("  incl. System Volume Information".into(), format!("{} (restore points, shadow copies, indexing data)", fmt_bytes(v))));
                    }
                    rows.push(if gap >= 0 {
                        ("Not counted".into(), fmt_bytes(gap as u64))
                    } else {
                        ("Counted more than used".into(), fmt_bytes(gap.unsigned_abs() as u64))
                    });
                    egui::Grid::new("space").num_columns(2).spacing([14.0, 4.0]).show(ui, |ui| {
                        for (k, v) in &rows {
                            ui.label(RichText::new(k).color(t.muted));
                            ui.label(v);
                            ui.end_row();
                        }
                    });
                    // Bar: counted vs not counted, on the used space.
                    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 12.0), Sense::hover());
                    let p = ui.painter_at(bar);
                    p.rect_filled(bar, CornerRadius::same(4), t.head);
                    let frac = if r.used == 0 { 1.0 } else { (r.counted as f64 / r.used as f64).clamp(0.0, 1.0) as f32 };
                    p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * frac, bar.height())), CornerRadius::same(4), t.accent);
                    ui.add_space(6.0);
                    let small = gap.unsigned_abs() as f64 <= r.used as f64 * 0.01;
                    let text = if r.fast_scan {
                        if small {
                            "Everything is accounted for, within 1% of the used space. What's left is space NTFS keeps for itself (reserved storage and bookkeeping that isn't a file).".to_owned()
                        } else if gap > 0 {
                            format!("{} isn't in any file Fast scan found: Windows' reserved storage (space set aside for updates), and space NTFS keeps for itself. Shown as is; DiskRoot doesn't guess.", fmt_bytes(gap as u64))
                        } else {
                            "Fast scan counted a little more than Windows reports as used (sizes rounded to whole clusters, and files changing during the scan).".to_owned()
                        }
                    } else if gap > 0 {
                        format!(
                            "A folder walk (no admin) can't see everything. The {} not counted is most likely in: the {} folders it couldn't read (System Volume Information with restore points and shadow copies, other users' folders, protected app folders), NTFS's own files ($MFT and friends), and Windows' reserved storage. A Fast scan (admin, read-only) reads all of them and measures each.",
                            fmt_bytes(gap as u64),
                            fmt_count(r.unreadable)
                        )
                    } else {
                        "This scan counted a little more than Windows reports as used (sizes rounded to whole clusters, and files changing during the scan).".to_owned()
                    };
                    ui.add(egui::Label::new(RichText::new(text).color(t.muted)).wrap());
                    if r.hardlink_names > 0 {
                        ui.add(egui::Label::new(RichText::new(format!("{} extra names of hardlinked files (the same data under two names, mostly in Windows' WinSxS) are counted once: 0 B each.", fmt_count(r.hardlink_names))).color(t.faint).size(12.0)).wrap());
                    }
                }
                Dialog::Unreadable(v) => {
                    let (root, paths) = (&v.0, &v.1);
                    ui.label(RichText::new(format!("Folders this scan couldn't read ({})", fmt_count(paths.len() as u64))).font(theme::semibold(16.0)));
                    ui.add(egui::Label::new(RichText::new("Windows didn't let a normal (no-admin) scan open these: folders only administrators or the system may read, like System Volume Information, other users' folders and WindowsApps. Their size isn't in the totals. Fast scan (admin, read-only) includes them.").color(t.muted)).wrap());
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().id_salt("unreadable").max_height(380.0).show(ui, |ui| {
                        for p in paths {
                            let r = ui.add(egui::Label::new(RichText::new(visible(p)).monospace().size(12.0)).truncate().sense(Sense::click()));
                            if r.on_hover_text("Click to show it in the tree").clicked() {
                                jump = rel_of(root, p);
                            }
                        }
                    });
                }
                Dialog::Writers(v) => {
                    if v.last.elapsed() >= Duration::from_secs(2) {
                        v.sampler.tick();
                        v.last = Instant::now();
                    }
                    ctx.request_repaint_after(Duration::from_secs(2));
                    let rate = |b: f64| if b < 1024.0 { "0".to_owned() } else { format!("{}/s", fmt_bytes(b as u64)) };
                    ui.label(RichText::new("Who's writing").font(theme::semibold(16.0)));
                    ui.add(egui::Label::new(RichText::new("Programs writing right now, measured every 2 s while this window is open. \"Writes\" counts everything a program writes (files, and also pipes and some network), so the drive's own counter is shown too: when the drive is busy while you're idle and one program leads, that's the one.").color(t.muted)).wrap());
                    ui.add_space(4.0);
                    for (n, letters) in &v.disks {
                        let now = v.sampler.disks.iter().find(|d| d.0 == *n).map_or(0.0, |d| d.1);
                        let total = v.sampler.disk_totals.get(n).copied().unwrap_or(0);
                        ui.label(RichText::new(format!("Drive {letters} · writing {} now · {} since this window opened", rate(now), fmt_bytes(total))).font(theme::semibold(13.0)));
                    }
                    let secs = v.sampler.seconds;
                    ui.label(RichText::new(format!("Measured for {:.0} s ({:.0} s of it idle: no key or mouse for 5 min)", secs, v.sampler.idle_seconds)).color(t.faint).size(12.0));
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().id_salt("writers-scroll").max_height(520.0).show(ui, |ui| {
                    let top = v.sampler.top(8);
                    if top.is_empty() {
                        ui.label(RichText::new(if secs < 1.0 { "Measuring…" } else { "Nothing written yet." }).color(t.faint));
                    } else {
                        egui::Grid::new("writers").num_columns(4).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                            for h in ["Program", "Now", "While you used the PC", "While idle"] {
                                ui.label(RichText::new(h).color(t.muted).size(12.0));
                            }
                            ui.end_row();
                            for w in &top {
                                if diskroot_session::offenders::for_process(&w.name).is_some() {
                                    ui.label(RichText::new(format!("⚠ {}", w.name)).color(theme::WARN));
                                } else {
                                    ui.label(&w.name);
                                }
                                ui.label(rate(w.now));
                                ui.label(fmt_bytes(w.active));
                                ui.label(RichText::new(fmt_bytes(w.idle)).color(if w.idle > 100 << 20 { theme::WARN } else { t.text }));
                                ui.end_row();
                            }
                        });
                    }
                    let known: Vec<&diskroot_session::offenders::Offender> = top.iter().map(|w| w.name.as_str()).chain(v.away.iter().map(|a| a.0.as_str())).filter_map(diskroot_session::offenders::for_process).collect();
                    let mut shown_titles: Vec<&str> = Vec::new();
                    for o in known {
                        if shown_titles.contains(&o.title) {
                            continue;
                        }
                        shown_titles.push(o.title);
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(RichText::new(format!("⚠ {}: {} {}", o.title, o.what, o.todo)).color(theme::WARN)).wrap())
                            .on_hover_text(format!("Checked {} against:\n{}", o.checked, o.sources.join("\n")));
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new("While you were away (background scans, last 7 days)").font(theme::semibold(13.0)));
                    if v.away_runs == 0 {
                        ui.add(egui::Label::new(RichText::new("No idle samples yet. Each scheduled scan first measures 30 s of writes; turn on View → Scheduled scans to catch programs that write while nobody is using the PC.").color(t.faint)).wrap());
                    } else {
                        ui.label(RichText::new(format!("{} idle samples of 30 s; average while idle:", v.away_runs)).color(t.muted).size(12.0));
                        for (name, r, c) in &v.away {
                            ui.label(format!("{name}: {} (≈ {} a day) · in {c} of {} samples", rate(*r), fmt_bytes((*r * 86_400.0) as u64), v.away_runs));
                        }
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new("Settings that write a lot (advice only: DiskRoot changes nothing)").font(theme::semibold(13.0)));
                    for tip in &v.advice {
                        ui.add(egui::Label::new(RichText::new(format!("• {tip}")).color(t.muted)).wrap());
                    }
                    if v.advice.is_empty() {
                        ui.label(RichText::new("Nothing to point out.").color(t.faint));
                    }
                    ui.add_space(8.0);
                    ui.label(RichText::new("Folders changed most often").font(theme::semibold(13.0)));
                    if self.watcher.is_none() {
                        ui.label(RichText::new("Needs live updates on and a scan on screen (it watches the scanned folder while DiskRoot is open).").color(t.faint));
                    } else {
                        let mut hot: Vec<(&String, &u32)> = self.hot.iter().collect();
                        hot.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
                        let root = self.summary.as_ref().map(|s| s.root.trim_end_matches('\\').to_owned()).unwrap_or_default();
                        ui.label(RichText::new(format!("Changes Windows reported under {} in the last {} (counts, not bytes):", visible(&root), remember::gap_text(self.hot_since.elapsed().as_secs()))).color(t.muted).size(12.0));
                        if hot.is_empty() {
                            ui.label(RichText::new("None yet.").color(t.faint));
                        }
                        for (folder, n) in hot.into_iter().take(8) {
                            let shown = if folder.is_empty() { root.clone() } else { format!("{root}\\{folder}") };
                            ui.label(format!("{n} × {}", visible(&shown)));
                        }
                    }
                    });
                }
                Dialog::Health(v) => {
                    let (drives, trim) = (&v.drives, &v.trim);
                    ui.label(RichText::new("Drive health").font(theme::semibold(16.0)));
                    ui.add(egui::Label::new(RichText::new("What the drives report about themselves: how much of their rated life is used, how much was written, temperature and errors. DiskRoot only asks for these reports; it changes nothing and needs no admin.").color(t.muted)).wrap());
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().id_salt("health").max_height(420.0).show(ui, |ui| {
                        if drives.is_empty() {
                            ui.label(RichText::new("Windows didn't list any drives.").color(t.faint));
                        }
                        for (d, trend) in drives {
                            let kind = match d.ssd {
                                Some(true) => format!("{} SSD", d.bus),
                                Some(false) => format!("{} hard disk", d.bus),
                                None => d.bus.clone(),
                            };
                            let letters = if d.letters.is_empty() { "no drive letter".to_owned() } else { d.letters.join(" ") };
                            ui.label(RichText::new(format!("{letters} · {} · {kind}", if d.model.is_empty() { "unknown model" } else { &d.model })).font(theme::semibold(13.5)));
                            let (level, text) = d.verdict();
                            let col = match level {
                                Health::Good => t.accent,
                                Health::Watch => theme::WARN,
                                Health::Bad => theme::cat_color(5),
                                Health::Unknown => t.muted,
                            };
                            ui.add(egui::Label::new(RichText::new(text).color(col).font(theme::semibold(13.0))).wrap());
                            for f in d.facts() {
                                ui.add(egui::Label::new(RichText::new(f).color(t.muted)).wrap());
                            }
                            if let Some(tr) = trend {
                                match tr.per_day {
                                    Some(b) => {
                                        ui.horizontal(|ui| {
                                            ui.label(format!("Writes: about {} a day (last {:.0} days)", fmt_bytes(b as u64), tr.days.min(30.0)));
                                            if tr.daily.len() >= 2 {
                                                size_chart(ui, &t, &tr.daily, vec2(180.0, 22.0), false);
                                            }
                                        });
                                    }
                                    None => {
                                        ui.label(RichText::new("Writes per day: shown after two days of notes (DiskRoot notes the numbers once a day when it scans).").color(t.faint));
                                    }
                                }
                                ui.add(egui::Label::new(RichText::new(tr.life.as_str())).wrap());
                            }
                            if let Some((letter, items)) = &v.at_risk
                                && d.letters.iter().any(|l| l.eq_ignore_ascii_case(letter))
                                && !items.is_empty()
                            {
                                let list = items.iter().map(|(k, b, f)| format!("{k} {} ({})", fmt_bytes(*b), fmt_files(*f))).collect::<Vec<_>>().join(" · ");
                                let urgent = level != Health::Good && level != Health::Unknown;
                                ui.add(egui::Label::new(RichText::new(format!("If {letter} failed, this would be lost unless you have a copy somewhere else: {list} (OneDrive folders aren't counted here). Keep a copy (File History, OneDrive, an external drive){}", if urgent { ", now" } else { "" }) + ".").color(if urgent { theme::WARN } else { t.muted })).wrap());
                            }
                            if let Some(o) = diskroot_session::offenders::for_drive(&d.model, &d.firmware) {
                                ui.add(egui::Label::new(RichText::new(format!("⚠ {}: {} {}", o.title, o.what, o.todo)).color(theme::WARN)).wrap())
                                    .on_hover_text(format!("Checked {} against:\n{}", o.checked, o.sources.join("\n")));
                            }
                            if !d.firmware.is_empty() {
                                ui.label(RichText::new(format!("Disk {} · firmware {}", d.number, d.firmware)).color(t.faint).size(11.5));
                            }
                            ui.add_space(8.0);
                        }
                    });
                    let trim_text = match trim {
                        Some(true) => "Windows sends TRIM to SSDs (on, as it should be).",
                        Some(false) => "Windows TRIM is turned off. SSDs stay faster and wear less with it on: run \"fsutil behavior set DisableDeleteNotify 0\" as admin to turn it back on.",
                        None => "Couldn't read whether Windows sends TRIM.",
                    };
                    ui.add(egui::Label::new(RichText::new(trim_text).color(if *trim == Some(false) { theme::WARN } else { t.muted })).wrap());
                    ui.add(egui::Label::new(RichText::new("\"Rated life used\" is the drive's own estimate from its flash wear; a drive often keeps working past 100%. \"Written\" is what Windows sent to it.").color(t.faint).size(11.5)).wrap());
                }
                Dialog::Growth(v) => {
                    ui.label(RichText::new("Size history").font(theme::semibold(16.0)));
                    ui.add(egui::Label::new(RichText::new(format!(
                        "{} · {} scans since {} · the history uses {} on this PC",
                        visible(&v.root),
                        v.points,
                        diskroot_session::date_text((v.first / 86_400) as u16),
                        fmt_bytes(v.disk)
                    )).color(t.muted)).wrap());
                    ui.add_space(6.0);
                    ui.label(RichText::new(if v.drive { "Used space on the drive" } else { "Size of the folder" }).font(theme::semibold(13.0)));
                    if v.chart.len() >= 2 {
                        size_chart(ui, &t, &v.chart, vec2(ui.available_width(), 120.0), true);
                    } else {
                        ui.label(RichText::new("One scan so far: the chart starts with the next one.").color(t.faint));
                    }
                    if let Some(f) = &v.forecast {
                        ui.add_space(4.0);
                        let col = if matches!(f, Forecast::Full { low, .. } if *low < 60) { theme::WARN } else { t.text };
                        ui.add(egui::Label::new(RichText::new(f.text(v.root.trim_end_matches('\\'))).color(col)).wrap())
                            .on_hover_text("From the drive's free space at the last scan of each day, over up to 60 days. Sudden one-off jumps (one big download) aren't counted as the pace. A rough guide, not a promise.");
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("What grew").font(theme::semibold(13.0)));
                        for (i, name) in ["Last day", "Last week", "Last month"].into_iter().enumerate() {
                            ui.selectable_value(&mut v.tab, i, name);
                        }
                    });
                    match &v.growers[v.tab] {
                        None => {
                            ui.label(RichText::new("Needs at least two scans.").color(t.faint));
                        }
                        Some(ch) => {
                            ui.label(RichText::new(format!(
                                "Comparing the scan from {} with the latest ({}). {} → {} ({})",
                                remember::age_text(ch.before_time),
                                remember::age_text(ch.after_time),
                                fmt_bytes(ch.before_total),
                                fmt_bytes(ch.after_total),
                                fmt_delta(i128::from(ch.after_total) - i128::from(ch.before_total))
                            )).color(t.muted).size(12.0));
                            if ch.mixed_methods() {
                                ui.add(egui::Label::new(RichText::new("One of these scans was a Fast scan and the other a folder walk: they count some things differently, so part of the change comes from the method.").color(theme::WARN).size(12.0)).wrap());
                            }
                            if ch.items.is_empty() {
                                ui.label(RichText::new("No folder changed by more than 1 MB (or 0.1% of the total).").color(t.faint));
                            } else {
                                egui::ScrollArea::vertical().id_salt("growers").max_height(220.0).show(ui, |ui| {
                                    egui::Grid::new("growers").num_columns(3).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                                        for c in &ch.items {
                                            let d = c.delta();
                                            ui.label(RichText::new(fmt_delta(d)).color(if d > 0 { theme::WARN } else { t.accent }).strong());
                                            ui.label(RichText::new(fmt_bytes(c.after)).color(t.muted));
                                            let shown = if c.path.is_empty() { "(the scanned folder itself)".to_owned() } else { c.path.clone() };
                                            let note = if c.before == 0 { "  new" } else if c.after == 0 { "  gone" } else { "" };
                                            let r = ui.add(egui::Label::new(format!("{shown}{note}")).truncate().sense(Sense::click()));
                                            if r.on_hover_text("Click to show it in the tree").clicked() {
                                                jump = Some(c.path.clone());
                                            }
                                            ui.end_row();
                                        }
                                    });
                                });
                            }
                        }
                    }
                    if !v.logs.is_empty() {
                        ui.add_space(8.0);
                        ui.label(RichText::new("Log files that keep growing").font(theme::semibold(13.0)));
                        ui.label(RichText::new("They grew in every one of the last scans: space now, and SSD writes all the time. Check what writes them before deleting (a program may need its log).").color(t.muted).size(12.0));
                        for g in v.logs.iter().take(8) {
                            let r = ui.add(egui::Label::new(format!("{} → {} over {} ({} scans) · {}", fmt_bytes(g.first), fmt_bytes(g.last), remember::gap_text(g.over), g.scans, g.path)).truncate().sense(Sense::click()));
                            if r.on_hover_text("Click to show it in the tree").clicked() {
                                jump = Some(g.path.clone());
                            }
                        }
                    }
                }
                Dialog::Memory => {
                    self.footprint.sample();
                    let f = self.footprint;
                    ui.label(RichText::new("DiskRoot's memory and CPU").font(theme::semibold(16.0)));
                    ui.add_space(4.0);
                    facts(ui, &t, &[
                        ("RAM in use", &format!("{} (the most so far: {})", fmt_bytes(f.ram), fmt_bytes(f.peak))),
                        ("Private", &format!("{} (memory only DiskRoot holds; Task Manager's \"Commit size\")", fmt_bytes(f.private))),
                        ("CPU", &format!("{:.1}% of the whole processor right now. Idle, DiskRoot uses almost none (well under 1% of one core).", f.cpu_pct)),
                    ]);
                    ui.add_space(6.0);
                    if let Some(s) = &self.scanned {
                        let rep = s.memory_report();
                        let sum = |keys: &[&str]| rep.iter().filter(|(k, _)| keys.iter().any(|p| k.starts_with(p))).map(|r| r.1 as u64).sum::<u64>();
                        let n = s.summary().files + s.summary().dirs;
                        ui.label(RichText::new(format!("The scan on screen ({} items) holds:", fmt_count(n))).font(theme::semibold(13.0)));
                        facts(ui, &t, &[
                            ("Names", &fmt_bytes(sum(&["index.names", "index.name_"]))),
                            ("Sizes and dates", &fmt_bytes(sum(&["index.size", "index.alloc", "index.files", "index.mtime", "index.added", "index.flags"]))),
                            ("Folder structure", &fmt_bytes(sum(&["index.parent", "children"]))),
                            ("Categories", &fmt_bytes(sum(&["cls.", "cats."]))),
                        ]);
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(RichText::new("Memory grows with the number of files scanned (about 80 bytes each), plus the window and the map. Freeing it keeps your scan saved on this PC: it reopens in about a second.").color(t.muted)).wrap());
                    } else if let Some(root) = &self.freed {
                        ui.add(egui::Label::new(RichText::new(format!("No scan in memory. Your scan of {} is saved and reopens in about a second.", visible(root))).color(t.muted)).wrap());
                    } else {
                        ui.label(RichText::new("No scan in memory.").color(t.muted));
                    }
                    ui.add_space(6.0);
                    let mut gentle = self.settings.gentle_scan;
                    if ui
                        .checkbox(&mut gentle, "Scan gently: other programs stay fast")
                        .on_hover_text("Scans run at Windows' background priority (CPU and disk). Measured on a full C: drive: another program's work took 85% longer during a normal scan and no longer during a gentle one; the scan itself took about 8% longer. Applies to the normal scan (Fast scan is already short).")
                        .changed()
                    {
                        self.settings.gentle_scan = gentle;
                        self.settings.save();
                    }
                    let mut auto = self.settings.free_when_minimized;
                    if ui.checkbox(&mut auto, "Free memory when DiskRoot has been minimized for 10 minutes").changed() {
                        self.settings.free_when_minimized = auto;
                        self.settings.save();
                    }
                    if !self.settings.remember_scans {
                        ui.label(RichText::new("Remembering scans is off (File menu), so freeing memory means scanning again later.").color(theme::WARN).size(12.0));
                    }
                }
                Dialog::About => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(56.0, 56.0)));
                        ui.vertical(|ui| {
                            ui.label(RichText::new("DiskRoot").font(theme::semibold(18.0)));
                            ui.label(format!("Version {} · disk space analyzer for Windows", env!("CARGO_PKG_VERSION")));
                        });
                    });
                    ui.add_space(6.0);
                    ui.label(RichText::new("DiskRoot gets to the root of what fills your drive: from the raw file table up to plain-English answers.").color(t.muted));
                }
                Dialog::Welcome => {
                    ui.label(RichText::new("Welcome to DiskRoot").font(theme::semibold(16.0)));
                    ui.add_space(4.0);
                    ui.label(RichText::new("Before you start, here is exactly what DiskRoot does on your computer:").color(t.muted));
                    ui.add_space(8.0);
                    facts(ui, &t, &[
                        ("Reads", "File names and sizes. It doesn't read what's inside your files, except when you start View → Find duplicate files."),
                        ("Stays offline", "No network connections, no accounts, no telemetry."),
                        ("Asks first", "Admin rights only when you ask for them, always through the Windows prompt: Fast scan (the helper exits after the scan), and the optional DISM cleanup and virtual-disk compaction."),
                        ("Deletes carefully", "Only when you confirm, always to the Recycle Bin, re-checked on disk first. Windows and system folders are locked."),
                        ("Keeps a record", "Every delete is written to a log on this PC (File → Deletion history)."),
                        ("Remembers scans", "Your last scans are saved on this PC (names and sizes only), so they reopen instantly and DiskRoot can show what changed. Turn it off or forget them in the File menu."),
                    ]);
                }
                Dialog::Security(ig) => {
                    ui.label(RichText::new("Security & privacy").font(theme::semibold(16.0)));
                    ui.add_space(8.0);
                    facts(ui, &t, &[
                        ("Network", "DiskRoot makes no network connections and sends nothing anywhere. The native app has no browser engine."),
                        ("Disk access", "Folders and drives are opened for reading only; scanning never changes your files. DiskRoot saves its own scans (compressed) in %LOCALAPPDATA%\\DiskRoot."),
                        ("Admin rights", "Only after the Windows prompt: for Fast scan (a helper reads the file table in about 1–5 s and exits), and if you use them, Windows' DISM cleanup and virtual-disk compaction."),
                        ("Deleting", "Always to the Recycle Bin, after you confirm. Right before moving, DiskRoot re-checks the item: if it changed, or a folder on its path became a link since the scan, it refuses."),
                        ("Warnings", "Extra confirmation for OneDrive folders (deleting syncs to your other devices), Git repositories (unpushed work) and very large deletes."),
                        ("History", "Each delete is logged on this PC in %LOCALAPPDATA%\\DiskRoot\\deletions.log."),
                        ("Remembered scans", "Names and sizes of your last scans (up to 8 drives or folders) are kept in %LOCALAPPDATA%\\DiskRoot\\cache for instant reopen and \"what changed\". Never uploaded. File → Remember scans turns it off; File → Forget remembered scans deletes them."),
                    ]);
                    ui.add_space(10.0);
                    ui.separator();
                    ui.label(RichText::new("This copy of DiskRoot").font(theme::semibold(13.5)));
                    ui.add_space(4.0);
                    let (sig_ok, sig_text) = match ig.signature {
                        SignatureState::Signed => (true, "Digitally signed, and Windows trusts the signature."),
                        SignatureState::Unsigned => (false, "Not signed (normal for a build from source). Official releases will be signed."),
                        SignatureState::Invalid => (false, "Has a signature Windows does NOT trust. Don't use Fast scan with this copy."),
                    };
                    let loc_text = if ig.protected_location {
                        "Installed in Program Files, which only administrators can change."
                    } else {
                        "Running from a folder your user account can change. Install DiskRoot to protect the admin helper used by Fast scan."
                    };
                    for (ok, k, v) in [(ig.protected_location, "Location", loc_text), (sig_ok, "Signature", sig_text)] {
                        let (mark, col) = if ok { ("✔", t.accent) } else { ("!", theme::WARN) };
                        ui.label(RichText::new(format!("{mark}  {k}")).font(theme::semibold(13.0)).color(col));
                        ui.add(egui::Label::new(RichText::new(v).color(t.muted)).wrap());
                        ui.add_space(4.0);
                    }
                    ui.add_space(2.0);
                    ui.label(RichText::new(&ig.exe).monospace().size(11.0).color(t.faint));
                }
                Dialog::History(entries) => {
                    ui.label(RichText::new("Deletion history").font(theme::semibold(16.0)));
                    ui.label(RichText::new("Everything deleted through DiskRoot on this PC, newest first. Recycled items can be restored from the Recycle Bin.").color(t.muted));
                    ui.add_space(8.0);
                    if entries.is_empty() {
                        ui.label(RichText::new("Nothing has been deleted with DiskRoot yet.").color(t.faint));
                    } else {
                        egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
                            egui::Grid::new("history").num_columns(4).striped(true).spacing([14.0, 5.0]).show(ui, |ui| {
                                for h in ["When (UTC)", "Result", "Size", "Item"] {
                                    ui.label(RichText::new(h).color(t.muted).size(12.0));
                                }
                                ui.end_row();
                                for e in entries.iter() {
                                    ui.label(RichText::new(&e.time).monospace().size(11.5));
                                    let col = if e.outcome == "recycled" { t.accent } else { t.muted };
                                    let short = e.outcome.split(':').next().unwrap_or("");
                                    ui.label(RichText::new(short).color(col)).on_hover_text(&e.outcome);
                                    ui.label(fmt_bytes(e.bytes));
                                    ui.add(egui::Label::new(visible(&e.path)).truncate())
                                        .on_hover_text(format!("{}\n{} · {}", e.path, e.category, fmt_files(u64::from(e.files))));
                                    ui.end_row();
                                }
                            });
                        });
                    }
                }
                Dialog::Changes(ch) => {
                    ui.label(RichText::new("What changed").font(theme::semibold(16.0)));
                    ui.label(
                        RichText::new(format!(
                            "{} · previous scan {} → latest scan {} ({} later)",
                            ch.root,
                            remember::age_text(ch.before_time),
                            remember::age_text(ch.after_time),
                            remember::gap_text(ch.after_time.saturating_sub(ch.before_time))
                        ))
                        .color(t.muted),
                    );
                    if ch.mixed_methods() {
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!(
                                "The previous scan was a {} and this one a {}. They count some things differently \
                                 (files with several names, folders only admins can read, System Volume Information), \
                                 so part of this comes from the scan method, not from files that changed. \
                                 Scan the same way twice for an exact comparison.",
                                Changes::method_name(&ch.before_mode),
                                Changes::method_name(&ch.after_mode)
                            ))
                            .color(theme::WARN),
                        );
                    }
                    if ch.hardlink_switch() {
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(RichText::new("This version of DiskRoot counts hardlinked files once (the same data under two names, mostly in Windows' WinSxS); the previous scan counted every name. Part of this drop is that, not deleted files.").color(theme::WARN)).wrap());
                    }
                    ui.add_space(4.0);
                    let total = i128::from(ch.after_total) - i128::from(ch.before_total);
                    ui.label(format!("{} → {}  ({})", fmt_bytes(ch.before_total), fmt_bytes(ch.after_total), fmt_delta(total)));
                    ui.add_space(8.0);
                    if ch.items.is_empty() {
                        ui.label(RichText::new("No folder changed by more than 1 MB (or 0.1% of the total).").color(t.faint));
                    } else {
                        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                            egui::Grid::new("changes").num_columns(4).striped(true).spacing([14.0, 5.0]).show(ui, |ui| {
                                for h in ["Change", "Now", "Before", "Folder"] {
                                    ui.label(RichText::new(h).color(t.muted).size(12.0));
                                }
                                ui.end_row();
                                for c in &ch.items {
                                    let d = c.delta();
                                    let col = if d > 0 { theme::WARN } else { t.accent };
                                    ui.label(RichText::new(fmt_delta(d)).color(col).strong());
                                    ui.label(fmt_bytes(c.after));
                                    ui.label(RichText::new(fmt_bytes(c.before)).color(t.muted));
                                    let shown = if c.path.is_empty() { "(the scanned folder itself)".to_owned() } else { c.path.clone() };
                                    let note = if c.before == 0 { "  new" } else if c.after == 0 { "  gone" } else { "" };
                                    let r = ui.add(egui::Label::new(format!("{shown}{note}")).truncate().sense(Sense::click()));
                                    if r.on_hover_text("Click to show it in the tree").clicked() {
                                        jump = Some(c.path.clone());
                                    }
                                    ui.end_row();
                                }
                            });
                        });
                    }
                }
                Dialog::ConfirmMany(c) => {
                    ui.label(RichText::new(format!("Move {} items to the Recycle Bin?", c.items.len())).font(theme::semibold(15.0)));
                    ui.label(format!("Frees {} in total", fmt_bytes(c.bytes)));
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().id_salt("many").max_height(160.0).show(ui, |ui| {
                        for p in &c.items {
                            ui.horizontal(|ui| {
                                ui.add_sized(vec2(70.0, 18.0), egui::Label::new(RichText::new(fmt_bytes(p.freed)).color(t.muted)));
                                ui.add(egui::Label::new(visible(&p.path.display().to_string())).truncate());
                            });
                        }
                    });
                    if !c.refused.is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new(format!("Skipped ({}): DiskRoot won't delete these", c.refused.len())).font(theme::semibold(13.0)).color(theme::cat_color(5)));
                        for (name, why) in &c.refused {
                            ui.add(egui::Label::new(RichText::new(format!("{}: {why}", visible(name))).color(t.muted)).wrap());
                        }
                    }
                    ui.add_space(6.0);
                    let w = &c.warnings;
                    let mut notes = Vec::new();
                    if w.cloud {
                        notes.push("Some items are synced with OneDrive: deleting them also deletes them from OneDrive and your other devices.".to_owned());
                    }
                    if w.git_repos > 0 {
                        notes.push(format!("They contain {} Git repositor{}: unpushed work exists only here.", w.git_repos, if w.git_repos == 1 { "y" } else { "ies" }));
                    }
                    if w.no_recycle_bin {
                        notes.push("Some are on a USB drive or network share: Windows may delete them permanently (it asks first).".to_owned());
                    }
                    if w.large {
                        notes.push("This is a lot of data. Windows may offer to delete items permanently if they're too big for the Recycle Bin.".to_owned());
                    }
                    if !w.apps_running.is_empty() {
                        notes.push(format!("{} is running and uses some of these files. Close it first.", w.apps_running.join(", ")));
                    }
                    for n in &notes {
                        egui::Frame::new().fill(theme::WARN.gamma_multiply(if t.dark { 0.22 } else { 0.14 })).corner_radius(CornerRadius::same(4)).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.add(egui::Label::new(RichText::new(n).color(t.text)).wrap());
                        });
                        ui.add_space(4.0);
                    }
                    if !notes.is_empty() {
                        ui.checkbox(&mut c.ack, "I understand. Move them to the Recycle Bin.");
                    }
                }
                Dialog::Dupes(dv) => {
                    ui.label(RichText::new("Find duplicate files").font(theme::semibold(16.0)));
                    ui.add_space(4.0);
                    // Results arrived?
                    if let Some((_, _, rx)) = &dv.running
                        && let Ok(g) = rx.try_recv()
                    {
                        dv.groups = Some(g);
                        dv.running = None;
                    }
                    if let Some((prog, cancel, _)) = &dv.running {
                        let read = prog.read.load(Ordering::Relaxed);
                        let total = prog.total.load(Ordering::Relaxed).max(1);
                        ui.label(RichText::new(format!("Comparing files… {} of about {} read", fmt_bytes(read), fmt_bytes(total))).color(t.muted));
                        ui.add(egui::ProgressBar::new((read as f32 / total as f32).min(1.0)).desired_width(ui.available_width()));
                        ui.ctx().request_repaint_after(Duration::from_millis(100));
                        if ui.button("Cancel").clicked() {
                            cancel.store(true, Ordering::Relaxed);
                        }
                    } else if let Some(groups) = &dv.groups {
                        let wasted: u64 = groups.iter().map(|g| g.wasted()).sum();
                        ui.label(format!("{} {} of identical files · {} could be freed by keeping one copy of each", fmt_count(groups.len() as u64), if groups.len() == 1 { "set" } else { "sets" }, fmt_bytes(wasted)));
                        ui.add_space(6.0);
                        egui::ScrollArea::vertical().id_salt("dupes").max_height(380.0).show(ui, |ui| {
                            for g in groups.iter().take(300) {
                                ui.label(RichText::new(format!("{} × {}  ·  {} extra", g.files.len(), fmt_bytes(g.size), fmt_bytes(g.wasted()))).font(theme::semibold(12.5)));
                                for (id, path) in &g.files {
                                    let r = ui.add(egui::Label::new(RichText::new(format!("    {}", visible(path))).color(t.muted)).truncate().sense(Sense::click()));
                                    if r.on_hover_text("Click to show it in the tree").clicked() {
                                        jump_node = Some(*id);
                                    }
                                }
                                ui.add_space(4.0);
                            }
                        });
                        if !groups.is_empty() {
                            ui.add_space(6.0);
                            ui.label(RichText::new("\"Mark extra copies\" keeps the first file of each set (the original-looking one: not named \"copy\" or \"(1)\", not in a backup, old or Downloads folder) and marks the others. Then press Delete to review them in one confirmation.").color(t.muted).size(11.5));
                            if ui.button("Mark extra copies").clicked() {
                                dupes_mark = Some(groups.iter().flat_map(|g| g.files.iter().skip(1).map(|f| f.0)).collect());
                            }
                        }
                    } else {
                        ui.add(egui::Label::new(RichText::new("This is the only part of DiskRoot that reads file contents: it compares files of the same size, first by their beginning and end, then in full. Nothing leaves your PC. Online-only cloud files are skipped (reading them would download them), and hardlinks (one file with two names) aren't counted as copies.").color(t.muted)).wrap());
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label("Only files of at least");
                            ui.add(egui::DragValue::new(&mut dv.min_mb).range(1..=100_000).suffix(" MB"));
                        });
                        ui.add_space(6.0);
                        if ui.add(egui::Button::new(RichText::new("Start").color(t.accent_ink)).fill(t.accent)).clicked() {
                            dupes_start = true;
                        }
                    }
                }
                Dialog::Files(fv) => {
                    ui.label(RichText::new("Largest files & file types").font(theme::semibold(16.0)));
                    ui.add_space(6.0);
                    ui.columns(2, |cols| {
                        let ui = &mut cols[0];
                        ui.label(RichText::new("Largest files").font(theme::semibold(13.0)));
                        let r = ui.add(TextEdit::singleline(&mut fv.query).hint_text("Search file names").desired_width(f32::INFINITY));
                        if r.changed() {
                            requery = Some(fv.query.clone());
                        }
                        egui::ScrollArea::vertical().id_salt("top-files").max_height(380.0).show(ui, |ui| {
                            if fv.top.is_empty() {
                                ui.label(RichText::new("No matching files.").color(t.faint));
                            }
                            for f in &fv.top {
                                ui.horizontal(|ui| {
                                    ui.add_sized(vec2(70.0, 18.0), egui::Label::new(RichText::new(fmt_bytes(f.on_disk)).color(t.muted)));
                                    let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                                    ui.painter().rect_filled(dot, CornerRadius::same(2), theme::cat_color(f.cat));
                                    let r = ui.add(egui::Label::new(visible(&f.name)).truncate().sense(Sense::click()));
                                    if r.on_hover_text(format!("{}\nModified {}\nClick to show it in the tree", visible(&f.path), diskroot_session::age_text(diskroot_session::today_days(), f.mtime))).clicked() {
                                        jump_node = Some(f.id);
                                    }
                                });
                            }
                        });
                        let ui = &mut cols[1];
                        ui.label(RichText::new("File types").font(theme::semibold(13.0)));
                        egui::ScrollArea::vertical().id_salt("file-types").max_height(410.0).show(ui, |ui| {
                            egui::Grid::new("types").num_columns(3).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                                for h in ["Type", "Size", "Files"] {
                                    ui.label(RichText::new(h).color(t.muted).size(12.0));
                                }
                                ui.end_row();
                                for ty in &fv.types {
                                    ui.label(if ty.ext.is_empty() { "(no extension)".to_owned() } else { format!(".{}", visible(&ty.ext)) });
                                    ui.label(fmt_bytes(ty.bytes));
                                    ui.label(RichText::new(fmt_count(ty.files)).color(t.muted));
                                    ui.end_row();
                                }
                            });
                        });
                    });
                }
                Dialog::Rules => {
                    let st = &self.rules_status;
                    ui.label(RichText::new("Custom rules").font(theme::semibold(16.0)));
                    ui.add_space(4.0);
                    ui.add(egui::Label::new(RichText::new("Add your own categories for folders and file types, e.g. \"Projects/*/out\" as Dev stuff > My build output. Your rules can't change what DiskRoot protects: they never apply where a built-in rule matches or inside protected places like C:\\Windows, and their safety label stays \"Your rule\".").color(t.muted)).wrap());
                    ui.add_space(8.0);
                    let (mark, col, text) = match (&st.error, st.found) {
                        (Some(e), _) => ("!", theme::WARN, format!("Not loaded: {e}")),
                        (None, true) => ("✔", t.accent, format!("{} of your own rules loaded", st.user_rules)),
                        (None, false) => ("·", t.muted, "No rules file yet. \"Edit rules\" creates one with an example.".to_owned()),
                    };
                    ui.label(RichText::new(format!("{mark}  {text}")).color(col).font(theme::semibold(13.0)));
                    if let Some(p) = &st.path {
                        ui.label(RichText::new(p.display().to_string()).monospace().size(11.0).color(t.faint));
                    }
                }
                Dialog::Compress(c) => {
                    ui.label(RichText::new("Compress").font(theme::semibold(16.0)));
                    ui.label(RichText::new(visible(&c.path)).monospace().color(t.muted));
                    ui.add_space(6.0);
                    let p = &c.plan;
                    if p.files.is_empty() {
                        ui.label("Nothing here can be compressed further.");
                    } else {
                        ui.label(format!("{} in {} files can be compressed.", fmt_bytes(p.bytes), fmt_count(p.files.len() as u64)));
                    }
                    for (why, (n, b)) in [
                        ("already compressed, sparse or online-only", p.skipped_already),
                        ("photos, videos, archives and other packed formats", p.skipped_formats),
                        ("too small to gain anything", p.skipped_tiny),
                    ] {
                        if n > 0 {
                            ui.label(RichText::new(format!("Left alone: {} files ({}), {why}", fmt_count(n), fmt_bytes(b))).color(t.faint).size(12.0));
                        }
                    }
                    if p.protected_inside > 0 {
                        ui.label(RichText::new(format!("Left alone: {} protected places inside", p.protected_inside)).color(t.faint).size(12.0));
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut c.level, Level::Fast, "Fast (recommended)");
                        ui.radio_value(&mut c.level, Level::Max, "Maximum");
                    });
                    let hint = match c.level {
                        Level::Fast => "3-15 times faster; usually saves 70-95% of what Maximum saves (most on data files).",
                        Level::Max => "The most savings, 3-15 times slower. Worth it mainly for text, code and programs you rarely change.",
                    };
                    ui.add(egui::Label::new(RichText::new(format!("{hint} Rough estimate: about {} saved (the real result depends on the data).", fmt_bytes(p.estimate(c.level)))).color(t.muted)).wrap());
                    // Compressing rewrites the files once: say what that costs an SSD.
                    let rewritten = p.bytes.saturating_sub(p.estimate(c.level));
                    ui.add(egui::Label::new(RichText::new(format!("SSD wear: compressing writes about {} once (the compressed copy of these files). Later reads cost nothing extra.", fmt_bytes(rewritten))).color(t.faint).size(12.0)).wrap());
                    ui.add_space(6.0);
                    let undo_text = if c.undo.files.is_empty() {
                        "Uncompress restores everything later (nothing here is compressed yet).".to_owned()
                    } else {
                        format!("{} files here are compressed now. Uncompress restores them; it needs {} free.", fmt_count(c.undo.files.len() as u64), fmt_bytes(c.undo.needs_free))
                    };
                    facts(ui, &t, &[
                        ("How", "Windows' own file compression, run by DiskRoot on several files at once. Files keep their place, names and dates, and open as usual: Windows unpacks them as they're read."),
                        ("Safe", "Each file is opened read-only and locked against changes while it's packed; files another program is writing are skipped. Nothing is deleted, moved or copied."),
                        ("Best for", "Programs, games, code, logs and documents you mostly read. Photos, videos, archives and most model weights barely shrink; files that look random are skipped."),
                        ("Note", "A file that's changed later is stored uncompressed again. Reading compressed files costs a little CPU."),
                        ("Undo", undo_text.as_str()),
                    ]);
                }
                Dialog::Similar(f) => {
                    ui.label(RichText::new("Similar folders").font(theme::semibold(16.0)));
                    ui.add(egui::Label::new(RichText::new(format!(
                        "{} folders looked at in {:.2} s. Likely copies have the same size, file count and names inside (contents aren't compared). Similar names share a name once case, versions and copy marks are ignored. Click a folder to show it in the tree.",
                        fmt_count(f.folders),
                        f.secs
                    )).color(t.muted)).wrap());
                    ui.add_space(6.0);
                    let member_rows = |ui: &mut Ui, g: &similar::Group, jump_node: &mut Option<u32>| {
                        for m in &g.members {
                            ui.horizontal(|ui| {
                                ui.add_sized(vec2(70.0, 18.0), egui::Label::new(RichText::new(fmt_bytes(m.bytes)).color(t.muted)));
                                let age = if m.mtime > 0 { diskroot_session::age_text(diskroot_session::today_days(), m.mtime) } else { String::new() };
                                ui.add_sized(vec2(90.0, 18.0), egui::Label::new(RichText::new(age).color(t.faint).size(11.5)));
                                let r = ui.add(egui::Label::new(visible(&m.path)).truncate().sense(Sense::click()));
                                if r.on_hover_text(format!("{} · {}\nClick to show it in the tree", visible(&m.path), fmt_files(u64::from(m.files)))).clicked() {
                                    *jump_node = Some(m.node);
                                }
                            });
                        }
                    };
                    egui::ScrollArea::vertical().id_salt("similar").max_height(420.0).show(ui, |ui| {
                        let freeable: u64 = f.copies.iter().map(|g| g.reclaim).sum();
                        ui.label(RichText::new(format!("Likely copies ({}) · up to {} could be freed", f.copies.len(), fmt_bytes(freeable))).font(theme::semibold(13.5)));
                        if f.copies.is_empty() {
                            ui.label(RichText::new("None found.").color(t.faint));
                        }
                        for (gi, g) in f.copies.iter().enumerate() {
                            egui::CollapsingHeader::new(format!("{} · {} copies · {} · {} could be freed", visible(&g.label), g.members.len(), fmt_bytes(g.bytes), fmt_bytes(g.reclaim)))
                                .id_salt(("copies", gi))
                                .show(ui, |ui| member_rows(ui, g, &mut jump_node));
                        }
                        ui.add_space(8.0);
                        ui.label(RichText::new(format!("Similar names ({})", f.names.len())).font(theme::semibold(13.5)));
                        if f.names.is_empty() {
                            ui.label(RichText::new("None found.").color(t.faint));
                        }
                        for (gi, g) in f.names.iter().enumerate() {
                            egui::CollapsingHeader::new(format!("{}… · {} folders · {}", visible(&g.label), g.members.len(), fmt_bytes(g.bytes)))
                                .id_salt(("names", gi))
                                .show(ui, |ui| member_rows(ui, g, &mut jump_node));
                        }
                    });
                    ui.add_space(4.0);
                    ui.add(egui::Label::new(RichText::new("DiskRoot never deletes from here. Check what a copy is before removing it: two folders can look alike and still differ inside.").color(t.muted).size(11.5)).wrap());
                }
                Dialog::Savings(v) => {
                    ui.label(RichText::new("Ways to save space").font(theme::semibold(16.0)));
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut v.tab, 0, format!("Compress ({})", v.sugg.items.len()));
                        ui.selectable_value(&mut v.tab, 1, format!("Other ways ({})", v.tips.len()));
                    });
                    ui.separator();
                    let today = diskroot_session::today_days();
                    if v.tab == 0 {
                        if let Some(why) = &v.sugg.not_possible {
                            ui.add(egui::Label::new(RichText::new(why.as_str()).color(t.muted)).wrap());
                        } else {
                            ui.horizontal(|ui| {
                                ui.label("Not changed for at least");
                                let before = v.min_days;
                                for (d, label) in [(30u16, "1 month"), (90, "3 months"), (180, "6 months"), (365, "1 year")] {
                                    ui.radio_value(&mut v.min_days, d, label);
                                }
                                if before != v.min_days
                                    && let Some(s) = &self.scanned
                                {
                                    let (s, days) = (s.clone(), v.min_days);
                                    let (tx, rx) = mpsc::channel();
                                    std::thread::spawn(move || {
                                        let _ = tx.send(s.compress_suggestions(days, SAVINGS_MIN, 200));
                                    });
                                    v.pending = Some(rx);
                                }
                                if v.pending.is_some() {
                                    ui.label(RichText::new("Updating…").color(t.faint));
                                }
                            });
                            if !v.sugg.has_arrival_dates {
                                ui.label(RichText::new("This scan was saved by an older DiskRoot and has no arrival dates, so downloaded files may look older than they are. Press Scan for exact ages.").color(theme::WARN).size(12.0));
                            }
                            let n = v.sugg.items.len();
                            let summary = if n == 0 {
                                "Nothing found: no folder or file that has been unchanged this long would save 100 MB or more. Try a shorter time.".to_owned()
                            } else {
                                format!(
                                    "{} could save about {} with Fast, {} with Maximum. Rough estimates: the real result depends on the data. Biggest saving first; click a name to show it in the tree.",
                                    if n == 1 { "1 place".to_owned() } else { format!("{n} places") },
                                    fmt_bytes(v.sugg.total(Level::Fast)),
                                    fmt_bytes(v.sugg.total(Level::Max))
                                )
                            };
                            ui.add(egui::Label::new(RichText::new(summary).color(t.muted)).wrap());
                            ui.add_space(4.0);
                            egui::ScrollArea::vertical().id_salt("savings-compress").max_height(360.0).show(ui, |ui| {
                                for c in &v.sugg.items {
                                    ui.horizontal(|ui| {
                                        let fast = c.estimate(Level::Fast);
                                        ui.add_sized(vec2(78.0, 18.0), egui::Label::new(RichText::new(format!("~{}", fmt_bytes(fast))).color(t.accent)));
                                        ui.add_sized(vec2(70.0, 18.0), egui::Label::new(RichText::new(fmt_bytes(c.on_disk)).color(t.muted)));
                                        ui.add_sized(vec2(90.0, 18.0), egui::Label::new(RichText::new(diskroot_session::age_text(today, c.mtime)).color(t.faint).size(11.5)));
                                        let level = c.suggested(today);
                                        let w = (ui.available_width() - 100.0).max(80.0);
                                        ui.allocate_ui_with_layout(vec2(w, 18.0), Layout::left_to_right(Align::Center), |ui| {
                                            ui.set_min_width(w);
                                            let r = ui.add(egui::Label::new(visible(&c.path)).truncate().sense(Sense::click()));
                                            let hover = format!(
                                                "{}\n{} on disk, {} of it can be packed\nAbout {} saved with Fast, {} with Maximum (suggested: {})\nNewest file changed or added {}\n\nClick to show it in the tree",
                                                visible(&c.path),
                                                fmt_bytes(c.on_disk),
                                                fmt_bytes(c.packable),
                                                fmt_bytes(fast),
                                                fmt_bytes(c.estimate(Level::Max)),
                                                level.label(),
                                                diskroot_session::date_text(c.mtime)
                                            );
                                            if r.on_hover_text(hover).clicked() {
                                                jump_node = Some(c.node);
                                            }
                                        });
                                        let tip = format!("Opens the Compress dialog with {} chosen", level.label());
                                        if ui.small_button("Compress…").on_hover_text(tip).clicked() {
                                            savings_go = Some(SavingsGo::Compress(c.node, level));
                                        }
                                    });
                                }
                            });
                            ui.add_space(6.0);
                            ui.add(egui::Label::new(RichText::new("Age: when the newest file inside last changed or arrived on this PC (a download keeps its author's old date, so the arrival date counts too). Windows doesn't reliably record when a file was last opened, so this means \"not changed since\", not \"not used since\". Compressed files still open as usual. Zip or 7z archives save more, but the files can't be opened until unpacked, so DiskRoot doesn't make them.").color(t.muted).size(11.5)).wrap());
                        }
                    } else {
                        ui.add(egui::Label::new(RichText::new("Biggest first, from this scan. DiskRoot doesn't delete anything from here: each way says how, or opens the right place.").color(t.muted)).wrap());
                        ui.add_space(4.0);
                        egui::ScrollArea::vertical().id_salt("savings-tips").max_height(400.0).show(ui, |ui| {
                            for (ti, tip) in v.tips.iter().enumerate() {
                                let size = tip.bytes.map_or_else(|| "size unknown".to_owned(), fmt_bytes);
                                egui::CollapsingHeader::new(format!("{size} · {}", tip.title)).id_salt(("tip", ti)).default_open(ti == 0).show(ui, |ui| {
                                    ui.add(egui::Label::new(RichText::new(tip.how.as_str()).color(t.muted)).wrap());
                                    for p in tip.places.iter().take(30) {
                                        ui.horizontal(|ui| {
                                            ui.add_sized(vec2(70.0, 18.0), egui::Label::new(RichText::new(fmt_bytes(p.bytes)).color(t.muted)));
                                            let age = if p.mtime > 0 { diskroot_session::age_text(today, p.mtime) } else { String::new() };
                                            ui.add_sized(vec2(90.0, 18.0), egui::Label::new(RichText::new(age).color(t.faint).size(11.5)));
                                            let w = (ui.available_width() - if p.vhdx { 90.0 } else { 0.0 }).max(80.0);
                                            ui.allocate_ui_with_layout(vec2(w, 18.0), Layout::left_to_right(Align::Center), |ui| {
                                                ui.set_min_width(w);
                                                let r = ui.add(egui::Label::new(visible(&p.path)).truncate().sense(Sense::click()));
                                                let hover = match &p.note {
                                                    Some(n) => format!("{}\n\n{n}\n\nClick to show it in the tree", visible(&p.path)),
                                                    None => format!("{}\n\nClick to show it in the tree", visible(&p.path)),
                                                };
                                                if r.on_hover_text(hover).clicked() {
                                                    jump_node = Some(p.node);
                                                }
                                            });
                                            if p.vhdx && ui.small_button("Compact…").clicked() {
                                                savings_go = Some(SavingsGo::Vhdx(p.node));
                                            }
                                        });
                                    }
                                    if tip.places.len() > 30 {
                                        ui.label(RichText::new(format!("…and {} smaller places", tip.places.len() - 30)).color(t.faint).size(11.5));
                                    }
                                    let marks: Vec<u32> = tip.places.iter().filter(|p| p.markable).map(|p| p.node).collect();
                                    if tip.button.is_some() || !marks.is_empty() {
                                        ui.horizontal(|ui| {
                                            if let Some((label, act)) = tip.button
                                                && ui.button(label).clicked()
                                            {
                                                savings_go = Some(SavingsGo::Open(act));
                                            }
                                            if !marks.is_empty()
                                                && ui
                                                    .button(format!("Mark the {} safe ones for review", marks.len()))
                                                    .on_hover_text("Selects them in the tree for one delete review. Nothing is deleted until you confirm.")
                                                    .clicked()
                                            {
                                                savings_go = Some(SavingsGo::Mark(marks));
                                            }
                                        });
                                    }
                                });
                            }
                        });
                    }
                }
                Dialog::Vhdx(path, size, busy) => {
                    ui.label(RichText::new("Compact a virtual disk").font(theme::semibold(16.0)));
                    ui.label(RichText::new(visible(path)).monospace().color(t.muted));
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(RichText::new(format!("This disk file is {}. WSL and Docker disks grow but never shrink by themselves: space freed inside (deleted files, pruned images) stays in the file until it's compacted.", fmt_bytes(*size))).color(t.muted)).wrap());
                    ui.add_space(6.0);
                    facts(ui, &t, &[
                        ("1. Free space inside", "Delete what you don't need inside first: 'docker system prune' for Docker; for WSL, delete files and run 'sudo fstrim -av' in the distro."),
                        ("2. Stop it", "The disk must not be in use: shut down WSL, or quit Docker Desktop."),
                        ("3. Compact", "Windows' diskpart attaches the disk read-only and compacts it (admin). This can take minutes."),
                    ]);
                    ui.add_space(6.0);
                    let (mark, col, text) = if *busy { ("!", theme::WARN, "In use right now: stop WSL or Docker first.") } else { ("✔", t.accent, "Not in use: ready to compact.") };
                    ui.label(RichText::new(format!("{mark}  {text}")).color(col).font(theme::semibold(13.0)));
                }
                Dialog::Alerts => {
                    ui.label(RichText::new("Size alerts").font(theme::semibold(16.0)));
                    ui.add(egui::Label::new(RichText::new("DiskRoot checks these folders after every scan and, with live updates on, while it's open. It doesn't run in the background. Right-click a folder > \"Alert me if it grows by\" to add one.").color(t.muted)).wrap());
                    ui.add_space(8.0);
                    if self.settings.alerts.is_empty() {
                        ui.label(RichText::new("No alerts yet.").color(t.faint));
                    }
                    let mut remove: Option<usize> = None;
                    egui::Grid::new("alerts").num_columns(4).striped(true).spacing([14.0, 5.0]).show(ui, |ui| {
                        for (i, a) in self.settings.alerts.iter().enumerate() {
                            ui.add(egui::Label::new(visible(&a.path)).truncate());
                            ui.label(RichText::new(format!("at {}", fmt_bytes(a.baseline))).color(t.muted));
                            ui.label(format!("grows by {}", fmt_bytes(a.grow)));
                            if ui.small_button("Remove").clicked() {
                                remove = Some(i);
                            }
                            ui.end_row();
                        }
                    });
                    if let Some(i) = remove {
                        self.settings.alerts.remove(i);
                        self.settings.save();
                    }
                }
                Dialog::Tool(a) => {
                    let _ = a;
                    ui.label(RichText::new("Clean up the component store with Windows").font(theme::semibold(16.0)));
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(RichText::new("WinSxS holds Windows components, and much of its apparent size is shared with C:\\Windows itself. Deleting files there breaks Windows. The supported way to shrink it is Windows' own DISM tool:").color(t.muted)).wrap());
                    ui.add_space(4.0);
                    ui.label(RichText::new("Dism.exe /Online /Cleanup-Image /StartComponentCleanup").monospace().size(12.0));
                    ui.add_space(6.0);
                    facts(ui, &t, &[
                        ("Admin", "Windows asks for permission first."),
                        ("Time", "Usually a few minutes, sometimes longer. It runs in its own window."),
                        ("Effect", "Removes superseded component versions. Updates installed before it can no longer be uninstalled."),
                        ("DiskRoot", "Starts DISM and waits. It deletes nothing itself here."),
                    ]);
                }
                Dialog::Leftovers(list) => {
                    ui.label(RichText::new("Leftovers").font(theme::semibold(16.0)));
                    let total: u64 = list.iter().map(|l| l.on_disk).sum();
                    ui.label(RichText::new(format!("{} items · {} · biggest first. Click a name to show it in the tree.", list.len(), fmt_bytes(total))).color(t.muted));
                    ui.add_space(6.0);
                    if list.is_empty() {
                        ui.label(RichText::new("Nothing found: no installers, archives or disk images in Downloads folders, and no Windows upgrade leftovers.").color(t.faint));
                    }
                    egui::ScrollArea::vertical().id_salt("leftovers").max_height(380.0).show(ui, |ui| {
                        for l in list.iter() {
                            ui.horizontal(|ui| {
                                ui.add_sized(vec2(70.0, 18.0), egui::Label::new(RichText::new(fmt_bytes(l.on_disk)).color(t.muted)));
                                ui.allocate_ui_with_layout(vec2(160.0, 18.0), Layout::left_to_right(Align::Center), |ui| {
                                    ui.set_min_width(160.0);
                                    let kind = if l.mtime > 0 { format!("{} · {}", l.kind, diskroot_session::age_text(diskroot_session::today_days(), l.mtime)) } else { l.kind.clone() };
                                    ui.add(egui::Label::new(RichText::new(kind).color(if l.deletable && !l.hint { t.text } else { theme::WARN })).truncate());
                                });
                                let r = ui.add(egui::Label::new(visible(&l.name)).truncate().sense(Sense::click()));
                                if r.on_hover_text(format!("{}\n\n{}\n\nClick to show it in the tree", visible(&l.path), l.note)).clicked() {
                                    jump_node = Some(l.id);
                                }
                                if let Some(a) = l.action {
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui.small_button("Open Storage settings").on_hover_text(&l.note).clicked() {
                                            tool_start = Some((a, false));
                                        }
                                    });
                                }
                            });
                        }
                    });
                    let can: Vec<u32> = list.iter().filter(|l| l.deletable && !l.hint).map(|l| l.id).collect();
                    if !can.is_empty() {
                        ui.add_space(6.0);
                        ui.add(egui::Label::new(RichText::new("Downloads are yours to judge: DiskRoot can't tell whether you still need an installer or archive. Marking selects them for one review; nothing is deleted until you confirm.").color(t.muted).size(11.5)).wrap());
                        if ui.button(format!("Mark the {} downloads for review", can.len())).clicked() {
                            leftover_marks = Some(can);
                        }
                    }
                }
                Dialog::Message(title, msg) => {
                    ui.label(RichText::new(title.as_str()).font(theme::semibold(15.0)));
                    ui.add_space(4.0);
                    ui.add(egui::Label::new(msg.as_str()).wrap());
                }
                Dialog::ConfirmDelete(c) => {
                    let info = &c.info;
                    ui.label(RichText::new(format!("Move \"{}\" to the Recycle Bin?", visible(&info.name))).font(theme::semibold(15.0)));
                    ui.add_space(4.0);
                    ui.label(RichText::new(visible(&info.path)).monospace().color(t.muted));
                    ui.add_space(6.0);
                    ui.label(format!("Frees {} · {}", fmt_bytes(info.on_disk), fmt_files(u64::from(info.files))));
                    if let Some(note) = self.scanned.as_ref().and_then(|s| s.rules().info.get(info.rule as usize)).and_then(|r| r.note.clone()) {
                        ui.add_space(4.0);
                        ui.add(egui::Label::new(RichText::new(note).color(t.muted)).wrap());
                    }
                    ui.add_space(6.0);
                    if let Some(why) = &info.protected {
                        ui.label(RichText::new(format!("DiskRoot won't delete this. {why}")).color(theme::cat_color(5)));
                    } else {
                        let w = &c.warnings;
                        let warn = |ui: &mut Ui, title: &str, text: &str| {
                            egui::Frame::new()
                                .fill(theme::WARN.gamma_multiply(if t.dark { 0.22 } else { 0.14 }))
                                .corner_radius(CornerRadius::same(4))
                                .inner_margin(egui::Margin::symmetric(10, 6))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.label(RichText::new(title).font(theme::semibold(13.0)));
                                    ui.add(egui::Label::new(RichText::new(text).color(t.muted)).wrap());
                                });
                            ui.add_space(4.0);
                        };
                        if w.cloud {
                            warn(ui, "Synced with OneDrive", "Deleting it here also deletes it from OneDrive and from your other devices. If you only want space back, use OneDrive's \"Free up space\" instead.");
                        }
                        if w.git_repos > 0 {
                            let n = if w.git_repos == 1 { "a Git repository".to_owned() } else { format!("{} Git repositories", w.git_repos) };
                            warn(ui, &format!("Contains {n}"), "Commits, branches or stashes that were never pushed exist only here.");
                        }
                        if w.no_recycle_bin {
                            warn(ui, "May be deleted permanently", "This is on a USB drive or network share. Windows often has no Recycle Bin there, so it may delete it for good (it asks first). Copy anything you might need back.");
                        }
                        if w.large {
                            warn(ui, "Large delete", "This is a lot of data. Windows may say it's too big for the Recycle Bin and offer to delete it permanently. Read that prompt carefully.");
                        }
                        if !w.apps_running.is_empty() {
                            warn(ui, &format!("{} is running", w.apps_running.join(", ")), "It uses these files. Close it first, or files in use stay behind and the app may misbehave.");
                        }
                        if w.needs_ack() {
                            ui.checkbox(&mut c.ack, "I understand. Move it to the Recycle Bin.");
                        } else {
                            ui.add(egui::Label::new(RichText::new("It goes to the Recycle Bin, so you can restore it. If it's too big for the bin, Windows asks before deleting it permanently.").color(t.muted)).wrap());
                        }
                    }
                }
            }
            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                match d {
                    Dialog::ConfirmDelete(c) if c.info.protected.is_none() => {
                        let ready = c.ack || !c.warnings.needs_ack();
                        let b = egui::Button::new(RichText::new("Move to Recycle Bin").color(Color32::WHITE)).fill(theme::cat_color(5));
                        if ui.add_enabled(ready, b).clicked() {
                            confirm = Some(c.info.id);
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        return;
                    }
                    Dialog::ConfirmMany(c) => {
                        let needs = c.warnings.needs_ack();
                        let ready = !c.items.is_empty() && (c.ack || !needs);
                        let b = egui::Button::new(RichText::new(format!("Move {} to Recycle Bin", c.items.len())).color(Color32::WHITE)).fill(theme::cat_color(5));
                        if ui.add_enabled(ready, b).clicked() {
                            action = Some("delete-many");
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        return;
                    }
                    Dialog::Schedule(v) => {
                        let b = egui::Button::new(RichText::new("Save").color(t.accent_ink)).fill(t.accent).min_size(vec2(80.0, 26.0));
                        if ui.add(b).clicked() {
                            action = Some("schedule-save");
                        }
                        if ui.add_enabled(!v.s.roots.is_empty(), egui::Button::new("Scan once now")).on_hover_text("One background scan right away, like the schedule would run it").clicked() {
                            action = Some("schedule-run");
                        }
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                        return;
                    }
                    Dialog::Home(_) => {
                        let ok = egui::Button::new(RichText::new("Close").color(t.accent_ink)).fill(t.accent).min_size(vec2(80.0, 26.0));
                        if ui.add(ok).clicked() {
                            close = true;
                        }
                        if ui.button("Where every GB is…").clicked() {
                            action = Some("home-space");
                        }
                        if ui.button("Size history…").clicked() {
                            action = Some("home-history");
                        }
                        if ui.checkbox(&mut self.settings.home_at_start, "Show this when DiskRoot starts").changed() {
                            self.settings.save();
                        }
                        return;
                    }
                    Dialog::Space(r) => {
                        let ok = egui::Button::new(RichText::new("Close").color(t.accent_ink)).fill(t.accent).min_size(vec2(80.0, 26.0));
                        if ui.add(ok).clicked() {
                            close = true;
                        }
                        if r.unreadable > 0 && ui.button(format!("Show the {} unreadable folders", fmt_count(r.unreadable))).clicked() {
                            action = Some("show-unreadable");
                        }
                        if !r.fast_scan && ui.button("⚡ Fast scan (admin)").on_hover_text("Reads the drive's file table: every file, including what a folder walk can't open. Read-only.").clicked() {
                            action = Some("space-fast-scan");
                        }
                        return;
                    }
                    Dialog::Unreadable(_) => {
                        let ok = egui::Button::new(RichText::new("Close").color(t.accent_ink)).fill(t.accent).min_size(vec2(80.0, 26.0));
                        if ui.add(ok).clicked() {
                            close = true;
                        }
                        return;
                    }
                    Dialog::Writers(_) => {
                        let ok = egui::Button::new(RichText::new("Close").color(t.accent_ink)).fill(t.accent).min_size(vec2(80.0, 26.0));
                        if ui.add(ok).clicked() {
                            close = true;
                        }
                        if ui.button("Drive health…").clicked() {
                            action = Some("health-refresh");
                        }
                        return;
                    }
                    Dialog::Health(_) => {
                        let ok = egui::Button::new(RichText::new("Close").color(t.accent_ink)).fill(t.accent).min_size(vec2(80.0, 26.0));
                        if ui.add(ok).clicked() {
                            close = true;
                        }
                        if ui.button("Check again").clicked() {
                            action = Some("health-refresh");
                        }
                        return;
                    }
                    Dialog::Growth(_) => {
                        let ok = egui::Button::new(RichText::new("Close").color(t.accent_ink)).fill(t.accent).min_size(vec2(80.0, 26.0));
                        if ui.add(ok).clicked() {
                            close = true;
                        }
                        if ui.button("Scheduled scans…").on_hover_text("Background scans fill the history for you").clicked() {
                            action = Some("open-schedule");
                        }
                        if ui.button("Clear history").on_hover_text("Deletes this drive's or folder's size history (DiskRoot's own file). Remembered scans stay.").clicked() {
                            action = Some("history-clear");
                        }
                        return;
                    }
                    Dialog::Memory => {
                        let ok = egui::Button::new(RichText::new("Close").color(t.accent_ink)).fill(t.accent).min_size(vec2(80.0, 26.0));
                        if ui.add(ok).clicked() {
                            close = true;
                        }
                        if ui.button("Scheduled scans…").clicked() {
                            action = Some("open-schedule");
                        }
                        if ui.add_enabled(self.scanned.is_some() && self.job.is_none(), egui::Button::new("Free memory now")).on_hover_text("Closes the scan on screen; it stays saved and reopens in about a second").clicked() {
                            action = Some("free-memory");
                        }
                        return;
                    }
                    Dialog::Welcome => {
                        let b = egui::Button::new(RichText::new("Got it").color(t.accent_ink)).fill(t.accent).min_size(vec2(90.0, 26.0));
                        if ui.add(b).clicked() {
                            action = Some("welcome-done");
                        }
                        return;
                    }
                    Dialog::Compress(c) => {
                        let b = egui::Button::new(RichText::new("Compress").color(t.accent_ink)).fill(t.accent).min_size(vec2(90.0, 26.0));
                        if ui.add_enabled(!c.plan.files.is_empty(), b).clicked() {
                            action = Some("compress-go");
                        }
                        if ui.add_enabled(!c.undo.files.is_empty(), egui::Button::new("Uncompress")).clicked() {
                            action = Some("uncompress-go");
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        return;
                    }
                    Dialog::Vhdx(_, _, busy) => {
                        let b = egui::Button::new(RichText::new("Compact (admin)").color(t.accent_ink)).fill(t.accent).min_size(vec2(110.0, 26.0));
                        if ui.add_enabled(!*busy, b).clicked() {
                            action = Some("vhdx-go");
                        }
                        if ui.button("Check again").clicked() {
                            action = Some("vhdx-check");
                        }
                        if ui.button("Shut down WSL").on_hover_text("Runs 'wsl --shutdown': stops every WSL distro").clicked() {
                            action = Some("wsl-shutdown");
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        return;
                    }
                    Dialog::Rules => {
                        let ok = egui::Button::new(RichText::new("Close").color(t.accent_ink)).fill(t.accent).min_size(vec2(80.0, 26.0));
                        if ui.add(ok).clicked() {
                            close = true;
                        }
                        if ui.button("Reload").on_hover_text("Read the file again and re-sort the current scan").clicked() {
                            action = Some("rules-reload");
                        }
                        if ui.button("Edit rules").on_hover_text("Opens the file in Notepad (creates it with an example first)").clicked() {
                            action = Some("rules-edit");
                        }
                        return;
                    }
                    Dialog::Tool(a) => {
                        let b = egui::Button::new(RichText::new("Start DISM").color(t.accent_ink)).fill(t.accent).min_size(vec2(90.0, 26.0));
                        if ui.add(b).clicked() {
                            tool_start = Some((*a, true));
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        return;
                    }
                    Dialog::History(_) => {
                        let ok = egui::Button::new(RichText::new("Close").color(t.accent_ink)).fill(t.accent).min_size(vec2(80.0, 26.0));
                        if ui.add(ok).clicked() {
                            close = true;
                        }
                        if ui.button("Open log folder").clicked() {
                            action = Some("open-log");
                        }
                        return;
                    }
                    _ => {}
                }
                let ok = egui::Button::new(RichText::new("OK").color(t.accent_ink)).fill(t.accent).min_size(vec2(80.0, 26.0));
                if ui.add(ok).clicked() {
                    close = true;
                }
            });
        });
        if close && let Some(Dialog::Dupes(dv)) = &self.dialog && let Some((_, c, _)) = &dv.running {
            c.store(true, Ordering::Relaxed);
        }
        // Escape / click outside closes everything except the first-run notice.
        if resp.should_close() && !matches!(self.dialog, Some(Dialog::Welcome)) {
            close = true;
        }
        match action {
            Some("welcome-done") => {
                self.settings.seen_notice = true;
                self.settings.save();
                self.dialog = None;
                if std::mem::take(&mut self.scan_after_welcome) {
                    self.start_scan(false);
                }
            }
            Some("delete-many") => {
                if let Some(Dialog::ConfirmMany(c)) = self.dialog.take() {
                    self.start_delete_many(c.items);
                }
            }
            Some(a @ ("compress-go" | "uncompress-go")) => {
                if let Some(Dialog::Compress(c)) = self.dialog.take() {
                    let CompressAsk { plan, undo, level, .. } = *c;
                    if a == "compress-go" {
                        self.start_compress(plan, Some(level));
                    } else {
                        // Restored files need their full size back: check first.
                        let free = compress::free_space(&undo.root).unwrap_or(u64::MAX);
                        if free < undo.needs_free.saturating_add(1 << 30) {
                            self.dialog = Some(Dialog::Message(
                                "Not enough free space".into(),
                                format!("Uncompressing needs about {} free (plus 1 GB to spare), and the drive has {}. Free some space first.", fmt_bytes(undo.needs_free), fmt_bytes(free)),
                            ));
                        } else {
                            self.start_compress(undo, None);
                        }
                    }
                }
            }
            Some("vhdx-check") => {
                if let Some(Dialog::Vhdx(path, _, busy)) = &mut self.dialog {
                    *busy = diskroot_session::power::in_use(std::path::Path::new(path));
                }
            }
            Some("wsl-shutdown") => {
                if let Err(e) = diskroot_session::power::wsl_shutdown() {
                    self.say(e);
                }
                if let Some(Dialog::Vhdx(path, _, busy)) = &mut self.dialog {
                    *busy = diskroot_session::power::in_use(std::path::Path::new(path));
                }
            }
            Some("vhdx-go") => {
                if let Some(Dialog::Vhdx(path, _, _)) = self.dialog.take() {
                    self.run_tool_job("Compacting the virtual disk (diskpart)".into(), move || diskroot_session::power::compact_vhdx(std::path::Path::new(&path)));
                }
            }
            Some("open-schedule") => self.open_schedule(),
            Some(a @ ("schedule-save" | "schedule-run")) => {
                if let Some(Dialog::Schedule(v)) = &mut self.dialog {
                    let exe = std::env::current_exe().map_err(|e| e.to_string());
                    let r = match (a, exe) {
                        (_, Err(e)) => Err(e),
                        ("schedule-run", Ok(exe)) => schedule::save(&v.s).and_then(|()| schedule::run_now(&exe)),
                        (_, Ok(exe)) => if v.on { schedule::install(&v.s, &exe) } else { schedule::remove() },
                    };
                    v.installed = schedule::installed();
                    v.msg = Some(match (r, a) {
                        (Err(e), _) => format!("Didn't work: {e}"),
                        (Ok(()), "schedule-run") => "Scanning in the background now. Its result shows up here and in What changed when it's done.".into(),
                        (Ok(()), _) if v.on => format!("Saved. Next background scan within {}.", if v.s.every_min < 60 { format!("{} minutes", v.s.every_min) } else { format!("{} h", v.s.every_min / 60) }),
                        (Ok(()), _) => "Saved. Automatic scans are off.".into(),
                    });
                }
            }
            Some("show-unreadable") => self.open_unreadable(),
            Some("home-changes") => {
                self.dialog = self.changes.clone().map(|c| Dialog::Changes(Box::new(c)));
            }
            Some("home-savings") => {
                self.dialog = None;
                self.open_savings();
            }
            Some("home-space") => self.open_space(),
            Some("home-history") => {
                self.dialog = None;
                self.open_growth();
            }
            Some("space-fast-scan") => {
                self.dialog = None;
                self.start_scan(true);
            }
            Some("health-refresh") => {
                self.open_health();
            }
            Some("history-clear") => {
                if let Some(Dialog::Growth(v)) = self.dialog.take() {
                    history::clear(&v.root);
                    self.set_hist(None);
                    self.say(format!("Cleared the size history of {}.", visible(&v.root)));
                }
            }
            Some("free-memory") => {
                self.dialog = None;
                self.free_memory();
            }
            Some("rules-reload") => self.reload_rules(),
            Some("rules-edit") => match diskroot_session::create_example_rules() {
                Ok(p) => {
                    let notepad = diskroot_session::tools::system32("notepad.exe");
                    if let Err(e) = std::process::Command::new(notepad).arg(&p).spawn() {
                        self.say(format!("Couldn't open Notepad: {e}"));
                    }
                }
                Err(e) => self.dialog = Some(Dialog::Message("Couldn't create the rules file".into(), e)),
            },
            Some("open-log") => {
                if let Some(p) = diskroot_session::safety::log_path() {
                    let _ = diskroot_session::reveal(&p.display().to_string());
                }
            }
            _ => {}
        }
        if dupes_start
            && let (Some(s), Some(Dialog::Dupes(dv))) = (&self.scanned, &mut self.dialog)
        {
            let cands = s.duplicate_candidates(dv.min_mb << 20);
            let prog = Arc::new(diskroot_session::dupes::Progress::default());
            let cancel = Arc::new(AtomicBool::new(false));
            let (tx, rx) = mpsc::channel();
            let (p2, c2) = (Arc::clone(&prog), Arc::clone(&cancel));
            std::thread::spawn(move || {
                let _ = tx.send(diskroot_session::dupes::find(cands, &c2, &p2));
            });
            dv.running = Some((prog, cancel, rx));
        }
        if let Some(go) = savings_go {
            match go {
                SavingsGo::Compress(n, level) => self.ask_compress_at(n, level),
                SavingsGo::Vhdx(n) => self.ask_vhdx(n),
                SavingsGo::Open(TipAction::Tool(a)) => {
                    self.dialog = None;
                    self.start_tool(a, false);
                }
                SavingsGo::Open(TipAction::Similar) => self.open_similar(),
                SavingsGo::Open(TipAction::Leftovers) => self.open_leftovers(),
                SavingsGo::Open(TipAction::Dupes) => self.open_dupes(),
                SavingsGo::Mark(ids) => {
                    self.dialog = None;
                    if self.settings.by_category {
                        self.settings.by_category = false;
                        self.regroup();
                    }
                    self.marked = ids.into_iter().collect();
                    self.rows = None;
                    self.say(format!("{} places marked · press Delete to review them", self.marked.len()));
                }
            }
        }
        if let Some((a, confirmed)) = tool_start {
            self.dialog = None;
            self.start_tool(a, confirmed);
        }
        if let Some(ids) = leftover_marks {
            self.dialog = None;
            if self.settings.by_category {
                self.settings.by_category = false;
                self.regroup();
            }
            self.marked = ids.into_iter().collect();
            self.rows = None;
            self.say(format!("{} downloads marked · press Delete to review them", self.marked.len()));
        }
        if let Some(ids) = dupes_mark {
            self.dialog = None;
            if self.settings.by_category {
                self.settings.by_category = false;
                self.regroup();
            }
            self.marked = ids.into_iter().collect();
            self.say(format!("{} extra copies marked · press Delete to review and move them to the Recycle Bin", self.marked.len()));
            self.rows = None;
        }
        if let (Some(q), Some(s), Some(Dialog::Files(fv))) = (requery, &self.scanned, &mut self.dialog) {
            fv.top = s.top_files(100, &q);
        }
        if let Some(n) = jump_node {
            self.dialog = None;
            if self.settings.by_category {
                self.settings.by_category = false;
                self.regroup();
            }
            self.reveal_node(n, None);
            self.scroll_to_selected = true;
        }
        if let Some(rel) = jump {
            self.dialog = None;
            self.show_rel(&rel);
        }
        if confirm.is_some() {
            if let Some(Dialog::ConfirmDelete(c)) = self.dialog.take()
                && let Some(p) = c.pending
            {
                self.start_delete(p);
            }
        } else if close {
            self.dialog = None;
        }
    }

    /// Selects a folder by its path relative to the scan root (from "what
    /// changed"). Folders that are gone just get a message.
    fn show_rel(&mut self, rel: &str) {
        let Some(n) = self.scanned.as_ref().and_then(|s| s.find_rel(rel)) else {
            self.say(format!("\"{rel}\" isn't in the current scan (it was removed)."));
            return;
        };
        if self.settings.by_category {
            self.settings.by_category = false;
            self.regroup();
        }
        self.reveal_node(n, None);
        self.scroll_to_selected = true;
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() || self.job.is_some() || self.palette.is_some() {
            return;
        }
        let (pal, list, insp) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::CTRL, Key::K),
                i.consume_key(egui::Modifiers::CTRL, Key::B),
                i.consume_key(egui::Modifiers::CTRL, Key::J),
            )
        });
        if pal {
            self.palette = Some(Palette::default());
            return;
        }
        if list {
            self.run_cmd(ctx, Cmd::ToggleList);
        }
        if insp {
            self.run_cmd(ctx, Cmd::ToggleInspector);
        }
        let typing = ctx.egui_wants_keyboard_input();
        if !typing && self.scanned.is_some() {
            let (f, esc) = ctx.input(|i| (i.key_pressed(Key::F) && i.modifiers.is_none(), i.key_pressed(Key::Escape)));
            if f {
                self.focus = !self.focus;
            } else if esc && self.focus {
                self.focus = false;
            }
        }
        let (f5, ctrl, back, up, down, left, right, enter, del) = ctx.input(|i| {
            (
                i.key_pressed(Key::F5),
                i.modifiers.ctrl,
                i.key_pressed(Key::Backspace),
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Delete),
            )
        });
        let space = ctx.input(|i| i.key_pressed(Key::Space));
        if f5 {
            self.start_scan(ctrl);
        }
        if typing {
            return;
        }
        if back
            && let Some(k) = self.map.as_ref().and_then(|m| m.up.clone()) {
                self.go_to(k);
            }
        if !self.tree_focus {
            return;
        }
        let Some(rows) = &self.rows else { return };
        let Some(i) = self.selected.as_ref().and_then(|k| rows.iter().position(|r| &r.key == k)) else { return };
        let row_key = rows[i].key.clone();
        let expandable = rows[i].expandable;
        let open = self.expanded.contains(&row_key);
        if down && i + 1 < rows.len() && rows[i + 1].more == 0 {
            self.select(rows[i + 1].key.clone());
        } else if up && i > 0 {
            self.select(rows[i - 1].key.clone());
        } else if expandable && ((right && !open) || (left && open)) {
            self.toggle(&row_key);
        } else if left {
            let d = rows[i].depth;
            if let Some(p) = rows[..i].iter().rposition(|r| r.depth < d) {
                self.select(rows[p].key.clone());
            }
        } else if enter && expandable {
            self.toggle(&row_key);
        } else if del
            && let Key_::Node(n) = row_key {
                self.ask_delete(n);
            } else if space
            && let Key_::Node(n) = row_key
            && n != 0
        {
            // Space marks / unmarks the selected row (like Ctrl+click).
            if !self.marked.remove(&n) {
                self.marked.insert(n);
            }
        }
    }

    /// Every command for the palette: (label, shortcut, command), only the
    /// ones that can run now.
    fn commands(&self) -> Vec<(&'static str, &'static str, Cmd)> {
        let scan = self.scanned.is_some();
        let idle = self.job.is_none();
        let mut v: Vec<(&'static str, &'static str, Cmd, bool)> = vec![
            ("Scan", "F5", Cmd::Scan, idle),
            ("Fast scan (MFT, admin)", "Ctrl+F5", Cmd::FastScan, idle),
            ("Up one folder", "Backspace", Cmd::Up, self.map.as_ref().is_some_and(|m| m.up.is_some())),
            ("Home: the drive at a glance", "Ctrl+H", Cmd::Home, scan),
            ("Ways to save space", "", Cmd::SaveSpace, scan),
            ("What changed since the previous scan", "", Cmd::Changes, self.changes.is_some()),
            ("Size history", "", Cmd::Growth, self.summary.is_some()),
            ("Where every GB is", "", Cmd::Space, scan),
            ("Who's writing to my drives", "", Cmd::Writers, true),
            ("Drive health", "", Cmd::Health, true),
            ("Scheduled scans", "", Cmd::Schedule, true),
            ("Find duplicate files", "", Cmd::Dupes, scan),
            ("Largest files & file types", "", Cmd::Files, scan),
            ("Similar folders", "", Cmd::Similar, scan),
            ("Leftovers", "", Cmd::Leftovers, scan),
            ("Map colours: size (category)", "", Cmd::ModeSize, self.settings.map_mode != 0),
            ("Map colours: growth since the previous scan", "", Cmd::ModeGrowth, self.settings.map_mode != 1),
            ("Map colours: reclaimable (safe to clear / check first)", "", Cmd::ModeReclaim, self.settings.map_mode != 2),
            (if self.settings.map_25d { "Map: flat (2D)" } else { "Map: tilted (2.5D)" }, "", Cmd::Tilt, self.scanned.is_some()),
            (if self.settings.animations { "Turn off smooth transitions" } else { "Turn on smooth transitions" }, "", Cmd::Animations, true),
            ("Switch to Sunburst", "", Cmd::Sunburst, self.settings.treemap),
            ("Switch to Treemap", "", Cmd::Treemap, !self.settings.treemap),
            ("Group by location", "", Cmd::ByLocation, self.settings.by_category),
            ("Group by category", "", Cmd::ByCategory, !self.settings.by_category),
            ("Colour by category", "", Cmd::ColorCategory, self.settings.color_by_age),
            ("Colour by age (last modified)", "", Cmd::ColorAge, !self.settings.color_by_age),
            (if self.focus { "Leave focus" } else { "Focus on the map" }, "F", Cmd::Focus, scan),
            (if self.settings.hide_list { "Show the folder list" } else { "Minimize the folder list" }, "Ctrl+B", Cmd::ToggleList, true),
            (if self.settings.hide_inspector { "Show the Inspector" } else { "Minimize the Inspector" }, "Ctrl+J", Cmd::ToggleInspector, true),
            (if self.settings.classic { "Look: Studio" } else { "Look: Classic" }, "", Cmd::Look, true),
            ("Light theme", "", Cmd::Light, self.settings.dark),
            ("Dark theme", "", Cmd::Dark, !self.settings.dark),
            ("Export to CSV", "", Cmd::Export, scan && idle),
            ("Custom rules", "", Cmd::Rules, true),
            ("Size alerts", "", Cmd::Alerts, true),
            ("Deletion history", "", Cmd::DeletionHistory, true),
            ("Memory and CPU", "", Cmd::Memory, true),
            ("Free memory", "", Cmd::FreeMemory, scan),
            ("Keyboard shortcuts", "", Cmd::Shortcuts, true),
            ("Security & privacy", "", Cmd::Security, true),
            ("About DiskRoot", "", Cmd::About, true),
        ];
        v.retain(|c| c.3);
        v.into_iter().map(|(l, k, c, _)| (l, k, c)).collect()
    }

    fn run_cmd(&mut self, ctx: &egui::Context, c: Cmd) {
        match c {
            Cmd::Scan => self.start_scan(false),
            Cmd::FastScan => self.start_scan(true),
            Cmd::Up => {
                if let Some(k) = self.map.as_ref().and_then(|m| m.up.clone()) {
                    self.go_to(k);
                }
            }
            Cmd::Home => self.open_home(),
            Cmd::SaveSpace => self.open_savings(),
            Cmd::Changes => {
                if let Some(ch) = &self.changes {
                    self.dialog = Some(Dialog::Changes(Box::new(ch.clone())));
                }
            }
            Cmd::Growth => self.open_growth(),
            Cmd::Space => self.open_space(),
            Cmd::Writers => self.dialog = Some(Dialog::Writers(Box::new(WritersView::open()))),
            Cmd::Health => self.open_health(),
            Cmd::Schedule => self.open_schedule(),
            Cmd::Dupes => self.open_dupes(),
            Cmd::Files => self.open_files(),
            Cmd::Similar => self.open_similar(),
            Cmd::Leftovers => self.open_leftovers(),
            Cmd::Sunburst | Cmd::Treemap => {
                self.settings.treemap = c == Cmd::Treemap;
                self.settings.save();
                if let Some(m) = &mut self.map {
                    m.cached = None;
                }
            }
            Cmd::ModeSize | Cmd::ModeGrowth | Cmd::ModeReclaim => {
                self.settings.map_mode = match c {
                    Cmd::ModeGrowth => 1,
                    Cmd::ModeReclaim => 2,
                    _ => 0,
                };
                self.settings.save();
                self.map = None;
            }
            Cmd::Look => {
                self.settings.classic = !self.settings.classic;
                self.settings.save();
                theme::set_classic(self.settings.classic);
                theme::apply(ctx, if self.settings.dark { Theme::Dark } else { Theme::Light });
                self.map = None;
                self.rows = None;
            }
            Cmd::Tilt => {
                self.settings.map_25d = !self.settings.map_25d;
                self.settings.save();
                if let Some(m) = &mut self.map {
                    // Rise up from the ground when switching on.
                    m.born = (self.settings.animations && self.settings.map_25d).then(Instant::now);
                }
            }
            Cmd::Animations => {
                self.settings.animations = !self.settings.animations;
                self.settings.save();
            }
            Cmd::ByLocation | Cmd::ByCategory => {
                self.settings.by_category = c == Cmd::ByCategory;
                self.regroup();
            }
            Cmd::ColorCategory | Cmd::ColorAge => {
                self.settings.color_by_age = c == Cmd::ColorAge;
                self.settings.save();
                self.map = None;
                self.rows = None;
            }
            Cmd::Focus => self.focus = !self.focus && self.scanned.is_some(),
            Cmd::ToggleList => {
                self.settings.hide_list = !self.settings.hide_list;
                self.settings.save();
            }
            Cmd::ToggleInspector => {
                self.settings.hide_inspector = !self.settings.hide_inspector;
                self.settings.save();
            }
            Cmd::Light | Cmd::Dark => {
                self.settings.dark = c == Cmd::Dark;
                theme::apply(ctx, if self.settings.dark { Theme::Dark } else { Theme::Light });
                self.settings.save();
                if let Some(m) = &mut self.map {
                    m.cached = None;
                }
            }
            Cmd::Export => self.start_export(),
            Cmd::Rules => self.dialog = Some(Dialog::Rules),
            Cmd::Alerts => self.dialog = Some(Dialog::Alerts),
            Cmd::DeletionHistory => self.open_history(),
            Cmd::Memory => self.dialog = Some(Dialog::Memory),
            Cmd::FreeMemory => self.free_memory(),
            Cmd::Shortcuts => {
                // Same text as Help > Keyboard shortcuts.
                self.dialog = Some(Dialog::Message(
                    "Keyboard shortcuts".into(),
                    [
                        "Ctrl+K · Command palette: every command and every folder, one search",
                        "F · Focus on the map (Esc to leave)",
                        "Ctrl+B · Show or minimize the folder list",
                        "Ctrl+J · Show or minimize the Inspector",
                        "F5 · Scan (no admin) · Ctrl+F5 · Fast scan (admin)",
                        "Ctrl+H · Home · Ctrl+F · Filter by name",
                        "↑ / ↓ / → / ← / Enter · Move in the tree, open and close folders",
                        "Backspace · Up one folder",
                        "Space or Ctrl+click · Mark / unmark · Delete · Move to the Recycle Bin (always asks first)",
                        "Esc · Close a window",
                    ]
                    .join("\n"),
                ));
            }
            Cmd::Security => self.open_security(),
            Cmd::About => self.dialog = Some(Dialog::About),
        }
    }

    /// The command palette window (Ctrl+K).
    fn palette_ui(&mut self, ctx: &egui::Context) {
        let Some(mut pal) = self.palette.take() else { return };
        let t = self.tokens();
        // Folder and file matches: worked out again only when the words change.
        let q = pal.query.trim().to_lowercase();
        if q != pal.hits_for {
            pal.hits = match &self.scanned {
                Some(s) if q.chars().count() >= 2 => s.find_names(&q, 40),
                _ => Vec::new(),
            };
            pal.hits_for = q.clone();
        }
        let words: Vec<&str> = q.split_whitespace().collect();
        let cmds: Vec<_> = self.commands().into_iter().filter(|(l, _, _)| {
            let l = l.to_lowercase();
            words.iter().all(|w| l.contains(w))
        }).collect();
        let total = cmds.len() + pal.hits.len();
        let (down, up, enter) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
            )
        });
        if down && pal.sel + 1 < total {
            pal.sel += 1;
        }
        if up && pal.sel > 0 {
            pal.sel -= 1;
        }
        pal.sel = pal.sel.min(total.saturating_sub(1));
        let mut pick: Option<Pick> = None;
        if enter && total > 0 {
            pick = Some(if pal.sel < cmds.len() { Pick::Cmd(cmds[pal.sel].2) } else { Pick::Node(pal.hits[pal.sel - cmds.len()].0.id) });
        }
        let resp = Modal::new(egui::Id::unique("diskroot-palette")).show(ctx, |ui| {
            ui.set_width(560.0);
            let r = ui.add(TextEdit::singleline(&mut pal.query).desired_width(f32::INFINITY).hint_text("Type a command or a folder name…"));
            if !pal.focused {
                r.request_focus();
                pal.focused = true;
            }
            if r.changed() {
                pal.sel = 0;
            }
            ui.add_space(6.0);
            let mut sel_rect = None;
            ScrollArea::vertical().max_height(380.0).auto_shrink([false, true]).show(ui, |ui| {
                let row = |ui: &mut Ui, selected: bool, left: &str, right: String, group: &str| {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, format!("{group}: {left}")));
                    let p = ui.painter();
                    if selected || resp.hovered() {
                        p.rect_filled(rect, CornerRadius::same(4), if selected { t.sel } else { t.hover });
                    }
                    p.text(pos2(rect.min.x + 8.0, rect.center().y), Align2::LEFT_CENTER, group, theme::regular(10.5), t.faint);
                    let name = map::fit(p, left, 13.0, rect.width() - 190.0).unwrap_or_default();
                    p.text(pos2(rect.min.x + 62.0, rect.center().y), Align2::LEFT_CENTER, name, theme::regular(13.0), t.text);
                    p.text(pos2(rect.max.x - 8.0, rect.center().y), Align2::RIGHT_CENTER, right, theme::regular(11.5), t.muted);
                    (resp, rect)
                };
                for (i, (label, key, c)) in cmds.iter().enumerate() {
                    let (r, rect) = row(ui, i == pal.sel, label, (*key).to_owned(), "Command");
                    if i == pal.sel {
                        sel_rect = Some(rect);
                    }
                    if r.clicked() {
                        pick = Some(Pick::Cmd(*c));
                    }
                }
                for (j, (f, dir)) in pal.hits.iter().enumerate() {
                    let i = cmds.len() + j;
                    let (r, rect) = row(ui, i == pal.sel, &f.name, fmt_bytes(f.on_disk), if *dir { "Folder" } else { "File" });
                    let r = r.on_hover_text(visible(&f.path));
                    if i == pal.sel {
                        sel_rect = Some(rect);
                    }
                    if r.clicked() {
                        pick = Some(Pick::Node(f.id));
                    }
                }
                if let Some(rect) = sel_rect.filter(|_| down || up) {
                    ui.scroll_to_rect(rect, None);
                }
                if total == 0 {
                    ui.label(RichText::new(if self.scanned.is_none() && words.len() == 1 { "No command matches. Scan a drive to search folders too." } else { "Nothing matches." }).color(t.muted));
                }
            });
            ui.add_space(4.0);
            ui.label(RichText::new("↑ ↓ to move · Enter to run · Esc to close").color(t.faint).size(11.0));
        });
        let close = resp.should_close() || pick.is_some();
        if !close {
            self.palette = Some(pal);
        }
        match pick {
            Some(Pick::Cmd(c)) => self.run_cmd(ctx, c),
            Some(Pick::Node(n)) => {
                self.reveal_node(n, None);
                self.tree_focus = true;
            }
            None => {}
        }
    }

    /// The Inspector's title bar: what's selected, its safety label, quick
    /// actions, and minimize / show.
    fn inspector_bar(&mut self, ui: &mut Ui) {
        let t = self.tokens();
        // What's selected: (name, size, colour, node, safety label + colour).
        type Sel = (String, u64, u8, Option<u32>, &'static str, Color32);
        let mut info: Option<Sel> = None;
        // Same rules as the buttons in the details below: (can compress, can recycle).
        let mut can = (false, false);
        if let (Some(s), Some(key)) = (&self.scanned, &self.selected) {
            let row = self.rows.as_ref().and_then(|r| r.iter().find(|r| &r.key == key));
            let (name, bytes, cat) = match (key, row) {
                (_, Some(r)) => (r.name.clone(), r.bytes, r.cat),
                (Key_::Node(n), None) => s.info(*n).map(|i| (i.name, i.on_disk, i.cat)).unwrap_or_default(),
                _ => (String::new(), 0, 0),
            };
            let (node, label, color) = match key {
                Key_::Node(n) if *n != 0 => {
                    let rule = s.info(*n).ok().and_then(|i| s.rules().info.get(i.rule as usize).cloned());
                    let (l, c) = match s.safety(*n) {
                        None if rule.as_ref().is_some_and(|r| r.user) => ("Your rule", t.muted),
                        Some(Safety::Green) => ("Safe to clear", theme::cat_color(2)),
                        Some(Safety::Yellow) => ("Check first", theme::cat_color(6)),
                        Some(Safety::Red) => ("Don't delete by hand", theme::cat_color(5)),
                        None => ("Unknown", t.muted),
                    };
                    if let Ok(i) = s.info(*n) {
                        let cloud = s.is_cloud(*n) || diskroot_session::safety::in_cloud_folder(std::path::Path::new(&i.path));
                        can = (i.dir && i.protected.is_none() && i.parent.is_some() && !cloud, i.protected.is_none() && l != "Don't delete by hand");
                    }
                    (Some(*n), l, c)
                }
                _ => (None, "", t.muted),
            };
            info = Some((name, bytes, cat, node, label, color));
        }
        let mut act: Option<(&'static str, u32)> = None;
        ui.horizontal(|ui| {
            ui.set_height(28.0);
            // Buttons first (right side), so they always get their room; the
            // name is shortened instead when the Inspector is narrow.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let hide = if self.settings.hide_inspector { "Show ▴" } else { "Minimize ▾" };
                if ui.small_button(hide).on_hover_text("Show or minimize the Inspector (Ctrl+J)").clicked() {
                    act = Some(("toggle", 0));
                }
                if let Some((_, _, _, Some(n), _, _)) = &info {
                    let busy = self.job.is_some();
                    if ui.add_enabled(!busy && can.1, egui::Button::new("Recycle Bin…").small()).on_hover_text("Move to the Recycle Bin (asks first, re-checks on disk)").clicked() {
                        act = Some(("delete", *n));
                    }
                    if can.0 && ui.add_enabled(!busy, egui::Button::new("Compress…").small()).on_hover_text("Windows' own file compression: same files, less space, undoable").clicked() {
                        act = Some(("compress", *n));
                    }
                }
                if !self.marked.is_empty() {
                    ui.label(RichText::new(format!("{} marked", self.marked.len())).color(theme::WARN).size(11.5));
                }
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    ui.label(RichText::new("INSPECTOR").color(t.faint).size(10.5));
                    ui.add_space(6.0);
                    let Some((name, bytes, cat, _, label, color)) = &info else { return };
                    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                    ui.painter().rect_filled(rect, CornerRadius::same(2), theme::cat_color(*cat));
                    let size = fmt_bytes(*bytes);
                    let w = |s: &str, f: egui::FontId| ui.painter().layout_no_wrap(s.to_owned(), f, t.text).size().x;
                    let rest = w(&size, theme::regular(13.0)) + if label.is_empty() { 0.0 } else { w(label, theme::regular(11.5)) + 6.0 } + 8.0;
                    let name_w = (ui.available_width() - rest).max(30.0);
                    ui.scope(|ui| {
                        ui.set_max_width(name_w);
                        ui.add(egui::Label::new(RichText::new(visible(name)).font(theme::semibold(12.5))).truncate());
                    });
                    ui.label(RichText::new(size).color(t.muted));
                    if !label.is_empty() {
                        ui.label(RichText::new(*label).color(*color).size(11.5));
                    }
                });
            });
        });
        match act {
            Some(("toggle", _)) => self.run_cmd(ui.ctx(), Cmd::ToggleInspector),
            Some(("delete", n)) => self.ask_delete(n),
            Some(("compress", n)) => self.ask_compress(n),
            _ => {}
        }
    }

    /// The minimized folder list: a thin strip that brings it back.
    fn list_strip(&mut self, ui: &mut Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(4.0);
            if ui.button("›").on_hover_text("Show the folder list (Ctrl+B)").clicked() {
                self.run_cmd(ui.ctx(), Cmd::ToggleList);
            }
        });
    }

    /// Speed guard: the map's transitions are the only animation, so they
    /// are where a slow PC shows. Frame intervals are measured while one
    /// runs; if the PC can't keep up (median over 50 ms, under ~20 fps),
    /// transitions are turned off, and over 120 ms the 2.5D map too. Once:
    /// whatever the user turns back on afterwards stays on.
    fn speed_guard(&mut self) {
        const RUN: Duration = Duration::from_millis(220);
        let now = Instant::now();
        let born = self.map.as_ref().and_then(|m| m.born);
        // Every frame drawn because of the transition counts: from the one
        // that started it to the first one after it ended.
        if let (Some(b), Some((prev, _))) = (born, self.last_frame)
            && prev + Duration::from_millis(500) >= b
            && prev < b + RUN
        {
            self.anim_frames.push(now.duration_since(prev).as_secs_f32() * 1000.0);
        }
        let over = born.is_none_or(|b| now.duration_since(b) >= RUN);
        if over && !self.anim_frames.is_empty() {
            let mut f = std::mem::take(&mut self.anim_frames);
            if f.len() >= 2 && !self.settings.slow_pc {
                f.sort_by(f32::total_cmp);
                let median = f[f.len() / 2];
                if median > 50.0 {
                    self.settings.slow_pc = true;
                    self.settings.animations = false;
                    let also_25d = median > 120.0 && self.settings.map_25d;
                    if also_25d {
                        self.settings.map_25d = false;
                        self.map = None;
                    }
                    self.settings.save();
                    self.say(format!(
                        "This PC draws the map slowly (about {:.0} frames a second), so smooth transitions{} are off now. View turns them back on.",
                        1000.0 / median,
                        if also_25d { " and the tilted map" } else { "" }
                    ));
                }
            }
        }
        self.last_frame = Some((now, !over));
    }

    /// Save space badge: the biggest single way to get space back, worked
    /// out on a worker thread after each scan, delete or compress.
    fn poll_badge(&mut self) {
        if std::mem::take(&mut self.badge_dirty) {
            self.badge = None;
            // The map's growth / reclaimable colours depend on the scan too.
            self.reclaim = None;
            if self.settings.map_mode != 0 {
                self.map = None;
            }
            self.badge_rx = self.scanned.as_ref().map(|s| {
                let s = s.clone(); // cheap: shared parts
                let ctx = self.egui_ctx.clone();
                let (tx, rx) = mpsc::channel();
                std::thread::spawn(move || {
                    let best = s.space_tips().into_iter().filter_map(|t| t.bytes.map(|b| (b, t.title))).max_by_key(|x| x.0).filter(|x| x.0 >= 100 << 20);
                    let _ = tx.send(best);
                    ctx.request_repaint();
                });
                rx
            });
        }
        if let Some(rx) = &self.badge_rx {
            match rx.try_recv() {
                Ok(b) => {
                    self.badge = b;
                    self.badge_rx = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.badge_rx = None,
            }
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let want = match (&self.scanned, &self.selected) {
            (Some(s), Some(Key_::Node(n))) => {
                let folder = s.info(*n).map(|i| i.folder).unwrap_or(0);
                format!("DiskRoot: {}", s.path(folder).unwrap_or_default())
            }
            (Some(s), _) => format!("DiskRoot: {}", s.summary().root),
            _ => "DiskRoot".into(),
        };
        if want != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(want.clone()));
            self.title = want;
        }
    }
}

/// Column positions for the tree-table, from the pane width.
struct Columns {
    size_r: f32,
    bar_x: f32,
    bar_w: f32,
    files_r: f32,
    cat_x: f32,
    wide: bool,
}

impl Columns {
    fn new(w: f32) -> Self {
        let wide = w > 760.0;
        let tail = if wide { 96.0 + 196.0 + 84.0 + 150.0 } else { 96.0 + 196.0 };
        let name_end = (w - tail).max(160.0);
        let size_r = name_end + 96.0 - 8.0;
        let bar_x = name_end + 96.0 + 10.0;
        Self { size_r, bar_x, bar_w: 130.0, files_r: bar_x + 196.0 + 74.0, cat_x: bar_x + 196.0 + 84.0, wide }
    }
}

impl eframe::App for DiskRootApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.show(ui);
    }
}

impl DiskRootApp {
    /// One frame of the whole app, on any driver.
    pub fn show(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let _bench = bench_frame(&ctx, self.scanned.is_some());
        if let Some(ms) = self.test_slow {
            std::thread::sleep(Duration::from_millis(ms));
        }
        self.speed_guard();
        self.poll_job(&ctx);
        self.poll_remember(&ctx);
        self.poll_savings(&ctx);
        self.poll_home(&ctx);
        if let Some(rx) = &self.owners_rx
            && let Ok(o) = rx.try_recv()
        {
            self.owners = Some(Arc::new(o));
            self.owners_rx = None;
        }
        self.poll_watch(&ctx);
        self.poll_badge();
        self.check_minimized(&ctx);
        if std::mem::take(&mut self.attention) {
            ctx.send_viewport_cmd(ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        }
        self.keyboard(&ctx);
        let t = self.tokens();
        let bar = |m: i8| Frame::new().fill(t.win).inner_margin(Margin::symmetric(m, 2));

        egui::Panel::top("menu").frame(bar(6)).show_separator_line(false).show(ui, |ui| self.menu_bar(ui));
        egui::Panel::top("toolbar")
            .frame(Frame::new().fill(t.win).inner_margin(Margin::symmetric(8, 5)).stroke(Stroke::new(1.0, t.line)))
            .exact_size(40.0)
            .show(ui, |ui| self.toolbar(ui));
        egui::Panel::bottom("status")
            .frame(Frame::new().fill(t.win).inner_margin(Margin::symmetric(10, 2)).stroke(Stroke::new(1.0, t.line)))
            .exact_size(26.0)
            .show(ui, |ui| self.status_bar(ui));
        // Studio layout: the map in the middle, the folder list on the left,
        // the Inspector under the map. Both docks resize and minimize
        // (Ctrl+B, Ctrl+J); Focus (F) shows only the map.
        let empty = self.scanned.is_none();
        let focus = self.focus && !empty;
        if self.drive_bar.is_some() && !focus {
            egui::Panel::bottom("drivebar")
                .frame(Frame::new().fill(t.win).inner_margin(Margin { left: 10, right: 10, top: 4, bottom: 4 }))
                .exact_size(34.0)
                .show_separator_line(false)
                .show(ui, |ui| self.drive_strip(ui));
        }
        if empty {
            egui::CentralPanel::default().frame(Frame::new().fill(t.win).inner_margin(Margin::same(6))).show(ui, |ui| {
                Self::pane_frame(&t).show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    self.start_screen(ui, &t);
                });
            });
        } else {
            let area = ctx.content_rect();
            if !focus {
                if self.settings.hide_list {
                    egui::Panel::left("list-min")
                        .frame(Frame::new().fill(t.win).inner_margin(Margin { left: 6, right: 0, top: 6, bottom: 6 }))
                        .exact_size(34.0)
                        .show_separator_line(false)
                        .show(ui, |ui| self.list_strip(ui));
                } else {
                    egui::Panel::left("list")
                        .frame(Frame::new().fill(t.win).inner_margin(Margin { left: 6, right: 0, top: 6, bottom: 6 }))
                        .default_size(area.width() * 0.4)
                        .size_range(300.0..=(area.width() * 0.6).max(320.0))
                        .resizable(true)
                        .show_separator_line(false)
                        .show(ui, |ui| self.tree_pane(ui));
                }
                let frame = Frame::new().fill(t.win).inner_margin(Margin { left: 6, right: 6, top: 0, bottom: 6 });
                if self.settings.hide_inspector {
                    egui::Panel::bottom("inspector-min").frame(frame).exact_size(36.0).show_separator_line(false).show(ui, |ui| {
                        Self::pane_frame(&t).inner_margin(Margin::symmetric(10, 2)).show(ui, |ui| self.inspector_bar(ui));
                    });
                } else {
                    egui::Panel::bottom("inspector")
                        .frame(frame)
                        .default_size((area.height() * 0.28).clamp(150.0, 250.0))
                        .size_range(120.0..=(area.height() * 0.5).max(140.0))
                        .resizable(true)
                        .show_separator_line(false)
                        .show(ui, |ui| {
                            Self::pane_frame(&t).inner_margin(Margin { left: 10, right: 10, top: 2, bottom: 0 }).show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                self.inspector_bar(ui);
                            });
                            ui.add_space(4.0);
                            // Each selection starts at the top of its details.
                            ScrollArea::vertical().id_salt(("details", self.selected.clone())).auto_shrink([false, false]).show(ui, |ui| self.details_pane(ui));
                        });
                }
            }
            egui::CentralPanel::default().frame(Frame::new().fill(t.win).inner_margin(Margin::same(6))).show(ui, |ui| self.map_pane(ui));
        }

        if self.scanned.is_some()
            && let Some(q) = self.test_palette.take()
        {
            self.palette = Some(Palette { query: q, ..Default::default() });
        }
        self.palette_ui(&ctx);
        self.dialogs(&ctx);
        self.toasts_ui(&ctx);
        self.update_title(&ctx);
    }
}

/// Label/explanation pairs in a two-column grid (dialogs).
fn facts(ui: &mut Ui, t: &Tokens, rows: &[(&str, &str)]) {
    // Stacked rather than a grid: wrapped labels inside an auto-sized grid
    // in a modal can grow the dialog taller than the window.
    for (k, v) in rows {
        ui.label(RichText::new(*k).font(theme::semibold(13.0)));
        ui.add(egui::Label::new(RichText::new(*v).color(t.muted)).wrap());
        ui.add_space(6.0);
    }
}

/// Toolbar button with a small painted shield (no emoji font needed).
fn shield_button(ui: &mut Ui, t: &Tokens) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(28.0, 24.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Security & privacy"));
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(3), t.hover);
    }
    let c = rect.center();
    let (w, h) = (6.5, 8.0);
    let pts = vec![
        pos2(c.x, c.y - h),
        pos2(c.x + w, c.y - h + 2.5),
        pos2(c.x + w, c.y + 1.0),
        pos2(c.x, c.y + h),
        pos2(c.x - w, c.y + 1.0),
        pos2(c.x - w, c.y - h + 2.5),
    ];
    p.add(egui::Shape::convex_polygon(pts, t.accent, egui::Stroke::NONE));
    let ink = egui::Stroke::new(1.6, t.accent_ink);
    p.line_segment([pos2(c.x - 3.0, c.y - 0.5), pos2(c.x - 0.8, c.y + 2.0)], ink);
    p.line_segment([pos2(c.x - 0.8, c.y + 2.0), pos2(c.x + 3.2, c.y - 3.0)], ink);
    resp
}

/// "+1.2 GB" / "−300 MB" for a size change.
/// `path` relative to the scan root `root` ("" for the root itself), if it's
/// inside it. Case-insensitive, like Windows paths.
/// A size-over-time line chart: (unix time, bytes). Hover shows the
/// nearest scan's date and size. `axis` adds min/max and date labels.
fn size_chart(ui: &mut Ui, t: &Tokens, pts: &[(u64, u64)], size: egui::Vec2, axis: bool) {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    if pts.len() < 2 {
        return;
    }
    let p = ui.painter_at(rect);
    let pad = if axis { egui::vec2(64.0, 16.0) } else { egui::vec2(1.0, 2.0) };
    let plot = Rect::from_min_max(rect.min + egui::vec2(pad.x, 2.0), rect.max - egui::vec2(2.0, pad.y));
    if axis {
        p.rect_filled(plot, CornerRadius::same(3), t.head);
    }
    let (t0, t1) = (pts[0].0 as f64, pts[pts.len() - 1].0.max(pts[0].0.saturating_add(1)) as f64);
    let lo = pts.iter().map(|x| x.1).min().unwrap_or(0);
    let hi = pts.iter().map(|x| x.1).max().unwrap_or(0);
    // Pad a flat line so it sits in the middle instead of on an edge.
    let (lo_f, hi_f) = if hi == lo { (lo as f64 * 0.95, hi as f64 * 1.05 + 1.0) } else { (lo as f64, hi as f64) };
    let at = |x: &(u64, u64)| {
        pos2(
            plot.min.x + ((x.0 as f64 - t0) / (t1 - t0)) as f32 * plot.width(),
            plot.max.y - (((x.1 as f64 - lo_f) / (hi_f - lo_f)) as f32 * plot.height()),
        )
    };
    let line: Vec<egui::Pos2> = pts.iter().map(at).collect();
    p.add(egui::Shape::line(line.clone(), egui::Stroke::new(if axis { 2.0 } else { 1.5 }, t.accent)));
    if axis {
        let font = egui::FontId::proportional(11.0);
        p.text(pos2(rect.min.x, plot.min.y), egui::Align2::LEFT_TOP, fmt_bytes(hi), font.clone(), t.muted);
        p.text(pos2(rect.min.x, plot.max.y), egui::Align2::LEFT_BOTTOM, fmt_bytes(lo), font.clone(), t.muted);
        let day = |s: u64| diskroot_session::date_text((s / 86_400) as u16);
        p.text(pos2(plot.min.x, rect.max.y), egui::Align2::LEFT_BOTTOM, day(pts[0].0), font.clone(), t.faint);
        p.text(pos2(plot.max.x, rect.max.y), egui::Align2::RIGHT_BOTTOM, day(pts[pts.len() - 1].0), font, t.faint);
    }
    if let Some(hover) = resp.hover_pos() {
        let i = line.iter().enumerate().min_by(|a, b| (a.1.x - hover.x).abs().total_cmp(&(b.1.x - hover.x).abs())).map_or(0, |x| x.0);
        p.circle_filled(line[i], 3.5, t.accent);
        let (when, bytes) = pts[i];
        resp.on_hover_text_at_pointer(format!(
            "{} · {} ({})",
            fmt_bytes(bytes),
            diskroot_session::date_text((when / 86_400) as u16),
            remember::age_text(when)
        ));
    }
}

/// Frame-time benchmark, off unless DISKROOT_BENCH=<seconds> is set: once a
/// scan is on screen, redraws continuously for that long, then writes frame
/// intervals and UI build times (ms) to DISKROOT_BENCH_OUT and closes.
struct BenchGuard(Instant);

/// Benchmark state: start, last frame, frame intervals, build times (ms).
type BenchState = (Instant, Instant, Vec<f32>, Vec<f32>);
static BENCH: std::sync::Mutex<Option<BenchState>> = std::sync::Mutex::new(None);

impl Drop for BenchGuard {
    fn drop(&mut self) {
        if let Some(b) = BENCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_mut() {
            b.3.push(self.0.elapsed().as_secs_f32() * 1000.0);
        }
    }
}

fn bench_frame(ctx: &egui::Context, ready: bool) -> Option<BenchGuard> {
    let secs = std::env::var("DISKROOT_BENCH").ok().and_then(|s| s.parse::<f64>().ok())?;
    if !ready {
        return None;
    }
    let now = Instant::now();
    let mut st = BENCH.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let (start, last, frames, builds) = st.get_or_insert_with(|| (now, now, Vec::new(), Vec::new()));
    frames.push(now.duration_since(*last).as_secs_f32() * 1000.0);
    *last = now;
    if now.duration_since(*start).as_secs_f64() >= secs {
        let stats = |v: &[f32]| {
            let mut v = v.to_vec();
            v.sort_by(f32::total_cmp);
            let q = |f: f64| v.get(((v.len().max(1) - 1) as f64 * f) as usize).copied().unwrap_or(0.0);
            format!("n {} mean {:.2} p50 {:.2} p95 {:.2} p99 {:.2} max {:.2}", v.len(), v.iter().sum::<f32>() / v.len().max(1) as f32, q(0.5), q(0.95), q(0.99), q(1.0))
        };
        let line = format!("interval {}
build    {}
", stats(&frames[1..]), stats(builds));
        if let Some(out) = std::env::var_os("DISKROOT_BENCH_OUT") {
            let _ = std::fs::write(out, line);
        }
        ctx.send_viewport_cmd(ViewportCommand::Close);
        None
    } else {
        ctx.request_repaint();
        Some(BenchGuard(now))
    }
}

/// Gives the map's response a description for screen readers.
fn map_resp_info(resp: &egui::Response, label: String) {
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, &label));
}

fn rel_of(root: &str, path: &str) -> Option<String> {
    let r = root.trim_end_matches('\\');
    let (head, tail) = (path.get(..r.len())?, path.get(r.len()..)?);
    if !head.eq_ignore_ascii_case(r) || !(tail.is_empty() || tail.starts_with('\\')) {
        return None;
    }
    Some(tail.trim_start_matches('\\').to_owned())
}

fn fmt_delta(d: i128) -> String {
    let sign = if d > 0 { "+" } else if d < 0 { "−" } else { "±" };
    format!("{sign}{}", fmt_bytes(d.unsigned_abs().min(u128::from(u64::MAX)) as u64))
}

/// One line for the status bar: "Since the previous scan (2 days ago): +1.2 GB".
fn changes_summary(ch: &Changes) -> String {
    let d = i128::from(ch.after_total) - i128::from(ch.before_total);
    let note = if ch.mixed_methods() { " (different scan method, see What changed)" } else { "" };
    format!("Since the previous scan ({}): {}{note}", remember::age_text(ch.before_time), fmt_delta(d))
}

/// Mix two colours: `k` = 0 gives `a`, 1 gives `b`.
fn lerp(a: Color32, b: Color32, k: f32) -> Color32 {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * k.clamp(0.0, 1.0)).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// Wrapped text in a column, left-aligned. (`ui.columns` lays out
/// "justified", which would stretch the spaces of wrapped lines.)
fn plain_note(ui: &mut Ui, text: RichText) {
    ui.with_layout(Layout::top_down(Align::Min), |ui| {
        ui.add(egui::Label::new(text).wrap());
    });
}
