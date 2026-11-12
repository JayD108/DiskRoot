//! Size history: how folders and the drive grew over time.
//!
//! One file per scanned root, `<key>.hist` next to the remembered scan in
//! `%LOCALAPPDATA%\DiskRoot\cache\`. A full folder snapshot of C: is ~39 MB,
//! far too big to keep one per scan, so the history keeps only folders of
//! at least 1 MB (~26k on C:) and, per scan, only those whose size changed
//! since the scan before (~300 between two scans 1.5 h apart, ~3 KB).
//!
//! Each point also records the volume's size and free space, which is
//! exact and the same for Fast scan and folder walk: the "drive full in
//! ~N days" forecast uses only that.
//!
//! Thinning keeps everything for 2 days, the last scan of each day for 60
//! days and the last of each week after that, with a hard cap per root.
//! Like the rest of the cache, it holds folder names and sizes only and
//! never leaves the PC.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::Serialize;

use crate::remember::{Changes, SnapDir, Snapshot, cache_dir, diff, key, path_key};

const MAGIC: &[u8; 8] = b"XYLMHST1";
/// Folders smaller than this aren't tracked (they don't matter for space).
pub const FLOOR: u64 = 1 << 20;
/// Hard cap per root; the oldest points go first.
const CAP: u64 = 16 << 20;
const MAX_NAMES: u32 = 5_000_000;
const MAX_POINTS: u32 = 100_000;
const MAX_CHANGES: u32 = 5_000_000;
const DAY: u64 = 86_400;

/// One scan in the history.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Point {
    pub time: u64,
    /// 1 = folder walk, 2 = Fast scan, 0 = unknown.
    pub method: u8,
    /// Size of the scanned root (on disk) and its file count.
    pub total: u64,
    pub files: u64,
    /// The volume holding the root: size and free space (0 = unknown).
    pub vol_total: u64,
    pub vol_free: u64,
    /// (name id, bytes) for folders whose size changed since the previous
    /// point; 0 = gone or under 1 MB.
    pub changes: Vec<(u32, u64)>,
}

impl Point {
    pub fn used(&self) -> Option<u64> {
        (self.vol_total > 0).then(|| self.vol_total.saturating_sub(self.vol_free))
    }
}

#[derive(Clone, Debug, Default)]
pub struct History {
    pub root: String,
    /// Folder paths relative to the root ("" = the root).
    pub names: Vec<String>,
    pub points: Vec<Point>,
}

fn method_of(mode: &str) -> u8 {
    if mode.starts_with("MFT") {
        2
    } else if mode.starts_with("folder walk") {
        1
    } else {
        0
    }
}

pub fn method_text(m: u8) -> &'static str {
    match m {
        2 => "Fast scan",
        1 => "folder walk",
        _ => "scan",
    }
}

impl History {
    fn new(root: &str) -> Self {
        Self { root: root.to_owned(), ..Self::default() }
    }

    /// Folder sizes (>= 1 MB) after each point, one map at a time.
    fn replay(&self, mut f: impl FnMut(usize, &HashMap<u32, u64>)) {
        let mut state: HashMap<u32, u64> = HashMap::new();
        for (i, p) in self.points.iter().enumerate() {
            for &(id, b) in &p.changes {
                if b == 0 {
                    state.remove(&id);
                } else {
                    state.insert(id, b);
                }
            }
            f(i, &state);
        }
    }

    fn name_id(&self, rel: &str) -> Option<u32> {
        let k = path_key(rel);
        self.names.iter().position(|n| path_key(n) == k).map(|i| i as u32)
    }

    /// Size of one folder (relative path, "" = root) at every point:
    /// (time, bytes); 0 = not there or under 1 MB.
    pub fn series(&self, rel: &str) -> Vec<(u64, u64)> {
        if rel.is_empty() {
            return self.points.iter().map(|p| (p.time, p.total)).collect();
        }
        let Some(id) = self.name_id(rel) else { return Vec::new() };
        let mut cur = 0;
        self.points
            .iter()
            .map(|p| {
                if let Some(&(_, b)) = p.changes.iter().find(|c| c.0 == id) {
                    cur = b;
                }
                (p.time, cur)
            })
            .collect()
    }

    /// Folder sizes (>= 1 MB) at point `i`, as a snapshot.
    pub fn snapshot_at(&self, i: usize) -> Snapshot {
        let mut out = Snapshot { root: self.root.clone(), ..Snapshot::default() };
        self.replay(|j, state| {
            if j == i {
                out.time = self.points[i].time;
                // The texts `Changes::mixed_methods` recognises.
                out.mode = match self.points[i].method {
                    2 => "MFT (direct read)",
                    1 => "folder walk",
                    _ => "",
                }
                .to_owned();
                out.dirs = state.iter().map(|(&id, &b)| SnapDir { path: self.names[id as usize].clone(), bytes: b, files: 0 }).collect();
            }
        });
        // The root's exact size, even under 1 MB.
        if let Some(r) = out.dirs.iter_mut().find(|d| d.path.is_empty()) {
            r.bytes = self.points.get(i).map_or(r.bytes, |p| p.total);
        }
        out
    }

    /// Where the space went since `secs` ago: compares the latest point
    /// with the newest one at least that old (or the oldest there is).
    pub fn growers(&self, secs: u64, limit: usize) -> Option<Changes> {
        let last = self.points.len().checked_sub(1)?;
        if last == 0 {
            return None;
        }
        let since = self.points[last].time.saturating_sub(secs);
        let base = self.points[..last].iter().rposition(|p| p.time <= since).unwrap_or(0);
        Some(diff(&self.snapshot_at(base), &self.snapshot_at(last), limit))
    }

    /// Adds a scan: the folders >= 1 MB of `snap` and the volume space.
    pub fn add(&mut self, snap: &Snapshot, vol: Option<(u64, u64)>) {
        let mut ids: HashMap<u64, u32> = self.names.iter().enumerate().map(|(i, n)| (path_key(n), i as u32)).collect();
        let mut last: HashMap<u32, u64> = HashMap::new();
        self.replay(|i, s| {
            if i + 1 == self.points.len() {
                last = s.clone();
            }
        });
        let mut now: HashMap<u32, u64> = HashMap::new();
        let (mut total, mut files) = (0, 0);
        for d in &snap.dirs {
            if d.path.is_empty() {
                (total, files) = (d.bytes, u64::from(d.files));
            }
            if d.bytes < FLOOR {
                continue;
            }
            let id = *ids.entry(path_key(&d.path)).or_insert_with(|| {
                self.names.push(d.path.clone());
                (self.names.len() - 1) as u32
            });
            now.insert(id, d.bytes);
        }
        let mut changes: Vec<(u32, u64)> = now.iter().filter(|&(id, b)| last.get(id) != Some(b)).map(|(&id, &b)| (id, b)).collect();
        changes.extend(last.keys().filter(|id| !now.contains_key(id)).map(|&id| (id, 0)));
        changes.sort_unstable();
        let (vol_total, vol_free) = vol.unwrap_or((0, 0));
        // A scan can't be older than the last one (clock changes): keep order.
        let time = snap.time.max(self.points.last().map_or(0, |p| p.time));
        self.points.push(Point { time, method: method_of(&snap.mode), total, files, vol_total, vol_free, changes });
    }

    /// Thins old points (see the module docs) and drops unused names.
    /// Sizes at the points that stay are unchanged.
    pub fn thin(&mut self, now: u64) {
        let keep = keep_mask(&self.points.iter().map(|p| p.time).collect::<Vec<_>>(), now);
        self.rebuild(&keep);
        while self.points.len() > 2 && self.encoded_len() > CAP {
            let mut keep = vec![true; self.points.len()];
            let drop = (self.points.len() / 10).max(1);
            keep[..drop].iter_mut().for_each(|k| *k = false);
            self.rebuild(&keep);
        }
    }

    /// Keeps only the points marked in `keep`, re-expressing each one's
    /// changes against the previous kept point, then renumbers names.
    fn rebuild(&mut self, keep: &[bool]) {
        let mut out: Vec<Point> = Vec::new();
        let mut emitted: HashMap<u32, u64> = HashMap::new();
        let points = std::mem::take(&mut self.points);
        let mut state: HashMap<u32, u64> = HashMap::new();
        for (i, p) in points.into_iter().enumerate() {
            for &(id, b) in &p.changes {
                if b == 0 {
                    state.remove(&id);
                } else {
                    state.insert(id, b);
                }
            }
            if !keep[i] {
                continue;
            }
            let mut ch: Vec<(u32, u64)> = state.iter().filter(|&(id, b)| emitted.get(id) != Some(b)).map(|(&id, &b)| (id, b)).collect();
            ch.extend(emitted.keys().filter(|id| !state.contains_key(id)).map(|&id| (id, 0)));
            ch.sort_unstable();
            emitted.clone_from(&state);
            out.push(Point { changes: ch, ..p });
        }
        // Renumber: only names some kept point mentions.
        let mut map: HashMap<u32, u32> = HashMap::new();
        let mut names = Vec::new();
        for p in &mut out {
            for c in &mut p.changes {
                let new = *map.entry(c.0).or_insert_with(|| {
                    names.push(std::mem::take(&mut self.names[c.0 as usize]));
                    (names.len() - 1) as u32
                });
                c.0 = new;
            }
        }
        self.names = names;
        self.points = out;
    }

    fn encoded_len(&self) -> u64 {
        let names: usize = self.names.iter().map(|n| n.len() + 2).sum();
        let points: usize = self.points.iter().map(|p| 45 + p.changes.len() * 12).sum();
        (names + points + 64) as u64
    }

    pub fn save(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(MAGIC)?;
        write_str(w, &self.root)?;
        w.write_all(&(self.names.len() as u32).to_le_bytes())?;
        for n in &self.names {
            write_str(w, n)?;
        }
        w.write_all(&(self.points.len() as u32).to_le_bytes())?;
        for p in &self.points {
            w.write_all(&p.time.to_le_bytes())?;
            w.write_all(&[p.method])?;
            for v in [p.total, p.files, p.vol_total, p.vol_free] {
                w.write_all(&v.to_le_bytes())?;
            }
            w.write_all(&(p.changes.len() as u32).to_le_bytes())?;
            for &(id, b) in &p.changes {
                w.write_all(&id.to_le_bytes())?;
                w.write_all(&b.to_le_bytes())?;
            }
        }
        Ok(())
    }

    /// Reads a history file, rejecting anything malformed (bad magic,
    /// absurd counts, names out of range, truncation, trailing bytes).
    pub fn load(r: &mut impl Read) -> io::Result<Self> {
        let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidData, format!("history: {m}"));
        let mut magic = [0u8; 8];
        r.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(bad("not a DiskRoot history"));
        }
        let root = read_str(r)?;
        let n = read_u32(r)?;
        if n > MAX_NAMES {
            return Err(bad("too many names"));
        }
        let mut names = Vec::with_capacity(n.min(1 << 16) as usize);
        for _ in 0..n {
            names.push(read_str(r)?);
        }
        let np = read_u32(r)?;
        if np > MAX_POINTS {
            return Err(bad("too many points"));
        }
        let mut points = Vec::with_capacity(np.min(1 << 12) as usize);
        for _ in 0..np {
            let time = read_u64(r)?;
            let mut m = [0u8; 1];
            r.read_exact(&mut m)?;
            let (total, files, vol_total, vol_free) = (read_u64(r)?, read_u64(r)?, read_u64(r)?, read_u64(r)?);
            let nc = read_u32(r)?;
            if nc > MAX_CHANGES {
                return Err(bad("too many changes"));
            }
            let mut changes = Vec::with_capacity(nc.min(1 << 16) as usize);
            for _ in 0..nc {
                let id = read_u32(r)?;
                if id >= n {
                    return Err(bad("name out of range"));
                }
                changes.push((id, read_u64(r)?));
            }
            // `add` keeps points in time order; anything else is damage.
            if points.last().is_some_and(|p: &Point| p.time > time) {
                return Err(bad("points out of order"));
            }
            points.push(Point { time, method: m[0], total, files, vol_total, vol_free, changes });
        }
        let mut extra = [0u8; 1];
        if r.read(&mut extra)? != 0 {
            return Err(bad("trailing data"));
        }
        Ok(Self { root, names, points })
    }
}

/// Which points to keep: all from the last 2 days, the last of each day
/// for 60 days, the last of each week before that; always the newest.
pub fn keep_mask(times: &[u64], now: u64) -> Vec<bool> {
    let mut keep = vec![false; times.len()];
    let bucket = |t: u64| -> Option<(u8, u64)> {
        let age = now.saturating_sub(t);
        if age < 2 * DAY {
            None // keep all
        } else if age < 60 * DAY {
            Some((1, t / DAY))
        } else {
            Some((2, t / (7 * DAY)))
        }
    };
    for i in 0..times.len() {
        keep[i] = match (bucket(times[i]), times.get(i + 1).map(|&t| bucket(t))) {
            (None, _) => true,
            (Some(_), None) => true, // newest
            (Some(b), Some(next)) => next != Some(b),
        };
    }
    keep
}

// ---------------------------------------------------------------------------
// "Drive full in ~N days"
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Forecast {
    /// Not enough history yet.
    NeedMore { days: u32 },
    /// Used space isn't growing (bytes a day, <= 0 or tiny).
    NotFilling { per_day: f64, span_days: u32, jumps: u32 },
    /// Growing: full in about low..high days (high None = "or longer").
    /// `jumps`: sudden one-off changes left out of the pace.
    Full { per_day: f64, low: u32, high: Option<u32>, span_days: u32, free: u64, jumps: u32 },
}

/// Robust trend of the volume's used space over the last 60 days: the
/// last scan of each day; rare sudden jumps (a day's change far beyond the
/// usual one, e.g. one big download) are taken out, since they already
/// happened and say nothing about the pace; then a Theil-Sen slope
/// (median of all pairwise slopes), with the range from their quartiles.
pub fn forecast(points: &[Point]) -> Forecast {
    let Some(last) = points.iter().rev().find(|p| p.used().is_some()) else { return Forecast::NeedMore { days: 0 } };
    let from = last.time.saturating_sub(60 * DAY);
    let mut daily: Vec<(u64, f64)> = Vec::new();
    for p in points.iter().filter(|p| p.time >= from) {
        let Some(u) = p.used() else { continue };
        match daily.last_mut() {
            Some(d) if d.0 / DAY == p.time / DAY => *d = (p.time, u as f64),
            _ => daily.push((p.time, u as f64)),
        }
    }
    let span = daily.last().map_or(0, |l| l.0).saturating_sub(daily.first().map_or(0, |f| f.0));
    let span_days = (span / DAY) as u32;
    if daily.len() < 4 || span < 7 * DAY {
        return Forecast::NeedMore { days: span_days };
    }
    let jumps = remove_jumps(&mut daily);
    let mut slopes = Vec::with_capacity(daily.len() * daily.len() / 2);
    for i in 0..daily.len() {
        for j in i + 1..daily.len() {
            let dt = daily[j].0.saturating_sub(daily[i].0) as f64 / DAY as f64;
            if dt > 0.5 {
                slopes.push((daily[j].1 - daily[i].1) / dt);
            }
        }
    }
    slopes.sort_by(f64::total_cmp);
    let q = |f: f64| slopes[((slopes.len() - 1) as f64 * f).round() as usize];
    let (q1, med, q3) = (q(0.25), q(0.5), q(0.75));
    let free = last.vol_free;
    // Under ~1 MB a day, or more than 10 years away: not filling up.
    if med <= 1e6 || free as f64 / med > 3650.0 {
        return Forecast::NotFilling { per_day: med, span_days, jumps };
    }
    let days = |rate: f64| (free as f64 / rate).round().clamp(0.0, 1e6) as u32;
    let low = days(q3.max(med));
    let high = (q1 > 1e6).then(|| days(q1));
    Forecast::Full { per_day: med, low, high, span_days, free, jumps }
}

/// Takes out rare sudden jumps between consecutive days: a change bigger
/// than 8x the usual daily change and at least 1 GB, when at most 1 in 5
/// days has one (more often than that, they *are* the pace). Later days
/// are shifted so only the jump itself is removed. Returns how many.
fn remove_jumps(daily: &mut [(u64, f64)]) -> u32 {
    let steps: Vec<f64> = daily.windows(2).map(|w| w[1].1 - w[0].1).collect();
    let mut abs: Vec<f64> = steps.iter().map(|d| d.abs()).collect();
    abs.sort_by(f64::total_cmp);
    let usual = abs.get(abs.len() / 2).copied().unwrap_or(0.0);
    let limit = (8.0 * usual).max(f64::from(1u32 << 30));
    let big: Vec<usize> = (0..steps.len()).filter(|&i| steps[i].abs() > limit).collect();
    if big.is_empty() || big.len() * 5 > steps.len() {
        return 0;
    }
    // A removed jump counts as a typical day.
    let mut signed = steps.clone();
    signed.sort_by(f64::total_cmp);
    let typical = signed[signed.len() / 2];
    let mut shift = 0.0;
    for i in 0..steps.len() {
        if big.contains(&i) {
            shift += steps[i] - typical;
        }
        daily[i + 1].1 -= shift;
    }
    big.len() as u32
}

impl Forecast {
    /// One honest sentence for `drive` (e.g. "C:").
    pub fn text(&self, drive: &str) -> String {
        let gb = |b: f64| diskroot_core::format::bytes(b.abs() as u64);
        match self {
            Self::NeedMore { days } => format!(
                "A forecast needs at least a week of scans of {drive} ({} so far). Scheduled scans collect them for you.",
                match days {
                    0 => "less than a day".to_owned(),
                    1 => "1 day".to_owned(),
                    d => format!("{d} days"),
                }
            ),
            Self::NotFilling { per_day, span_days, jumps } => {
                let s = if *per_day < -1e6 {
                    format!("{drive} isn't filling up: used space went down by about {} a day over the last {span_days} days", gb(*per_day))
                } else {
                    format!("{drive} isn't filling up: used space stayed about the same over the last {span_days} days")
                };
                format!("{s}{}.", jumps_text(*jumps))
            }
            Self::Full { per_day, low, high, span_days, jumps, .. } => {
                let when = match (low, high) {
                    (l, _) if *l > 730 => "more than 2 years".to_owned(),
                    (l, None) => format!("{} or longer", days_text(*l)),
                    (l, Some(h)) if *h > 730 => format!("{} to over 2 years", days_text(*l)),
                    (l, Some(h)) if days_text(*l) == days_text(*h) => format!("about {}", days_text(*l)),
                    (l, Some(h)) => format!("about {}–{}", round_days(*l), days_text(*h)),
                };
                format!("At the pace of the last {span_days} days (+{} a day{}), {drive} would be full in {when}.", gb(*per_day), jumps_text(*jumps))
            }
        }
    }
}

fn jumps_text(n: u32) -> &'static str {
    match n {
        0 => "",
        1 => ", not counting one sudden jump",
        _ => ", not counting a few sudden jumps",
    }
}

fn round_days(d: u32) -> u32 {
    match d {
        0..=14 => d,
        15..=60 => (d + 2) / 5 * 5,
        _ => (d + 5) / 10 * 10,
    }
}

fn days_text(d: u32) -> String {
    match round_days(d) {
        0 => "under a day".into(),
        1 => "1 day".into(),
        d => format!("{d} days"),
    }
}

// ---------------------------------------------------------------------------
// Files on disk
// ---------------------------------------------------------------------------

pub fn path(root: &str) -> Option<PathBuf> {
    Some(cache_dir()?.join(format!("{}.hist", key(root))))
}

/// The history of `root`, if there is one (and it isn't damaged).
pub fn load(root: &str) -> Option<History> {
    let p = path(root)?;
    let (mut r, _) = crate::cachefile::open(&p).ok()?;
    let h = History::load(&mut r).ok()?;
    (key(&h.root) == key(root)).then_some(h)
}

/// After a scan is remembered: add it to the root's history. A damaged
/// or foreign file is replaced by a fresh history.
pub fn record(snap: &Snapshot) -> io::Result<()> {
    let p = path(&snap.root).ok_or_else(|| io::Error::other("no LOCALAPPDATA"))?;
    let mut h = load(&snap.root).unwrap_or_else(|| History::new(&snap.root));
    h.add(snap, crate::disk_space(&snap.root));
    h.thin(snap.time);
    crate::cachefile::write(&p, |mut w| h.save(&mut w))
}

/// Deletes the history of `root`.
pub fn clear(root: &str) {
    if let Some(p) = path(root) {
        let _ = fs::remove_file(p);
    }
}

/// Bytes used by all history files.
pub fn disk_usage() -> u64 {
    let Some(dir) = cache_dir() else { return 0 };
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    entries.flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".hist")).filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()
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
    String::from_utf8(s).map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "history: bad UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;
    const MB: u64 = 1 << 20;
    const GB: u64 = 1 << 30;

    fn snap(time: u64, dirs: &[(&str, u64)]) -> Snapshot {
        Snapshot {
            time,
            root: "C:\\".into(),
            mode: "folder walk".into(),
            dirs: dirs.iter().map(|&(p, b)| SnapDir { path: p.into(), bytes: b, files: 1 }).collect(),
        }
    }

    #[test]
    fn records_only_changes_and_replays_exact_sizes() {
        let mut h = History::new("C:\\");
        h.add(&snap(1000, &[("", 500 * MB), ("a", 300 * MB), ("a\\x", 200 * MB), ("tiny", 10)]), Some((100 * GB, 40 * GB)));
        h.add(&snap(2000, &[("", 700 * MB), ("a", 500 * MB), ("a\\x", 200 * MB)]), Some((100 * GB, 39 * GB)));
        h.add(&snap(3000, &[("", 300 * MB), ("b", 2 * MB)]), None);
        assert!(!h.names.iter().any(|n| n == "tiny"), "under 1 MB isn't tracked");
        assert_eq!(h.points[1].changes.len(), 2, "a\\x didn't change: {:?}", h.points[1].changes);
        assert_eq!(h.series("a\\x"), [(1000, 200 * MB), (2000, 200 * MB), (3000, 0)]);
        assert_eq!(h.series("A"), [(1000, 300 * MB), (2000, 500 * MB), (3000, 0)], "case-insensitive");
        assert_eq!(h.series(""), [(1000, 500 * MB), (2000, 700 * MB), (3000, 300 * MB)]);
        assert_eq!(h.points[0].used(), Some(60 * GB));
        assert_eq!(h.points[2].used(), None);
        let s = h.snapshot_at(1);
        let mut got: Vec<(String, u64)> = s.dirs.iter().map(|d| (d.path.clone(), d.bytes)).collect();
        got.sort();
        assert_eq!(got, [("".into(), 700 * MB), ("a".into(), 500 * MB), ("a\\x".into(), 200 * MB)]);
    }

    #[test]
    fn growers_point_at_the_folder_that_grew() {
        let mut h = History::new("C:\\");
        h.add(&snap(0, &[("", 2 * GB), ("a", GB), ("a\\x", 100 * MB), ("b", GB)]), None);
        h.add(&snap(DAY, &[("", 4 * GB), ("a", 3 * GB), ("a\\x", 2100 * MB), ("b", GB)]), None);
        let g = h.growers(7 * DAY, 10).unwrap();
        assert_eq!(g.items[0].path, "a\\x");
        assert_eq!(g.items[0].delta(), i128::from(2000 * MB));
        assert!(!g.mixed_methods());
        let mut fast = snap(2 * DAY, &[("", 4 * GB), ("a", 3 * GB), ("a\\x", 2100 * MB), ("b", GB)]);
        fast.mode = "MFT (direct read)".into();
        h.add(&fast, None);
        assert!(h.growers(DAY, 10).unwrap().mixed_methods(), "walk then Fast scan is flagged");
        assert!(History::new("C:\\").growers(DAY, 10).is_none());
    }

    #[test]
    fn thinning_keeps_the_right_points_and_exact_sizes() {
        let now = 200 * DAY;
        // A scan every 4 hours for 200 days; folder "a" grows 1 MB each time.
        let mut h = History::new("C:\\");
        let mut t = 0;
        let mut k = 0;
        while t <= now {
            k += 1;
            h.add(&snap(t, &[("", (k + 10) * MB), ("a", (k + 1) * MB), ("same", 5 * MB)]), Some((GB, GB / 2)));
            t += 4 * 3600;
        }
        let before: Vec<(u64, u64)> = h.series("a");
        h.thin(now);
        let after = h.series("a");
        assert!(after.iter().all(|p| before.contains(p)), "sizes at kept points are unchanged");
        let recent = after.iter().filter(|p| now - p.0 < 2 * DAY).count();
        assert_eq!(recent, 12, "every scan from the last 2 days");
        let daily = after.iter().filter(|p| (2 * DAY..60 * DAY).contains(&(now - p.0))).count();
        assert!((57..=59).contains(&daily), "one a day: {daily}");
        let weekly = after.iter().filter(|p| now - p.0 >= 60 * DAY).count();
        assert!((19..=22).contains(&weekly), "one a week: {weekly}");
        assert_eq!(after.last(), before.last(), "newest kept");
        assert_eq!(h.series("same").iter().filter(|p| p.1 == 5 * MB).count(), h.points.len());
        // Round trip after thinning.
        let mut buf = Vec::new();
        h.save(&mut buf).unwrap();
        let back = History::load(&mut buf.as_slice()).unwrap();
        assert_eq!(back.series("a"), after);
        assert!(buf.len() < 200_000, "{} bytes", buf.len());
    }

    #[test]
    fn a_month_of_scans_on_a_big_drive_stays_small() {
        // 26k folders >= 1 MB (like C:), ~300 change per scan, every 4 h for 30 days.
        let names: Vec<String> = (0..26_000).map(|i| format!("Users\\me\\folder {i}\\sub")).collect();
        let mut sizes: Vec<u64> = (0..26_000).map(|i| (i % 500 + 1) * MB).collect();
        let mut h = History::new("C:\\");
        let mut rng = 7u64;
        for scan in 0..180u64 {
            for _ in 0..300 {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                sizes[(rng % 26_000) as usize] += 4096;
            }
            let mut dirs: Vec<(&str, u64)> = names.iter().map(String::as_str).zip(sizes.iter().copied()).collect();
            dirs.push(("", 100 * GB));
            h.add(&snap(scan * 4 * 3600, &dirs), Some((500 * GB, 100 * GB)));
            h.thin(scan * 4 * 3600);
        }
        let mut buf = Vec::new();
        h.save(&mut buf).unwrap();
        assert!(buf.len() < 8 << 20, "{} bytes", buf.len());
        eprintln!("simulated month: {:.1} MB, {} points", buf.len() as f64 / MB as f64, h.points.len());
    }

    #[test]
    fn loader_rejects_damage_and_never_panics() {
        let mut h = History::new("C:\\");
        h.add(&snap(1, &[("", 5 * MB), ("a", 2 * MB)]), Some((GB, MB)));
        h.add(&snap(2, &[("", 6 * MB), ("a", 3 * MB), ("b", 2 * MB)]), Some((GB, MB)));
        let mut buf = Vec::new();
        h.save(&mut buf).unwrap();
        assert_eq!(History::load(&mut buf.as_slice()).unwrap().points, h.points);
        assert!(History::load(&mut &buf[..buf.len() - 1]).is_err(), "truncated");
        let mut extra = buf.clone();
        extra.push(0);
        assert!(History::load(&mut extra.as_slice()).is_err(), "trailing");
        let mut swapped = h.clone();
        swapped.points.swap(0, 1);
        let mut sb = Vec::new();
        swapped.save(&mut sb).unwrap();
        assert!(History::load(&mut sb.as_slice()).is_err(), "times out of order");
        let mut rng = 0x9e37_79b9_7f4a_7c15_u64;
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
            if let Ok(h) = History::load(&mut b.as_slice()) {
                // Whatever loads must be safe to use.
                let _ = h.series("a");
                let _ = h.growers(DAY, 5);
                let _ = forecast(&h.points);
            }
        }
    }

    fn daily(used_gb: &[f64]) -> Vec<Point> {
        used_gb
            .iter()
            .enumerate()
            .map(|(d, &u)| Point { time: d as u64 * DAY + 3600, vol_total: 500 * GB, vol_free: 500 * GB - (u * GB as f64) as u64, ..Point::default() })
            .collect()
    }

    #[test]
    fn forecast_steady_growth() {
        // 400 GB used, +2 GB a day, 100 GB free at the end -> ~50 days.
        let pts = daily(&(0..21).map(|d| 360.0 + 2.0 * d as f64).collect::<Vec<_>>());
        let Forecast::Full { per_day, low, high, span_days, .. } = forecast(&pts) else { panic!("{:?}", forecast(&pts)) };
        assert!((per_day / GB as f64 - 2.0).abs() < 0.01);
        assert_eq!((low, high, span_days), (50, Some(50), 20));
        assert_eq!(forecast(&pts).text("C:"), "At the pace of the last 20 days (+2.00 GB a day), C: would be full in about 50 days.");
    }

    #[test]
    fn forecast_ignores_a_one_off_jump() {
        // Flat, then one 50 GB download on day 10, flat again.
        let pts = daily(&(0..21).map(|d| if d < 10 { 300.0 } else { 350.0 }).collect::<Vec<_>>());
        let f = forecast(&pts);
        assert!(matches!(f, Forecast::NotFilling { .. }), "{f:?}");
        // Steady +1 GB/day with one jump still reads about 1 GB a day.
        let pts = daily(&(0..30).map(|d| 300.0 + d as f64 + if d >= 15 { 40.0 } else { 0.0 }).collect::<Vec<_>>());
        let Forecast::Full { per_day, .. } = forecast(&pts) else { panic!() };
        assert!((per_day / GB as f64 - 1.0).abs() < 0.05, "{}", per_day / GB as f64);
        assert!(forecast(&pts).text("C:").contains("not counting one sudden jump"), "{}", forecast(&pts).text("C:"));
        // Big changes every few days are the pace, not one-off jumps.
        let pts = daily(&(0..21).map(|d| 300.0 + 20.0 * (d / 3) as f64).collect::<Vec<_>>());
        let Forecast::Full { per_day, jumps, .. } = forecast(&pts) else { panic!() };
        assert_eq!(jumps, 0);
        assert!(per_day / (GB as f64) > 5.0, "{}", per_day / GB as f64);
    }

    #[test]
    fn forecast_shrinking_and_too_little_data() {
        let pts = daily(&(0..14).map(|d| 300.0 - d as f64).collect::<Vec<_>>());
        let f = forecast(&pts);
        assert!(matches!(f, Forecast::NotFilling { .. }), "{f:?}");
        assert!(f.text("C:").contains("went down"), "{}", f.text("C:"));
        assert_eq!(forecast(&daily(&[1.0, 2.0, 3.0, 4.0, 5.0])), Forecast::NeedMore { days: 4 });
        assert_eq!(forecast(&[]), Forecast::NeedMore { days: 0 });
        assert!(forecast(&daily(&[1.0, 2.0])).text("C:").contains("1 day so far"));
        // Many scans on only 3 days: still not enough.
        let mut pts = Vec::new();
        for d in [0u64, 1, 9] {
            for h in 0..6 {
                pts.push(Point { time: d * DAY + h * 3600, vol_total: 100 * GB, vol_free: 50 * GB - d * GB, ..Point::default() });
            }
        }
        assert!(matches!(forecast(&pts), Forecast::NeedMore { .. }));
    }

    #[test]
    fn forecast_far_away_reads_as_years() {
        let pts = daily(&(0..20).map(|d| 10.0 + 0.1 * d as f64).collect::<Vec<_>>());
        let t = forecast(&pts).text("D:");
        assert!(t.contains("more than 2 years") || t.contains("isn't filling"), "{t}");
    }
}
