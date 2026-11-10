//! Drishti desktop app: Tauri backend.
//!
//! Holds the scanned tree, its classification and category totals in
//! memory, and answers small queries from the UI: a pruned view for the
//! diagram, a ranked child list, breadcrumbs, the by-category view, item
//! info, and safe delete. The UI never receives the full tree.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod elevate;
mod recycle;

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};
use std::time::Instant;

use drishti_core::classify::{CATEGORIES, CatCrumb, CatKey, CatModel, CatStats, Category, Classification, RuleInfo, Rules, SubType};
use drishti_core::view::{Children, ViewLimits, ViewNode, ancestors, build_view};
use drishti_core::{Index, ScanStats, flags};
use drishti_scan_mft::{Mode, Used, scan_auto};
use serde::Serialize;
use tauri::State;

static RULES: LazyLock<Rules> = LazyLock::new(Rules::builtin);

struct Loaded {
    index: Index,
    children: Children,
    cls: Classification,
    cats: CatStats,
    stats: ScanStats,
    mode: String,
    classify_ms: f64,
}

impl Loaded {
    fn new(index: Index, stats: ScanStats, mode: String) -> Self {
        let t = Instant::now();
        let children = Children::build(&index);
        let cls = Classification::run(&index, &RULES);
        let cats = CatStats::compute(&index, &cls, RULES.subs.len());
        let classify_ms = t.elapsed().as_secs_f64() * 1000.0;
        Self { index, children, cls, cats, stats, mode, classify_ms }
    }

    fn model(&self) -> CatModel<'_> {
        CatModel { idx: &self.index, cls: &self.cls, stats: &self.cats, rules: &RULES }
    }

    fn check(&self, node: u32) -> Result<(), String> {
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
}

#[derive(Default)]
struct AppState {
    loaded: RwLock<Option<Loaded>>,
}

impl AppState {
    fn read<R>(&self, f: impl FnOnce(&Loaded) -> Result<R, String>) -> Result<R, String> {
        let guard = self.loaded.read().unwrap_or_else(|p| p.into_inner());
        f(guard.as_ref().ok_or("Nothing scanned yet")?)
    }

    fn write<R>(&self, f: impl FnOnce(&mut Loaded) -> Result<R, String>) -> Result<R, String> {
        let mut guard = self.loaded.write().unwrap_or_else(|p| p.into_inner());
        f(guard.as_mut().ok_or("Nothing scanned yet")?)
    }

    fn set(&self, l: Loaded) -> Summary {
        let s = summary(&l);
        *self.loaded.write().unwrap_or_else(|p| p.into_inner()) = Some(l);
        s
    }
}

#[derive(Serialize)]
struct Summary {
    root: String,
    mode: String,
    elapsed_ms: f64,
    classify_ms: f64,
    files: u64,
    dirs: u64,
    unreadable: u64,
    size: u64,
    on_disk: u64,
    /// Share of bytes in a real category (not Other).
    categorized: f64,
    admin_hint: bool,
}

fn summary(l: &Loaded) -> Summary {
    let total: u64 = l.cats.cat_total.iter().sum();
    let other = l.cats.cat_total[drishti_core::classify::CAT_OTHER as usize];
    Summary {
        root: l.index.name(0).to_owned(),
        mode: l.mode.clone(),
        elapsed_ms: l.stats.elapsed.as_secs_f64() * 1000.0,
        classify_ms: l.classify_ms,
        files: l.stats.files,
        dirs: l.stats.dirs,
        unreadable: l.stats.unreadable,
        size: l.index.size(0),
        on_disk: l.index.alloc(0),
        categorized: if total > 0 { (total - other) as f64 / total as f64 } else { 0.0 },
        admin_hint: l.mode.contains("administrator"),
    }
}

#[derive(Serialize)]
struct Taxonomy {
    cats: &'static [Category],
    subs: &'static [SubType],
    rules: &'static [RuleInfo],
}

/// Category names, sub-types and rule notes (sent once at start-up).
#[tauri::command]
fn taxonomy() -> Taxonomy {
    Taxonomy { cats: &CATEGORIES, subs: &RULES.subs, rules: &RULES.info }
}

#[tauri::command]
fn list_drives() -> Vec<String> {
    ('A'..='Z')
        .map(|c| format!("{c}:\\"))
        .filter(|d| Path::new(d).exists())
        .collect()
}

/// A path passed on the command line (e.g. from Explorer) to scan at start.
#[tauri::command]
fn initial_path() -> Option<String> {
    std::env::args().nth(1).filter(|a| !a.starts_with('-'))
}

fn mode_text(used: &Used) -> String {
    match used {
        Used::Mft => "MFT (direct read)".into(),
        Used::Walk { fallback_reason: None } => "folder walk".into(),
        Used::Walk { fallback_reason: Some(r) } => format!("folder walk ({r})"),
    }
}

#[tauri::command]
async fn scan(state: State<'_, AppState>, path: String) -> Result<Summary, String> {
    let loaded = tauri::async_runtime::spawn_blocking(move || {
        let threads = drishti_scan_walk::default_threads();
        let (s, used) = scan_auto(Path::new(&path), Mode::Auto, threads).map_err(|e| format!("Scan failed: {e}"))?;
        Ok::<_, String>(Loaded::new(s.index, s.stats, mode_text(&used)))
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(state.set(loaded))
}

/// Fast MFT scan through the elevated helper (shows a UAC prompt).
#[tauri::command]
async fn scan_admin(state: State<'_, AppState>, drive: String) -> Result<Summary, String> {
    let loaded = tauri::async_runtime::spawn_blocking(move || {
        let (index, stats, mode) = elevate::scan_with_helper(&drive)?;
        Ok::<_, String>(Loaded::new(index, stats, mode))
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(state.set(loaded))
}

#[tauri::command]
fn summary_now(state: State<'_, AppState>) -> Result<Summary, String> {
    state.read(|l| Ok(summary(l)))
}

#[tauri::command]
fn view(state: State<'_, AppState>, node: u32, depth: u8) -> Result<Vec<ViewNode>, String> {
    state.read(|l| {
        l.check(node)?;
        let lim = ViewLimits { max_depth: depth.clamp(1, 10), ..ViewLimits::default() };
        let mut nodes = build_view(&l.index, &l.children, node, &lim);
        l.decorate(&mut nodes);
        Ok(nodes)
    })
}

#[derive(Serialize)]
struct Item {
    id: u32,
    name: String,
    size: u64,
    on_disk: u64,
    files: u32,
    dir: bool,
    link: bool,
    cloud: bool,
    /// Colour category, own sub-type and rule (for notes).
    cat: u8,
    sub: u16,
    rule: u16,
    protected: bool,
}

#[derive(Serialize)]
struct ListOut {
    /// All children of the folder (the list may be cut at `limit`).
    total: usize,
    items: Vec<Item>,
    /// Bytes per category inside this folder, biggest first.
    breakdown: Vec<(u8, u64)>,
}

#[tauri::command]
fn list(state: State<'_, AppState>, node: u32, limit: usize) -> Result<ListOut, String> {
    state.read(|l| {
        l.check(node)?;
        let idx = &l.index;
        let kids: Vec<u32> = l.children.live(idx, node).collect();
        let items = kids
            .iter()
            .take(limit.min(2000))
            .map(|&c| Item {
                id: c,
                name: idx.name(c).to_owned(),
                size: idx.size(c),
                on_disk: idx.alloc(c),
                files: idx.files(c),
                dir: idx.is_dir(c),
                link: idx.flags(c) & flags::LINK != 0,
                cloud: idx.flags(c) & flags::CLOUD != 0,
                cat: l.cats.dominant[c as usize],
                sub: l.cls.sub[c as usize],
                rule: l.cls.rule[c as usize],
                protected: l.cls.protection(idx, &RULES, c).is_some(),
            })
            .collect();
        let mut breakdown: Vec<(u8, u64)> = l
            .cats
            .breakdown(node)
            .into_iter()
            .enumerate()
            .filter(|&(_, v)| v > 0)
            .map(|(c, v)| (c as u8, v))
            .collect();
        breakdown.sort_unstable_by_key(|&(_, v)| std::cmp::Reverse(v));
        Ok(ListOut { total: kids.len(), items, breakdown })
    })
}

#[derive(Serialize)]
struct Crumb {
    id: u32,
    name: String,
}

#[tauri::command]
fn crumbs(state: State<'_, AppState>, node: u32) -> Result<Vec<Crumb>, String> {
    state.read(|l| {
        l.check(node)?;
        Ok(ancestors(&l.index, node).into_iter().map(|(id, name)| Crumb { id, name }).collect())
    })
}

#[derive(Serialize)]
struct CatViewOut {
    nodes: Vec<ViewNode>,
    crumbs: Vec<CatCrumb>,
}

#[tauri::command]
fn category_view(state: State<'_, AppState>, key: String) -> Result<CatViewOut, String> {
    state.read(|l| {
        let k = CatKey::parse(&key, &RULES).ok_or("Unknown category")?;
        let m = l.model();
        let mut nodes = m.view(k, 0.002);
        l.decorate(&mut nodes);
        Ok(CatViewOut { nodes, crumbs: m.crumbs(k) })
    })
}

/// Everything the delete dialog shows, including whether deleting is
/// allowed.
#[derive(Serialize)]
struct ItemInfo {
    id: u32,
    name: String,
    path: String,
    dir: bool,
    on_disk: u64,
    files: u32,
    cat: u8,
    sub: u16,
    rule: u16,
    protected: Option<String>,
    /// The folder to open for this item (itself, or its parent for files).
    folder: u32,
}

fn item_info(l: &Loaded, node: u32) -> ItemInfo {
    let idx = &l.index;
    let i = node as usize;
    let parent = idx.parent(node);
    ItemInfo {
        id: node,
        name: idx.name(node).to_owned(),
        path: idx.path(node),
        dir: idx.is_dir(node),
        on_disk: idx.alloc(node),
        files: idx.files(node),
        cat: l.cls.cat[i],
        sub: l.cls.sub[i],
        rule: l.cls.rule[i],
        protected: l.cls.protection(idx, &RULES, node),
        folder: if idx.is_dir(node) || parent == drishti_core::NO_PARENT { node } else { parent },
    }
}

#[tauri::command]
fn info(state: State<'_, AppState>, node: u32) -> Result<ItemInfo, String> {
    state.read(|l| {
        l.check(node)?;
        Ok(item_info(l, node))
    })
}

#[derive(Serialize)]
struct DeleteOut {
    freed: u64,
    name: String,
    cancelled: bool,
}

/// Moves an item to the Recycle Bin, then removes it from the in-memory
/// tree. Protected items are refused here, not just greyed out in the UI.
#[tauri::command]
async fn delete(state: State<'_, AppState>, node: u32) -> Result<DeleteOut, String> {
    let (path, name, freed) = state.read(|l| {
        l.check(node)?;
        let info = item_info(l, node);
        if let Some(why) = info.protected {
            return Err(why);
        }
        Ok((PathBuf::from(info.path), info.name, info.on_disk))
    })?;
    let outcome = tauri::async_runtime::spawn_blocking(move || recycle::to_recycle_bin(&path))
        .await
        .map_err(|e| e.to_string())??;
    if matches!(outcome, recycle::Outcome::Cancelled) {
        return Ok(DeleteOut { freed: 0, name, cancelled: true });
    }
    state.write(|l| {
        l.index.remove_subtree(node, &l.children);
        l.cats = CatStats::compute(&l.index, &l.cls, RULES.subs.len());
        Ok(())
    })?;
    Ok(DeleteOut { freed, name, cancelled: false })
}

/// Shows the item selected in Windows Explorer.
#[tauri::command]
fn reveal(state: State<'_, AppState>, node: u32) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let path = state.read(|l| {
        l.check(node)?;
        Ok(l.index.path(node))
    })?;
    if path.contains('"') {
        return Err("Path contains a quote character".into());
    }
    std::process::Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{path}\""))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open Explorer: {e}"))
}

fn main() {
    tauri::Builder::default()
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            taxonomy,
            list_drives,
            initial_path,
            scan,
            scan_admin,
            summary_now,
            view,
            list,
            crumbs,
            category_view,
            info,
            delete,
            reveal
        ])
        .run(tauri::generate_context!())
        .expect("error while running Drishti");
}
