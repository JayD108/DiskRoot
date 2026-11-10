# Competitor research

[← Back to README](../README.md) · Researched 11 Nov 2026 from public sources (vendor pages, release notes, READMEs). Speed figures marked "claims" are the makers' own and **not verified by us**; only WizTree 4.33 has been measured against Xylem so far, on one laptop.

## The short version
- **Fast MFT scanning is no longer rare.** WizTree, TreeSize Free, WinDirStat 2.5+ (an "advanced" option), SizeTrend, eDirStat, Fast WinDirStat and several new small open-source projects all read the NTFS Master File Table. "Fast" alone is not a crown any more.
- **Closest technically: eDirStat.** Rust + egui, an MFT scanner, BLAKE3 duplicate finder, snapshots, MIT licence. It claims to be "up to 2.5x" faster than WizTree.
- **Closest in purpose: SizeTrend** (closed source, free + Professional). It explains common space consumers, points to safe cleanup tools, finds duplicates and shows "what changed", but unlimited comparisons are Professional only.
- **What Xylem combines that we found in no single competitor:** open source + MFT speed + plain-English "is it safe to delete" for known places + free "what changed" (list + change column) + hardened delete safety (re-check by file ID, spelling-proof locks, refusal of decoy names, OneDrive/Git warnings) + a window that never runs as admin, with a small elevated helper that writes nothing to disk + zero network (measured) + fuzz-tested parsers.

## Windows tools

| Tool | Licence / price | Scanning | Explains items / safe-to-delete | What changed | Duplicates | Notes |
|---|---|---|---|---|---|---|
| **Xylem** | Free, open source (MIT/Apache) | MFT via small elevated helper (~1.2–1.5 s for C:, measured); folder walk without admin (~4 s) | Yes: categories, notes, safety labels, locks | Yes, free: list + tree column | Yes (opt-in, BLAKE3, hardlink-aware) | Window never elevated; 0 network (measured); signing pending |
| **WizTree** | Free for personal use; paid supporter licence for business | MFT (fastest-known reference) | No | Export/re-import CSV to compare | No | Closed source; EULA bans reverse engineering |
| **WinDirStat 2.x** | Free, open source (GPL) | Folder walk; **MFT scanning option since 2.5 (Jan 2026)** | No | "Folder tracking history" (2.5) | Yes (since 2.0) | Big revival since Oct 2024; dark mode, cleanups |
| **TreeSize Free / Pro** | Free / paid (Pro) | **Free uses MFT** on local NTFS; Pro up to 32 threads elsewhere | No | Pro: snapshot comparison, incl. Windows shadow copies | Pro | Strong in business/network use |
| **SizeTrend** | Free + Professional (closed) | Reads NTFS/FAT32/exFAT metadata directly | **Yes**: overview of common consumers with explanations; points to safe cleanup tools | **Yes** (Free: one automatic comparison; Pro: unlimited) | Yes | New (2023–26); MCP integration for AI assistants |
| **eDirStat** | MIT source; binaries sold on itch.io | Work-stealing walker; MFT on Windows (admin) | No | Snapshots (.edst) | Yes (BLAKE3, hardlink-aware) | Rust + egui; claims up to 2.5x vs WizTree 4.31, 53.7x vs WinDirStat 1.1 |
| **Fast WinDirStat** | Open source fork | MFT | No | No | — | Claims ~50x vs WinDirStat (unverified); repo looks inactive |
| **FolderSizes** | Paid | Fast walker (multi-thread) | Reports (largest, oldest, temp…) | Pro: snapshots, trend analyzer, capacity planning | Yes | Business-oriented |
| **SpaceSniffer** | Free (closed) | Folder walk | No | No | No | Treemap, portable |
| **Canopy** | GPL, new (v1.0, 0★) | MFT via USN enumeration (admin) | No | No | No | C++/WPF |
| Others (GitHub, 2025–26) | Mostly tiny new projects | Many claim MFT scanning | Some add "cleanup suggestions" | — | Some | e.g. rust-dirstat, SpaceLens, custodian, heft, Diskoryx |

## Other platforms (for design reference)
- **DaisyDisk** (macOS, paid): sunburst map, collect-then-delete. **GrandPerspective** (macOS, open source): treemap.
- **dua-cli, dust, gdu, pdu** (command line, open source): fast parallel walkers; no MFT reading, no GUI.

## What this means for Xylem's claims
- **OK to say:** "~7x faster than WizTree 4.33 in our test (full scan + CSV of every file on a 1.5 TB drive: 1.7 s vs 12.5 s)", with the method linked.
- **Not OK yet:** "the fastest". eDirStat claims to beat WizTree, WinDirStat now has an MFT mode, and TreeSize Free uses the MFT. None of these were measured by us.
- **Strongest honest positioning:** *the open-source disk analyzer that tells you what's safe to delete and what changed — at MFT speed, offline, with a security design you can audit.*

## Next: a fair benchmark
Same PC, same drive, warm cache, 5 runs each, median, time to a complete scan (and separately scan + full export where the tool supports it):
Xylem (MFT + walk), WizTree, WinDirStat 2.8 (MFT option), TreeSize Free, eDirStat, SizeTrend Free. Publish the script and raw numbers in `docs/BENCHMARKS.md`.

## Sources
- WinDirStat 2.5 (MFT, dark mode): https://alternativeto.net/news/2026/1/windirstat-2-5-adds-dark-mode-ntfs-mft-scanning-improved-search-and-new-file-actions · 2.0: https://alternativeto.net/news/2024/10/windirstat-releases-first-update-in-19-years-with-many-new-features-and-improvements
- TreeSize features: https://www.jam-software.com/treesize/features.shtml · snapshots: https://manuals.jam-software.com/treesize/EN/snapshots.html · storage losses: https://www.jam-software.com/treesize/understand-storage-losses.shtml
- WizTree: https://diskanalyzer.com/about · licences: https://diskanalyzer.com/donate · what's new: https://www.diskanalyzer.com/whats-new
- SizeTrend: https://www.dimastr.com/sizetrend/ · review: https://techdirectarchive.com/2026/08/27/sizetrend-a-fast-windows-disk-space-analyzer-for-storage-analysis/
- eDirStat: https://docs.rs/crate/edirstat/2.0.0
- Fast WinDirStat: https://www.idealsoftware.com/opensource/fast-windirstat.html
- FolderSizes snapshots/trends: https://www.foldersizes.com/support/help/snapshot-comparison · https://www.foldersizes.com/screens/trend-analyzer
- Canopy: https://github.com/nathannncurtis/canopy
- Command-line tools: https://docs.rs/crate/dua-cli/2.39.0/source/README.md · https://github.com/dundee/gdu/blob/v5.27.0/README.md · https://docs.rs/crate/parallel-disk-usage/0.24.0/source/README.md
- macOS: https://appmus.com/vs/daisydisk-vs-grandperspective
