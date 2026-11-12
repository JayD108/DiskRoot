//! Benchmark harness.
//!
//! - `gen`:    build a repeatable synthetic folder tree
//! - `run`:    time diskroot's scanner over several runs (warm cache)
//! - `time`:   time any external command the same way (e.g. another tool's CLI)
//! - `verify`: compare diskroot's totals with a simple std::fs reference walk
//!
//! Cold-cache runs need a reboot, or emptying the standby list with
//! Sysinternals RAMMap, before each run.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use clap::{Parser, Subcommand};
use diskroot_core::flags;
use diskroot_core::format::{bytes, count};
use diskroot_core::Index;
use diskroot_scan_mft::{Mode, Used, scan_auto};
use diskroot_scan_walk::{ScanOptions, default_threads, scan, sys};

#[derive(Parser)]
#[command(name = "diskroot-bench", about = "Benchmark and accuracy harness for diskroot")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a synthetic folder tree (DIR must be empty or not exist)
    Gen {
        dir: PathBuf,
        #[arg(long, default_value_t = 100_000)]
        files: u64,
        #[arg(long, default_value_t = 25)]
        per_dir: u64,
        #[arg(long, default_value_t = 8)]
        fanout: u64,
        /// Largest file size in bytes (sizes are pseudo-random up to this)
        #[arg(long, default_value_t = 8192)]
        max_size: u64,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// Time diskroot scans of PATH
    Run {
        path: PathBuf,
        #[arg(long, default_value_t = 5)]
        runs: usize,
        #[arg(long)]
        threads: Option<usize>,
        /// auto, mft or walk
        #[arg(long, default_value = "auto", value_parser = parse_mode)]
        mode: Mode,
    },
    /// Compare the MFT scan with the folder walk on a whole drive (admin)
    Modes {
        drive: PathBuf,
        #[arg(long)]
        threads: Option<usize>,
    },
    /// Time an external command: diskroot-bench time --runs 3 -- prog args...
    Time {
        #[arg(long, default_value_t = 3)]
        runs: usize,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        cmd: Vec<String>,
    },
    /// Check diskroot's file count and total size against a std::fs walk
    Verify {
        path: PathBuf,
        #[arg(long)]
        threads: Option<usize>,
    },
}

fn main() -> ExitCode {
    let result = match Cli::parse().cmd {
        Cmd::Gen { dir, files, per_dir, fanout, max_size, seed } => {
            generate(&dir, files, per_dir.max(1), fanout.max(1), max_size, seed)
        }
        Cmd::Run { path, runs, threads, mode } => run(&path, runs.max(1), threads, mode),
        Cmd::Modes { drive, threads } => compare_modes(&drive, threads),
        Cmd::Time { runs, cmd } => time_command(&cmd, runs.max(1)),
        Cmd::Verify { path, threads } => verify(&path, threads),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Small deterministic PRNG so trees are identical across machines.
struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

fn generate(dir: &Path, files: u64, per_dir: u64, fanout: u64, max_size: u64, seed: u64) -> std::io::Result<bool> {
    if dir.exists() && fs::read_dir(dir)?.next().is_some() {
        eprintln!("refusing to write into non-empty folder {}", dir.display());
        return Ok(false);
    }
    fs::create_dir_all(dir)?;
    let start = Instant::now();
    let dirs_needed = files.div_ceil(per_dir);
    let mut rng = SplitMix(seed);
    let mut queue = VecDeque::from([dir.to_path_buf()]);
    let (mut made, mut dirs_made, mut total) = (0u64, 1u64, 0u64);

    while made < files {
        let Some(d) = queue.pop_front() else { break };
        for _ in 0..per_dir.min(files - made) {
            let size = rng.next() % (max_size + 1);
            fs::File::create(d.join(format!("file_{made:08}.bin")))?.set_len(size)?;
            made += 1;
            total += size;
        }
        for _ in 0..fanout {
            if dirs_made >= dirs_needed {
                break;
            }
            let sub = d.join(format!("dir_{dirs_made:07}"));
            fs::create_dir(&sub)?;
            queue.push_back(sub);
            dirs_made += 1;
        }
    }
    println!(
        "Created {} files ({}) in {} folders under {} in {:.1} s",
        count(made),
        bytes(total),
        count(dirs_made),
        dir.display(),
        start.elapsed().as_secs_f64()
    );
    Ok(true)
}

fn summarize(times: &mut [Duration]) -> (Duration, Duration) {
    times.sort();
    (times[0], times[times.len() / 2])
}

fn parse_mode(s: &str) -> Result<Mode, String> {
    match s {
        "auto" => Ok(Mode::Auto),
        "mft" => Ok(Mode::Mft),
        "walk" => Ok(Mode::Walk),
        _ => Err("expected auto, mft or walk".into()),
    }
}

fn used_text(u: &Used) -> String {
    match u {
        Used::Mft => "MFT".into(),
        Used::Walk { fallback_reason: None } => "folder walk".into(),
        Used::Walk { fallback_reason: Some(r) } => format!("folder walk ({r})"),
    }
}

fn run(path: &Path, runs: usize, threads: Option<usize>, mode: Mode) -> std::io::Result<bool> {
    let threads = threads.unwrap_or_else(default_threads);
    println!("Scanning {} x{runs} with {threads} threads", path.display());
    let mut times = Vec::with_capacity(runs);
    let mut last = None;
    for r in 1..=runs {
        let (s, used) = scan_auto(path, mode, threads)?;
        println!(
            "  run {r} [{}]: {:.3} s  ({} files, index {})",
            used_text(&used),
            s.stats.elapsed.as_secs_f64(),
            count(s.stats.files),
            bytes(s.index.heap_bytes() as u64)
        );
        times.push(s.stats.elapsed);
        last = Some(s.stats);
    }
    let stats = last.expect("at least one run");
    let (min, median) = summarize(&mut times);
    let rate = (stats.files + stats.dirs) as f64 / median.as_secs_f64();
    println!(
        "min {:.3} s · median {:.3} s · {} entries/s · peak RAM {}",
        min.as_secs_f64(),
        median.as_secs_f64(),
        count(rate as u64),
        bytes(sys::peak_memory_bytes().unwrap_or(0))
    );
    Ok(true)
}

fn time_command(cmd: &[String], runs: usize) -> std::io::Result<bool> {
    println!("Timing `{}` x{runs}", cmd.join(" "));
    let mut times = Vec::with_capacity(runs);
    let mut all_ok = true;
    for r in 1..=runs {
        let start = Instant::now();
        let status = Command::new(&cmd[0]).args(&cmd[1..]).status()?;
        let t = start.elapsed();
        println!("  run {r}: {:.3} s  (exit {status})", t.as_secs_f64());
        all_ok &= status.success();
        times.push(t);
    }
    let (min, median) = summarize(&mut times);
    println!("min {:.3} s · median {:.3} s", min.as_secs_f64(), median.as_secs_f64());
    Ok(all_ok)
}

#[derive(Default)]
struct RefTotals {
    files: u64,
    size: u64,
    unreadable: u64,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetFileInformationByHandle(h: *mut std::ffi::c_void, info: *mut [u32; 13]) -> i32;
}

/// (number of names, (volume serial, file index)) of a file, via
/// GetFileInformationByHandle on a handle opened for attributes only.
fn link_id(p: &Path) -> Option<(u32, (u32, u64))> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    let f = fs::OpenOptions::new().access_mode(0x80).share_mode(7).open(p).ok()?;
    let mut info = [0u32; 13];
    // SAFETY: valid handle; BY_HANDLE_FILE_INFORMATION is 13 DWORDs.
    (unsafe { GetFileInformationByHandle(f.as_raw_handle(), &mut info) } != 0)
        .then(|| (info[10], (info[7], (u64::from(info[11]) << 32) | u64::from(info[12]))))
}

/// Plain single-threaded walk with std::fs. Like diskroot, it does not follow
/// symlinks or junctions (std treats both as symlinks on Windows).
fn reference_walk(root: &Path) -> RefTotals {
    let mut t = RefTotals::default();
    let mut seen: std::collections::HashSet<(u32, u64)> = std::collections::HashSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else {
            t.unreadable += 1;
            continue;
        };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                t.files += 1;
                // A file with several names (hardlinks) counts its bytes once,
                // like DiskRoot's walk (asked file by file here, independently).
                let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
                match link_id(&entry.path()) {
                    Some((links, id)) if links > 1 && !seen.insert(id) => {}
                    _ => t.size += len,
                }
            }
        }
    }
    t
}

fn verify(path: &Path, threads: Option<usize>) -> std::io::Result<bool> {
    let opts = ScanOptions { threads: threads.unwrap_or_else(default_threads) };
    let s = scan(path, &opts)?;
    let idx = &s.index;
    let (mut files, mut size) = (0u64, 0u64);
    for i in 0..idx.len() as u32 {
        if idx.flags(i) & (flags::DIR | flags::LINK) == 0 {
            files += 1;
            size += idx.size(i);
        }
    }
    let r = reference_walk(path);

    println!("             {:>15}  {:>20}  {:>10}", "files", "bytes", "unreadable");
    println!("diskroot      {:>15}  {:>20}  {:>10}", count(files), count(size), count(s.stats.unreadable));
    println!("std::fs      {:>15}  {:>20}  {:>10}", count(r.files), count(r.size), count(r.unreadable));
    let ok = files == r.files && size == r.size;
    if ok {
        println!("MATCH");
    } else {
        println!(
            "MISMATCH: files {:+}, bytes {:+} (small differences are normal on a live drive)",
            files as i64 - r.files as i64,
            size as i128 - r.size as i128
        );
    }
    Ok(ok)
}

/// Size, size on disk and file count of each top-level entry, by lowercase name.
fn top_level(idx: &Index) -> std::collections::HashMap<String, (u64, u64, u32)> {
    (1..idx.len() as u32)
        .filter(|&i| idx.parent(i) == 0)
        .map(|i| (idx.name(i).to_lowercase(), (idx.size(i), idx.alloc(i), idx.files(i))))
        .collect()
}

fn compare_modes(drive: &Path, threads: Option<usize>) -> std::io::Result<bool> {
    let threads = threads.unwrap_or_else(default_threads);
    let (m, _) = scan_auto(drive, Mode::Mft, threads)?;
    let (w, _) = scan_auto(drive, Mode::Walk, threads)?;
    let gb = |b: u64| b as f64 / (1u64 << 30) as f64;

    println!("{:<12} {:>9} {:>12} {:>10} {:>11} {:>11}", "", "time", "files", "folders", "size", "on disk");
    for (label, s, extra) in [
        ("MFT", &m, format!("orphans {}", count(m.stats.orphans))),
        ("folder walk", &w, format!("unreadable {}", count(w.stats.unreadable))),
    ] {
        println!(
            "{label:<12} {:>8.2}s {:>12} {:>10} {:>10.2}G {:>10.2}G  {extra}",
            s.stats.elapsed.as_secs_f64(),
            count(s.stats.files),
            count(s.stats.dirs),
            gb(s.index.size(0)),
            gb(s.index.alloc(0)),
        );
    }

    let (mt, wt) = (top_level(&m.index), top_level(&w.index));
    let mut names: Vec<&String> = mt.keys().chain(wt.keys()).collect();
    names.sort();
    names.dedup();
    let mut rows: Vec<_> = names
        .into_iter()
        .map(|n| {
            let a = mt.get(n).copied().unwrap_or_default();
            let b = wt.get(n).copied().unwrap_or_default();
            (a.0.abs_diff(b.0), n, a, b)
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.0));
    let same = rows.iter().filter(|r| r.2 == r.3).count();
    println!("\nTop-level entries identical (size, on disk, files): {same}/{}", rows.len());
    println!("Largest differences:\n{:>10} {:>10} {:>9} {:>9}  name", "MFT", "walk", "files", "files");
    for (diff, n, a, b) in rows.iter().take(12) {
        if *diff == 0 && a.2 == b.2 {
            break;
        }
        println!("{:>9.2}G {:>9.2}G {:>9} {:>9}  {n}", gb(a.0), gb(b.0), a.2, b.2);
    }
    Ok(true)
}
