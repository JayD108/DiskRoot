use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand, ValueEnum};
use diskroot_core::format::{bytes, count};
use diskroot_core::report::{self, Entry, SortBy};
use diskroot_scan_mft::{Mode, Used, scan_auto};
use diskroot_scan_walk::{default_threads, sys};

#[derive(Parser)]
#[command(name = "diskroot", version, about = "Fast disk space analyzer for Windows")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Scan a drive or folder and list the biggest folders and files
    Scan(ScanArgs),
    /// Size history the app keeps of a remembered drive or folder: how it
    /// grew, what grew the most, and when the drive would be full
    History(HistoryArgs),
    /// Drive health: SSD wear, data written, temperature, errors (the
    /// drives' own reports; read-only, no admin)
    Health(HealthArgs),
    /// Who's writing: top programs by bytes written over a few seconds,
    /// and each drive's own write counter (read-only, no admin)
    Writes(WritesArgs),
}

#[derive(Args)]
struct WritesArgs {
    /// How long to measure, in seconds
    #[arg(long, default_value_t = 10)]
    secs: u64,
    /// How many programs to list
    #[arg(long, default_value_t = 15)]
    top: usize,
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct HealthArgs {
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct HistoryArgs {
    /// Drive or folder the app scanned, e.g. C: or C:\Users
    path: String,
    /// Show one folder's size over time (path inside the scanned root,
    /// e.g. Users\me\Downloads)
    #[arg(long)]
    folder: Option<String>,
    /// What grew over this many days
    #[arg(long, default_value_t = 7)]
    days: u64,
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
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
    // Upgrading from the previous version: history and health notes move along.
    let _ = diskroot_session::migrate_previous();
    match cli.cmd {
        Cmd::Scan(args) => match run_scan(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Cmd::Writes(args) => match run_writes(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Cmd::Health(args) => match run_health(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        Cmd::History(args) => match run_history(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

fn run_writes(args: &WritesArgs) -> Result<(), String> {
    use diskroot_session::{health, writes};
    if args.secs == 0 || args.secs > 3600 {
        return Err("--secs must be 1 to 3600".into());
    }
    let mut disks: Vec<(u32, Vec<String>)> = Vec::new();
    for d in diskroot_session::drives() {
        if let Some(n) = health::disk_of(&d) {
            let l = d.trim_end_matches('\\').to_owned();
            match disks.iter_mut().find(|x| x.0 == n) {
                Some(x) => x.1.push(l),
                None => disks.push((n, vec![l])),
            }
        }
    }
    let mut s = writes::Sampler::new(&disks.iter().map(|d| d.0).collect::<Vec<_>>());
    if !args.json {
        eprintln!("Measuring writes for {} s…", args.secs);
    }
    std::thread::sleep(std::time::Duration::from_secs(args.secs));
    s.tick();
    let secs = s.seconds.max(0.001);
    let top = s.top(args.top);
    if args.json {
        let out = serde_json::json!({
            "seconds": s.seconds,
            "idle_seconds": s.idle_seconds,
            "drives": disks.iter().map(|(n, l)| serde_json::json!({"disk": n, "letters": l, "bytes_written": s.disk_totals.get(n).copied().unwrap_or(0)})).collect::<Vec<_>>(),
            "programs": top.iter().map(|t| serde_json::json!({"name": t.name, "bytes_written": t.active + t.idle, "bytes_per_sec": (t.active + t.idle) as f64 / secs, "while_idle": t.idle})).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out).expect("valid JSON"));
        return Ok(());
    }
    for (n, l) in &disks {
        let b = s.disk_totals.get(n).copied().unwrap_or(0);
        println!("Drive {} (disk {n}): {} written ({}/s)", l.join(" "), bytes(b), bytes((b as f64 / secs) as u64));
    }
    println!("\n{:>10}  {:>10}  Program   (writes include pipes and some network, not only disk)", "Written", "Per sec");
    for t in &top {
        let w = t.active + t.idle;
        println!("{:>10}  {:>8}/s  {}", bytes(w), bytes((w as f64 / secs) as u64), t.name);
    }
    if top.is_empty() {
        println!("  nothing written");
    }
    Ok(())
}

fn run_health(args: &HealthArgs) -> Result<(), String> {
    use diskroot_session::health;
    let drives = health::drives();
    let trim = health::trim_enabled();
    health::record_daily(&drives);
    let snaps = health::snaps();
    if args.json {
        let list: Vec<_> = drives
            .iter()
            .map(|d| {
                let (level, text) = d.verdict();
                serde_json::json!({
                    "disk": d.number, "letters": d.letters, "model": d.model, "firmware": d.firmware, "bus": d.bus,
                    "ssd": d.ssd, "trim_supported": d.trim, "verdict": level, "verdict_text": text,
                    "nvme": d.nvme.as_ref().map(|h| serde_json::json!({
                        "critical_warning": h.critical_warning, "temperature_c": h.temp_c, "spare_pct": h.spare,
                        "spare_threshold_pct": h.spare_threshold, "rated_life_used_pct": h.used_pct,
                        // As strings: JSON numbers can't hold every 128-bit counter.
                        "bytes_written": h.written_bytes.to_string(), "bytes_read": h.read_bytes.to_string(),
                        "power_on_hours": h.power_on_hours.to_string(), "power_cycles": h.power_cycles.to_string(),
                        "unsafe_shutdowns": h.unsafe_shutdowns.to_string(), "media_errors": h.media_errors.to_string(),
                        "error_log_entries": h.error_log_entries.to_string(),
                        "warning_temp_minutes": h.warn_temp_minutes, "critical_temp_minutes": h.crit_temp_minutes,
                    })),
                    "note": d.note,
                    "trend": health::trend(&snaps, d),
                })
            })
            .collect();
        let out = serde_json::json!({ "windows_trim_on": trim, "drives": list });
        println!("{}", serde_json::to_string_pretty(&out).expect("valid JSON"));
        return Ok(());
    }
    if drives.is_empty() {
        return Err("Windows didn't list any drives".into());
    }
    for d in &drives {
        let kind = match d.ssd {
            Some(true) => format!("{} SSD", d.bus),
            Some(false) => format!("{} hard disk", d.bus),
            None => d.bus.clone(),
        };
        let letters = if d.letters.is_empty() { "no letter".to_owned() } else { d.letters.join(" ") };
        println!("Disk {} · {letters} · {} · {kind} · firmware {}", d.number, d.model, d.firmware);
        println!("  {}", d.verdict().1);
        for f in d.facts() {
            println!("  {f}");
        }
        if let Some(t) = health::trend(&snaps, d) {
            match t.per_day {
                Some(b) => println!("  Writes: about {} a day (last {:.0} days)", bytes(b as u64), t.days.min(30.0)),
                None => println!("  Writes per day: shown after two days of notes"),
            }
            println!("  {}", t.life);
        }
        println!();
    }
    println!(
        "{}",
        match trim {
            Some(true) => "Windows sends TRIM to SSDs (on).",
            Some(false) => "Windows TRIM is OFF: run `fsutil behavior set DisableDeleteNotify 0` as admin to turn it on.",
            None => "Couldn't read whether Windows sends TRIM.",
        }
    );
    Ok(())
}

fn day(secs: u64) -> String {
    diskroot_core::date_text(u16::try_from(secs / 86_400).unwrap_or(u16::MAX))
}

fn run_history(args: &HistoryArgs) -> Result<(), String> {
    use diskroot_session::history::{self, method_text};
    use diskroot_session::remember::age_text;
    let mut root = args.path.trim().replace('/', "\\");
    if root.len() == 2 && root.ends_with(':') {
        root.push('\\');
    }
    let root = diskroot_session::real_path(&root).unwrap_or(root);
    let h = history::load(&root).ok_or_else(|| {
        format!("no size history for {root} yet. The DiskRoot app keeps one for every scan it remembers (or turn on View > Scheduled scans).")
    })?;
    let label = root.trim_end_matches('\\').to_owned();
    let fc = (root.len() <= 3).then(|| history::forecast(&h.points));
    let growers = h.growers(args.days * 86_400, 20);
    let series = args.folder.as_deref().map(|f| h.series(f.trim_matches('\\')));
    if args.json {
        let points: Vec<_> = h
            .points
            .iter()
            .map(|p| serde_json::json!({"time": p.time, "method": method_text(p.method), "on_disk": p.total, "files": p.files, "volume_size": p.vol_total, "volume_free": p.vol_free}))
            .collect();
        let out = serde_json::json!({
            "root": h.root,
            "points": points,
            "forecast": fc,
            "forecast_text": fc.as_ref().map(|f| f.text(&label)),
            "grew": growers.map(|g| serde_json::json!({"days": args.days, "from": g.before_time, "to": g.after_time, "items": g.items})),
            "folder": series.map(|s| s.into_iter().map(|(t, b)| serde_json::json!({"time": t, "on_disk": b})).collect::<Vec<_>>()),
        });
        println!("{}", serde_json::to_string_pretty(&out).expect("valid JSON"));
        return Ok(());
    }
    let first = h.points.first().map_or(0, |p| p.time);
    println!("Size history of {} · {} scans since {} · history files use {}", h.root, h.points.len(), day(first), bytes(history::disk_usage()));
    if let Some(f) = &fc {
        println!("  {}", f.text(&label));
    }
    println!("\nLatest scans");
    println!("  {:<12} {:<14} {:<12} {:>10}  {:>10}", "Date", "When", "Method", "On disk", "Drive free");
    for p in h.points.iter().rev().take(10) {
        let free = if p.vol_total > 0 { bytes(p.vol_free) } else { "-".into() };
        println!("  {:<12} {:<14} {:<12} {:>10}  {:>10}", day(p.time), age_text(p.time), method_text(p.method), bytes(p.total), free);
    }
    if let Some(s) = series {
        println!("\n{} over time (0 B = not there or under 1 MB)", args.folder.as_deref().unwrap_or_default());
        if s.is_empty() {
            println!("  never 1 MB or more in the history");
        }
        for (t, b) in s.iter().rev().take(20) {
            println!("  {:<12} {:<14} {:>10}", day(*t), age_text(*t), bytes(*b));
        }
    }
    match growers {
        None => println!("\nWhat grew: needs at least two scans."),
        Some(g) => {
            println!("\nWhat changed since the scan from {} (asked: last {} days)", age_text(g.before_time), args.days);
            if g.items.is_empty() {
                println!("  no folder changed by more than 1 MB (or 0.1% of the total)");
            }
            for c in &g.items {
                let d = c.delta();
                let sign = if d >= 0 { "+" } else { "-" };
                let path = if c.path.is_empty() { "(the scanned folder itself)" } else { &c.path };
                println!("  {sign}{:>9}  {:>10}  {path}", bytes(d.unsigned_abs() as u64), bytes(c.after));
            }
        }
    }
    Ok(())
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
        let mut w = BufWriter::with_capacity(1 << 20, File::create(path)?);
        report::write_csv(idx, &mut w)?;
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

