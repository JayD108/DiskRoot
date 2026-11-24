//! The workspace tabs under the toolbar, the Large files page, and the
//! triage parts of the Duplicates page:
//! - Same names: files sharing a name in different folders, whatever their
//!   contents, with what is known about the contents.
//! - Marked for later: files the user isn't sure about, kept on this PC.
//! - The compare view: copies side by side (folder, exact size, created,
//!   modified, content), with "Verify now" (full BLAKE3).

use std::collections::HashMap;

use super::pages::{DupesGo, DupesPage, card, chip, file_name, fixed_box, page_header, parent_of, resolve};
use super::*;
use diskroot_session::{activity, dupes, later};

/// What the compare view shows for one file (read once, when it's shown).
#[derive(Clone, Default)]
pub(super) struct Meta {
    pub exists: bool,
    pub bytes: Option<u64>,
    pub created: Option<u64>,
    pub modified: Option<u64>,
}

fn read_meta(path: &str) -> Meta {
    let secs = |t: std::io::Result<std::time::SystemTime>| t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs());
    match std::fs::symlink_metadata(path) {
        Ok(m) => Meta { exists: true, bytes: Some(m.len()), created: secs(m.created()), modified: secs(m.modified()) },
        Err(_) => Meta::default(),
    }
}

/// `path` cut from the left at a folder boundary ("…\photos\2019") to
/// about `max` characters.
pub(super) fn tail_path(path: &str, max: usize) -> String {
    let n = path.chars().count();
    if n <= max {
        return path.to_owned();
    }
    let tail: String = path.chars().skip(n - max.saturating_sub(1)).collect();
    match tail.find('\\') {
        Some(i) if i + 1 < tail.len() => format!("…{}", &tail[i..]),
        _ => format!("…{tail}"),
    }
}

/// One column of the compare view.
pub(super) struct CmpRow {
    pub path: String,
    /// Size from the scan.
    pub size: u64,
    pub node: Option<u32>,
    /// What is known about the contents, and its colour.
    pub content: (String, Color32),
    pub later: bool,
}

pub(super) fn later_item(path: &str, size: u64, why: String) -> later::Later {
    later::Later { path: path.to_owned(), size, added: activity::now(), why }
}

/// The "Later" toggle on a row; true when clicked.
pub(super) fn later_button(ui: &mut Ui, t: &Tokens, flagged: bool) -> bool {
    let text = if flagged { RichText::new("✓ Later").color(theme::warn()) } else { RichText::new("Later").color(t.text) };
    ui.small_button(text)
        .on_hover_text(if flagged { "On your \"Marked for later\" list. Click to take it off." } else { "Not sure yet? Put it on the \"Marked for later\" list (saved on this PC). Nothing is deleted." })
        .clicked()
}

/// The title of a set with its badge, and the Compare / Verify buttons.
#[allow(clippy::too_many_arguments)]
pub(super) fn set_header(ui: &mut Ui, title: String, badge: (&str, Color32), compare: &mut bool, verifying: bool, paths: &[String], out: &mut Option<DupesGo>) {
    ui.add(egui::Label::new(RichText::new(title).font(theme::semibold(theme::ts::TITLE))).truncate());
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let v = egui::Button::new(if verifying { "Reading…" } else { "Verify now" });
            if ui.add_enabled(!verifying, v).on_hover_text("Reads each file in full now and compares the BLAKE3 fingerprints (also shows if a file changed since the search). Online-only cloud files are skipped.").clicked() {
                *out = Some(DupesGo::Verify(paths.to_vec()));
            }
            if ui.selectable_label(*compare, "Compare side by side").on_hover_text("Folder, exact size, created and modified time and contents of each file, next to each other").clicked() {
                *compare = !*compare;
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                chip(ui, badge.0, badge.1.gamma_multiply(0.22), badge.1);
            });
        });
    });
}

/// Content status of `path` in an identical set.
pub(super) fn identical_status(hashes: &HashMap<String, Option<String>>, paths: &[String], path: &str, t: &Tokens) -> (String, Color32) {
    let mine = hashes.get(&path.to_lowercase());
    let first = paths.iter().find_map(|p| hashes.get(&p.to_lowercase()).cloned().flatten());
    match (mine, first) {
        (Some(Some(h)), Some(f)) if *h == f => (format!("Identical · BLAKE3 {}… (verified now)", &h[..12]), theme::cat_color(2)),
        (Some(Some(h)), _) => (format!("Changed since the search · BLAKE3 {}…", &h[..12]), theme::cat_color(5)),
        (Some(None), _) => ("Couldn't be read now (in use, gone or online-only)".to_owned(), theme::warn()),
        (None, _) => ("Identical (BLAKE3 match in the search)".to_owned(), t.accent),
    }
}

/// Side-by-side compare: one column per file, one row per detail. Values
/// that differ between the files are highlighted.
pub(super) fn compare_grid(ui: &mut Ui, t: &Tokens, id: impl std::hash::Hash + std::fmt::Debug, rows: &[CmpRow], meta: &mut HashMap<String, Meta>, out: &mut Option<DupesGo>) {
    for r in rows {
        meta.entry(r.path.to_lowercase()).or_insert_with(|| read_meta(&r.path));
    }
    let m: Vec<Meta> = rows.iter().map(|r| meta.get(&r.path.to_lowercase()).cloned().unwrap_or_default()).collect();
    let when = |s: Option<u64>| s.map_or("unknown".to_owned(), activity::local_text);
    let differs = |v: &[String]| v.windows(2).any(|w| w[0] != w[1]);
    let names: Vec<String> = rows.iter().map(|r| visible(file_name(&r.path))).collect();
    let folders: Vec<String> = rows.iter().map(|r| visible(parent_of(&r.path))).collect();
    let sizes: Vec<String> = rows.iter().zip(&m).map(|(r, m)| m.bytes.unwrap_or(r.size).to_string()).collect();
    let created: Vec<String> = m.iter().map(|m| when(m.created)).collect();
    let modified: Vec<String> = m.iter().map(|m| when(m.modified)).collect();
    let col_w = 260.0;
    ScrollArea::both().id_salt(id).auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("cmp-grid").num_columns(rows.len() + 1).striped(true).spacing([14.0, 8.0]).min_col_width(90.0).max_col_width(col_w).show(ui, |ui| {
            let head = |ui: &mut Ui, s: &str| {
                ui.label(RichText::new(s).color(t.muted).size(theme::ts::SMALL));
            };
            head(ui, "");
            for (i, _) in rows.iter().enumerate() {
                ui.label(RichText::new(format!("Copy {}", i + 1)).font(theme::semibold(theme::ts::BODY)));
            }
            ui.end_row();
            let line = |ui: &mut Ui, label: &str, vals: &[String], mono: bool| {
                head(ui, label);
                let d = differs(vals);
                for v in vals {
                    let mut rt = RichText::new(v).color(if d { theme::warn() } else { t.text });
                    if mono {
                        rt = rt.font(theme::num(theme::ts::SMALL));
                    }
                    ui.add(egui::Label::new(rt).wrap()).on_hover_text(if d { "Differs between the files" } else { "The same for all" });
                }
                ui.end_row();
            };
            line(ui, "Name", &names, false);
            line(ui, "Folder", &folders, false);
            let bytes_text: Vec<String> = sizes.iter().map(|b| b.parse::<u64>().map_or(b.clone(), |n| format!("{} bytes ({})", fmt_count(n), fmt_bytes(n)))).collect();
            line(ui, "Exact size", &bytes_text, true);
            line(ui, "Created", &created, true);
            line(ui, "Modified", &modified, true);
            head(ui, "Contents");
            for r in rows {
                ui.add(egui::Label::new(RichText::new(&r.content.0).color(r.content.1)).wrap());
            }
            ui.end_row();
            head(ui, "Status");
            for (r, m) in rows.iter().zip(&m) {
                let mut bits = Vec::new();
                if !m.exists {
                    bits.push("no longer there");
                }
                if r.node.is_none() {
                    bits.push("not in this scan");
                }
                if r.later {
                    bits.push("marked for later");
                }
                ui.label(RichText::new(if bits.is_empty() { "ok".to_owned() } else { bits.join(" · ") }).color(if m.exists { t.muted } else { theme::cat_color(5) }).size(theme::ts::SMALL));
            }
            ui.end_row();
            head(ui, "");
            for r in rows {
                ui.horizontal(|ui| {
                    if ui.small_button("Explorer").on_hover_text("Open its folder with the file selected").clicked() {
                        *out = Some(DupesGo::Explorer(r.path.clone()));
                    }
                    if diskroot_session::can_preview(&r.path) && ui.small_button("Open").on_hover_text("Open it in its usual app to look at it (photos, videos, music and documents only)").clicked() {
                        *out = Some(DupesGo::Preview(r.path.clone()));
                    }
                    if let Some(n) = r.node
                        && ui.small_button("Map").clicked()
                    {
                        *out = Some(DupesGo::Map(n));
                    }
                });
            }
            ui.end_row();
        });
    });
}

/// A finished "Verify now" arrives.
pub(super) fn poll_verify(ctx: &egui::Context, d: &mut DupesPage) {
    let Some(rx) = &d.verifying else { return };
    match rx.try_recv() {
        Ok((paths, hashes)) => {
            for (p, h) in paths.into_iter().zip(hashes) {
                d.hashes.insert(p.to_lowercase(), h);
            }
            d.verifying = None;
            d.meta.clear();
        }
        Err(TryRecvError::Disconnected) => d.verifying = None,
        Err(TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(150)),
    }
}

/// Same-name set: a content key per file (files with the same key are
/// identical), and where the key came from.
fn content_keys(hashes: &HashMap<String, Option<String>>, run_index: &HashMap<String, usize>, g: &diskroot_session::NameGroup) -> Vec<Option<(String, &'static str)>> {
    g.files
        .iter()
        .map(|f| {
            let lp = f.path.to_lowercase();
            if let Some(h) = hashes.get(&lp) {
                return h.clone().map(|h| (format!("h{h}"), "verified now"));
            }
            if let Some(&gi) = run_index.get(&lp) {
                return Some((format!("g{gi}"), "from the duplicate search"));
            }
            if g.files.iter().filter(|o| o.size == f.size).count() == 1 {
                return Some((format!("s{}", f.size), "size differs"));
            }
            None
        })
        .collect()
}

/// Per file: (content text, colour), and a badge for the whole set.
fn name_statuses(hashes: &HashMap<String, Option<String>>, run_index: &HashMap<String, usize>, g: &diskroot_session::NameGroup, t: &Tokens) -> (Vec<(String, Color32)>, (&'static str, Color32)) {
    let keys = content_keys(hashes, run_index, g);
    let mut letters: Vec<&str> = Vec::new();
    let mut texts = Vec::new();
    let mut shared = false;
    for k in &keys {
        match k {
            Some((key, src)) => {
                let n = keys.iter().filter(|o| o.as_ref().is_some_and(|o| o.0 == *key)).count();
                let li = letters.iter().position(|l| l == key).unwrap_or_else(|| {
                    letters.push(key);
                    letters.len() - 1
                });
                let letter = (b'A' + (li % 26) as u8) as char;
                if n > 1 {
                    shared = true;
                    texts.push((format!("Contents {letter} · identical to {} other{} here ({src})", n - 1, if n > 2 { "s" } else { "" }), theme::cat_color(2)));
                } else {
                    texts.push((format!("Contents {letter} · different from the others ({src})"), t.text));
                }
            }
            None => texts.push(("Same size as another: not compared yet (Verify now)".to_owned(), t.muted)),
        }
    }
    let badge = if shared {
        ("SAME NAME · HAS IDENTICAL COPIES", theme::cat_color(2))
    } else if keys.iter().all(Option::is_some) {
        ("SAME NAME · DIFFERENT CONTENTS", theme::warn())
    } else {
        ("SAME NAME", theme::cat_color(6))
    };
    (texts, badge)
}

impl DiskRootApp {
    // ------------------------------------------------------------------
    // Workspace tabs
    // ------------------------------------------------------------------

    /// The full-window workspaces, as tabs under the toolbar (Ctrl+1..6).
    pub(super) fn workspace_tabs(&mut self, ui: &mut Ui) {
        let t = self.tokens();
        let scanned = self.scanned.is_some();
        let marks = self.marked.len();
        let tabs: [(u8, String, bool, bool, &str); 6] = [
            (0, "Storage overview".into(), matches!(self.page, Page::Map | Page::Home), true, "The folder list, the map and the Inspector (Home: Ctrl+H)"),
            (1, "Duplicates & same names".into(), self.page == Page::Dupes, scanned, "Identical files, files with the same name, similar folders, and what you marked for later"),
            (2, "Large files".into(), self.page == Page::Files, scanned, "The biggest files and file types in this scan"),
            (3, "Drive status".into(), self.page == Page::Status, true, "Health, who's writing, size history and past scans"),
            (4, format!("Review {marks} marked"), self.page == Page::Review, marks > 0, "Check each marked item and decide: delete or keep"),
            (5, "Activity".into(), self.page == Page::Activity, true, "What was deleted and compressed, and when"),
        ];
        let mut go: Option<u8> = None;
        if self.dialog.is_none() && self.palette.is_none() && !ui.ctx().egui_wants_keyboard_input() {
            let keys = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6];
            for (i, k) in keys.iter().enumerate() {
                if tabs[i].3 && ui.input_mut(|inp| inp.consume_key(egui::Modifiers::CTRL, *k)) {
                    go = Some(i as u8);
                }
            }
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (i, (n, label, on, shown, tip)) in tabs.iter().enumerate() {
                if !shown {
                    continue;
                }
                let warn = *n == 4;
                let color = if *on { t.text } else if warn { theme::warn() } else { t.muted };
                let text = RichText::new(label).font(if *on { theme::semibold(theme::ts::BODY) } else { theme::regular(theme::ts::BODY) }).color(color);
                let r = ui.add(egui::Button::new(text).frame(false).min_size(vec2(0.0, 28.0))).on_hover_text(format!("{tip} (Ctrl+{})", i + 1));
                let line = Rect::from_min_max(pos2(r.rect.min.x + 4.0, r.rect.max.y - 2.0), pos2(r.rect.max.x - 4.0, r.rect.max.y));
                if *on {
                    ui.painter().rect_filled(line, CornerRadius::same(1), t.accent);
                } else if r.hovered() {
                    ui.painter().rect_filled(line, CornerRadius::same(1), t.line);
                }
                if r.clicked() {
                    go = Some(*n);
                }
                ui.add_space(10.0);
            }
        });
        match go {
            Some(0) => self.page = Page::Map,
            Some(1) => {
                let tab = self.dupes.as_ref().map_or(0, |d| d.tab);
                self.open_dupes_tab(tab);
            }
            Some(2) => {
                if self.files_view.is_some() {
                    self.page = Page::Files;
                } else {
                    self.open_files();
                }
            }
            Some(3) => self.open_status(self.status_tab),
            Some(4) => self.open_review(),
            Some(5) => self.open_activity(),
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Large files
    // ------------------------------------------------------------------

    pub(super) fn files_page(&mut self, ui: &mut Ui) {
        let t = self.tokens();
        let Some(mut fv) = self.files_view.take() else { return };
        let mut go: Option<(&'static str, u32, String)> = None;
        Frame::new().inner_margin(Margin { left: 20, right: 20, top: 18, bottom: 12 }).show(ui, |ui| {
            page_header(ui, &t, "Large files", "The biggest files in this scan and which file types take the most space. Mark files to review them for the Recycle Bin, or open their folder.");
            ui.add_space(8.0);
            let r = ui.add(TextEdit::singleline(&mut fv.query).hint_text("Search file names").desired_width(320.0));
            if r.changed()
                && let Some(s) = &self.scanned
            {
                fv.top = s.top_files(500, &fv.query);
            }
            ui.add_space(6.0);
            let h = ui.available_height().max(120.0);
            let w = ui.available_width();
            let left = (w * 0.64).max(320.0);
            ui.horizontal_top(|ui| {
                fixed_box(ui, vec2(left, h), |ui| {
                    card(&t).inner_margin(Margin::same(8)).show(ui, |ui| {
                        ui.set_min_size(vec2(left - 18.0, h - 18.0));
                        ui.set_max_width(left - 18.0);
                        ui.label(RichText::new(format!("Largest files ({})", fv.top.len())).font(theme::semibold(theme::ts::SECTION)));
                        if fv.top.is_empty() {
                            ui.label(RichText::new("No matching files.").color(t.faint));
                        }
                        let today = diskroot_session::today_days();
                        ScrollArea::vertical().id_salt("large-files").auto_shrink([false, false]).show_rows(ui, 40.0, fv.top.len(), |ui, range| {
                            for f in &fv.top[range] {
                                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::hover());
                                let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(4.0, 2.0))).layout(Layout::left_to_right(Align::Center)));
                                let ui = &mut child;
                                let mut on = self.marked.contains(&f.id);
                                if ui.checkbox(&mut on, "").on_hover_text("Mark it (Review marked shows everything marked before anything is deleted)").changed() {
                                    go = Some(("mark", f.id, String::new()));
                                }
                                ui.add_sized(vec2(76.0, 20.0), egui::Label::new(RichText::new(fmt_bytes(f.on_disk)).font(theme::num(theme::ts::BODY))));
                                let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                                ui.painter().rect_filled(dot, CornerRadius::same(2), theme::cat_color(f.cat));
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui.small_button("Map").on_hover_text("Show it on the map").clicked() {
                                        go = Some(("map", f.id, String::new()));
                                    }
                                    if ui.small_button("Explorer").clicked() {
                                        go = Some(("explorer", 0, f.path.clone()));
                                    }
                                    let age = if f.mtime > 0 { diskroot_session::age_text(today, f.mtime) } else { String::new() };
                                    ui.add_sized(vec2(90.0, 20.0), egui::Label::new(RichText::new(age).color(t.faint).size(theme::ts::CAPTION)));
                                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                        ui.vertical(|ui| {
                                            ui.add(egui::Label::new(RichText::new(visible(&f.name)).font(theme::semibold(theme::ts::SMALL))).truncate()).on_hover_text(visible(&f.path));
                                            ui.add(egui::Label::new(RichText::new(visible(parent_of(&f.path))).color(t.muted).size(theme::ts::CAPTION)).truncate());
                                        });
                                    });
                                });
                            }
                        });
                    });
                });
                ui.add_space(8.0);
                let rw = ui.available_width();
                fixed_box(ui, vec2(rw, h), |ui| {
                    card(&t).inner_margin(Margin::same(8)).show(ui, |ui| {
                        ui.set_min_size(vec2(rw - 18.0, h - 18.0));
                        ui.label(RichText::new(format!("File types ({})", fv.types.len())).font(theme::semibold(theme::ts::SECTION)));
                        ScrollArea::vertical().id_salt("large-types").auto_shrink([false, false]).show(ui, |ui| {
                            egui::Grid::new("types").num_columns(3).striped(true).spacing([12.0, 4.0]).show(ui, |ui| {
                                for h in ["Type", "Size", "Files"] {
                                    ui.label(RichText::new(h).color(t.muted).size(theme::ts::SMALL));
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
                });
            });
        });
        self.files_view = Some(fv);
        match go {
            Some(("mark", n, _)) => {
                if !self.marked.remove(&n) {
                    self.marked.insert(n);
                }
                self.rows = None;
            }
            Some(("map", n, _)) => self.jump_to(n),
            Some(("explorer", _, p)) => self.explorer(&p),
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Duplicates: shared
    // ------------------------------------------------------------------

    /// Work a Duplicates tab needs before it is drawn (same-name sets,
    /// which "for later" files still exist, the path index of the search).
    pub(super) fn prepare_dupes_tab(&mut self) {
        let (Some(s), Some(d)) = (&self.scanned, &mut self.dupes) else { return };
        if let Some(run) = &d.run {
            let key = (run.time, run.groups.iter().map(|g| g.files.len()).sum());
            if d.run_index_for != Some(key) {
                d.run_index = run.groups.iter().enumerate().flat_map(|(gi, g)| g.files.iter().map(move |f| (f.1.to_lowercase(), gi))).collect();
                d.run_index_for = Some(key);
            }
        } else {
            d.run_index.clear();
            d.run_index_for = None;
        }
        match d.tab {
            3 if d.names.is_none() || d.names_for != d.names_min << 20 => {
                let v = s.name_collisions(d.names_min << 20, 5000);
                d.names_pick = v.iter().map(|g| vec![false; g.files.len()]).collect();
                d.names_sel = 0;
                d.names_shown_for = None;
                d.names_for = d.names_min << 20;
                d.names = Some(v);
            }
            4 => {
                d.later_exists = d.later.iter().map(|l| (l.path.to_lowercase(), std::fs::symlink_metadata(&l.path).is_ok())).collect();
            }
            _ => {}
        }
    }

    pub(super) fn verify_files(&mut self, paths: Vec<String>) {
        let Some(d) = &mut self.dupes else { return };
        let (tx, rx) = mpsc::channel();
        let ctx = self.egui_ctx.clone();
        std::thread::spawn(move || {
            let h = dupes::hash_paths(&paths, &AtomicBool::new(false));
            let _ = tx.send((paths, h));
            ctx.request_repaint();
        });
        d.verifying = Some(rx);
    }

    pub(super) fn mark_later(&mut self, items: Vec<later::Later>) {
        let Some(d) = &mut self.dupes else { return };
        let n = later::add(&mut d.later, items);
        match later::save(&d.later) {
            Ok(()) => self.say(format!("{n} marked for later · they stay on the \"Marked for later\" tab, also after a restart. Nothing was deleted.")),
            Err(e) => self.say(format!("Couldn't save the \"for later\" list: {e}")),
        }
    }

    pub(super) fn unmark_later(&mut self, paths: &[String]) {
        let Some(d) = &mut self.dupes else { return };
        later::remove(&mut d.later, paths);
        for p in paths {
            d.later_pick.remove(&p.to_lowercase());
        }
        if let Err(e) = later::save(&d.later) {
            self.say(format!("Couldn't save the \"for later\" list: {e}"));
        }
    }

    /// After a delete: the deleted files leave the sets on screen and the
    /// "for later" list.
    pub(super) fn forget_deleted(&mut self, paths: &[String]) {
        if paths.is_empty() {
            return;
        }
        let gone = |p: &str| paths.iter().any(|g| g.eq_ignore_ascii_case(p));
        let d = match &mut self.dupes {
            Some(d) => d,
            None => {
                // Not opened yet: still keep the saved list right.
                let mut v = later::load();
                if later::remove(&mut v, paths) > 0 {
                    let _ = later::save(&v);
                }
                return;
            }
        };
        if let Some(run) = &d.run
            && run.groups.iter().any(|g| g.files.iter().any(|f| gone(&f.1)))
        {
            let mut run = run.clone();
            for g in &mut run.groups {
                g.files.retain(|f| !gone(&f.1));
            }
            run.groups.retain(|g| g.files.len() > 1);
            let past = d.from_past;
            d.show(run, past);
        }
        if let Some(names) = &mut d.names {
            for (gi, g) in names.iter_mut().enumerate() {
                let keep: Vec<bool> = g.files.iter().map(|f| !gone(&f.path)).collect();
                let mut it = keep.iter();
                g.files.retain(|_| *it.next().unwrap_or(&true));
                if let Some(p) = d.names_pick.get_mut(gi) {
                    let mut it = keep.iter();
                    p.retain(|_| *it.next().unwrap_or(&true));
                }
            }
            let mut i = 0;
            let keep: Vec<bool> = names.iter().map(|g| g.files.len() > 1).collect();
            names.retain(|_| {
                i += 1;
                keep[i - 1]
            });
            let mut it = keep.iter();
            d.names_pick.retain(|_| *it.next().unwrap_or(&true));
            d.names_shown_for = None;
        }
        for p in paths {
            d.hashes.remove(&p.to_lowercase());
            d.meta.remove(&p.to_lowercase());
        }
        if later::remove(&mut d.later, paths) > 0 {
            let _ = later::save(&d.later);
        }
    }

    // ------------------------------------------------------------------
    // Same names
    // ------------------------------------------------------------------

    pub(super) fn names_tab(&mut self, ui: &mut Ui, t: &Tokens, d: &mut DupesPage, out: &mut Option<DupesGo>) {
        ui.horizontal(|ui| {
            ui.label("Files of at least");
            let r = ui.add(egui::DragValue::new(&mut d.names_min).range(0..=100_000).suffix(" MB"));
            for mb in [0u64, 1, 10, 100] {
                if ui.selectable_label(d.names_min == mb, if mb == 0 { "any size".to_owned() } else { format!("{mb} MB") }).clicked() {
                    d.names_min = mb;
                    *out = Some(DupesGo::Names);
                }
            }
            if r.lost_focus() || r.drag_stopped() {
                *out = Some(DupesGo::Names);
            }
            ui.add(TextEdit::singleline(&mut d.names_filter).hint_text("Filter by name or folder").desired_width(200.0));
        });
        plain_note(ui, RichText::new("Files with the same name in different folders, whatever is inside: a renamed copy is not shown here, and two files with the same name can be completely different. The Contents row tells what is known; Verify now reads them to be sure.").color(t.muted).size(theme::ts::SMALL));
        let Some(names) = d.names.take() else { return };
        self.names_body(ui, t, d, &names, out);
        d.names = Some(names);
    }

    fn names_body(&self, ui: &mut Ui, t: &Tokens, d: &mut DupesPage, names: &[diskroot_session::NameGroup], out: &mut Option<DupesGo>) {
        let Some(s) = &self.scanned else { return };
        let scan_root = s.summary().root;
        if names.is_empty() {
            ui.add_space(20.0);
            ui.label(RichText::new("No files share a name at this size. Try a smaller minimum.").color(t.muted));
            return;
        }
        let key = (d.names_filter.to_lowercase(), names.len());
        if d.names_shown_for.as_ref() != Some(&key) {
            d.names_shown = (0..names.len()).filter(|&i| key.0.is_empty() || names[i].files.iter().any(|f| f.path.to_lowercase().contains(&key.0))).collect();
            if !d.names_shown.contains(&d.names_sel) {
                d.names_sel = d.names_shown.first().copied().unwrap_or(0);
            }
            d.names_shown_for = Some(key);
        }
        ui.add_space(6.0);
        let bottom = 52.0;
        let h = (ui.available_height() - bottom).max(140.0);
        let w = ui.available_width();
        let left_w = (w * 0.36).clamp(240.0, 440.0);
        let today = diskroot_session::today_days();
        ui.horizontal_top(|ui| {
            fixed_box(ui, vec2(left_w, h), |ui| {
                card(t).inner_margin(Margin::same(6)).show(ui, |ui| {
                    ui.set_min_size(vec2(left_w - 14.0, h - 14.0));
                    ui.set_max_width(left_w - 14.0);
                    if d.names_shown.is_empty() {
                        ui.label(RichText::new("No set matches the filter.").color(t.faint));
                    }
                    ScrollArea::vertical().id_salt("name-sets").auto_shrink([false, false]).show_rows(ui, 40.0, d.names_shown.len(), |ui, range| {
                        for gi in d.names_shown[range].iter().copied() {
                            let g = &names[gi];
                            let ticked = d.names_pick[gi].iter().filter(|x| **x).count();
                            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                            let p = ui.painter();
                            if d.names_sel == gi {
                                p.rect_filled(r, CornerRadius::same(6), t.head);
                            } else if resp.hovered() {
                                p.rect_filled(r, CornerRadius::same(6), t.field);
                            }
                            let name = super::pages::one_line(ui, visible(&g.name), theme::semibold(theme::ts::SMALL), t.text, r.width() - 110.0);
                            let p = ui.painter();
                            p.galley(r.min + vec2(8.0, 3.0), name, t.text);
                            let what = if g.same_size() { "same size" } else { "sizes differ" };
                            p.text(r.min + vec2(8.0, 21.0), Align2::LEFT_TOP, format!("{} files · {what}", g.files.len()), theme::regular(theme::ts::CAPTION), t.muted);
                            p.text(pos2(r.max.x - 8.0, r.min.y + 4.0), Align2::RIGHT_TOP, fmt_bytes(g.total()), theme::num(theme::ts::SMALL), t.muted);
                            if ticked > 0 {
                                p.text(pos2(r.max.x - 8.0, r.min.y + 21.0), Align2::RIGHT_TOP, format!("{ticked} ticked"), theme::regular(theme::ts::MICRO), theme::warn());
                            }
                            resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, &g.name));
                            if resp.clicked() {
                                d.names_sel = gi;
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
                    let Some(g) = names.get(d.names_sel) else { return };
                    let gi = d.names_sel;
                    let (status, badge) = name_statuses(&d.hashes, &d.run_index, g, t);
                    let paths: Vec<String> = g.files.iter().map(|f| f.path.clone()).collect();
                    set_header(ui, visible(&g.name), badge, &mut d.compare, d.verifying.is_some(), &paths, out);
                    ui.label(RichText::new(format!("{} files named like this · {} in all · {}", g.files.len(), fmt_bytes(g.total()), if g.same_size() { "all the same size" } else { "sizes differ, so at least some are different files" })).color(t.muted));
                    ui.add_space(6.0);
                    if d.compare {
                        let rows: Vec<CmpRow> = g
                            .files
                            .iter()
                            .zip(&status)
                            .map(|(f, st)| CmpRow { path: f.path.clone(), size: f.size, node: resolve(s, &scan_root, f.id, &f.path), content: st.clone(), later: later::contains(&d.later, &f.path) })
                            .collect();
                        compare_grid(ui, t, ("cmp-name", gi), &rows, &mut d.meta, out);
                        return;
                    }
                    ScrollArea::vertical().id_salt(("name-set", gi)).auto_shrink([false, false]).show(ui, |ui| {
                        for (i, f) in g.files.iter().enumerate() {
                            let node = resolve(s, &scan_root, f.id, &f.path);
                            let on = d.names_pick[gi][i];
                            Frame::new().stroke(Stroke::new(1.0, if on { theme::warn() } else { t.line })).corner_radius(CornerRadius::same(t.radius)).inner_margin(Margin::symmetric(10, 5)).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    let mut tick = on;
                                    if ui.add_enabled(node.is_some(), egui::Checkbox::new(&mut tick, "")).on_hover_text("Tick to delete, mark for later or review it").changed() {
                                        d.names_pick[gi][i] = tick;
                                    }
                                    ui.add_sized(vec2(76.0, 20.0), egui::Label::new(RichText::new(fmt_bytes(f.size)).font(theme::num(theme::ts::BODY)))).on_hover_text(format!("{} bytes", fmt_count(f.size)));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        let flagged = later::contains(&d.later, &f.path);
                                        if later_button(ui, t, flagged) {
                                            *out = Some(if flagged { DupesGo::Unlater(vec![f.path.clone()]) } else { DupesGo::Later(vec![later_item(&f.path, f.size, format!("Same name as {} other file(s)", g.files.len() - 1))]) });
                                        }
                                        if let Some(n) = node
                                            && ui.small_button("Map").clicked()
                                        {
                                            *out = Some(DupesGo::Map(n));
                                        }
                                        if ui.small_button("Explorer").clicked() {
                                            *out = Some(DupesGo::Explorer(f.path.clone()));
                                        }
                                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                            ui.vertical(|ui| {
                                                let when = if f.mtime > 0 { format!(" · modified {}", diskroot_session::age_text(today, f.mtime)) } else { String::new() };
                                                // Same names differ by folder: keep the end of the path in view.
                                                let room = ((ui.available_width() / 6.2) as usize).saturating_sub(when.chars().count()).max(16);
                                                ui.add(egui::Label::new(RichText::new(format!("{}{when}", tail_path(&visible(parent_of(&f.path)), room))).font(theme::semibold(theme::ts::SMALL))).truncate()).on_hover_text(visible(&f.path));
                                                ui.add(egui::Label::new(RichText::new(&status[i].0).color(status[i].1).size(theme::ts::CAPTION)).truncate());
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
        // Bottom bar.
        let mut picked: Vec<(usize, usize)> = Vec::new();
        for (gi, p) in d.names_pick.iter().enumerate() {
            for (i, on) in p.iter().enumerate() {
                if *on {
                    picked.push((gi, i));
                }
            }
        }
        let bytes: u64 = picked.iter().map(|&(gi, i)| names[gi].files[i].size).sum();
        let n = picked.len();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(if n == 0 { "Nothing ticked. Tick files in a set to act on them.".to_owned() } else { format!("{n} ticked · {}", fmt_bytes(bytes)) }).font(theme::semibold(theme::ts::BODY)).color(if n == 0 { t.muted } else { t.text }));
            if n > 0 && ui.small_button("Untick all").clicked() {
                for p in &mut d.names_pick {
                    p.iter_mut().for_each(|x| *x = false);
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let nodes = || picked.iter().filter_map(|&(gi, i)| resolve(s, &scan_root, names[gi].files[i].id, &names[gi].files[i].path)).collect::<Vec<u32>>();
                let del = egui::Button::new(RichText::new(format!("Delete {n} now…")).color(Color32::WHITE).font(theme::semibold(theme::ts::BODY))).fill(theme::cat_color(5)).min_size(vec2(120.0, 30.0));
                if ui.add_enabled(n > 0, del).on_hover_text("Straight to the confirmation: Recycle Bin by default, or permanently if you tick that there").clicked() {
                    *out = Some(DupesGo::DeleteNow(nodes()));
                }
                if ui.add_enabled(n > 0, egui::Button::new(format!("Mark {n} for later")).min_size(vec2(0.0, 30.0))).clicked() {
                    *out = Some(DupesGo::Later(picked.iter().map(|&(gi, i)| later_item(&names[gi].files[i].path, names[gi].files[i].size, format!("Same name as {} other file(s)", names[gi].files.len() - 1))).collect()));
                }
                if ui.add_enabled(n > 0, egui::Button::new(format!("Review {n}…")).min_size(vec2(0.0, 30.0))).on_hover_text("Adds them to Review marked").clicked() {
                    *out = Some(DupesGo::Review(nodes().into_iter().map(|n| (Some(n), None)).collect()));
                }
            });
        });
    }

    // ------------------------------------------------------------------
    // Marked for later
    // ------------------------------------------------------------------

    pub(super) fn later_tab(&mut self, ui: &mut Ui, t: &Tokens, d: &mut DupesPage, out: &mut Option<DupesGo>) {
        let Some(s) = &self.scanned else { return };
        let scan_root = s.summary().root;
        plain_note(ui, RichText::new("Files you weren't sure about. The list is saved on this PC and is still here after a restart. Nothing on it is deleted unless you choose Delete.").color(t.muted));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(TextEdit::singleline(&mut d.later_filter).hint_text("Filter by name or folder").desired_width(220.0));
            let missing: Vec<String> = d.later.iter().filter(|l| d.later_exists.get(&l.path.to_lowercase()) == Some(&false)).map(|l| l.path.clone()).collect();
            if !missing.is_empty() && ui.button(format!("Take off the {} that are gone", missing.len())).clicked() {
                *out = Some(DupesGo::Unlater(missing));
            }
        });
        if d.later.is_empty() {
            ui.add_space(16.0);
            card(t).show(ui, |ui| {
                ui.set_width(ui.available_width());
                plain_note(ui, RichText::new("Nothing marked for later. On Identical files or Same names, use \"Later\" on a file or \"Mark for later\" for the ticked ones.").color(t.muted));
            });
            return;
        }
        let q = d.later_filter.to_lowercase();
        let shown: Vec<usize> = (0..d.later.len()).filter(|&i| q.is_empty() || d.later[i].path.to_lowercase().contains(&q)).collect();
        ui.add_space(6.0);
        let h = (ui.available_height() - 52.0).max(140.0);
        card(t).inner_margin(Margin::same(8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(h);
            ScrollArea::vertical().id_salt("later-list").auto_shrink([false, false]).show_rows(ui, 50.0, shown.len(), |ui, range| {
                for &i in &shown[range] {
                    let l = &d.later[i];
                    let lp = l.path.to_lowercase();
                    let exists = d.later_exists.get(&lp).copied().unwrap_or(true);
                    let node = s.find_rel(&rel_of(&scan_root, &l.path).unwrap_or_default()).filter(|&n| n != 0 && exists);
                    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::hover());
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(6.0, 3.0))).layout(Layout::left_to_right(Align::Center)));
                    let ui = &mut child;
                    let mut on = d.later_pick.contains(&lp);
                    if ui.checkbox(&mut on, "").changed() {
                        if on {
                            d.later_pick.insert(lp.clone());
                        } else {
                            d.later_pick.remove(&lp);
                        }
                    }
                    ui.add_sized(vec2(76.0, 20.0), egui::Label::new(RichText::new(fmt_bytes(l.size)).font(theme::num(theme::ts::BODY))));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Take off").on_hover_text("Remove it from this list (the file stays)").clicked() {
                            *out = Some(DupesGo::Unlater(vec![l.path.clone()]));
                        }
                        if let Some(n) = node
                            && ui.small_button("Map").clicked()
                        {
                            *out = Some(DupesGo::Map(n));
                        }
                        if exists && ui.small_button("Explorer").clicked() {
                            *out = Some(DupesGo::Explorer(l.path.clone()));
                        }
                        if exists && diskroot_session::can_preview(&l.path) && ui.small_button("Open").on_hover_text("Open it in its usual app").clicked() {
                            *out = Some(DupesGo::Preview(l.path.clone()));
                        }
                        ui.add_sized(vec2(130.0, 20.0), egui::Label::new(RichText::new(format!("marked {}", activity::local_text(l.added))).color(t.faint).size(theme::ts::CAPTION)));
                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.vertical(|ui| {
                                let name = RichText::new(visible(file_name(&l.path))).font(theme::semibold(theme::ts::SMALL)).color(if exists { t.text } else { t.faint });
                                ui.add(egui::Label::new(name).truncate()).on_hover_text(visible(&l.path));
                                let mut bits = vec![visible(parent_of(&l.path))];
                                if !l.why.is_empty() {
                                    bits.push(l.why.clone());
                                }
                                if !exists {
                                    bits.push("no longer there".into());
                                } else if node.is_none() {
                                    bits.push("not in this scan (scan its drive to delete it here)".into());
                                }
                                ui.add(egui::Label::new(RichText::new(bits.join(" · ")).color(t.muted).size(theme::ts::CAPTION)).truncate());
                            });
                        });
                    });
                }
            });
        });
        let picked: Vec<&later::Later> = d.later.iter().filter(|l| d.later_pick.contains(&l.path.to_lowercase())).collect();
        let n = picked.len();
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} on the list · {n} ticked", d.later.len())).font(theme::semibold(theme::ts::BODY)));
            if ui.small_button("Tick all shown").clicked() {
                for &i in &shown {
                    d.later_pick.insert(d.later[i].path.to_lowercase());
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let nodes = || picked.iter().filter_map(|l| s.find_rel(&rel_of(&scan_root, &l.path)?).filter(|&n| n != 0)).collect::<Vec<u32>>();
                let del = egui::Button::new(RichText::new(format!("Delete {n} now…")).color(Color32::WHITE).font(theme::semibold(theme::ts::BODY))).fill(theme::cat_color(5)).min_size(vec2(120.0, 30.0));
                if ui.add_enabled(n > 0, del).on_hover_text("Straight to the confirmation (only files in this scan)").clicked() {
                    *out = Some(DupesGo::DeleteNow(nodes()));
                }
                if ui.add_enabled(n > 0, egui::Button::new(format!("Review {n}…"))).clicked() {
                    *out = Some(DupesGo::Review(nodes().into_iter().map(|n| (Some(n), None)).collect()));
                }
                if ui.add_enabled(n > 0, egui::Button::new(format!("Take {n} off the list"))).clicked() {
                    *out = Some(DupesGo::Unlater(picked.iter().map(|l| l.path.clone()).collect()));
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::tail_path;

    #[test]
    fn tail_path_keeps_the_end() {
        assert_eq!(tail_path(r"C:\a\b", 20), r"C:\a\b");
        assert_eq!(tail_path(r"C:\Users\me\AppData\Local\Temp\dr-dupe-test\c", 20), r"…\dr-dupe-test\c");
        assert!(tail_path(r"C:\averyveryverylongfoldername", 10).starts_with('…'));
    }
}
