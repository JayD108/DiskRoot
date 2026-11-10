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
