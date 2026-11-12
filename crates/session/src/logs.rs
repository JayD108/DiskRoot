//! Log files that keep growing ("runaway logs"): they take space now and
//! wear the SSD over time. After each remembered scan, the biggest `.log`
//! and `.etl` files (10 MB or more, at most 300) are noted in
//! `<key>.logs.json` next to the remembered scan (last 8 scans). A file
//! that grew in each of the last 3 or more scans is listed.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use diskroot_core::{Index, flags};

const MIN: u64 = 10 << 20;
const MAX_FILES: usize = 300;
const KEEP_SCANS: usize = 8;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Noted {
    pub time: u64,
    /// (path relative to the root, bytes on disk)
    pub files: Vec<(String, u64)>,
}

/// A log file that grew in each of the last `scans` scans.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Growing {
    pub path: String,
    pub first: u64,
    pub last: u64,
    pub scans: usize,
    /// Seconds between the first and the last of those scans.
    pub over: u64,
}

fn is_log(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".log") || n.ends_with(".etl")
}

fn rel_path(idx: &Index, mut id: u32) -> String {
    let mut parts = Vec::new();
    while id != 0 && parts.len() < 512 {
        parts.push(idx.name(id));
        let p = idx.parent(id);
        if p == diskroot_core::NO_PARENT {
            break;
        }
        id = p;
    }
    parts.reverse();
    parts.join("\\")
}

/// The biggest log files in a scan.
pub fn collect(idx: &Index) -> Vec<(String, u64)> {
    let mut big: Vec<(u32, u64)> = (1..idx.len() as u32)
        .filter(|&id| !idx.is_dir(id) && idx.flags(id) & flags::DELETED == 0 && idx.alloc(id) >= MIN && is_log(idx.name(id)))
        .map(|id| (id, idx.alloc(id)))
        .collect();
    big.sort_unstable_by_key(|x| std::cmp::Reverse(x.1));
    big.truncate(MAX_FILES);
    big.into_iter().map(|(id, b)| (rel_path(idx, id), b)).collect()
}

/// Files that grew in each of the last 3+ noted scans (by at least 10 MB in
/// total), biggest growth first.
pub fn growing(noted: &[Noted]) -> Vec<Growing> {
    if noted.len() < 3 {
        return Vec::new();
    }
    let maps: Vec<HashMap<String, u64>> = noted.iter().map(|n| n.files.iter().map(|(p, b)| (p.to_lowercase(), *b)).collect()).collect();
    let last = noted.last().expect("3+ scans");
    let mut out = Vec::new();
    for (path, now) in &last.files {
        let key = path.to_lowercase();
        // Walk back while each older scan had it strictly smaller.
        let mut k = maps.len() - 1;
        let mut size = *now;
        while k > 0 {
            match maps[k - 1].get(&key) {
                Some(&b) if b < size => {
                    size = b;
                    k -= 1;
                }
                _ => break,
            }
        }
        let scans = maps.len() - k;
        if scans >= 3 && now.saturating_sub(size) >= MIN {
            out.push(Growing { path: path.clone(), first: size, last: *now, scans, over: last.time.saturating_sub(noted[k].time) });
        }
    }
    out.sort_by_key(|g| std::cmp::Reverse(g.last - g.first));
    out
}

fn path(root: &str) -> Option<std::path::PathBuf> {
    Some(crate::remember::cache_dir()?.join(format!("{}.logs.json", crate::remember::key(root))))
}

pub fn load(root: &str) -> Vec<Noted> {
    path(root).and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// After a remembered scan: note its log files.
pub fn record(idx: &Index, time: u64) {
    let root = idx.name(0).to_owned();
    let Some(p) = path(&root) else { return };
    let mut all = load(&root);
    all.push(Noted { time, files: collect(idx) });
    if all.len() > KEEP_SCANS {
        all.drain(..all.len() - KEEP_SCANS);
    }
    if let Ok(text) = serde_json::to_string(&all) {
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const MB: u64 = 1 << 20;

    fn n(t: u64, files: &[(&str, u64)]) -> Noted {
        Noted { time: t, files: files.iter().map(|&(p, b)| (p.to_owned(), b * MB)).collect() }
    }

    #[test]
    fn finds_logs_that_grow_every_scan() {
        let noted = [
            n(0, &[("Windows\\Logs\\CBS\\CBS.log", 100), ("app\\steady.log", 50), ("app\\big.etl", 20)]),
            n(10, &[("Windows\\Logs\\CBS\\CBS.log", 140), ("app\\steady.log", 50), ("app\\big.etl", 25)]),
            n(20, &[("windows\\logs\\cbs\\cbs.log", 190), ("app\\steady.log", 50), ("app\\big.etl", 27)]),
        ];
        let g = growing(&noted);
        assert_eq!(g.len(), 1, "steady doesn't grow, big.etl grew only 7 MB: {g:?}");
        assert_eq!((g[0].first, g[0].last, g[0].scans, g[0].over), (100 * MB, 190 * MB, 3, 20));
        assert!(growing(&noted[..2]).is_empty(), "needs 3 scans");
        // A shrink in between (log rotated) breaks the run.
        let rotated = [n(0, &[("a.log", 100)]), n(1, &[("a.log", 20)]), n(2, &[("a.log", 40)]), n(3, &[("a.log", 60)])];
        let g = growing(&rotated);
        assert_eq!((g[0].first, g[0].scans), (20 * MB, 3));
    }

    #[test]
    fn collects_big_log_files_with_their_paths() {
        let mut idx = Index::new("C:\\");
        let w = idx.push(0, "Windows", 0, 0, flags::DIR);
        let l = idx.push(w, "Logs", 0, 0, flags::DIR);
        idx.push(l, "CBS.log", 50 * MB, 50 * MB, 0);
        idx.push(l, "small.log", MB, MB, 0);
        idx.push(l, "big.bin", 90 * MB, 90 * MB, 0);
        idx.push(w, "trace.ETL", 12 * MB, 12 * MB, 0);
        idx.compute_totals();
        assert_eq!(collect(&idx), [("Windows\\Logs\\CBS.log".to_owned(), 50 * MB), ("Windows\\trace.ETL".to_owned(), 12 * MB)]);
    }
}
