//! Remembered UI preferences, stored as a small JSON file in
//! `%APPDATA%\Xylem\settings.json`. Purely local; failures are ignored
//! (the app just starts with defaults).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub dark: bool,
    pub treemap: bool,
    pub by_category: bool,
    pub last_path: String,
    /// The first-launch notice was shown and confirmed.
    pub seen_notice: bool,
    /// Save scans on this PC for instant reopen and "what changed".
    pub remember_scans: bool,
    /// Colour the map and tree by last-modified date instead of category.
    pub color_by_age: bool,
    /// Watch the scanned folder while the app is open and update sizes.
    pub live_updates: bool,
    /// "Tell me if this folder grows by more than N GB".
    pub alerts: Vec<Alert>,
    /// Free the scan from memory after the window has been minimized for
    /// 10 minutes (it stays saved; reopening takes about a second).
    pub free_when_minimized: bool,
    /// Scan in Windows' background mode so other programs stay fast.
    pub gentle_scan: bool,
}

/// A size alert on a folder (checked after scans and live updates while
/// Xylem is open).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Alert {
    /// Full path of the folder.
    pub path: String,
    /// Size on disk when the alert was set (or last fired).
    pub baseline: u64,
    /// Fire when it grows by at least this many bytes.
    pub grow: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Self { dark: false, treemap: false, by_category: false, last_path: "C:\\".into(), seen_notice: false, remember_scans: true, color_by_age: false, live_updates: true, alerts: Vec::new(), free_when_minimized: false, gentle_scan: false }
    }
}

fn file() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Xylem").join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            // Editors like older Notepad save a byte-order mark; accept it.
            .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(f) = file() else { return };
        if let Some(dir) = f.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(f, s);
        }
    }
}
