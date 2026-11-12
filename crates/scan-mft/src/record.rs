//! Parsing of NTFS MFT "FILE" records.
//!
//! Everything is bounds-checked. A corrupt or half-written record is
//! skipped, never trusted. Layout reference: the NTFS on-disk format as
//! documented by Microsoft (ntifs.h) and the Linux-NTFS project.

pub const ATTR_STANDARD_INFORMATION: u32 = 0x10;
pub const ATTR_ATTRIBUTE_LIST: u32 = 0x20;
pub const ATTR_FILE_NAME: u32 = 0x30;
pub const ATTR_DATA: u32 = 0x80;
pub const ATTR_REPARSE_POINT: u32 = 0xC0;
const ATTR_END: u32 = 0xFFFF_FFFF;

const RECORD_IN_USE: u16 = 0x0001;
const RECORD_IS_DIRECTORY: u16 = 0x0002;
const ATTR_FLAG_COMPRESSED: u16 = 0x0001;
const ATTR_FLAG_SPARSE: u16 = 0x8000;
/// NTFS protects every 512 bytes of a record, whatever the sector size.
const FIXUP_STRIDE: usize = 512;
/// On disk, this $STANDARD_INFORMATION bit means "the file has extended
/// attributes" (FILE_ATTRIBUTE_EA). Windows reuses the same value for
/// FILE_ATTRIBUTE_RECALL_ON_OPEN, a cloud bit that only folder listings
/// report. Smart App Control puts EAs on most executables and many other
/// files, so keeping this bit made ~450k ordinary files on the test laptop
/// look "online-only". Cloud files are still caught by their reparse tag.
const SI_HAS_EA: u32 = 0x0004_0000;
/// The named stream that holds the real bytes of CompactOS (WOF) files.
const WOF_STREAM: &str = "WofCompressedData";
/// MFT references are 48-bit record number + 16-bit sequence number.
const REF_MASK: u64 = 0xFFFF_FFFF_FFFF;

/// Marks "no record" in u32 record-number fields.
pub const NO_REC: u32 = u32::MAX;

pub fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

pub fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

pub fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Applies the update-sequence fixup in place. Returns false if the record
/// isn't a FILE record or was torn (only partly written).
pub fn apply_fixup(rec: &mut [u8]) -> bool {
    if rec.len() < 48 || &rec[0..4] != b"FILE" {
        return false;
    }
    let (Some(usa_off), Some(usa_count)) = (u16_at(rec, 4), u16_at(rec, 6)) else { return false };
    let (usa_off, usa_count) = (usa_off as usize, usa_count as usize);
    if usa_count < 2 || usa_off + usa_count * 2 > rec.len() || (usa_count - 1) * FIXUP_STRIDE > rec.len() {
        return false;
    }
    let usn = [rec[usa_off], rec[usa_off + 1]];
    for i in 1..usa_count {
        let end = i * FIXUP_STRIDE - 2;
        if rec[end..end + 2] != usn {
            return false;
        }
        let fix = usa_off + i * 2;
        rec[end] = rec[fix];
        rec[end + 1] = rec[fix + 1];
    }
    true
}

pub struct Header {
    pub flags: u16,
    pub sequence: u16,
    /// Base record number (0 if this is a base record).
    pub base: u64,
    pub first_attr: usize,
    pub used: usize,
}

pub fn header(rec: &[u8]) -> Option<Header> {
    Some(Header {
        flags: u16_at(rec, 22)?,
        sequence: u16_at(rec, 16)?,
        base: u64_at(rec, 32)? & REF_MASK,
        first_attr: u16_at(rec, 20)? as usize,
        used: (u32_at(rec, 24)? as usize).min(rec.len()),
    })
}

/// One attribute inside a FILE record.
pub struct Attr<'a> {
    pub typ: u32,
    raw: &'a [u8],
}

impl<'a> Attr<'a> {
    pub fn non_resident(&self) -> bool {
        self.raw[8] != 0
    }

    fn flags(&self) -> u16 {
        u16_at(self.raw, 12).unwrap_or(0)
    }

    /// Attribute name as raw UTF-16LE bytes (empty for unnamed).
    pub fn name(&self) -> Option<&'a [u8]> {
        let chars = self.raw[9] as usize;
        if chars == 0 {
            return Some(&[]);
        }
        let off = u16_at(self.raw, 10)? as usize;
        self.raw.get(off..off + chars * 2)
    }

    pub fn is_unnamed(&self) -> bool {
        self.raw[9] == 0
    }

    /// Value of a resident attribute.
    pub fn value(&self) -> Option<&'a [u8]> {
        if self.non_resident() {
            return None;
        }
        let len = u32_at(self.raw, 16)? as usize;
        let off = u16_at(self.raw, 20)? as usize;
        self.raw.get(off..off + len)
    }

    pub fn lowest_vcn(&self) -> Option<u64> {
        u64_at(self.raw, 16)
    }

    fn runlist_offset(&self) -> Option<usize> {
        Some(u16_at(self.raw, 32)? as usize)
    }

    pub fn runlist(&self) -> Option<&'a [u8]> {
        self.raw.get(self.runlist_offset()?..)
    }

    pub fn data_size(&self) -> Option<u64> {
        u64_at(self.raw, 48)
    }

    /// Bytes actually written so far ("valid data length").
    pub fn initialized_size(&self) -> Option<u64> {
        u64_at(self.raw, 56)
    }

    /// Bytes actually used on disk. For compressed or sparse streams this
    /// is the "compressed size" field, which only exists in their longer
    /// (72-byte) header.
    pub fn on_disk(&self) -> Option<u64> {
        let packed = self.flags() & (ATTR_FLAG_COMPRESSED | ATTR_FLAG_SPARSE) != 0;
        if packed && self.runlist_offset()? >= 72 {
            u64_at(self.raw, 64)
        } else {
            u64_at(self.raw, 40)
        }
    }
}

pub struct Attrs<'a> {
    rec: &'a [u8],
    off: usize,
    end: usize,
}

pub fn attrs<'a>(rec: &'a [u8], h: &Header) -> Attrs<'a> {
    Attrs { rec, off: h.first_attr, end: h.used }
}

impl<'a> Iterator for Attrs<'a> {
    type Item = Attr<'a>;

    fn next(&mut self) -> Option<Attr<'a>> {
        let off = self.off;
        self.off = self.end; // assume failure; fixed below on success
        if off + 16 > self.end {
            return None;
        }
        let typ = u32_at(self.rec, off)?;
        if typ == ATTR_END {
            return None;
        }
        let len = u32_at(self.rec, off + 4)? as usize;
        if len < 16 || off + len > self.end {
            return None;
        }
        self.off = off + len;
        Some(Attr { typ, raw: &self.rec[off..off + len] })
    }
}

fn utf16_eq(raw: &[u8], s: &str) -> bool {
    let mut units = raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c));
    s.encode_utf16().all(|u| units.next() == Some(u)) && units.next().is_none()
}

/// One run of a non-resident attribute: `len` clusters starting at `lcn`
/// (`None` for a sparse run with no clusters on disk).
#[derive(Debug, PartialEq, Eq)]
pub struct Run {
    pub lcn: Option<u64>,
    pub len: u64,
}

fn read_le(bytes: &[u8], signed: bool) -> i64 {
    let mut v: u64 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        v |= u64::from(b) << (8 * i);
    }
    let bits = bytes.len() * 8;
    if signed && bits < 64 && bytes.last().is_some_and(|&b| b & 0x80 != 0) {
        v |= u64::MAX << bits; // sign-extend
    }
    v as i64
}

/// Decodes an NTFS run list (mapping pairs). Returns None if malformed.
pub fn decode_runs(data: &[u8]) -> Option<Vec<Run>> {
    let mut out = Vec::new();
    let mut pos = 0;
    let mut lcn: i64 = 0;
    loop {
        let h = *data.get(pos)?;
        pos += 1;
        if h == 0 {
            return Some(out);
        }
        let (len_size, off_size) = ((h & 0x0F) as usize, (h >> 4) as usize);
        if len_size == 0 || len_size > 8 || off_size > 8 {
            return None;
        }
        let len = read_le(data.get(pos..pos + len_size)?, false) as u64;
        pos += len_size;
        if off_size == 0 {
            out.push(Run { lcn: None, len });
        } else {
            lcn = lcn.checked_add(read_le(data.get(pos..pos + off_size)?, true))?;
            pos += off_size;
            if lcn < 0 {
                return None;
            }
            out.push(Run { lcn: Some(lcn as u64), len });
        }
    }
}

/// What we keep from one in-use FILE record.
#[derive(Clone, Copy, Debug)]
pub struct Parsed {
    pub rec: u32,
    /// Base record this belongs to (== `rec` for base records).
    pub base: u32,
    pub sequence: u16,
    pub parent: u32,
    pub parent_seq: u16,
    /// Which chunk's name arena holds the name (set when merging).
    pub name_chunk: u32,
    pub name_off: u32,
    pub name_len: u16,
    /// 0 = no name, 1 = DOS-only, 2 = POSIX, 3 = Win32.
    pub name_rank: u8,
    pub is_dir: bool,
    /// Holds the unnamed data stream's first extent (so `size` is valid).
    pub has_data: bool,
    pub attrs: u32,
    pub reparse_tag: u32,
    pub size: u64,
    /// Bytes on disk (unnamed stream plus CompactOS stream).
    pub alloc: u64,
    /// Last-modified day from $STANDARD_INFORMATION (0 = unknown).
    pub mtime: u16,
    /// Created-on-this-volume day from $STANDARD_INFORMATION (0 = unknown).
    pub added: u16,
}

/// Parses one record in place (the fixup modifies `rec`). Names are
/// appended to `names`. Returns None for unused or corrupt records.
pub fn parse_record(rec_no: u32, rec: &mut [u8], names: &mut String) -> Option<Parsed> {
    if !apply_fixup(rec) {
        return None;
    }
    let h = header(rec)?;
    if h.flags & RECORD_IN_USE == 0 {
        return None;
    }
    let base = if h.base == 0 { rec_no } else { u32::try_from(h.base).ok()? };
    let mut p = Parsed {
        rec: rec_no,
        base,
        sequence: h.sequence,
        parent: NO_REC,
        parent_seq: 0,
        name_chunk: 0,
        name_off: 0,
        name_len: 0,
        name_rank: 0,
        is_dir: h.flags & RECORD_IS_DIRECTORY != 0,
        has_data: false,
        attrs: 0,
        reparse_tag: 0,
        size: 0,
        alloc: 0,
        mtime: 0,
        added: 0,
    };
    for a in attrs(rec, &h) {
        match a.typ {
            ATTR_STANDARD_INFORMATION => {
                if let Some(v) = a.value() {
                    if let Some(x) = u32_at(v, 32) {
                        p.attrs = x & !SI_HAS_EA;
                    }
                    // Offset 0: creation, 8: last data modification (FILETIMEs).
                    if let Some(t) = v.get(0..8) {
                        p.added = diskroot_core::filetime_days(u64::from_le_bytes(t.try_into().expect("8 bytes")));
                    }
                    if let Some(t) = v.get(8..16) {
                        p.mtime = diskroot_core::filetime_days(u64::from_le_bytes(t.try_into().expect("8 bytes")));
                    }
                }
            }
            ATTR_FILE_NAME => {
                if let Some(v) = a.value() {
                    take_name(&mut p, v, names);
                }
            }
            ATTR_DATA => take_data(&mut p, &a),
            ATTR_REPARSE_POINT => {
                if let Some(t) = a.value().and_then(|v| u32_at(v, 0)) {
                    p.reparse_tag = t;
                }
            }
            _ => {}
        }
    }
    Some(p)
}

fn take_name(p: &mut Parsed, v: &[u8], names: &mut String) {
    let Some(parent_ref) = u64_at(v, 0) else { return };
    let (Some(&chars), Some(&namespace)) = (v.get(64), v.get(65)) else { return };
    let rank = match namespace {
        1 | 3 => 3, // Win32, Win32+DOS
        0 => 2,     // POSIX
        2 => 1,     // DOS 8.3 only
        _ => return,
    };
    if rank <= p.name_rank {
        return; // keep the better (or first) name; extra hardlinks are ignored
    }
    let Some(raw) = v.get(66..66 + chars as usize * 2) else { return };
    let Ok(parent) = u32::try_from(parent_ref & REF_MASK) else { return };

    // Replace the weaker name if it is the last thing in the arena.
    if p.name_rank > 0 && p.name_off as usize + p.name_len as usize == names.len() {
        names.truncate(p.name_off as usize);
    }
    let start = names.len();
    names.extend(
        char::decode_utf16(raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)))
            .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)),
    );
    let (Ok(off), Ok(len)) = (u32::try_from(start), u16::try_from(names.len() - start)) else {
        names.truncate(start);
        return;
    };
    p.name_off = off;
    p.name_len = len;
    p.name_rank = rank;
    p.parent = parent;
    p.parent_seq = (parent_ref >> 48) as u16;
}

fn take_data(p: &mut Parsed, a: &Attr) {
    let Some(name) = a.name() else { return };
    if name.is_empty() {
        if a.non_resident() {
            // Only the first extent (VCN 0) carries the stream sizes.
            if a.lowest_vcn() != Some(0) {
                return;
            }
            if let (Some(size), Some(disk)) = (a.data_size(), a.on_disk()) {
                p.size = size;
                p.alloc = p.alloc.saturating_add(disk);
                p.has_data = true;
            }
        } else if let Some(v) = a.value() {
            p.size = v.len() as u64; // stored inside the MFT record: 0 extra on disk
            p.has_data = true;
        }
    } else if utf16_eq(name, WOF_STREAM)
        && a.non_resident()
        && a.lowest_vcn() == Some(0)
        && let Some(disk) = a.on_disk()
    {
        p.alloc = p.alloc.saturating_add(disk);
    }
}

/// Record numbers holding extra unnamed $DATA extents, read from a resident
/// $ATTRIBUTE_LIST value. Only needed for $MFT itself.
pub fn data_extension_records(list: &[u8]) -> Vec<u64> {
    let mut out = Vec::new();
    let mut off = 0;
    while let (Some(typ), Some(len)) = (u32_at(list, off), u16_at(list, off + 4)) {
        let len = len as usize;
        if len < 26 {
            break;
        }
        let name_chars = list.get(off + 6).copied().unwrap_or(1);
        if let (ATTR_DATA, 0, Some(r)) = (typ, name_chars, u64_at(list, off + 16)) {
            let r = r & REF_MASK;
            if r != 0 && !out.contains(&r) {
                out.push(r);
            }
        }
        off += len;
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const RECORD: usize = 1024;

    fn align8(n: usize) -> usize {
        n.next_multiple_of(8)
    }

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    pub fn resident(typ: u32, name: &str, value: &[u8]) -> Vec<u8> {
        let name = utf16(name);
        let value_off = align8(24 + name.len());
        let len = align8(value_off + value.len());
        let mut a = vec![0u8; len];
        a[0..4].copy_from_slice(&typ.to_le_bytes());
        a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
        a[9] = (name.len() / 2) as u8;
        a[10..12].copy_from_slice(&24u16.to_le_bytes());
        a[16..20].copy_from_slice(&(value.len() as u32).to_le_bytes());
        a[20..22].copy_from_slice(&(value_off as u16).to_le_bytes());
        a[24..24 + name.len()].copy_from_slice(&name);
        a[value_off..value_off + value.len()].copy_from_slice(value);
        a
    }

    pub fn non_resident(name: &str, runs: &[u8], alloc: u64, size: u64, packed: Option<u64>) -> Vec<u8> {
        let name = utf16(name);
        let header = if packed.is_some() { 72 } else { 64 };
        let runs_off = align8(header + name.len());
        let len = align8(runs_off + runs.len() + 1);
        let mut a = vec![0u8; len];
        a[0..4].copy_from_slice(&ATTR_DATA.to_le_bytes());
        a[4..8].copy_from_slice(&(len as u32).to_le_bytes());
        a[8] = 1;
        a[9] = (name.len() / 2) as u8;
        a[10..12].copy_from_slice(&(header as u16).to_le_bytes());
        if packed.is_some() {
            a[12..14].copy_from_slice(&ATTR_FLAG_SPARSE.to_le_bytes());
        }
        a[32..34].copy_from_slice(&(runs_off as u16).to_le_bytes());
        a[40..48].copy_from_slice(&alloc.to_le_bytes());
        a[48..56].copy_from_slice(&size.to_le_bytes());
        if let Some(c) = packed {
            a[64..72].copy_from_slice(&c.to_le_bytes());
        }
        a[header..header + name.len()].copy_from_slice(&name);
        a[runs_off..runs_off + runs.len()].copy_from_slice(runs);
        a
    }

    pub fn file_name(parent: u64, parent_seq: u16, namespace: u8, name: &str) -> Vec<u8> {
        let n = utf16(name);
        let mut v = vec![0u8; 66 + n.len()];
        v[0..8].copy_from_slice(&(parent | u64::from(parent_seq) << 48).to_le_bytes());
        v[64] = (n.len() / 2) as u8;
        v[65] = namespace;
        v[66..].copy_from_slice(&n);
        resident(ATTR_FILE_NAME, "", &v)
    }

    pub fn std_info(attrs: u32) -> Vec<u8> {
        let mut v = vec![0u8; 72];
        v[32..36].copy_from_slice(&attrs.to_le_bytes());
        // Modified 2024-01-01 (day 19723).
        v[8..16].copy_from_slice(&((19_723u64 * 86_400 + 11_644_473_600) * 10_000_000).to_le_bytes());
        resident(ATTR_STANDARD_INFORMATION, "", &v)
    }

    /// Builds a valid 1 KB FILE record with the update-sequence applied.
    pub fn record(attrs: &[Vec<u8>], flags: u16, base: u64, sequence: u16) -> Vec<u8> {
        let mut r = vec![0u8; RECORD];
        r[0..4].copy_from_slice(b"FILE");
        r[4..6].copy_from_slice(&48u16.to_le_bytes()); // USA offset
        r[6..8].copy_from_slice(&3u16.to_le_bytes()); // 1 + 1024/512
        r[16..18].copy_from_slice(&sequence.to_le_bytes());
        r[20..22].copy_from_slice(&56u16.to_le_bytes()); // first attribute
        r[22..24].copy_from_slice(&flags.to_le_bytes());
        r[32..40].copy_from_slice(&base.to_le_bytes());
        let mut off = 56;
        for a in attrs {
            r[off..off + a.len()].copy_from_slice(a);
            off += a.len();
        }
        r[off..off + 4].copy_from_slice(&ATTR_END.to_le_bytes());
        r[24..28].copy_from_slice(&((off + 8) as u32).to_le_bytes());
        r[28..32].copy_from_slice(&(RECORD as u32).to_le_bytes());
        // Update sequence: stash each sector's last 2 bytes, stamp the USN.
        let usn = [0x2A, 0x00];
        r[48..50].copy_from_slice(&usn);
        for i in 1..3 {
            let end = i * 512 - 2;
            r[48 + i * 2] = r[end];
            r[48 + i * 2 + 1] = r[end + 1];
            r[end..end + 2].copy_from_slice(&usn);
        }
        r
    }

    #[test]
    fn decodes_run_lists() {
        // 0x21: 1-byte length, 2-byte offset. Second run goes backwards.
        let runs = [0x21, 0x10, 0x00, 0x01, 0x11, 0x08, 0xF0, 0x01, 0x04, 0x00];
        let r = decode_runs(&runs).unwrap();
        assert_eq!(
            r,
            [
                Run { lcn: Some(0x100), len: 0x10 },
                Run { lcn: Some(0x100 - 0x10), len: 0x08 },
                Run { lcn: None, len: 4 },
            ]
        );
        assert!(decode_runs(&[0x21, 0x10]).is_none(), "truncated");
        assert!(decode_runs(&[0x11, 0x01, 0x80, 0x00]).is_none(), "negative LCN");
    }

    #[test]
    fn parses_a_file_record() {
        let mut r = record(
            &[
                std_info(0x20),
                file_name(5, 5, 2, "LONGFI~1.TXT"),
                file_name(5, 5, 1, "long file name.txt"),
                non_resident("", &[0x11, 0x04, 0x20], 16384, 10_000, None),
            ],
            RECORD_IN_USE,
            0,
            7,
        );
        let mut names = String::new();
        let p = parse_record(42, &mut r, &mut names).unwrap();
        assert_eq!(&names[p.name_off as usize..][..p.name_len as usize], "long file name.txt");
        assert_eq!(names, "long file name.txt", "DOS name was replaced");
        assert_eq!((p.parent, p.parent_seq, p.base, p.sequence), (5, 5, 42, 7));
        assert_eq!((p.size, p.alloc, p.has_data, p.is_dir), (10_000, 16384, true, false));
        assert_eq!(p.attrs, 0x20);
    }

    /// Regression: the on-disk "has extended attributes" bit (Smart App
    /// Control tags files with EAs) shares its value with the cloud bit
    /// RECALL_ON_OPEN. It must not make a local file look online-only.
    #[test]
    fn extended_attributes_are_not_cloud() {
        let mut r = record(&[std_info(0x20 | SI_HAS_EA), file_name(5, 5, 1, "cargo.exe")], RECORD_IN_USE, 0, 1);
        let p = parse_record(50, &mut r, &mut String::new()).unwrap();
        assert_eq!(p.attrs, 0x20);
        assert_eq!(p.mtime, 19_723, "last-modified day from $STANDARD_INFORMATION");
        assert_eq!(diskroot_core::win::node_flags(p.attrs, p.reparse_tag) & diskroot_core::flags::CLOUD, 0);
        // A real OneDrive placeholder is still cloud (reparse tag).
        assert_ne!(diskroot_core::win::node_flags(0x20 | 0x0040_0000, 0x9000_601A) & diskroot_core::flags::CLOUD, 0);
    }

    #[test]
    fn counts_compactos_and_sparse_on_disk_size() {
        let mut r = record(
            &[
                file_name(5, 5, 1, "notepad.exe"),
                non_resident("", &[0x01, 0x10, 0x00], 65536, 200_000, Some(0)),
                non_resident(WOF_STREAM, &[0x11, 0x02, 0x40], 8192, 7000, None),
            ],
            RECORD_IN_USE,
            0,
            1,
        );
        let p = parse_record(9, &mut r, &mut String::new()).unwrap();
        assert_eq!(p.size, 200_000);
        assert_eq!(p.alloc, 8192, "sparse main stream (0) + WOF stream");
    }

    #[test]
    fn rejects_bad_records() {
        let good = record(&[file_name(5, 5, 1, "a")], RECORD_IN_USE, 0, 1);

        let mut torn = good.clone();
        torn[510] ^= 0xFF; // sector 1 tail no longer matches the USN
        assert!(parse_record(1, &mut torn, &mut String::new()).is_none());

        let mut unused = record(&[file_name(5, 5, 1, "a")], 0, 0, 1);
        assert!(parse_record(1, &mut unused, &mut String::new()).is_none());

        let mut junk = vec![0xAB; RECORD];
        assert!(parse_record(1, &mut junk, &mut String::new()).is_none());

        // Attribute claiming to run past the record end: parse stops safely.
        let mut bad_len = good.clone();
        bad_len[56 + 4..56 + 8].copy_from_slice(&5000u32.to_le_bytes());
        let p = parse_record(1, &mut bad_len, &mut String::new()).unwrap();
        assert_eq!(p.name_rank, 0);
    }

    #[test]
    fn reads_extension_and_directory_flags() {
        let mut ext = record(&[non_resident("", &[0x11, 0x01, 0x05], 4096, 100, None)], RECORD_IN_USE, 77, 1);
        let p = parse_record(80, &mut ext, &mut String::new()).unwrap();
        assert_eq!((p.base, p.rec, p.has_data), (77, 80, true));

        let mut dir = record(&[file_name(5, 5, 3, "Users")], RECORD_IN_USE | RECORD_IS_DIRECTORY, 0, 1);
        assert!(parse_record(30, &mut dir, &mut String::new()).unwrap().is_dir);
    }

    #[test]
    fn finds_mft_extension_records_in_attribute_list() {
        let mut list = Vec::new();
        for (typ, rec) in [(ATTR_STANDARD_INFORMATION, 0u64), (ATTR_DATA, 0), (ATTR_DATA, 17), (ATTR_DATA, 17), (ATTR_DATA, 18)] {
            let mut e = vec![0u8; 32];
            e[0..4].copy_from_slice(&typ.to_le_bytes());
            e[4..6].copy_from_slice(&32u16.to_le_bytes());
            e[16..24].copy_from_slice(&(rec | 1 << 48).to_le_bytes());
            list.extend(e);
        }
        assert_eq!(data_extension_records(&list), [17, 18]);
    }
}
