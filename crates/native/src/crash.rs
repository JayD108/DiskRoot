//! When DiskRoot hits a bug: write a short report and say so, instead of the
//! window just vanishing (a GUI app has no console for the panic message).
//!
//! - Every panic writes `%LOCALAPPDATA%\DiskRoot\crashes\crash-<time>.txt`
//!   (version, thread, source location, message; at most 3 per run).
//! - A panic on the window's thread also shows a Windows message box, because
//!   the window closes right after.
//! - Worker threads that panic are only reported: the UI notices their channel
//!   closed and says which task stopped (see `app.rs`).
//! - The next start offers the newest report (`take_unseen`).

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn MessageBoxW(owner: *mut c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
}

const MB_ICONERROR: u32 = 0x10;
const MB_OK: u32 = 0;
const MB_TOPMOST: u32 = 0x0004_0000;
const MB_SETFOREGROUND: u32 = 0x0001_0000;

static WRITTEN: AtomicU32 = AtomicU32::new(0);
static LAST: Mutex<Option<PathBuf>> = Mutex::new(None);

fn dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("DiskRoot").join("crashes"))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The report text (no file contents; the message itself may name a path).
fn report(thread: &str, location: &str, message: &str) -> String {
    format!(
        "DiskRoot crash report\r\n\
         Version: {}\r\n\
         Time (Unix seconds): {}\r\n\
         Thread: {thread}\r\n\
         Where: {location}\r\n\
         Message: {message}\r\n\r\n\
         This file stays on this PC. DiskRoot never sends it anywhere.\r\n\
         To help fix the bug, attach it to an issue at github.com/JayD108/DiskRoot/issues\r\n\
         (check the message first: it can contain a file or folder name).\r\n",
        env!("CARGO_PKG_VERSION"),
        now_secs()
    )
}

fn write(text: &str, window: bool) -> Option<PathBuf> {
    let d = dir()?;
    std::fs::create_dir_all(&d).ok()?;
    let f = d.join(format!("crash-{}-{}.txt", now_secs(), std::process::id()));
    std::fs::write(&f, text).ok()?;
    // The window closed: the next start offers the report. (A worker's
    // report is mentioned right away by the UI instead.)
    if window {
        let _ = std::fs::write(d.join("unseen.txt"), f.to_string_lossy().as_bytes());
    }
    Some(f)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// Install the panic hook. Call once, early in `main`.
pub fn install() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        let location = info.location().map_or_else(|| "unknown".into(), |l| format!("{}:{}", l.file(), l.line()));
        let current = std::thread::current();
        let thread = current.name().unwrap_or("worker").to_owned();
        if WRITTEN.fetch_add(1, Ordering::SeqCst) >= 3 {
            return;
        }
        let file = write(&report(&thread, &location, &message), thread == "main");
        if let Ok(mut l) = LAST.lock() {
            l.clone_from(&file);
        }
        if thread == "main" {
            let where_ = file.map_or_else(|| "A report could not be saved.".to_owned(), |f| format!("A short report was saved here:\n{}", f.display()));
            let text = wide(&format!(
                "DiskRoot hit a bug and has to close.\n\nScanning only reads, so the scan didn't change your files. Anything you deleted earlier is listed in File > Activity.\n\n{where_}"
            ));
            // On a thread of its own: the window's thread is inside a window
            // callback here, where a message box may not show.
            let shown = std::thread::spawn(move || {
                let caption = wide("DiskRoot");
                // SAFETY: NUL-terminated UTF-16 strings that outlive the call.
                unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), MB_OK | MB_ICONERROR | MB_TOPMOST | MB_SETFOREGROUND) };
            });
            let _ = shown.join();
        }
    }));
}

/// The report written in this run, if a worker thread hit a bug.
pub fn last() -> Option<PathBuf> {
    LAST.lock().ok().and_then(|l| l.clone())
}

/// A report from an earlier run that the user hasn't been told about.
pub fn take_unseen() -> Option<PathBuf> {
    let marker = dir()?.join("unseen.txt");
    let path = std::fs::read_to_string(&marker).ok()?;
    let _ = std::fs::remove_file(&marker);
    let p = PathBuf::from(path.trim());
    p.exists().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_names_version_and_place() {
        let r = report("main", "src/app.rs:10", "index out of bounds");
        assert!(r.contains(env!("CARGO_PKG_VERSION")));
        assert!(r.contains("src/app.rs:10"));
        assert!(r.contains("never sends it"));
    }
}
