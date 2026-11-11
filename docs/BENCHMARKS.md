# Benchmarks

[← Back to README](../README.md) · Measured 11 Nov 2026 with [`tools/bench-competitors.ps1`](../tools/bench-competitors.ps1). Run it yourself: `.\tools\bench-competitors.ps1 -Mode user`, then the same with `-Mode admin` from an elevated PowerShell.

## Result

**Xylem was the fastest in every test, with and without admin rights.**

### With admin rights (fast NTFS / MFT modes)

| Test | Xylem | eDirStat 2.0 | WizTree 4.33 | WinDirStat 2.9 |
|---|---|---|---|---|
| Scan the whole drive | **1.07 s** | 2.41 s (2.3x slower) | 4.42 s (4.1x slower) | — |
| Scan + CSV of every file | **1.59 s** | — | 16.57 s (10.4x slower) | 8.09 s (5.1x slower) |

### Without admin rights (folder walks)

| Test | Xylem | eDirStat 2.0 | WizTree 4.33 | WinDirStat 2.9 |
|---|---|---|---|---|
| Scan the whole drive | **4.29 s** | 5.28 s (1.2x slower) | 20.83 s (4.9x slower) | — |
| Scan + CSV of every file | **4.85 s** | — | 34.74 s (7.2x slower) | 21.13 s (4.4x slower) |

Medians of 3 timed runs (after 1 warm-up run per tool). Individual runs are in `bench-results/results.csv` (not committed; re-run to regenerate).

## Method
- **Machine:** one laptop: Intel Core Ultra 9 275HX (24 threads), NVMe SSD, Windows 11 Home. Drive C: is 1.86 TB with ~1.55 TB used, ~1.67M files and ~317k folders visible without admin (~2.0M items with admin).
- **Same drive, warm cache, tools interleaved round-robin** so a busy moment on the disk doesn't hit just one tool.
- **Time** = wall clock from process start to exit: what a user waits for, including start-up and writing output.
- **"Scan"** = the smallest output each tool offers:
  - Xylem: `xylem-cli scan C: --top 1`.
  - eDirStat: `--benchmark C:\` (its headless benchmark mode).
  - WizTree: `/export` limited to the top level, folders only.
- **"Scan + CSV"** = a CSV listing every file and folder:
  - Xylem: `--export`.
  - WizTree: `/export`.
  - WinDirStat: `/SaveTo`.
- **Fast modes:** Xylem `--mode mft`, WizTree `/admin=1`, WinDirStat and eDirStat with their NTFS scanners (all elevated). Without admin: `--mode walk`, `/admin=0`, and each tool's normal walker.
- **Every scan was complete:** item counts agree between tools (eDirStat 1,668,474 files / 317,176 folders vs Xylem 1,668,411 / 317,824 without admin; WinDirStat's admin CSV has 2,055,330 rows).

## Caveats (read before quoting)
- **One machine, one drive.** Results on HDDs, other CPUs or very different file trees may differ. Please send us your numbers.
- **Background load:** six fuzzing processes were using 6 of the 24 CPU threads during the runs. They slow every tool alike, but absolute times on an idle PC will be somewhat lower.
- **eDirStat** was built from its MIT source on crates.io (2.0.0, release profile, nightly toolchain as its README requires). Its paid itch.io binaries may be built differently.
- **Not measured:**
  - **TreeSize 9.9.1:** its command line didn't scan on this PC. `/NOGUI` exited after ~3 s with no output, and `/CSV` export didn't finish within 5 minutes, even for a small folder; most likely a first-run or trial dialog. To be added once that's resolved.
  - **SizeTrend 4.0:** no documented command line to time.
- **WizTree "scan only"** still writes a tiny CSV; WizTree has no scan-only command. Its scan+CSV numbers are the more direct comparison.
- Versions: Xylem 0.1.0 (commit after d0223b4), WizTree 4.33, WinDirStat 2.9.3, eDirStat 2.0.0.
