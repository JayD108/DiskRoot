//! The elevated scan helper, built into `xylem.exe`.
//!
//! When the app needs a fast MFT scan it starts itself through the UAC
//! prompt as `xylem.exe --helper-scan C: --save-index <file>`. In that mode
//! no window is created: it scans, writes the index file, and exits.
//!
//! This code runs with admin rights, so it only accepts exactly that
//! command line, only writes a NEW `xylem-*.idx` file directly inside the
//! user's temp folder (never overwrites: a planted file or link makes it
//! fail), and opens the drive read-only.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use xylem_scan_mft::scan_drive;

/// Exit codes of the helper process.
pub const OK: i32 = 0;
pub const BAD_ARGS: i32 = 2;
pub const SCAN_FAILED: i32 = 3;
pub const WRITE_FAILED: i32 = 4;

/// If the process was started in helper mode, runs the helper and returns
/// its exit code; otherwise returns None and the app starts normally.
pub fn run_if_helper(args: &[String]) -> Option<i32> {
    if args.get(1).map(String::as_str) != Some("--helper-scan") {
        return None;
    }
    let code = match (args.get(2), args.get(3).map(String::as_str), args.get(4), args.len()) {
        (Some(drive), Some("--save-index"), Some(out), 5) => run(drive, Path::new(out)),
        _ => BAD_ARGS,
    };
    Some(code)
}

fn run(drive: &str, out: &Path) -> i32 {
    let Some(letter) = drive_letter(drive) else { return BAD_ARGS };
    if check_index_path(out).is_err() {
        return BAD_ARGS;
    }
    let scan = match scan_drive(letter, xylem_scan_walk::default_threads()) {
        Ok(s) => s,
        Err(_) => return SCAN_FAILED,
    };
    let write = || -> std::io::Result<()> {
        // create_new: never overwrite (see module docs).
        let mut w = BufWriter::with_capacity(1 << 20, File::create_new(out)?);
        scan.index.save(&scan.stats, "MFT (direct read)", &mut w)?;
        w.flush()
    };
    if write().is_err() {
        let _ = std::fs::remove_file(out);
        return WRITE_FAILED;
    }
    OK
}

/// "C:" or "C:\" -> 'C'.
fn drive_letter(s: &str) -> Option<char> {
    let mut c = s.chars();
    let l = c.next().filter(char::is_ascii_alphabetic)?;
    matches!(c.as_str(), ":" | ":\\").then_some(l.to_ascii_uppercase())
}

/// Only `xylem-*.idx`, directly inside the user's temp folder.
pub fn check_index_path(path: &Path) -> Result<(), String> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !(name.starts_with("xylem-") && name.ends_with(".idx")) {
        return Err("index file must be named xylem-*.idx".into());
    }
    let parent = path.parent().map(std::fs::canonicalize).transpose().map_err(|e| e.to_string())?;
    let temp = std::fs::canonicalize(std::env::temp_dir()).map_err(|e| e.to_string())?;
    if parent.as_deref() != Some(temp.as_path()) {
        return Err("index file must be directly inside the temp folder".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn index_path_is_restricted_to_temp() {
        let temp = std::env::temp_dir();
        assert!(check_index_path(&temp.join("xylem-123-abc.idx")).is_ok());
        assert!(check_index_path(&temp.join("evil.dll")).is_err(), "wrong name");
        assert!(check_index_path(&temp.join("xylem-1.dll")).is_err(), "wrong extension");
        assert!(check_index_path(Path::new(r"C:\Windows\xylem-1.idx")).is_err(), "outside temp");
        assert!(check_index_path(&temp.join("sub").join("xylem-1.idx")).is_err(), "not directly in temp");
    }

    #[test]
    fn only_the_exact_helper_command_line_is_accepted() {
        assert_eq!(run_if_helper(&args(&["xylem.exe"])), None, "normal start");
        assert_eq!(run_if_helper(&args(&["xylem.exe", "C:\\Users"])), None, "open a folder");
        assert_eq!(run_if_helper(&args(&["x", "--helper-scan"])), Some(BAD_ARGS));
        assert_eq!(run_if_helper(&args(&["x", "--helper-scan", "C:", "--out", "f"])), Some(BAD_ARGS));
        assert_eq!(run_if_helper(&args(&["x", "--helper-scan", "C:", "--save-index", "f", "extra"])), Some(BAD_ARGS));
        // Wrong drive syntax or a path outside temp: refused before any scan.
        assert_eq!(run_if_helper(&args(&["x", "--helper-scan", "C:\\Users", "--save-index", "f"])), Some(BAD_ARGS));
        assert_eq!(
            run_if_helper(&args(&["x", "--helper-scan", "C:", "--save-index", r"C:\Windows\xylem-1.idx"])),
            Some(BAD_ARGS)
        );
        assert_eq!(drive_letter("d:\\"), Some('D'));
        assert_eq!(drive_letter("1:"), None);
    }
}
