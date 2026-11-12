//! View > Similar folders: folders and projects that look like copies of
//! each other, or that share a name.
//!
//! Works on the scan in memory only (no file is opened), read-only:
//!
//! - **Likely copies:** same size, same number of files, and the same names
//!   and sizes directly inside. Only the top-most copies are listed (the
//!   folders inside two copies are copies too). Contents are not compared.
//! - **Similar names:** names that reduce to the same family once case,
//!   separators, copy markers ("- Copy", "(2)", "backup"), versions ("v3")
//!   and numbers are ignored, e.g. `APALA`, `APALA_dev`, `APALA_v3`,
//!   `apala-release`. Names that occur everywhere (`src`, `data`, `bin`…)
//!   are ignored, so the groups stay meaningful.
//!
//! Places that are full of repeats by design are not looked into: Windows,
//! protected places, AppData and ProgramData, package and build caches
//! (`node_modules`, `.git`, `site-packages`, `__pycache__`, `.venv`…).

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use serde::Serialize;
use diskroot_core::view::Children;
use diskroot_core::{Index, NO_PARENT, flags};

#[derive(Clone, Debug, Serialize)]
pub struct Member {
    pub node: u32,
    pub path: String,
    /// Size on disk.
    pub bytes: u64,
    pub files: u32,
    /// Newest last-modified day inside (days since 1970; 0 = unknown).
    pub mtime: u16,
}

#[derive(Clone, Debug, Serialize)]
pub struct Group {
    /// The shared name (copies: the folder name; names: the family).
    pub label: String,
    /// Biggest first.
    pub members: Vec<Member>,
    pub bytes: u64,
    /// Copies only: space freed if all but one were removed.
    pub reclaim: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Found {
    pub copies: Vec<Group>,
    pub names: Vec<Group>,
    /// Folders looked at.
    pub folders: u64,
    pub secs: f64,
}

/// At most this many groups of each kind are listed (biggest first).
const MAX_GROUPS: usize = 200;
/// A name family seen in more folders than this is too generic to mean anything.
const MAX_FAMILY: usize = 40;
/// Likely copies must hold at least this much and 2+ files (tiny matches are noise).
const MIN_COPY_BYTES: u64 = 1 << 20;
/// Similar-name groups: each member at least this big, the group at least 1 MB.
const MIN_MEMBER_BYTES: u64 = 16 << 10;

/// Folders not looked into: repeats there are normal.
fn skip_inside(name: &str) -> bool {
    const SKIP: &[&str] = &[
        "windows", "$recycle.bin", "system volume information", "appdata", "programdata", "node_modules", ".git", ".svn", ".hg",
        "__pycache__", "site-packages", "dist-packages", ".venv", "venv", ".tox", ".cache", ".gradle", ".m2", ".cargo", ".rustup",
        ".npm", ".nuget", "bower_components", ".idea", ".vs", ".pytest_cache", ".mypy_cache", "winsxs",
    ];
    let n = name.to_ascii_lowercase();
    SKIP.contains(&n.as_str())
}

/// Protected places that belong to Windows or installed programs.
fn is_system(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(n.as_str(), "program files" | "program files (x86)" | "recovery" | "boot" | "$windows.~bt" | "$windows.~ws" | "windows.old" | "$winreagent" | "perflogs" | "msocache")
}

/// Words that say nothing about which project a folder belongs to.
fn generic(word: &str) -> bool {
    const WORDS: &[&str] = &[
        "data", "src", "source", "sources", "test", "tests", "doc", "docs", "bin", "lib", "libs", "build", "out", "output", "dist", "assets",
        "images", "img", "res", "resources", "config", "configs", "scripts", "tools", "utils", "util", "common", "shared", "include", "temp",
        "tmp", "cache", "logs", "log", "files", "file", "documents", "downloads", "desktop", "music", "videos", "pictures", "program",
        "programs", "microsoft", "windows", "app", "apps", "default", "debug", "release", "main", "master", "packages", "package",
        "modules", "module", "core", "api", "web", "public", "static", "new", "old", "my", "the", "and", "for", "folder", "project",
        "projects", "misc", "other", "stuff", "work", "backup", "backups", "archive", "examples", "example", "samples", "sample",
        "models", "model", "results", "runs", "notebooks", "plugins", "plugin", "extensions", "themes", "fonts", "icons", "locales",
        "lang", "languages", "x64", "x86", "amd64", "arm64", "win", "win32", "win64", "setup", "install", "installer", "update",
        "updates", "user", "users", "public", "local", "system", "common files", "steam", "steamapps", "raw", "train", "val",
        "valid", "validation", "deps", "processed", "interim", "external", "checkpoints", "weights", "outputs", "figures", "plots",
        "target", "release", "debug", "incremental", "fingerprint", "obj", "node", "env", "envs",
    ];
    WORDS.contains(&word)
}

/// Lowercase alphanumeric tokens: split at separators, camelCase and
/// letter/digit boundaries. "NeuroPulse_v3 (2)" → ["neuro", "pulse", "v", "3", "2"].
fn tokens(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev: Option<char> = None;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev = None;
            continue;
        }
        let boundary = match prev {
            Some(p) => (p.is_lowercase() && c.is_uppercase()) || (p.is_alphabetic() != c.is_alphabetic()),
            None => false,
        };
        if boundary && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        cur.extend(c.to_lowercase());
        prev = Some(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// (full key, family key) of a folder name, or None if it says nothing.
/// Full key: every meaningful token joined ("neuro_pulse" = "NeuroPulse (1)").
/// Family: the first meaningful token ("apala" for APALA_dev, APALA_v3…).
fn keys(name: &str) -> Option<(String, String)> {
    let t = tokens(name);
    let noise = |w: &str| {
        w.chars().all(|c| c.is_ascii_digit())
            || matches!(w, "copy" | "of" | "v" | "ver" | "version" | "bak" | "backup" | "old" | "new" | "final" | "orig" | "original" | "tmp" | "temp")
    };
    let kept: Vec<&str> = t.iter().map(String::as_str).filter(|w| !noise(w)).collect();
    let first = *kept.first()?;
    let full: String = kept.concat();
    let had_number = t.iter().any(|w| w.chars().all(|c| c.is_ascii_digit()));
    // "22AUG", "v2_2023": the number is what tells them apart.
    if full.chars().count() < 3 || generic(&full) || (had_number && full.chars().count() <= 3) {
        return None;
    }
    let family = if first.chars().count() >= 3 && !generic(first) { first.to_owned() } else { full.clone() };
    Some((full, family))
}

fn fnv(h: u64, bytes: &[u8]) -> u64 {
    bytes.iter().fold(h, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

struct Uf(Vec<u32>);
impl Uf {
    fn find(&mut self, mut x: u32) -> u32 {
        while self.0[x as usize] != x {
            let p = self.0[x as usize];
            self.0[x as usize] = self.0[p as usize];
            x = p;
        }
        x
    }
    fn join(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb) as usize] = ra.min(rb);
        }
    }
}

/// Finds likely copies and similar names. `protected(node)` says whether a
/// protected place starts at that folder (not looked into).
pub fn find(idx: &Index, ch: &Children, protected: impl Fn(u32) -> bool) -> Found {
    let t0 = Instant::now();
    // 1. The folders worth looking at.
    let mut dirs: Vec<u32> = Vec::new();
    let mut stack = vec![0u32];
    while let Some(x) = stack.pop() {
        for c in ch.live(idx, x) {
            // Protected places (like the user folder) are only protected from
            // deleting; this view only reads, so it looks inside them, except
            // where repeats are normal (skip list) and Windows' own protected places.
            if !idx.is_dir(c) || idx.flags(c) & (flags::LINK | flags::DELETED) != 0 || skip_inside(idx.name(c)) || (protected(c) && is_system(idx.name(c))) {
                continue;
            }
            // Build output marked as a cache (Rust's target\, others): repeats by design.
            if ch.live(idx, c).any(|k| idx.name(k).eq_ignore_ascii_case("CACHEDIR.TAG")) {
                continue;
            }
            dirs.push(c);
            stack.push(c);
        }
    }
    let mut found = Found { folders: dirs.len() as u64, ..Found::default() };

    // 2. Likely copies: fingerprint = own size, file count, and each child's
    //    (lowercase name, size), order-independent.
    let mut by_print: HashMap<u64, Vec<u32>> = HashMap::new();
    let mut kids: Vec<u64> = Vec::new();
    for &d in &dirs {
        if idx.size(d) < MIN_COPY_BYTES || idx.files(d) < 2 {
            continue;
        }
        kids.clear();
        for c in ch.live(idx, d) {
            let name = idx.name(c).to_lowercase();
            kids.push(fnv(fnv(0xcbf2_9ce4_8422_2325, name.as_bytes()), &idx.size(c).to_le_bytes()));
        }
        kids.sort_unstable();
        let mut h = fnv(0xcbf2_9ce4_8422_2325, &idx.size(d).to_le_bytes());
        h = fnv(h, &idx.files(d).to_le_bytes());
        for k in &kids {
            h = fnv(h, &k.to_le_bytes());
        }
        by_print.entry(h).or_default().push(d);
    }
    let mut copy_groups: Vec<Vec<u32>> = by_print.into_values().filter(|v| v.len() >= 2).collect();
    for g in &mut copy_groups {
        g.sort_unstable();
    }
    // Biggest first; ties by position in the tree, so results never vary.
    copy_groups.sort_by_key(|g| (std::cmp::Reverse(idx.size(g[0]) * g.len() as u64), g[0]));
    let mut listed: HashSet<u32> = HashSet::new();
    let inside_listed = |n: u32, listed: &HashSet<u32>| {
        let mut cur = idx.parent(n);
        while cur != NO_PARENT {
            if listed.contains(&cur) {
                return true;
            }
            cur = idx.parent(cur);
        }
        false
    };
    for g in copy_groups {
        if g.iter().all(|&n| inside_listed(n, &listed)) {
            continue; // folders inside copies already listed
        }
        listed.extend(g.iter().copied());
        if found.copies.len() < MAX_GROUPS {
            found.copies.push(group(idx, idx.name(g[0]).to_owned(), &g, true));
        }
    }

    // 3. Similar names: join folders sharing a full key or a family key,
    //    unless that key is everywhere (generic).
    let mut full_of: Vec<Option<(String, String)>> = Vec::with_capacity(dirs.len());
    let mut count_full: HashMap<String, usize> = HashMap::new();
    let mut count_family: HashMap<(u32, String), usize> = HashMap::new();
    for &d in &dirs {
        let k = if idx.alloc(d) >= MIN_MEMBER_BYTES { keys(idx.name(d)) } else { None };
        if let Some((full, fam)) = &k {
            *count_full.entry(full.clone()).or_default() += 1;
            *count_family.entry((idx.parent(d), fam.clone())).or_default() += 1;
        }
        full_of.push(k);
    }
    let mut uf = Uf((0..dirs.len() as u32).collect());
    // Same full name: anywhere on the drive. Same family (first word):
    // only next to each other, in one parent folder (APALA, APALA_dev, …).
    let mut first_full: HashMap<&str, u32> = HashMap::new();
    let mut first_family: HashMap<(u32, &str), u32> = HashMap::new();
    for (i, k) in full_of.iter().enumerate() {
        let Some((full, fam)) = k else { continue };
        let i = i as u32;
        if count_full[full] <= MAX_FAMILY {
            let j = *first_full.entry(full.as_str()).or_insert(i);
            uf.join(i, j);
        }
        if count_family[&(idx.parent(dirs[i as usize]), fam.clone())] <= MAX_FAMILY {
            let j = *first_family.entry((idx.parent(dirs[i as usize]), fam.as_str())).or_insert(i);
            uf.join(i, j);
        }
    }
    let mut groups: HashMap<u32, Vec<u32>> = HashMap::new();
    for (i, k) in full_of.iter().enumerate() {
        if k.is_some() {
            let r = uf.find(i as u32);
            groups.entry(r).or_default().push(dirs[i]);
        }
    }
    let mut name_groups: Vec<(String, Vec<u32>)> = groups
        .into_iter()
        .filter(|(_, v)| v.len() >= 2)
        .map(|(r, v)| (full_of[r as usize].as_ref().map(|k| k.1.clone()).unwrap_or_default(), v))
        // A folder and the copies inside it, all named alike, add nothing new.
        .filter(|(_, v)| !(v.iter().all(|&n| listed.contains(&n) || inside_listed(n, &listed))))
        .collect();
    for (_, v) in &mut name_groups {
        // A folder never appears with folders inside it (its size already
        // holds theirs): keep the outer ones, the projects themselves.
        let set: HashSet<u32> = v.iter().copied().collect();
        v.retain(|&n| !ancestors_of(idx, n).iter().any(|a| set.contains(a)));
    }
    name_groups.retain(|(_, v)| v.len() >= 2 && v.iter().map(|&n| idx.alloc(n)).sum::<u64>() >= 1 << 20);
    for (_, v) in &mut name_groups {
        v.sort_unstable();
    }
    name_groups.sort_by_key(|(_, v)| (std::cmp::Reverse(v.iter().map(|&n| idx.alloc(n)).sum::<u64>()), v[0]));
    found.names = name_groups.into_iter().take(MAX_GROUPS).map(|(label, v)| group(idx, label, &v, false)).collect();
    found.secs = t0.elapsed().as_secs_f64();
    found
}

fn ancestors_of(idx: &Index, n: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut cur = idx.parent(n);
    while cur != NO_PARENT {
        out.push(cur);
        cur = idx.parent(cur);
    }
    out
}

fn group(idx: &Index, label: String, nodes: &[u32], copies: bool) -> Group {
    let mut members: Vec<Member> = nodes
        .iter()
        .map(|&n| Member { node: n, path: idx.path(n), bytes: idx.alloc(n), files: idx.files(n), mtime: idx.mtime(n) })
        .collect();
    members.sort_by_key(|m| std::cmp::Reverse(m.bytes));
    // Copies: the name most of them have (ties: the shortest).
    let label = if copies {
        let mut count: HashMap<&str, usize> = HashMap::new();
        for &n in nodes {
            *count.entry(idx.name(n)).or_default() += 1;
        }
        count.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.len().cmp(&a.0.len())).then(b.0.cmp(a.0))).map_or(label, |(n, _)| n.to_owned())
    } else {
        label
    };
    let bytes: u64 = members.iter().map(|m| m.bytes).sum();
    let reclaim = if copies { bytes - members[0].bytes } else { 0 };
    Group { label, members, bytes, reclaim }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_reduce_to_families() {
        assert_eq!(tokens("NeuroPulse_v3 (2)"), ["neuro", "pulse", "v", "3", "2"]);
        assert_eq!(keys("APALA_dev"), Some(("apaladev".into(), "apala".into())));
        assert_eq!(keys("APALA_v3"), Some(("apala".into(), "apala".into())));
        assert_eq!(keys("apala-release").unwrap().1, "apala");
        assert_eq!(keys("Neuro_Pulse").unwrap().0, keys("NeuroPulse - Copy (1)").unwrap().0);
        assert_eq!(keys("src"), None, "generic");
        assert_eq!(keys("2023"), None, "only a number");
        assert_eq!(keys("22AUG"), None, "dates: the number is what matters");
        assert_eq!(keys("x"), None, "too short");
        // A generic first word falls back to the whole name.
        assert_eq!(keys("data_raw").unwrap(), ("dataraw".into(), "dataraw".into()));
    }

    /// C:\ with proj\{a.bin,b.bin} copied to backup\proj and Desktop\proj - Copy,
    /// APALA / APALA_dev / APALA_v3 siblings, and many generic "src" folders.
    fn sample() -> (Index, Children) {
        const MB: u64 = 1 << 20;
        let mut idx = Index::new("C:\\");
        let d = |idx: &mut Index, p: u32, n: &str| idx.push(p, n, 0, 0, flags::DIR);
        let f = |idx: &mut Index, p: u32, n: &str, s: u64| idx.push(p, n, s, s, 0);
        for parent_name in ["work", "backup", "Desktop"] {
            let p = d(&mut idx, 0, parent_name);
            let proj = d(&mut idx, p, if parent_name == "Desktop" { "proj - Copy" } else { "proj" });
            f(&mut idx, proj, "a.bin", 3 * MB);
            f(&mut idx, proj, "B.bin", 2 * MB);
            let inner = d(&mut idx, proj, "inner");
            f(&mut idx, inner, "c.bin", 2 * MB);
            f(&mut idx, inner, "d.bin", MB);
        }
        let research = d(&mut idx, 0, "research");
        for (n, s) in [("APALA", 10 * MB), ("APALA_dev", 3 * MB), ("APALA_v3", MB), ("Unrelated", 5 * MB)] {
            let x = d(&mut idx, research, n);
            f(&mut idx, x, "f", s);
        }
        let srcs = d(&mut idx, 0, "code");
        for i in 0..50 {
            let p = d(&mut idx, srcs, &format!("repo{i}"));
            let s = d(&mut idx, p, "src");
            f(&mut idx, s, "m.rs", 100 << 10);
        }
        let win = d(&mut idx, 0, "Windows");
        for _ in 0..2 {
            let w = d(&mut idx, win, "same");
            f(&mut idx, w, "x", 2 * MB);
            f(&mut idx, w, "y", 2 * MB);
        }
        idx.compute_totals();
        let ch = Children::build(&idx);
        (idx, ch)
    }

    #[test]
    fn finds_top_level_copies_only() {
        let (idx, ch) = sample();
        let f = find(&idx, &ch, |_| false);
        let labels: Vec<&str> = f.copies.iter().map(|g| g.label.as_str()).collect();
        // proj (3 copies, one renamed "- Copy") and work = backup one level
        // up; inner\ is implied and Windows isn't looked into.
        assert_eq!(labels, ["proj", "work"], "{labels:?}");
        let g = &f.copies[0];
        assert_eq!(g.members.len(), 3, "work, backup and Desktop copies");
        assert_eq!(g.reclaim, 2 * g.members[0].bytes);
        assert_eq!(f.copies[1].members.len(), 2);
    }

    #[test]
    fn finds_name_families_but_not_generic_names() {
        let (idx, ch) = sample();
        let f = find(&idx, &ch, |_| false);
        let apala = f.names.iter().find(|g| g.label == "apala").expect("APALA family");
        let mut names: Vec<&str> = apala.members.iter().map(|m| m.path.rsplit('\\').next().unwrap()).collect();
        names.sort_unstable();
        assert_eq!(names, ["APALA", "APALA_dev", "APALA_v3"]);
        assert!(f.names.iter().all(|g| g.label != "src"), "generic names are ignored");
        assert!(f.names.iter().all(|g| !g.members.iter().any(|m| m.path.contains("Unrelated"))));
    }
}
