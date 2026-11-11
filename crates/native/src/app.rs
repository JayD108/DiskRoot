//! The native window: menu bar, toolbar, tree-table, map, details pane,
//! status bar ("classic utility, modern finish"). All data comes from `xylem_session`; scans and deletes
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
use xylem_session::integrity::{Integrity, SignatureState};
use xylem_session::remember::{self, Changes, PrevSizes};
use xylem_session::safety::{DeleteWarnings, LogEntry, visible};
use xylem_session::{Action, DeleteOut, FileRow, ItemInfo, Leftover, PendingDelete, RulesStatus, Safety, Scanned, Shape, Summary, TypeRow};

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
    /// A Windows tool Xylem started and waits for (e.g. DISM).
    Tool(String),
}

/// A remembered scan opened at startup: the scan, when it was made, the
/// "what changed" between the last two scans, and how long opening took.
struct Reopened {
    scanned: Scanned,
    time: u64,
    changes: Option<Changes>,
    prev: Option<PrevSizes>,
    took: Duration,
}

enum Done {
    Scanned(Box<Result<Scanned, String>>),
    Deleted(u32, Result<DeleteOut, String>),
    Reopened(Box<Option<Reopened>>),
    Exported(Result<std::path::PathBuf, String>),
    DeletedMany(Vec<(u32, Result<DeleteOut, String>)>),
    Tool(Result<String, String>),
}

struct Job {
    kind: JobKind,
    started: Instant,
    rx: Receiver<Done>,
    /// Set by the Cancel button (Fast scan): stops waiting for the helper.
    cancel: Arc<AtomicBool>,
}

enum Dialog {
    About,
    /// First launch: what Xylem does and doesn't do.
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
    /// View > Leftovers.
    Leftovers(Vec<Leftover>),
    /// Explains a Windows tool before starting it.
    Tool(Action),
}

/// Deleting several marked items at once: each one prepared and armed on
/// its own (all the usual checks), plus the ones Xylem refused and why.
struct ConfirmMany {
    items: Vec<PendingDelete>,
    refused: Vec<(String, String)>,
    bytes: u64,
    warnings: DeleteWarnings,
    ack: bool,
}

/// Result of the background "remember" step: what changed, and the previous
/// scan's folder sizes for the tree's change column.
type Remembered = (Result<Option<Changes>, String>, Option<PrevSizes>);

/// The drive strip at the bottom of the window.
struct DriveBar {
    root: String,
    /// Volume size and free space (None if Windows didn't say).
    space: Option<(u64, u64)>,
    scanned: u64,
    /// The 5 biggest top-level items: (node, name, bytes on disk, colour category).
    top: Vec<(u32, String, u64, u8)>,
}

/// A running duplicate search: shared progress, the cancel flag, and where
/// the result arrives.
type DupesRun = (Arc<xylem_session::dupes::Progress>, Arc<AtomicBool>, Receiver<Vec<xylem_session::dupes::DupGroup>>);

/// The duplicate finder window: intro, running (progress), or results.
struct DupesView {
    min_mb: u64,
    running: Option<DupesRun>,
    groups: Option<Vec<xylem_session::dupes::DupGroup>>,
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
    ages: Option<[u64; xylem_session::AGE_UNKNOWN + 1]>,
}

pub struct XylemApp {
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
    /// The Xylem logo (start screen, About), uploaded once.
    logo: egui::TextureHandle,
    scroll_to_selected: bool,
    /// The user's own rules: loaded or not, and why.
    rules_status: RulesStatus,
    /// Live updates: the running watch on the scan root.
    watcher: Option<xylem_session::watch::Watcher>,
    /// New items seen since the scan (relative path -> bytes): not in the
    /// tree until the next scan.
    live_new: std::collections::HashMap<String, u64>,
    /// Windows reported more changes than it could list: rescan to be exact.
    live_overflow: bool,
    /// Flash the taskbar button (a size alert fired).
    attention: bool,
}

impl XylemApp {
    pub fn new(cc: &eframe::CreationContext<'_>, start: Option<String>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let settings = Settings::load();
        theme::apply(&cc.egui_ctx, if settings.dark { Theme::Dark } else { Theme::Light });
        let mut app = Self {
            path: start.clone().unwrap_or_else(|| settings.last_path.clone()),
            settings,
            drives: xylem_session::drives(),
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
            scan_after_welcome: false,
            remember_rx: None,
            changes: None,
            remembered_at: None,
            drive_bar: None,
            prev_sizes: None,
            marked: BTreeSet::new(),
            links_cache: None,
            logo: cc.egui_ctx.load_texture(
                "xylem-logo",
                egui::ColorImage::from_rgba_unmultiplied([128, 128], include_bytes!("../assets/logo-128.rgba")),
                egui::TextureOptions::LINEAR,
            ),
            scroll_to_selected: false,
            rules_status: xylem_session::reload_rules(),
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
        app
    }

    fn tokens(&self) -> Tokens {
        theme::tokens(if self.settings.dark { Theme::Dark } else { Theme::Light })
    }

    fn kind(&self) -> Kind {
        if self.settings.treemap { Kind::Treemap } else { Kind::Sunburst }
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.status = Some((msg.into(), Instant::now()));
    }

    fn invalidate(&mut self) {
        self.rows = None;
        self.map = None;
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
        std::thread::spawn(move || {
            let r = if fast { xylem_session::scan_admin(&p, &c) } else { xylem_session::scan(&p) };
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
            let r = xylem_session::reopen(&p).map(|(scanned, time)| Reopened {
                scanned,
                time,
                changes: remember::last_changes(&p),
                prev: remember::previous_sizes(&p),
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
            let _ = tx.send((r, prev));
        });
        self.remember_rx = Some(rx);
    }

    fn poll_remember(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.remember_rx else { return };
        match rx.try_recv() {
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(200)),
            Err(TryRecvError::Disconnected) => self.remember_rx = None,
            Ok((r, prev)) => {
                self.remember_rx = None;
                self.prev_sizes = prev;
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
                        let _ = xylem_session::reveal(&p.display().to_string());
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
                self.say(format!(
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
                self.show_scan(s, sum);
                self.remembered_at = None;
                self.changes = None;
                self.prev_sizes = None;
                self.start_remember();
            }
            Err(e) => self.dialog = Some(Dialog::Message("Scan didn't finish".into(), e)),
        }
    }

    fn refresh_drive_bar(&mut self) {
        self.drive_bar = self.scanned.as_ref().map(|s| {
            let sum = s.summary();
            let top = s.list(0, 5).map(|l| l.items.into_iter().map(|i| (i.id, i.name, i.on_disk, i.cat)).collect()).unwrap_or_default();
            DriveBar { space: xylem_session::disk_space(&sum.root), root: sum.root, scanned: sum.on_disk, top }
        });
    }

    fn show_scan(&mut self, s: Scanned, sum: Summary) {
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
    }

    /// Live updates: (re)starts watching the scan root.
    fn start_watch(&mut self) {
        self.watcher = None;
        self.live_new.clear();
        self.live_overflow = false;
        if !self.settings.live_updates {
            return;
        }
        if let Some(sum) = &self.summary {
            match xylem_session::watch::Watcher::start(&sum.root) {
                Ok(w) => self.watcher = Some(w),
                Err(e) => self.say(format!("Live updates are off for this scan: {e}")),
            }
        }
    }

    /// Applies what the watcher reported (at most once a second).
    fn poll_watch(&mut self, ctx: &egui::Context) {
        use xylem_session::watch::Event;
        let Some(w) = &self.watcher else { return };
        ctx.request_repaint_after(Duration::from_secs(1));
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
        self.say(msg);
        self.show_scan(r.scanned, sum);
        self.remembered_at = Some(r.time);
        self.changes = r.changes;
        self.prev_sizes = r.prev;
    }

    fn finish_delete(&mut self, node: u32, r: Result<DeleteOut, String>) {
        self.job = None;
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
        let c = xylem_session::categories().get(own_cat as usize).map_or("", |c| c.name);
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
        if self.map.as_ref().is_some_and(|m| m.root == root) {
            return;
        }
        let Some(s) = &self.scanned else { return };
        let (nodes, up) = match &root {
            MapRoot::Node(n) => {
                let nodes = s.view(*n, 3).unwrap_or_default();
                let up = s.info(*n).ok().and_then(|i| i.parent).map(Key_::Node);
                (nodes, up)
            }
            MapRoot::Cat(k) => match s.category_view(k) {
                Ok(v) => {
                    let up = (v.crumbs.len() > 1).then(|| Key_::Cat(v.crumbs[v.crumbs.len() - 2].key.clone()));
                    (v.nodes, up)
                }
                Err(_) => (Vec::new(), None),
            },
        };
        let kids = map::children(&nodes);
        let ages = match (&root, self.settings.color_by_age) {
            (MapRoot::Node(n), true) => Some(s.age_breakdown(*n, xylem_session::today_days())),
            _ => None,
        };
        self.map = Some(MapState { root, nodes, kids, cached: None, up, ages });
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
                    .on_hover_text("Saves each scan on this PC (names and sizes only) so it reopens instantly and Xylem can show what changed.")
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
                if ui.add_enabled(self.scanned.is_some() && self.job.is_none(), egui::Button::new("Export to CSV")).on_hover_text("Every file and folder of this scan, saved in Documents\\Xylem").clicked() {
                    self.start_export();
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
                if ui.add_enabled(self.scanned.is_some(), egui::Button::new("Find duplicate files…")).clicked() {
                    self.dialog = Some(Dialog::Dupes(Box::new(DupesView { min_mb: 10, running: None, groups: None })));
                    ui.close();
                }
                if ui.add_enabled(self.scanned.is_some(), egui::Button::new("Largest files & file types…")).clicked() {
                    self.open_files();
                    ui.close();
                }
                if ui
                    .add_enabled(self.scanned.is_some(), egui::Button::new("Leftovers…"))
                    .on_hover_text("Old installers, archives and disk images in Downloads, and Windows upgrade leftovers")
                    .clicked()
                {
                    if let Some(s) = &self.scanned {
                        let mut list = s.leftovers(300);
                        list.extend(s.orphan_hints(&xylem_session::orphans::Programs::installed(), 50 << 20, 90));
                        list.sort_by_key(|l| std::cmp::Reverse(l.on_disk));
                        self.dialog = Some(Dialog::Leftovers(list));
                    }
                    ui.close();
                }
                ui.separator();
                let mut changed = false;
                changed |= ui.radio_value(&mut self.settings.treemap, false, "Sunburst map").clicked();
                changed |= ui.radio_value(&mut self.settings.treemap, true, "Treemap").clicked();
                let mut live = self.settings.live_updates;
                if ui.checkbox(&mut live, "Live updates").on_hover_text("Watch the scanned folder while Xylem is open and update sizes as files change").changed() {
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
                if ui.button("About Xylem").clicked() {
                    self.dialog = Some(Dialog::About);
                    ui.close();
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
            ui.add(TextEdit::singleline(&mut self.path).desired_width(190.0).hint_text("C:\\ or a folder"));
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
            let fast = egui::Button::new(RichText::new("⚡  Fast scan").color(t.accent_ink)).fill(t.accent);
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
            ui.selectable_value(&mut tm, false, "◎ Sunburst");
            ui.selectable_value(&mut tm, true, "▦ Treemap");
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
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if shield_button(ui, &t).on_hover_text("Security & privacy: what Xylem does with your disk").clicked() {
                    self.open_security();
                }
                let r = ui.add(TextEdit::singleline(&mut self.filter).desired_width(200.0).hint_text("Filter by name  (Ctrl+F)"));
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
            ui.label(msg);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let muted = |s: String| RichText::new(s).color(t.muted).size(11.5);
                ui.label(RichText::new("●  Read-only").color(t.accent).size(11.5));
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
                        ui.label(RichText::new(format!("{} unreadable", fmt_count(s.unreadable))).color(theme::cat_color(6)).size(11.5))
                            .on_hover_text("Some folders couldn't be read. Fast scan (admin) sees everything.");
                    }
                }
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
        ui.horizontal_centered(|ui| {
            ui.label(RichText::new(text).font(theme::semibold(12.5)));
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
    }

    /// Before the first scan: one card per drive (used / free bar), click
    /// to scan it.
    fn start_screen(&mut self, ui: &mut Ui, t: &Tokens) {
        ui.add_space(28.0);
        let mut pick: Option<String> = None;
        ui.vertical_centered(|ui| {
            ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(72.0, 72.0)));
            ui.add_space(8.0);
            ui.label(RichText::new("See what's filling your drive").font(theme::semibold(16.0)));
            ui.add_space(4.0);
            ui.label(RichText::new("Pick a drive to scan. Fast scan (admin) reads a whole NTFS drive in seconds.").color(t.muted));
            ui.add_space(14.0);
            for d in &self.drives {
                let space = xylem_session::disk_space(d);
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
            self.start_scan(false);
        }
    }

    fn pane_frame(t: &Tokens) -> Frame {
        Frame::new()
            .fill(t.panel)
            .stroke(Stroke::new(1.0, t.line_strong))
            .corner_radius(CornerRadius::same(4))
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
            p.rect_filled(hr, CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 }, t.head);
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
            let today = self.settings.color_by_age.then(xylem_session::today_days);
            let row_color = |r: &Row| match (today, &r.key) {
                (Some(d), Key_::Node(_)) => theme::age_color(xylem_session::age_bucket(d, r.mtime)),
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
            ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, rows.len(), |ui, range| {
                for i in range {
                    let r = &rows[i];
                    let (rect, resp) = ui.allocate_exact_size(vec2(full, ROW_H), Sense::click());
                    if scroll_to == Some(i) {
                        resp.scroll_to_me(Some(Align::Center));
                    }
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
                        p.text(pos2(rect.min.x + cols.size_r, y), Align2::RIGHT_CENTER, fmt_bytes(r.bytes), theme::regular(12.5), t.text);
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
                                if r.dir {
                                    ui.menu_button("Alert me if it grows by", |ui| {
                                        for (label, act) in [("1 GB", "alert-1"), ("5 GB", "alert-5"), ("10 GB", "alert-10"), ("50 GB", "alert-50")] {
                                            if ui.button(label).clicked() {
                                                ctx_action = Some((i, act));
                                                ui.close();
                                            }
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
                        a if a.starts_with("alert-") => {
                            let gb: u64 = a[6..].parse().unwrap_or(1);
                            self.add_alert(n, gb << 30);
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
            self.say("Nothing here is labelled \"safe to clear\" by Xylem's rules, so nothing was marked.");
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
            Action::SystemProtection => xylem_session::tools::open_system_protection(),
            Action::StorageSettings => xylem_session::tools::open_storage_settings(),
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
                    let _ = tx.send(Done::Tool(xylem_session::tools::dism_component_cleanup()));
                });
                self.job = Some(Job { kind: JobKind::Tool("Windows component cleanup (DISM)".into()), started: Instant::now(), rx, cancel: Arc::default() });
                return;
            }
        };
        if let Err(e) = r {
            self.dialog = Some(Dialog::Message("Couldn't open it".into(), e));
        }
    }

    fn add_alert(&mut self, n: u32, grow: u64) {
        let Some(s) = &self.scanned else { return };
        let Ok(info) = s.info(n) else { return };
        self.settings.alerts.retain(|a| !a.path.eq_ignore_ascii_case(&info.path));
        self.settings.alerts.push(Alert { path: info.path.clone(), baseline: info.on_disk, grow });
        self.settings.save();
        self.say(format!("Alert set: Xylem tells you when {} grows by {} (while it's open, or at the next scan)", info.name, fmt_bytes(grow)));
    }

    fn reload_rules(&mut self) {
        self.rules_status = xylem_session::reload_rules();
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
            && let Err(e) = s.path(n).and_then(|p| xylem_session::reveal(&p)) {
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
                self.dialog = Some(Dialog::Message("Can't delete this".into(), "Xylem won't delete the whole scanned folder or drive.".into()));
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
        warnings.large |= bytes >= xylem_session::safety::LARGE_BYTES || files >= u64::from(xylem_session::safety::LARGE_FILES);
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
        self.dialog = Some(Dialog::Security(Box::new(xylem_session::integrity::check_current_exe())));
    }

    /// Writes the scan to Documents\Xylem\<root> <date>.csv on a worker thread.
    fn start_export(&mut self) {
        let Some(s) = &self.scanned else { return };
        let (idx, _, _) = s.remember_parts();
        let root = s.summary().root;
        let docs = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).unwrap_or_default().join("Documents").join("Xylem");
        let safe: String = root.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' }).collect();
        let stamp = xylem_session::safety::utc_now().replace([':', ' '], "-");
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
        self.dialog = Some(Dialog::History(xylem_session::safety::read_log(200)));
    }

    fn map_pane(&mut self, ui: &mut Ui) {
        let t = self.tokens();
        self.ensure_map();
        Self::pane_frame(&t).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            let full = ui.available_width();
            // Title strip
            let (hr, _) = ui.allocate_exact_size(vec2(full, 26.0), Sense::hover());
            let p = ui.painter();
            p.rect_filled(hr, CornerRadius { nw: 4, ne: 4, sw: 0, se: 0 }, t.head);
            p.line_segment([hr.left_bottom(), hr.right_bottom()], Stroke::new(1.0, t.line));
            let Some(m) = &self.map else {
                p.text(pos2(hr.min.x + 10.0, hr.center().y), Align2::LEFT_CENTER, "Map", theme::semibold(12.0), t.text);
                return;
            };
            if m.nodes.is_empty() {
                return;
            }
            let root = &m.nodes[0];
            if let Some(title) = map::fit(p, &root.name, 12.0, full * 0.55) {
                p.text(pos2(hr.min.x + 10.0, hr.center().y), Align2::LEFT_CENTER, title, theme::semibold(12.0), t.text);
            }
            p.text(
                pos2(hr.max.x - 10.0, hr.center().y),
                Align2::RIGHT_CENTER,
                format!("{} · {}", fmt_bytes(root.size), fmt_files(u64::from(root.files))),
                theme::regular(11.5),
                t.muted,
            );

            // Legend (bottom)
            let legend = self.legend_items();
            let rows = legend.len().div_ceil(2) as f32;
            let legend_h = if legend.is_empty() { 0.0 } else { rows * 20.0 + 14.0 };
            let avail = ui.available_size();
            let (map_rect, resp) = ui.allocate_exact_size(vec2(full, (avail.y - legend_h).max(80.0)), Sense::click());

            let kind = self.kind();
            let tree_hover = self.hover_tree.as_ref().and_then(|k| self.map_index(k));
            let selected = self.selected.as_ref().and_then(|k| self.map_index(k)).filter(|&i| i != 0);
            {
                let m = self.map.as_mut().expect("map exists");
                let stale = !matches!(&m.cached, Some((r, k, _)) if *r == map_rect && *k == kind);
                if stale {
                    m.cached = Some((map_rect, kind, map::layout(kind, &m.nodes, &m.kids, map_rect)));
                }
            }
            let m = self.map.as_ref().expect("map exists");
            let layout = &m.cached.as_ref().expect("layout").2;
            let hit = resp.hover_pos().and_then(|q| map::hit(layout, q));
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
                today: self.settings.color_by_age.then(xylem_session::today_days),
            };
            map::paint(&ui.painter_at(map_rect), layout, &pc);

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
                            ui.label(RichText::new(format!("{what}: {}", xylem_session::age_text(xylem_session::today_days(), n.mtime))).color(t.muted));
                        }
                        if !n.hint.is_empty() {
                            ui.label(RichText::new(&n.hint).color(t.faint).size(11.5));
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
                    if resp.hovered() {
                        lp.rect_filled(r, CornerRadius::same(3), t.hover);
                    }
                    lp.rect_filled(Rect::from_center_size(pos2(cx + 6.0, cy), vec2(10.0, 10.0)), CornerRadius::same(2), theme::swatch(*cat));
                    lp.text(pos2(cx + 17.0, cy), Align2::LEFT_CENTER, name, theme::regular(12.0), t.text);
                    lp.text(
                        pos2(cx + col_w - 12.0, cy),
                        Align2::RIGHT_CENTER,
                        format!("{} · {}", fmt_bytes(*bytes), fmt_pct(*bytes, *total)),
                        theme::regular(11.5),
                        t.muted,
                    );
                    if resp.clicked() {
                        clicked_leg = Some(key.clone());
                    }
                }
                ui.allocate_space(vec2(full, legend_h - 4.0));
                if let Some(k) = clicked_leg
                    && let Key_::Cat(key) = &k
                    && !key.starts_with("age:") {
                        if !self.settings.by_category {
                            self.settings.by_category = true;
                            self.regroup();
                        }
                        let cat = key.strip_prefix("cat:").and_then(|c| c.parse().ok()).unwrap_or(0);
                        self.reveal_cat(&key.clone(), cat);
                    }
            }
        });
    }

    /// (key, name, bytes, total, colour) entries under the map.
    fn legend_items(&self) -> Vec<(Key_, String, u64, u64, u8)> {
        let (Some(s), Some(m)) = (&self.scanned, &self.map) else { return Vec::new() };
        let cats = xylem_session::categories();
        let mut out = Vec::new();
        if self.settings.color_by_age {
            if let Some(ages) = &m.ages {
                let total: u64 = ages.iter().sum();
                for (b, &v) in ages.iter().enumerate().filter(|x| *x.1 > 0) {
                    let name = xylem_session::AGE_BUCKETS.get(b).map_or("Date unknown", |x| x.1);
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
                Ok(i) if !i.dir => xylem_session::safety::hardlink_names(std::path::Path::new(&i.path)),
                _ => Vec::new(),
            };
            self.links_cache = Some((*n, names));
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
                    let (label, color, note) = match rule.and_then(|r| r.safety) {
                        None if rule.is_some_and(|r| r.user) => (
                            "Your rule",
                            t.muted,
                            Some(rule.and_then(|r| r.note.clone()).unwrap_or_else(|| "Classified by your own rule. Check what it is before removing it.".into())),
                        ),
                        Some(Safety::Green) => ("Safe to clear", theme::cat_color(2), rule.and_then(|r| r.note.clone())),
                        Some(Safety::Yellow) => ("Check first", theme::cat_color(6), rule.and_then(|r| r.note.clone())),
                        Some(Safety::Red) => ("Don't delete by hand", theme::cat_color(5), rule.and_then(|r| r.note.clone())),
                        None => ("Unknown", t.muted, Some("No rule for this yet. Check what it is before removing it.".into())),
                    };
                    let parent_size = info.parent.and_then(|p| s.info(p).ok()).map(|p| p.on_disk);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            egui::Grid::new("props").num_columns(2).spacing([14.0, 1.0]).show(ui, |ui| {
                                let key = |ui: &mut Ui, s: &str| {
                                    ui.label(RichText::new(s).color(t.muted));
                                };
                                key(ui, "Name");
                                ui.label(RichText::new(&info.name).font(theme::semibold(13.0)));
                                ui.end_row();
                                key(ui, "Location");
                                ui.label(&info.path);
                                ui.end_row();
                                key(ui, "Size on disk");
                                let mut size = format!("{} · {}", fmt_bytes(info.on_disk), fmt_files(u64::from(info.files)));
                                if let Some(ps) = parent_size {
                                    size += &format!(" · {} of its folder", fmt_pct(info.on_disk, ps));
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
                                    ui.label(format!("{} · {}", xylem_session::date_text(mt), xylem_session::age_text(xylem_session::today_days(), mt)));
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
                                    Action::DismCleanup => ("Clean up with Windows…", "Runs Windows' DISM component cleanup (admin). Xylem itself deletes nothing here."),
                                    Action::SystemProtection => ("System Protection…", "Opens Windows' System Protection settings to manage restore points."),
                                    Action::StorageSettings => ("Open Storage settings…", "Windows removes these: Settings > System > Storage > Temporary files."),
                                };
                                if ui.add_sized([170.0, 28.0], egui::Button::new(text)).on_hover_text(hint).clicked() {
                                    tool_action = Some(a);
                                }
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
                                .on_hover_text("Marks every place here that Xylem's rules label \"safe to clear\". Press Delete to review them in one confirmation.")
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
                    "Approve the Windows prompt. Xylem then reads the drive's Master File Table, read-only.".to_owned(),
                ),
                JobKind::Delete(n) => (format!("Moving \"{n}\" to the Recycle Bin"), "Windows is moving the files…".to_owned()),
                JobKind::Reopen(p) => (format!("Opening the remembered scan of {p}"), "Loading the last scan from this PC…".to_owned()),
                JobKind::Export(p) => ("Exporting to CSV".to_owned(), format!("Writing {p}…")),
                JobKind::DeleteMany(n) => (format!("Moving {n} items to the Recycle Bin"), "Each item is re-checked on disk first…".to_owned()),
                JobKind::Tool(name) => (
                    name.clone(),
                    "Approve the Windows prompt. The tool runs in its own window and can take several minutes; Xylem waits until it's done.".to_owned(),
                ),
            };
            Modal::new(egui::Id::unique("xylem-job")).show(ctx, |ui| {
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
                let w = r.width() * 0.28;
                let k = (1.0 - (secs * 2.2).cos()) / 2.0; // 0 → 1 → 0, smooth
                let x = r.min.x + k * (r.width() - w);
                p.rect_filled(Rect::from_min_size(pos2(x, r.min.y), vec2(w, r.height())), CornerRadius::same(4), t.accent);
                if matches!(job.kind, JobKind::Scan(_)) {
                    let found = xylem_session::scan_progress();
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
                });
            });
            return;
        }
        let Some(d) = &mut self.dialog else { return };
        let mut close = false;
        let mut confirm: Option<u32> = None;
        let mut action: Option<&'static str> = None;
        let mut jump: Option<String> = None;
        let wide = matches!(d, Dialog::History(_) | Dialog::Changes(_) | Dialog::Files(_) | Dialog::ConfirmMany(_) | Dialog::Dupes(_) | Dialog::Leftovers(_));
        let mut leftover_marks: Option<Vec<u32>> = None;
        let mut tool_start: Option<(Action, bool)> = None;
        let mut dupes_start = false;
        let mut dupes_mark: Option<Vec<u32>> = None;
        let mut requery: Option<String> = None;
        let mut jump_node: Option<u32> = None;
        // One window id per dialog kind, so a dialog never inherits another
        // one's remembered size.
        let kind = std::mem::discriminant(&*d);
        let resp = Modal::new(egui::Id::unique("xylem-dialog").with(kind)).show(ctx, |ui| {
            ui.set_width(if wide { 720.0 } else { 500.0 });
            match d {
                Dialog::About => {
                    ui.horizontal(|ui| {
                        ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(56.0, 56.0)));
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Xylem").font(theme::semibold(18.0)));
                            ui.label(format!("Version {} · disk space analyzer for Windows", env!("CARGO_PKG_VERSION")));
                        });
                    });
                    ui.add_space(6.0);
                    ui.label(RichText::new("Xylem is the plant tissue that carries water up from the roots. This app carries your drive's file table up into a map.").color(t.muted));
                }
                Dialog::Welcome => {
                    ui.label(RichText::new("Welcome to Xylem").font(theme::semibold(16.0)));
                    ui.add_space(4.0);
                    ui.label(RichText::new("Before you start, here is exactly what Xylem does on your computer:").color(t.muted));
                    ui.add_space(8.0);
                    facts(ui, &t, &[
                        ("Reads", "File names and sizes. It doesn't read what's inside your files, except when you start View → Find duplicate files."),
                        ("Stays offline", "No network connections, no accounts, no telemetry."),
                        ("Asks first", "Admin rights only for Fast scan, through the Windows prompt. The helper exits after the scan."),
                        ("Deletes carefully", "Only when you confirm, always to the Recycle Bin, re-checked on disk first. Windows and system folders are locked."),
                        ("Keeps a record", "Every delete is written to a log on this PC (File → Deletion history)."),
                        ("Remembers scans", "Your last scans are saved on this PC (names and sizes only), so they reopen instantly and Xylem can show what changed. Turn it off or forget them in the File menu."),
                    ]);
                }
                Dialog::Security(ig) => {
                    ui.label(RichText::new("Security & privacy").font(theme::semibold(16.0)));
                    ui.add_space(8.0);
                    facts(ui, &t, &[
                        ("Network", "Xylem makes no network connections and sends nothing anywhere. The native app has no browser engine."),
                        ("Disk access", "Drives are opened for reading only. Scanning never writes to the disk."),
                        ("Admin rights", "Only for Fast scan, after the Windows prompt. A helper reads the file table (about 1–5 s) and exits."),
                        ("Deleting", "Always to the Recycle Bin, after you confirm. Right before moving, Xylem re-checks the item: if it changed, or a folder on its path became a link since the scan, it refuses."),
                        ("Warnings", "Extra confirmation for OneDrive folders (deleting syncs to your other devices), Git repositories (unpushed work) and very large deletes."),
                        ("History", "Each delete is logged on this PC in %LOCALAPPDATA%\\Xylem\\deletions.log."),
                        ("Remembered scans", "Names and sizes of your last scans (up to 8 drives or folders) are kept in %LOCALAPPDATA%\\Xylem\\cache for instant reopen and \"what changed\". Never uploaded. File → Remember scans turns it off; File → Forget remembered scans deletes them."),
                    ]);
                    ui.add_space(10.0);
                    ui.separator();
                    ui.label(RichText::new("This copy of Xylem").font(theme::semibold(13.5)));
                    ui.add_space(4.0);
                    let (sig_ok, sig_text) = match ig.signature {
                        SignatureState::Signed => (true, "Digitally signed, and Windows trusts the signature."),
                        SignatureState::Unsigned => (false, "Not signed (normal for a build from source). Official releases will be signed."),
                        SignatureState::Invalid => (false, "Has a signature Windows does NOT trust. Don't use Fast scan with this copy."),
                    };
                    let loc_text = if ig.protected_location {
                        "Installed in Program Files, which only administrators can change."
                    } else {
                        "Running from a folder your user account can change. Install Xylem to protect the admin helper used by Fast scan."
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
                    ui.label(RichText::new("Everything deleted through Xylem on this PC, newest first. Recycled items can be restored from the Recycle Bin.").color(t.muted));
                    ui.add_space(8.0);
                    if entries.is_empty() {
                        ui.label(RichText::new("Nothing has been deleted with Xylem yet.").color(t.faint));
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
                            "{} · previous scan {} → latest scan {}",
                            ch.root,
                            remember::age_text(ch.before_time),
                            remember::age_text(ch.after_time)
                        ))
                        .color(t.muted),
                    );
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
                        ui.label(RichText::new(format!("Skipped ({}): Xylem won't delete these", c.refused.len())).font(theme::semibold(13.0)).color(theme::cat_color(5)));
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
                        ui.add(egui::Label::new(RichText::new("This is the only part of Xylem that reads file contents: it compares files of the same size, first by their beginning and end, then in full. Nothing leaves your PC. Online-only cloud files are skipped (reading them would download them), and hardlinks (one file with two names) aren't counted as copies.").color(t.muted)).wrap());
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
                                    if r.on_hover_text(format!("{}\nModified {}\nClick to show it in the tree", visible(&f.path), xylem_session::age_text(xylem_session::today_days(), f.mtime))).clicked() {
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
                    ui.add(egui::Label::new(RichText::new("Add your own categories for folders and file types, e.g. \"Projects/*/out\" as Dev stuff > My build output. Your rules can't change what Xylem protects: they never apply where a built-in rule matches or inside protected places like C:\\Windows, and their safety label stays \"Your rule\".").color(t.muted)).wrap());
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
                Dialog::Alerts => {
                    ui.label(RichText::new("Size alerts").font(theme::semibold(16.0)));
                    ui.add(egui::Label::new(RichText::new("Xylem checks these folders after every scan and, with live updates on, while it's open. It doesn't run in the background. Right-click a folder > \"Alert me if it grows by\" to add one.").color(t.muted)).wrap());
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
                        ("Xylem", "Starts DISM and waits. It deletes nothing itself here."),
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
                                    let kind = if l.mtime > 0 { format!("{} · {}", l.kind, xylem_session::age_text(xylem_session::today_days(), l.mtime)) } else { l.kind.clone() };
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
                        ui.add(egui::Label::new(RichText::new("Downloads are yours to judge: Xylem can't tell whether you still need an installer or archive. Marking selects them for one review; nothing is deleted until you confirm.").color(t.muted).size(11.5)).wrap());
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
                        ui.label(RichText::new(format!("Xylem won't delete this. {why}")).color(theme::cat_color(5)));
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
                    Dialog::Welcome => {
                        let b = egui::Button::new(RichText::new("Got it").color(t.accent_ink)).fill(t.accent).min_size(vec2(90.0, 26.0));
                        if ui.add(b).clicked() {
                            action = Some("welcome-done");
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
            Some("rules-reload") => self.reload_rules(),
            Some("rules-edit") => match xylem_session::create_example_rules() {
                Ok(p) => {
                    let notepad = xylem_session::tools::system32("notepad.exe");
                    if let Err(e) = std::process::Command::new(notepad).arg(&p).spawn() {
                        self.say(format!("Couldn't open Notepad: {e}"));
                    }
                }
                Err(e) => self.dialog = Some(Dialog::Message("Couldn't create the rules file".into(), e)),
            },
            Some("open-log") => {
                if let Some(p) = xylem_session::safety::log_path() {
                    let _ = xylem_session::reveal(&p.display().to_string());
                }
            }
            _ => {}
        }
        if dupes_start
            && let (Some(s), Some(Dialog::Dupes(dv))) = (&self.scanned, &mut self.dialog)
        {
            let cands = s.duplicate_candidates(dv.min_mb << 20);
            let prog = Arc::new(xylem_session::dupes::Progress::default());
            let cancel = Arc::new(AtomicBool::new(false));
            let (tx, rx) = mpsc::channel();
            let (p2, c2) = (Arc::clone(&prog), Arc::clone(&cancel));
            std::thread::spawn(move || {
                let _ = tx.send(xylem_session::dupes::find(cands, &c2, &p2));
            });
            dv.running = Some((prog, cancel, rx));
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
        if self.dialog.is_some() || self.job.is_some() {
            return;
        }
        let typing = ctx.egui_wants_keyboard_input();
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

    fn update_title(&mut self, ctx: &egui::Context) {
        let want = match (&self.scanned, &self.selected) {
            (Some(s), Some(Key_::Node(n))) => {
                let folder = s.info(*n).map(|i| i.folder).unwrap_or(0);
                format!("Xylem: {}", s.path(folder).unwrap_or_default())
            }
            (Some(s), _) => format!("Xylem: {}", s.summary().root),
            _ => "Xylem".into(),
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

impl eframe::App for XylemApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_job(&ctx);
        self.poll_remember(&ctx);
        self.poll_watch(&ctx);
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
        if self.drive_bar.is_some() {
            egui::Panel::bottom("drivebar")
                .frame(Frame::new().fill(t.win).inner_margin(Margin { left: 10, right: 10, top: 4, bottom: 4 }))
                .exact_size(34.0)
                .show_separator_line(false)
                .show(ui, |ui| self.drive_strip(ui));
        }
        egui::Panel::bottom("details")
            .frame(Frame::new().fill(t.win).inner_margin(Margin { left: 6, right: 6, top: 0, bottom: 6 }))
            .exact_size(184.0)
            .show_separator_line(false)
            .show(ui, |ui| self.details_pane(ui));
        let w = ctx.content_rect().width();
        egui::Panel::right("map")
            .frame(Frame::new().fill(t.win).inner_margin(Margin { left: 0, right: 6, top: 6, bottom: 6 }))
            .default_size(w * 0.43)
            .size_range(320.0..=w * 0.7)
            .resizable(true)
            .show_separator_line(false)
            .show(ui, |ui| self.map_pane(ui));
        egui::CentralPanel::default()
            .frame(Frame::new().fill(t.win).inner_margin(Margin { left: 6, right: 6, top: 6, bottom: 6 }))
            .show(ui, |ui| self.tree_pane(ui));

        self.dialogs(&ctx);
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
    format!("Since the previous scan ({}): {}", remember::age_text(ch.before_time), fmt_delta(d))
}
