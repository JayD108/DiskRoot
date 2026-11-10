use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use drishti_core::format::{bytes, count};
use drishti_core::{Index, NO_PARENT};
use drishti_core::report::{self, Entry, SortBy};
use drishti_scan_mft::{Mode, Used, scan_auto};
use drishti_scan_walk::{default_threads, sys};

#[derive(Parser)]
#[command(name = "drishti", version, about = "Fast disk space analyzer for Windows")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scan a drive or folder and list the biggest folders and files
    Scan(ScanArgs),
}

#[derive(Args)]
struct ScanArgs {
    /// Drive or folder to scan, e.g. C: or C:\Users
    path: PathBuf,
    /// How many folders and files to list
    #[arg(long, default_value_t = 50)]
    top: usize,
    /// Worker threads (default: one per CPU core, 4-32)
    #[arg(long)]
    threads: Option<usize>,
    /// Rank by size on disk or by logical size
    #[arg(long, value_enum, default_value_t = By::Disk)]
    by: By,
    /// Print JSON instead of tables
    #[arg(long)]
    json: bool,
    /// Also write every file and folder to this CSV file
    #[arg(long)]
    export: Option<PathBuf>,
    /// Save the scanned tree to this file (used by the app's admin helper)
    #[arg(long)]
    save_index: Option<PathBuf>,
    /// auto: read the MFT for whole NTFS drives when running as admin,
    /// otherwise walk folders
    #[arg(long, value_enum, default_value_t = ModeArg::Auto)]
    mode: ModeArg,
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    Auto,
    Mft,
    Walk,
}

#[derive(Clone, Copy, ValueEnum)]
enum By {
    Disk,
    Size,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Scan(args) => match run_scan(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

fn run_scan(args: &ScanArgs) -> std::io::Result<()> {
    let mode = match args.mode {
        ModeArg::Auto => Mode::Auto,
        ModeArg::Mft => Mode::Mft,
        ModeArg::Walk => Mode::Walk,
    };
    let (s, used) = scan_auto(&args.path, mode, args.threads.unwrap_or_else(default_threads))?;
    let idx = &s.index;
    let mode_text = match &used {
        Used::Mft => "MFT (direct read)".to_owned(),
        Used::Walk { fallback_reason: None } => "folder walk".to_owned(),
        Used::Walk { fallback_reason: Some(r) } => format!("folder walk ({r})"),
    };
    let by = match args.by {
        By::Disk => SortBy::OnDisk,
        By::Size => SortBy::Size,
    };
    let folders = report::top_folders(idx, args.top, by);
    let files = report::top_files(idx, args.top, by);
    if let Some(path) = &args.export {
        export_csv(idx, path)?;
    }
    if let Some(path) = &args.save_index {
        check_index_path(path)?;
        // create_new: never overwrite. This may run elevated, so a planted
        // file or link at this path must make it fail, not overwrite.
        let mut w = BufWriter::with_capacity(1 << 20, File::create_new(path)?);
        idx.save(&s.stats, &mode_text, &mut w)?;
        w.flush()?;
    }
    let peak = sys::peak_memory_bytes().unwrap_or(0);

    if args.json {
        let out = serde_json::json!({
            "root": idx.name(0),
            "mode": mode_text,
            "elapsed_ms": s.stats.elapsed.as_secs_f64() * 1000.0,
            "files": s.stats.files,
            "folders": s.stats.dirs,
            "unreadable_folders": s.stats.unreadable,
            "size": idx.size(0),
            "on_disk": idx.alloc(0),
            "peak_memory": peak,
            "top_folders": folders,
            "top_files": files,
        });
        println!("{}", serde_json::to_string_pretty(&out).expect("valid JSON"));
        return Ok(());
    }

    println!("Scanned {} in {:.2} s · mode: {mode_text}", idx.name(0), s.stats.elapsed.as_secs_f64());
    println!(
        "  {} files · {} folders · {} unreadable folders{}",
        count(s.stats.files),
        count(s.stats.dirs),
        count(s.stats.unreadable),
        if s.stats.unreadable > 0 { " (run as administrator to include more)" } else { "" },
    );
    println!(
        "  Total {} · on disk {} · peak RAM {}",
        bytes(idx.size(0)),
        bytes(idx.alloc(0)),
        bytes(peak)
    );
    let what = match by {
        SortBy::OnDisk => "size on disk",
        SortBy::Size => "size",
    };
    print_table(&format!("Top {} folders (by {what})", folders.len()), &folders, true);
    print_table(&format!("Top {} files (by {what})", files.len()), &files, false);
    Ok(())
}

/// `--save-index` exists for the app's admin helper, which runs elevated.
/// Only allow writing `drishti-*.idx` directly inside the user's temp folder,
/// so an elevated run can't be pointed at system locations.
fn check_index_path(path: &Path) -> std::io::Result<()> {
    let refuse = |why: &str| Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, why.to_owned()));
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if !(name.starts_with("drishti-") && name.ends_with(".idx")) {
        return refuse("--save-index file must be named drishti-*.idx");
    }
    let parent = path.parent().map(std::fs::canonicalize).transpose()?;
    let temp = std::fs::canonicalize(std::env::temp_dir())?;
    if parent.as_deref() != Some(temp.as_path()) {
        return refuse("--save-index file must be directly inside the temp folder");
    }
    Ok(())
}

/// Writes one CSV row per file and folder: path, size, on disk, files, kind.
/// Folder paths are cached (parents always come before children), so each
/// row costs one string join instead of a walk up to the root.
fn export_csv(idx: &Index, path: &Path) -> std::io::Result<()> {
    let mut w = BufWriter::with_capacity(1 << 20, File::create(path)?);
    writeln!(w, "Path,Size,OnDisk,Files,Kind")?;
    let mut dir_paths: Vec<Option<Box<str>>> = vec![None; idx.len()];
    let mut buf = String::new();
    for i in 0..idx.len() as u32 {
        buf.clear();
        let parent = idx.parent(i);
        if parent == NO_PARENT {
            buf.push_str(idx.name(i));
        } else {
            let p = dir_paths[parent as usize].as_deref().expect("parent folder seen first");
            buf.push_str(p);
            if !p.ends_with('\\') {
                buf.push('\\');
            }
            buf.push_str(idx.name(i));
        }
        let is_dir = idx.is_dir(i);
        write!(w, "\"{}\",{},{},{},{}", buf.replace('"', "\"\""), idx.size(i), idx.alloc(i), idx.files(i), if is_dir { "dir" } else { "file" })?;
        w.write_all(b"\n")?;
        if is_dir {
            dir_paths[i as usize] = Some(buf.as_str().into());
        }
    }
    w.flush()
}

fn print_table(title: &str, rows: &[Entry], with_files: bool) {
    println!("\n{title}");
    if with_files {
        println!("{:>4}  {:>9}  {:>9}  {:>11}  Path", "#", "On disk", "Size", "Files");
    } else {
        println!("{:>4}  {:>9}  {:>9}  Path", "#", "On disk", "Size");
    }
    for (i, e) in rows.iter().enumerate() {
        if with_files {
            println!(
                "{:>4}  {:>9}  {:>9}  {:>11}  {}",
                i + 1,
                bytes(e.on_disk),
                bytes(e.size),
                count(u64::from(e.files)),
                e.path
            );
        } else {
            println!("{:>4}  {:>9}  {:>9}  {}", i + 1, bytes(e.on_disk), bytes(e.size), e.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_path_is_restricted_to_temp() {
        let temp = std::env::temp_dir();
        assert!(check_index_path(&temp.join("drishti-123-abc.idx")).is_ok());
        assert!(check_index_path(&temp.join("evil.dll")).is_err(), "wrong name");
        assert!(check_index_path(&temp.join("drishti-1.dll")).is_err(), "wrong extension");
        assert!(check_index_path(Path::new(r"C:\Windows\drishti-1.idx")).is_err(), "outside temp");
        assert!(check_index_path(&temp.join("sub").join("drishti-1.idx")).is_err(), "not directly in temp");
    }
}
