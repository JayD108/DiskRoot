# PRD: Visual Disk Analyzer for Windows

**Owner:** Mrittunjay Dubey · **Status:** Draft v1 · **Date:** 10 Nov 2026
**Name:** Xylem (renamed from the working name "Drishti" on 10 Nov 2026)

---

## 1. One-line pitch
**WizTree-level speed + DaisyDisk-level beauty + it tells you *what* each thing is and *what changed*.** Free and open source, for Windows.

## 2. Problem
When the C: drive fills up, you want answers to 3 questions:
1. **Where** is the space going? (which folders/files)
2. **What** is it? (game? node_modules? AI model cache? Windows system stuff?)
3. **What changed?** (what grew since last week?)

Today's tools mostly answer only #1. The fast ones look dated, and the pretty ones are slow, paid or Mac-only. Almost none answer #2 and #3 for free.

---

## 3. What exists today, and the gaps

| Tool | Good at | Weaknesses (our opportunity) |
|---|---|---|
| **WizTree** (Win) | Fastest scans, because it reads the NTFS MFT directly | Closed source. Free only for personal use; commercial use needs a paid supporter code. The speed trick works only on NTFS drives. UI is functional but not modern. Doesn't show per-app sizes. No history or "what changed" view. |
| **WinDirStat** (Win) | Free, open source, treemap, actively updated again (v2.x, dark mode added Jan 2026) | Slower than MFT-based scanning. Dated look. Only a treemap (no sunburst). No automated cleanup. Windows only. |
| **TreeSize Free** (Win) | Clean folder tree, treemap | Search, duplicates and most exports are locked to paid editions. Snapshot compare is a paid feature. The free edition ignores some NTFS features (hardlinks, alternate data streams, dedup), so sizes can differ from Explorer. |
| **SpaceSniffer** (Win) | Live zoomable treemap, filters | Closed source. Reviewers call the treemap cluttered or overwhelming. English only. |
| **DaisyDisk** (Mac) | Best-looking sunburst UI | Paid and Mac only. Doesn't tell you whether something is safe to delete. Doesn't track changes made outside the app. |
| **Filelight** (KDE, Win/Linux) | Free sunburst ("rings") | Basic: no file-type classification, no history, no explanations. |
| **SquirrelDisk** (Rust/Tauri) | Open source, sunburst + treemap, cross-platform | Author says it's not fully stable. Unsigned binaries trigger Windows warnings. Some users find the sunburst hard to read. |
| **ncdu / gdu / dust** (terminal) | Fast, scriptable, great over SSH | Text only, no visual diagram. |
| **Windows Settings → Storage** | Built in | Vague buckets like "Other" hide the real culprit. |
| **New wave** (DiskBrain, SpaceRover, FreshDisk, Delta) | Snapshots, "AI explains this folder" | Mostly paid or proprietary, or early-stage. Free tiers are capped (e.g. a few AI calls/day, a few snapshots). |

### Common gaps (summary)
1. **Speed vs beauty:** fast tools look old, pretty tools are slow, paid or Mac-only.
2. **Location, not meaning:** tools show *folders*, not *categories* (dev cache, game, AI model, system).
3. **No "what is this / is it safe?":** users are scared to delete unknown folders.
4. **No time dimension:** "what grew since last week" is mostly paid.
5. **Licensing:** the fastest tool isn't open source or free for work use.
6. **One visualization only:** a dense treemap overwhelms beginners.
7. **Accuracy traps:** hardlinks, cloud-only (OneDrive) placeholder files, compression and ADS make sizes wrong.

---

## 4. Target users
- **Primary:** students and developers on Windows laptops with small SSDs (256–512 GB), full of node_modules, Android/Gradle caches, venvs, Hugging Face models and games.
- **Secondary:** normal home users who just want "free up 20 GB safely".

## 5. Goals and non-goals
**Goals**
- Full-drive scan in seconds on NTFS (MFT-based).
- Beautiful, interactive hierarchy diagram (sunburst + treemap).
- Every file is auto-classified into categories.
- Plain-language "what is this?" plus a safety label for known folders.
- Snapshot history: "what changed since last scan".

**Non-goals (v1)**
- Not a "PC cleaner" or registry cleaner. No auto-deleting.
- No macOS/Linux in v1 (Windows first).
- No cloud account or telemetry.

---

## 6. Features

**Priority:** P0 = must have in MVP · P1 = next version · P2 = later

### A. Scanning engine
| # | Feature | Priority |
|---|---|---|
| A1 | Fast NTFS scan by reading the MFT (the hidden table listing every file on the drive) | P0 |
| A2 | Fallback normal folder scan for non-NTFS drives (USB/exFAT) and single folders | P0 |
| A3 | Correct sizes: count hardlinks once, cloud-only files as 0 on disk, show both "size" and "size on disk" | P0 |
| A4 | Show results progressively while scanning | P1 |
| A5 | Live update via the USN journal (Windows' log of file changes), so no full rescan is needed | P2 |

### B. Visual UI (the main differentiator)
| # | Feature | Priority |
|---|---|---|
| B1 | **Sunburst view**: center = drive, rings = folder levels, click to zoom in, breadcrumb to go back | P0 |
| B2 | **Treemap view** (rectangles sized by space), switchable with B1 | P0 |
| B3 | Side panel: ranked list of the biggest folders/files, synced with the diagram | P0 |
| B4 | Hover tooltip: size, % of parent, file count, category | P0 |
| B5 | Small slices grouped into "N small items" so the chart stays readable | P0 |
| B6 | Dark/light theme, smooth zoom animations | P1 |
| B7 | Icicle/flame view (horizontal bars, easier for deep folders) | P2 |

### C. Classification and hierarchy (answers "what is it?")
| # | Feature | Priority |
|---|---|---|
| C1 | Auto-classify everything into **categories**: Apps & Games · Media (video/photo/music) · Documents · **Dev stuff** (node_modules, .git, build/, venv, Gradle/Android SDK) · **AI models** (Hugging Face/Ollama caches) · System (WinSxS, pagefile, hibernation, restore points) · Temp & caches · Downloads · Cloud-synced · Other | P0 |
| C2 | **Two hierarchy modes** in the same diagram: *By location* (Drive → Folder → File) and *By category* (Category → Sub-type → Folder → File) | P0 |
| C3 | Rule file (JSON) for classification, so new rules can be added without code changes | P1 |
| C4 | Per-app grouping: "Chrome uses 4.2 GB across 3 locations" | P1 |
| C5 | Duplicate file finder (size first, then hash) | P2 |

### D. Explain and stay safe (answers "can I delete this?")
| # | Feature | Priority |
|---|---|---|
| D1 | Built-in knowledge base: a plain-English note for known folders, e.g. "WinSxS = Windows component store, don't delete manually" | P1 |
| D2 | Safety badge: 🟢 safe to clear · 🟡 check first · 🔴 system, don't touch | P1 |
| D3 | Delete goes to the Recycle Bin by default; protected system paths are locked | P0 |
| D4 | Suggested actions such as "Clear npm cache" or "Run Disk Cleanup", shown as suggestions only and never automatic | P2 |
| D5 | Optional AI "explain this unknown folder" (opt-in, sends only the path name, never file contents) | P2 |

### E. History (answers "what changed?")
| # | Feature | Priority |
|---|---|---|
| E1 | Save a lightweight snapshot after each scan (local file) | P1 |
| E2 | "Changes since last scan" view: grew / shrank / new, highlighted on the diagram | P1 |
| E3 | Timeline chart of drive usage over time | P2 |

### F. Extras
| # | Feature | Priority |
|---|---|---|
| F1 | Right-click "Analyze this folder" in Explorer | P1 |
| F2 | Export to CSV/JSON | P1 |
| F3 | CLI mode (`tool scan C: --json`) | P2 |

---

## 7. MVP (version 1 scope)
**A1, A2, A3, B1, B2, B3, B4, B5, C1, C2, D3.**
In short: a fast scan, a sunburst/treemap, categories and safe delete. That alone beats WinDirStat on looks and classification, and comes close to WizTree on speed.

## 8. Success metrics (targets, to be measured)
- Scan of ~1M files on an SSD: **under 10 s** (MFT mode).
- RAM: **under 500 MB** for 1M files.
- UI stays at 60 fps while zooming.
- Category accuracy: over 90% of bytes land in a non-"Other" category on a typical dev laptop.

## 9. Suggested tech (open decision)
- **Core engine:** C++ (Win32 API: `DeviceIoControl` + `FSCTL_ENUM_USN_DATA` first, raw MFT parsing later). This fits your C++/DSA strength.
- **UI options:**
  1. C++ core as a DLL + **C# WinUI/WPF** UI (easiest polished Windows UI)
  2. C++ + **Qt** (single language)
  3. Rust/C++ core + **Tauri** (web-tech UI with D3.js for the sunburst; SquirrelDisk uses this approach)
- **Data model:** a flat array of nodes (id, parentId, size, categoryId) instead of pointer trees. This is faster and lighter in memory.

## 10. Risks
- **Admin rights** are needed for MFT reading. Fall back to the slow scan without them.
- **Raw MFT parsing is hard** (binary NTFS format). Build in phases.
- **Wrong "safe" labels could cause data loss.** Keep D2 conservative and always use the Recycle Bin.
- **Unsigned .exe** triggers SmartScreen warnings. Plan code signing later.

## 11. Build phases
1. **Phase 1:** slow recursive scan + text output of the top 50 folders (learn the basics).
2. **Phase 2:** fast USN/MFT enumeration + sizes.
3. **Phase 3:** sunburst + treemap UI + list panel.
4. **Phase 4:** classification + category mode. **← MVP done**
5. **Phase 5:** snapshots/diff, explanations, safety badges.

---

## Sources
- [WizTree EULA](https://diskanalyzer.com/eula)
- [WizTree vs WinDirStat: pros and cons (AppMus)](https://appmus.com/vs/wiztree-vs-windirstat)
- [WizTree pricing and limits (TechYorker)](https://techyorker.com/products/wiztree/)
- [WinDirStat updates thread (MalwareTips)](https://malwaretips.com/threads/windirstat-updates-thread.133269/post-1188297)
- [TreeSize Free knowledge base (JAM Software)](https://knowledgebase.jam-software.com/treesize_free)
- [FolderSizes vs TreeSize](https://www.foldersizes.com/features/treesize)
- [SpaceSniffer (Wikipedia)](https://en.wikipedia.org/wiki/SpaceSniffer)
- [DaisyDisk tips (official guide)](https://daisydiskapp.com/guide/4/en/TipsAndTricks/)
- [DaisyDisk user discussion (MPU forum)](https://talk.macpowerusers.com/t/daisy-disk-is-so-dead/44609)
- [SquirrelDisk (GitHub)](https://github.com/adileo/squirreldisk)
- [SquirrelDisk (OpenAlternative)](https://openalternative.co/squirreldisk)
- [Filelight (KDE)](https://apps.kde.org/filelight)
- [du vs ncdu vs gdu vs dust](https://blog.frytea.com/en/posts/note/1545)
- [DiskBrain](https://diskbrain-webpage.pages.dev/)
- [SpaceRover](https://spacerover-web.pages.dev/)
- [FreshDisk (AlternativeTo)](https://alternativeto.net/software/freshdisk/about/)
