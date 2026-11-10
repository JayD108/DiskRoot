//! Fast scanner that reads the NTFS Master File Table (MFT) directly.
//!
//! The MFT lists every file on an NTFS volume in fixed-size records. Reading
//! it front to back in big sequential chunks is much faster than asking
//! Windows to list every folder. It needs administrator rights, and it only
//! works for whole NTFS drives. [`scan_auto`] falls back to the
//! folder-walk scanner otherwise.

#[cfg(not(windows))]
compile_error!("xylem-scan-mft only supports Windows");

#[cfg(test)]
mod fuzz;
mod record;
mod scan;
mod volume;

use std::io;
use std::path::Path;

use xylem_core::Scan;
pub use scan::scan_drive;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// MFT for whole drives when possible, otherwise folder walk.
    Auto,
    /// MFT only; error if not possible.
    Mft,
    /// Folder walk only.
    Walk,
}

/// Which scanner actually ran.
#[derive(Clone, Debug)]
pub enum Used {
    Mft,
    /// Folder walk, with the reason MFT wasn't used (if Auto fell back).
    Walk { fallback_reason: Option<String> },
}

/// Drive letter if `path` is a drive root like `C:` or `C:\`.
pub fn drive_letter(path: &Path) -> Option<char> {
    let s = path.to_str()?;
    let mut chars = s.chars();
    let letter = chars.next().filter(char::is_ascii_alphabetic)?;
    match (chars.next(), chars.as_str()) {
        (Some(':'), "" | "\\" | "/") => Some(letter.to_ascii_uppercase()),
        _ => None,
    }
}

fn mft_error_reason(e: &io::Error) -> String {
    match e.kind() {
        io::ErrorKind::PermissionDenied => "MFT read needs administrator rights".into(),
        _ if e.raw_os_error() == Some(1) => "drive is not NTFS".into(), // ERROR_INVALID_FUNCTION
        _ => format!("MFT read failed: {e}"),
    }
}

/// Scans `path` with the chosen mode.
pub fn scan_auto(path: &Path, mode: Mode, threads: usize) -> io::Result<(Scan, Used)> {
    let walk = |reason: Option<String>| {
        let opts = xylem_scan_walk::ScanOptions { threads };
        xylem_scan_walk::scan(path, &opts).map(|s| (s, Used::Walk { fallback_reason: reason }))
    };
    let letter = drive_letter(path);
    match (mode, letter) {
        (Mode::Walk, _) => walk(None),
        (Mode::Mft, None) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "MFT mode needs a whole drive, like C:",
        )),
        (Mode::Mft, Some(l)) => scan_drive(l, threads).map(|s| (s, Used::Mft)),
        (Mode::Auto, None) => walk(None),
        (Mode::Auto, Some(l)) => match scan_drive(l, threads) {
            Ok(s) => Ok((s, Used::Mft)),
            Err(e) => walk(Some(mft_error_reason(&e))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_drive_roots() {
        assert_eq!(drive_letter(Path::new("C:")), Some('C'));
        assert_eq!(drive_letter(Path::new("d:\\")), Some('D'));
        assert_eq!(drive_letter(Path::new("C:\\Users")), None);
        assert_eq!(drive_letter(Path::new("\\\\server\\share")), None);
        assert_eq!(drive_letter(Path::new("1:")), None);
    }
}

/// Entry points for coverage-guided fuzzing (`fuzz/`). Not a stable API.
#[doc(hidden)]
pub mod fuzzing {
    /// One raw MFT record (fixups are applied in place).
    pub fn parse_record(rec: &mut [u8]) -> bool {
        let mut names = String::new();
        crate::record::parse_record(1, rec, &mut names).is_some()
    }

    /// A data-run list and an attribute list.
    pub fn runs(data: &[u8]) {
        let _ = crate::record::decode_runs(data);
        let _ = crate::record::data_extension_records(data);
    }

    pub fn boot_sector(b: &[u8]) {
        let _ = crate::volume::parse_boot_sector(b);
    }

    /// Many records (1 KB each) through the parser and the tree builder:
    /// parent cycles, orphans, duplicate record numbers, extension records.
    pub fn tree(data: &[u8]) {
        let mut names = String::new();
        let mut recs = Vec::new();
        for (i, chunk) in data.chunks(1024).take(4096).enumerate() {
            let mut r = chunk.to_vec();
            r.resize(1024, 0);
            if let Some(p) = crate::record::parse_record(i as u32, &mut r, &mut names) {
                recs.push(p);
            }
        }
        let total = recs.iter().map(|p| p.rec.max(p.base) as usize + 1).max().unwrap_or(8).clamp(8, 1 << 16);
        let recs: Vec<_> = recs.into_iter().filter(|p| (p.rec as usize) < total && (p.base as usize) < total).collect();
        let (idx, _) = crate::scan::build_tree('C', vec![crate::scan::ChunkOut { recs, names }], total);
        for i in 1..idx.len() as u32 {
            assert!(idx.parent(i) < i);
        }
    }
}
