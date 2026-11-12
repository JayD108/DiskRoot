//! End-to-end scan of a real temporary folder tree.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use diskroot_core::{Index, flags};
use diskroot_scan_walk::{ScanOptions, scan};

struct TempTree(PathBuf);

impl Drop for TempTree {
    fn drop(&mut self) {
        // Remove the junction first so cleanup can never follow it.
        let _ = fs::remove_dir(self.0.join("loop"));
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write_file(path: &Path, len: usize) {
    fs::File::create(path).unwrap().write_all(&vec![7u8; len]).unwrap();
}

fn find(idx: &Index, name: &str) -> u32 {
    (0..idx.len() as u32)
        .find(|&i| idx.name(i) == name)
        .unwrap_or_else(|| panic!("{name} not in index"))
}

fn build_tree(tag: &str) -> TempTree {
    let root = std::env::temp_dir().join(format!("diskroot-test-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("a").join("b")).unwrap();
    fs::create_dir_all(root.join("empty")).unwrap();
    write_file(&root.join("one.bin"), 1000);
    write_file(&root.join("a").join("two.bin"), 2000);
    write_file(&root.join("a").join("b").join("नमस्ते.txt"), 3000);

    // A junction pointing back at the root: following it would loop forever.
    let status = Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(root.join("loop"))
        .arg(&root)
        .output()
        .expect("run mklink");
    assert!(status.status.success(), "mklink /J failed: {status:?}");
    TempTree(root)
}

#[test]
fn scans_tree_with_unicode_and_junction() {
    let tree = build_tree("main");
    let s = scan(&tree.0, &ScanOptions { threads: 4 }).unwrap();
    let idx = &s.index;

    assert_eq!(s.stats.files, 3);
    assert_eq!(s.stats.dirs, 4); // a, a\b, empty, loop
    assert_eq!(s.stats.unreadable, 0);
    assert_eq!(idx.size(0), 6000);
    assert_eq!(idx.files(0), 3);
    assert!(idx.alloc(0) >= 6000, "size on disk should cover the data");

    let junction = find(idx, "loop");
    assert_eq!(idx.flags(junction) & (flags::DIR | flags::LINK), flags::DIR | flags::LINK);
    assert_eq!(idx.files(junction), 0, "junction must not be followed");

    let a = find(idx, "a");
    assert_eq!(idx.size(a), 5000);
    let hindi = find(idx, "नमस्ते.txt");
    assert!(idx.path(hindi).ends_with(r"a\b\नमस्ते.txt"));
}

#[test]
fn single_thread_gives_same_result() {
    let tree = build_tree("single");
    let one = scan(&tree.0, &ScanOptions { threads: 1 }).unwrap();
    let many = scan(&tree.0, &ScanOptions { threads: 8 }).unwrap();
    assert_eq!(one.index.size(0), many.index.size(0));
    assert_eq!(one.index.len(), many.index.len());
}

#[test]
fn rejects_a_file_as_root() {
    let tree = build_tree("file");
    let err = scan(&tree.0.join("one.bin"), &ScanOptions::default()).err().unwrap();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

/// Files compressed by Windows' file compression (compact /EXE, CompactOS,
/// DiskRoot's Compress) list only an emptied main stream, often 0 bytes on
/// disk. The walk must ask for their real size (bug found 11 Nov 2026: a
/// 188 GB folder showed 47 GB on disk instead of 171 GB).
#[test]
fn windows_compressed_files_count_their_real_size() {
    let root = std::env::temp_dir().join(format!("diskroot-test-wof-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let tree = TempTree(root.clone());
    let text: String = (0..200_000).map(|i| format!("row {i} of a repetitive table\n")).collect();
    fs::write(root.join("packed.txt"), &text).unwrap();
    fs::write(root.join("plain.txt"), &text).unwrap();
    let compact = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join("System32").join("compact.exe");
    let ok = Command::new(compact).args(["/C", "/EXE:XPRESS4K", "/Q"]).arg(root.join("packed.txt")).output().is_ok_and(|o| o.status.success());
    if !ok {
        eprintln!("compact.exe /EXE not available here: skipped");
        return;
    }
    let s = scan(&tree.0, &ScanOptions { threads: 2 }).unwrap();
    let (packed, plain) = (find(&s.index, "packed.txt"), find(&s.index, "plain.txt"));
    let (pa, pl) = (s.index.alloc(packed), s.index.alloc(plain));
    assert!(pa > 0, "a compressed file still takes space: {pa}");
    assert!(pa * 3 < pl, "and much less than the plain copy: {pa} vs {pl}");
    assert_eq!(s.index.size(packed), s.index.size(plain), "same size");
}
