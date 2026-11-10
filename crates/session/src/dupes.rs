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

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::windows::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use serde::Serialize;

use crate::safety;

/// A set of files with identical contents.
#[derive(Clone, Debug, Serialize)]
pub struct DupGroup {
    pub size: u64,
    /// (node, full path) of each copy.
    pub files: Vec<(u32, String)>,
}

impl DupGroup {
    /// Space that deleting all but one copy would free.
    pub fn wasted(&self) -> u64 {
        self.size.saturating_mul(self.files.len().saturating_sub(1) as u64)
    }
}

/// Progress shared with the UI: bytes read so far, and the total to read.
#[derive(Default)]
pub struct Progress {
    pub read: AtomicU64,
    pub total: AtomicU64,
}

const EDGE: u64 = 64 * 1024;
/// RECALL_ON_DATA_ACCESS | RECALL_ON_OPEN | OFFLINE: reading would download
/// or wait for the file.
const DONT_READ: u32 = 0x0040_0000 | 0x0004_0000 | 0x1000;

fn readable(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && m.file_attributes() & DONT_READ == 0)
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

/// Same-size candidate groups -> groups of identical files, biggest waste
/// first. Blocking; run it on a worker thread.
pub fn find(candidates: Vec<(u64, Vec<(u32, String)>)>, cancel: &AtomicBool, p: &Progress) -> Vec<DupGroup> {
    // Rough total to read: the edges of every candidate (full reads are added as needed).
    let total: u64 = candidates.iter().map(|(s, f)| s.min(&(2 * EDGE)) * f.len() as u64).sum();
    p.total.store(total, Ordering::Relaxed);
    let mut out = Vec::new();
    for (size, files) in candidates {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        // Readable, one entry per real file (hardlinks collapse).
        let mut seen = std::collections::HashSet::new();
        let files: Vec<(u32, String)> = files
            .into_iter()
            .filter(|(_, path)| readable(Path::new(path)))
            .filter(|(_, path)| safety::file_id(Path::new(path)).is_ok_and(|id| seen.insert(id)))
            .collect();
        if files.len() < 2 {
            continue;
        }
        let mut by_quick: HashMap<[u8; 32], Vec<(u32, String)>> = HashMap::new();
        for f in files {
            if let Some(h) = quick_hash(Path::new(&f.1), size, p) {
                by_quick.entry(h).or_default().push(f);
            }
        }
        for (_, same) in by_quick.into_iter().filter(|(_, v)| v.len() > 1) {
            if size <= 2 * EDGE {
                // The quick hash already covered the whole file.
                out.push(DupGroup { size, files: same });
                continue;
            }
            p.total.fetch_add(size * same.len() as u64, Ordering::Relaxed);
            let mut by_full: HashMap<[u8; 32], Vec<(u32, String)>> = HashMap::new();
            for f in same {
                if let Some(h) = full_hash(Path::new(&f.1), cancel, p) {
                    by_full.entry(h).or_default().push(f);
                }
            }
            out.extend(by_full.into_values().filter(|v| v.len() > 1).map(|files| DupGroup { size, files }));
        }
    }
    // The copy to keep comes first: the "original-looking" one.
    for g in &mut out {
        g.files.sort_by_key(|(_, p)| (copy_score(p), p.len(), p.to_lowercase()));
    }
    out.sort_by_key(|g| std::cmp::Reverse(g.wasted()));
    out
}

/// Higher = more likely a copy: "copy" or " (1)" in the name, or inside a
/// backup / old / temp / Downloads / Recycle Bin folder.
fn copy_score(path: &str) -> u32 {
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
        let dir = std::env::temp_dir().join(format!("xylem-dupes-{}", std::process::id()));
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
        let _ = std::fs::remove_dir_all(&dir);
    }
}
