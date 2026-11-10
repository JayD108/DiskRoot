//! Drishti desktop app: Tauri backend.
//!
//! Holds the scanned tree in memory and answers small queries from the UI:
//! a pruned view for the diagram, a ranked child list, breadcrumbs. The UI
//! never receives the full tree.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod elevate;

use std::path::Path;
use std::sync::{Arc, Mutex};

use drishti_core::view::{Children, ViewLimits, ViewNode, ancestors, build_view};
use drishti_core::{Index, ScanStats, flags};
use drishti_scan_mft::{Mode, Used, scan_auto};
use serde::Serialize;
use tauri::State;

struct Loaded {
    index: Index,
    children: Children,
    stats: ScanStats,
    mode: String,
}

impl Loaded {
    fn new(index: Index, stats: ScanStats, mode: String) -> Self {
        let children = Children::build(&index);
        Self { index, children, stats, mode }
    }
}

#[derive(Default)]
struct AppState {
    loaded: Mutex<Option<Arc<Loaded>>>,
}

impl AppState {
    fn get(&self) -> Result<Arc<Loaded>, String> {
        self.loaded
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .ok_or_else(|| "Nothing scanned yet".to_owned())
    }

    fn set(&self, l: Loaded) -> Summary {
        let s = summary(&l);
        *self.loaded.lock().unwrap_or_else(|p| p.into_inner()) = Some(Arc::new(l));
        s
    }
}

#[derive(Serialize)]
struct Summary {
    root: String,
    mode: String,
    elapsed_ms: f64,
    files: u64,
    dirs: u64,
    unreadable: u64,
    size: u64,
    on_disk: u64,
    admin_hint: bool,
}

fn summary(l: &Loaded) -> Summary {
    Summary {
        root: l.index.name(0).to_owned(),
        mode: l.mode.clone(),
        elapsed_ms: l.stats.elapsed.as_secs_f64() * 1000.0,
        files: l.stats.files,
        dirs: l.stats.dirs,
        unreadable: l.stats.unreadable,
        size: l.index.size(0),
        on_disk: l.index.alloc(0),
        admin_hint: l.mode.contains("administrator"),
    }
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
}

#[derive(Serialize)]
struct Crumb {
    id: u32,
    name: String,
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

fn check_node(l: &Loaded, node: u32) -> Result<(), String> {
    if (node as usize) < l.index.len() { Ok(()) } else { Err(format!("No such item: {node}")) }
}

#[tauri::command]
fn view(state: State<'_, AppState>, node: u32, depth: u8) -> Result<Vec<ViewNode>, String> {
    let l = state.get()?;
    check_node(&l, node)?;
    let lim = ViewLimits { max_depth: depth.clamp(1, 10), ..ViewLimits::default() };
    Ok(build_view(&l.index, &l.children, node, &lim))
}

#[derive(Serialize)]
struct ListOut {
    /// All children of the folder (the list may be cut at `limit`).
    total: usize,
    items: Vec<Item>,
}

#[tauri::command]
fn list(state: State<'_, AppState>, node: u32, limit: usize) -> Result<ListOut, String> {
    let l = state.get()?;
    check_node(&l, node)?;
    let idx = &l.index;
    let kids = l.children.of(node);
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
        })
        .collect();
    Ok(ListOut { total: kids.len(), items })
}

#[tauri::command]
fn crumbs(state: State<'_, AppState>, node: u32) -> Result<Vec<Crumb>, String> {
    let l = state.get()?;
    check_node(&l, node)?;
    Ok(ancestors(&l.index, node).into_iter().map(|(id, name)| Crumb { id, name }).collect())
}

/// Shows the item selected in Windows Explorer.
#[tauri::command]
fn reveal(state: State<'_, AppState>, node: u32) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let l = state.get()?;
    check_node(&l, node)?;
    let path = l.index.path(node);
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
            list_drives,
            initial_path,
            scan,
            scan_admin,
            view,
            list,
            crumbs,
            reveal
        ])
        .run(tauri::generate_context!())
        .expect("error while running Drishti");
}
