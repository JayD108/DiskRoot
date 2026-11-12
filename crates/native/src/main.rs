//! DiskRoot: native desktop app (the default UI).
//!
//! One small exe:
//! - `diskroot.exe [path]` opens the window (and scans `path` if given).
//! - `diskroot.exe --helper-scan C: --pipe <private pipe>` is the elevated fast-
//!   scan helper the app starts through the UAC prompt; it never opens a
//!   window (see `diskroot_session::helper`).
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
    if part < whole && p >= 99.0 {
        // Rounding would say "100%" for something that isn't everything.
        return format!("{:.1}%", (p * 10.0).floor() / 10.0);
    }
    if p >= 10.0 { format!("{p:.0}%") } else { format!("{p:.1}%") }
}

/// Drawing: Direct3D 12 through wgpu. Any graphics chip works (the low-power
/// one is preferred: an interface needs little); with no graphics driver
/// at all (virtual machines, Remote Desktop, "Microsoft Basic Display
/// Adapter"), Windows' built-in software renderer WARP draws on the CPU.
/// DISKROOT_RENDERER=software forces WARP (for testing slow PCs).
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, wgpu};
    let force_software = std::env::var("DISKROOT_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("software"));
    let mut cfg = WgpuConfiguration::default();
    // A UI redraws only on change: one frame in flight is enough, and lowest latency.
    cfg.surface = eframe::egui_wgpu::SurfaceConfig { present_mode: wgpu::PresentMode::AutoNoVsync, desired_maximum_frame_latency: Some(1) };
    if let WgpuSetup::CreateNew(new) = &mut cfg.wgpu_setup {
        new.instance_descriptor.backends = wgpu::Backends::DX12;
        new.power_preference = wgpu::PowerPreference::LowPower;
        new.native_adapter_selector = Some(Arc::new(move |adapters: &[wgpu::Adapter], surface: Option<&wgpu::Surface<'_>>| {
            let fits = |a: &&wgpu::Adapter| surface.is_none_or(|s| a.is_surface_supported(s));
            let software = |a: &&wgpu::Adapter| a.get_info().device_type == wgpu::DeviceType::Cpu;
            let rank = |a: &&wgpu::Adapter| match a.get_info().device_type {
                wgpu::DeviceType::IntegratedGpu => 0,
                wgpu::DeviceType::DiscreteGpu => 1,
                wgpu::DeviceType::VirtualGpu | wgpu::DeviceType::Other => 2,
                wgpu::DeviceType::Cpu => 3,
            };
            let pick = if force_software {
                adapters.iter().filter(fits).find(software)
            } else {
                adapters.iter().filter(fits).min_by_key(rank)
            };
            pick.cloned().ok_or_else(|| "no Direct3D 12 adapter (not even Windows' software renderer)".to_owned())
        }));
    }
    cfg
}

fn main() {
    // Before anything can load a DLL: System32 only (DLL planting defence).
    diskroot_session::integrity::harden_dll_search();
    let args: Vec<String> = std::env::args().collect();
    // Elevated helper mode: no window, just scan and exit.
    if let Some(code) = diskroot_session::helper::run_if_helper(&args) {
        std::process::exit(code);
    }
    // Elevated virtual-disk compaction (View > details of a .vhdx): no window.
    if let Some(code) = diskroot_session::power::run_if_compact_helper(&args) {
        std::process::exit(code);
    }
    // Scheduled scan (Task Scheduler, as the user, no admin): no window.
    if let Some(code) = diskroot_session::schedule::run_if_scheduled(&args) {
        std::process::exit(code);
    }

    // OpenGL by default: lightest (about 110 MB less memory than Direct3D 12
    // on our test PC). Direct3D 12 / WARP only when asked, or as the
    // automatic fallback when OpenGL can't start (no graphics driver).
    let use_d3d = std::env::var("DISKROOT_RENDERER").is_ok_and(|v| v.eq_ignore_ascii_case("software") || v.eq_ignore_ascii_case("d3d12"));
    let icon = IconData { rgba: include_bytes!("../assets/icon-64.rgba").to_vec(), width: 64, height: 64 };
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_title("DiskRoot")
            .with_inner_size([1360.0, 860.0])
            .with_min_inner_size([960.0, 600.0])
            .with_icon(Arc::new(icon)),
        renderer: if use_d3d { eframe::Renderer::Wgpu } else { eframe::Renderer::Glow },
        // Test switch for the fallback: ask OpenGL for an impossible setting so
        // it refuses to start, like on a PC without a graphics driver.
        multisampling: if !use_d3d && std::env::var_os("DISKROOT_TEST_GL_FAIL").is_some() { 128 } else { 0 },
        wgpu_options: wgpu_options(),
        ..Default::default()
    };
    // Explorer's right-click menu passes "%V"; for a drive that is "C:\",
    // which Windows' argument rules turn into `C:"`. Drop the stray quote.
    let start = args
        .get(1)
        .map(|a| a.trim().trim_end_matches('"'))
        .filter(|a| !a.is_empty() && !a.starts_with('-'))
        .map(str::to_owned);
    // OpenGL may fail with an error or a panic (eframe panics when no
    // matching OpenGL configuration exists). Either way, before the window
    // first appeared, it's a renderer problem: try Direct3D 12 instead.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        eframe::run_native("DiskRoot", options, Box::new(move |cc| Ok(Box::new(app::DiskRootApp::new(cc, start)))))
    }));
    let result = match result {
        Ok(r) => r.map_err(|e| e.to_string()),
        Err(_) if app::STARTED.load(std::sync::atomic::Ordering::Relaxed) => std::process::exit(101), // a real crash
        Err(_) => Err("the renderer didn't start".to_owned()),
    };
    if let Err(e) = result {
        // OpenGL didn't start (e.g. no graphics driver: virtual machines,
        // Remote Desktop, "Microsoft Basic Display Adapter"). Start again with
        // Direct3D 12, which falls back to Windows' software renderer. A new
        // process, because the window system can only start once per process.
        if !use_d3d
            && let Ok(exe) = std::env::current_exe()
            && std::process::Command::new(exe).args(&args[1..]).env("DISKROOT_RENDERER", "d3d12").spawn().is_ok()
        {
            std::process::exit(0); // hand over to the new process
        }
        eprintln!("DiskRoot could not start: {e}");
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
        assert_eq!(fmt_pct(9952, 10_000), "99.5%", "not everything: never 100%");
        assert_eq!(fmt_pct(9999, 10_000), "99.9%");
        assert_eq!(fmt_pct(10, 10), "100%");
    }
}
