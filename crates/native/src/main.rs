//! Xylem: native desktop app (the default UI).
//!
//! One small exe:
//! - `xylem.exe [path]` opens the window (and scans `path` if given).
//! - `xylem.exe --helper-scan C: --save-index <file>` is the elevated fast-
//!   scan helper the app starts through the UAC prompt; it never opens a
//!   window (see `xylem_session::helper`).
//!
//! The UI is egui (immediate mode, OpenGL): no browser engine, one process,
//! and it only redraws on input, so an idle window costs ~nothing.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod map;
mod settings;
mod theme;

use std::sync::Arc;

use eframe::egui::{IconData, ViewportBuilder};

/// 1.52 TB style sizes (1 KB = 1024 bytes, like Explorer).
pub fn fmt_bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[u])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[u])
    } else {
        format!("{v:.2} {}", UNITS[u])
    }
}

/// 1,234,567 style counts.
pub fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// "1 file" / "12,345 files".
pub fn fmt_files(n: u64) -> String {
    format!("{} {}", fmt_count(n), if n == 1 { "file" } else { "files" })
}

pub fn fmt_pct(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "0%".into();
    }
    let p = part as f64 * 100.0 / whole as f64;
    if p >= 10.0 { format!("{p:.0}%") } else { format!("{p:.1}%") }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Elevated helper mode: no window, just scan and exit.
    if let Some(code) = xylem_session::helper::run_if_helper(&args) {
        std::process::exit(code);
    }

    let icon = IconData { rgba: include_bytes!("../assets/icon-64.rgba").to_vec(), width: 64, height: 64 };
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("Xylem")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([960.0, 600.0])
            .with_icon(Arc::new(icon)),
        ..Default::default()
    };
    let start = args.get(1).filter(|a| !a.starts_with('-')).cloned();
    let result = eframe::run_native("Xylem", options, Box::new(move |cc| Ok(Box::new(app::XylemApp::new(cc, start)))));
    if let Err(e) = result {
        eprintln!("Xylem could not start: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_bytes(1023), "1023 B");
        assert_eq!(fmt_bytes(15 * 1024 * 1024), "15.0 MB");
        assert_eq!(fmt_count(1_234_567), "1,234,567");
        assert_eq!(fmt_pct(1, 3), "33%");
        assert_eq!(fmt_pct(1, 300), "0.3%");
        assert_eq!(fmt_pct(1, 0), "0%");
    }
}
