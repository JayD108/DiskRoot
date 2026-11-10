# Dev log

A running record of what was asked, what was decided, and what was built and measured, so no context is lost between sessions. **Append a new dated section every session.** Newest entries go at the bottom.

Machine: Core Ultra 9 275HX (8 P-cores + 16 E-cores), NVMe SSD, Windows 11, C: = 2 TB NTFS with ~1.56 TB used and ~1.77M MFT records. Toolchain: Rust 1.99.1 (MSVC), Tauri 2.12.5, Node 24 (not used by the build).

---

## 2026-11-10: Session 1 (Phases 0 to 3)

### Planning
- **Ask:** read `PRD.md` and review it.
  - **Findings:** `FSCTL_ENUM_USN_DATA` returns no file sizes, correct sizes (A3) are underestimated, the admin model was missing, and a UI stack hadn't been chosen.
- **Ask:** deeper research plus a stack that beats every existing tool.
  - **Result:** `TECHSTACK.md`: a Rust core, a raw MFT reader in an elevated helper, a USN-journal index cache, and a Tauri 2 UI.
- **WizTree inspection:** v4.33 is installed. Its EULA forbids reverse engineering, so it is used **only as a black-box benchmark**.

### Phase 1: user-mode scanner
- Installed Rust with `winget install Rustlang.Rustup`. VS Build Tools (C++) were already present.
- Workspace crates: `core` (struct-of-arrays Index), `scan-walk` (parallel `NtQueryDirectoryFile`), `cli`, `bench`.
- First real C: scan: 15.4 s cold, 4.88 s warm, 138 MB RAM. Exact match against std::fs on `C:\Program Files`.
- **Snag:** Smart App Control sometimes blocks freshly built unsigned exes (os error 4551). Retrying worked, and later builds ran.

### Speed round 1 (folder walk)

| Change | Warm time |
|---|---|
| Baseline | 4.88 s |
| 12 threads | 4.25 s |
| `FileFullDirectoryInformation` (no 8.3 names) | 4.02 s |
| `NtOpenFile` relative to the parent handle + LIFO stack | 3.98 s |
| 8 threads (default) | 3.61 s |

More threads were slower: P/E cores plus NTFS contention.

### WizTree head-to-head (both writing a full CSV)

| Mode | WizTree | xylem |
|---|---|---|
| User | 26–30 s, 790 MB | 4.0–4.9 s, 117 MB |
| Admin | 15–18 s, 844 MB | 4.1–6.4 s, 165 MB (still folder walk at that point) |

Accuracy vs WizTree: 279/309 top-level folders exact. The 12 GB gap was System Volume Information plus NTFS metafiles.

### Phase 2: MFT reader (`crates/scan-mft`)
- Boot sector → `$MFT` record 0 → extents (with attribute-list support) → 4 MB unbuffered reads → bounds-checked record parsing with fixups → extension-record merge → parent links with sequence check → BFS.
- First version: 3.37 s. Time grew linearly with threads because **each raw-volume handle costs ~0.35 s**.
- **Fix: one shared overlapped handle**, which brought it to 1.15–1.27 s.
  - About 0.36 s of that is the first raw MFT read (likely NTFS flushing its cache, which can't be avoided).
  - Read+parse runs at ~3.2 GB/s, the SSD's limit.
- **Accuracy:** MFT on-disk total 1,557.98 GB vs Windows used space ~1,558.2 GB. The folder walk says 1,563.9 GB and WizTree 1,583 GB, because both count hardlinks once per name.
- Head-to-head (admin, CSV): **xylem 1.68–1.82 s, 290 MB vs WizTree 12.5–15.8 s, 844 MB**.

### Speed round 2
- Synthetic `build_tree` benchmark (1.77M records, random parents): 0.705 s → **0.209 s**.
  - BFS is now split into an id pass over the compact CSR plus a sequential column fill.
  - `Index::from_columns` and `with_capacity` were added.
- Real C: scan inside the app: 1.29 s.

### Phase 3: Tauri app (`crates/app`)
- Vanilla HTML/JS/CSS UI with no build step: Canvas 2D sunburst, squarified treemap, side list, breadcrumbs, tooltip.
- Rust sends only a pruned view: depth ≤ 6, items under 0.2% grouped, at most 120 children per folder.
- Admin fast scan: `ShellExecuteExW("runas")` runs `xylem.exe scan C: --mode mft --save-index <temp>`, then the app loads the validated index.
- Verified with window-only screenshots plus UI Automation (`tools/ui-*.ps1`): sunburst, treemap, the admin flow end to end (1.29 s), and the folder-only error toast.
- Fixed:
  - The full-ring seam line.
  - Sibling colors were too similar.
  - The view-switch accessibility role (tabs → toggle buttons).
  - The list's scroll position wasn't reset.

### Review pass (before Phase 4)
- **Security, elevated helper:**
  - `--save-index` uses `create_new` (no overwrite through planted links).
  - The path must be `xylem-*.idx` directly in `%TEMP%`.
  - The app uses a random file name and loads with a header-vs-file-length check.
  - The `XYLEM_HELPER` override works in debug builds only.
- **UI:**
  - A navigation sequence token, so stale responses can't overwrite newer ones.
  - The list says "+ N smaller items not listed" when cut at 400.
- Test scripts moved from the session scratchpad into `tools/`. The project is now a git repo, and `docs/` holds the PRD, tech stack, this log and HANDOFF.

## 2026-11-10: Session 2 (Phase 4 finish, rename, audit, UI prototype)

### Phase 4 finished
- Extension-classified files are now grouped **by folder**. Datasets showed "1014 more places"; it now shows real folders.
- File counts for categories and sub-types.
- Stats keep plain arrays for the common case, plus a map only for mixed places: 0.18 s → ~0.09 s.
- Verified in the app on a throwaway `%TEMP%\xylem-demo` project:
  - The delete dialog shows the path, size, *Dev stuff › node_modules*, a 🟢 badge and the note.
  - Confirming moved `node_modules` to the Recycle Bin and left `notes.txt` untouched.
- Fixed:
  - The header counts weren't live after a delete.
  - "1 files" plural.
  - Short `MRITTU~1` roots (now expanded with `canonicalize`).

### Renamed Drishti → Xylem
- Folder `Storage/xylem`, crates `xylem-*`.
- Exes `xylem.exe`, `xylem-app.exe`, `xylem-bench.exe`.
- Tauri identifier `com.xylem.app`.
- Helper files `xylem-*.idx`, env vars `XYLEM_TRACE` / `XYLEM_HELPER`, index magic `XYLMIDX1`.
- 35 tests pass.

### Re-verified end to end under the new name
- Walk: C: median 3.26 s; `verify` exact on Program Files.
- MFT in the app via UAC:
  - All of C: in category view, 95% classified.
  - The helper exited, and no `xylem-*.idx` was left behind.
- MFT timing (5 runs): median **1.48 s**. Read+parse 0.53 s (unchanged), build 0.11–0.25 s (faster), peak RAM **255 MB** (was 290), index 90 MB (was 126).
  - "extents" (the first raw read) is 0.71–0.80 s, up from 0.36. That code is unchanged; it's likely the NTFS metadata flush, which depends on how busy the disk is.
  - One in-app run showed 5.04 s right after a build. **Open question:** can that flush cost be reduced?

### Security audit facts (for the Security.md discussion)
- The volume is opened `read(true)` + NO_BUFFERING + OVERLAPPED. The MFT code has no write APIs.
- `unsafe` blocks: core 0, scan-walk 5, scan-mft 7, app 3 (all FFI).
- `reqwest` and `hyper` are in Cargo.lock as **non-Windows-only** Tauri dependencies. `cargo tree --target x86_64-pc-windows-msvc` shows nothing, so they aren't compiled on Windows.
- **Runtime network measurement:** the WebView2 browser process keeps **2 HTTPS connections to Microsoft** (2603:1046::/32, Azure). They stay even with `--disable-background-networking --disable-component-update`, which are confirmed in its command line. Xylem's code makes none.
- **WebView2 memory:** ~248 MB private across 6 processes; xylem-app itself ~9 MB.

### UI prototype (design.md)
- `xylem-ui-prototype/` (index.html, styles.css, app.js): standalone, no build, no network.
- The plant-tissue theme:
  - Stem cross-section sunburst with a pith and growth rings.
  - Unfurl animation once, and a "sap rising" scan bar.
  - Vessel texture done in pure CSS.
  - Inspector panel; consent and shield modals with corrected (honest) copy.
- Switches: `?nowelcome`, `?still`. Rendered with headless Edge to verify.

### README
- Rewritten to merge the naming story with the real architecture.
- Inaccurate claims from naming_and_understanding.md corrected: WebGL → Canvas 2D; FSCTL volume data → boot sector; Aho-Corasick → hashed rule lookup; "zero-allocation"; USN instant reopen is planned, not built; "1.7 s" was scan+export.

### Tooling note
- A screen-region capture once grabbed the user's browser. `ui-screenshot.ps1` now uses PrintWindow (app pixels only, no focus steal).
