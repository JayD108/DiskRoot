//! Full pages besides the map and Home:
//! - Drive status: health, who's writing, size history and past scans, as tabs.
//! - Duplicates: identical files (with a resolver: which copy to keep) and
//!   similar folders, plus the searches saved on this PC.
//! - Review marked: every marked item, ticked to delete or not, before the
//!   usual confirmation.
//! - Activity: what was deleted and compressed through DiskRoot, and what
//!   changed on the drive, with times.

use super::*;
use diskroot_session::{activity, dupes, later};
use super::triage::{self, Meta};
use std::collections::HashMap;

/// A running duplicate search: shared progress, the cancel flag, and where
/// the finished search arrives.
pub(super) type DupesRun = (Arc<dupes::Progress>, Arc<AtomicBool>, Receiver<dupes::Run>);

/// "Verify now" results: the paths and their full BLAKE3 (None: unreadable).
pub(super) type Verified = (Vec<String>, Vec<Option<String>>);

/// The Duplicates page.
pub(super) struct DupesPage {
    /// 0 identical files, 1 similar folders, 2 past searches.
    pub tab: u8,
    pub min_mb: u64,
    /// Search only the folder selected in the list (else the whole scan).
    pub only_selected: bool,
    pub running: Option<DupesRun>,
    pub started: Instant,
    /// The search on screen (just finished, or opened from the past ones).
    pub run: Option<dupes::Run>,
    /// The search on screen was opened from the saved ones.
    pub from_past: bool,
    /// Per set: which copy is kept, and which copies are chosen to remove.
    pub keep: Vec<usize>,
    pub remove: Vec<Vec<bool>>,
    /// The set shown on the right.
    pub sel: usize,
    pub filter: String,
    /// 0 all, then KINDS.
    pub kind: u8,
    /// Sets that pass the filter, and the filter they were made for.
    pub shown: Vec<usize>,
    pub shown_for: Option<(String, u8, usize)>,
    /// Which copy to keep when choosing for every set (see KEEP_RULES).
    pub rule: u8,
    pub past: Vec<dupes::RunInfo>,
    pub similar: Option<Box<similar::Found>>,
    /// Similar folders picked for review.
    pub sim_pick: BTreeSet<u32>,
    /// Same names: the sets (made for `names_for`, in bytes), the one shown,
    /// the ticked files per set, and the name filter.
    pub names: Option<Vec<diskroot_session::NameGroup>>,
    pub names_min: u64,
    pub names_for: u64,
    pub names_sel: usize,
    pub names_pick: Vec<Vec<bool>>,
    pub names_filter: String,
    pub names_shown: Vec<usize>,
    pub names_shown_for: Option<(String, usize)>,
    /// Marked for later (saved on this PC, see session::later), the ticked
    /// ones (lowercase paths), the filter, and which still exist.
    pub later: Vec<later::Later>,
    pub later_pick: BTreeSet<String>,
    pub later_filter: String,
    pub later_exists: HashMap<String, bool>,
    /// Show the set side by side (metadata compare) instead of as a list.
    pub compare: bool,
    /// File details read for the compare view, by lowercase path.
    pub meta: HashMap<String, Meta>,
    /// Full BLAKE3 from "Verify now", by lowercase path (None: unreadable).
    pub hashes: HashMap<String, Option<String>>,
    pub verifying: Option<Receiver<Verified>>,
    /// Lowercase path -> identical set, for the search on screen (and the
    /// search time it was built for).
    pub run_index: HashMap<String, usize>,
    pub run_index_for: Option<(u64, usize)>,
}

impl DupesPage {
    fn new() -> Self {
        Self {
            tab: 0,
            min_mb: 10,
            only_selected: false,
            running: None,
            started: Instant::now(),
            run: None,
            from_past: false,
            keep: Vec::new(),
            remove: Vec::new(),
            sel: 0,
            filter: String::new(),
            kind: 0,
            shown: Vec::new(),
            shown_for: None,
            rule: 0,
            past: dupes::past_runs(),
            similar: None,
            sim_pick: BTreeSet::new(),
            names: None,
            names_min: 10,
            names_for: 0,
            names_sel: 0,
            names_pick: Vec::new(),
            names_filter: String::new(),
            names_shown: Vec::new(),
            names_shown_for: None,
            later: later::load(),
            later_pick: BTreeSet::new(),
            later_filter: String::new(),
            later_exists: HashMap::new(),
            compare: false,
            meta: HashMap::new(),
            hashes: HashMap::new(),
            verifying: None,
            run_index: HashMap::new(),
            run_index_for: None,
        }
    }

    /// Shows `run`: the original-looking copy kept in each set, nothing chosen yet.
    pub(super) fn show(&mut self, run: dupes::Run, from_past: bool) {
        self.keep = vec![0; run.groups.len()];
        self.remove = run.groups.iter().map(|g| vec![false; g.files.len()]).collect();
        self.sel = 0;
        self.shown_for = None;
        self.run = Some(run);
        self.from_past = from_past;
    }
}

/// File kinds for the filter: (name, extensions).
pub(super) const KINDS: [(&str, &[&str]); 5] = [
    ("Photos", &["jpg", "jpeg", "png", "heic", "heif", "gif", "webp", "bmp", "tif", "tiff", "raw", "cr2", "cr3", "nef", "arw", "dng", "orf", "rw2"]),
    ("Videos", &["mp4", "mkv", "mov", "avi", "wmv", "m4v", "webm", "mts", "m2ts", "3gp", "flv"]),
    ("Music", &["mp3", "flac", "wav", "m4a", "aac", "ogg", "wma", "opus", "aiff"]),
    ("Documents", &["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "odt", "ods", "odp", "epub", "rtf", "csv", "md"]),
    ("Archives & installers", &["zip", "7z", "rar", "iso", "exe", "msi", "tar", "gz", "xz", "cab", "img", "dmg", "apk"]),
];

/// 1-based index into KINDS, or KINDS.len() + 1 for anything else.
pub(super) fn kind_of(path: &str) -> u8 {
    let ext = path.rsplit('\\').next().and_then(|n| n.rsplit_once('.')).map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    KINDS.iter().position(|(_, list)| list.contains(&ext.as_str())).map_or(KINDS.len() as u8 + 1, |i| i as u8 + 1)
}

/// Ways to pick the copy to keep in every set.
const KEEP_RULES: [&str; 4] = ["the original-looking one", "the newest", "the oldest", "the one with the shortest path"];

pub(super) fn file_name(path: &str) -> &str {
    path.rsplit('\\').next().unwrap_or(path)
}

pub(super) fn parent_of(path: &str) -> &str {
    path.rsplit_once('\\').map_or("", |(p, _)| p)
}

/// The node for `path` in the scan on screen: `hint` if it still is that
/// path, else looked up by its path below the scan root.
pub(super) fn resolve(s: &Scanned, root: &str, hint: u32, path: &str) -> Option<u32> {
    if s.path(hint).is_ok_and(|p| p.eq_ignore_ascii_case(path)) {
        return Some(hint);
    }
    s.find_rel(&rel_of(root, path)?).filter(|&n| n != 0)
}

pub(super) fn card(t: &Tokens) -> Frame {
    Frame::new().fill(t.panel).stroke(Stroke::new(1.0, t.line)).corner_radius(CornerRadius::same(t.radius + 4)).inner_margin(Margin::same(16))
}

/// A column of exactly `size` at the cursor (allocate_ui_with_layout may
/// grow to the whole remaining height in a row, pushing what's below it
/// off the page).
pub(super) fn fixed_box<R>(ui: &mut Ui, size: egui::Vec2, add: impl FnOnce(&mut Ui) -> R) -> R {
    let rect = Rect::from_min_size(ui.cursor().min, size);
    let r = ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(Layout::top_down(Align::Min)), add).inner;
    ui.advance_cursor_after_rect(rect);
    r
}

/// Text cut to one line with "…" (for rows drawn with the painter).
pub(super) fn one_line(ui: &Ui, text: String, font: egui::FontId, color: Color32, width: f32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple(text, font, color, width.max(20.0));
    job.wrap = egui::text::TextWrapping { max_width: width.max(20.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.painter().layout_job(job)
}

/// A small coloured label.
pub(super) fn chip(ui: &mut Ui, text: &str, fill: Color32, ink: Color32) {
    let font = theme::semibold(theme::ts::MICRO);
    let g = ui.painter().layout_no_wrap(text.to_owned(), font, ink);
    let (r, _) = ui.allocate_exact_size(g.size() + vec2(12.0, 4.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same(6), fill);
    ui.painter().galley(r.min + vec2(6.0, 2.0), g, ink);
}

/// Page title, one line of explanation, and "Back to the map". True when
/// Back was pressed.
pub(super) fn page_header(ui: &mut Ui, t: &Tokens, title: &str, about: &str) -> bool {
    let mut back = false;
    // In a short window the explanation moves into the title's tooltip, so
    // the lists keep their room.
    let short = ui.ctx().content_rect().height() < 700.0;
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            back = ui.button("Back to the map  (Esc)").clicked();
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let size = if short { theme::ts::TITLE } else { theme::ts::DISPLAY };
                let r = ui.add(egui::Label::new(RichText::new(title).font(theme::semibold(size))).truncate());
                if short {
                    r.on_hover_text(about);
                }
            });
        });
    });
    if !short {
        plain_note(ui, RichText::new(about).color(t.muted));
    }
    back
}

/// A row of page tabs; returns the clicked one.
fn tabs(ui: &mut Ui, t: &Tokens, current: u8, names: &[String]) -> Option<u8> {
    let mut out = None;
    ui.horizontal(|ui| {
        for (i, n) in names.iter().enumerate() {
            let on = current as usize == i;
            let text = RichText::new(n).font(if on { theme::semibold(theme::ts::LEAD) } else { theme::regular(theme::ts::LEAD) }).color(if on { t.text } else { t.muted });
            let r = ui.add(egui::Button::new(text).selected(on).min_size(vec2(0.0, 28.0)));
            if r.clicked() && !on {
                out = Some(i as u8);
            }
        }
    });
    out
}

/// What one row of the Review page shows (worked out when the marks change).
pub(super) struct ReviewRow {
    node: u32,
    name: String,
    path: String,
    bytes: u64,
    dir: bool,
    label: &'static str,
    color: Color32,
    what: String,
    age: String,
}

/// The Review marked page: rows for the marks it was built for, and the
/// rows the user unticked (kept).
#[derive(Default)]
pub(super) struct ReviewView {
    rows: Vec<ReviewRow>,
    for_marks: Vec<u32>,
    pub untick: HashSet<u32>,
}

/// The Activity page: both logs read when it opened, and the filter.
pub(super) struct ActivityView {
    entries: Vec<activity::Entry>,
    /// 0 all, 1 deleted, 2 compressed, 3 duplicate searches.
    filter: u8,
    /// What's in the Recycle Bin now, to offer Restore on deletions.
    bin: Vec<diskroot_session::restore::BinItem>,
}

impl DiskRootApp {
    // ------------------------------------------------------------------
    // Shared helpers
    // ------------------------------------------------------------------

    pub(super) fn explorer(&mut self, path: &str) {
        if let Err(e) = diskroot_session::reveal(path) {
            self.say(e);
        }
    }

    /// Leaves a page for the map with `n` selected.
    pub(super) fn jump_to(&mut self, n: u32) {
        self.page = Page::Map;
        if self.settings.by_category {
            self.settings.by_category = false;
            self.regroup();
        }
        self.reveal_node(n, None);
        self.scroll_to_selected = true;
    }

    // ------------------------------------------------------------------
    // Drive status
    // ------------------------------------------------------------------

    /// Opens the Drive status page on `tab` (0 health, 1 who's writing,
    /// 2 size history, 3 past scans).
    pub(super) fn open_status(&mut self, tab: u8) {
        self.status_tab = tab;
        self.page = Page::Status;
        self.prepare_status_tab();
    }

    fn prepare_status_tab(&mut self) {
        match self.status_tab {
            0 if self.health_view.is_none() => self.read_health(),
            1 if self.writers_view.is_none() => self.writers_view = Some(Box::new(WritersView::open())),
            2 => self.growth_view = Some(self.build_growth().map(Box::new)),
            _ => {}
        }
    }

    pub(super) fn status_page(&mut self, ui: &mut Ui) {
        let t = self.tokens();
        let mut back = false;
        let mut go: Option<&'static str> = None;
        Frame::new().inner_margin(Margin { left: 20, right: 20, top: 18, bottom: 0 }).show(ui, |ui| {
            back = page_header(ui, &t, "Your drive status", "Everything about the drive itself in one place: how healthy it is, what writes to it, how it grew, and the scans DiskRoot remembers. DiskRoot only reads here; it changes nothing.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let names = ["Health", "Who's writing", "Size history", "Past scans"].map(String::from);
                if let Some(i) = tabs(ui, &t, self.status_tab, &names) {
                    self.status_tab = i;
                    go = Some("tab");
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.button("Scheduled scans…").on_hover_text("Let DiskRoot scan by itself every few hours, so the history and \"what changed\" fill up").clicked() {
                        go = Some("schedule");
                    }
                    if ui.button("Activity…").on_hover_text("What you deleted and compressed with DiskRoot, and when").clicked() {
                        go = Some("activity");
                    }
                });
            });
            ui.separator();
        });
        if go == Some("tab") {
            self.prepare_status_tab();
        }
        match self.status_tab {
            0 => self.health_body(ui),
            1 => {
                if let Some(mut v) = self.writers_view.take() {
                    ScrollArea::vertical().id_salt("writers-page").auto_shrink([false, false]).show(ui, |ui| {
                        Frame::new().inner_margin(Margin::same(20)).show(ui, |ui| self.writers_body(ui, &mut v));
                    });
                    self.writers_view = Some(v);
                }
            }
            2 => {
                let mut jump: Option<String> = None;
                let mut act: Option<&'static str> = None;
                match self.growth_view.take() {
                    Some(Ok(mut v)) => {
                        ScrollArea::vertical().id_salt("growth-page").auto_shrink([false, false]).show(ui, |ui| {
                            Frame::new().inner_margin(Margin::same(20)).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    if ui.button("Clear history").on_hover_text("Deletes this drive's or folder's size history (DiskRoot's own file). Remembered scans stay.").clicked() {
                                        act = Some("clear");
                                    }
                                });
                                ui.add_space(6.0);
                                growth_body(ui, &t, &mut v, &mut jump);
                            });
                        });
                        self.growth_view = Some(Ok(v));
                    }
                    Some(Err(why)) => {
                        Frame::new().inner_margin(Margin::same(20)).show(ui, |ui| {
                            card(&t).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                plain_note(ui, RichText::new(why.as_str()).color(t.muted));
                            });
                        });
                        self.growth_view = Some(Err(why));
                    }
                    None => {}
                }
                if act == Some("clear")
                    && let Some(Ok(v)) = self.growth_view.take()
                {
                    history::clear(&v.root);
                    self.set_hist(None);
                    self.say(format!("Cleared the size history of {}.", visible(&v.root)));
                    self.growth_view = Some(self.build_growth().map(Box::new));
                }
                if let Some(rel) = jump {
                    self.page = Page::Map;
                    self.show_rel(&rel);
                }
            }
            _ => self.past_scans_body(ui, &t),
        }
        if back {
            self.page = Page::Map;
        }
        match go {
            Some("schedule") => self.open_schedule(),
            Some("activity") => self.open_activity(),
            _ => {}
        }
    }

    /// Who's writing, as the second tab of Drive status.
    fn writers_body(&mut self, ui: &mut Ui, v: &mut WritersView) {
        let t = self.tokens();
        if v.last.elapsed() >= Duration::from_secs(2) {
            v.sampler.tick();
            v.last = Instant::now();
        }
        ui.ctx().request_repaint_after(Duration::from_secs(2));
        let rate = |b: f64| if b < 1024.0 { "0".to_owned() } else { format!("{}/s", fmt_bytes(b as u64)) };
        plain_note(ui, RichText::new("Programs writing right now, measured every 2 s while this tab is open. \"Writes\" counts everything a program writes (files, and also pipes and some network), so the drive's own counter is shown too: when the drive is busy while you're idle and one program leads, that's the one.").color(t.muted));
        ui.add_space(8.0);
        card(&t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (n, letters) in &v.disks {
                let now = v.sampler.disks.iter().find(|d| d.0 == *n).map_or(0.0, |d| d.1);
                let total = v.sampler.disk_totals.get(n).copied().unwrap_or(0);
                ui.label(RichText::new(format!("Drive {letters} · writing {} now · {} since this tab opened", rate(now), fmt_bytes(total))).font(theme::semibold(theme::ts::LEAD)));
            }
            let secs = v.sampler.seconds;
            ui.label(RichText::new(format!("Measured for {:.0} s ({:.0} s of it idle: no key or mouse for 5 min)", secs, v.sampler.idle_seconds)).color(t.faint).size(theme::ts::SMALL));
            ui.add_space(6.0);
            let top = v.sampler.top(8);
            if top.is_empty() {
                ui.label(RichText::new(if secs < 1.0 { "Measuring…" } else { "Nothing written yet." }).color(t.faint));
            } else {
                egui::Grid::new("writers").num_columns(4).striped(true).spacing([18.0, 5.0]).show(ui, |ui| {
                    for h in ["Program", "Now", "While you used the PC", "While idle"] {
                        ui.label(RichText::new(h).color(t.muted).size(theme::ts::SMALL));
                    }
                    ui.end_row();
                    for w in &top {
                        if diskroot_session::offenders::for_process(&w.name).is_some() {
                            ui.label(RichText::new(format!("⚠ {}", w.name)).color(theme::warn()));
                        } else {
                            ui.label(&w.name);
                        }
                        ui.label(rate(w.now));
                        ui.label(fmt_bytes(w.active));
                        ui.label(RichText::new(fmt_bytes(w.idle)).color(if w.idle > 100 << 20 { theme::warn() } else { t.text }));
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
                ui.add(egui::Label::new(RichText::new(format!("⚠ {}: {} {}", o.title, o.what, o.todo)).color(theme::warn())).wrap())
                    .on_hover_text(format!("Checked {} against:\n{}", o.checked, o.sources.join("\n")));
            }
        });
        ui.add_space(12.0);
        ui.columns(2, |cols| {
            card(&t).show(&mut cols[0], |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("While you were away (background scans, last 7 days)").font(theme::semibold(theme::ts::LEAD)));
                ui.add_space(4.0);
                if v.away_runs == 0 {
                    plain_note(ui, RichText::new("No idle samples yet. Each scheduled scan first measures 30 s of writes; turn on Scheduled scans to catch programs that write while nobody is using the PC.").color(t.faint));
                } else {
                    ui.label(RichText::new(format!("{} idle samples of 30 s; average while idle:", v.away_runs)).color(t.muted).size(theme::ts::SMALL));
                    for (name, r, c) in &v.away {
                        plain_note(ui, RichText::new(format!("{name}: {} (≈ {} a day) · in {c} of {} samples", rate(*r), fmt_bytes((*r * 86_400.0) as u64), v.away_runs)));
                    }
                }
            });
            card(&t).show(&mut cols[1], |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new("Folders changed most often").font(theme::semibold(theme::ts::LEAD)));
                ui.add_space(4.0);
                if self.watcher.is_none() {
                    plain_note(ui, RichText::new("Needs live updates on and a scan on screen (it watches the scanned folder while DiskRoot is open).").color(t.faint));
                } else {
                    let mut hot: Vec<(&String, &u32)> = self.hot.iter().collect();
                    hot.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
                    let root = self.summary.as_ref().map(|s| s.root.trim_end_matches('\\').to_owned()).unwrap_or_default();
                    plain_note(ui, RichText::new(format!("Changes Windows reported under {} in the last {} (counts, not bytes):", visible(&root), remember::gap_text(self.hot_since.elapsed().as_secs()))).color(t.muted).size(theme::ts::SMALL));
                    if hot.is_empty() {
                        ui.label(RichText::new("None yet.").color(t.faint));
                    }
                    for (folder, n) in hot.into_iter().take(8) {
                        let shown = if folder.is_empty() { root.clone() } else { format!("{root}\\{folder}") };
                        ui.add(egui::Label::new(format!("{n} × {}", visible(&shown))).truncate());
                    }
                }
            });
        });
        ui.add_space(12.0);
        card(&t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Settings that write a lot (advice only: DiskRoot changes nothing)").font(theme::semibold(theme::ts::LEAD)));
            ui.add_space(4.0);
            for tip in &v.advice {
                plain_note(ui, RichText::new(format!("• {tip}")).color(t.muted));
            }
            if v.advice.is_empty() {
                ui.label(RichText::new("Nothing to point out.").color(t.faint));
            }
        });
    }

    /// Every scan DiskRoot remembers for the root on screen (its size history).
    fn past_scans_body(&mut self, ui: &mut Ui, t: &Tokens) {
        let root = self.summary.as_ref().map(|s| s.root.clone());
        let h = self.hist.clone().or_else(|| root.as_deref().and_then(history::load).map(Arc::new));
        let mut jump: Option<String> = None;
        ScrollArea::vertical().id_salt("past-scans").auto_shrink([false, false]).show(ui, |ui| {
            Frame::new().inner_margin(Margin::same(20)).show(ui, |ui| {
                let Some(root) = root else {
                    plain_note(ui, RichText::new("Scan a drive or folder first: its past scans are listed here.").color(t.faint));
                    return;
                };
                let Some(h) = h.filter(|h| !h.points.is_empty()) else {
                    card(t).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        plain_note(ui, RichText::new(format!("No past scans of {} yet. DiskRoot remembers each scan (folders of 1 MB or more, on this PC only); scan again later or turn on Scheduled scans.", visible(&root))).color(t.muted));
                    });
                    return;
                };
                plain_note(ui, RichText::new(format!("{} scans of {} remembered on this PC, newest first. Times are this PC's local time. Click a change to see the folders that changed between two scans.", h.points.len(), visible(&root))).color(t.muted));
                ui.add_space(8.0);
                card(t).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::Grid::new("past-scans-grid").num_columns(6).striped(true).spacing([18.0, 6.0]).show(ui, |ui| {
                        for head in ["When", "How", "Size", "Files", "Change", "Drive"] {
                            ui.label(RichText::new(head).color(t.muted).size(theme::ts::SMALL));
                        }
                        ui.end_row();
                        let n = h.points.len();
                        for i in (0..n).rev() {
                            let p = &h.points[i];
                            ui.label(RichText::new(activity::local_text(p.time)).font(theme::num(theme::ts::SMALL)));
                            ui.label(RichText::new(history::method_text(p.method)).color(t.muted));
                            ui.label(fmt_bytes(p.total));
                            ui.label(RichText::new(fmt_count(p.files)).color(t.muted));
                            if i > 0 {
                                let d = i128::from(p.total) - i128::from(h.points[i - 1].total);
                                let r = ui.add(egui::Label::new(RichText::new(fmt_delta(d)).color(if d > 0 { theme::warn() } else { t.accent })).sense(Sense::click()));
                                let biggest = p.changes.iter().filter_map(|(id, b)| h.names.get(*id as usize).map(|n| (n, *b))).max_by_key(|x| x.1);
                                let tip = match biggest {
                                    Some((name, _)) => format!("{} folders changed since the scan before. One of the biggest: {}\nClick to show it in the tree", p.changes.len(), if name.is_empty() { visible(&root) } else { name.clone() }),
                                    None => "No folder changed by 1 MB or more".to_owned(),
                                };
                                if r.on_hover_text(tip).clicked()
                                    && let Some((name, _)) = biggest
                                {
                                    jump = Some(name.clone());
                                }
                            } else {
                                ui.label(RichText::new("first").color(t.faint));
                            }
                            ui.label(RichText::new(p.used().map_or_else(String::new, |u| format!("{} used · {} free", fmt_bytes(u), fmt_bytes(p.vol_free)))).color(t.muted));
                            ui.end_row();
                        }
                    });
                });
            });
        });
        if let Some(rel) = jump {
            self.page = Page::Map;
            self.show_rel(&rel);
        }
    }

    // ------------------------------------------------------------------
    // Duplicates
    // ------------------------------------------------------------------

    /// Opens the Duplicates page (0 identical files, 1 similar folders, 2 past searches).
    pub(super) fn open_dupes_tab(&mut self, tab: u8) {
        if self.scanned.is_none() {
            return;
        }
        let d = self.dupes.get_or_insert_with(|| Box::new(DupesPage::new()));
        d.tab = tab;
        if tab == 2 {
            d.past = dupes::past_runs();
        }
        if tab == 1 {
            d.similar = None;
        }
        self.page = Page::Dupes;
        self.prepare_dupes_tab();
    }

    fn start_dupes(&mut self) {
        let (Some(s), Some(d)) = (&self.scanned, &mut self.dupes) else { return };
        let scan_root = s.summary().root;
        let root = match (&self.selected, d.only_selected) {
            (Some(Key_::Node(n)), true) => s.info(*n).ok().filter(|i| i.dir).map_or(scan_root.clone(), |i| i.path),
            _ => scan_root.clone(),
        };
        let min = d.min_mb << 20;
        let mut cands = s.duplicate_candidates(min);
        if root != scan_root {
            for (_, files) in &mut cands {
                files.retain(|(_, p)| rel_of(&root, p).is_some());
            }
            cands.retain(|(_, f)| f.len() > 1);
        }
        let prog = Arc::new(dupes::Progress::default());
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (p2, c2) = (Arc::clone(&prog), Arc::clone(&cancel));
        let ctx = self.egui_ctx.clone();
        std::thread::spawn(move || {
            let t0 = Instant::now();
            let n: u64 = cands.iter().map(|c| c.1.len() as u64).sum();
            let cache = dupes::load_cache(&root);
            let threads = dupes::threads_for(&root);
            let (groups, new_cache) = dupes::find_with(cands, &c2, &p2, threads, &cache);
            let run = dupes::Run {
                time: activity::now(),
                root,
                min_size: min,
                secs: t0.elapsed().as_secs_f64(),
                read: p2.read.load(Ordering::Relaxed),
                reused: p2.reused.load(Ordering::Relaxed),
                candidates: n,
                groups,
            };
            if !c2.load(Ordering::Relaxed) {
                let _ = dupes::save(&run, &new_cache);
                activity::append(&activity::Logged {
                    time: run.time,
                    kind: "duplicates".into(),
                    path: run.root.clone(),
                    files: run.groups.iter().map(|g| g.files.len() as u64).sum(),
                    before: run.wasted(),
                    after: 0,
                    note: format!("{} sets of identical files", run.groups.len()),
                });
            }
            let _ = tx.send(run);
            ctx.request_repaint();
        });
        d.running = Some((prog, cancel, rx));
        d.started = Instant::now();
        d.run = None;
    }

    /// A finished search arrives (also while another page is shown).
    pub(super) fn poll_dupes(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.dupes else { return };
        let Some((_, cancel, rx)) = &d.running else { return };
        match rx.try_recv() {
            Ok(run) => {
                let stopped = cancel.load(Ordering::Relaxed);
                d.running = None;
                if stopped {
                    self.say("Duplicate search stopped. Nothing was saved.");
                    return;
                }
                let (n, w) = (run.groups.len(), run.wasted());
                d.show(run, false);
                d.past = dupes::past_runs();
                if self.page != Page::Dupes {
                    self.say(format!("Duplicate search done: {n} sets, {} could be freed. Open Duplicates to resolve them.", fmt_bytes(w)));
                }
            }
            Err(TryRecvError::Disconnected) => {
                d.running = None;
                self.say("The duplicate search stopped because of a bug in DiskRoot. Nothing was changed.");
            }
            Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(150)),
        }
    }

    pub(super) fn dupes_page(&mut self, ui: &mut Ui) {
        let t = self.tokens();
        let Some(mut d) = self.dupes.take() else { return };
        triage::poll_verify(ui.ctx(), &mut d);
        let mut back = false;
        let mut switched = false;
        Frame::new().inner_margin(Margin { left: 20, right: 20, top: 18, bottom: 0 }).show(ui, |ui| {
            back = page_header(ui, &t, "Duplicates & same names", "Identical files (same bytes, BLAKE3) and files that only share a name. Compare them, delete, or mark them for later. Nothing is deleted without a confirmation.");
            ui.add_space(8.0);
            let sets = d.run.as_ref().map_or(String::new(), |r| format!(" ({})", r.groups.len()));
            let same = d.names.as_ref().map_or(String::new(), |n| format!(" ({})", n.len()));
            // Shown order; d.tab keeps its old numbers (0 identical, 1 similar, 2 past, 3 same names, 4 later).
            const ORDER: [u8; 5] = [0, 3, 1, 4, 2];
            let names = [
                format!("Identical files{sets}"),
                format!("Same names{same}"),
                "Similar folders".to_owned(),
                format!("Marked for later ({})", d.later.len()),
                format!("Past searches ({})", d.past.len()),
            ];
            let cur = ORDER.iter().position(|&o| o == d.tab).unwrap_or(0) as u8;
            if let Some(i) = tabs(ui, &t, cur, &names) {
                d.tab = ORDER[i as usize];
                switched = true;
                if d.tab == 2 {
                    d.past = dupes::past_runs();
                }
            }
            ui.separator();
        });
        let mut out: Option<DupesGo> = None;
        Frame::new().inner_margin(Margin { left: 20, right: 20, top: 8, bottom: 12 }).show(ui, |ui| match d.tab {
            0 => self.identical_tab(ui, &t, &mut d, &mut out),
            1 => self.similar_tab(ui, &t, &mut d, &mut out),
            3 => self.names_tab(ui, &t, &mut d, &mut out),
            4 => self.later_tab(ui, &t, &mut d, &mut out),
            _ => past_tab(ui, &t, &mut d, &mut out),
        });
        self.dupes = Some(d);
        if switched {
            self.prepare_dupes_tab();
        }
        if back {
            self.page = Page::Map;
        }
        match out {
            Some(DupesGo::Start) => self.start_dupes(),
            Some(DupesGo::NewSearch) => {
                if let Some(d) = &mut self.dupes {
                    d.run = None;
                }
            }
            Some(DupesGo::Explorer(p)) => self.explorer(&p),
            Some(DupesGo::Preview(p)) => {
                if let Err(e) = diskroot_session::open_file(&p) {
                    self.say(e);
                }
            }
            Some(DupesGo::Map(n)) => self.jump_to(n),
            Some(DupesGo::Review(items)) => self.review_these(items),
            Some(DupesGo::Open(file)) => match dupes::load_run(&file) {
                Some(run) => {
                    if let Some(d) = &mut self.dupes {
                        d.show(run, true);
                        d.tab = 0;
                    }
                }
                None => self.say("That saved search couldn't be read."),
            },
            Some(DupesGo::Forget(file)) => {
                dupes::forget(&file);
                if let Some(d) = &mut self.dupes {
                    d.past = dupes::past_runs();
                }
            }
            Some(DupesGo::Verify(paths)) => self.verify_files(paths),
            Some(DupesGo::Later(items)) => self.mark_later(items),
            Some(DupesGo::Unlater(paths)) => self.unmark_later(&paths),
            Some(DupesGo::DeleteNow(nodes)) => self.ask_delete_nodes(nodes),
            Some(DupesGo::Names) => {
                if let Some(d) = &mut self.dupes {
                    d.names = None;
                }
                self.prepare_dupes_tab();
            }
            None => {}
        }
    }

    fn identical_tab(&mut self, ui: &mut Ui, t: &Tokens, d: &mut DupesPage, out: &mut Option<DupesGo>) {
        if let Some((p, cancel, _)) = &d.running {
            dupes_progress(ui, t, p, cancel, d.started);
            return;
        }
        let Some(s) = &self.scanned else { return };
        let scan_root = s.summary().root;
        if d.run.is_none() {
            // Intro: how it works, the options, Start.
            ScrollArea::vertical().id_salt("dupes-intro").auto_shrink([false, false]).show(ui, |ui| {
                ui.columns(3, |cols| {
                    let steps = [
                        ("1", "Same size", "Files with exactly the same size are listed from the scan. Nothing is opened yet, so this is instant."),
                        ("2", "Quick check", "The first and last 64 KB of each are compared. Most look-alikes already differ here."),
                        ("3", "Full compare", "Only files still matching are read in full and compared byte for byte (BLAKE3)."),
                    ];
                    for (col, (n, title, text)) in cols.iter_mut().zip(steps) {
                        card(t).show(col, |ui| {
                            ui.set_width(ui.available_width());
                            ui.set_min_height(110.0);
                            ui.horizontal(|ui| {
                                chip(ui, n, t.accent, t.accent_ink);
                                ui.label(RichText::new(title).font(theme::semibold(theme::ts::SECTION)));
                            });
                            ui.add_space(4.0);
                            plain_note(ui, RichText::new(text).color(t.muted));
                        });
                    }
                });
                ui.add_space(12.0);
                card(t).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Search").font(theme::semibold(theme::ts::SECTION)));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("Only files of at least");
                        ui.add(egui::DragValue::new(&mut d.min_mb).range(1..=100_000).suffix(" MB"));
                        for mb in [1u64, 10, 100, 1000] {
                            if ui.selectable_label(d.min_mb == mb, if mb >= 1000 { "1 GB".to_owned() } else { format!("{mb} MB") }).clicked() {
                                d.min_mb = mb;
                            }
                        }
                    });
                    let sel_dir = match &self.selected {
                        Some(Key_::Node(n)) if *n != 0 => s.info(*n).ok().filter(|i| i.dir).map(|i| i.path),
                        _ => None,
                    };
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut d.only_selected, false, format!("The whole scan ({})", visible(&scan_root)));
                        match &sel_dir {
                            Some(p) => {
                                ui.radio_value(&mut d.only_selected, true, format!("Only {}", visible(p)));
                            }
                            None => {
                                d.only_selected = false;
                                ui.label(RichText::new("(select a folder in the list to search only there)").color(t.faint).size(theme::ts::SMALL));
                            }
                        }
                    });
                    let cands = s.duplicate_candidates(d.min_mb << 20);
                    let (n, sizes) = (cands.iter().map(|c| c.1.len()).sum::<usize>(), cands.len());
                    ui.add_space(4.0);
                    plain_note(ui, RichText::new(format!("{} files share a size with another file ({} different sizes) in the whole scan. Those are the only ones that get read.", fmt_count(n as u64), fmt_count(sizes as u64))).color(t.muted).size(theme::ts::SMALL));
                    ui.add_space(8.0);
                    let go = egui::Button::new(RichText::new("Find duplicates").color(t.accent_ink).font(theme::semibold(theme::ts::LEAD))).fill(t.accent).min_size(vec2(160.0, 32.0));
                    if ui.add_enabled(n > 1, go).clicked() {
                        *out = Some(DupesGo::Start);
                    }
                });
                ui.add_space(12.0);
                if let Some(last) = d.past.iter().find(|r| r.root.eq_ignore_ascii_case(&scan_root)) {
                    card(t).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("Last search here: {} · {} sets · {} could be freed", activity::local_text(last.time), last.sets, fmt_bytes(last.wasted))).font(theme::semibold(theme::ts::BODY)));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Open it").clicked() {
                                    *out = Some(DupesGo::Open(last.file.clone()));
                                }
                            });
                        });
                        plain_note(ui, RichText::new("A new search reuses what the last one read: files that haven't changed since aren't read again, so it's much faster.").color(t.faint).size(theme::ts::SMALL));
                    });
                    ui.add_space(12.0);
                }
                plain_note(ui, RichText::new("This is the only part of DiskRoot that reads file contents. Nothing leaves your PC. Online-only cloud files are skipped (reading them would download them), hardlinks (one file with two names) aren't counted as copies, and on an SSD several files are read at once.").color(t.faint).size(theme::ts::SMALL));
            });
            return;
        }
        let Some(run) = &d.run else { return };
        // Results: summary line, filters, the sets on the left, one set on the right.
        let wasted = run.wasted();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} sets · {} could be freed by keeping one copy of each", fmt_count(run.groups.len() as u64), fmt_bytes(wasted))).font(theme::semibold(theme::ts::SECTION)));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("New search…").clicked() {
                    *out = Some(DupesGo::NewSearch);
                }
                if ui.button("Search again").on_hover_text("Same settings; unchanged files aren't read again").clicked() {
                    *out = Some(DupesGo::Start);
                }
            });
        });
        let reused = if run.reused > 0 { format!(" · {} files reused from the last search", fmt_count(run.reused)) } else { String::new() };
        let past = if d.from_past { " · saved search: the files may have changed since" } else { "" };
        ui.label(RichText::new(format!("{} · searched {} · {} files compared, {} read in {:.1} s{reused}{past}", activity::local_text(run.time), visible(&run.root), fmt_count(run.candidates), fmt_bytes(run.read), run.secs)).color(t.faint).size(theme::ts::SMALL));
        if run.groups.is_empty() {
            ui.add_space(20.0);
            ui.label(RichText::new("No identical files found. Try a smaller minimum size.").color(t.muted));
            return;
        }
        ui.add_space(6.0);
        // Filters and "choose for every set".
        ui.horizontal(|ui| {
            ui.add(TextEdit::singleline(&mut d.filter).hint_text("Filter by name or folder").desired_width(200.0));
            if ui.selectable_label(d.kind == 0, "All").clicked() {
                d.kind = 0;
            }
            for (i, (name, _)) in KINDS.iter().enumerate() {
                if ui.selectable_label(d.kind == i as u8 + 1, *name).clicked() {
                    d.kind = i as u8 + 1;
                }
            }
            if ui.selectable_label(d.kind == KINDS.len() as u8 + 1, "Other").clicked() {
                d.kind = KINDS.len() as u8 + 1;
            }
        });
        let key = (d.filter.to_lowercase(), d.kind, run.groups.len());
        if d.shown_for.as_ref() != Some(&key) {
            d.shown = (0..run.groups.len())
                .filter(|&i| {
                    let g = &run.groups[i];
                    (key.1 == 0 || g.files.first().is_some_and(|f| kind_of(&f.1) == key.1)) && (key.0.is_empty() || g.files.iter().any(|f| f.1.to_lowercase().contains(&key.0)))
                })
                .collect();
            if !d.shown.contains(&d.sel) {
                d.sel = d.shown.first().copied().unwrap_or(0);
            }
            d.shown_for = Some(key);
        }
        ui.horizontal(|ui| {
            ui.label("In every shown set, keep");
            egui::ComboBox::from_id_salt("keep-rule").selected_text(KEEP_RULES[d.rule as usize]).show_ui(ui, |ui| {
                for (i, r) in KEEP_RULES.iter().enumerate() {
                    ui.selectable_value(&mut d.rule, i as u8, *r);
                }
            });
            let choose = egui::Button::new(RichText::new(format!("Choose the other copies ({} sets)", d.shown.len())).color(t.accent_ink)).fill(t.accent);
            if ui.add(choose).on_hover_text("Picks the copy to keep in each shown set and ticks the others for removal. Nothing is deleted: you review them next.").clicked() {
                let today = diskroot_session::today_days();
                for &gi in &d.shown {
                    let g = &run.groups[gi];
                    let age = |i: usize| resolve(s, &scan_root, g.files[i].0, &g.files[i].1).map_or(today, |n| s.mtime(n));
                    let k = match d.rule {
                        1 => (0..g.files.len()).max_by_key(|&i| (age(i), std::cmp::Reverse(i))).unwrap_or(0),
                        2 => (0..g.files.len()).min_by_key(|&i| (age(i), i)).unwrap_or(0),
                        3 => (0..g.files.len()).min_by_key(|&i| (g.files[i].1.len(), i)).unwrap_or(0),
                        _ => 0,
                    };
                    d.keep[gi] = k;
                    d.remove[gi] = (0..g.files.len()).map(|i| i != k).collect();
                }
            }
            if ui.button("Clear choices").clicked() {
                for r in &mut d.remove {
                    r.iter_mut().for_each(|x| *x = false);
                }
            }
        });
        ui.add_space(6.0);
        // Two columns: sets | the selected set.
        let bottom = 52.0;
        let h = (ui.available_height() - bottom).max(160.0);
        let w = ui.available_width();
        let left_w = (w * 0.38).clamp(260.0, 460.0);
        ui.horizontal_top(|ui| {
            fixed_box(ui, vec2(left_w, h), |ui| {
                card(t).inner_margin(Margin::same(6)).show(ui, |ui| {
                    ui.set_min_size(vec2(left_w - 14.0, h - 14.0));
                    ui.set_max_width(left_w - 14.0);
                    if d.shown.is_empty() {
                        ui.label(RichText::new("No set matches the filter.").color(t.faint));
                    }
                    ScrollArea::vertical().id_salt("dupe-sets").auto_shrink([false, false]).show_rows(ui, 40.0, d.shown.len(), |ui, range| {
                        for gi in d.shown[range].iter().copied() {
                            let g = &run.groups[gi];
                            let chosen = d.remove[gi].iter().filter(|x| **x).count();
                            let on = d.sel == gi;
                            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                            let p = ui.painter();
                            if on {
                                p.rect_filled(r, CornerRadius::same(6), t.head);
                            } else if resp.hovered() {
                                p.rect_filled(r, CornerRadius::same(6), t.field);
                            }
                            let first = g.files.first().map_or("", |f| file_name(&f.1));
                            let name = one_line(ui, visible(first), theme::semibold(theme::ts::SMALL), t.text, r.width() - 110.0);
                            let p = ui.painter();
                            p.galley(r.min + vec2(8.0, 3.0), name, t.text);
                            p.text(r.min + vec2(8.0, 21.0), Align2::LEFT_TOP, format!("{} copies × {}", g.files.len(), fmt_bytes(g.size)), theme::regular(theme::ts::CAPTION), t.muted);
                            p.text(pos2(r.max.x - 8.0, r.min.y + 4.0), Align2::RIGHT_TOP, format!("+{}", fmt_bytes(g.wasted())), theme::num(theme::ts::SMALL), t.accent);
                            if chosen > 0 {
                                p.text(pos2(r.max.x - 8.0, r.min.y + 21.0), Align2::RIGHT_TOP, format!("{chosen} chosen"), theme::regular(theme::ts::MICRO), theme::warn());
                            }
                            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, first));
                            if resp.clicked() {
                                d.sel = gi;
                            }
                        }
                    });
                });
            });
            ui.add_space(8.0);
            let rw = ui.available_width();
            fixed_box(ui, vec2(rw, h), |ui| {
                card(t).show(ui, |ui| {
                    ui.set_min_size(vec2(rw - 34.0, h - 34.0));
                    let Some(g) = run.groups.get(d.sel) else { return };
                    let gi = d.sel;
                    let paths: Vec<String> = g.files.iter().map(|f| f.1.clone()).collect();
                    triage::set_header(ui, visible(g.files.first().map_or("", |f| file_name(&f.1))), ("IDENTICAL · BLAKE3", t.accent), &mut d.compare, d.verifying.is_some(), &paths, out);
                    ui.label(RichText::new(format!("{} identical copies of {} · keeping one frees {}", g.files.len(), fmt_bytes(g.size), fmt_bytes(g.wasted()))).color(t.muted))
                        .on_hover_text("Identical means byte for byte the same, not just the same name. The kept copy is suggested from its name and folder (not \"copy\" or \"(1)\", not in a backup, old or Downloads folder); pick another if that's the one you use.");
                    ui.add_space(6.0);
                    if d.compare {
                        let rows: Vec<triage::CmpRow> = g
                            .files
                            .iter()
                            .map(|(hint, path)| triage::CmpRow {
                                path: path.clone(),
                                size: g.size,
                                node: resolve(s, &scan_root, *hint, path),
                                content: triage::identical_status(&d.hashes, &paths, path, t),
                                later: later::contains(&d.later, path),
                            })
                            .collect();
                        triage::compare_grid(ui, t, ("cmp-dupe", gi), &rows, &mut d.meta, out);
                        return;
                    }
                    ScrollArea::vertical().id_salt(("dupe-set", gi)).auto_shrink([false, false]).show(ui, |ui| {
                        for (i, (hint, path)) in g.files.iter().enumerate() {
                            let node = resolve(s, &scan_root, *hint, path);
                            let kept = d.keep[gi] == i;
                            Frame::new().fill(if kept { t.field } else { Color32::TRANSPARENT }).stroke(Stroke::new(1.0, if kept { t.accent } else { t.line })).corner_radius(CornerRadius::same(t.radius)).inner_margin(Margin::symmetric(10, 5)).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    if ui.radio(kept, "Keep").on_hover_text("Keep this copy; the others can be removed").clicked() {
                                        d.keep[gi] = i;
                                        d.remove[gi][i] = false;
                                    }
                                    let mut rm = d.remove[gi][i];
                                    if ui.add_enabled(!kept && node.is_some(), egui::Checkbox::new(&mut rm, "Remove")).on_hover_text("Choose this copy for the Recycle Bin (you review it first)").changed() {
                                        d.remove[gi][i] = rm;
                                    }
                                    ui.add_space(6.0);
                                    // Buttons on the right first, so the name is shortened instead.
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        let flagged = later::contains(&d.later, path);
                                        if triage::later_button(ui, t, flagged) {
                                            *out = Some(if flagged {
                                                DupesGo::Unlater(vec![path.clone()])
                                            } else {
                                                DupesGo::Later(vec![triage::later_item(path, g.size, format!("Identical copy of {}", visible(&g.files[d.keep[gi]].1)))])
                                            });
                                        }
                                        if let Some(n) = node
                                            && ui.small_button("Map").on_hover_text("Show it on the map").clicked()
                                        {
                                            *out = Some(DupesGo::Map(n));
                                        }
                                        if ui.small_button("Explorer").on_hover_text("Opens the folder with this file selected: open it, compare, or rename it there").clicked() {
                                            *out = Some(DupesGo::Explorer(path.clone()));
                                        }
                                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                            ui.vertical(|ui| {
                                                let when = node.map(|n| s.mtime(n)).filter(|&m| m > 0).map(|m| format!("modified {}", diskroot_session::date_text(m)));
                                                let mut bits: Vec<String> = vec![visible(parent_of(path))];
                                                bits.extend(when);
                                                if dupes::copy_score(path) > 0 {
                                                    bits.push("looks like a copy".into());
                                                }
                                                if node.is_none() {
                                                    bits.push("no longer in this scan".into());
                                                }
                                                if later::contains(&d.later, path) {
                                                    bits.push("marked for later".into());
                                                }
                                                ui.add(egui::Label::new(RichText::new(visible(file_name(path))).font(theme::semibold(theme::ts::SMALL))).truncate()).on_hover_text(visible(path));
                                                ui.add(egui::Label::new(RichText::new(bits.join(" · ")).color(t.muted).size(theme::ts::CAPTION)).truncate()).on_hover_text(visible(path));
                                            });
                                        });
                                    });
                                });
                            });
                            ui.add_space(4.0);
                        }
                    });
                });
            });
        });
        // Bottom bar: what's chosen, and Review.
        let (mut n, mut bytes) = (0usize, 0u64);
        for (gi, g) in run.groups.iter().enumerate() {
            let c = d.remove[gi].iter().filter(|x| **x).count();
            n += c;
            bytes += g.size * c as u64;
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(if n == 0 { "Nothing chosen yet. Tick copies to remove, or use \"Choose the other copies\".".to_owned() } else { format!("{n} copies chosen · {} would be freed", fmt_bytes(bytes)) }).font(theme::semibold(theme::ts::BODY)).color(if n == 0 { t.muted } else { t.text }));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let chosen = || {
                    let mut v = Vec::new();
                    for (gi, g) in run.groups.iter().enumerate() {
                        for (i, (hint, path)) in g.files.iter().enumerate() {
                            if d.remove[gi][i] {
                                v.push((gi, *hint, path.clone()));
                            }
                        }
                    }
                    v
                };
                let del = egui::Button::new(RichText::new(format!("Delete {n} now…")).color(Color32::WHITE).font(theme::semibold(theme::ts::BODY))).fill(theme::cat_color(5)).min_size(vec2(120.0, 30.0));
                if ui.add_enabled(n > 0, del).on_hover_text("Straight to the confirmation: Recycle Bin by default, or permanently if you tick that there").clicked() {
                    *out = Some(DupesGo::DeleteNow(chosen().into_iter().filter_map(|(_, h, p)| resolve(s, &scan_root, h, &p)).collect()));
                }
                if ui.add_enabled(n > 0, egui::Button::new(format!("Mark {n} for later")).min_size(vec2(0.0, 30.0))).on_hover_text("Not sure yet? Keeps them on the \"Marked for later\" list (saved on this PC, also after a restart). Nothing is deleted.").clicked() {
                    *out = Some(DupesGo::Later(
                        chosen().into_iter().map(|(gi, _, p)| triage::later_item(&p, run.groups[gi].size, format!("Identical copy of {}", visible(&run.groups[gi].files[d.keep[gi]].1)))).collect(),
                    ));
                }
                let b = egui::Button::new(RichText::new(format!("Review {n} and remove…")).color(t.accent_ink).font(theme::semibold(theme::ts::BODY))).fill(t.accent).min_size(vec2(170.0, 30.0));
                if ui.add_enabled(n > 0, b).on_hover_text("Opens Review marked: check each one, untick what to keep, then move the rest to the Recycle Bin").clicked() {
                    let mut items = Vec::new();
                    for (gi, g) in run.groups.iter().enumerate() {
                        let kept = g.files[d.keep[gi]].1.clone();
                        for (i, (hint, path)) in g.files.iter().enumerate() {
                            if d.remove[gi][i] {
                                items.push((resolve(s, &scan_root, *hint, path), Some(kept.clone())));
                            }
                        }
                    }
                    *out = Some(DupesGo::Review(items));
                }
            });
        });
    }

    fn similar_tab(&mut self, ui: &mut Ui, t: &Tokens, d: &mut DupesPage, out: &mut Option<DupesGo>) {
        let Some(s) = &self.scanned else { return };
        let f = d.similar.get_or_insert_with(|| Box::new(s.similar_folders()));
        plain_note(ui, RichText::new(format!(
            "{} folders looked at in {:.2} s. Likely copies have the same size, file count and names inside (contents aren't compared). Similar names share a name once case, versions and copy marks are ignored. Open one in Explorer to compare or tidy it there, or tick folders to review them for the Recycle Bin.",
            fmt_count(f.folders),
            f.secs
        )).color(t.muted));
        ui.add_space(6.0);
        let h = (ui.available_height() - 46.0).max(160.0);
        let picks = &mut d.sim_pick;
        ScrollArea::vertical().id_salt("similar-page").auto_shrink([false, false]).max_height(h).show(ui, |ui| {
            let rows = |ui: &mut Ui, g: &similar::Group, picks: &mut BTreeSet<u32>, out: &mut Option<DupesGo>| {
                for m in &g.members {
                    ui.horizontal(|ui| {
                        let mut on = picks.contains(&m.node);
                        if ui.checkbox(&mut on, "").on_hover_text("Tick to review it for the Recycle Bin").changed() {
                            if on {
                                picks.insert(m.node);
                            } else {
                                picks.remove(&m.node);
                            }
                        }
                        ui.add_sized(vec2(72.0, 18.0), egui::Label::new(RichText::new(fmt_bytes(m.bytes)).color(t.muted)));
                        let age = if m.mtime > 0 { diskroot_session::age_text(diskroot_session::today_days(), m.mtime) } else { String::new() };
                        ui.add_sized(vec2(90.0, 18.0), egui::Label::new(RichText::new(age).color(t.faint).size(theme::ts::CAPTION)));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("Show on map").clicked() {
                                *out = Some(DupesGo::Map(m.node));
                            }
                            if ui.small_button("Open in Explorer").on_hover_text("Opens this folder in Explorer, so you can compare and change it there").clicked() {
                                *out = Some(DupesGo::Explorer(m.path.clone()));
                            }
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                ui.add(egui::Label::new(visible(&m.path)).truncate()).on_hover_text(format!("{} · {}", visible(&m.path), fmt_files(u64::from(m.files))));
                            });
                        });
                    });
                }
            };
            let freeable: u64 = f.copies.iter().map(|g| g.reclaim).sum();
            ui.label(RichText::new(format!("Likely copies ({}) · up to {} could be freed", f.copies.len(), fmt_bytes(freeable))).font(theme::semibold(theme::ts::LEAD)));
            if f.copies.is_empty() {
                ui.label(RichText::new("None found.").color(t.faint));
            }
            for (gi, g) in f.copies.iter().enumerate() {
                egui::CollapsingHeader::new(format!("{} · {} copies · {} · {} could be freed", visible(&g.label), g.members.len(), fmt_bytes(g.bytes), fmt_bytes(g.reclaim)))
                    .id_salt(("copies", gi))
                    .default_open(gi < 3)
                    .show(ui, |ui| rows(ui, g, picks, out));
            }
            ui.add_space(10.0);
            ui.label(RichText::new(format!("Similar names ({})", f.names.len())).font(theme::semibold(theme::ts::LEAD)));
            if f.names.is_empty() {
                ui.label(RichText::new("None found.").color(t.faint));
            }
            for (gi, g) in f.names.iter().enumerate() {
                egui::CollapsingHeader::new(format!("{}… · {} folders · {}", visible(&g.label), g.members.len(), fmt_bytes(g.bytes)))
                    .id_salt(("names", gi))
                    .show(ui, |ui| rows(ui, g, picks, out));
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let n = d.sim_pick.len();
            ui.label(RichText::new("Two folders can look alike and still differ inside: check before removing one.").color(t.muted).size(theme::ts::SMALL));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let b = egui::Button::new(RichText::new(format!("Review {n} folders…")).color(t.accent_ink)).fill(t.accent).min_size(vec2(150.0, 30.0));
                if ui.add_enabled(n > 0, b).clicked() {
                    *out = Some(DupesGo::Review(d.sim_pick.iter().map(|&n| (Some(n), None)).collect()));
                    d.sim_pick.clear();
                }
            });
        });
    }

    // ------------------------------------------------------------------
    // Review marked
    // ------------------------------------------------------------------

    /// Adds `items` (node, kept copy for duplicates) to the marks and opens Review.
    fn review_these(&mut self, items: Vec<(Option<u32>, Option<String>)>) {
        let gone = items.iter().filter(|i| i.0.is_none()).count();
        for (n, kept) in items {
            if let Some(n) = n {
                self.marked.insert(n);
                if let Some(k) = kept {
                    self.dupe_of.insert(n, k);
                }
            }
        }
        if self.settings.by_category {
            self.settings.by_category = false;
            self.regroup();
        }
        self.rows = None;
        if gone > 0 {
            self.say(format!("{gone} files are no longer in this scan (moved or deleted since) and were left out."));
        }
        self.open_review();
    }

    pub(super) fn open_review(&mut self) {
        if self.scanned.is_some() {
            self.page = Page::Review;
        }
    }

    fn review_rows(&mut self) {
        let marks: Vec<u32> = self.marked.iter().copied().collect();
        if self.review.for_marks == marks {
            return;
        }
        let Some(s) = &self.scanned else { return };
        let t = self.tokens();
        let today = diskroot_session::today_days();
        let mut rows: Vec<ReviewRow> = marks
            .iter()
            .filter_map(|&n| {
                let i = s.info(n).ok()?;
                let rule = s.rules().info.get(i.rule as usize);
                let (label, color) = match s.safety(n) {
                    None if rule.is_some_and(|r| r.user) => ("Your rule", t.muted),
                    Some(Safety::Green) => ("Safe to clear", theme::cat_color(2)),
                    Some(Safety::Yellow) => ("Check first", theme::cat_color(6)),
                    Some(Safety::Red) => ("Don't delete by hand", theme::cat_color(5)),
                    None => ("Not labelled", t.muted),
                };
                let what = match self.dupe_of.get(&n) {
                    Some(k) => format!("Identical copy of {}", visible(k)),
                    None => rule.and_then(|r| r.note.clone()).unwrap_or_else(|| self.cat_label(i.cat, i.sub)),
                };
                let m = s.mtime(n);
                Some(ReviewRow {
                    node: n,
                    name: i.name,
                    path: i.path,
                    bytes: i.on_disk,
                    dir: i.dir,
                    label,
                    color,
                    what,
                    age: if m > 0 { diskroot_session::age_text(today, m) } else { String::new() },
                })
            })
            .collect();
        rows.sort_by_key(|r| (std::cmp::Reverse(r.bytes), r.node));
        self.review.untick.retain(|n| marks.contains(n));
        self.review.rows = rows;
        self.review.for_marks = marks;
    }

    pub(super) fn review_page(&mut self, ui: &mut Ui) {
        let t = self.tokens();
        self.review_rows();
        let mut back = false;
        let mut go: Option<(&'static str, u32, String)> = None;
        Frame::new().inner_margin(Margin { left: 20, right: 20, top: 18, bottom: 12 }).show(ui, |ui| {
            back = page_header(ui, &t, "Review marked items", "Look at each marked item before anything is deleted. Untick what you want to keep; the ticked ones go to the Recycle Bin after one more confirmation (with all the usual checks), so they can be restored.");
            ui.add_space(10.0);
            let rows = &self.review.rows;
            let ticked: Vec<&ReviewRow> = rows.iter().filter(|r| !self.review.untick.contains(&r.node)).collect();
            let bytes: u64 = ticked.iter().map(|r| r.bytes).sum();
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{} marked · {} ticked to delete · {}", rows.len(), ticked.len(), fmt_bytes(bytes))).font(theme::semibold(theme::ts::SECTION)));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add_enabled(!rows.is_empty(), egui::Button::new("Unmark all")).clicked() {
                        go = Some(("clear", 0, String::new()));
                    }
                    if ui.add_enabled(!self.review.untick.is_empty(), egui::Button::new("Unmark the unticked")).clicked() {
                        go = Some(("drop-unticked", 0, String::new()));
                    }
                    if ui.add_enabled(!rows.is_empty(), egui::Button::new("Tick all")).clicked() {
                        go = Some(("tick-all", 0, String::new()));
                    }
                    if ui.add_enabled(!rows.is_empty(), egui::Button::new("Untick all")).clicked() {
                        go = Some(("untick-all", 0, String::new()));
                    }
                });
            });
            ui.add_space(6.0);
            if rows.is_empty() {
                card(&t).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    plain_note(ui, RichText::new("Nothing is marked. Mark items with Ctrl+click or Space in the list, with \"Mark all\" in Save space, or choose copies on the Duplicates page.").color(t.muted));
                });
                return;
            }
            let h = (ui.available_height() - 52.0).max(140.0);
            card(&t).inner_margin(Margin::same(8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(h);
                ScrollArea::vertical().id_salt("review").auto_shrink([false, false]).show_rows(ui, 52.0, rows.len(), |ui, range| {
                    for r in &rows[range] {
                        let on = !self.review.untick.contains(&r.node);
                        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 50.0), Sense::hover());
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(6.0, 3.0))).layout(Layout::left_to_right(Align::Center)));
                        let ui = &mut child;
                        let mut tick = on;
                        if ui.checkbox(&mut tick, "").on_hover_text("Ticked: move to the Recycle Bin. Unticked: keep it.").changed() {
                            go = Some(("toggle", r.node, String::new()));
                        }
                        ui.add_sized(vec2(76.0, 20.0), egui::Label::new(RichText::new(fmt_bytes(r.bytes)).font(theme::num(theme::ts::BODY)).color(if on { t.text } else { t.faint })));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("✕").on_hover_text("Unmark (remove from this list)").clicked() {
                                go = Some(("unmark", r.node, String::new()));
                            }
                            if ui.small_button("Map").on_hover_text("Show it on the map").clicked() {
                                go = Some(("map", r.node, String::new()));
                            }
                            if ui.small_button("Explorer").on_hover_text("Show it in Explorer to look inside or open it").clicked() {
                                go = Some(("explorer", r.node, r.path.clone()));
                            }
                            ui.add_sized(vec2(80.0, 20.0), egui::Label::new(RichText::new(&r.age).color(t.faint).size(theme::ts::CAPTION)));
                            chip(ui, r.label, r.color.gamma_multiply(0.22), r.color);
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                ui.vertical(|ui| {
                                    let icon = if r.dir { "📁 " } else { "" };
                                    ui.add(egui::Label::new(RichText::new(format!("{icon}{}", visible(&r.name))).font(theme::semibold(theme::ts::SMALL)).color(if on { t.text } else { t.faint })).truncate()).on_hover_text(visible(&r.path));
                                    ui.add(egui::Label::new(RichText::new(format!("{} · {}", r.what, visible(&r.path))).color(t.muted).size(theme::ts::MICRO)).truncate());
                                });
                            });
                        });
                    }
                });
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Unticked items are unmarked when you continue. Protected places are refused at the next step, with the reason.").color(t.muted).size(theme::ts::SMALL));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let b = egui::Button::new(RichText::new(format!("Move {} to the Recycle Bin…", ticked.len())).color(Color32::WHITE).font(theme::semibold(theme::ts::BODY))).fill(theme::cat_color(5)).min_size(vec2(200.0, 30.0));
                    if ui.add_enabled(!ticked.is_empty() && self.job.is_none(), b).clicked() {
                        go = Some(("delete", 0, String::new()));
                    }
                });
            });
        });
        if back {
            self.page = Page::Map;
        }
        let Some((what, n, path)) = go else { return };
        match what {
            "toggle" => {
                if !self.review.untick.remove(&n) {
                    self.review.untick.insert(n);
                }
            }
            "tick-all" => self.review.untick.clear(),
            "untick-all" => self.review.untick = self.marked.iter().copied().collect(),
            "unmark" => {
                self.marked.remove(&n);
                self.dupe_of.remove(&n);
                self.rows = None;
            }
            "drop-unticked" | "delete" => {
                for n in std::mem::take(&mut self.review.untick) {
                    self.marked.remove(&n);
                    self.dupe_of.remove(&n);
                }
                self.rows = None;
                if what == "delete" {
                    self.ask_delete_many();
                }
            }
            "clear" => {
                self.marked.clear();
                self.dupe_of.clear();
                self.review.untick.clear();
                self.rows = None;
            }
            "map" => self.jump_to(n),
            "explorer" => self.explorer(&path),
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Activity
    // ------------------------------------------------------------------

    pub(super) fn open_activity(&mut self) {
        self.activity = Some(ActivityView { entries: activity::read(2000), filter: 0, bin: diskroot_session::restore::list() });
        self.page = Page::Activity;
    }

    pub(super) fn activity_page(&mut self, ui: &mut Ui) {
        let t = self.tokens();
        let Some(mut v) = self.activity.take() else { return };
        let mut back = false;
        let mut go: Option<(&'static str, String)> = None;
        let since = self.prev_visit;
        ScrollArea::vertical().id_salt("activity").auto_shrink([false, false]).show(ui, |ui| {
            Frame::new().inner_margin(Margin::same(20)).show(ui, |ui| {
                back = page_header(ui, &t, "Activity", "What you did with DiskRoot on this PC, and what changed on the drive between scans. Times are this PC's local time. Kept only on this PC.");
                ui.add_space(12.0);
                // Since the last visit.
                let new: Vec<&activity::Entry> = v.entries.iter().filter(|e| since > 0 && e.time > since).collect();
                card(&t).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    let title = if since > 0 { format!("Since your last visit ({})", activity::local_text(since)) } else { "Since your last visit".to_owned() };
                    ui.label(RichText::new(title).font(theme::semibold(theme::ts::SECTION)));
                    ui.add_space(4.0);
                    let count = |k: &str| new.iter().filter(|e| e.kind == k).count();
                    let freed: u64 = new.iter().filter(|e| e.kind == "deleted").map(|e| e.bytes).sum();
                    let saved: u64 = new.iter().filter(|e| e.kind == "compressed").map(|e| e.bytes).sum();
                    let mut bits = Vec::new();
                    if count("deleted") > 0 {
                        bits.push(format!("{} deleted ({} freed)", count("deleted"), fmt_bytes(freed)));
                    }
                    if count("compressed") > 0 {
                        bits.push(format!("{} compressed ({} saved)", count("compressed"), fmt_bytes(saved)));
                    }
                    if count("uncompressed") > 0 {
                        bits.push(format!("{} uncompressed", count("uncompressed")));
                    }
                    if count("duplicates") > 0 {
                        bits.push(format!("{} duplicate searches", count("duplicates")));
                    }
                    plain_note(ui, RichText::new(if bits.is_empty() { "Nothing done with DiskRoot since then.".to_owned() } else { bits.join(" · ") }).color(t.text));
                    match &self.changes {
                        Some(ch) => {
                            let d = i128::from(ch.after_total) - i128::from(ch.before_total);
                            plain_note(ui, RichText::new(format!(
                                "On the drive: {} → {} ({}) between the scan of {} and the scan of {}.",
                                fmt_bytes(ch.before_total),
                                fmt_bytes(ch.after_total),
                                fmt_delta(d),
                                activity::local_text(ch.before_time),
                                activity::local_text(ch.after_time)
                            )).color(t.muted));
                        }
                        None => plain_note(ui, RichText::new("No two scans to compare yet, so no drive changes to show.").color(t.faint)),
                    }
                });
                ui.add_space(12.0);
                // What DiskRoot did.
                card(&t).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("What you did with DiskRoot").font(theme::semibold(theme::ts::SECTION)));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.small_button("Open log folder").clicked() {
                                go = Some(("logs", String::new()));
                            }
                            // Right to left: shown as All, Deleted, Compressed, Duplicate searches.
                            for (f, name) in [(3u8, "Duplicate searches"), (2, "Compressed"), (1, "Deleted"), (0, "All")] {
                                if ui.selectable_label(v.filter == f, name).clicked() {
                                    v.filter = f;
                                }
                            }
                        });
                    });
                    ui.add_space(6.0);
                    let shown: Vec<&activity::Entry> = v
                        .entries
                        .iter()
                        .filter(|e| match v.filter {
                            1 => e.kind == "deleted" || e.kind == "not deleted" || e.kind == "restored",
                            2 => e.kind == "compressed" || e.kind == "uncompressed",
                            3 => e.kind == "duplicates",
                            _ => true,
                        })
                        .collect();
                    if shown.is_empty() {
                        ui.label(RichText::new("Nothing here yet.").color(t.faint));
                    }
                    egui::Grid::new("activity-grid").num_columns(5).striped(true).spacing([16.0, 6.0]).show(ui, |ui| {
                        if !shown.is_empty() {
                            for h in ["When", "What", "Size", "Item", ""] {
                                ui.label(RichText::new(h).color(t.muted).size(theme::ts::SMALL));
                            }
                            ui.end_row();
                        }
                        for e in shown.iter().take(500) {
                            let fresh = since > 0 && e.time > since;
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(activity::local_text(e.time)).font(theme::num(theme::ts::SMALL)));
                                if fresh {
                                    chip(ui, "new", t.accent, t.accent_ink);
                                }
                            });
                            let (label, col) = match e.kind.as_str() {
                                "deleted" => ("Deleted", theme::cat_color(5)),
                                "restored" => ("Restored", t.accent),
                                "not deleted" => ("Not deleted", t.muted),
                                "compressed" => ("Compressed", t.accent),
                                "uncompressed" => ("Uncompressed", theme::cat_color(6)),
                                "duplicates" => ("Duplicate search", theme::cat_color(3)),
                                _ => ("Other", t.muted),
                            };
                            ui.horizontal(|ui| chip(ui, label, col.gamma_multiply(0.22), col));
                            let size = match e.kind.as_str() {
                                "deleted" => format!("{} freed", fmt_bytes(e.bytes)),
                                "restored" => format!("{} back", fmt_bytes(e.after)),
                                "compressed" => format!("{} saved", fmt_bytes(e.bytes)),
                                "uncompressed" => format!("{} given back", fmt_bytes(e.bytes)),
                                "duplicates" => format!("{} in extra copies", fmt_bytes(e.bytes)),
                                _ => fmt_bytes(e.before),
                            };
                            ui.label(RichText::new(size).color(t.muted));
                            let detail = match e.kind.as_str() {
                                "compressed" | "uncompressed" => format!("{}\n{} files · {} → {} on disk · {}", visible(&e.path), fmt_count(e.files), fmt_bytes(e.before), fmt_bytes(e.after), e.note),
                                _ => format!("{}\n{}", visible(&e.path), e.note),
                            };
                            ui.add(egui::Label::new(visible(&e.path)).truncate()).on_hover_text(detail);
                            ui.horizontal(|ui| {
                                if e.kind != "deleted" && ui.small_button("Explorer").clicked() {
                                    go = Some(("explorer", e.path.clone()));
                                }
                                if e.kind == "deleted" {
                                    // Still in the Recycle Bin: one click puts it back.
                                    let in_bin = diskroot_session::restore::find(&v.bin, &e.path, e.time).is_some();
                                    if in_bin {
                                        if ui.small_button("Restore").on_hover_text("Move it back from the Recycle Bin to where it was. Nothing is overwritten.").clicked() {
                                            go = Some(("restore", format!("{}|{}", e.time, e.path)));
                                        }
                                    } else if ui.small_button("Recycle Bin").on_hover_text("Not in the Recycle Bin any more (emptied or restored). Opens it.").clicked() {
                                        go = Some(("bin", String::new()));
                                    }
                                }
                                if e.kind == "duplicates" && ui.small_button("Open").clicked() {
                                    go = Some(("dupes", String::new()));
                                }
                            });
                            ui.end_row();
                        }
                    });
                });
                ui.add_space(12.0);
                // What changed on the drive.
                card(&t).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("What changed on the drive").font(theme::semibold(theme::ts::SECTION)));
                    ui.add_space(4.0);
                    let Some(ch) = &self.changes else {
                        plain_note(ui, RichText::new("Scan again later (or turn on Scheduled scans) and the folders that grew or shrank show up here.").color(t.faint));
                        return;
                    };
                    plain_note(ui, RichText::new(format!("Folders that changed between the scan of {} and the scan of {}, biggest change first. \"Last change\" is the newest file date inside, from the latest scan.", activity::local_text(ch.before_time), activity::local_text(ch.after_time))).color(t.muted).size(theme::ts::SMALL));
                    if ch.items.is_empty() {
                        ui.label(RichText::new("No folder changed by more than 1 MB.").color(t.faint));
                        return;
                    }
                    let s = self.scanned.as_ref();
                    egui::Grid::new("activity-changes").num_columns(4).striped(true).spacing([16.0, 5.0]).show(ui, |ui| {
                        for h in ["Change", "Now", "Last change", "Folder"] {
                            ui.label(RichText::new(h).color(t.muted).size(theme::ts::SMALL));
                        }
                        ui.end_row();
                        for c in ch.items.iter().take(50) {
                            let d = c.delta();
                            ui.label(RichText::new(fmt_delta(d)).font(theme::num(theme::ts::SMALL)).color(if d > 0 { theme::warn() } else { t.accent }));
                            ui.label(RichText::new(fmt_bytes(c.after)).color(t.muted));
                            let when = s.and_then(|s| s.find_rel(&c.path).map(|n| s.mtime(n))).filter(|&m| m > 0).map_or_else(|| if c.after == 0 { "removed".to_owned() } else { String::new() }, diskroot_session::date_text);
                            ui.label(RichText::new(when).color(t.faint));
                            let name = if c.path.is_empty() { "(the scanned folder itself)".to_owned() } else { visible(&c.path) };
                            let note = if c.before == 0 { "  new" } else if c.after == 0 { "  gone" } else { "" };
                            if ui.add(egui::Label::new(format!("{name}{note}")).truncate().sense(Sense::click())).on_hover_text("Show it on the map").clicked() {
                                go = Some(("rel", c.path.clone()));
                            }
                            ui.end_row();
                        }
                    });
                });
            });
        });
        self.activity = Some(v);
        if back {
            self.page = Page::Map;
        }
        match go {
            Some(("logs", _)) => {
                if let Some(dir) = activity::log_path().and_then(|p| p.parent().map(|d| d.display().to_string())) {
                    let _ = std::process::Command::new(diskroot_session::integrity::windows_dir().join("explorer.exe")).arg(dir).spawn();
                }
            }
            Some(("explorer", p)) => self.explorer(&p),
            Some(("bin", _)) => {
                let _ = std::process::Command::new(diskroot_session::integrity::windows_dir().join("explorer.exe")).arg("shell:RecycleBinFolder").spawn();
            }
            Some(("dupes", _)) => self.open_dupes_tab(2),
            Some(("restore", key)) => {
                let (time, path) = key.split_once('|').map_or((0, key.as_str()), |(a, b)| (a.parse().unwrap_or(0), b));
                let bin = diskroot_session::restore::list();
                let result = match diskroot_session::restore::find(&bin, path, time) {
                    Some(item) => diskroot_session::restore::restore(item).map(|()| item.size),
                    None => Err("It's no longer in the Recycle Bin.".into()),
                };
                match result {
                    Ok(size) => {
                        activity::append(&activity::Logged { time: activity::now(), kind: "restored".into(), path: path.to_owned(), files: 0, before: 0, after: size, note: "from the Recycle Bin".into() });
                        self.say(format!("Restored {}. Scan again to see it on the map.", visible(path)));
                    }
                    Err(e) => self.dialog = Some(super::Dialog::Message("Couldn't restore it".into(), e)),
                }
                if let Some(a) = &mut self.activity {
                    a.entries = activity::read(2000);
                    a.bin = diskroot_session::restore::list();
                }
            }
            Some(("rel", rel)) => {
                self.page = Page::Map;
                self.show_rel(&rel);
            }
            _ => {}
        }
    }
}

/// What a click on the Duplicates page asks for (done after drawing).
pub(super) enum DupesGo {
    Start,
    /// Read these files in full now (compare view's "Verify").
    Verify(Vec<String>),
    /// Add to / take off the "marked for later" list.
    Later(Vec<later::Later>),
    Unlater(Vec<String>),
    /// Straight to the delete confirmation (no Review page).
    DeleteNow(Vec<u32>),
    /// Same names: look again with the minimum size now set.
    Names,
    /// Back to the search options.
    NewSearch,
    Explorer(String),
    /// Open a photo/video/document in its usual app.
    Preview(String),
    Map(u32),
    /// (node if still in the scan, the kept copy for duplicates).
    Review(Vec<(Option<u32>, Option<String>)>),
    Open(PathBuf),
    Forget(PathBuf),
}

use std::path::PathBuf;

/// The search running: its three steps, a progress bar, and Cancel.
fn dupes_progress(ui: &mut Ui, t: &Tokens, p: &dupes::Progress, cancel: &AtomicBool, started: Instant) {
    let phase = p.phase.load(Ordering::Relaxed);
    let (done, of) = (p.done.load(Ordering::Relaxed), p.of.load(Ordering::Relaxed).max(1));
    let (read, total) = (p.read.load(Ordering::Relaxed), p.total.load(Ordering::Relaxed));
    let secs = started.elapsed().as_secs_f64().max(0.1);
    ui.add_space(20.0);
    card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new("Looking for identical files…").font(theme::semibold(theme::ts::TITLE)));
        ui.add_space(8.0);
        let steps = ["Checking which files can be read (and hardlinks)", "Quick check: first and last 64 KB", "Full compare of files still matching"];
        for (i, s) in steps.iter().enumerate() {
            let (mark, col) = match (i as u8).cmp(&phase) {
                std::cmp::Ordering::Less => ("✔", t.accent),
                std::cmp::Ordering::Equal => ("●", theme::warn()),
                std::cmp::Ordering::Greater => ("○", t.faint),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(mark).color(col));
                ui.label(RichText::new(*s).color(if i as u8 == phase { t.text } else { t.muted }));
                if i as u8 == phase {
                    ui.label(RichText::new(format!("{} of {} files", fmt_count(done.min(of)), fmt_count(of))).color(t.faint));
                }
            });
        }
        ui.add_space(8.0);
        let frac = if phase == 0 { done as f32 / of as f32 * 0.1 } else if total > 0 { 0.1 + 0.9 * (read as f32 / total as f32).min(1.0) } else { 0.1 + 0.9 * (done as f32 / of as f32) };
        ui.add(egui::ProgressBar::new(frac.min(1.0)).desired_width(ui.available_width()));
        ui.add_space(4.0);
        let reused = p.reused.load(Ordering::Relaxed);
        let threads = p.threads.load(Ordering::Relaxed);
        ui.label(RichText::new(format!(
            "{} of about {} read · {}/s · {} at a time{} · {:.0} s",
            fmt_bytes(read),
            fmt_bytes(total),
            fmt_bytes((read as f64 / secs) as u64),
            if threads > 1 { format!("{threads} files") } else { "1 file (hard disk or unknown drive)".to_owned() },
            if reused > 0 { format!(" · {} files reused from the last search", fmt_count(reused)) } else { String::new() },
            secs
        )).color(t.muted));
        ui.add_space(8.0);
        let stopping = cancel.load(Ordering::Relaxed);
        if ui.add_enabled(!stopping, egui::Button::new(if stopping { "Stopping…" } else { "Cancel" })).clicked() {
            cancel.store(true, Ordering::Relaxed);
        }
        plain_note(ui, RichText::new("You can leave this page: the search keeps going and tells you when it's done.").color(t.faint).size(theme::ts::SMALL));
    });
}

fn past_tab(ui: &mut Ui, t: &Tokens, d: &mut DupesPage, out: &mut Option<DupesGo>) {
    plain_note(ui, RichText::new("Every duplicate search is saved on this PC (the newest 10 per place), so you can come back to its results. The files may have changed since: anything moved or deleted is shown as no longer in the scan.").color(t.muted));
    ui.add_space(8.0);
    if d.past.is_empty() {
        ui.label(RichText::new("No saved searches yet.").color(t.faint));
        return;
    }
    ScrollArea::vertical().id_salt("past-dupes").auto_shrink([false, false]).show(ui, |ui| {
        card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            egui::Grid::new("past-dupes-grid").num_columns(6).striped(true).spacing([18.0, 6.0]).show(ui, |ui| {
                for h in ["When", "Where", "Sets", "Could free", "Took", ""] {
                    ui.label(RichText::new(h).color(t.muted).size(theme::ts::SMALL));
                }
                ui.end_row();
                for r in &d.past {
                    ui.label(RichText::new(activity::local_text(r.time)).font(theme::num(theme::ts::SMALL)));
                    ui.add(egui::Label::new(visible(&r.root)).truncate()).on_hover_text(format!("{}\nFiles of at least {}", visible(&r.root), fmt_bytes(r.min_size)));
                    ui.label(fmt_count(r.sets as u64));
                    ui.label(RichText::new(fmt_bytes(r.wasted)).color(t.accent));
                    ui.label(RichText::new(format!("{:.1} s", r.secs)).color(t.muted));
                    ui.horizontal(|ui| {
                        if ui.small_button("Open").clicked() {
                            *out = Some(DupesGo::Open(r.file.clone()));
                        }
                        if ui.small_button("Forget").on_hover_text("Deletes this saved search (not the files)").clicked() {
                            *out = Some(DupesGo::Forget(r.file.clone()));
                        }
                    });
                    ui.end_row();
                }
            });
        });
    });
}

/// Size history, as the third tab of Drive status.
fn growth_body(ui: &mut Ui, t: &Tokens, v: &mut GrowthView, jump: &mut Option<String>) {
    plain_note(ui, RichText::new(format!(
        "{} · {} scans since {} · the history uses {} on this PC",
        visible(&v.root),
        v.points,
        diskroot_session::date_text((v.first / 86_400) as u16),
        fmt_bytes(v.disk)
    )).color(t.muted));
    ui.add_space(8.0);
    card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(if v.drive { "Used space on the drive" } else { "Size of the folder" }).font(theme::semibold(theme::ts::LEAD)));
        if v.chart.len() >= 2 {
            size_chart(ui, t, &v.chart, vec2(ui.available_width(), 160.0), true);
        } else {
            ui.label(RichText::new("One scan so far: the chart starts with the next one.").color(t.faint));
        }
        if let Some(f) = &v.forecast {
            ui.add_space(4.0);
            let col = if matches!(f, Forecast::Full { low, .. } if *low < 60) { theme::warn() } else { t.text };
            ui.add(egui::Label::new(RichText::new(f.text(v.root.trim_end_matches('\\'))).color(col)).wrap())
                .on_hover_text("From the drive's free space at the last scan of each day, over up to 60 days. Sudden one-off jumps (one big download) aren't counted as the pace. A rough guide, not a promise.");
        }
    });
    ui.add_space(12.0);
    card(t).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new("What grew").font(theme::semibold(theme::ts::LEAD)));
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
                    "Comparing the scan of {} with the latest ({}). {} → {} ({})",
                    activity::local_text(ch.before_time),
                    activity::local_text(ch.after_time),
                    fmt_bytes(ch.before_total),
                    fmt_bytes(ch.after_total),
                    fmt_delta(i128::from(ch.after_total) - i128::from(ch.before_total))
                )).color(t.muted).size(12.0));
                if ch.mixed_methods() {
                    ui.add(egui::Label::new(RichText::new("One of these scans was a Fast scan and the other a folder walk: they count some things differently, so part of the change comes from the method.").color(theme::warn()).size(theme::ts::SMALL)).wrap());
                }
                if ch.items.is_empty() {
                    ui.label(RichText::new("No folder changed by more than 1 MB (or 0.1% of the total).").color(t.faint));
                } else {
                    egui::Grid::new("growers").num_columns(3).striped(true).spacing([14.0, 4.0]).show(ui, |ui| {
                        for c in ch.items.iter().take(40) {
                            let d = c.delta();
                            ui.label(RichText::new(fmt_delta(d)).color(if d > 0 { theme::warn() } else { t.accent }).strong());
                            ui.label(RichText::new(fmt_bytes(c.after)).color(t.muted));
                            let shown = if c.path.is_empty() { "(the scanned folder itself)".to_owned() } else { c.path.clone() };
                            let note = if c.before == 0 { "  new" } else if c.after == 0 { "  gone" } else { "" };
                            let r = ui.add(egui::Label::new(format!("{shown}{note}")).truncate().sense(Sense::click()));
                            if r.on_hover_text("Click to show it in the tree").clicked() {
                                *jump = Some(c.path.clone());
                            }
                            ui.end_row();
                        }
                    });
                }
            }
        }
    });
    if !v.logs.is_empty() {
        ui.add_space(12.0);
        card(t).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new("Log files that keep growing").font(theme::semibold(theme::ts::LEAD)));
            ui.label(RichText::new("They grew in every one of the last scans: space now, and SSD writes all the time. Check what writes them before deleting (a program may need its log).").color(t.muted).size(theme::ts::SMALL));
            for g in v.logs.iter().take(8) {
                let r = ui.add(egui::Label::new(format!("{} → {} over {} ({} scans) · {}", fmt_bytes(g.first), fmt_bytes(g.last), remember::gap_text(g.over), g.scans, g.path)).truncate().sense(Sense::click()));
                if r.on_hover_text("Click to show it in the tree").clicked() {
                    *jump = Some(g.path.clone());
                }
            }
        });
    }
}
