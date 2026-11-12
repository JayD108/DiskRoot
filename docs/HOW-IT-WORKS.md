# How DiskRoot works

[← Back to README](../README.md) · What each feature does: [FEATURES-AND-DEVELOPMENT.md](FEATURES-AND-DEVELOPMENT.md)

This page follows your data from the disk to the screen: how a scan reads the drive, how millions of files fit in a small amount of memory, how DiskRoot decides what a folder is, and what happens, step by step, when you delete something.

**Contents**
- [The big picture](#the-big-picture)
- [The name](#the-name)
- [1. Roots: reading the drive](#1-roots-reading-the-drive)
- [2. Vessels: the data model](#2-vessels-the-data-model)
- [3. Meaning: what each thing is](#3-meaning-what-each-thing-is)
- [4. Canopy: from millions of files to one picture](#4-canopy-from-millions-of-files-to-one-picture)
- [5. Memory: reopen and "what changed"](#5-memory-reopen-and-what-changed)
- [6. Live updates and alerts](#6-live-updates-and-alerts)
- [7. Deleting, step by step](#7-deleting-step-by-step)
- [8. Windows' own tools](#8-windows-own-tools)
- [9. Where the time goes](#9-where-the-time-goes)
- [10. Accuracy](#10-accuracy)

---

## The big picture

```
                 ┌──────────────────── diskroot.exe  (your normal user, never admin) ─────────────────────┐
  You ─ click ─► │  UI (egui): tree · map · details        ◄── asks ──   engine (diskroot-session)        │
                 │  redraws only when something changes                 Scanned = index + rules +     │
                 │                                                       categories + totals          │
                 │  worker threads: scan · delete · export · duplicate finder · live-update watcher    │
                 └─────────▲───────────────────────────────────────────────────────┬──────────────────┘
                           │ private pipe: the finished scan (checked before use)  │ Fast scan → Windows
                           │                                                       ▼ UAC prompt
                 ┌─────────┴──── diskroot.exe --helper-scan C: --pipe <name>  (admin, ~1-2 s) ────────────┐
                 │ 1. open drive C: read-only  2. connect to the pipe  3. DROP admin power             │
                 │ 4. read the Master File Table  5. build the tree  6. send it  7. exit               │
                 └────────────────────────────────────────────────────────────────────────────────────┘
  Without admin, the window walks the folders itself (no prompt, any file system).
```

One program, `diskroot.exe`, plays three roles: the app, the Fast-scan helper and the virtual-disk helper. Which one depends on how it's started. Everything heavy runs on worker threads, so the window never freezes.

## The name

> **DiskRoot** gets to the *root* of what fills your drive: it reads the drive from the bottom (the raw file table) and carries the answer all the way up to plain English. Like a plant, it works from the roots up:

| A plant | DiskRoot |
|---|---|
| **Roots** reach into the soil | The helper drops straight into the drive: the **boot sector**, then the raw **Master File Table**, not the folder APIs. |
| **Vessels** carry sap upward | Every file is one row in flat arrays. Sizes **roll up** from children to parents in a single backwards loop. |
| **Growth rings** record time | The sunburst is a stem cross-section: the middle is the current folder, each ring one level deeper. Colour by age shows how long ago each part last grew. |
| **The canopy** opens to the light | Only what's visible reaches the UI: a pruned view of a few thousand shapes, never millions. |

---

## 1. Roots: reading the drive

### The MFT reader (`crates/scan-mft`): Fast scan, NTFS, admin
Every NTFS drive keeps a table with one 1 KB record per file and folder: the **Master File Table (MFT)**. Reading it directly is much faster than asking Windows folder by folder.

1. **Boot sector** (`volume.rs`): the first sector of the drive says how big a cluster is, how big an MFT record is, and where the MFT starts. Values outside what NTFS really uses are rejected, so a damaged drive can't make DiskRoot allocate absurd buffers.
2. **The MFT's own record (record 0)** describes where the MFT itself lies on disk, as a list of fragments (a *run list*). Very fragmented MFTs are followed through extension records.
3. **Reading:** the MFT is split into 4 MB jobs. Several threads read them with unbuffered, overlapped I/O through **one shared drive handle** (opening one handle per thread cost ~0.35 s each).
4. **Parsing** (`record.rs`), for each 1 KB record:
   - **Fixups:** NTFS stamps the end of every 512-byte block so a half-written record can be detected. Records that fail the check are skipped.
   - **Attributes read:** `$STANDARD_INFORMATION` (attributes such as hidden/compressed/sparse, the **last-modified time** and the **created time**, i.e. when the file arrived on this volume), `$FILE_NAME` (the name and the **parent folder's record number + sequence number**; the long Windows name is preferred over the 8.3 short name), `$DATA` (logical size and **size on disk**, including CompactOS files whose real data sits in a named stream), `$REPARSE_POINT` (marks links, which are never followed, and OneDrive placeholders), `$ATTRIBUTE_LIST` (records spread over several entries are folded together).
   - Every read is bounds-checked; this code is fuzz-tested with billions of hostile inputs.
5. **Building the tree** (`scan.rs`):
   - Each record is linked to its parent. The parent's *sequence number* must match, so a link to a deleted, reused record is rejected (the file becomes an *orphan* and is counted separately).
   - The tree is walked **breadth-first from the root (record 5)**, giving every reachable item a new id in that order. Parents therefore always get smaller ids than their children, which is the key to everything fast in step 2 below. Records in a cycle (only on a corrupt MFT) are never reached and can't hang the scan.
6. **Hand-off:** the helper writes the finished tree into the private pipe and exits. The window reads it with the same checked loader it uses for its own cache files.

Each file record becomes exactly one entry, so a hardlinked file is counted **once**, and the total matches Windows' own "used space".

### The folder walk (`crates/scan-walk`): no admin, any drive
1. Eight worker threads share a stack of folders to list.
2. Each folder is opened **relative to its parent's handle**, which is fast and immune to long paths.
3. One `NtQueryDirectoryFile` call returns up to 64 KB of entries, each with its name, size, size on disk, attributes and last-modified time.
4. Links are recorded but never entered. Folders that can't be read are marked *unreadable*.
5. Results go into the same index as the MFT reader's, so everything after this point is identical.

---

## 2. Vessels: the data model

### The index (`crates/core`)
Instead of millions of small objects, DiskRoot keeps **one array per property**, and a file is just a number (`u32` id) into all of them:

| Column | Bytes per item | What it holds |
|---|---|---|
| `parent` | 4 | id of the folder it's in |
| `size`, `alloc` | 8 + 8 | logical size and size on disk; for folders, the total of everything inside |
| `files` | 4 | number of files inside (1 for a file) |
| `flags` | 1 | folder, link, cloud, unreadable, compressed, sparse, deleted |
| `name_off`, `name_len` | 4 + 2 | where the name sits in one big shared text block |
| `mtime` | 2 | last-modified day; for folders, the newest inside |
| names | ~22 on average | all names, UTF-8, one after another |

- **Why parent id < child id matters:** to give every folder its total size, DiskRoot walks the arrays **once, backwards**, adding each item to its parent. Because children always come after their parents, every child is finished before its parent is read. 2 million items take a few milliseconds.
- **Memory:** about 81 bytes per item including the category data below. 154 MB for a 2-million-item drive.
- **File format** (`store.rs`, `XYLMIDX2`): the same columns written one after another (only folders' own sizes, totals are recomputed). When it's loaded, the file's declared sizes must match its real length before anything is allocated, every parent id must be smaller than its child's, names must be valid and may not contain `\ / :`. A damaged or crafted file is rejected, never trusted. Version 1 files (DiskRoot 0.1) still load, with dates unknown.

---

## 3. Meaning: what each thing is

### Rules (`classify.rs` + `rules.json`)
Rules are data, not code: 119 folder/file patterns, 16 marker files and 110 file extensions. Each rule gives a **category**, a **sub-type**, a plain-English **note**, a **safety label** and maybe a **protection** level, plus the programs that use the place and a Windows tool that cleans it.

Patterns look like `node_modules`, `AppData/Local/Temp` (matches the end of any path), `/Windows/WinSxS` (anchored at the drive root) or `/Users/*/Downloads` (`*` = any text).

### One pass over the whole tree
Items are visited in id order, so a folder is always decided before what's inside it:
1. **Look up** rules whose last part matches the item's name, through a hash table. Most names match nothing, so this is nearly free. Anchored rules are checked against the item's depth first, which rules out most candidates without walking up the tree.
2. **The most specific match wins** (more path parts, then fewer wildcards).
3. **Marker files:** if no rule matched a folder but it holds `CACHEDIR.TAG`, it's *Build output*.
4. **Your own rules** come last: only where no built-in rule matched, and never inside a place protected as a whole tree. A protected-tree bit is passed down to everything inside such a place to enforce this.
5. **Otherwise inherit:** the item takes its parent's category.
6. **Extension rules** (`.mp4`, `.gguf`, …) re-classify files inside general places (Documents, Desktop, unclassified folders), or anywhere for "strong" types such as AI model files.

About 0.15 s for 1.8 million items, running in parallel with building the child lists.

### Anchors and category totals
The folder where a classification was decided is the item's **anchor**. The Category view lists anchors ("which folders make up *Dev stuff › node_modules*"), and because every byte has exactly one anchor, the numbers always add up. Per-folder category totals are kept as a sparse list (only folders that mix categories need an entry), which is what colours each folder by its dominant category.

### Protection
`protection(item)` walks up from the item: if any folder on the way was matched by a rule that protects its whole tree (Windows, Program Files…), or the item itself is protected, it can't be deleted. Separately, Windows and Program Files are compared **by file identity**, so another spelling of the path can't get around the lock (see section 7).

---

## 4. Canopy: from millions of files to one picture

- **The engine API (`crates/session`):** one `Scanned` value answers every question the UI asks: `list` (a folder's children, biggest first), `view` (shapes for the map), `category_view`, `info` (the details pane), `prepare_delete`, `leftovers`, `top_files`, `age_breakdown`, … The UI never touches the raw index.
- **Child lists:** a compact table (start offset + ids) built once per scan, so "children of X" is a slice, not a search.
- **The pruned view for the map:** at most about 6 levels deep, and anything smaller than 0.2% of the current folder is merged into "N small items". A sunburst of a whole drive is a few thousand shapes, not millions.
- **The app (`crates/native`):** egui, drawn with OpenGL. The layout is cached, and the window redraws only on input, while a job runs, or once a second while live updates are on. No browser engine, one process, fonts from Windows.
- **Jobs:** scans, deletes, exports, the duplicate finder and Windows tools run on worker threads and report back through channels. A progress dialog shows while they run.

---

## 5. Memory: reopen and "what changed"

`remember.rs`, after a scan is on screen (a background thread, so the scan itself isn't slowed):
1. **Saves the scan** (the index file format above) as `<fingerprint>.idx` in `%LOCALAPPDATA%\DiskRoot\cache`. The fingerprint is a hash of the scanned path.
2. **Saves a folder snapshot**: every folder's path and size. The previous snapshot is kept as `.prev.snap`.
3. **Compares** the new snapshot with the previous one. Folders are matched by a fingerprint of their path, and DiskRoot reports the **deepest** folder where a change happened (at least 1 MB or 0.1% of the total), not every parent above it. These numbers also feed the tree's Change column.
4. **Instant reopen:** at start, the last root's file is loaded, checked like any other file, and shown with its age. C: reopens in about 0.3–0.4 s.
5. At most 8 roots are kept. After a delete, the saved scan of that root is dropped, because it's out of date.
6. **Size history** (`history.rs`): the folders of 1 MB or more are added to `<fingerprint>.hist` as one point (time, scan method, root size, the volume's size and free space, and only the folders whose size changed since the previous point). Old points are thinned (every scan for 2 days, daily for 60 days, weekly after) by replaying the sizes and re-expressing the kept points, so their sizes stay exact. "Drive full in ~N days" fits a robust trend to the volume's daily used space (rare sudden jumps removed, Theil-Sen slope, range from quartiles) and needs a week of scans.

## 6. Live updates and alerts

`watch.rs`:
1. A thread asks Windows to report every change under the scanned folder (`ReadDirectoryChangesW`, recursive: names, sizes, write times). No admin needed.
2. Reported paths are collected and handed to the app **at most once per second**.
3. The app re-measures only those paths: a file whose size changed moves every folder above it by the difference (and their newest date), a deleted item leaves the tree, a new item is counted in the status bar.
4. If Windows reports that its buffer overflowed, the app says "press F5" instead of guessing.

Size alerts are checked after every scan and every live update. Each alert stores the folder's size when it was set; when the folder has grown by the chosen amount, the alert fires once and re-arms at the new size. Nothing runs when DiskRoot is closed.

---

## 7. Deleting, step by step

`prepare_delete` → `arm` → `recycle` in `crates/session`:

1. **Prepare** (when you press Delete):
   - Refuse protected items, folders that **contain** a protected place, and the scan root itself.
   - Refuse names Windows could silently turn into a different file: a trailing dot or space (`report.` → `report`), device names (`CON`, `NUL`…), paths of 260+ characters.
   - Collect warnings: OneDrive, Git repositories inside, a large delete, a USB/network drive, a running program that uses the place.
2. **Arm** (when the dialog opens): record the **file identity** (volume serial + file ID) of the item and of every folder up to the scan root.
3. **Recycle** (when you confirm):
   - Re-open every folder on the way, from the top down, and compare identities. Keep them **open without delete-sharing**, so none of them can be renamed, deleted or swapped for a junction until the move is done.
   - Check again that the item isn't Windows or Program Files by identity, that it still exists, is the same type, and no folder on its path became a link since the scan.
   - Let Windows move it to the Recycle Bin.
   - Release the folders.
4. **Log** the result (recycled, cancelled or refused) in `deletions.log`, and update the tree and the category totals.

## 8. Windows' own tools

DiskRoot itself only ever moves things to the Recycle Bin. Everything else is done by Windows' own tools, only when you click, started by their **full path in System32** and **never through a shell**, so no argument is ever interpreted by `cmd.exe`:

| Action | Tool | Admin |
|---|---|---|
| Shrink WinSxS | `Dism.exe /Online /Cleanup-Image /StartComponentCleanup` | Yes (UAC) |
| Restore points | System Protection settings | Windows asks |
| `Windows.old`, upgrade files | Settings > Storage | Windows asks |
| Compress / uncompress a folder | DiskRoot's engine: Windows' file compression (`FSCTL_SET_EXTERNAL_BACKING` / `FSCTL_DELETE_EXTERNAL_BACKING`), files in parallel | No |
| Compact a WSL/Docker disk | `diskpart`: attach read-only, compact, detach; fed through a pipe by `diskroot.exe --helper-compact-vhdx`, which re-checks the path and that the disk isn't in use | Yes (UAC) |
| Shut down WSL | `wsl.exe --shutdown` | No |
| Cleanup plan | A PowerShell script DiskRoot writes for you to review (dry run unless `-Run`) | DiskRoot never runs it |

---

## 9. Where the time goes

Measured on one laptop (Intel Core Ultra 9 275HX, NVMe SSD), drive C: with about 2 million items:

| Step | Time |
|---|---|
| Fast scan, whole drive (helper start to tree on screen) | ~1.1 s |
| ↳ building the tree from 1.77M records | 0.16 s |
| ↳ classification, 1.8M items | 0.12–0.15 s |
| Folder walk, whole drive (no admin) | ~4.3–5 s |
| Reopen a remembered scan | 0.3–0.4 s |
| Scan data in memory | 154 MB |

Comparisons with other tools are in [BENCHMARKS.md](BENCHMARKS.md).

## 10. Accuracy

| Situation | What DiskRoot does |
|---|---|
| Hardlinks (one file, several names) | Both count them once. Fast scan reads one record per file; the folder walk asks for each file's ID and gives the bytes to one name (outside WinSxS first, then the alphabetically first path), 0 B to the others. The details pane lists all names of a file. |
| Compressed, sparse and CompactOS files | Size on disk = bytes actually used. Files packed by Windows' file compression list as ~0 bytes, so the folder walk asks Windows for their real size |
| OneDrive online-only files | Size on disk ≈ 0, tagged *cloud* |
| Symlinks and junctions | Tagged *link*, never followed |
| `pagefile.sys`, `hiberfil.sys`, `$MFT`, System Volume Information | Included in Fast scan; they're real space |
| Live updates | A changed file's size on disk comes from Windows when it's compressed or sparse, else it's rounded to 4 KB until the next scan |
| Folder dates | A folder shows the newest last-modified date of anything inside it |
| Checked how | The folder walk is compared item by item with a plain Rust directory walk (`diskroot-bench verify`): an exact match on `C:\Program Files`. Fast-scan totals match Windows' used space. |
