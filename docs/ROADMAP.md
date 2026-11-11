# Roadmap

[← Back to README](../README.md)

**Rule for every step:** never slower than today, and every change keeps the constraints in [FEATURE-PROPOSALS.md](FEATURE-PROPOSALS.md#the-constraints-every-feature-must-keep) (read-only scanning, careful changes, least privilege, offline, OSI-only components).

## Done
- **Scanning:** MFT reader (C: ≈ 1.1 s with admin) and folder walk (≈ 4.3 s without); fastest in [our benchmark](BENCHMARKS.md).
- **Understanding:** categories, plain-English notes, safety labels, protected places.
- **The app:** tree-table, sunburst and treemap (curved labels), drive strip with the top 5, start screen with drive cards, light/dark themes, keyboard navigation.
- **Instant reopen and "what changed"** (list and Change column).
- **Memory:** scan data for 1.9M items 204 → 143 MB.
- **Finding space:** largest files with search, file types, duplicate finder, hardlink names, CSV export, Explorer right-click.
- **Deleting safely:** Recycle Bin only, re-check by file identity, spelling-proof locks, OneDrive/Git/large/USB warnings, multi-select, deletion history.
- **Security:** admin helper that writes nothing to disk, DLL-planting defence, fuzzing, `cargo audit` + `cargo deny` in CI ([status](SECURITY-STATUS.md)).
- **Release:** installer, portable zip, checksums, CI, SignPath-ready pipeline ([signing](SIGNPATH.md)).

## Next
- **Phase 8, "Clean up with confidence":** dependency purge, leftovers view, more cache rules, custom rules, Windows-native cleanups (DISM, System Protection).
- **Phase 9, "See it happen":** age heatmap, live updates, size alerts, orphaned-AppData hints.
- **Phase 10, "Power tools":** export to PowerShell, NTFS compression, guided WSL/Docker disk compaction.

Details and the reasoning for each: [FEATURE-PROPOSALS.md](FEATURE-PROPOSALS.md).

## Before 1.0
- Wider testing: other PCs, an HDD, non-English names, exFAT/FAT32 drives.
- Destructive security tests on disposable machines; 24-hour fuzzing.
- Code signing (SignPath Foundation), then 1.0.
