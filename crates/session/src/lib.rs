//! UI-agnostic engine API used by the Xylem app (`xylem.exe`) and kept
//! separate from the UI so it can be tested on its own.
//!
//! A [`Scanned`] holds one scan: the tree, its classification and category
//! totals. Every UI question ("what's inside this folder", "draw this
//! folder", "is this safe to delete") is a method here, so the UI never
//! holds the full tree.

pub mod dupes;
pub mod elevate;
pub mod helper;
pub mod integrity;
pub mod lockdown;
pub mod recycle;
pub mod remember;
pub mod safety;

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use serde::Serialize;
use xylem_core::classify::{CATEGORIES, CatCrumb, CatKey, CatModel, CatStats, Classification, NCAT, Rules};
use xylem_core::view::{Children, ViewLimits, ViewNode, ancestors, build_view};
use xylem_core::{Index, NO_PARENT, ScanStats, flags};
use xylem_scan_mft::{Mode, Used, scan_auto};

pub use xylem_core::classify::{Category, RuleInfo, Safety, SubType};
pub use xylem_core::view::ViewNode as Shape;

/// The built-in classification rules (compiled in, not editable at runtime).
pub static RULES: LazyLock<Rules> = LazyLock::new(Rules::builtin);

pub fn categories() -> &'static [Category] {
    &CATEGORIES
}

/// Items found so far by a running folder-walk scan (0 when idle).
pub fn scan_progress() -> u64 {
    xylem_scan_walk::PROGRESS.load(std::sync::atomic::Ordering::Relaxed)
}

/// (total, free) bytes of the volume holding `path` (free = free for this user).
pub fn disk_space(path: &str) -> Option<(u64, u64)> {
    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetDiskFreeSpaceExW(dir: *const u16, free_to_caller: *mut u64, total: *mut u64, total_free: *mut u64) -> i32;
    }
    let mut root = path.to_owned();
    if !root.ends_with('\\') {
        root.push('\\');
    }
    let w: Vec<u16> = root.encode_utf16().chain([0]).collect();
    let (mut free, mut total, mut total_free) = (0u64, 0u64, 0u64);
    // SAFETY: NUL-terminated path and three valid out pointers.
    let ok = unsafe { GetDiskFreeSpaceExW(w.as_ptr(), &mut free, &mut total, &mut total_free) } != 0;
    (ok && total > 0).then_some((total, free))
}

/// Drive roots that exist, e.g. `["C:\\", "D:\\"]`.
pub fn drives() -> Vec<String> {
    // GetDriveTypeW answers from the local mount table, so network drives
    // are skipped without touching the network (no SMB traffic at startup).
    ('A'..='Z')
        .map(|c| format!("{c}:\\"))
        .filter(|d| matches!(drive_type(d), 2 | 3 | 5 | 6)) // removable, fixed, CD, RAM disk
        .filter(|d| Path::new(d).exists())
        .collect()
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetDriveTypeW(root: *const u16) -> u32;
}

/// 0 unknown, 1 no root, 2 removable, 3 fixed, 4 remote, 5 CD-ROM, 6 RAM disk.
pub fn drive_type(path: &str) -> u32 {
    let root: String = if path.starts_with(r"\\") { return 4 } else { path.chars().take(2).chain("\\".chars()).collect() };
    let w: Vec<u16> = root.encode_utf16().chain([0]).collect();
    // SAFETY: NUL-terminated buffer that outlives the call.
    unsafe { GetDriveTypeW(w.as_ptr()) }
}

fn mode_text(used: &Used) -> String {
    match used {
        Used::Mft => "MFT (direct read)".into(),
        Used::Walk { fallback_reason: None } => "folder walk".into(),
        Used::Walk { fallback_reason: Some(r) } => format!("folder walk ({r})"),
    }
}

/// Scans a drive or folder in this process (MFT if possible, else walk).
/// Blocking: call it from a worker thread.
pub fn scan(path: &str) -> Result<Scanned, String> {
    let path = real_path(path)?;
    let threads = xylem_scan_walk::default_threads();
    let (s, used) = scan_auto(Path::new(&path), Mode::Auto, threads).map_err(|e| format!("Scan failed: {e}"))?;
    Ok(Scanned::new(s.index, s.stats, mode_text(&used)))
}

/// The real, full path of what the user typed: short 8.3 names
/// (`C:\PROGRA~1`), `subst` drive letters, junctions and symlinks are
/// resolved. Protection rules match on folder names, so they must see the
/// real path, not whatever spelling was typed.
pub fn real_path(path: &str) -> Result<String, String> {
    let p = path.trim();
    // "C:" alone means "the current folder on C:", not the drive.
    let p = if p.len() == 2 && p.ends_with(':') { format!("{p}\\") } else { p.to_owned() };
    let c = std::fs::canonicalize(&p).map_err(|e| format!("Can't open \"{path}\": {e}"))?;
    let s = c.to_string_lossy();
    Ok(if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        s.into_owned()
    })
}

/// The remembered scan of `root` (instant reopen), and when it was made.
/// Blocking: call it from a worker thread.
pub fn reopen(root: &str) -> Option<(Scanned, u64)> {
    let (index, stats, mode, time) = remember::reopen(root)?;
    Some((Scanned::new(index, stats, mode), time))
}

/// Fast MFT scan through the elevated helper (shows the UAC prompt).
/// Blocking: call it from a worker thread.
pub fn scan_admin(drive: &str, cancel: &std::sync::atomic::AtomicBool) -> Result<Scanned, String> {
    // Fast scan runs this very exe with admin rights. A copy in a folder the
    // user can write to (Downloads, a portable zip) could be swapped by any
    // program running as the user, which would then get admin rights from
    // the user's "Yes". So release builds only elevate from Program Files.
    #[cfg(not(debug_assertions))]
    if !integrity::fast_scan_allowed() {
        return Err(integrity::FAST_SCAN_NEEDS_INSTALL.into());
    }
    let drive = real_path(drive)?;
    let (index, stats, mode) = elevate::scan_with_helper(&drive, cancel)?;
    Ok(Scanned::new(index, stats, mode))
}

#[derive(Clone, Debug, Serialize)]
pub struct Summary {
    pub root: String,
    pub mode: String,
    pub elapsed_ms: f64,
    pub classify_ms: f64,
    pub files: u64,
    pub dirs: u64,
    pub unreadable: u64,
    pub size: u64,
    pub on_disk: u64,
    /// Share of bytes in a real category (not Other).
    pub categorized: f64,
    pub admin_hint: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct Item {
    pub id: u32,
    pub name: String,
    pub size: u64,
    pub on_disk: u64,
    pub files: u32,
    pub dir: bool,
    pub link: bool,
    pub cloud: bool,
    /// Colour category (dominant inside folders), own sub-type and rule.
    pub cat: u8,
    pub own_cat: u8,
    pub sub: u16,
    pub rule: u16,
    pub protected: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ListOut {
    /// All children of the folder (the list may be cut at `limit`).
    pub total: usize,
    pub items: Vec<Item>,
    /// Bytes per category inside this folder, biggest first.
    pub breakdown: Vec<(u8, u64)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Crumb {
    pub id: u32,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CatViewOut {
    pub nodes: Vec<ViewNode>,
    pub crumbs: Vec<CatCrumb>,
}

/// One row of the by-category tree: a category, a sub-type, or a place
/// (a real folder/file holding bytes of that sub-type).
#[derive(Clone, Debug, Serialize)]
pub struct CatRow {
    /// "cat:3" / "sub:12" for groups; empty for places.
    pub key: String,
    /// Index node of a place.
    pub node: Option<u32>,
    pub name: String,
    pub bytes: u64,
    pub files: u64,
    pub cat: u8,
    pub sub: u16,
    pub rule: u16,
    pub has_children: bool,
}

/// Everything the details pane and delete dialog show.
#[derive(Clone, Debug, Serialize)]
pub struct ItemInfo {
    pub id: u32,
    pub name: String,
    pub path: String,
    pub dir: bool,
    pub on_disk: u64,
    pub size: u64,
    pub files: u32,
    pub cat: u8,
    pub sub: u16,
    pub rule: u16,
    pub protected: Option<String>,
    /// The folder to open for this item (itself, or its parent for files).
    pub folder: u32,
    pub parent: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeleteOut {
    pub freed: u64,
    pub name: String,
    pub cancelled: bool,
}

/// A row of the "largest files" list.
#[derive(Clone, Debug, Serialize)]
pub struct FileRow {
    pub id: u32,
    pub name: String,
    pub path: String,
    pub on_disk: u64,
    pub cat: u8,
}

/// A row of the "file types" list.
#[derive(Clone, Debug, Serialize)]
pub struct TypeRow {
    /// Lowercase extension without the dot ("" = no extension).
    pub ext: String,
    pub bytes: u64,
    pub files: u64,
}

/// One scan, classified, ready to answer UI questions.
pub struct Scanned {
    /// Shared so the "remember this scan" thread can save it while the UI
    /// uses it; a delete copies it only if that save is still running.
    index: Arc<Index>,
    children: Children,
    cls: Classification,
    cats: CatStats,
    stats: ScanStats,
    mode: String,
    classify_ms: f64,
    /// Folders deleted through Xylem (files come from the live totals).
    removed_dirs: u64,
}

impl Scanned {
    pub fn new(mut index: Index, stats: ScanStats, mode: String) -> Self {
        let t = Instant::now();
        index.shrink_to_fit();
        let children = Children::build(&index);
        let cls = Classification::run(&index, &RULES);
        let cats = CatStats::compute(&index, &cls, RULES.subs.len());
        let classify_ms = t.elapsed().as_secs_f64() * 1000.0;
        Self { index: Arc::new(index), children, cls, cats, stats, mode, classify_ms, removed_dirs: 0 }
    }

    fn model(&self) -> CatModel<'_> {
        CatModel { idx: &self.index, cls: &self.cls, stats: &self.cats, rules: &RULES }
    }

    /// Ok if `node` exists and wasn't deleted.
    pub fn check(&self, node: u32) -> Result<(), String> {
        if (node as usize) < self.index.len() && self.index.flags(node) & flags::DELETED == 0 {
            Ok(())
        } else {
            Err(format!("No such item: {node}"))
        }
    }

    /// Adds category data to view nodes of real items.
    fn decorate(&self, nodes: &mut [ViewNode]) {
        for n in nodes.iter_mut().filter(|n| n.id >= 0 && n.key.is_empty()) {
            let i = n.id as usize;
            if n.cat == u8::MAX {
                n.cat = self.cats.dominant[i];
            }
            (n.own_cat, n.sub, n.rule) = (self.cls.cat[i], self.cls.sub[i], self.cls.rule[i]);
        }
    }

    pub fn summary(&self) -> Summary {
        let total: u64 = self.cats.cat_total.iter().sum();
        let other = self.cats.cat_total[xylem_core::classify::CAT_OTHER as usize];
        Summary {
            root: self.index.name(0).to_owned(),
            mode: self.mode.clone(),
            elapsed_ms: self.stats.elapsed.as_secs_f64() * 1000.0,
            classify_ms: self.classify_ms,
            // Live counts, so they stay right after deleting things.
            files: u64::from(self.index.files(0)),
            dirs: self.stats.dirs.saturating_sub(self.removed_dirs),
            unreadable: self.stats.unreadable,
            size: self.index.size(0),
            on_disk: self.index.alloc(0),
            categorized: if total > 0 { (total - other) as f64 / total as f64 } else { 0.0 },
            admin_hint: self.mode.contains("administrator"),
        }
    }

    /// Pruned shapes for drawing folder `node` (sunburst/treemap).
    pub fn view(&self, node: u32, depth: u8) -> Result<Vec<ViewNode>, String> {
        self.check(node)?;
        let lim = ViewLimits { max_depth: depth.clamp(1, 10), ..ViewLimits::default() };
        let mut nodes = build_view(&self.index, &self.children, node, &lim);
        self.decorate(&mut nodes);
        Ok(nodes)
    }

    fn item(&self, c: u32) -> Item {
        let idx = &self.index;
        Item {
            id: c,
            name: idx.name(c).to_owned(),
            size: idx.size(c),
            on_disk: idx.alloc(c),
            files: idx.files(c),
            dir: idx.is_dir(c),
            link: idx.flags(c) & flags::LINK != 0,
            cloud: idx.flags(c) & flags::CLOUD != 0,
            cat: self.cats.dominant[c as usize],
            own_cat: self.cls.cat[c as usize],
            sub: self.cls.sub[c as usize],
            rule: self.cls.rule[c as usize],
            protected: self.cls.protection(idx, &RULES, c).is_some(),
        }
    }

    /// The node itself as a list row (for the tree root).
    pub fn item_of(&self, node: u32) -> Result<Item, String> {
        self.check(node)?;
        Ok(self.item(node))
    }

    /// Children of folder `node`, biggest first, plus its category breakdown.
    pub fn list(&self, node: u32, limit: usize) -> Result<ListOut, String> {
        self.check(node)?;
        let kids: Vec<u32> = self.children.live(&self.index, node).collect();
        let items = kids.iter().take(limit).map(|&c| self.item(c)).collect();
        Ok(ListOut { total: kids.len(), items, breakdown: self.breakdown(node) })
    }

    /// Bytes per category inside `node`, biggest first.
    pub fn breakdown(&self, node: u32) -> Vec<(u8, u64)> {
        let mut b: Vec<(u8, u64)> = self
            .cats
            .breakdown(node)
            .into_iter()
            .enumerate()
            .filter(|&(_, v)| v > 0)
            .map(|(c, v)| (c as u8, v))
            .collect();
        b.sort_unstable_by_key(|&(_, v)| std::cmp::Reverse(v));
        b
    }

    pub fn crumbs(&self, node: u32) -> Result<Vec<Crumb>, String> {
        self.check(node)?;
        Ok(ancestors(&self.index, node).into_iter().map(|(id, name)| Crumb { id, name }).collect())
    }

    /// The by-category diagram rooted at `key` ("", "cat:3", "sub:12").
    pub fn category_view(&self, key: &str) -> Result<CatViewOut, String> {
        let k = CatKey::parse(key, &RULES).ok_or("Unknown category")?;
        let m = self.model();
        let mut nodes = m.view(k, 0.002);
        self.decorate(&mut nodes);
        Ok(CatViewOut { nodes, crumbs: m.crumbs(k) })
    }

    /// Bytes and files of a category key ("" = everything).
    pub fn cat_total(&self, key: &str) -> (u64, u64) {
        match CatKey::parse(key, &RULES) {
            Some(CatKey::Cat(c)) => (self.cats.cat_total[c as usize], self.cats.cat_files[c as usize]),
            Some(CatKey::Sub(s)) => (self.cats.sub_total[s as usize], self.cats.sub_files[s as usize]),
            _ => (self.cats.cat_total.iter().sum(), self.cats.cat_files.iter().sum()),
        }
    }

    /// Rows under a by-category key: "" -> categories, "cat:N" -> sub-types,
    /// "sub:N" -> places (all of them, biggest first).
    pub fn cat_children(&self, key: &str, limit: usize) -> Result<Vec<CatRow>, String> {
        let k = CatKey::parse(key, &RULES).ok_or("Unknown category")?;
        let st = &self.cats;
        let rows = match k {
            CatKey::All => {
                let mut cats: Vec<u8> = (0..NCAT as u8).filter(|&c| st.cat_total[c as usize] > 0).collect();
                cats.sort_unstable_by_key(|&c| std::cmp::Reverse(st.cat_total[c as usize]));
                cats.into_iter()
                    .map(|c| CatRow {
                        key: CatKey::Cat(c).key(),
                        node: None,
                        name: CATEGORIES[c as usize].name.to_owned(),
                        bytes: st.cat_total[c as usize],
                        files: st.cat_files[c as usize],
                        cat: c,
                        sub: u16::MAX,
                        rule: u16::MAX,
                        has_children: true,
                    })
                    .collect()
            }
            CatKey::Cat(c) => {
                let mut subs: Vec<u16> = (0..RULES.subs.len() as u16)
                    .filter(|&s| RULES.sub_cat(s) == c && st.sub_total[s as usize] > 0)
                    .collect();
                subs.sort_unstable_by_key(|&s| std::cmp::Reverse(st.sub_total[s as usize]));
                subs.into_iter()
                    .map(|s| CatRow {
                        key: CatKey::Sub(s).key(),
                        node: None,
                        name: RULES.subs[s as usize].name.clone(),
                        bytes: st.sub_total[s as usize],
                        files: st.sub_files[s as usize],
                        cat: c,
                        sub: s,
                        rule: u16::MAX,
                        has_children: true,
                    })
                    .collect()
            }
            CatKey::Sub(s) => st.places_by_sub[s as usize]
                .iter()
                .filter(|p| self.index.flags(p.node) & flags::DELETED == 0)
                .take(limit)
                .map(|p| CatRow {
                    key: String::new(),
                    node: Some(p.node),
                    name: self.index.name(p.node).to_owned(),
                    bytes: p.bytes,
                    files: u64::from(p.files),
                    cat: RULES.sub_cat(s),
                    sub: s,
                    rule: self.cls.rule[p.node as usize],
                    has_children: false,
                })
                .collect(),
        };
        Ok(rows)
    }

    pub fn info(&self, node: u32) -> Result<ItemInfo, String> {
        self.check(node)?;
        let idx = &self.index;
        let i = node as usize;
        let parent = idx.parent(node);
        Ok(ItemInfo {
            id: node,
            name: idx.name(node).to_owned(),
            path: idx.path(node),
            dir: idx.is_dir(node),
            on_disk: idx.alloc(node),
            size: idx.size(node),
            files: idx.files(node),
            cat: self.cls.cat[i],
            sub: self.cls.sub[i],
            rule: self.cls.rule[i],
            protected: self.cls.protection(idx, &RULES, node),
            folder: if idx.is_dir(node) || parent == NO_PARENT { node } else { parent },
            parent: (parent != NO_PARENT).then_some(parent),
        })
    }

    /// Step 1 of a delete: everything needed to recycle `node`, or why not.
    /// Protection is enforced here, not just in the UI. Also collects the
    /// warnings the confirmation dialog must show (cloud, Git, large).
    pub fn prepare_delete(&self, node: u32) -> Result<PendingDelete, String> {
        let info = self.info(node)?;
        if let Some(why) = info.protected {
            return Err(why);
        }
        if node == 0 {
            return Err("Xylem won't delete the whole scanned folder or drive.".into());
        }
        let idx = &self.index;
        let path = PathBuf::from(&info.path);
        if let Some(why) = safety::unusual_name(&path) {
            return Err(format!("Xylem won't delete this: {why} Use File Explorer instead."));
        }
        let mut warnings = safety::DeleteWarnings {
            cloud: idx.flags(node) & flags::CLOUD != 0 || safety::in_cloud_folder(&path),
            git_repos: 0,
            large: info.on_disk >= safety::LARGE_BYTES || info.files >= safety::LARGE_FILES,
            no_recycle_bin: matches!(drive_type(&info.path), 2 | 4),
        };
        // One pass over the subtree: protected places inside (refuse), Git
        // repos and cloud placeholders (warn).
        let mut stack = vec![node];
        while let Some(x) = stack.pop() {
            if x != node
                && let Some(what) = self.cls.protected_here(&RULES, x)
            {
                return Err(format!("It contains {}, which Xylem protects ({what}).", idx.path(x)));
            }
            if x != node && idx.name(x).eq_ignore_ascii_case(".git") {
                warnings.git_repos += 1;
            }
            if idx.flags(x) & flags::CLOUD != 0 {
                warnings.cloud = true;
            }
            if idx.is_dir(x) {
                stack.extend(self.children.live(idx, x));
            }
        }
        if idx.name(node).eq_ignore_ascii_case(".git") {
            warnings.git_repos += 1;
        }
        Ok(PendingDelete {
            node,
            path,
            name: info.name,
            freed: info.on_disk,
            files: info.files,
            dir: info.dir,
            was_link: idx.flags(node) & flags::LINK != 0,
            scan_root: PathBuf::from(idx.name(0)),
            category: CATEGORIES[self.cls.cat[node as usize] as usize].name.to_owned(),
            warnings,
            identity: None,
        })
    }

    /// Step 3 of a delete: after the Recycle Bin move succeeded, drop the
    /// item from the tree and refresh category totals.
    pub fn apply_delete(&mut self, node: u32) {
        let mut stack = vec![node];
        while let Some(x) = stack.pop() {
            if self.index.is_dir(x) && self.index.flags(x) & flags::DELETED == 0 {
                self.removed_dirs += 1;
            }
            stack.extend(self.children.live(&self.index, x));
        }
        Arc::make_mut(&mut self.index).remove_subtree(node, &self.children);
        self.cats = CatStats::compute(&self.index, &self.cls, RULES.subs.len());
    }

    /// The `limit` largest files (size on disk), optionally only those whose
    /// name contains `query` (case-insensitive). One pass with a small heap.
    pub fn top_files(&self, limit: usize, query: &str) -> Vec<FileRow> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;
        let q = query.trim().to_lowercase();
        let idx = &self.index;
        let mut heap: BinaryHeap<Reverse<(u64, u32)>> = BinaryHeap::with_capacity(limit + 1);
        for id in 1..idx.len() as u32 {
            if idx.is_dir(id) || idx.flags(id) & flags::DELETED != 0 {
                continue;
            }
            let a = idx.alloc(id);
            if heap.len() == limit && heap.peek().is_some_and(|Reverse((m, _))| a <= *m) {
                continue;
            }
            if !q.is_empty() && !idx.name(id).to_lowercase().contains(&q) {
                continue;
            }
            heap.push(Reverse((a, id)));
            if heap.len() > limit {
                heap.pop();
            }
        }
        let mut v: Vec<(u64, u32)> = heap.into_iter().map(|Reverse(x)| x).collect();
        v.sort_unstable_by_key(|&(a, id)| (Reverse(a), id));
        v.into_iter()
            .map(|(a, id)| FileRow { id, name: idx.name(id).to_owned(), path: idx.path(id), on_disk: a, cat: self.cls.cat[id as usize] })
            .collect()
    }

    /// Files that could be duplicates: same size (≥ `min_size`), at least
    /// two of them, not cloud placeholders. From the scan only: no file is
    /// opened here. Biggest sizes first.
    pub fn duplicate_candidates(&self, min_size: u64) -> Vec<(u64, Vec<(u32, String)>)> {
        let idx = &self.index;
        let mut by_size: std::collections::HashMap<u64, Vec<u32>> = std::collections::HashMap::new();
        for id in 1..idx.len() as u32 {
            let f = idx.flags(id);
            if idx.is_dir(id) || f & (flags::DELETED | flags::CLOUD | flags::LINK) != 0 || idx.size(id) < min_size {
                continue;
            }
            by_size.entry(idx.size(id)).or_default().push(id);
        }
        let mut v: Vec<(u64, Vec<(u32, String)>)> = by_size
            .into_iter()
            .filter(|(_, ids)| ids.len() > 1)
            .map(|(size, ids)| (size, ids.into_iter().map(|id| (id, idx.path(id))).collect()))
            .collect();
        v.sort_unstable_by_key(|g| std::cmp::Reverse(g.0));
        v
    }

    /// Bytes and file counts per file extension (lowercase, "" = none),
    /// biggest first.
    pub fn file_types(&self, limit: usize) -> Vec<TypeRow> {
        let idx = &self.index;
        let mut m: std::collections::HashMap<String, (u64, u64)> = std::collections::HashMap::new();
        for id in 1..idx.len() as u32 {
            if idx.is_dir(id) || idx.flags(id) & flags::DELETED != 0 {
                continue;
            }
            let name = idx.name(id);
            let ext = match name.rfind('.') {
                Some(i) if i > 0 && name.len() - i <= 12 => &name[i + 1..],
                _ => "",
            };
            let key = if ext.bytes().any(|b| b.is_ascii_uppercase()) { ext.to_ascii_lowercase() } else { ext.to_owned() };
            let e = m.entry(key).or_insert((0, 0));
            e.0 = e.0.saturating_add(idx.alloc(id));
            e.1 += 1;
        }
        let mut v: Vec<TypeRow> = m.into_iter().map(|(ext, (bytes, files))| TypeRow { ext, bytes, files }).collect();
        v.sort_unstable_by_key(|r| std::cmp::Reverse(r.bytes));
        v.truncate(limit);
        v
    }

    /// Writes the whole scan as CSV to `path` (see `report::write_csv`).
    /// Blocking: use a worker thread (via `remember_parts`' index).
    pub fn export_csv(index: &Index, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let mut w = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
        xylem_core::report::write_csv(index, &mut w)?;
        w.flush()
    }

    /// Heap bytes per data structure (for memory work).
    pub fn memory_report(&self) -> Vec<(&'static str, usize)> {
        let mut v = self.index.heap_columns();
        v.push(("children", self.children.heap_bytes()));
        v.extend(self.cls.heap_bytes());
        v.extend(self.cats.heap_bytes());
        v
    }

    /// What the background "remember" step needs: the shared index, the
    /// scan stats and the mode label.
    pub fn remember_parts(&self) -> (Arc<Index>, ScanStats, String) {
        (Arc::clone(&self.index), self.stats.clone(), self.mode.clone())
    }

    /// Node of a folder given its path relative to the scan root
    /// ("Users\me"), case-insensitive. Used to jump from "what changed".
    pub fn find_rel(&self, rel: &str) -> Option<u32> {
        let mut cur = 0u32;
        for comp in rel.split('\\').filter(|c| !c.is_empty()) {
            cur = self.children.live(&self.index, cur).find(|&c| self.index.name(c).eq_ignore_ascii_case(comp))?;
        }
        Some(cur)
    }

    /// Full path of a node (for Explorer, copying, etc.).
    pub fn path(&self, node: u32) -> Result<String, String> {
        self.check(node)?;
        Ok(self.index.path(node))
    }
}

/// A delete that passed the protection check (see [`Scanned::prepare_delete`]).
pub struct PendingDelete {
    pub node: u32,
    pub path: PathBuf,
    pub name: String,
    pub freed: u64,
    pub files: u32,
    pub dir: bool,
    pub was_link: bool,
    pub scan_root: PathBuf,
    pub category: String,
    pub warnings: safety::DeleteWarnings,
    /// File identities of the item and every folder up to the scan root,
    /// taken when the user was asked (see [`PendingDelete::arm`]).
    identity: Option<Vec<(PathBuf, safety::FileId)>>,
}

impl PendingDelete {
    /// Records the on-disk identity of the item and of every folder above
    /// it, up to and including the scan root. Call it when the confirmation
    /// dialog opens; `recycle` then refuses if anything was swapped in the
    /// meantime.
    pub fn arm(&mut self) -> Result<(), String> {
        if let Some(why) = safety::hard_lock(&self.path) {
            return Err(why);
        }
        self.identity = Some(safety::identity_chain(&self.path, &self.scan_root)?);
        Ok(())
    }

    /// Step 2: re-check the item on disk, move it to the Recycle Bin, and
    /// write the result to the local deletion log. Blocking (Windows may
    /// show its own progress or "too big to recycle" prompt): use a worker
    /// thread.
    pub fn recycle(&self) -> Result<DeleteOut, String> {
        let identity = match &self.identity {
            Some(chain) => Ok(chain.clone()),
            None => safety::identity_chain(&self.path, &self.scan_root),
        };
        // The folders above the item stay pinned (can't be renamed or
        // swapped) from the identity check until the move is done.
        let result = identity.and_then(|chain| safety::pin_identity(&chain)).and_then(|pinned| {
            let r = safety::hard_lock(&self.path)
                .map_or(Ok(()), Err)
                .and_then(|()| safety::verify_before_recycle(&self.path, &self.scan_root, self.dir, self.was_link))
                .and_then(|()| recycle::to_recycle_bin(&self.path));
            drop(pinned);
            r
        });
        let cancelled = matches!(result, Ok(recycle::Outcome::Cancelled));
        safety::append_log(&safety::LogEntry {
            time: safety::utc_now(),
            path: self.path.display().to_string(),
            bytes: self.freed,
            files: self.files,
            category: self.category.clone(),
            outcome: match &result {
                Ok(recycle::Outcome::Recycled) => "recycled".into(),
                Ok(recycle::Outcome::Cancelled) => "cancelled".into(),
                Err(e) => format!("refused: {e}"),
            },
        });
        result?;
        Ok(DeleteOut { freed: if cancelled { 0 } else { self.freed }, name: self.name.clone(), cancelled })
    }
}

/// Shows `path` selected in Windows Explorer.
pub fn reveal(path: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    if path.contains('"') {
        return Err("Path contains a quote character".into());
    }
    // Absolute path: a bare "explorer.exe" is searched for in this exe's own
    // folder first, where a planted copy would run instead.
    std::process::Command::new(integrity::windows_dir().join("explorer.exe"))
        .raw_arg(format!("/select,\"{path}\""))
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open Explorer: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo() -> Scanned {
        let mut idx = Index::new("C:\\");
        let users = idx.push(0, "Users", 0, 0, flags::DIR);
        let me = idx.push(users, "me", 0, 0, flags::DIR);
        let proj = idx.push(me, "proj", 0, 0, flags::DIR);
        let nm = idx.push(proj, "node_modules", 0, 0, flags::DIR);
        idx.push(nm, "a.js", 4096, 4096, 0);
        idx.push(me, "movie.mp4", 9000, 9000, 0);
        let win = idx.push(0, "Windows", 0, 0, flags::DIR);
        idx.push(win, "x.dll", 100, 100, 0);
        idx.compute_totals();
        Scanned::new(idx, ScanStats::default(), "test".into())
    }

    #[test]
    fn answers_ui_questions() {
        let s = demo();
        let l = s.list(0, 10).unwrap();
        assert_eq!(l.total, 2);
        assert_eq!(l.items[0].name, "Users");
        assert!(s.list(999, 10).is_err());
        let crumbs = s.crumbs(4).unwrap();
        assert_eq!(crumbs.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["C:\\", "Users", "me", "proj", "node_modules"]);
        assert!(s.view(0, 3).unwrap().len() > 3);
        assert!(s.info(7).unwrap().protected.is_some(), "Windows is protected");
        assert!(s.prepare_delete(7).is_err());
        assert!(s.prepare_delete(4).is_ok(), "node_modules can be deleted");
        assert!(s.prepare_delete(0).is_err(), "never the scan root");
    }

    #[test]
    fn category_rows_drill_down() {
        let s = demo();
        let cats = s.cat_children("", 100).unwrap();
        assert!(cats.iter().all(|r| r.key.starts_with("cat:") && r.has_children));
        let total: u64 = cats.iter().map(|r| r.bytes).sum();
        assert_eq!(total, s.summary().on_disk);
        let dev = cats.iter().find(|r| r.name == "Dev stuff").unwrap();
        let subs = s.cat_children(&dev.key, 100).unwrap();
        let places = s.cat_children(&subs[0].key, 100).unwrap();
        assert_eq!(places[0].name, "node_modules");
        assert_eq!(places[0].node, Some(4));
        assert!(s.cat_children("bogus", 1).is_err());
    }

    /// Real disk, read-only: a scan that starts inside Windows must keep
    /// everything in it locked (it once didn't: only the root folder's own
    /// name was checked against the rules, not its parents).
    #[test]
    fn scanning_inside_windows_keeps_it_locked() {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        for sub in ["Fonts", "System32\\drivers"] {
            let s = scan(&format!("{windir}\\{sub}")).unwrap();
            let items = s.list(0, 50).unwrap().items;
            assert!(!items.is_empty(), "{sub} has items");
            for it in items {
                assert!(s.prepare_delete(it.id).is_err(), "{sub}\\{} must be locked", it.name);
            }
        }
    }

    #[test]
    fn refuses_folders_that_contain_protected_places() {
        // Deleting "Users" from a scan of C:\ would take profile folders and
        // anything protected inside with it.
        let s = demo();
        assert!(s.prepare_delete(1).is_err() || s.info(1).unwrap().protected.is_some());
        let mut idx = Index::new("C:\\");
        let stuff = idx.push(0, "stuff", 0, 0, flags::DIR);
        let win = idx.push(stuff, "Windows", 0, 0, flags::DIR);
        idx.push(win, "x.dll", 1, 1, 0);
        idx.compute_totals();
        let s = Scanned::new(idx, ScanStats::default(), "test".into());
        // C:\stuff\Windows isn't the real Windows folder, so nothing is locked.
        assert!(s.prepare_delete(stuff).is_ok());
    }

    #[test]
    fn real_path_resolves_short_names() {
        let windir = integrity::windows_dir();
        let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into());
        // C:\PROGRA~1 (8.3 name), if 8.3 names exist on this volume.
        let short = format!("{}PROGRA~1", &pf[..3]);
        if std::path::Path::new(&short).exists() {
            assert_eq!(real_path(&short).unwrap().to_lowercase(), pf.to_lowercase());
        }
        assert_eq!(real_path(&windir.to_string_lossy().to_uppercase()).unwrap().to_lowercase(), windir.to_string_lossy().to_lowercase());
        assert!(real_path("C:").unwrap().ends_with('\\'), "drive letter means the drive root");
        assert!(real_path("Z:\\no\\such\\folder\\xylem").is_err());
    }

    #[test]
    fn delete_refuses_swapped_items_and_odd_names() {
        let root = std::env::temp_dir().join(format!("xylem-arm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a")).unwrap();
        std::fs::write(root.join("a").join("f.txt"), b"x").unwrap();
        let s = scan(&root.to_string_lossy()).unwrap();
        let a = s.find_rel("a").unwrap();
        let mut pending = s.prepare_delete(a).unwrap();
        pending.arm().unwrap();
        // Swap folder "a" for another folder with the same name after the
        // user was asked: the delete must be refused, nothing recycled.
        std::fs::rename(root.join("a"), root.join("a-old")).unwrap();
        std::fs::create_dir_all(root.join("a")).unwrap();
        let err = pending.recycle().unwrap_err();
        assert!(err.contains("replaced or moved"), "{err}");
        assert!(root.join("a").exists() && root.join("a-old").exists());

        assert!(safety::unusual_name(std::path::Path::new("C:\\x\\report.")).is_some());
        assert!(safety::unusual_name(std::path::Path::new("C:\\x\\notes \\a")).is_some());
        assert!(safety::unusual_name(std::path::Path::new("C:\\x\\con.txt")).is_some());
        assert!(safety::unusual_name(std::path::Path::new("C:\\x\\console.txt")).is_none());
        assert!(safety::unusual_name(std::path::Path::new(&format!("C:\\{}", "a".repeat(300)))).is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `cargo test --release -p xylem-session memory_of_a_real_scan -- --ignored --nocapture`
    #[test]
    #[ignore = "scans all of C: (slow); run by hand for memory work"]
    fn memory_of_a_real_scan() {
        let s = scan("C:\\").unwrap();
        let rep = s.memory_report();
        let n = s.summary().files + s.summary().dirs;
        let total: usize = rep.iter().map(|r| r.1).sum();
        for (name, b) in &rep {
            println!("{name:<16} {:>7.1} MB  {:>5.1} B/node", *b as f64 / 1048576.0, *b as f64 / n as f64);
        }
        println!("{:<16} {:>7.1} MB  {:>5.1} B/node  ({n} nodes)", "TOTAL", total as f64 / 1048576.0, total as f64 / n as f64);
    }

    /// DEL-06/07 on real files: decoys Windows would silently rewrite
    /// ("report." → "report") must be refused, never hitting the real file.
    #[test]
    fn decoy_names_are_refused_on_real_files() {
        let dir = std::env::temp_dir().join(format!("xylem-decoys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let verbatim = |name: &str| PathBuf::from(format!(r"\\?\{}\{name}", dir.display()));
        std::fs::write(dir.join("report"), b"the real one").unwrap();
        for decoy in ["report.", "report ", "CON.txt", "nul"] {
            std::fs::write(verbatim(decoy), b"decoy").unwrap();
        }
        let s = scan(&dir.to_string_lossy()).unwrap();
        for decoy in ["report.", "report ", "CON.txt", "nul"] {
            let n = s.find_rel(decoy).unwrap_or_else(|| panic!("scan found {decoy:?}"));
            assert!(s.prepare_delete(n).is_err(), "{decoy:?} must be refused");
        }
        let real = s.find_rel("report").unwrap();
        assert!(s.prepare_delete(real).is_ok());
        assert_eq!(std::fs::read(dir.join("report")).unwrap(), b"the real one");
        for decoy in ["report.", "report ", "CON.txt", "nul"] {
            let _ = std::fs::remove_file(verbatim(decoy));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn largest_files_and_types() {
        let s = demo();
        let top: Vec<String> = s.top_files(2, "").into_iter().map(|r| r.name).collect();
        assert_eq!(top, ["movie.mp4", "a.js"]);
        let js = s.top_files(10, "JS");
        assert_eq!(js.len(), 1, "case-insensitive name search");
        assert!(js[0].path.ends_with(r"node_modules\a.js"));
        let types = s.file_types(10);
        assert_eq!((types[0].ext.as_str(), types[0].bytes, types[0].files), ("mp4", 9000, 1));
        assert_eq!(types.len(), 3);
    }

    #[test]
    fn delete_warnings_find_git_and_cloud() {
        let mut idx = Index::new("D:\\");
        let code = idx.push(0, "code", 0, 0, flags::DIR);
        let repo = idx.push(code, "app", 0, 0, flags::DIR);
        let git = idx.push(repo, ".git", 0, 0, flags::DIR);
        idx.push(git, "HEAD", 10, 4096, 0);
        let sync = idx.push(0, "synced", 0, 0, flags::DIR);
        idx.push(sync, "photo.jpg", 5000, 0, flags::CLOUD);
        idx.compute_totals();
        let s = Scanned::new(idx, ScanStats::default(), "test".into());
        let w = s.prepare_delete(code).unwrap().warnings;
        assert_eq!((w.git_repos, w.cloud, w.large), (1, false, false));
        assert!(w.needs_ack());
        let w = s.prepare_delete(sync).unwrap().warnings;
        assert_eq!((w.git_repos, w.cloud), (0, true));
        assert!(!s.prepare_delete(git).unwrap().warnings.cloud);
        assert_eq!(s.prepare_delete(git).unwrap().warnings.git_repos, 1);
    }

    #[test]
    fn apply_delete_updates_everything() {
        let mut s = demo();
        let before = s.summary();
        s.apply_delete(4);
        let after = s.summary();
        assert_eq!(before.on_disk - after.on_disk, 4096);
        assert_eq!(after.files, before.files - 1);
        assert!(s.check(4).is_err());
        assert!(s.cat_children("", 100).unwrap().iter().all(|r| r.name != "Dev stuff"));
    }
}
