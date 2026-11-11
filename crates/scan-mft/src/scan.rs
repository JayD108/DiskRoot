//! Parallel MFT scan.
//!
//! 1. Split the MFT into 4 MB read jobs.
//! 2. N threads share ONE overlapped volume handle (one handle per thread
//!    cost ~0.35 s each), take jobs, read them and parse every record.
//! 3. Fold extension records into their base records.
//! 4. Link children to parents via the parent reference in $FILE_NAME and
//!    walk breadth-first from the root (record 5) to fill the Index.
//!
//! Each file record becomes exactly one node, so hardlinked files are
//! counted once. Junctions can't cause loops, because the tree comes from
//! parent links, not from following paths.

use std::io;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Instant;

use xylem_core::win::{FILE_ATTRIBUTE_DIRECTORY, node_flags};
use xylem_core::{Columns, Index, NO_PARENT, Scan, ScanStats, flags};

use crate::record::{NO_REC, Parsed, parse_record};
use crate::volume::{AlignedBuf, Extent, Volume, VolumeInfo, mft_extents};

const CHUNK_BYTES: u64 = 4 << 20;
/// The root folder is always MFT record 5.
const ROOT_RECORD: u32 = 5;

struct Job {
    first_record: u32,
    records: usize,
    disk_offset: u64,
    bytes: usize,
}

pub(crate) struct ChunkOut {
    pub(crate) recs: Vec<Parsed>,
    pub(crate) names: String,
}

/// Scans a whole NTFS drive by reading its MFT. Needs administrator rights.
pub fn scan_drive(letter: char, threads: usize) -> io::Result<Scan> {
    let start = Instant::now();
    let vol = Volume::open(letter)?;
    scan_volume_from(&vol, letter, threads, start)
}

/// Like [`scan_drive`], on a volume opened earlier with [`Volume::open`].
/// The elevated helper opens the volume first, then drops its rights
/// (`xylem_session::lockdown`), and only then parses: the open handle keeps
/// its read access, but a parser bug can no longer use admin rights.
pub fn scan_volume(vol: &Volume, letter: char, threads: usize) -> io::Result<Scan> {
    scan_volume_from(vol, letter, threads, Instant::now())
}

fn scan_volume_from(vol: &Volume, letter: char, threads: usize, start: Instant) -> io::Result<Scan> {
    let t_open = start.elapsed();
    let main_reader = vol.reader()?;
    let mut info = main_reader.boot_info()?;
    let t_boot = start.elapsed();
    let extents = mft_extents(&main_reader, &mut info)?;
    let t_layout = start.elapsed();

    let total = u32::try_from(info.mft_valid_len / info.record_size)
        .map_err(|_| io::Error::new(io::ErrorKind::Unsupported, "more than 4 billion MFT records"))?;
    let jobs = make_jobs(&extents, &info, total);

    let next = AtomicUsize::new(0);
    let results = Mutex::new(Vec::with_capacity(jobs.len()));
    let first_error: Mutex<Option<io::Error>> = Mutex::new(None);
    let record_size = info.record_size as usize;

    thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                let fail = |e: io::Error| {
                    first_error.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert(e);
                };
                let reader = match vol.reader() {
                    Ok(r) => r,
                    Err(e) => return fail(e),
                };
                let mut buf = AlignedBuf::new(CHUNK_BYTES as usize);
                while let Some(job) = jobs.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let bytes = &mut buf.as_mut_slice()[..job.bytes];
                    if let Err(e) = reader.read_exact_at(bytes, job.disk_offset) {
                        return fail(e);
                    }
                    let mut out = ChunkOut {
                        recs: Vec::with_capacity(job.records),
                        names: String::with_capacity(job.records * 16),
                    };
                    for k in 0..job.records {
                        let rec = &mut bytes[k * record_size..(k + 1) * record_size];
                        if let Some(p) = parse_record(job.first_record + k as u32, rec, &mut out.names) {
                            out.recs.push(p);
                        }
                    }
                    results.lock().unwrap_or_else(|p| p.into_inner()).push(out);
                }
            });
        }
    });
    if let Some(e) = first_error.into_inner().unwrap_or_else(|p| p.into_inner()) {
        return Err(e);
    }
    let chunks = results.into_inner().unwrap_or_else(|p| p.into_inner());
    let t_read = start.elapsed();
    let (index, mut stats) = build_tree(letter, chunks, total as usize);
    stats.elapsed = start.elapsed();
    if std::env::var_os("XYLEM_TRACE").is_some() {
        eprintln!(
            "[mft] {} records, {} MB MFT, {} extents, {} jobs, {threads} threads: open {:.3} s, boot {:.3} s, extents {:.3} s, read+parse {:.3} s, build {:.3} s",
            total,
            info.mft_valid_len >> 20,
            extents.len(),
            jobs.len(),
            t_open.as_secs_f64(),
            (t_boot - t_open).as_secs_f64(),
            (t_layout - t_boot).as_secs_f64(),
            (t_read - t_layout).as_secs_f64(),
            (stats.elapsed - t_read).as_secs_f64()
        );
    }
    Ok(Scan { index, stats })
}

fn make_jobs(extents: &[Extent], info: &VolumeInfo, total: u32) -> Vec<Job> {
    let cluster = info.bytes_per_cluster;
    let valid = u64::from(total) * info.record_size;
    let mut jobs = Vec::new();
    for e in extents {
        let mut mft_off = e.vcn * cluster;
        let end = ((e.vcn + e.clusters) * cluster).min(valid);
        let mut disk = e.lcn * cluster;
        while mft_off < end {
            let bytes = (end - mft_off).min(CHUNK_BYTES);
            jobs.push(Job {
                first_record: (mft_off / info.record_size) as u32,
                records: (bytes / info.record_size) as usize,
                disk_offset: disk,
                // Reads must be whole sectors; the extent is whole clusters,
                // so rounding up stays inside it.
                bytes: bytes.next_multiple_of(info.bytes_per_sector) as usize,
            });
            mft_off += bytes;
            disk += bytes;
        }
    }
    jobs
}

const NO_SLOT: u64 = u64::MAX;

fn slot_of(chunk: usize, pos: usize) -> u64 {
    ((chunk as u64) << 32) | pos as u64
}

fn get<'a>(chunks: &'a [ChunkOut], slot: &[u64], rec: u32) -> Option<&'a Parsed> {
    let s = *slot.get(rec as usize)?;
    if s == NO_SLOT {
        return None;
    }
    chunks[(s >> 32) as usize].recs.get((s & 0xFFFF_FFFF) as usize)
}

pub(crate) fn build_tree(letter: char, mut chunks: Vec<ChunkOut>, total: usize) -> (Index, ScanStats) {
    let trace = std::env::var_os("XYLEM_TRACE").is_some();
    let t0 = Instant::now();
    let lap = |what: &str| {
        if trace {
            eprintln!("[build] {what}: {:.3} s", t0.elapsed().as_secs_f64());
        }
    };
    // 1. Find each base record; set aside extension records.
    let mut slot = vec![NO_SLOT; total];
    let mut extensions = Vec::new();
    for (ci, ch) in chunks.iter_mut().enumerate() {
        for (ri, p) in ch.recs.iter_mut().enumerate() {
            p.name_chunk = ci as u32;
            if p.base == p.rec {
                slot[p.rec as usize] = slot_of(ci, ri);
            } else {
                extensions.push(*p);
            }
        }
    }

    lap("slots");
    // 2. Fold extension records into their base record.
    for e in &extensions {
        let Some(&s) = slot.get(e.base as usize) else { continue };
        if s == NO_SLOT {
            continue;
        }
        let b = &mut chunks[(s >> 32) as usize].recs[(s & 0xFFFF_FFFF) as usize];
        if e.has_data && !b.has_data {
            b.size = e.size;
            b.has_data = true;
        }
        b.alloc = b.alloc.saturating_add(e.alloc);
        if e.name_rank > b.name_rank {
            (b.name_chunk, b.name_off, b.name_len, b.name_rank) = (e.name_chunk, e.name_off, e.name_len, e.name_rank);
            (b.parent, b.parent_seq) = (e.parent, e.parent_seq);
        }
        if b.reparse_tag == 0 {
            b.reparse_tag = e.reparse_tag;
        }
    }

    lap("extensions");
    // 3. Validate each parent link once, then build children lists in
    //    compressed (CSR) form: children[child_start[r]..child_start[r+1]].
    let mut parent_of = vec![NO_REC; total];
    let mut child_start = vec![0u32; total + 1];
    let (mut named, mut name_bytes) = (0usize, 0usize);
    for ch in &chunks {
        for p in ch.recs.iter().filter(|p| p.base == p.rec && p.name_rank > 0) {
            if p.rec == ROOT_RECORD {
                continue;
            }
            named += 1;
            let ok = p.parent != p.rec
                && get(&chunks, &slot, p.parent).is_some_and(|d| d.is_dir && d.sequence == p.parent_seq);
            if ok {
                parent_of[p.rec as usize] = p.parent;
                child_start[p.parent as usize + 1] += 1;
                name_bytes += p.name_len as usize;
            }
        }
    }
    for i in 1..child_start.len() {
        child_start[i] += child_start[i - 1];
    }
    let mut fill = child_start.clone();
    let mut children = vec![0u32; child_start[total] as usize];
    for (rec, &parent) in parent_of.iter().enumerate() {
        if parent != NO_REC {
            let at = &mut fill[parent as usize];
            children[*at as usize] = rec as u32;
            *at += 1;
        }
    }
    drop((parent_of, fill));

    lap("children");
    // 4a. Breadth-first over the child lists only (compact u32 arrays) to
    //     give every reachable record its node id. BFS order guarantees
    //     parent id < child id.
    let cap = children.len() + 1;
    let mut node_of = vec![u32::MAX; total];
    let mut parent_node = Vec::with_capacity(cap);
    let mut order = Vec::with_capacity(cap); // order[node] = record
    node_of[ROOT_RECORD as usize] = 0;
    order.push(ROOT_RECORD);
    parent_node.push(NO_PARENT);
    let mut head = 0;
    while head < order.len() {
        let rec = order[head] as usize;
        for &c in &children[child_start[rec] as usize..child_start[rec + 1] as usize] {
            node_of[c as usize] = order.len() as u32;
            order.push(c);
            parent_node.push(head as u32);
        }
        head += 1;
    }
    // Records in a parent-link cycle (only on a corrupt MFT) are never
    // reached from the root, so size everything by what BFS reached.
    let n = order.len();
    drop((children, child_start, order));
    lap("bfs ids");

    // 4b. Fill the columns reading parsed records in disk order (sequential).
    let root_name = format!("{letter}:\\");
    let mut cols = Columns {
        parent: parent_node,
        size: vec![0; n],
        alloc: vec![0; n],
        flags: vec![0; n],
        name_off: vec![0; n],
        name_len: vec![0; n],
        names: String::with_capacity(root_name.len() + name_bytes),
    };
    cols.names.push_str(&root_name);
    cols.name_len[0] = root_name.len() as u16;
    cols.flags[0] = flags::DIR;
    let mut stats = ScanStats::default();
    for ch in &chunks {
        for p in &ch.recs {
            let node = node_of[p.rec as usize];
            if node == u32::MAX || node == 0 || p.base != p.rec {
                continue;
            }
            let node = node as usize;
            let names = &chunks[p.name_chunk as usize].names;
            cols.name_off[node] = cols.names.len() as u32;
            cols.name_len[node] = p.name_len;
            cols.names.push_str(&names[p.name_off as usize..p.name_off as usize + p.name_len as usize]);
            let mut f = node_flags(p.attrs & !FILE_ATTRIBUTE_DIRECTORY, p.reparse_tag);
            if p.is_dir {
                f |= flags::DIR;
                stats.dirs += 1;
            } else {
                cols.size[node] = p.size;
                cols.alloc[node] = p.alloc;
                stats.files += 1;
            }
            cols.flags[node] = f;
        }
    }
    drop((chunks, slot, node_of));
    let mut index = Index::from_columns(cols).expect("BFS order upholds the index invariants");
    lap("bfs");
    stats.orphans = (named as u64).saturating_sub(stats.files + stats.dirs);
    index.compute_totals();
    lap("totals");
    (index, stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::tests::{file_name, non_resident, record, std_info};

    const IN_USE: u16 = 1;
    const DIR: u16 = 3;

    /// Builds a tiny fake MFT: root(5) -> Users(30) -> a.bin(31), plus a
    /// hardlinked file, an orphan and a file whose parent was reused.
    fn fake_mft() -> Vec<ChunkOut> {
        let mut names = String::new();
        let mut recs = Vec::new();
        let mut add = |rec_no: u32, mut r: Vec<u8>| {
            if let Some(p) = parse_record(rec_no, &mut r, &mut names) {
                recs.push(p);
            }
        };
        add(5, record(&[file_name(5, 5, 3, ".")], DIR, 0, 5));
        add(30, record(&[file_name(5, 5, 1, "Users")], DIR, 0, 2));
        add(31, record(&[std_info(0x20), file_name(30, 2, 1, "a.bin"), non_resident("", &[0x11, 0x01, 0x10], 4096, 1000, None)], IN_USE, 0, 1));
        // One file, two hardlink names: counted once.
        add(32, record(&[file_name(30, 2, 1, "link1"), file_name(5, 5, 1, "link2"), non_resident("", &[0x11, 0x01, 0x20], 8192, 5000, None)], IN_USE, 0, 1));
        // Orphan: parent record 99 doesn't exist.
        add(33, record(&[file_name(99, 1, 1, "lost")], IN_USE, 0, 1));
        // Stale parent reference: Users has sequence 2, this says 1.
        add(34, record(&[file_name(30, 1, 1, "stale")], IN_USE, 0, 1));
        // Data stream lives in an extension record (base 35).
        add(35, record(&[file_name(5, 5, 1, "big.vhdx")], IN_USE, 0, 1));
        add(36, record(&[non_resident("", &[0x11, 0x01, 0x30], 1 << 20, 900_000, None)], IN_USE, 35, 1));
        vec![ChunkOut { recs, names }]
    }

    #[test]
    fn builds_tree_from_parent_links() {
        let (idx, stats) = build_tree('C', fake_mft(), 40);
        assert_eq!(idx.name(0), "C:\\");
        assert_eq!(stats.dirs, 1);
        assert_eq!(stats.files, 3, "a.bin, hardlinked file once, big.vhdx");
        assert_eq!(stats.orphans, 2, "lost + stale");
        assert_eq!(idx.size(0), 1000 + 5000 + 900_000);
        assert_eq!(idx.alloc(0), 4096 + 8192 + (1 << 20));

        let users = (0..idx.len() as u32).find(|&i| idx.name(i) == "Users").unwrap();
        assert!(idx.is_dir(users));
        assert_eq!(idx.size(users), 6000);
        let big = (0..idx.len() as u32).find(|&i| idx.name(i) == "big.vhdx").unwrap();
        assert_eq!(idx.size(big), 900_000, "size merged from extension record");
    }

    /// Synthetic MFT shaped like the real C: drive: 1.77M records, ~16% dirs.
    fn synthetic_chunks(n: u32) -> Vec<ChunkOut> {
        let mut rng = 0x1234_5678_u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng
        };
        let mut dirs = vec![ROOT_RECORD];
        let mut chunks = Vec::new();
        let mut first = 0;
        while first < n {
            let mut out = ChunkOut { recs: Vec::new(), names: String::new() };
            for rec in first..(first + 4096).min(n) {
                if rec < ROOT_RECORD {
                    continue;
                }
                let is_dir = rec == ROOT_RECORD || next() % 6 == 0;
                let parent = if rec == ROOT_RECORD { ROOT_RECORD } else { dirs[(next() % dirs.len() as u64) as usize] };
                let off = out.names.len() as u32;
                out.names.push_str(&format!("entry_{rec}.dat"));
                out.recs.push(Parsed {
                    rec,
                    base: rec,
                    sequence: 1,
                    parent,
                    parent_seq: 1,
                    name_chunk: 0,
                    name_off: off,
                    name_len: (out.names.len() as u32 - off) as u16,
                    name_rank: 3,
                    is_dir,
                    has_data: !is_dir,
                    attrs: 0x20,
                    reparse_tag: 0,
                    size: next() % 100_000,
                    alloc: 4096,
                });
                if is_dir {
                    dirs.push(rec);
                }
            }
            chunks.push(out);
            first += 4096;
        }
        chunks
    }

    #[test]
    #[ignore = "speed benchmark: cargo test --release -p xylem-scan-mft -- --ignored --nocapture"]
    fn build_speed() {
        let n = 1_770_000;
        let mut best = f64::MAX;
        for _ in 0..3 {
            let chunks = synthetic_chunks(n);
            let t = Instant::now();
            let (idx, stats) = build_tree('C', chunks, n as usize);
            best = best.min(t.elapsed().as_secs_f64());
            assert_eq!(stats.orphans, 0);
            assert_eq!(idx.len() as u64, stats.files + stats.dirs + 1);
        }
        println!("build_tree for {n} records: best {best:.3} s");
    }

    #[test]
    fn splits_mft_into_read_jobs() {
        let info = VolumeInfo {
            bytes_per_sector: 512,
            bytes_per_cluster: 4096,
            record_size: 1024,
            mft_valid_len: 0,
            mft_start_lcn: 0,
        };
        // 2048 clusters (8 MB) then 512 clusters (2 MB); only 9 MB valid.
        let extents = [
            Extent { vcn: 0, lcn: 100, clusters: 2048 },
            Extent { vcn: 2048, lcn: 9000, clusters: 512 },
        ];
        let total = (9 << 20) / 1024;
        let jobs = make_jobs(&extents, &info, total);
        assert_eq!(jobs.len(), 3);
        assert_eq!((jobs[0].first_record, jobs[0].disk_offset, jobs[0].bytes), (0, 100 * 4096, 4 << 20));
        assert_eq!((jobs[1].first_record, jobs[1].disk_offset), (4096, 100 * 4096 + (4 << 20)));
        assert_eq!((jobs[2].first_record, jobs[2].disk_offset, jobs[2].bytes), (8192, 9000 * 4096, 1 << 20));
        let records: usize = jobs.iter().map(|j| j.records).sum();
        assert_eq!(records, total as usize);
    }
}
