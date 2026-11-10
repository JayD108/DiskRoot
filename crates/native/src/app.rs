//! The native window: menu bar, toolbar, tree-table, map, details pane,
//! status bar. Same structure as the design prototype ("classic utility,
//! modern finish"). All data comes from `xylem_session`; scans and deletes
//! run on worker threads so the window never freezes.

use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Frame, Key, Layout, Margin, Modal, Rect, RichText, ScrollArea, Sense,
    Stroke, StrokeKind, TextEdit, Ui, ViewportCommand, pos2, vec2,
};
use xylem_session::integrity::{Integrity, SignatureState};
use xylem_session::remember::{self, Changes};
use xylem_session::safety::{DeleteWarnings, LogEntry, visible};
use xylem_session::{DeleteOut, ItemInfo, PendingDelete, RULES, Safety, Scanned, Shape, Summary};

use crate::map::{self, Hit, Kind, Layout as MapLayout, PaintCtx};
use crate::settings::Settings;
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
}

enum JobKind {
    Scan(String),
    Fast(String),
    Delete(String),
    Reopen(String),
}

/// A remembered scan opened at startup: the scan, when it was made, the
/// "what changed" between the last two scans, and how long opening took.
struct Reopened {
    scanned: Scanned,
    time: u64,
    changes: Option<Changes>,
    took: Duration,
}

enum Done {
    Scanned(Box<Result<Scanned, String>>),
    Deleted(u32, Result<DeleteOut, String>),
    Reopened(Box<Option<Reopened>>),
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
    Message(String, String),
    ConfirmDelete(Box<Confirm>),
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
    remember_rx: Option<Receiver<Result<Option<Changes>, String>>>,
    /// What changed between the previous scan and the current one.
    changes: Option<Changes>,
    /// Set when the tree on screen is a remembered scan (its scan time).
    remembered_at: Option<u64>,
    scroll_to_selected: bool,
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
            scroll_to_selected: false,
        };
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
            let _ = tx.send(remember::remember(&idx, &stats, &mode).map_err(|e| e.to_string()));
        });
        self.remember_rx = Some(rx);
    }

    fn poll_remember(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.remember_rx else { return };
        match rx.try_recv() {
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(200)),
            Err(TryRecvError::Disconnected) => self.remember_rx = None,
            Ok(r) => {
                self.remember_rx = None;
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
                ctx.request_repaint_after(Duration::from_millis(120));
            }
            Err(TryRecvError::Disconnected) => {
                self.job = None;
                self.dialog = Some(Dialog::Message("Something went wrong".into(), "The background task stopped unexpectedly.".into()));
            }
            Ok(Done::Scanned(r)) => self.finish_scan(*r),
            Ok(Done::Deleted(node, r)) => self.finish_delete(node, r),
            Ok(Done::Reopened(r)) => self.finish_reopen(*r),
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
                self.start_remember();
            }
            Err(e) => self.dialog = Some(Dialog::Message("Scan didn't finish".into(), e)),
        }
    }

    fn show_scan(&mut self, s: Scanned, sum: Summary) {
        self.summary = Some(sum);
        self.scanned = Some(s);
        self.expanded.clear();
        self.expanded.insert(self.root_key());
        self.selected = Some(self.root_key());
        self.invalidate();
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
                    if self.selected == Some(Key_::Node(node)) {
                        self.selected = parent.map(Key_::Node);
                    }
                }
                self.invalidate();
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
        match RULES.subs.get(sub as usize) {
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
        self.map = Some(MapState { root, nodes, kids, cached: None, up });
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
                ui.separator();
                let mut changed = false;
                changed |= ui.radio_value(&mut self.settings.treemap, false, "Sunburst map").clicked();
                changed |= ui.radio_value(&mut self.settings.treemap, true, "Treemap").clicked();
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
                let r = ui.add(TextEdit::singleline(&mut self.filter).desired_width(200.0).hint_text("Filter by name"));
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
                _ => "Ready".into(),
            };
            ui.label(msg);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let muted = |s: String| RichText::new(s).color(t.muted).size(11.5);
                ui.label(RichText::new("●  Read-only").color(t.accent).size(11.5));
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
                p.text(pos2(hr.min.x + cols.cat_x, hr.center().y), Align2::LEFT_CENTER, "Category", f, t.muted);
            }

            if self.scanned.is_none() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("See what's filling your drive").font(theme::semibold(16.0)));
                    ui.add_space(6.0);
                    ui.label(RichText::new("Pick a drive and press Scan, or Fast scan for whole NTFS drives (reads the file table directly).").color(t.muted));
                });
                return;
            }

            let rows = self.rows.take().unwrap_or_default();
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
                    if selected {
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
                        // Category swatch
                        let col = if r.cat == u8::MAX { t.faint } else { theme::cat_color(r.cat) };
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
                        let col = if r.cat == u8::MAX { t.faint } else { theme::cat_color(r.cat) };
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
                            if let Some(c) = map::fit(&p, &r.cat_label, 12.0, rect.max.x - (rect.min.x + cols.cat_x) - 8.0) {
                                p.text(pos2(rect.min.x + cols.cat_x, y), Align2::LEFT_CENTER, c, theme::regular(12.0), t.muted);
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
                            });
                        }
                }
            });
            self.scroll_to_selected = false;
            self.hover_tree = hovered;

            // Apply interactions after drawing.
            if let Some((i, twisty)) = clicked {
                self.tree_focus = true;
                if twisty && rows[i].expandable {
                    toggled = Some(i);
                } else if rows[i].more == 0 {
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
            if let Some((i, act)) = ctx_action
                && let Key_::Node(n) = rows[i].key {
                    self.select(Key_::Node(n));
                    match act {
                        "reveal" => self.reveal_in_explorer(n),
                        _ => self.ask_delete(n),
                    }
                }
            self.rows = if rebuild { None } else { Some(rows) };
        });
    }

    fn reveal_in_explorer(&mut self, n: u32) {
        if let Some(s) = &self.scanned
            && let Err(e) = s.path(n).and_then(|p| xylem_session::reveal(&p)) {
                self.dialog = Some(Dialog::Message("Couldn't open Explorer".into(), e));
            }
    }

    fn ask_delete(&mut self, n: u32) {
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

    fn open_security(&mut self) {
        self.dialog = Some(Dialog::Security(Box::new(xylem_session::integrity::check_current_exe())));
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
                        if n.merged > 0 {
                            ui.label(RichText::new(format!("{} items too small to draw", fmt_count(u64::from(n.merged)))).color(t.muted));
                        } else if !label.is_empty() {
                            ui.label(RichText::new(label).color(t.muted));
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
                    lp.rect_filled(Rect::from_center_size(pos2(cx + 6.0, cy), vec2(10.0, 10.0)), CornerRadius::same(2), theme::cat_color(*cat));
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
                    && let Key_::Cat(key) = &k {
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
        let t = self.tokens();
        Self::pane_frame(&t).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            let Some(s) = &self.scanned else {
                ui.label(RichText::new("Select a folder or file to see what it is and whether it's safe to remove.").color(t.faint));
                return;
            };
            let mut action: Option<(&'static str, u32)> = None;
            match &self.selected {
                Some(Key_::Node(n)) => {
                    let Ok(info) = s.info(*n) else { return };
                    let rule = RULES.info.get(info.rule as usize);
                    let (label, color, note) = match rule.and_then(|r| r.safety) {
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
                                ui.label(size);
                                ui.end_row();
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
                }
                None => {}
            }
            if let Some((a, n)) = action {
                if a == "reveal" { self.reveal_in_explorer(n) } else { self.ask_delete(n) }
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
            };
            Modal::new(egui::Id::unique("xylem-job")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.label(RichText::new(title).font(theme::semibold(14.0)));
                ui.add_space(6.0);
                ui.label(RichText::new(text).color(t.muted));
                ui.add_space(10.0);
                // Indeterminate bar: a sap pulse moving along the track.
                let (r, _) = ui.allocate_exact_size(vec2(420.0, 8.0), Sense::hover());
                let p = ui.painter();
                p.rect_filled(r, CornerRadius::same(2), t.head);
                let w = r.width() * 0.25;
                let x = r.min.x + (secs * 0.6).fract() * (r.width() + w) - w;
                let pulse = Rect::from_min_max(pos2(x.max(r.min.x), r.min.y), pos2((x + w).min(r.max.x), r.max.y));
                p.rect_filled(pulse, CornerRadius::same(2), t.accent);
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
        let wide = matches!(d, Dialog::History(_) | Dialog::Changes(_));
        // One window id per dialog kind, so a dialog never inherits another
        // one's remembered size.
        let kind = std::mem::discriminant(&*d);
        let resp = Modal::new(egui::Id::unique("xylem-dialog").with(kind)).show(ctx, |ui| {
            ui.set_width(if wide { 720.0 } else { 500.0 });
            match d {
                Dialog::About => {
                    ui.label(RichText::new("Xylem").font(theme::semibold(16.0)));
                    ui.label(format!("Version {} · disk space analyzer for Windows", env!("CARGO_PKG_VERSION")));
                    ui.add_space(6.0);
                    ui.label(RichText::new("Xylem is the plant tissue that carries water up from the roots. This app carries your drive's file table up into a map.").color(t.muted));
                }
                Dialog::Welcome => {
                    ui.label(RichText::new("Welcome to Xylem").font(theme::semibold(16.0)));
                    ui.add_space(4.0);
                    ui.label(RichText::new("Before you start, here is exactly what Xylem does on your computer:").color(t.muted));
                    ui.add_space(8.0);
                    facts(ui, &t, &[
                        ("Reads", "File names and sizes. It never opens or reads what's inside your files."),
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
                    if let Some(note) = RULES.info.get(info.rule as usize).and_then(|r| r.note.as_ref()) {
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
                    Dialog::Welcome => {
                        let b = egui::Button::new(RichText::new("Got it").color(t.accent_ink)).fill(t.accent).min_size(vec2(90.0, 26.0));
                        if ui.add(b).clicked() {
                            action = Some("welcome-done");
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
            Some("open-log") => {
                if let Some(p) = xylem_session::safety::log_path() {
                    let _ = xylem_session::reveal(&p.display().to_string());
                }
            }
            _ => {}
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
        egui::Panel::bottom("details")
            .frame(Frame::new().fill(t.win).inner_margin(Margin { left: 6, right: 6, top: 0, bottom: 6 }))
            .exact_size(158.0)
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
fn fmt_delta(d: i128) -> String {
    let sign = if d > 0 { "+" } else if d < 0 { "−" } else { "±" };
    format!("{sign}{}", fmt_bytes(d.unsigned_abs().min(u128::from(u64::MAX)) as u64))
}

/// One line for the status bar: "Since the previous scan (2 days ago): +1.2 GB".
fn changes_summary(ch: &Changes) -> String {
    let d = i128::from(ch.after_total) - i128::from(ch.before_total);
    format!("Since the previous scan ({}): {}", remember::age_text(ch.before_time), fmt_delta(d))
}
