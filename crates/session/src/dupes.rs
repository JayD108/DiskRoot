//! Duplicate finder (opt-in: it is the only feature that reads file
//! contents).
//!
//! 1. Candidates: files of the same size (≥ a minimum), from the scan, not
//!    cloud placeholders. No file is opened for this step.
//! 2. Hardlinks: names of the same file (same volume + file ID) are one
//!    file, not duplicates; deleting one would free nothing.
//! 3. Quick check: BLAKE3 of the first and last 64 KB.
//! 4. Full check: BLAKE3 of the whole file, only for files still matching.
//!
//! Files that are online-only (OneDrive and similar), offline, locked or
//! unreadable are skipped, never downloaded or waited for.
//!
//! Faster repeats: on an SSD several files are read at once, and the
//! fingerprints of the last search are kept on this PC. A file whose size
//! and last-write time haven't changed isn't read again.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

use crate::safety;

/// One copy: (node, full path).
type Copy = (u32, String);

/// A set of files with identical contents.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DupGroup {
    pub size: u64,
    /// (node, full path) of each copy. Node ids are only valid for the scan
    /// the search ran on; a saved search is matched again by path.
    pub files: Vec<(u32, String)>,
}

impl DupGroup {
    /// Space that deleting all but one copy would free.
    pub fn wasted(&self) -> u64 {
        self.size.saturating_mul(self.files.len().saturating_sub(1) as u64)
    }
}

/// Progress shared with the UI.
#[derive(Default)]
pub struct Progress {
    /// Bytes read so far, and the rough total to read.
    pub read: AtomicU64,
    pub total: AtomicU64,
    /// 0 checking files, 1 quick check (first and last 64 KB), 2 full compare, 3 done.
    pub phase: AtomicU8,
    /// Files handled in the current step, and how many it has.
    pub done: AtomicU64,
    pub of: AtomicU64,
    /// Files whose fingerprint came from the last search (not read again).
    pub reused: AtomicU64,
    /// Files read at the same time.
    pub threads: AtomicU64,
}

const EDGE: u64 = 64 * 1024;
/// RECALL_ON_DATA_ACCESS | RECALL_ON_OPEN | OFFLINE: reading would download
/// or wait for the file.
const DONT_READ: u32 = 0x0040_0000 | 0x0004_0000 | 0x1000;

/// (file identity, last-write time) if the file can be read without
/// downloading or waiting for it.
fn probe(path: &Path) -> Option<(safety::FileId, u64)> {
    let m = std::fs::symlink_metadata(path).ok()?;
    if !m.is_file() || m.file_attributes() & DONT_READ != 0 {
        return None;
    }
    Some((safety::file_id(path).ok()?, m.last_write_time()))
}

fn quick_hash(path: &Path, size: u64, p: &Progress) -> Option<[u8; 32]> {
    let mut f = File::open(path).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; EDGE as usize];
    let n = size.min(EDGE) as usize;
    f.read_exact(&mut buf[..n]).ok()?;
    h.update(&buf[..n]);
    if size > EDGE {
        f.seek(SeekFrom::Start(size.saturating_sub(EDGE).max(EDGE))).ok()?;
        let m = (size - size.saturating_sub(EDGE).max(EDGE)) as usize;
        f.read_exact(&mut buf[..m]).ok()?;
        h.update(&buf[..m]);
    }
    p.read.fetch_add(size.min(2 * EDGE), Ordering::Relaxed);
    Some(*h.finalize().as_bytes())
}

fn full_hash(path: &Path, cancel: &AtomicBool, p: &Progress) -> Option<[u8; 32]> {
    let mut f = File::open(path).ok()?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        p.read.fetch_add(n as u64, Ordering::Relaxed);
    }
    Some(*h.finalize().as_bytes())
}

/// Fingerprints from an earlier search, by lowercased path: valid while
/// the file's size and last-write time are the same.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HashCache {
    pub files: HashMap<String, Cached>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Cached {
    pub size: u64,
    pub mtime: u64,
    pub quick: [u8; 32],
    pub full: Option<[u8; 32]>,
}

/// Runs `f` on every item with up to `threads` workers; results in order
/// (`None` where a cancel stopped the work first).
fn par_map<T: Sync, R: Send>(items: &[T], threads: usize, cancel: &AtomicBool, f: impl Fn(&T) -> R + Sync) -> Vec<Option<R>> {
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<Option<R>>> = Mutex::new((0..items.len()).map(|_| None).collect());
    std::thread::scope(|sc| {
        for _ in 0..threads.clamp(1, items.len().max(1)) {
            sc.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= items.len() || cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let r = f(&items[i]);
                    if let Ok(mut o) = out.lock() {
                        o[i] = Some(r);
                    }
                }
            });
        }
    });
    out.into_inner().unwrap_or_default()
}

/// How many files to read at once: several on an SSD, one on a hard disk
/// (it would only seek back and forth) or when unsure.
pub fn threads_for(root: &str) -> usize {
    match crate::health::is_ssd(root) {
        Some(true) => std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8),
        _ => 1,
    }
}

/// Same-size candidate groups -> groups of identical files, biggest waste
/// first. Blocking; run it on a worker thread.
pub fn find(candidates: Vec<(u64, Vec<(u32, String)>)>, cancel: &AtomicBool, p: &Progress) -> Vec<DupGroup> {
    find_with(candidates, cancel, p, 1, &HashCache::default()).0
}

/// [`find`] with `threads` readers and fingerprints reused from `cache`.
/// Also returns the fingerprints to keep for next time.
pub fn find_with(candidates: Vec<(u64, Vec<(u32, String)>)>, cancel: &AtomicBool, p: &Progress, threads: usize, cache: &HashCache) -> (Vec<DupGroup>, HashCache) {
    p.threads.store(threads as u64, Ordering::Relaxed);
    // 1. Readable files, one entry per real file (hardlinks collapse).
    let flat: Vec<(u64, u32, String)> = candidates.into_iter().flat_map(|(s, f)| f.into_iter().map(move |(n, path)| (s, n, path))).collect();
    p.phase.store(0, Ordering::Relaxed);
    p.of.store(flat.len() as u64, Ordering::Relaxed);
    let probed = par_map(&flat, threads, cancel, |(_, _, path)| {
        let r = probe(Path::new(path));
        p.done.fetch_add(1, Ordering::Relaxed);
        r
    });
    let mut by_size: HashMap<u64, Vec<(u32, String, u64)>> = HashMap::new();
    let mut seen = std::collections::HashSet::new();
    for ((size, node, path), pr) in flat.into_iter().zip(probed) {
        if let Some(Some((id, mtime))) = pr
            && seen.insert(id)
        {
            by_size.entry(size).or_default().push((node, path, mtime));
        }
    }
    by_size.retain(|_, v| v.len() > 1);
    let hit = |path: &str, size: u64, mtime: u64| cache.files.get(&path.to_lowercase()).filter(|c| c.size == size && c.mtime == mtime);
    // 2. Quick check: first and last 64 KB (or the remembered fingerprint).
    let quick_items: Vec<(u64, u32, String, u64)> = by_size.iter().flat_map(|(s, v)| v.iter().map(|(n, path, m)| (*s, *n, path.clone(), *m))).collect();
    let total: u64 = quick_items.iter().filter(|(s, _, path, m)| hit(path, *s, *m).is_none()).map(|(s, ..)| (*s).min(2 * EDGE)).sum();
    p.total.store(total, Ordering::Relaxed);
    p.done.store(0, Ordering::Relaxed);
    p.of.store(quick_items.len() as u64, Ordering::Relaxed);
    p.phase.store(1, Ordering::Relaxed);
    let quick = par_map(&quick_items, threads, cancel, |(size, _, path, mtime)| {
        let r = match hit(path, *size, *mtime) {
            Some(c) => {
                p.reused.fetch_add(1, Ordering::Relaxed);
                Some((c.quick, c.full))
            }
            None => quick_hash(Path::new(path), *size, p).map(|q| (q, None)),
        };
        p.done.fetch_add(1, Ordering::Relaxed);
        r
    });
    type Quick = (u32, String, u64, Option<[u8; 32]>);
    let mut by_quick: HashMap<(u64, [u8; 32]), Vec<Quick>> = HashMap::new();
    for ((size, node, path, mtime), q) in quick_items.into_iter().zip(quick) {
        if let Some(Some((qh, full))) = q {
            by_quick.entry((size, qh)).or_default().push((node, path, mtime, full));
        }
    }
    // Keep every fingerprint, also for files that turned out unique.
    let mut new_cache = HashCache::default();
    for ((size, qh), v) in &by_quick {
        for (_, path, mtime, full) in v {
            new_cache.files.insert(path.to_lowercase(), Cached { size: *size, mtime: *mtime, quick: *qh, full: *full });
        }
    }
    by_quick.retain(|_, v| v.len() > 1);
    let mut out = Vec::new();
    // 3. Full compare, only where the quick check matched (small files are
    // already covered whole by the quick check).
    let mut full_items: Vec<(u64, u32, String, Option<[u8; 32]>)> = Vec::new();
    for ((size, _), v) in &by_quick {
        if *size <= 2 * EDGE {
            out.push(DupGroup { size: *size, files: v.iter().map(|f| (f.0, f.1.clone())).collect() });
        } else {
            full_items.extend(v.iter().map(|f| (*size, f.0, f.1.clone(), f.3)));
        }
    }
    let to_read: u64 = full_items.iter().filter(|f| f.3.is_none()).map(|f| f.0).sum();
    p.total.fetch_add(to_read, Ordering::Relaxed);
    p.done.store(0, Ordering::Relaxed);
    p.of.store(full_items.len() as u64, Ordering::Relaxed);
    p.phase.store(2, Ordering::Relaxed);
    let fulls = par_map(&full_items, threads, cancel, |(_, _, path, known)| {
        let r = known.or_else(|| full_hash(Path::new(path), cancel, p));
        p.done.fetch_add(1, Ordering::Relaxed);
        r
    });
    let mut by_full: HashMap<(u64, [u8; 32]), Vec<Copy>> = HashMap::new();
    for ((size, node, path, _), fh) in full_items.into_iter().zip(fulls) {
        if let Some(Some(fh)) = fh {
            if let Some(c) = new_cache.files.get_mut(&path.to_lowercase()) {
                c.full = Some(fh);
            }
            by_full.entry((size, fh)).or_default().push((node, path));
        }
    }
    out.extend(by_full.into_iter().filter(|(_, v)| v.len() > 1).map(|((size, _), files)| DupGroup { size, files }));
    // The copy to keep comes first: the "original-looking" one.
    for g in &mut out {
        g.files.sort_by_key(|(_, p)| (copy_score(p), p.len(), p.to_lowercase()));
    }
    out.sort_by_key(|g| (std::cmp::Reverse(g.wasted()), g.files.first().map(|f| f.1.to_lowercase())));
    p.phase.store(3, Ordering::Relaxed);
    (out, new_cache)
}

/// One finished search, as saved on this PC.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    /// Unix seconds when it finished.
    pub time: u64,
    /// What was searched (the scan root, or a folder in it).
    pub root: String,
    pub min_size: u64,
    pub secs: f64,
    /// Bytes read, and files whose fingerprint was reused.
    pub read: u64,
    pub reused: u64,
    /// Files of the same size that were compared.
    pub candidates: u64,
    pub groups: Vec<DupGroup>,
}

impl Run {
    pub fn wasted(&self) -> u64 {
        self.groups.iter().map(DupGroup::wasted).sum()
    }
}

/// A saved search without its groups, for the list of past searches.
#[derive(Clone, Debug)]
pub struct RunInfo {
    pub file: PathBuf,
    pub time: u64,
    pub root: String,
    pub sets: usize,
    pub wasted: u64,
    pub secs: f64,
    pub min_size: u64,
}

/// `%LOCALAPPDATA%\DiskRoot\duplicates`.
pub fn dir() -> Option<PathBuf> {
    if cfg!(test) {
        return Some(std::env::temp_dir().join(format!("diskroot-test-dupes-{}", std::process::id())));
    }
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("DiskRoot").join("duplicates"))
}

/// Saved searches kept per searched place.
const KEEP_RUNS: usize = 10;

/// Saves a finished search and the fingerprints for next time; keeps the
/// newest [`KEEP_RUNS`] searches of the same place.
pub fn save(run: &Run, cache: &HashCache) -> std::io::Result<()> {
    let d = dir().ok_or_else(|| std::io::Error::other("no LOCALAPPDATA"))?;
    std::fs::create_dir_all(&d)?;
    let k = crate::remember::key(&run.root);
    std::fs::write(d.join(format!("{k}-{:012}.run.json", run.time)), serde_json::to_vec(run).map_err(std::io::Error::other)?)?;
    std::fs::write(d.join(format!("{k}.hashes.json")), serde_json::to_vec(cache).map_err(std::io::Error::other)?)?;
    let prefix = format!("{k}-");
    let mut mine: Vec<PathBuf> = std::fs::read_dir(&d)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&prefix) && n.ends_with(".run.json")))
        .collect();
    mine.sort();
    while mine.len() > KEEP_RUNS {
        let _ = std::fs::remove_file(mine.remove(0));
    }
    Ok(())
}

/// The fingerprints saved by the last search of `root` (empty if none).
pub fn load_cache(root: &str) -> HashCache {
    dir()
        .and_then(|d| std::fs::read(d.join(format!("{}.hashes.json", crate::remember::key(root)))).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn load_run(file: &Path) -> Option<Run> {
    serde_json::from_slice(&std::fs::read(file).ok()?).ok()
}

/// Every saved search, newest first.
pub fn past_runs() -> Vec<RunInfo> {
    let Some(rd) = dir().and_then(|d| std::fs::read_dir(d).ok()) else { return Vec::new() };
    let mut v: Vec<RunInfo> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.to_string_lossy().ends_with(".run.json"))
        .filter_map(|file| {
            let r = load_run(&file)?;
            Some(RunInfo { time: r.time, sets: r.groups.len(), wasted: r.wasted(), secs: r.secs, min_size: r.min_size, root: r.root, file })
        })
        .collect();
    v.sort_by_key(|r| std::cmp::Reverse(r.time));
    v
}

/// Deletes one saved search (not the files it found).
pub fn forget(file: &Path) {
    let _ = std::fs::remove_file(file);
}

/// Higher = more likely a copy: "copy" or " (1)" in the name, or inside a
/// backup / old / temp / Downloads / Recycle Bin folder.
pub fn copy_score(path: &str) -> u32 {
    let low = path.to_lowercase();
    let name = low.rsplit('\\').next().unwrap_or("");
    let mut s = 0;
    if name.contains("copy") || name.contains(" (") {
        s += 2;
    }
    for dir in [r"\backup", r"\old", r"\temp\", r"\tmp\", r"\downloads\", "$recycle.bin"] {
        if low.contains(dir) {
            s += 1;
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_real_duplicates_only() {
        let dir = std::env::temp_dir().join(format!("diskroot-dupes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let mut other = big.clone();
        other[150_000] ^= 1; // same size, same edges, different middle
        std::fs::write(dir.join("a.bin"), &big).unwrap();
        std::fs::write(dir.join("b.bin"), &big).unwrap();
        std::fs::write(dir.join("c.bin"), &other).unwrap();
        std::fs::hard_link(dir.join("a.bin"), dir.join("a-link.bin")).unwrap();
        let p = |n: &str| dir.join(n).display().to_string();
        let cands = vec![(300_000, vec![(1, p("a.bin")), (2, p("b.bin")), (3, p("c.bin")), (4, p("a-link.bin"))])];
        let groups = find(cands, &AtomicBool::new(false), &Progress::default());
        assert_eq!(groups.len(), 1, "{groups:?}");
        let mut names: Vec<&str> = groups[0].files.iter().map(|f| f.1.rsplit('\\').next().unwrap()).collect();
        names.sort_unstable();
        // a.bin and its hardlink are one file; c.bin differs in the middle.
        assert!(names == ["a.bin", "b.bin"] || names == ["a-link.bin", "b.bin"], "{names:?}");
        assert_eq!(groups[0].wasted(), 300_000);
        assert!(copy_score(r"C:\x\backup\trip (1).mp4") > copy_score(r"C:\x\photos\trip.mp4"));

        // Several readers, then a second search that reuses every fingerprint.
        let cands = || vec![(300_000, vec![(1, p("a.bin")), (2, p("b.bin")), (3, p("c.bin"))])];
        let p1 = Progress::default();
        let (g1, cache) = find_with(cands(), &AtomicBool::new(false), &p1, 4, &HashCache::default());
        assert_eq!(g1.len(), 1);
        assert_eq!(p1.reused.load(Ordering::Relaxed), 0);
        let p2 = Progress::default();
        let (g2, _) = find_with(cands(), &AtomicBool::new(false), &p2, 4, &cache);
        assert_eq!(g2.len(), 1);
        assert_eq!(p2.reused.load(Ordering::Relaxed), 3, "nothing read again");
        assert_eq!(p2.read.load(Ordering::Relaxed), 0);
        // A changed file is read again: b now matches c instead of a.
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(dir.join("b.bin"), &other).unwrap();
        let (g3, _) = find_with(cands(), &AtomicBool::new(false), &Progress::default(), 2, &cache);
        assert_eq!(g3.len(), 1, "{g3:?}");
        assert!(g3[0].files.iter().any(|f| f.1.ends_with("c.bin")) && g3[0].files.iter().any(|f| f.1.ends_with("b.bin")));

        // Saved searches.
        let run = Run { time: 5, root: dir.display().to_string(), min_size: 1, secs: 0.1, read: 0, reused: 0, candidates: 3, groups: g3 };
        save(&run, &cache).unwrap();
        assert!(load_cache(&run.root).files.len() >= 3);
        assert!(past_runs().iter().any(|r| r.time == 5 && r.sets == 1));
        let _ = std::fs::remove_dir_all(super::dir().unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
