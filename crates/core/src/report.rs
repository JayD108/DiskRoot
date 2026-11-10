//! "Top N" reports over a scanned index.

use serde::Serialize;

use crate::Index;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy {
    /// Logical file size.
    Size,
    /// Size on disk (what actually frees space).
    OnDisk,
}

#[derive(Clone, Debug, Serialize)]
pub struct Entry {
    pub path: String,
    pub size: u64,
    pub on_disk: u64,
    pub files: u32,
}

/// Largest folders (excluding the scan root itself).
pub fn top_folders(idx: &Index, n: usize, by: SortBy) -> Vec<Entry> {
    top(idx, n, by, |i| i != 0 && idx.is_dir(i))
}

/// Largest individual files.
pub fn top_files(idx: &Index, n: usize, by: SortBy) -> Vec<Entry> {
    top(idx, n, by, |i| !idx.is_dir(i))
}

fn top(idx: &Index, n: usize, by: SortBy, keep: impl Fn(u32) -> bool) -> Vec<Entry> {
    debug_assert!(idx.totals_ready(), "call compute_totals first");
    if n == 0 {
        return Vec::new();
    }
    let key = |i: u32| match by {
        SortBy::Size => idx.size(i),
        SortBy::OnDisk => idx.alloc(i),
    };
    // Biggest first; ties broken by id so output is stable.
    let order = |a: &u32, b: &u32| key(*b).cmp(&key(*a)).then(a.cmp(b));

    let count = u32::try_from(idx.len()).expect("index fits in u32");
    let mut ids: Vec<u32> = (0..count).filter(|&i| keep(i)).collect();
    if ids.len() > n {
        ids.select_nth_unstable_by(n - 1, order);
        ids.truncate(n);
    }
    ids.sort_unstable_by(order);
    ids.into_iter()
        .map(|i| Entry {
            path: idx.path(i),
            size: idx.size(i),
            on_disk: idx.alloc(i),
            files: idx.files(i),
        })
        .collect()
}

/// Writes one CSV row per file and folder: path, size, on disk, files,
/// kind. Folder paths are cached (parents always come before children), so
/// each row costs one string join instead of a walk up to the root. Items
/// deleted through Xylem are left out. Every path starts with the drive or
/// share (never `=`, `+`, `-`, `@`) and is quoted, so spreadsheet apps
/// can't treat a cell as a formula.
pub fn write_csv(idx: &Index, w: &mut impl std::io::Write) -> std::io::Result<()> {
    use crate::{NO_PARENT, flags};
    writeln!(w, "Path,Size,OnDisk,Files,Kind")?;
    let mut dir_paths: Vec<Option<Box<str>>> = vec![None; idx.len()];
    let mut buf = String::new();
    for i in 0..idx.len() as u32 {
        if idx.flags(i) & flags::DELETED != 0 {
            continue;
        }
        buf.clear();
        let parent = idx.parent(i);
        if parent == NO_PARENT {
            buf.push_str(idx.name(i));
        } else {
            let Some(p) = dir_paths[parent as usize].as_deref() else { continue };
            buf.push_str(p);
            if !p.ends_with('\\') {
                buf.push('\\');
            }
            buf.push_str(idx.name(i));
        }
        let is_dir = idx.is_dir(i);
        write!(w, "\"{}\",{},{},{},{}", buf.replace('"', "\"\""), idx.size(i), idx.alloc(i), idx.files(i), if is_dir { "dir" } else { "file" })?;
        w.write_all(b"\n")?;
        if is_dir {
            dir_paths[i as usize] = Some(buf.as_str().into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flags;

    #[test]
    fn picks_largest() {
        let mut idx = Index::new("C:\\");
        let a = idx.push(0, "a", 0, 0, flags::DIR);
        let b = idx.push(0, "b", 0, 0, flags::DIR);
        idx.push(a, "small", 10, 4096, 0);
        idx.push(b, "big", 9000, 8192, 0);
        idx.push(b, "mid", 500, 4096, 0);
        idx.compute_totals();

        let files = top_files(&idx, 2, SortBy::Size);
        let names: Vec<_> = files.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(names, ["C:\\b\\big", "C:\\b\\mid"]);

        let dirs = top_folders(&idx, 10, SortBy::OnDisk);
        assert_eq!(dirs.len(), 2);
        assert_eq!(dirs[0].path, "C:\\b");
        assert_eq!(dirs[0].on_disk, 8192 + 4096);
        assert_eq!(dirs[0].files, 2);

        assert!(top_files(&idx, 0, SortBy::Size).is_empty());
    }
}
