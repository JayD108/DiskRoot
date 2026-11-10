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
}

impl Default for Settings {
    fn default() -> Self {
        Self { dark: false, treemap: false, by_category: false, last_path: "C:\\".into() }
    }
}

fn file() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("Xylem").join("settings.json"))
}

impl Settings {
    pub fn load() -> Self {
        file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
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
