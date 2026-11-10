//! Xylem web UI: the optional add-on (Tauri + WebView2).
//!
//! A thin layer: every command forwards to `xylem-session`, the same engine
//! API the native UI uses, so both UIs behave identically. The fast-scan
//! helper is `xylem.exe` (the native app), which must sit next to this exe.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::RwLock;

use serde::Serialize;
use tauri::State;
use xylem_session::{
    CatViewOut, Category, Crumb, DeleteOut, ItemInfo, ListOut, RULES, RuleInfo, Scanned, Shape, SubType, Summary,
};

#[derive(Default)]
struct AppState {
    scanned: RwLock<Option<Scanned>>,
}

impl AppState {
    fn read<R>(&self, f: impl FnOnce(&Scanned) -> Result<R, String>) -> Result<R, String> {
        let guard = self.scanned.read().unwrap_or_else(|p| p.into_inner());
        f(guard.as_ref().ok_or("Nothing scanned yet")?)
    }

    fn set(&self, s: Scanned) -> Summary {
        let summary = s.summary();
        *self.scanned.write().unwrap_or_else(|p| p.into_inner()) = Some(s);
        summary
    }
}

#[derive(Serialize)]
struct Taxonomy {
    cats: &'static [Category],
    subs: &'static [SubType],
    rules: &'static [RuleInfo],
}

#[tauri::command]
fn taxonomy() -> Taxonomy {
    Taxonomy { cats: xylem_session::categories(), subs: &RULES.subs, rules: &RULES.info }
}

#[tauri::command]
fn list_drives() -> Vec<String> {
    xylem_session::drives()
}

/// A path passed on the command line (e.g. from Explorer) to scan at start.
#[tauri::command]
fn initial_path() -> Option<String> {
    std::env::args().nth(1).filter(|a| !a.starts_with('-'))
}

#[tauri::command]
async fn scan(state: State<'_, AppState>, path: String) -> Result<Summary, String> {
    let s = tauri::async_runtime::spawn_blocking(move || xylem_session::scan(&path)).await.map_err(|e| e.to_string())??;
    Ok(state.set(s))
}

#[tauri::command]
async fn scan_admin(state: State<'_, AppState>, drive: String) -> Result<Summary, String> {
    let s = tauri::async_runtime::spawn_blocking(move || xylem_session::scan_admin(&drive))
        .await
        .map_err(|e| e.to_string())??;
    Ok(state.set(s))
}

#[tauri::command]
fn summary_now(state: State<'_, AppState>) -> Result<Summary, String> {
    state.read(|s| Ok(s.summary()))
}

#[tauri::command]
fn view(state: State<'_, AppState>, node: u32, depth: u8) -> Result<Vec<Shape>, String> {
    state.read(|s| s.view(node, depth))
}

#[tauri::command]
fn list(state: State<'_, AppState>, node: u32, limit: usize) -> Result<ListOut, String> {
    state.read(|s| s.list(node, limit.min(2000)))
}

#[tauri::command]
fn crumbs(state: State<'_, AppState>, node: u32) -> Result<Vec<Crumb>, String> {
    state.read(|s| s.crumbs(node))
}

#[tauri::command]
fn category_view(state: State<'_, AppState>, key: String) -> Result<CatViewOut, String> {
    state.read(|s| s.category_view(&key))
}

#[tauri::command]
fn info(state: State<'_, AppState>, node: u32) -> Result<ItemInfo, String> {
    state.read(|s| s.info(node))
}

/// Recycle Bin delete. Protection is enforced by the session, not the UI.
#[tauri::command]
async fn delete(state: State<'_, AppState>, node: u32) -> Result<DeleteOut, String> {
    let pending = state.read(|s| s.prepare_delete(node))?;
    let out = tauri::async_runtime::spawn_blocking(move || pending.recycle()).await.map_err(|e| e.to_string())??;
    if !out.cancelled {
        let mut guard = state.scanned.write().unwrap_or_else(|p| p.into_inner());
        if let Some(s) = guard.as_mut() {
            s.apply_delete(node);
        }
    }
    Ok(out)
}

#[tauri::command]
fn reveal(state: State<'_, AppState>, node: u32) -> Result<(), String> {
    let path = state.read(|s| s.path(node))?;
    xylem_session::reveal(&path)
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
        .expect("error while running Xylem (web UI)");
}
