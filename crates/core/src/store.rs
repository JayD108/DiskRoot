//! Saving and loading a scanned index (binary, little-endian).
//!
//! Used to hand a scan from the elevated helper process to the app. Loading
//! never trusts the file: everything goes through `Index::from_columns`,
//! which validates the tree.

use std::io::{self, Read, Write};
use std::time::Duration;

use crate::{Columns, Index, ScanStats, flags};

const MAGIC: &[u8; 8] = b"XYLMIDX1";
/// Refuse absurd sizes from a corrupt file before allocating.
const MAX_NODES: u64 = 1 << 32;
const MAX_NAME_BYTES: u64 = 16 << 30;

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("index file: {msg}"))
}

impl Index {
    /// Writes the index plus scan stats and a mode label.
    pub fn save(&self, stats: &ScanStats, mode: &str, w: &mut impl Write) -> io::Result<()> {
        let n = self.len();
        w.write_all(MAGIC)?;
        for v in [
            n as u64,
            self.names.len() as u64,
            stats.files,
            stats.dirs,
            stats.unreadable,
            stats.orphans,
            stats.elapsed.as_nanos() as u64,
        ] {
            w.write_all(&v.to_le_bytes())?;
        }
        w.write_all(&(mode.len() as u32).to_le_bytes())?;
        w.write_all(mode.as_bytes())?;

        // Own (not rolled-up) sizes: folders own 0 bytes, files their size.
        let own = |i: usize, v: &[u64]| if self.flags[i] & flags::DIR != 0 { 0 } else { v[i] };
        for &p in &self.parent {
            w.write_all(&p.to_le_bytes())?;
        }
        for i in 0..n {
            w.write_all(&own(i, &self.size).to_le_bytes())?;
        }
        for i in 0..n {
            w.write_all(&own(i, &self.alloc).to_le_bytes())?;
        }
        w.write_all(&self.flags)?;
        for &o in &self.name_off {
            w.write_all(&o.to_le_bytes())?;
        }
        for &l in &self.name_len {
            w.write_all(&l.to_le_bytes())?;
        }
        w.write_all(self.names.as_bytes())
    }

    /// Like [`Index::load`], but first checks that the sizes declared in the
    /// header match `file_len` exactly, so a corrupt header can't make us
    /// allocate gigabytes before failing.
    pub fn load_checked(r: &mut impl Read, file_len: u64) -> io::Result<(Index, ScanStats, String)> {
        Self::load_inner(r, Some(file_len))
    }

    /// Reads an index written by [`Index::save`], with totals computed.
    pub fn load(r: &mut impl Read) -> io::Result<(Index, ScanStats, String)> {
        Self::load_inner(r, None)
    }

    fn load_inner(r: &mut impl Read, file_len: Option<u64>) -> io::Result<(Index, ScanStats, String)> {
        let mut magic = [0u8; 8];
        r.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(bad("not a xylem index"));
        }
        let mut head = [0u64; 7];
        for v in &mut head {
            let mut b = [0u8; 8];
            r.read_exact(&mut b)?;
            *v = u64::from_le_bytes(b);
        }
        let [n, name_bytes, files, dirs, unreadable, orphans, nanos] = head;
        if n == 0 || n > MAX_NODES || name_bytes > MAX_NAME_BYTES {
            return Err(bad("implausible sizes"));
        }
        let (n, name_bytes) = (n as usize, name_bytes as usize);
        let mut b4 = [0u8; 4];
        r.read_exact(&mut b4)?;
        let mode_len = u32::from_le_bytes(b4) as usize;
        if mode_len > 4096 {
            return Err(bad("implausible mode label"));
        }
        if let Some(len) = file_len {
            // magic + 7 header words + mode length + mode + columns + names
            let expected = 8 + 7 * 8 + 4 + mode_len as u64 + n as u64 * (4 + 8 + 8 + 1 + 4 + 2) + name_bytes as u64;
            if expected != len {
                return Err(bad("size doesn't match its header"));
            }
        }
        let mut mode = vec![0u8; mode_len];
        r.read_exact(&mut mode)?;
        let mode = String::from_utf8(mode).map_err(|_| bad("mode label not UTF-8"))?;

        fn column<const W: usize, T>(r: &mut impl Read, n: usize, conv: fn([u8; W]) -> T) -> io::Result<Vec<T>> {
            let mut raw = vec![0u8; n * W];
            r.read_exact(&mut raw)?;
            Ok(raw.as_chunks::<W>().0.iter().map(|c| conv(*c)).collect())
        }
        let parent = column(r, n, u32::from_le_bytes)?;
        let size = column(r, n, u64::from_le_bytes)?;
        let alloc = column(r, n, u64::from_le_bytes)?;
        let mut flag_bytes = vec![0u8; n];
        r.read_exact(&mut flag_bytes)?;
        let name_off = column(r, n, u32::from_le_bytes)?;
        let name_len = column(r, n, u16::from_le_bytes)?;
        let mut names = vec![0u8; name_bytes];
        r.read_exact(&mut names)?;
        let names = String::from_utf8(names).map_err(|_| bad("names not UTF-8"))?;

        let cols = Columns { parent, size, alloc, flags: flag_bytes, name_off, name_len, names };
        let mut index = Index::from_columns(cols).map_err(|e| bad(&e))?;
        index.compute_totals();
        let stats = ScanStats { files, dirs, unreadable, orphans, elapsed: Duration::from_nanos(nanos) };
        Ok((index, stats, mode))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let mut idx = Index::new("C:\\");
        let d = idx.push(0, "दस्तावेज़", 0, 0, flags::DIR);
        idx.push(d, "a.bin", 100, 4096, 0);
        idx.push(0, "b.bin", 7, 0, flags::CLOUD);
        idx.compute_totals();
        let stats = ScanStats { files: 2, dirs: 1, unreadable: 3, orphans: 4, elapsed: Duration::from_millis(1234) };

        let mut buf = Vec::new();
        idx.save(&stats, "MFT", &mut buf).unwrap();
        let (back, s, mode) = Index::load(&mut buf.as_slice()).unwrap();

        assert_eq!(mode, "MFT");
        assert_eq!((s.files, s.dirs, s.unreadable, s.orphans, s.elapsed), (2, 1, 3, 4, Duration::from_millis(1234)));
        assert_eq!(back.len(), 4);
        assert_eq!(back.path(2), "C:\\दस्तावेज़\\a.bin");
        assert_eq!((back.size(0), back.alloc(0), back.files(0)), (107, 4096, 2));
        assert_eq!(back.flags(3), flags::CLOUD);
    }

    #[test]
    fn rejects_corrupt_files() {
        let mut idx = Index::new("C:\\");
        idx.push(0, "x", 1, 1, 0);
        idx.compute_totals();
        let mut buf = Vec::new();
        idx.save(&ScanStats::default(), "walk", &mut buf).unwrap();

        assert!(Index::load(&mut &buf[..buf.len() - 1]).is_err(), "truncated");
        assert!(Index::load_checked(&mut buf.as_slice(), buf.len() as u64).is_ok(), "exact length accepted");
        assert!(Index::load_checked(&mut buf.as_slice(), buf.len() as u64 + 1).is_err(), "length mismatch");
        let mut wrong = buf.clone();
        wrong[0] ^= 0xFF; // any byte that differs from the magic
        assert!(Index::load(&mut wrong.as_slice()).is_err(), "bad magic");
        let mut huge = buf.clone();
        huge[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(Index::load(&mut huge.as_slice()).is_err(), "absurd node count");
    }
}
