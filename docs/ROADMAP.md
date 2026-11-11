# Roadmap

[← Back to README](../README.md)

## Rules every change keeps
1. **Speed:** scans never get slower (C: ≈ 1.1 s with admin, ≈ 4.3 s without), no idle CPU use, memory not meaningfully higher. Every feature is benchmarked before it's merged.
2. **Read-only scanning:** scanning never writes to the scanned drive.
3. **Careful changes:** anything that changes files happens only after you confirm, is re-checked on disk first, and is undoable where possible (Recycle Bin).
4. **Least privilege:** the window never runs as admin. Admin work goes through a minimal helper, or through Windows' own tools behind Windows' own UAC prompt.
5. **Offline and private:** no network; nothing runs at startup unless you turn it on.
6. **Open source only:** OSI-licensed components.

## Releases at a glance
| Release | Theme | Status |
|---|---|---|
| v0.1.0 | Scanners, the app, categories, safe delete | Released (unsigned) |
| v0.2.0 | Instant reopen, "what changed", lower memory, finding space, UI refresh | Built and tested; release checks in progress |
| v0.3 | Phase 8: Clean up with confidence | Next |
| v0.4 | Phase 9: See it happen | Planned |
| v0.5 | Phase 10: Power tools | Planned |
| 1.0 | Signed, tested on more hardware | Planned |

## Done
- **Scanning:** MFT reader (C: ≈ 1.1 s with admin) and folder walk (≈ 4.3 s without); fastest in [our benchmark](BENCHMARKS.md).
- **Understanding:** categories, plain-English notes, safety labels, protected places.
- **The app:** tree-table, sunburst and treemap (curved labels), drive strip with % used and the top 5, start screen with drive cards, light/dark themes, keyboard navigation, Ctrl+F filter.
- **Instant reopen and "what changed"** (list and Change column).
- **Memory:** scan data for 1.9M items 204 → 143 MB.
- **Finding space:** largest files with search, file types, duplicate finder, hardlink names, CSV export, Explorer right-click.
- **Deleting safely:** Recycle Bin only, re-check by file identity, spelling-proof locks, OneDrive/Git/large/USB warnings, multi-select, deletion history.
- **Security:** admin helper that writes nothing to disk, DLL-planting defence, fuzzing, `cargo audit` + `cargo deny` in CI ([details](SECURITY-AND-PRIVACY.md)).
- **Release:** installer, portable zip, checksums, CI with a real install/uninstall test, signing-ready pipeline.

## v0.2.0: left before release
1. Check the overnight fuzzing results; fix anything found.
2. Re-run the benchmarks on an idle PC.
3. Measure Fast-scan peak memory and the first raw read with admin.
4. Re-run the full local test.
5. Bump the version, write the release notes, tag `v0.2.0`.

## Planned features (15 ideas reviewed)
Each idea was checked against the code and the rules above. "Partly" means part of it already works today.

| # | Feature | Today | Plan | Release |
|---|---|---|---|---|
| 2 | **Dependency purge** (node_modules, Rust `target`, venv…) | Partly: detected and deletable | "Mark all" on a sub-type; Rust `target` found by its `CACHEDIR.TAG`; one confirmation; shows how to recreate it | v0.3 |
| 15 | **Leftovers view** (old installers, ISOs, Windows.old) | Partly: rules exist | View → Leftovers, with the right route for each (e.g. Windows' own cleanup for Windows.old) | v0.3 |
| 3 | **More cache rules** (Adobe, Spotify, Discord/Teams, browsers, shader and package caches) | Partly | Notes and safety labels; warn if the owning app is running. Docker/WSL get guided steps instead | v0.3 |
| 6 | **Custom rules** | No | `%APPDATA%\Xylem\rules.json`, glob/extension only (no regex: too slow for 2M items). Can add categories and colours, **can never unlock protected places** | v0.3 |
| 10 | **WinSxS cleanup** | Note only | Button that starts Windows' own DISM `/StartComponentCleanup` in a visible console behind UAC | v0.3 |
| 9 | **Restore points / shadow copies** | Shown as locked | Show how much System Protection uses; open Windows' own settings to manage it. Xylem deletes nothing here | v0.3 |
| 12 | **Age heatmap** | No | Last-modified date per item (≤ +8 MB, ≤ +5% scan time, measured); a third colour mode: Category / Location / Age | v0.4 |
| 4 | **Live updates while open** | No | ReadDirectoryChangesW watcher (no admin), at most one UI update per second; "press F5" if it falls behind | v0.4 |
| 5 | **Size alerts** | No | "Tell me if a folder grows more than N GB" while the app is open. A tray mode only if users ask, and opt-in | v0.4 |
| 11 | **Leftover app data hints** | No | Match AppData folders to uninstalled apps; amber "possibly left over" with the evidence; never marked safe or selected automatically | v0.4 |
| 13 | **Export plan to PowerShell** | No | Generated script re-checks every path, uses the Recycle Bin, refuses links, quotes safely; Xylem only writes it, never runs it | v0.5 |
| 1 | **NTFS compression** | Measures compressed sizes | Windows' own `compact.exe`, user folders only, estimate first, logged, undoable | v0.5 |
| 7 | **WSL/Docker disk compaction** | Detection only | Guided: explain, check WSL/Docker are stopped, then Windows' own tool behind UAC with a final confirm | v0.5 |
| 14 | **Archive & replace** | No | Deferred: needs a careful verify-before-delete design | After 1.0 |
| 8 | **Space Shifter** (move + junction) | No | Deferred: high data-loss risk; only with copy → verify → swap → original kept in the Recycle Bin | After 1.0 |

Every feature ships with tests, a benchmark showing no scan slowdown, docs, and a line in the release notes.

## Research (in progress)
A parallel study of a faster, leaner and more secure engine. Its results slot in where they fit, ideally before v0.4 (the age heatmap changes the data layout too). Topics:
- Parse drive data outside the admin helper, so a parser bug can never run with admin rights.
- Skip unused file-table records; find out why the first raw read is slow.
- Store repeated names once; compress the remembered-scan cache.
- Delete through a handle opened by file ID, with long-path support.

## Before 1.0
- Wider testing: other PCs, an HDD, non-English names, exFAT/FAT32/ReFS drives.
- Destructive security tests on disposable machines; 24-hour fuzzing.
- Screen-reader access to the map.
- Code signing (SignPath Foundation), then 1.0.
