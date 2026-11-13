# Benchmarks

[← Back to README](../README.md) · Measured 11 Nov 2026 with [`tools/bench-competitors.ps1`](../tools/bench-competitors.ps1). Run it yourself: `.\tools\bench-competitors.ps1 -Mode user`, then the same with `-Mode admin` from an elevated PowerShell.

**What's measured:** how long you wait for a whole drive to be scanned, from starting the program to its exit, the same way for every tool. "With admin" uses each tool's fast NTFS mode (reading the file table directly); "without admin" uses each tool's normal folder walk. "+ CSV" also writes a list of every file and folder, which shows how fast each tool can hand its data on.

## Result

**DiskRoot was the fastest in every test, with and without admin rights.**

### With admin rights (fast NTFS / MFT modes)

| Test | DiskRoot | eDirStat 2.0 | WizTree 4.33 | WinDirStat 2.9 |
|---|---|---|---|---|
| Scan the whole drive | **1.07 s** | 2.41 s (2.3x slower) | 4.42 s (4.1x slower) | — |
| Scan + CSV of every file | **1.59 s** | — | 16.57 s (10.4x slower) | 8.09 s (5.1x slower) |

### Without admin rights (folder walks)

| Test | DiskRoot | eDirStat 2.0 | WizTree 4.33 | WinDirStat 2.9 |
|---|---|---|---|---|
| Scan the whole drive | **4.29 s** | 5.28 s (1.2x slower) | 20.83 s (4.9x slower) | — |
| Scan + CSV of every file | **4.85 s** | — | 34.74 s (7.2x slower) | 21.13 s (4.4x slower) |

Medians of 3 timed runs (after 1 warm-up run per tool). Individual runs are in `bench-results/results.csv` (not committed; re-run to regenerate).

### DiskRoot 0.2 against 0.1
0.2 added dates, live updates, rules and more. The rule is that scans don't get slower, so it was measured against the commit before Phases 8–10, the two builds alternating 5 times each on the same drive (with background fuzzing running, so both are slower than on an idle PC):

| | Before (fa32b84) | 0.2 |
|---|---|---|
| Whole-drive folder walk (no admin), median | 5.37 s | **5.21 s** |
| Scan data in memory, 1.99M items | 150.0 MB | 153.8 MB (+3.8 MB: one date per item) |
| Classification, 1.8M items (synthetic) | 0.12–0.18 s | 0.12–0.16 s |

No measurable slowdown; memory is up 2.5% for the dates.

### Inside a scan
| Step (drive C:, ~2M items) | Time |
|---|---|
| Building the tree from 1.77M MFT records | 0.16 s |
| Classifying 1.8M items (categories, notes, safety) | 0.12–0.15 s |
| Reopening a remembered scan | 0.3–0.4 s |

## Drawing the window
Full C: scan (1.7M files) open, window 2062×1346, frame after frame (`DISKROOT_BENCH`). Developer laptop (Core Ultra 9 275HX), 13 Nov 2026, DiskRoot 0.7.

| Renderer | Frame interval | Same, limited to 2 cores | Peak RAM |
|---|---|---|---|
| OpenGL (normal) | 4.2 ms | 4.4–4.6 ms | ~280–320 MB |
| CPU renderer, everything redrawn (worst case) | 12 ms | 41 ms | ~205 MB |
| CPU renderer, nothing changed | 2 ms | 2 ms | ~205 MB |
| Windows' own software renderer (Direct3D WARP), for comparison | ~400 ms | 520–640 ms | ~445 MB |

- Building the UI itself takes 0.3–0.5 ms a frame in every case.
- Full redraws: Studio look, 2D map. The Classic look was a little cheaper (11 ms / 34 ms: fewer rounded corners); the 2.5D map adds about 2–5 ms.
- Idle (nothing changing): no frames are drawn, 0% CPU.
- The CPU renderer is the fallback for PCs where OpenGL can't start; on this laptop it was forced with `DISKROOT_RENDERER=software`. Slower CPUs will be slower; the speed guard turns transitions off below ~20 frames a second.

## Compression

How much DiskRoot's **Compress** saves, by file type. Measured 12 Nov 2026 on copies of real files (the originals were only read) with the open probe [`tools/compress-probe`](../tools/compress-probe): each copy compressed with every Windows (WOF) algorithm on one thread, checked byte-for-byte identical afterwards, then uncompressed. zstd and lz4 (in memory, not usable transparently by Windows) are there for comparison. Percent = space saved; MB/s = input per second, one thread (the engine runs many files in parallel).

| File type (size) | XPRESS4K | XPRESS8K | **XPRESS16K (Fast)** | **LZX (Maximum)** | zstd 1 | zstd 3 | zstd 9 | lz4 |
|---|---|---|---|---|---|---|---|---|
| `.jsonl` text (266 MB) | 48.5% | 52.6% | **54.7%** · 719 MB/s | **66.4%** · 48 MB/s | 65.8% | 70.8% | 75.4% | 50.2% |
| `.csv` (660 MB) | 47.2% | 50.1% | **51.5%** · 724 MB/s | **54.2%** · 38 MB/s | 57.5% | 53.3% | 55.5% | 14.6% |
| `.pkl` (48 MB) | 56.9% | 60.4% | **62.3%** · 956 MB/s | **69.2%** · 55 MB/s | 67.6% | 70.3% | 74.7% | 55.7% |
| `.dll` (305 MB) | 49.7% | 56.0% | **60.3%** · 862 MB/s | **72.0%** · 58 MB/s | 72.0% | 76.2% | 78.6% | 60.1% |
| `.edf` EEG (177 MB) | 26.7% | 30.8% | **32.9%** · 723 MB/s | **41.8%** · 67 MB/s | 36.9% | 42.8% | 45.7% | 10.4% |
| `.edf` EEG (577 MB) | 29.5% | 31.8% | **32.6%** · 689 MB/s | **37.8%** · 64 MB/s | 33.5% | 38.2% | 40.9% | 14.8% |
| `.bin` token ids (260 MB) | 20.3% | 23.8% | **25.7%** · 756 MB/s | **36.4%** · 66 MB/s | 24.7% | 35.6% | 46.4% | 14.8% |
| `.npy` float32 (547 MB) | 10.8% | 13.5% | **14.8%** · 653 MB/s | **16.9%** · 91 MB/s | 18.8% | 18.1% | 18.4% | 5.0% |
| `.parquet` (378 MB) | 3.4% | 6.2% | **7.6%** · 865 MB/s | **8.1%** · 125 MB/s | 8.8% | 9.0% | 9.9% | 0% |
| `.pt` weights (441 MB) | 1.7% | 4.6% | **6.0%** · 909 MB/s | **6.7%** · 140 MB/s | 7.4% | 7.3% | 7.4% | 0% |
| `.safetensors` (45 MB) | 1.7% | 4.6% | **6.0%** · 844 MB/s | **6.7%** · 102 MB/s | 7.3% | 7.2% | 7.3% | 0% |
| `.h5` satellite (486 MB) | 0.7% | 1.5% | **2.4%** · 1049 MB/s | **6.7%** · 159 MB/s | 3.1% | 9.2% | 16.9% | 1.9% |

- **Fast (XPRESS16K) is the right default:** 70–95% of Maximum's saving at 5–15× the speed (one thread).
- **Maximum (LZX) pays off on text, code and programs** (+7–12 points), hardly on data files (+0.5–2 points). HDF5 is the exception (2.4% → 6.7%).
- **zstd saves a few points more** on most types, and much more on `.h5` and token files at higher levels, but Windows can't read zstd-compressed files transparently. See [HELP-WANTED.md](HELP-WANTED.md#1-compression-engine-biggest-opportunity).

### Predicting the saving before compressing

Compressing a few 16 KB chunks spread over the file with Windows' XPRESS-Huffman (`RtlCompressBuffer`), and adding them up the way WOF stores chunks (back to back), predicts the real Fast saving closely:

| File type | Real Fast saving | Predicted from 16 chunks (≈1–3 ms) | 64 chunks (≈4–12 ms) | 256 chunks (≈17–47 ms) |
|---|---|---|---|---|
| `.jsonl` | 54.7% | 54.9% | 55.1% | 54.7% |
| `.csv` | 51.5% | 52.4% | 51.7% | 51.6% |
| `.dll` | 60.3% | 64.4% | 61.9% | 59.9% |
| `.pkl` | 62.3% | 65.5% | 64.9% | 64.1% |
| `.edf` (577 MB) | 32.6% | 35.6% | 33.2% | 32.7% |
| `.bin` tokens | 25.7% | 25.4% | 26.1% | 25.8% |
| `.npy` | 14.8% | 14.9% | 14.8% | 14.8% |
| `.parquet` | 7.6% | 8.8% | 7.8% | 7.6% |
| `.pt` | 6.0% | 9.8% | 7.0% | 6.3% |
| `.h5` | 2.4% | 5.8% | 3.1% | 2.5% |

64 chunks (1 MB read per file) are within 2.6 points on every type here, in about 10 ms. Not used by the app yet: today's estimates come from a per-type table built from these measurements (item C1 in [HELP-WANTED.md](HELP-WANTED.md)).

## Size history

Measured 12 Nov 2026 on the C: drive below (two real remembered scans 1.5 h apart, in memory and a temp file; `diskroot-rnd` E13):

| | |
|---|---|
| Folders in a full snapshot | 307,917 (39 MB file) |
| Folders of 1 MB or more (kept in the history) | 25,942 |
| First scan in the history | 2.67 MB |
| Each scan after that (folders whose size changed) | 308 folders, ~3–4 KB |
| Add a scan + thin | 13 ms |
| Load the history | 20 ms |
| Simulated month, a scan every 4 h (26k folders, 300 change each time) | 1.5 MB, 40 points after thinning |

## What stays in "Other"

13 Nov 2026, the C: drive below (remembered index, read-only; `diskroot-rnd` E15):

| | Before smarter categories | After |
|---|---|---|
| Unclassified | 22.8 GB | 1.6 GB |
| All of "Other" (incl. archives, installers) | 70.7 GB (4.5%) | 46.4 GB (3.0%) |
| Classify 1.98M items | 128 ms | 141 ms |

Biggest moves: `.tmp` 7.7 GB, model weights (`.bin` next to `tokenizer_config.json`) 5.0 GB, `.csv` 4.1 GB, Oracle `.dbf` 1.5 GB, `.partial` 1.5 GB, `.jsonl` 1.1 GB.

## DiskRoot's own writes

What saving one scan of C: (1.7M files) writes, 13 Nov 2026:

| | 0.5.0 | 0.5.1 (lz4) |
|---|---|---|
| Remembered scan (`.idx`) | 106 MB | 37.2 MB |
| Folder list (`.snap`) | 39 MB | 6.5 MB |
| Size history (`.hist`) | 2.7 MB | 0.6 MB |
| "While you were away" baseline | 39 MB copy | hardlink, 0 B |
| Reopen (load, warm cache) | ~102 ms | ~131 ms |

## Method
- **Machine:** one laptop: Intel Core Ultra 9 275HX (24 threads), NVMe SSD, Windows 11 Home. Drive C: is 1.86 TB with ~1.55 TB used, ~1.67M files and ~317k folders visible without admin (~2.0M items with admin).
- **Same drive, warm cache, tools interleaved round-robin** so a busy moment on the disk doesn't hit just one tool.
- **Time** = wall clock from process start to exit: what a user waits for, including start-up and writing output.
- **"Scan"** = the smallest output each tool offers:
  - DiskRoot: `droot scan C: --top 1`.
  - eDirStat: `--benchmark C:\` (its headless benchmark mode).
  - WizTree: `/export` limited to the top level, folders only.
- **"Scan + CSV"** = a CSV listing every file and folder:
  - DiskRoot: `--export`.
  - WizTree: `/export`.
  - WinDirStat: `/SaveTo`.
- **Fast modes:** DiskRoot `--mode mft`, WizTree `/admin=1`, WinDirStat and eDirStat with their NTFS scanners (all elevated). Without admin: `--mode walk`, `/admin=0`, and each tool's normal walker.
- **Every scan was complete:** item counts agree between tools (eDirStat 1,668,474 files / 317,176 folders vs DiskRoot 1,668,411 / 317,824 without admin; WinDirStat's admin CSV has 2,055,330 rows).

## Caveats (read before quoting)
- **One machine, one drive.** Results on HDDs, other CPUs or very different file trees may differ. Please send us your numbers.
- **Background load:** six fuzzing processes were using 6 of the 24 CPU threads during the runs. They slow every tool alike, but absolute times on an idle PC will be somewhat lower.
- **eDirStat** was built from its MIT source on crates.io (2.0.0, release profile, nightly toolchain as its README requires). Its paid itch.io binaries may be built differently.
- **Not measured:**
  - **TreeSize 9.9.1:** its command line didn't scan on this PC. `/NOGUI` exited after ~3 s with no output, and `/CSV` export didn't finish within 5 minutes, even for a small folder; most likely a first-run or trial dialog. To be added once that's resolved.
  - **SizeTrend 4.0:** no documented command line to time.
- **WizTree "scan only"** still writes a tiny CSV; WizTree has no scan-only command. Its scan+CSV numbers are the more direct comparison.
- Versions: DiskRoot 0.1.0 (commit after d0223b4), WizTree 4.33, WinDirStat 2.9.3, eDirStat 2.0.0.
