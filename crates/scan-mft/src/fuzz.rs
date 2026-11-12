//! Fuzz tests: the MFT parser reads raw bytes straight off the disk, so a
//! corrupt or hostile volume must never crash it, hang it or make it read
//! out of bounds. These feed it millions of random and mutated inputs.
//! No extra tools needed: they run with `cargo test` (deterministic seeds).
//! Set `DISKROOT_FUZZ_ROUNDS` for a longer run.

use crate::record::tests::{file_name, non_resident, record, std_info};
use crate::record::{data_extension_records, decode_runs, parse_record};
use crate::scan::{ChunkOut, build_tree};
use crate::volume::parse_boot_sector;

pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    pub(crate) fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub(crate) fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    /// Flip, overwrite or insert "interesting" values at random spots.
    pub(crate) fn mutate(&mut self, buf: &mut [u8]) {
        if buf.is_empty() {
            return;
        }
        for _ in 0..1 + self.below(8) {
            let at = self.below(buf.len());
            match self.below(5) {
                0 => buf[at] ^= 1 << self.below(8),
                1 => buf[at] = self.next() as u8,
                2 => buf[at] = [0x00, 0xFF, 0x7F, 0x80, 0x01][self.below(5)],
                3 => {
                    // A 16- or 32-bit boundary value (lengths and offsets).
                    let v: u32 = [0, 0xFFFF, 0xFFFF_FFFF, 0x8000_0000, 1024, 4096][self.below(6)];
                    for (i, b) in v.to_le_bytes().iter().enumerate().take(buf.len() - at) {
                        buf[at + i] = *b;
                    }
                }
                _ => {
                    let from = self.below(buf.len());
                    buf[at] = buf[from];
                }
            }
        }
    }
}

fn rounds(default: usize) -> usize {
    std::env::var("DISKROOT_FUZZ_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn seed_records() -> Vec<Vec<u8>> {
    vec![
        record(&[file_name(5, 5, 3, ".")], 3, 0, 5),
        record(&[std_info(0x20), file_name(30, 2, 1, "a.bin"), non_resident("", &[0x11, 0x01, 0x10], 4096, 1000, None)], 1, 0, 1),
        record(&[file_name(30, 2, 1, "l1"), file_name(5, 5, 2, "L1~1"), non_resident("", &[0x21, 0x10, 0x00, 0x01, 0x11, 0x08, 0xF0], 8192, 5000, Some(4096))], 1, 0, 1),
        record(&[non_resident("", &[0x11, 0x01, 0x30], 1 << 20, 900_000, None)], 1, 35, 1),
    ]
}

#[test]
fn parse_record_survives_mutated_records() {
    let seeds = seed_records();
    let mut rng = Rng::new(0xC0FFEE);
    let mut names = String::new();
    for round in 0..rounds(200_000) {
        let mut r = seeds[round % seeds.len()].clone();
        rng.mutate(&mut r);
        if round % 7 == 0 {
            r.truncate(rng.below(r.len() + 1)); // short reads
        }
        if let Some(p) = parse_record(round as u32, &mut r, &mut names) {
            // Whatever it accepts must point inside the shared name buffer.
            assert!(p.name_off as usize + p.name_len as usize <= names.len(), "round {round}");
        }
        if names.len() > 1 << 20 {
            names.clear();
        }
    }
}

#[test]
fn parse_record_survives_pure_noise() {
    let mut rng = Rng::new(42);
    let mut names = String::new();
    for round in 0..rounds(50_000) {
        let len = [0, 1, 48, 512, 1024, 4096][round % 6];
        let mut r: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        if len >= 4 && round % 2 == 0 {
            r[..4].copy_from_slice(b"FILE"); // get past the signature check
        }
        let _ = parse_record(round as u32, &mut r, &mut names);
        names.clear();
    }
}

#[test]
fn run_lists_never_panic_or_overflow() {
    let mut rng = Rng::new(7);
    for round in 0..rounds(300_000) {
        let len = rng.below(40);
        let mut v: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        if round % 3 == 0 && !v.is_empty() {
            v[0] = 0x88; // max-width length + offset fields
        }
        if let Some(runs) = decode_runs(&v) {
            assert!(runs.len() <= v.len(), "each run uses at least one byte");
        }
        let _ = data_extension_records(&v);
    }
}

#[test]
fn boot_sector_parser_rejects_garbage() {
    let mut rng = Rng::new(99);
    let mut good = vec![0u8; 512];
    good[3..11].copy_from_slice(b"NTFS    ");
    good[0x0B..0x0D].copy_from_slice(&512u16.to_le_bytes());
    good[0x0D] = 8;
    good[0x30..0x38].copy_from_slice(&786_432u64.to_le_bytes());
    good[0x40] = (-10i8) as u8;
    for round in 0..rounds(200_000) {
        let mut b = good.clone();
        rng.mutate(&mut b);
        if round % 5 == 0 {
            b.truncate(rng.below(513));
        }
        if let Ok(i) = parse_boot_sector(&b) {
            // Accepted geometry must be sane enough to size buffers from.
            assert!(i.bytes_per_sector.is_power_of_two() && (512..=4096).contains(&i.bytes_per_sector));
            assert!(i.bytes_per_cluster >= i.bytes_per_sector && i.bytes_per_cluster <= 2 << 20);
            assert!(i.record_size >= 1024 && i.record_size <= 64 << 10, "record size {}", i.record_size);
        }
    }
}

#[test]
fn tree_build_survives_random_parent_links() {
    // Random (possibly cyclic, self-referencing, out-of-range) parent links
    // must still produce a valid tree: parent index < child index.
    let mut rng = Rng::new(2026);
    for round in 0..rounds(300) {
        let total = 8 + rng.below(200);
        let mut names = String::new();
        let mut recs = Vec::new();
        let mut root = record(&[file_name(5, 5, 3, ".")], 3, 0, 5);
        recs.extend(parse_record(5, &mut root, &mut names));
        for rec_no in 0..total as u32 {
            if rec_no == 5 {
                continue;
            }
            let parent = rng.below(total + 4) as u64;
            let seq = rng.below(3) as u16;
            let flags = [1, 3][rng.below(2)];
            let base = if rng.below(10) == 0 { rng.below(total + 4) as u64 } else { 0 };
            let mut r = record(&[file_name(parent, seq, 1, "n"), non_resident("", &[0x11, 0x01, 0x10], 4096, 100, None)], flags, base, 1);
            if rng.below(4) == 0 {
                rng.mutate(&mut r);
            }
            recs.extend(parse_record(rec_no, &mut r, &mut names));
        }
        let (idx, _) = build_tree('C', vec![ChunkOut { recs, names }], total);
        for i in 1..idx.len() as u32 {
            assert!(idx.parent(i) < i, "round {round}: parent must come first");
        }
        assert!(idx.size(0) >= idx.size(idx.len() as u32 - 1));
    }
}

/// Writes seed inputs for the coverage-guided fuzzers in `fuzz/`:
/// `cargo test -p diskroot-scan-mft write_fuzz_seeds -- --ignored`
#[test]
#[ignore = "writes files into fuzz/corpus; run by hand"]
fn write_fuzz_seeds() {
    let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus");
    let put = |target: &str, name: &str, data: &[u8]| {
        let dir = base.join(target);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), data).unwrap();
    };
    let recs = seed_records();
    for (i, r) in recs.iter().enumerate() {
        put("mft_record", &format!("rec{i}"), r);
    }
    put("mft_tree", "four", &recs.concat());
    put("run_list", "two", &[0x21, 0x10, 0x00, 0x01, 0x11, 0x08, 0xF0, 0x01, 0x04, 0x00]);
    let mut boot = vec![0u8; 512];
    boot[3..11].copy_from_slice(b"NTFS    ");
    boot[0x0B..0x0D].copy_from_slice(&512u16.to_le_bytes());
    boot[0x0D] = 8;
    boot[0x30..0x38].copy_from_slice(&786_432u64.to_le_bytes());
    boot[0x40] = (-10i8) as u8;
    put("boot_sector", "ntfs", &boot);
    let mut idx = diskroot_core::Index::new("C:\\");
    let d = idx.push(0, "dir", 0, 0, diskroot_core::flags::DIR);
    idx.push(d, "a.bin", 100, 4096, 0);
    idx.push(0, "b.txt", 7, 0, diskroot_core::flags::CLOUD);
    idx.compute_totals();
    let mut buf = Vec::new();
    idx.save(&diskroot_core::ScanStats::default(), "MFT", &mut buf).unwrap();
    put("index_file", "small", &buf);
}
