//! Putting a deleted item back from the Recycle Bin (Activity > Restore).
//!
//! Windows keeps each recycled item as two files in
//! `<drive>:\$Recycle.Bin\<user SID>\`: `$R<id><ext>` (the item itself, a
//! file or a folder) and `$I<id><ext>` (where it was, when it was deleted,
//! its size). DiskRoot reads the `$I` files of the folders this account can
//! open (its own), and restores by moving `$R…` back to where it was, like
//! Explorer's Restore. It never overwrites: if something is at that path
//! now, it refuses and says so.

use std::path::{Path, PathBuf};

/// One item in the Recycle Bin.
#[derive(Clone, Debug)]
pub struct BinItem {
    /// Where it was before it was deleted.
    pub original: String,
    /// When it was deleted (Unix seconds, UTC).
    pub deleted: u64,
    /// Its size as Windows recorded it.
    pub size: u64,
    info: PathBuf,
    data: PathBuf,
}

/// Seconds between 1601-01-01 (FILETIME) and 1970-01-01.
const EPOCH_DIFF: u64 = 11_644_473_600;

/// Reads one `$I` file: version 1 (Vista to 8: fixed 260-character path)
/// or version 2 (Windows 10 and 11: length-prefixed path).
fn parse_info(b: &[u8]) -> Option<(String, u64, u64)> {
    let u64_at = |i: usize| b.get(i..i + 8).map(|x| u64::from_le_bytes(x.try_into().unwrap_or([0; 8])));
    let version = u64_at(0)?;
    let size = u64_at(8)?;
    let filetime = u64_at(16)?;
    let name: &[u8] = match version {
        1 => b.get(24..24 + 520)?,
        2 => {
            let chars = u32::from_le_bytes(b.get(24..28)?.try_into().ok()?) as usize;
            b.get(28..28 + chars.checked_mul(2)?)?
        }
        _ => return None,
    };
    let wide: Vec<u16> = name.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
    let original = String::from_utf16(&wide).ok()?;
    if original.is_empty() {
        return None;
    }
    let deleted = (filetime / 10_000_000).saturating_sub(EPOCH_DIFF);
    Some((original, deleted, size))
}

/// Everything in the Recycle Bins this account can read, on every drive.
pub fn list() -> Vec<BinItem> {
    let mut out = Vec::new();
    for d in crate::drives() {
        let Ok(subs) = std::fs::read_dir(Path::new(&d).join("$Recycle.Bin")) else { continue };
        for sub in subs.flatten() {
            let Ok(files) = std::fs::read_dir(sub.path()) else { continue };
            for f in files.flatten() {
                let name = f.file_name().to_string_lossy().into_owned();
                let Some(rest) = name.strip_prefix("$I") else { continue };
                let data = sub.path().join(format!("$R{rest}"));
                if !data.exists() {
                    continue;
                }
                let Ok(bytes) = std::fs::read(f.path()) else { continue };
                if let Some((original, deleted, size)) = parse_info(&bytes) {
                    out.push(BinItem { original, deleted, size, info: f.path(), data });
                }
            }
        }
    }
    out
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetLongPathNameW(short: *const u16, long: *mut u16, len: u32) -> u32;
}

/// Short 8.3 folder names (`MRITTU~1`) expanded: the Recycle Bin records long
/// names. The item itself is gone, so its folder is expanded.
fn long_form(path: &str) -> String {
    let p = Path::new(path);
    let (Some(parent), Some(name)) = (p.parent(), p.file_name()) else { return path.to_owned() };
    let w: Vec<u16> = parent.as_os_str().to_string_lossy().encode_utf16().chain([0]).collect();
    let mut buf = vec![0u16; 1024];
    // SAFETY: NUL-terminated input; the output buffer's length is passed.
    let n = unsafe { GetLongPathNameW(w.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) } as usize;
    if n == 0 || n >= buf.len() {
        return path.to_owned();
    }
    Path::new(&String::from_utf16_lossy(&buf[..n])).join(name).to_string_lossy().into_owned()
}

/// The bin item for something DiskRoot deleted at `time` (Unix seconds): the
/// same path, deleted within two minutes of the log entry.
pub fn find<'a>(items: &'a [BinItem], path: &str, time: u64) -> Option<&'a BinItem> {
    let long = long_form(path);
    items
        .iter()
        .filter(|i| (i.original.eq_ignore_ascii_case(path) || i.original.eq_ignore_ascii_case(&long)) && i.deleted.abs_diff(time) <= 120)
        .min_by_key(|i| i.deleted.abs_diff(time))
}

/// Moves the item back to where it was. Refuses if something is there now.
pub fn restore(item: &BinItem) -> Result<(), String> {
    let to = Path::new(&item.original);
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(format!("Something is at {} now. Move or rename it first; DiskRoot doesn't overwrite.", item.original));
    }
    if !item.data.exists() {
        return Err("It's no longer in the Recycle Bin (emptied, or restored already).".into());
    }
    // Same drive (each drive has its own Recycle Bin), so this is a rename.
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't recreate {}: {e}", parent.display()))?;
    }
    std::fs::rename(&item.data, to).map_err(|e| format!("Couldn't move it back: {e}"))?;
    // The record of where it was; Explorer's Restore removes it too.
    let _ = std::fs::remove_file(&item.info);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v2(path: &str, size: u64, unix: u64) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(2u64.to_le_bytes());
        b.extend(size.to_le_bytes());
        b.extend(((unix + EPOCH_DIFF) * 10_000_000).to_le_bytes());
        let w: Vec<u16> = path.encode_utf16().chain([0]).collect();
        b.extend((w.len() as u32).to_le_bytes());
        for c in w {
            b.extend(c.to_le_bytes());
        }
        b
    }

    #[test]
    fn reads_both_versions() {
        let (p, t, s) = parse_info(&v2(r"C:\Users\me\big.iso", 4096, 1_763_900_000)).unwrap();
        assert_eq!((p.as_str(), t, s), (r"C:\Users\me\big.iso", 1_763_900_000, 4096));
        let mut v1 = Vec::new();
        v1.extend(1u64.to_le_bytes());
        v1.extend(7u64.to_le_bytes());
        v1.extend(((10 + EPOCH_DIFF) * 10_000_000).to_le_bytes());
        let mut name = vec![0u8; 520];
        for (i, c) in r"D:\x.txt".encode_utf16().enumerate() {
            name[i * 2..i * 2 + 2].copy_from_slice(&c.to_le_bytes());
        }
        v1.extend(name);
        assert_eq!(parse_info(&v1).unwrap(), (r"D:\x.txt".to_owned(), 10, 7));
        assert!(parse_info(&[0u8; 10]).is_none(), "too short");
        assert!(parse_info(&v2("", 1, 1)).is_none(), "no path");
    }

    #[test]
    fn restores_and_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("diskroot-restore-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        let data = dir.join("bin").join("$Rabc.txt");
        let info = dir.join("bin").join("$Iabc.txt");
        std::fs::write(&data, b"hello").unwrap();
        std::fs::write(&info, b"x").unwrap();
        let original = dir.join("gone").join("a.txt");
        let item = BinItem { original: original.to_string_lossy().into_owned(), deleted: 100, size: 5, info: info.clone(), data: data.clone() };
        assert!(find(std::slice::from_ref(&item), &item.original.to_uppercase(), 160).is_some(), "same path, a minute apart");
        assert!(find(std::slice::from_ref(&item), &item.original, 1000).is_none(), "too far apart");
        restore(&item).unwrap();
        assert_eq!(std::fs::read(&original).unwrap(), b"hello");
        assert!(!info.exists() && !data.exists());
        // Again: something is there now, and the bin copy is gone.
        std::fs::write(&data, b"other").unwrap();
        let e = restore(&item).unwrap_err();
        assert!(e.contains("doesn't overwrite"), "{e}");
        assert_eq!(std::fs::read(&original).unwrap(), b"hello");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real Recycle Bin, with a throwaway temp file only:
    /// `cargo test -p diskroot-session restore -- --ignored`.
    #[test]
    #[ignore = "uses the real Recycle Bin"]
    fn round_trip_through_the_real_recycle_bin() {
        let dir = std::env::temp_dir().join(format!("diskroot-restore-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("restore-me.txt");
        std::fs::write(&f, b"come back").unwrap();
        let path = f.to_string_lossy().into_owned();
        let when = crate::activity::now();
        assert!(matches!(crate::recycle::to_recycle_bin(&f), Ok(crate::recycle::Outcome::Recycled)));
        assert!(!f.exists());
        let bin = list();
        let item = find(&bin, &path, when).expect("found in the Recycle Bin");
        restore(item).unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"come back");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
