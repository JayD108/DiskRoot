# Tech Stack & Architecture: Visual Disk Analyzer

**Companion to:** `disk-analyzer-PRD.md` · **Status:** Proposal v1 · **Date:** 10 Nov 2026

---

## 0. TL;DR

| Layer | Choice |
|---|---|
| Scan engine | **Rust**, three scanners: raw MFT reader (admin), parallel native-API walker (no admin), USN journal (incremental updates) |
| Memory model | Flat struct-of-arrays plus one string arena, about 40–60 bytes per file |
| Persistence | Index cache plus snapshots in a custom binary format, zstd-compressed |
| UI shell | **Tauri 2** (WebView2, built into Windows 11) |
| UI framework | **SvelteKit or SolidJS** + TypeScript |
| Diagram rendering | **Custom WebGL2 renderer.** Layout is computed in Rust and only visible geometry is sent to the UI |
| Elevation | Unelevated UI plus a small **elevated helper process** over a named pipe |
| Delete | `IFileOperation` → Recycle Bin |
| CLI | Same Rust core, `clap` |

**How we beat everything:**
1. Read the MFT faster: big sequential reads plus parallel parsing.
2. **Avoid full rescans:** keep a cached index and update it from the USN journal, so reopening takes under 1 s.
3. Use about 10× less RAM than WizTree.
4. Explain the results. Each item gets a category, a safety label and a "what changed" view.

---

## Final review verdict

**The PRD is good on product and weak on engineering.** The problem, gaps and target users are correct. Three changes make it a "beat everything" plan:
1. **The fast path must be raw MFT parsing**, not `FSCTL_ENUM_USN_DATA` (which has no sizes).
2. **Never rescan:** keep a cached index plus USN journal deltas. This is the one thing WizTree, WinDirStat and Drive Sonar all lack, and it makes every launch after the first instant.
3. **Use Rust + Tauri 2 + a WebGL renderer fed by Rust-side layout**, with an elevated helper process for admin work.

Everything else (classification, safety labels, diff, HTML report) is where we win on output, because no free tool does it.

---

## WizTree on this laptop (what we can and can't learn)

**Installed:** WizTree **4.33** at `C:\Program Files\WizTree` (`WizTree.exe` 32-bit 7.4 MB, `WizTree64.exe` 11 MB, 31 locales, Inno Setup installer).

**What we can't do:** its EULA (`license.txt`) says you may not *"decompile, disassemble or reverse engineer the Software or attempt to do any such things."* So we **do not** inspect its binary (no disassembly, import/string dumps or API tracing). That's also cleaner for an open-source project: our code must be provably our own (clean-room).

**What we can learn legally:**
- **Packaging:** it's a single self-contained exe with no DLLs. It works portable and starts instantly. **We should match this**: a single Tauri exe, the helper embedded or next to it, and a portable mode.
- **Black-box benchmarking:** running it normally (allowed for personal use) gives us our baseline numbers:
  - Time a full C: scan in the GUI (admin, MFT mode) and in non-admin mode.
  - Use its documented CSV export (`WizTree64.exe "C:" /export="out.csv" /admin=1`) and **diff its per-folder sizes against ours**. That works as a free accuracy oracle for hardlinks, compression and cloud files.
  - Record peak RAM in Task Manager.
- **Public facts:** reads the MFT directly on NTFS, needs admin for that, falls back to normal enumeration otherwise. Our design (§3) already covers this, built from Microsoft's NTFS documentation and open-source parsers.

---

## 1. Deeper review of the PRD (issues to fix)

| # | Issue | Fix |
|---|---|---|
| 1 | **`FSCTL_ENUM_USN_DATA` does not return file sizes.** It returns only names, parent IDs and attributes. Getting sizes needs a second lookup per file, which is slow. | Make the **raw MFT reader** the main fast path from Phase 2. Use USN only for incremental updates. |
| 2 | **A full rescan on every launch** is how every competitor works, and it's the biggest waste of time. | Cache the index on disk and replay USN journal changes since the last run. Reopening takes under 1 s (the same idea *Everything* uses). |
| 3 | **"Faster than WizTree" is undefined.** Warm vs cold, NVMe vs SATA, file count? | Build a benchmark harness in Phase 1 (§8). |
| 4 | **Admin handling isn't designed.** If the whole app runs elevated, Explorer drag-and-drop stops working (Windows blocks drag-and-drop into elevated windows) and the attack surface grows. | Unelevated UI plus an elevated **scan helper** that only reads the MFT and streams results back. |
| 5 | **A3 (correct sizes) hides many edge cases.** The PRD misses **WOF/CompactOS-compressed files**, which are common on Windows 11 system folders, and sparse files. | See §4: the full list of size rules. |
| 6 | **The sunburst can't draw 1M nodes**, and B5 alone isn't a rendering plan. | Rust computes the layout and culls anything under ~2 px. The UI receives only a few thousand shapes (§5). |
| 7 | **Windows 11 context menu (F1).** A simple registry verb appears only under "Show more options". | Implement `IExplorerCommand` with a sparse package identity (P1, harder than it looks). |
| 8 | **Competitor claim is out of date.** *Drive Sonar* (Rust + Tauri, user mode) reports warm scans about 2× faster than WizTree and about 6× less RAM (author-run test, unsigned binaries). So "fast without admin" is already possible, and we need to beat it too. | Add it to the competitor table. Our user-mode scanner must match it. |
| 9 | **Language.** The PRD says C++. | Recommend **Rust** (§2). C++ is still workable if you prefer. |
| 10 | **No output beyond the UI.** | Add a **single-file HTML report** export and a JSON/CSV CLI. |

---

## 2. Why Rust for the core (and not C++)

- **Parsing raw on-disk binary data (MFT records) is where memory bugs happen.** A malformed or corrupted record must never crash the app or, worse, corrupt memory inside an elevated process. Safe Rust plus bounds-checked slices prevents that whole class of bugs.
- **Fearless parallelism:** `rayon` parallelises record parsing with almost no extra code.
- **Ecosystem:** `windows` (official Microsoft bindings), and NTFS/MFT crates to learn from: `ntfs` (Colin Finck), `mft`, `usn-journal-rs`, `heft-mft`.
- **Tauri is Rust-native:** core and UI backend in one language, with no FFI layer.
- Your C++/DSA skills carry over directly: the hot code is arrays, indices and cache-friendly loops.

> If you'd rather stay in C++: C++20 core + Tauri via a C ABI, or C++ + Qt Quick. Everything below except the crate names still applies.

---

## 3. Scan engine architecture

```
                 ┌──────────── elevated helper (admin) ───────────┐
 Drive  ──────►  │ Raw MFT reader → parse workers → node stream   │──┐ named pipe
                 └────────────────────────────────────────────────┘  │
                 ┌──────────── unelevated core ───────────────────┐  │
 Folder ──────►  │ Parallel NtQueryDirectoryFile walker           │◄─┘
                 │ Index (SoA) ◄── USN journal delta updater      │
                 │ Size aggregation · Classifier · Snapshot/diff  │
                 │ Layout engine (sunburst/treemap geometry)      │
                 └──────────────────────┬─────────────────────────┘
                                        │ Tauri IPC (binary)
                                   WebView2 UI (WebGL2)
```

### 3.1 Scanner 1: Raw MFT reader (fastest, needs admin)
1. Open `\\.\C:` and call `FSCTL_GET_NTFS_VOLUME_DATA` to get the MFT start cluster and the record size (usually 1 KB).
2. Read MFT record 0 (the `$MFT` file itself) and parse its `$DATA` **run list** to find every MFT fragment on disk.
3. **Pipeline:**
   - One I/O thread issues large reads (4–16 MB, overlapped, `FILE_FLAG_NO_BUFFERING`, 2–4 in flight).
   - N parser threads (`rayon`) process the chunks in parallel. MFT records are independent of each other, so this parallelises well.
4. For each in-use record:
   - Apply the **fixup array** (update sequence).
   - Read `$STANDARD_INFORMATION`, every `$FILE_NAME` (parent ID plus name; prefer Win32 names over DOS 8.3 names), and every `$DATA` (real size, allocated size, named streams = ADS).
   - Handle `$ATTRIBUTE_LIST` records whose attributes spill into extension records.
5. Build the tree with one O(n) pass using the parent file reference numbers. Directory totals come from one bottom-up pass.

**Rough math (target, not measured):** 1M files ≈ 1 GB of MFT. On NVMe (~3 GB/s) the read takes about 0.3 s, and parsing is spread across cores. **Goal: under 2 s cold for 1M files on NVMe, under 1 s warm.**

### 3.2 Scanner 2: Parallel user-mode walker (no admin, non-NTFS, single folders)
- Call `NtQueryDirectoryFileEx` with the `FileIdExtdDirectoryInformation` class and big buffers (64–256 KB). One call returns hundreds of entries with sizes, attributes and file IDs.
- Use a **work-stealing pool** with one directory per task, plus thread count tuned per drive type (SSDs like 8–16 threads, HDDs like 1–2).
- Fall back to `FindFirstFileExW` with `FIND_FIRST_EX_LARGE_FETCH` + `FindExInfoBasic` on non-NTFS drives (exFAT/FAT32/network).
- Target: match or beat Drive Sonar's reported warm-scan figures.

### 3.3 Scanner 3: USN journal (incremental updates, our biggest advantage)
- On exit, save the index plus the journal ID and last USN.
- On the next launch, load the index (memory-mapped, under 200 ms) and call `FSCTL_READ_USN_JOURNAL` from the saved USN. Then apply creates, deletes, renames and size changes.
- If the journal wrapped around or its ID changed, do a full rescan.
- This also powers **live updates** while the app is open (PRD A5 comes almost for free).

### 3.4 Data model (memory)
Struct-of-arrays, with indices instead of pointers:

```
parent:    Vec<u32>      // index into the same arrays
first_child / next_sibling: Vec<u32>
size:      Vec<u64>      // logical
alloc:     Vec<u64>      // on disk
name_off:  Vec<u32>      // into one UTF-16 or UTF-8 string arena
name_len:  Vec<u16>
category:  Vec<u8>
flags:     Vec<u8>       // dir, hardlink, cloud, compressed, sparse, reparse, system
frn:       Vec<u64>      // NTFS file ID (for USN + diff)
```
That's about 40–60 bytes per node including the name, so **1M files ≈ 50–80 MB**. WizTree was measured at about 635 MB in the same comparison.

---

## 4. Size correctness rules (PRD A3)

| Case | How to detect | Rule |
|---|---|---|
| Hardlinks | More than one `$FILE_NAME` / link count > 1 | Count bytes **once** (against the first path); show a "hardlinked" badge on the others |
| Cloud-only (OneDrive) | Reparse tag `IO_REPARSE_TAG_CLOUD_*`, `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS` | Size on disk = allocated (often 0); show a "cloud" badge |
| NTFS compression / sparse | Allocated size < logical size | Show both sizes; sort by **size on disk** by default |
| **WOF / CompactOS** | Reparse tag `IO_REPARSE_TAG_WOF`, data in `::$WofCompressedData` stream | Size on disk = that stream's allocated size |
| Alternate data streams | Named `$DATA` attributes | Add to the file total; list them in the details panel |
| Symlinks / junctions | Reparse points | **Never follow** (avoids double counting and loops) |
| System files | `pagefile.sys`, `hiberfil.sys`, `swapfile.sys`, `$MFT`, System Volume Information | Show them (big!), label 🔴, explain them |
| Tiny resident files | Data stored inside the MFT record itself | Size on disk = 0 |

---

## 5. UI and rendering (the "beautiful" part)

### Why Tauri 2 plus a custom WebGL renderer
- **Polish:** a web UI gives the best typography, animation and theming for the effort, which is what makes DaisyDisk look good.
- **Small and light:** Tauri uses Windows' built-in WebView2, so installers are a few MB, not 100+ MB like Electron.
- **Speed:** the UI never holds the full tree. Rust computes **only the visible geometry**: arcs and rectangles bigger than ~2 px, about 2k–20k shapes. Those go over Tauri IPC as a **binary buffer** (not JSON) and are drawn by WebGL2 (instanced quads and arcs). That keeps the UI at 60 fps on any laptop.
- Alternative if the webview ever becomes the bottleneck: **egui + wgpu** (fully native GPU). It is faster in raw drawing but much harder to make look good. Benchmark both in Phase 3 week 1 before committing.

### Views
| View | Notes |
|---|---|
| **Sunburst** (default) | About 5 visible rings, animated zoom, breadcrumb, "N small items" grouping |
| **Treemap** | Squarified layout (Bruls et al.), cushion shading like WinDirStat but modern |
| **Icicle** | Horizontal bars, best for deep folder trees |
| **Category mode** | Same diagram, with the hierarchy regrouped as Category → Sub-type → Folder → File |
| **Diff mode** | Colour = grew (red) / shrank (green) / new (blue) since the last snapshot |

### Libraries
- `d3-hierarchy`/`d3-scale` only as a layout reference. The real layout is ported to Rust so it runs off the UI thread.
- `@tanstack/virtual` for the ranked side list (handles millions of rows).
- Fonts: one variable font (e.g. Inter) bundled offline.

---

## 6. Classification and explanations ("what is it?")

- **Rule engine in Rust.** Rules are data files (TOML/JSON) in the repo, so the community can add rules without touching code:
  ```toml
  [[rule]]
  match = "**/node_modules"          # glob on path
  category = "dev.node"
  safety = "green"
  explain = "npm packages. Safe to delete; `npm install` restores them."
  action = "Delete folder (project will need npm install)"
  ```
- **Fast matching:** an Aho-Corasick automaton over path components plus an extension → category table. Classification runs once per directory and is inherited by children, so it is O(n) for the whole drive.
- **Built-in packs:** Windows system, dev tools (Node, Python, Gradle/Android, Rust target/, .NET obj/bin, Docker/WSL vhdx), AI (Hugging Face, Ollama, PyTorch hub, LM Studio), games (Steam, Epic), browsers, media.
- **Per-app grouping (C4):** map known paths to apps, e.g. `%LOCALAPPDATA%\Google\Chrome` + Program Files + cache → "Chrome".
- **Safety labels are conservative:** unknown items are never 🟢. 🔴 paths cannot be deleted from the app at all.

---

## 7. Snapshots, history and output ("what changed?" plus better output)

- **Snapshot file:** a columnar dump of (frn, parent, alloc size, path hash) plus per-folder totals, **zstd**-compressed. About 5–15 MB per 1M files. Keep the last N.
- **Diff:** join on FRN, falling back to path hash. Gives grew/shrank/new/deleted at folder level in well under 1 s.
- **Timeline chart (E3):** drive totals and per-category totals from each snapshot.
- **Output beyond the app:**
  - **Self-contained HTML report:** one file with an interactive sunburst and top lists, so you can send it to someone. No competitor offers this for free.
  - CLI: `xylem scan C: --json | --csv | --top 50 | --diff last`
  - **"Reclaimable now" summary:** "You can safely free 23.4 GB: node_modules 9.1 GB, npm cache 3.2 GB, Temp 2.8 GB…"

---

## 8. Benchmark harness (build in Phase 1)

- Test drives: a real C: drive, plus a synthetic folder generator (1M small files, deep trees, hardlinks, ADS, sparse, compressed).
- **Cold runs:** reboot, or empty the standby list with Sysinternals RAMMap. **Warm runs:** take the median of 5.
- Compare against WizTree, WinDirStat, TreeSize Free and Drive Sonar on the same machine.
- Record: time to first result, time to full result, peak RAM, and accuracy against `fsutil`/Explorer.
- Keep it in CI as a regression gate so later changes can't make scanning slower.

---

## 9. Targets (update PRD §8)

| Metric | WizTree (reported) | **Our target** |
|---|---|---|
| Cold scan, 1M files, NVMe, admin | seconds | **< 2 s** |
| Reopen app (cached index + USN) | full rescan | **< 1 s** |
| User-mode warm scan | n/a (needs admin) | **≤ Drive Sonar** |
| Peak RAM, 1M files | ~635 MB (one test) | **< 150 MB** |
| UI frame rate | n/a | **60 fps** at any zoom |
| Bytes classified (non-"Other") | none | **> 90%** |

---

## 10. Project layout

```
xylem/
├─ crates/
│  ├─ core/        # index, aggregation, classifier, snapshot, diff, layout
│  ├─ scan-mft/    # raw MFT reader (used by helper)
│  ├─ scan-walk/   # parallel NtQueryDirectoryFile walker
│  ├─ usn/         # journal reader + delta apply
│  ├─ helper/      # elevated helper .exe (MFT → pipe)
│  ├─ cli/         # clap CLI
│  └─ bench/       # harness + synthetic tree generator
├─ app/            # Tauri 2 + Svelte/Solid UI, WebGL renderer
└─ rules/          # classification + explanation packs (TOML)
```

Key crates: `windows`, `rayon`, `crossbeam`, `memmap2`, `zstd`, `aho-corasick`, `globset`, `serde`, `clap`, `criterion`, `tauri`.

---

## 11. Revised build phases

1. **Phase 1:** Rust workspace, parallel user-mode walker, top-50 text output, **benchmark harness**.
2. **Phase 2:** raw MFT reader in the elevated helper, size rules (§4), accuracy tests against Explorer.
3. **Phase 3:** Tauri UI, renderer spike (WebGL vs egui, 1 week), sunburst + treemap + list.
4. **Phase 4:** classifier + category mode + safe delete. **← MVP**
5. **Phase 5:** index cache + USN delta (instant reopen), snapshots/diff, explanations/safety badges.
6. **Phase 6:** HTML report, CLI, Explorer menu, code signing.

> Optional reorder: move the **index cache + USN** step into the MVP. "Opens instantly every time" is the most noticeable win over WizTree.

---

## 12. Setup needed on this PC (not yet installed)
- Rust via `rustup` (MSVC toolchain)
- Visual Studio 2022 **Build Tools** with the "Desktop development with C++" workload (needed by Rust on Windows and by Tauri)
- Node.js: already installed (v24)
- WebView2: built into Windows 11

---

## Sources
- [FSCTL_QUERY_FILE_LAYOUT (Microsoft)](https://learn.microsoft.com/windows-hardware/drivers/ifs/fsctl-query-file-layout)
- [FSCTL_GET_NTFS_FILE_RECORD (Microsoft)](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_get_ntfs_file_record)
- [NtQueryDirectoryFileEx (Microsoft)](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntquerydirectoryfileex)
- [FILE_ID_EXTD_DIR_INFORMATION (NtDoc)](https://ntdoc.m417z.com/file_id_extd_dir_information)
- [About WizTree](https://diskanalyzer.com/about)
- [WizTree MFT scans explained (WindowsForum)](https://windowsforum.com/news/wiztree-fast-ntfs-mft-scans-explained-admin-setup-folder-modes-and-windirstat.447023/)
- [WinDirStat vs WizTree vs Drive Sonar benchmarks (DEV)](https://dev.to/barun_saha/windirstat-vs-wiztree-vs-drive-sonar-which-free-disk-space-analyzer-should-you-use-in-2026-56lo)
- [Best Windows Disk Space Analyzers 2026 (FolderSizes, vendor)](https://www.foldersizes.com/compare)
- [ntfs crate (Colin Finck)](https://de.github.com/ColinFinck/ntfs) · [mft crate](https://lib.rs/crates/mft) · [usn-journal-rs](https://docs.rs/usn-journal-rs) · [heft-mft](https://docs.rs/crate/heft-mft/0.2.1)
- [Tauri vs Iced vs egui performance (L. Kalbertodt)](https://lukaskalbertodt.github.io/2023/02/03/tauri-iced-egui-performance-comparison.html)
