//! Remembered scans: instant reopen and "what changed since the last scan".
//!
//! Per scanned root (drive or folder), in `%LOCALAPPDATA%\Xylem\cache\`:
//! - `<key>.idx`: the full index of the latest scan (instant reopen). Same
//!   format as the admin helper's file, loaded with the same length-checked,
//!   fuzz-tested loader.
//! - `<key>.snap`: folder sizes of the latest scan, and how it was made
//!   (Fast scan or folder walk: they count some things differently).
//! - `<key>.prev.snap`: folder sizes of the scan before it.
//! - `<key>.hist`: size history (folders of 1 MB or more), see `history`.
//!
//! These files hold file and folder **names and sizes**, nothing else, and
//! never leave the PC. The user can turn this off or forget everything.
//! Saving runs on a background thread after the scan is shown, so scans
//! are never slower.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use xylem_core::{Index, NO_PARENT, ScanStats, flags};

const SNAP_MAGIC: &[u8; 8] = b"XYLMSNP2";
/// Snapshots saved by Xylem 0.2.0 and older (no scan method stored).
const SNAP_MAGIC_V1: &[u8; 8] = b"XYLMSNP1";
/// Keep remembered scans for at most this many roots (oldest dropped).
const MAX_ROOTS: usize = 8;
/// Sanity limit for snapshot files (C: has ~300k folders).
const MAX_SNAP_DIRS: u32 = 20_000_000;

pub fn cache_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Xylem").join("cache"))
}

/// Stable file-name key for a scan root (FNV-1a of the lowercased path).
pub fn key(root: &str) -> String {
    let norm = root.trim().replace('/', "\\").to_lowercase();
    let norm = if norm.len() > 3 { norm.trim_end_matches('\\').to_owned() } else { norm };
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in norm.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// "just now", "5 min ago", "3 h ago", "2 days ago".
pub fn age_text(then_secs: u64) -> String {
    span_text(now_secs().saturating_sub(then_secs))
}

/// How far apart two scans were: "under a minute", "3 min", "2 h", "4 days".
pub fn gap_text(secs: u64) -> String {
    match secs {
        0..=59 => "under a minute".into(),
        60..=3599 => format!("{} min", secs / 60),
        3600..=86_399 => format!("{} h", secs / 3600),
        86_400..=172_799 => "1 day".into(),
        _ => format!("{} days", secs / 86_400),
    }
}

fn span_text(s: u64) -> String {
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        86_400..=172_799 => "1 day ago".into(),
        _ => format!("{} days ago", s / 86_400),
    }
}

// ---------------------------------------------------------------------------
// Snapshots: folder sizes only
// ---------------------------------------------------------------------------

/// Folder sizes of one scan. Paths are relative to the root ("" = root,
/// "Users\\me").
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub time: u64,
    pub root: String,
    /// How the scan was made, e.g. "MFT (direct read)" or "folder walk".
    /// Empty for snapshots saved by Xylem 0.2.0 and older.
    pub mode: String,
    pub dirs: Vec<SnapDir>,
}

#[derive(Clone, Debug)]
pub struct SnapDir {
    pub path: String,
    pub bytes: u64,
    pub files: u32,
}

impl Snapshot {
    /// All live folders of `idx`, with rolled-up sizes.
    pub fn of(idx: &Index) -> Self {
        let n = idx.len();
        // slot[node] = position of that folder in `dirs` (u32::MAX for files).
        // 4 bytes per item instead of an Option<String> (24 bytes) per item.
        let mut slot = vec![u32::MAX; n];
        slot[0] = 0;
        let mut dirs = vec![SnapDir { path: String::new(), bytes: idx.alloc(0), files: idx.files(0) }];
        for id in 1..n as u32 {
            if !idx.is_dir(id) || idx.flags(id) & flags::DELETED != 0 {
                continue;
            }
            let p = idx.parent(id);
            if p == NO_PARENT || slot[p as usize] == u32::MAX {
                continue;
            }
            let parent_path = &dirs[slot[p as usize] as usize].path;
            let path = if parent_path.is_empty() {
                idx.name(id).to_owned()
            } else {
                let mut s = String::with_capacity(parent_path.len() + 1 + idx.name(id).len());
                s.push_str(parent_path);
                s.push('\\');
                s.push_str(idx.name(id));
                s
            };
            slot[id as usize] = dirs.len() as u32;
            dirs.push(SnapDir { path, bytes: idx.alloc(id), files: idx.files(id) });
        }
        dirs.shrink_to_fit();
        Self { time: now_secs(), root: idx.name(0).to_owned(), mode: String::new(), dirs }
    }

    pub fn save(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(SNAP_MAGIC)?;
        w.write_all(&self.time.to_le_bytes())?;
        write_str(w, &self.root)?;
        write_str(w, &self.mode)?;
        w.write_all(&(self.dirs.len() as u32).to_le_bytes())?;
        for d in &self.dirs {
            w.write_all(&d.bytes.to_le_bytes())?;
            w.write_all(&d.files.to_le_bytes())?;
            write_str(w, &d.path)?;
        }
        Ok(())
    }

    /// Reads a snapshot, rejecting anything malformed (bad magic, absurd
    /// counts, truncated data, invalid UTF-8, trailing bytes).
    pub fn load(r: &mut impl Read) -> io::Result<Self> {
        let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, format!("snapshot: {m}"));
        let mut magic = [0u8; 8];
        r.read_exact(&mut magic)?;
        let v2 = &magic == SNAP_MAGIC;
        if !v2 && &magic != SNAP_MAGIC_V1 {
            return Err(bad("not a Xylem snapshot"));
        }
        let time = read_u64(r)?;
        let root = read_str(r)?;
        let mode = if v2 { read_str(r)? } else { String::new() };
        let n = read_u32(r)?;
        if n > MAX_SNAP_DIRS {
            return Err(bad("too many folders"));
        }
        let mut dirs = Vec::with_capacity(n.min(1 << 20) as usize);
        for _ in 0..n {
            let bytes = read_u64(r)?;
            let files = read_u32(r)?;
            let path = read_str(r)?;
            dirs.push(SnapDir { path, bytes, files });
        }
        let mut extra = [0u8; 1];
        if r.read(&mut extra)? != 0 {
            return Err(bad("trailing data"));
        }
        Ok(Self { time, root, mode, dirs })
    }
}

fn write_str(w: &mut impl Write, s: &str) -> io::Result<()> {
    let len = u16::try_from(s.len()).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path too long"))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(s.as_bytes())
}
fn read_u64(r: &mut impl Read) -> io::Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}
fn read_u32(r: &mut impl Read) -> io::Result<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b)?;
    Ok(u32::from_le_bytes(b))
}
fn read_str(r: &mut impl Read) -> io::Result<String> {
    let mut b = [0u8; 2];
    r.read_exact(&mut b)?;
    let mut s = vec![0u8; u16::from_le_bytes(b) as usize];
    r.read_exact(&mut s)?;
    String::from_utf8(s).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "snapshot: bad UTF-8"))
}

// ---------------------------------------------------------------------------
// What changed
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize)]
pub struct Change {
    /// Relative to the scan root ("" = the root itself).
    pub path: String,
    pub before: u64,
    pub after: u64,
}

impl Change {
    pub fn delta(&self) -> i128 {
        i128::from(self.after) - i128::from(self.before)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Changes {
    pub root: String,
    pub before_time: u64,
    pub after_time: u64,
    pub before_total: u64,
    pub after_total: u64,
    /// The folders where the change actually happened, biggest first.
    pub items: Vec<Change>,
    /// How each scan was made (empty = unknown, from Xylem 0.2.0 or older).
    pub before_mode: String,
    pub after_mode: String,
}

/// "mft" or "walk" for a scan-mode text, `None` if unknown.
fn method(mode: &str) -> Option<&'static str> {
    if mode.contains("MFT") {
        Some("mft")
    } else if mode.starts_with("folder walk") {
        Some("walk")
    } else {
        None
    }
}

impl Changes {
    /// True when one scan was a Fast scan and the other a folder walk. They
    /// count some things differently (hardlinks; folders only admins can
    /// read; System Volume Information), so part of the difference comes
    /// from the method, not from files that changed.
    pub fn mixed_methods(&self) -> bool {
        matches!((method(&self.before_mode), method(&self.after_mode)), (Some(a), Some(b)) if a != b)
    }

    /// "Fast scan" / "folder walk" for the user, if known.
    pub fn method_name(mode: &str) -> &'static str {
        match method(mode) {
            Some("mft") => "Fast scan",
            Some(_) => "folder walk",
            None => "scan",
        }
    }
}

/// FNV-1a over the lowercased path: a 64-bit fingerprint, so comparing
/// two snapshots doesn't need lowercase copies of every path.
pub fn path_key(p: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for c in p.chars().flat_map(char::to_lowercase) {
        let mut buf = [0u8; 4];
        for &b in c.encode_utf8(&mut buf).as_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

/// Compares two snapshots of the same root and picks the folders where the
/// change happened: big enough to matter, and not just the sum of one
/// child's change (so you see `...\node_modules`, not every parent above it).
pub fn diff(prev: &Snapshot, cur: &Snapshot, limit: usize) -> Changes {
    let before: HashMap<u64, u64> = prev.dirs.iter().map(|d| (path_key(&d.path), d.bytes)).collect();
    let after: HashMap<u64, u64> = cur.dirs.iter().map(|d| (path_key(&d.path), d.bytes)).collect();
    // (path, before, after), borrowing the snapshots' strings.
    let mut all: Vec<(&str, u64, u64)> = cur
        .dirs
        .iter()
        .map(|d| (d.path.as_str(), before.get(&path_key(&d.path)).copied().unwrap_or(0), d.bytes))
        .collect();
    all.extend(prev.dirs.iter().filter(|d| !after.contains_key(&path_key(&d.path))).map(|d| (d.path.as_str(), d.bytes, 0)));
    let delta = |b: u64, a: u64| i128::from(a) - i128::from(b);
    all.retain(|&(_, b, a)| a != b);

    let root_key = path_key("");
    let total = before.get(&root_key).copied().unwrap_or(0).max(after.get(&root_key).copied().unwrap_or(0));
    let threshold = i128::from((total / 1000).max(1 << 20)); // 0.1% of the drive, at least 1 MiB

    // Biggest same-direction change among each folder's direct children.
    let parent_key = |p: &str| -> Option<u64> {
        if p.is_empty() { None } else { Some(path_key(p.rfind('\\').map_or("", |i| &p[..i]))) }
    };
    let mut grow: HashMap<u64, i128> = HashMap::new();
    let mut shrink: HashMap<u64, i128> = HashMap::new();
    for &(path, b, a) in &all {
        if let Some(k) = parent_key(path) {
            let d = delta(b, a);
            let slot = if d > 0 { grow.entry(k).or_insert(0) } else { shrink.entry(k).or_insert(0) };
            *slot = (*slot).max(d.abs());
        }
    }
    let mut items: Vec<Change> = all
        .into_iter()
        .filter(|&(path, b, a)| {
            let d = delta(b, a);
            if d.abs() < threshold {
                return false;
            }
            let k = path_key(path);
            let child = if d > 0 { grow.get(&k) } else { shrink.get(&k) }.copied().unwrap_or(0);
            child * 4 < d.abs() * 3 // keep unless one child explains >= 75%
        })
        .map(|(path, before, after)| Change { path: path.to_owned(), before, after })
        .collect();
    items.sort_by_key(|c| std::cmp::Reverse(c.delta().abs()));
    items.truncate(limit);
    Changes {
        root: cur.root.clone(),
        before_time: prev.time,
        after_time: cur.time,
        before_total: before.get(&root_key).copied().unwrap_or(0),
        after_total: after.get(&root_key).copied().unwrap_or(0),
        items,
        before_mode: prev.mode.clone(),
        after_mode: cur.mode.clone(),
    }
}

// ---------------------------------------------------------------------------
// Files on disk
// ---------------------------------------------------------------------------

fn paths(root: &str) -> Option<(PathBuf, PathBuf, PathBuf)> {
    let dir = cache_dir()?;
    let k = key(root);
    Some((dir.join(format!("{k}.idx")), dir.join(format!("{k}.snap")), dir.join(format!("{k}.prev.snap"))))
}

/// Writes to a temporary file and renames it into place, so a crash or a
/// full disk never leaves a half-written cache file behind.
fn write_atomic(path: &Path, f: impl FnOnce(&mut BufWriter<File>) -> io::Result<()>) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut w = BufWriter::with_capacity(1 << 20, File::create(&tmp)?);
    let r = f(&mut w).and_then(|()| w.flush());
    drop(w);
    match r {
        Ok(()) => fs::rename(&tmp, path),
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn load_snap(path: &Path) -> Option<Snapshot> {
    Snapshot::load(&mut BufReader::new(File::open(path).ok()?)).ok()
}

/// The scan time stored in a snapshot's header, without reading the rest
/// (reopen needs only this; parsing the whole C: snapshot cost ~46 ms).
fn snap_time(path: &Path) -> Option<u64> {
    let mut head = [0u8; 16];
    File::open(path).ok()?.read_exact(&mut head).ok()?;
    (&head[..8] == SNAP_MAGIC || &head[..8] == SNAP_MAGIC_V1).then(|| u64::from_le_bytes(head[8..].try_into().expect("8 bytes")))
}

/// After a scan: save the index (instant reopen) and the folder snapshot,
/// keep the previous snapshot, and return what changed since it (if there
/// was one). Runs on a background thread.
pub fn remember(idx: &Arc<Index>, stats: &ScanStats, mode: &str) -> io::Result<Option<Changes>> {
    let root = idx.name(0).to_owned();
    let (idx_path, snap_path, prev_path) = paths(&root).ok_or_else(|| io::Error::other("no LOCALAPPDATA"))?;
    fs::create_dir_all(idx_path.parent().expect("cache dir"))?;
    let mode = mode.replace(" via admin helper", "");
    let mut snap = Snapshot::of(idx);
    snap.mode.clone_from(&mode);
    let prev = load_snap(&snap_path).filter(|p| p.root.eq_ignore_ascii_case(&root));
    if prev.is_some() {
        let _ = fs::remove_file(&prev_path);
        fs::rename(&snap_path, &prev_path)?;
    }
    write_atomic(&snap_path, |w| snap.save(w))?;
    write_atomic(&idx_path, |w| idx.save(stats, &mode, w))?;
    // Size history is extra: a failure there never fails remembering.
    let _ = crate::history::record(&snap);
    prune();
    Ok(prev.map(|p| diff(&p, &snap, 50)))
}

/// Saves only the index of a scan (for instant reopen), leaving the folder
/// snapshots alone: they belong to "what changed". Used before freeing a
/// scan from memory, when the saved copy was dropped after a change.
pub fn save_index(idx: &Index, stats: &ScanStats, mode: &str) -> io::Result<()> {
    let (idx_path, _, _) = paths(idx.name(0)).ok_or_else(|| io::Error::other("no LOCALAPPDATA"))?;
    fs::create_dir_all(idx_path.parent().expect("cache dir"))?;
    let mode = mode.replace(" via admin helper", "");
    write_atomic(&idx_path, |w| idx.save(stats, &mode, w))
}

fn seen_path(root: &str) -> Option<PathBuf> {
    Some(cache_dir()?.join(format!("{}.seen.snap", key(root))))
}

/// Background scans: before replacing the current snapshot, keep it as the
/// "last seen" baseline if there isn't one yet (the state the user last
/// looked at), so the app can say what changed while they were away.
pub fn keep_seen(root: &str) {
    let (Some((_, snap, _)), Some(seen)) = (paths(root), seen_path(root)) else { return };
    if !seen.exists() && snap.exists() {
        let _ = fs::copy(&snap, &seen);
    }
}

/// What changed between the last-seen baseline and the latest scan, if
/// background scans ran since the user last looked.
pub fn away_changes(root: &str) -> Option<Changes> {
    let (_, snap, _) = paths(root)?;
    Some(diff(&load_snap(&seen_path(root)?)?, &load_snap(&snap)?, 50))
}

/// The user has seen the latest state: drop the baseline.
pub fn clear_seen(root: &str) {
    if let Some(p) = seen_path(root) {
        let _ = fs::remove_file(p);
    }
}

/// Whether a remembered index exists for `root` (instant reopen possible).
pub fn has_index(root: &str) -> bool {
    paths(root).is_some_and(|(p, _, _)| p.is_file())
}

/// The remembered scan of `root`, if any: (index, stats, mode, scan time).
pub fn reopen(root: &str) -> Option<(Index, ScanStats, String, u64)> {
    let (idx_path, snap_path, _) = paths(root)?;
    let f = File::open(&idx_path).ok()?;
    let len = f.metadata().ok()?.len();
    let (index, stats, mode) = Index::load_checked(&mut BufReader::with_capacity(1 << 20, f), len).ok()?;
    if !index.name(0).eq_ignore_ascii_case(root.trim()) && key(index.name(0)) != key(root) {
        return None;
    }
    let time = snap_time(&snap_path).unwrap_or_else(|| {
        fs::metadata(&idx_path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
    });
    Some((index, stats, mode, time))
}

/// Folder sizes of the scan before the latest one, keyed by
/// `path_key(relative path)`. Lets the tree show a "change" next to each
/// folder. ~16 bytes per folder.
pub type PrevSizes = HashMap<u64, u64>;

pub fn previous_sizes(root: &str) -> Option<PrevSizes> {
    let (_, _, prev_path) = paths(root)?;
    let s = load_snap(&prev_path)?;
    Some(s.dirs.iter().map(|d| (path_key(&d.path), d.bytes)).collect())
}

/// What changed between the last two remembered scans of `root`.
pub fn last_changes(root: &str) -> Option<Changes> {
    let (_, snap_path, prev_path) = paths(root)?;
    Some(diff(&load_snap(&prev_path)?, &load_snap(&snap_path)?, 50))
}

/// After a delete the remembered index is out of date: drop it (the folder
/// snapshots stay, so "what changed" still works).
pub fn forget_index(root: &str) {
    if let Some((idx_path, _, _)) = paths(root) {
        let _ = fs::remove_file(idx_path);
    }
}

/// Deletes every remembered scan. Only touches Xylem's own cache files.
pub fn forget_all() -> io::Result<usize> {
    let Some(dir) = cache_dir() else { return Ok(0) };
    let mut n = 0;
    let Ok(entries) = fs::read_dir(&dir) else { return Ok(0) };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        let ours = [".idx", ".snap", ".hist", ".tmp"].iter().any(|ext| name.ends_with(ext));
        if ours && e.file_type().is_ok_and(|t| t.is_file()) {
            fs::remove_file(e.path())?;
            n += 1;
        }
    }
    Ok(n)
}

/// Keeps the cache to MAX_ROOTS roots, dropping the least recently scanned.
fn prune() {
    let Some(dir) = cache_dir() else { return };
    let Ok(entries) = fs::read_dir(&dir) else { return };
    let mut idx: Vec<(SystemTime, String)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let k = name.strip_suffix(".idx")?.to_owned();
            Some((e.metadata().ok()?.modified().ok()?, k))
        })
        .collect();
    idx.sort_by_key(|x| std::cmp::Reverse(x.0));
    for (_, k) in idx.into_iter().skip(MAX_ROOTS) {
        for ext in ["idx", "snap", "prev.snap", "seen.snap", "hist"] {
            let _ = fs::remove_file(dir.join(format!("{k}.{ext}")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(sizes: &[(&str, u64)]) -> Index {
        // Builds C:\ with folders a, a\x, b and one file per entry.
        let mut idx = Index::new("C:\\");
        let a = idx.push(0, "a", 0, 0, flags::DIR);
        let ax = idx.push(a, "x", 0, 0, flags::DIR);
        let b = idx.push(0, "b", 0, 0, flags::DIR);
        for &(dir, sz) in sizes {
            let parent = match dir {
                "a" => a,
                "a\\x" => ax,
                "b" => b,
                _ => 0,
            };
            idx.push(parent, "f", sz, sz, 0);
        }
        idx.compute_totals();
        idx
    }

    #[test]
    fn diff_points_at_the_folder_that_changed() {
        const MB: u64 = 1 << 20;
        let before = Snapshot::of(&tree(&[("a\\x", 10 * MB), ("b", 50 * MB)]));
        let after = Snapshot::of(&tree(&[("a\\x", 900 * MB), ("b", 20 * MB)]));
        let ch = diff(&before, &after, 10);
        let paths: Vec<&str> = ch.items.iter().map(|c| c.path.as_str()).collect();
        // a\x grew; "a" and the root only grew because of it, so they're left out.
        assert_eq!(paths, ["a\\x", "b"], "{:?}", ch.items);
        assert_eq!(ch.items[0].delta(), i128::from(890 * MB));
        assert_eq!(ch.items[1].delta(), -i128::from(30 * MB));
        assert_eq!(ch.after_total, 920 * MB);
    }

    #[test]
    fn removed_and_new_folders_are_reported() {
        const MB: u64 = 1 << 20;
        let mut i1 = Index::new("C:\\");
        let old = i1.push(0, "old", 0, 0, flags::DIR);
        i1.push(old, "f", 300 * MB, 300 * MB, 0);
        i1.compute_totals();
        let mut i2 = Index::new("C:\\");
        let new = i2.push(0, "new", 0, 0, flags::DIR);
        i2.push(new, "f", 200 * MB, 200 * MB, 0);
        i2.compute_totals();
        let ch = diff(&Snapshot::of(&i1), &Snapshot::of(&i2), 10);
        let got: Vec<(&str, u64, u64)> = ch.items.iter().map(|c| (c.path.as_str(), c.before, c.after)).collect();
        assert_eq!(got, [("old", 300 * MB, 0), ("new", 0, 200 * MB)]);
    }

    #[test]
    fn snapshot_round_trips_and_rejects_damage() {
        let s = Snapshot::of(&tree(&[("a\\x", 5), ("b", 7)]));
        let mut buf = Vec::new();
        s.save(&mut buf).unwrap();
        let back = Snapshot::load(&mut buf.as_slice()).unwrap();
        assert_eq!(back.dirs.len(), s.dirs.len());
        assert_eq!(back.dirs[2].path, "a\\x");
        // The header-only read used by reopen agrees with the full load.
        let p = std::env::temp_dir().join(format!("xylem-snap-time-{}.snap", std::process::id()));
        std::fs::write(&p, &buf).unwrap();
        assert_eq!(snap_time(&p), Some(back.time));
        std::fs::write(&p, b"XYLMIDX1........").unwrap();
        assert_eq!(snap_time(&p), None, "wrong magic");
        let _ = std::fs::remove_file(&p);
        assert!(Snapshot::load(&mut &buf[..buf.len() - 1]).is_err(), "truncated");
        let mut extra = buf.clone();
        extra.push(0);
        assert!(Snapshot::load(&mut extra.as_slice()).is_err(), "trailing");
        // Fuzz: random damage must be rejected or loaded, never panic.
        let mut rng = 0x1234_5678_9abc_def0_u64;
        for _ in 0..20_000 {
            let mut b = buf.clone();
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let at = (rng as usize) % b.len();
            b[at] = (rng >> 32) as u8;
            if rng.is_multiple_of(3) {
                b.truncate((rng as usize >> 8) % (b.len() + 1));
            }
            let _ = Snapshot::load(&mut b.as_slice());
        }
    }

    #[test]
    fn scan_method_is_kept_and_mixed_methods_are_flagged() {
        const MB: u64 = 1 << 20;
        let mut walk = Snapshot::of(&tree(&[("a\\x", 10 * MB)]));
        walk.mode = "folder walk (not running as administrator)".into();
        let mut fast = Snapshot::of(&tree(&[("a\\x", 20 * MB)]));
        fast.mode = "MFT (direct read)".into();
        // The mode survives a save/load round trip.
        let mut buf = Vec::new();
        fast.save(&mut buf).unwrap();
        assert_eq!(Snapshot::load(&mut buf.as_slice()).unwrap().mode, "MFT (direct read)");
        assert!(diff(&walk, &fast, 10).mixed_methods(), "walk then Fast scan");
        assert!(diff(&fast, &walk, 10).mixed_methods(), "Fast scan then walk");
        let mut walk2 = walk.clone();
        walk2.mode = "folder walk".into();
        assert!(!diff(&walk, &walk2, 10).mixed_methods(), "two walks (reason text differs)");
        let mut old = walk.clone();
        old.mode = String::new();
        assert!(!diff(&old, &fast, 10).mixed_methods(), "unknown method: no warning");
        assert_eq!(Changes::method_name("MFT (direct read)"), "Fast scan");
        assert_eq!(Changes::method_name("folder walk"), "folder walk");
    }

    #[test]
    fn snapshots_from_xylem_0_2_still_load() {
        // Version 1 layout: magic, time, root, count, then (bytes, files, path) per folder.
        let mut v1 = Vec::new();
        v1.extend_from_slice(b"XYLMSNP1");
        v1.extend_from_slice(&1234u64.to_le_bytes());
        v1.extend_from_slice(&3u16.to_le_bytes());
        v1.extend_from_slice(b"C:\\");
        v1.extend_from_slice(&1u32.to_le_bytes());
        v1.extend_from_slice(&42u64.to_le_bytes());
        v1.extend_from_slice(&7u32.to_le_bytes());
        v1.extend_from_slice(&0u16.to_le_bytes());
        let s = Snapshot::load(&mut v1.as_slice()).unwrap();
        assert_eq!((s.time, s.root.as_str(), s.mode.as_str(), s.dirs.len(), s.dirs[0].bytes), (1234, "C:\\", "", 1, 42));
    }

    #[test]
    fn gap_text_reads_naturally() {
        assert_eq!(gap_text(5), "under a minute");
        assert_eq!(gap_text(180), "3 min");
        assert_eq!(gap_text(7200), "2 h");
        assert_eq!(gap_text(3 * 86_400), "3 days");
    }

    #[test]
    fn keys_ignore_case_and_trailing_slash() {
        assert_eq!(key("C:\\Users\\Me\\"), key("c:/users/me"));
        assert_ne!(key("C:\\"), key("D:\\"));
        assert_eq!(key("C:\\"), key("c:\\"));
        assert_eq!(age_text(now_secs()), "just now");
    }
}
