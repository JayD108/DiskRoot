//! "Mark for later": files the user isn't sure about yet, kept on this PC
//! (`%LOCALAPPDATA%\DiskRoot\later.json`) so the list is still there after
//! a restart. Only paths, sizes and a short reason are stored; nothing is
//! deleted or moved by this list.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// One flagged file or folder.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Later {
    pub path: String,
    pub size: u64,
    /// When it was flagged (Unix seconds).
    pub added: u64,
    /// Why, e.g. "Identical copy of D:\Photos\a.jpg" or "Same name as ...".
    #[serde(default)]
    pub why: String,
}

/// `%LOCALAPPDATA%\DiskRoot\later.json` (a temp file in tests).
pub fn file() -> Option<PathBuf> {
    if cfg!(test) {
        return Some(std::env::temp_dir().join(format!("diskroot-test-later-{}.json", std::process::id())));
    }
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("DiskRoot").join("later.json"))
}

/// The saved list (empty if there is none or it can't be read).
pub fn load() -> Vec<Later> {
    file().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Writes the list (to a temp file first, then renamed over the old one, so
/// a crash mid-write never leaves half a list).
pub fn save(list: &[Later]) -> std::io::Result<()> {
    let p = file().ok_or_else(|| std::io::Error::other("no LOCALAPPDATA"))?;
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(list).map_err(std::io::Error::other)?)?;
    std::fs::rename(&tmp, &p)
}

/// True if `path` is on the list (case-insensitive, like Windows paths).
pub fn contains(list: &[Later], path: &str) -> bool {
    list.iter().any(|l| l.path.eq_ignore_ascii_case(path))
}

/// Adds the items that aren't on the list yet; returns how many were new.
pub fn add(list: &mut Vec<Later>, items: impl IntoIterator<Item = Later>) -> usize {
    let mut n = 0;
    for it in items {
        if !contains(list, &it.path) {
            list.push(it);
            n += 1;
        }
    }
    n
}

/// Removes these paths; returns how many were removed.
pub fn remove(list: &mut Vec<Later>, paths: &[String]) -> usize {
    let before = list.len();
    list.retain(|l| !paths.iter().any(|p| p.eq_ignore_ascii_case(&l.path)));
    before - list.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn survives_a_save_and_load() {
        let _ = std::fs::remove_file(file().unwrap());
        assert!(load().is_empty());
        let mut v = Vec::new();
        let it = |p: &str| Later { path: p.into(), size: 5, added: 1, why: "test".into() };
        assert_eq!(add(&mut v, [it(r"C:\a\x.jpg"), it(r"C:\b\x.jpg"), it(r"c:\A\X.JPG")]), 2);
        save(&v).unwrap();
        let back = load();
        assert_eq!(back, v);
        let mut back = back;
        assert_eq!(remove(&mut back, &[r"C:\A\x.jpg".into()]), 1);
        assert!(contains(&back, r"c:\b\X.jpg"));
        let _ = std::fs::remove_file(file().unwrap());
    }
}
