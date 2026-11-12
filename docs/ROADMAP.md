# Roadmap

[← Back to README](../README.md) · Every feature explained: [FEATURES-AND-DEVELOPMENT.md](FEATURES-AND-DEVELOPMENT.md)

## Rules every change keeps
1. **Speed:** scans never get slower (C: ≈ 1.1 s with admin, ≈ 4.3 s without), near-zero idle CPU, memory not meaningfully higher. Every feature is benchmarked before it's merged.
2. **Read-only scanning:** scanning never changes your files; DiskRoot's own cache is compressed and kept small (it's SSD writes too).
3. **Careful changes:** anything that changes files happens only after you confirm, is re-checked on disk first, and is undoable where possible (Recycle Bin).
4. **Least privilege:** the window doesn't need admin (it warns if started elevated, and never reads the raw drive itself). Admin work goes through a minimal helper, or through Windows' own tools behind Windows' own UAC prompt.
5. **Offline and private:** no network; nothing runs at startup unless you turn it on.
6. **Open source only:** OSI-licensed components.

## Releases
| Release | What's in it | Status |
|---|---|---|
| v0.1.0 | Scanners, the app, categories, safe delete | Released (unsigned) |
| v0.2.0 | Instant reopen, "what changed", lower memory, finding space, UI refresh, Phases 8–10 below, a hardened admin helper | Released (unsigned) |
| v0.3.0 | Own fast compression engine (right-click Compress), Similar folders, real size of compressed files in the normal scan, "what changed" knows the scan method | Released (unsigned) |
| v0.4.0 | Ways to save space (what to compress, other ways to get space back), arrival dates ("changed or added"), measured compression estimates, cache rules safe only in app data, compression research call | Released (unsigned) |
| v0.4.1 | One download with the CLI on PATH, always-latest download links | Released (unsigned) |
| v0.5.0 | Home, Drive health and Who's writing (no admin), scheduled scans, size history and forecast, warnings, Where every GB is, hardlinks once in the folder walk, smarter categories (projects, owners, teach DiskRoot), DiskRoot's own RAM/CPU | Ready, waiting for the final test and the owner's go |
| 1.0 | Signed, tested on more hardware | Planned |

## Done
- **Scanning:** MFT reader (C: ≈ 1.1 s with admin) and folder walk (≈ 4.3 s without); fastest in [our benchmark](BENCHMARKS.md). Last-modified and created dates come along for free (+2 bytes per item each).
- **Understanding:** categories, plain-English notes, safety labels, protected places, marker files (`CACHEDIR.TAG`), custom rules.
- **The app:** tree-table, sunburst and treemap (curved labels), colour by category or by age, drive strip with % used and the top 5, start screen with drive cards, light/dark themes, keyboard navigation, Ctrl+F filter.
- **Change over time:** instant reopen, "what changed" (list and Change column), live updates while open, size alerts.
- **Finding space:** largest files with search, file types, duplicate finder, hardlink names, Leftovers view, CSV export, Explorer right-click.
- **Cleaning up:** "Mark all" for safe-to-clear places, Recycle Bin only, re-check by file identity with the folder chain locked until the move ends, spelling-proof locks, OneDrive/Git/large/USB/running-app warnings, multi-select, deletion history.
- **Power tools (opt-in):** PowerShell cleanup plan, fast folder compression (own engine), similar-folder finder, guided WSL/Docker disk compaction, DISM / System Protection / Storage settings.
- **Ways to save space:** what could be compressed (big, not changed or added for a while, measured estimates), and every other way to get space back with its size and how.
- **Security:** admin helper that writes nothing to disk and drops its rights before parsing, DLL-planting defence, fuzzing, `cargo audit` + `cargo deny` in CI ([details](SECURITY-AND-PRIVACY.md)).
- **Release:** installer, portable zip, checksums, CI with a real install/uninstall test, signing-ready pipeline.

## The 15 reviewed feature ideas
| # | Feature | Status |
|---|---|---|
| 2 | Dependency purge ("Mark all", Rust `target` by `CACHEDIR.TAG`) | Done in 0.2 |
| 15 | Leftovers view (installers, archives, ISOs in Downloads; Windows upgrade leftovers) | Done in 0.2 |
| 3 | More cache rules, with a warning while the owning app runs | Done in 0.2 |
| 6 | Custom rules (glob/extension; can't unlock protection) | Done in 0.2 |
| 10 | WinSxS cleanup with Windows' DISM | Done in 0.2 |
| 9 | Restore points: open Windows' System Protection | Done in 0.2 |
| 12 | Age view (last modified) | Done in 0.2 |
| 4 | Live updates while open | Done in 0.2 |
| 5 | Size alerts (while open; no background mode) | Done in 0.2 |
| 11 | Leftover app-data hints (evidence shown, never marked) | Done in 0.2 |
| 13 | Export a cleanup plan to PowerShell | Done in 0.2 |
| 1 | NTFS compression with compact.exe (undoable) | Done in 0.2 |
| 7 | Guided WSL/Docker virtual-disk compaction | Done in 0.2 |
| 14 | Archive & replace | After 1.0: needs a careful verify-before-delete design |
| 8 | Space Shifter (move + junction) | After 1.0: high data-loss risk; only with copy → verify → swap |

## Research (in progress)
A study of a faster, leaner engine. Results ship when they're measured to help:
- ~~Skip unused file-table records~~: measured, no gain (99% of records are in use on a real C:).
- ~~Find out why the first raw read is slow~~: about 0.15 s on every new drive handle, wherever it reads; nothing to overlap it with. Read size and thread count are already at the disk's limit (3.6 GB/s).
- A separate sandboxed reader for Fast scan, so a parser bug couldn't even read with admin rights.
- ~~Count hardlinks once in the folder walk too.~~ Done in 0.5.0.
- Store repeated names once; compress the remembered-scan cache.

## Compression research (open to contributors)
See the call in [CONTRIBUTING.md](../CONTRIBUTING.md#call-for-compression-research-) and the numbers in [BENCHMARKS.md](BENCHMARKS.md#compression).
- Sampled estimates (a few 16 KB chunks per file) instead of per-extension guesses.
- A "Smart" level choosing XPRESS16K or LZX per file.
- Lossless transforms for float arrays and model weights; a verified archive-and-restore mode for cold data.

## Before 1.0
- Wider testing: other PCs, an HDD, non-English names, exFAT/FAT32/ReFS drives.
- Destructive security tests on disposable machines; 24-hour fuzzing.
- Screen-reader access to the map.
- Code signing (SignPath Foundation), then 1.0.
