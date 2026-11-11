//! Core data model for the disk analyzer.
//!
//! The file tree is a flat struct-of-arrays indexed by `u32` node ids rather
//! than a pointer tree. That costs far less memory per file and is much
//! friendlier to the CPU cache.
//!
//! Invariant: a node's parent always has a smaller id than the node itself.
//! Folder totals can therefore be rolled up with a single reverse pass.

pub mod classify;
pub mod format;
pub mod report;
pub mod store;
pub mod view;
pub mod win;

use std::time::Duration;

/// Result of scanning a drive or folder.
pub struct Scan {
    pub index: Index,
    pub stats: ScanStats,
}

#[derive(Clone, Debug, Default)]
pub struct ScanStats {
    pub files: u64,
    pub dirs: u64,
    /// Folders that could not be listed (usually access denied).
    pub unreadable: u64,
    /// MFT records whose parent chain doesn't reach the root (MFT scan only).
    pub orphans: u64,
    pub elapsed: Duration,
}

/// Parent id of the root node.
pub const NO_PARENT: u32 = u32::MAX;

/// Per-node flag bits.
pub mod flags {
    pub const DIR: u8 = 1 << 0;
    /// Symlink or junction (name-surrogate reparse point). Never followed.
    pub const LINK: u8 = 1 << 1;
    /// Cloud placeholder, e.g. a OneDrive "online-only" file.
    pub const CLOUD: u8 = 1 << 2;
    /// Folder could not be listed (e.g. access denied).
    pub const UNREADABLE: u8 = 1 << 3;
    pub const COMPRESSED: u8 = 1 << 4;
    pub const SPARSE: u8 = 1 << 5;
    /// Deleted from disk through Xylem; excluded from totals and views.
    pub const DELETED: u8 = 1 << 6;
}

/// Raw columns for [`Index::from_columns`]. Index `i` describes node `i`.
#[derive(Default)]
pub struct Columns {
    pub parent: Vec<u32>,
    pub size: Vec<u64>,
    pub alloc: Vec<u64>,
    pub flags: Vec<u8>,
    pub name_off: Vec<u32>,
    pub name_len: Vec<u16>,
    pub names: String,
    /// Last-modified day of each node ([`filetime_days`]); 0 = unknown.
    /// May be left empty (all unknown).
    pub mtime: Vec<u16>,
}

/// Seconds between 1601-01-01 (FILETIME epoch) and 1970-01-01.
const EPOCH_GAP_SECS: u64 = 11_644_473_600;

/// A Windows FILETIME (100 ns ticks since 1601) as days since 1970-01-01,
/// in a u16 (good until the year 2149). 0 means unknown (or before 1970).
/// Two bytes per item: ~3.8 MB for a 1.9M-item drive.
pub fn filetime_days(ft: u64) -> u16 {
    let secs = ft / 10_000_000;
    if secs <= EPOCH_GAP_SECS {
        return 0;
    }
    u16::try_from((secs - EPOCH_GAP_SECS) / 86_400).unwrap_or(u16::MAX).max(1)
}

/// Today as days since 1970-01-01 (same scale as [`Index::mtime`]).
pub fn today_days() -> u16 {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    u16::try_from(secs / 86_400).unwrap_or(u16::MAX)
}

/// Age buckets for the age view: (at most this many days old, label).
pub const AGE_BUCKETS: [(u16, &str); 6] = [
    (31, "This month"),
    (182, "1-6 months"),
    (365, "6-12 months"),
    (730, "1-2 years"),
    (1826, "2-5 years"),
    (u16::MAX, "Older"),
];
/// Bucket index for items with no date.
pub const AGE_UNKNOWN: usize = AGE_BUCKETS.len();

/// Which [`AGE_BUCKETS`] entry a last-modified day falls in.
pub fn age_bucket(today: u16, mtime: u16) -> usize {
    if mtime == 0 {
        return AGE_UNKNOWN;
    }
    let d = today.saturating_sub(mtime);
    AGE_BUCKETS.iter().position(|b| d <= b.0).unwrap_or(AGE_BUCKETS.len() - 1)
}

/// "2024-01-31" for a day number (days since 1970-01-01); "unknown" for 0.
pub fn date_text(days: u16) -> String {
    if days == 0 {
        return "unknown".into();
    }
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = i64::from(days) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// "3 days ago", "5 months ago", "2 years ago".
pub fn age_text(today: u16, mtime: u16) -> String {
    if mtime == 0 {
        return "date unknown".into();
    }
    let d = today.saturating_sub(mtime);
    match d {
        0 => "today".into(),
        1 => "yesterday".into(),
        2..=44 => format!("{d} days ago"),
        45..=547 => format!("{} months ago", (d + 15) / 30),
        _ => format!("{:.1} years ago", f64::from(d) / 365.25),
    }
}

/// The scanned file tree.
///
/// Before [`Index::compute_totals`], `size`/`alloc` hold each node's own
/// bytes. Afterwards, folders hold the totals of their whole subtree.
#[derive(Clone, Debug)]
pub struct Index {
    parent: Vec<u32>,
    size: Vec<u64>,
    alloc: Vec<u64>,
    files: Vec<u32>,
    flags: Vec<u8>,
    name_off: Vec<u32>,
    name_len: Vec<u16>,
    names: String,
    /// Last-modified day; for folders (after totals) the newest inside.
    mtime: Vec<u16>,
    totals_ready: bool,
}

impl Index {
    /// Frees spare capacity left from growing the columns (the folder walk
    /// grows them as it goes; names alone could hold ~20 MB of slack on a
    /// full drive). One copy, a few ms.
    pub fn shrink_to_fit(&mut self) {
        self.parent.shrink_to_fit();
        self.size.shrink_to_fit();
        self.alloc.shrink_to_fit();
        self.files.shrink_to_fit();
        self.flags.shrink_to_fit();
        self.name_off.shrink_to_fit();
        self.name_len.shrink_to_fit();
        self.names.shrink_to_fit();
        self.mtime.shrink_to_fit();
    }

    /// Heap bytes held, per column (for memory work).
    pub fn heap_columns(&self) -> Vec<(&'static str, usize)> {
        vec![
            ("index.parent", self.parent.capacity() * 4),
            ("index.size", self.size.capacity() * 8),
            ("index.alloc", self.alloc.capacity() * 8),
            ("index.files", self.files.capacity() * 4),
            ("index.flags", self.flags.capacity()),
            ("index.name_off", self.name_off.capacity() * 4),
            ("index.name_len", self.name_len.capacity() * 2),
            ("index.names", self.names.capacity()),
            ("index.mtime", self.mtime.capacity() * 2),
        ]
    }

    /// Creates an index whose root node (id 0) is a folder named `root_name`.
    pub fn new(root_name: &str) -> Self {
        Self::with_capacity(root_name, 0, 0)
    }

    /// Like [`Index::new`], with room for `nodes` nodes and `name_bytes` of
    /// names reserved up front. Avoids re-copying while growing, and avoids
    /// up to 2x spare capacity.
    pub fn with_capacity(root_name: &str, nodes: usize, name_bytes: usize) -> Self {
        let mut idx = Self {
            parent: Vec::with_capacity(nodes),
            size: Vec::with_capacity(nodes),
            alloc: Vec::with_capacity(nodes),
            files: Vec::with_capacity(nodes),
            flags: Vec::with_capacity(nodes),
            name_off: Vec::with_capacity(nodes),
            name_len: Vec::with_capacity(nodes),
            names: String::with_capacity(name_bytes + root_name.len()),
            mtime: Vec::with_capacity(nodes),
            totals_ready: false,
        };
        idx.push_raw(NO_PARENT, root_name, 0, 0, flags::DIR);
        idx
    }

    /// Builds an index from ready-made columns (fast bulk path for
    /// scanners). Validates the invariants instead of trusting them: every
    /// non-root parent id is smaller than the node's id, and every name
    /// range is inside the arena on char boundaries.
    pub fn from_columns(c: Columns) -> Result<Self, String> {
        let n = c.parent.len();
        let lens = [c.size.len(), c.alloc.len(), c.flags.len(), c.name_off.len(), c.name_len.len()];
        if n == 0 || lens.iter().any(|&l| l != n) {
            return Err(format!("column lengths differ: {n} vs {lens:?}"));
        }
        if c.parent[0] != NO_PARENT || c.flags[0] & flags::DIR == 0 {
            return Err("node 0 must be a root folder".into());
        }
        u32::try_from(n).map_err(|_| "more than 4 billion nodes")?;
        for i in 1..n {
            if c.parent[i] as usize >= i {
                return Err(format!("node {i} has parent {} (must be smaller)", c.parent[i]));
            }
        }
        for i in 0..n {
            let (off, len) = (c.name_off[i] as usize, c.name_len[i] as usize);
            let ok = off + len <= c.names.len() && c.names.is_char_boundary(off) && c.names.is_char_boundary(off + len);
            if !ok {
                return Err(format!("node {i} has a bad name range"));
            }
        }
        let mtime = if c.mtime.is_empty() {
            vec![0; n]
        } else if c.mtime.len() == n {
            c.mtime
        } else {
            return Err(format!("mtime column has {} entries for {n} nodes", c.mtime.len()));
        };
        let files = c.flags.iter().map(|&f| u32::from(f & flags::DIR == 0)).collect();
        Ok(Self {
            parent: c.parent,
            size: c.size,
            alloc: c.alloc,
            files,
            flags: c.flags,
            name_off: c.name_off,
            name_len: c.name_len,
            names: c.names,
            mtime,
            totals_ready: false,
        })
    }

    /// Adds a child node and returns its id. `size` is the logical size,
    /// `alloc` the size on disk. Both should be 0 for folders.
    pub fn push(&mut self, parent: u32, name: &str, size: u64, alloc: u64, flags: u8) -> u32 {
        assert!((parent as usize) < self.len(), "parent must be added before child");
        assert!(!self.totals_ready, "cannot add nodes after compute_totals");
        self.push_raw(parent, name, size, alloc, flags)
    }

    fn push_raw(&mut self, parent: u32, name: &str, size: u64, alloc: u64, f: u8) -> u32 {
        let id = u32::try_from(self.parent.len()).expect("more than 4 billion nodes");
        let off = u32::try_from(self.names.len()).expect("name storage over 4 GiB");
        let len = u16::try_from(name.len()).expect("name over 64 KiB");
        self.names.push_str(name);
        self.parent.push(parent);
        self.size.push(size);
        self.alloc.push(alloc);
        self.files.push(u32::from(f & flags::DIR == 0));
        self.flags.push(f);
        self.name_off.push(off);
        self.name_len.push(len);
        self.mtime.push(0);
        id
    }

    /// Sets a node's last-modified day (see [`filetime_days`]).
    pub fn set_mtime(&mut self, id: u32, days: u16) {
        self.mtime[id as usize] = days;
    }

    /// Last-modified day (days since 1970; 0 = unknown). For folders, once
    /// totals are computed: the newest file or folder inside.
    pub fn mtime(&self, id: u32) -> u16 {
        self.mtime[id as usize]
    }

    /// Rolls file sizes and counts up into every folder. Runs once; later
    /// calls do nothing.
    pub fn compute_totals(&mut self) {
        if self.totals_ready {
            return;
        }
        // Saturating: sizes come from disk structures, and a corrupt record
        // claiming ~2^64 bytes must not wrap the totals (found by fuzzing).
        for i in (1..self.len()).rev() {
            let p = self.parent[i] as usize;
            self.size[p] = self.size[p].saturating_add(self.size[i]);
            self.alloc[p] = self.alloc[p].saturating_add(self.alloc[i]);
            self.files[p] = self.files[p].saturating_add(self.files[i]);
            self.mtime[p] = self.mtime[p].max(self.mtime[i]);
        }
        self.totals_ready = true;
    }

    pub fn totals_ready(&self) -> bool {
        self.totals_ready
    }

    pub fn len(&self) -> usize {
        self.parent.len()
    }

    /// Always false: the root node exists from construction.
    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn parent(&self, id: u32) -> u32 {
        self.parent[id as usize]
    }

    /// Logical size (subtree total for folders once totals are computed).
    pub fn size(&self, id: u32) -> u64 {
        self.size[id as usize]
    }

    /// Size on disk (subtree total for folders once totals are computed).
    pub fn alloc(&self, id: u32) -> u64 {
        self.alloc[id as usize]
    }

    /// Number of files in the subtree (1 for a file).
    pub fn files(&self, id: u32) -> u32 {
        self.files[id as usize]
    }

    pub fn flags(&self, id: u32) -> u8 {
        self.flags[id as usize]
    }

    pub fn add_flags(&mut self, id: u32, f: u8) {
        self.flags[id as usize] |= f;
    }

    pub fn is_dir(&self, id: u32) -> bool {
        self.flags(id) & flags::DIR != 0
    }

    pub fn name(&self, id: u32) -> &str {
        let off = self.name_off[id as usize] as usize;
        let len = self.name_len[id as usize] as usize;
        &self.names[off..off + len]
    }

    /// Full path of a node, built by walking up to the root.
    pub fn path(&self, id: u32) -> String {
        let mut chain = Vec::new();
        let mut cur = id;
        while cur != NO_PARENT {
            chain.push(cur);
            cur = self.parent(cur);
        }
        let mut out = String::new();
        for &n in chain.iter().rev() {
            if !out.is_empty() && !out.ends_with('\\') {
                out.push('\\');
            }
            out.push_str(self.name(n));
        }
        out
    }

    /// Approximate heap memory used by the index.
    pub fn heap_bytes(&self) -> usize {
        self.parent.capacity() * 4
            + self.size.capacity() * 8
            + self.alloc.capacity() * 8
            + self.files.capacity() * 4
            + self.flags.capacity()
            + self.name_off.capacity() * 4
            + self.name_len.capacity() * 2
            + self.names.capacity()
            + self.mtime.capacity() * 2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Index {
        let mut idx = Index::new("C:\\");
        let users = idx.push(0, "Users", 0, 0, flags::DIR);
        let me = idx.push(users, "me", 0, 0, flags::DIR);
        idx.push(me, "a.bin", 100, 4096, 0);
        idx.push(me, "b.bin", 50, 4096, 0);
        idx.push(0, "pagefile.sys", 1000, 1000, 0);
        idx
    }

    #[test]
    fn date_text_and_buckets() {
        assert_eq!(date_text(19_723), "2024-01-01");
        assert_eq!(date_text(0), "unknown");
        assert_eq!(date_text(19_783), "2024-03-01", "leap year February");
        assert_eq!(age_bucket(20_000, 20_000), 0);
        assert_eq!(age_bucket(20_000, 20_000 - 100), 1);
        assert_eq!(age_bucket(20_000, 1), AGE_BUCKETS.len() - 1);
        assert_eq!(age_bucket(20_000, 0), AGE_UNKNOWN);
        assert_eq!(age_bucket(20_000, 20_005), 0, "future dates count as new");
        assert_eq!(age_text(100, 99), "yesterday");
        assert_eq!(age_text(1000, 700), "10 months ago");
    }

    #[test]
    fn dates_convert_and_roll_up_to_the_newest() {
        // 2024-01-01 00:00 UTC = 19723 days after 1970-01-01.
        let ft = (19_723u64 * 86_400 + 11_644_473_600) * 10_000_000;
        assert_eq!(filetime_days(ft), 19_723);
        assert_eq!(filetime_days(0), 0, "unknown");
        assert_eq!(filetime_days(1), 0, "before 1970 counts as unknown");
        assert_eq!(filetime_days(u64::MAX), u16::MAX, "clamped, never wraps");
        assert!(today_days() > 20_000);
        let mut idx = sample();
        idx.set_mtime(3, 100);
        idx.set_mtime(4, 300);
        idx.set_mtime(5, 200);
        idx.compute_totals();
        assert_eq!((idx.mtime(2), idx.mtime(1), idx.mtime(0)), (300, 300, 300));
        assert_eq!(idx.mtime(3), 100, "files keep their own date");
    }

    #[test]
    fn totals_roll_up() {
        let mut idx = sample();
        idx.compute_totals();
        assert_eq!(idx.size(0), 1150);
        assert_eq!(idx.alloc(0), 4096 * 2 + 1000);
        assert_eq!(idx.files(0), 3);
        assert_eq!(idx.size(1), 150); // Users
        assert_eq!(idx.files(1), 2);
        idx.compute_totals(); // idempotent
        assert_eq!(idx.size(0), 1150);
    }

    #[test]
    fn paths_join_cleanly() {
        let idx = sample();
        assert_eq!(idx.path(0), "C:\\");
        assert_eq!(idx.path(3), "C:\\Users\\me\\a.bin");
        let mut folder = Index::new("D:\\data");
        let f = folder.push(0, "x.txt", 1, 1, 0);
        assert_eq!(folder.path(f), "D:\\data\\x.txt");
    }

    #[test]
    fn bulk_build_matches_push_and_validates() {
        let cols = || Columns {
            parent: vec![NO_PARENT, 0, 1],
            size: vec![0, 0, 100],
            alloc: vec![0, 0, 4096],
            flags: vec![flags::DIR, flags::DIR, 0],
            name_off: vec![0, 3, 8],
            name_len: vec![3, 5, 5],
            names: "C:\\Usersa.bin".into(),
            mtime: vec![],
        };
        let mut idx = Index::from_columns(cols()).unwrap();
        idx.compute_totals();
        assert_eq!(idx.path(2), "C:\\Users\\a.bin");
        assert_eq!((idx.size(0), idx.files(0)), (100, 1));

        let mut bad = cols();
        bad.parent[1] = 2; // parent after child
        assert!(Index::from_columns(bad).is_err());
        let mut bad = cols();
        bad.name_len[2] = 50; // name past the arena
        assert!(Index::from_columns(bad).is_err());
        let mut bad = cols();
        bad.size.pop();
        assert!(Index::from_columns(bad).is_err());
    }

    #[test]
    #[should_panic]
    fn rejects_unknown_parent() {
        let mut idx = Index::new("C:\\");
        idx.push(5, "x", 0, 0, 0);
    }
}
