<div align="center">

# xylem

**WizTree-class speed. Something nicer to look at. It tells you what things *are*.**

A Rust + Tauri disk analyzer for Windows. It reads the NTFS Master File Table directly, maps a 1.5 TB drive in about a second and a half, sorts every file into categories that mean something, and tells you what's safe to clean.

*Local only. No accounts, no telemetry, no uploads.*

</div>

---

| Measured on the author's laptop (C:, 1.53 TB used, ~1.8M MFT records) | Xylem | WizTree 4.33 |
|---|---|---|
| Full-drive scan, admin (MFT) | **1.2–1.5 s** | n/a (only measured with export) |
| Scan + export a CSV of every file, admin | **1.7–1.8 s** | 12.5–15.8 s |
| Scan without admin | **~3.3 s** | 26–30 s |
| Peak RAM, MFT scan | **~255 MB** | 844 MB |
| Total vs Windows' own "used space" (≈1,558 GB) | **1,558 GB** | 1,583 GB (counts hardlinks once per name) |
| Bytes sorted into a real category | **95%** | n/a |

WizTree was used only as a black box: run normally and timed. Its license forbids reverse engineering, and none was done.

## Why Xylem?

When an SSD fills up, you need three answers:
1. **Where** is the space going?
2. **What** is it: a game, `node_modules`, a model cache, Windows itself?
3. **What changed** since last week?

Today's tools answer the first question well and the others barely:
- **WinDirStat / TreeSize Free** are friendly and open (WinDirStat), but they walk every folder, which takes minutes on a big drive.
- **WizTree** is very fast, but it's closed source, needs a paid license for work use, and shows *where* the big files are, not *what* they are.
- **DaisyDisk** is beautiful, but Mac-only.
- None of them tells you plainly **"this is safe to delete"**, and none of them shows you what changed.

Xylem's answer:
- **Raw speed:** a short-lived admin helper reads the MFT directly (4 MB chunks, parallel parsing, one shared overlapped handle) and exits.
- **A small memory model:** 1.8M files stored as a flat struct of arrays (~50 bytes per file, no per-file objects).
- **Meaning, not just sizes:** a data-driven rule set sorts files into *Dev stuff, AI & data, Apps & Games, System, Temp & caches…* and explains what each place is.
- **Safe cleanup:** delete goes to the Recycle Bin. Windows, program folders and your profile's own folders are locked.
- **Next:** instant reopen through the USN journal, and "what changed since last scan" (see the roadmap).

## The name

> In botany, **xylem** is the vascular tissue that pulls water up from the deepest roots to the canopy, a one-way highway that keeps a tree alive.

Xylem's architecture follows the same path:

| Botany | Xylem |
|---|---|
| **Roots** reach into the soil | The helper drops straight into the drive: the **boot sector**, then the raw **Master File Table**, not the folder APIs. |
| **Vessels** carry sap upward | Every file is one row in flat arrays. Sizes **roll up** from children to parents in a single backwards loop, like sap rising. |
| **Growth rings** record structure | The sunburst *is* a stem cross-section: the pith in the middle is the current folder, and each ring is one level deeper. |
| **The canopy** opens to the light | Only what's visible reaches the UI: a pruned view of a few thousand shapes, never millions. |

## Quick start

```powershell
git clone <repo> xylem
cd xylem
cargo build --release --workspace          # Rust 1.88+, MSVC build tools

.\target\release\xylem-app.exe             # desktop app
.\target\release\xylem.exe scan C: --top 20   # command line (MFT mode when run as admin)
```

In the app, pick a drive and press **Scan**, or **Fast scan (admin)** for whole NTFS drives. Switch between **By location** and **By category**, and between **Sunburst** and **Treemap**. Click to zoom in; the center, Backspace or the breadcrumbs go back up. Use 🗑 or right-click to move something to the Recycle Bin.

CLI options: `--mode auto|mft|walk`, `--by disk|size`, `--json`, `--export all.csv`, `--threads N`.

*Screenshots: add `docs/screenshots/sunburst.png`, `category.png` and `delete.png` before the first public release.*

## How it works

```
                    ┌──────────────── xylem-app.exe (normal user) ──────────────────┐
 You ── clicks ───► │  UI (HTML/JS, Canvas 2D)  ◄── small JSON views ──  Rust backend│
                    │   sunburst · treemap · list · delete dialog       Index + rules│
                    └───────────────▲────────────────────────────────────┬──────────┘
                                    │ loads + validates index file       │ Fast scan
                                    │                                    ▼ (UAC prompt)
                    ┌───────────────┴──── xylem.exe (admin, short-lived) ───────────┐
                    │  boot sector ─► $MFT runs ─► read + parse ─► tree ─► save, exit│
                    └────────────────────────────────────────────────────────────────┘
 Without admin: the app runs the folder-walk scanner itself (no prompt).
```

### 1. Roots: two scanners
**MFT reader (`crates/scan-mft`)**, for admin on NTFS:
1. Read the **boot sector**: cluster size, record size, where the MFT starts.
2. Read **record 0** (the MFT's own entry). Its run list says where each MFT fragment lives; very fragmented MFTs are followed through extension records.
3. Read the MFT in **4 MB unbuffered chunks** on several threads through **one shared overlapped handle**. A handle per thread cost ~0.35 s *each*.
4. **Parse** each 1 KB record with bounds checks. The fixup (torn-write check) is verified, and corrupt records are skipped, never trusted.
5. **Fold** extension records into their base record, **link** each file to its parent (checking sequence numbers, so stale links are rejected), and build the tree breadth-first.

Each file record becomes exactly one entry, so a hardlinked file is counted **once**. That's why the total matches Windows' used space.

**Folder walk (`crates/scan-walk`)**, the no-admin path, works on any drive:
- `NtQueryDirectoryFile` returns hundreds of entries with sizes per call.
- Each folder is opened *relative to its parent's handle*, so Windows never re-parses the full path.
- 8 workers share a depth-first stack (more threads were measured to be slower).
- Symlinks and junctions are never followed.

`--mode auto` uses the MFT when it can, and otherwise walks folders and says why.

### 2. Vessels: the data model (`crates/core`)
`Index` is a struct of arrays indexed by a `u32` id: `parent[] size[] alloc[] files[] flags[] name_off[] name_len[]` plus one string arena.
- The rule **parent id < child id** makes the size roll-up one backwards loop.
- `from_columns` builds in bulk and **validates** the tree.
- `store.rs` saves and loads it, checking the length against the header, the UTF-8, and the invariants.
- `view.rs` sends the UI a **pruned view**: about 6 levels, with items under 0.2% merged into "N small items".

### 3. Meaning: classification (`crates/core/src/classify.rs` + `rules.json`)
- **Rules are data, not code.** A folder or file pattern (`node_modules`, `/Windows/WinSxS`, `AppData/Local/Temp`, `/Users/*/Downloads`) gives a category, a sub-type, a plain-English note, a safety level (🟢/🟡/🔴) and a protection level.
- **One pass in id order:** a match decides a node, everything inside inherits it, and deeper matches win. Rules are found by a hash of the lowercased name, and anchored rules are skipped at the wrong depth. **~0.1–0.2 s for 1.8M files.**
- **Extension rules** (`.mp4`, `.npy`, `.gguf`…) refine files only inside "weak" places (Desktop, Documents, unclassified folders), or anywhere if marked strong (model files).
- **By-category view:** Category → Sub-type → **places** (the folders holding those bytes). Sizes add up exactly.

### 4. Canopy: the app (`crates/app`)
- **Backend:** Tauri commands (`scan`, `scan_admin`, `view`, `list`, `crumbs`, `category_view`, `info`, `delete`, `reveal`). The UI never holds the full tree.
- **Frontend:** plain HTML/CSS/JS with no build step: a Canvas 2D sunburst and squarified treemap, a ranked list, tooltips with notes and safety badges, and a delete dialog.
- **Safe delete:** `SHFileOperationW` with *allow undo*, so things go to the Recycle Bin, and Windows itself warns if something is too big to recycle. Protected items are refused **in the backend**, not just hidden in the UI.

## Accuracy rules

| Situation | What Xylem does |
|---|---|
| Hardlinks | MFT mode counts once. The folder walk can't tell, so it counts each name (like other tools). |
| Compressed / sparse / CompactOS files | Size on disk = bytes actually used (including the `WofCompressedData` stream) |
| OneDrive online-only files | Size on disk ≈ 0, tagged `cloud` |
| Symlinks / junctions | Tagged `link`, never followed |
| `pagefile.sys`, `hiberfil.sys`, `$MFT`, System Volume Information | Included in MFT mode; they're real space |

## Security & privacy

- **Read-only disk access.** The volume is opened with read access only (`GENERIC_READ`, unbuffered, overlapped). The MFT code contains no write calls.
- **Admin only when asked, only briefly.** `xylem-app.exe` never runs elevated. "Fast scan" starts `xylem.exe` through the Windows UAC prompt; it scans, writes its result, and exits (measured: no elevated process left behind).
- **Hardened handoff:**
  - The helper only writes a *new* file (`create_new`, so a planted file or link makes it fail rather than overwrite) named `xylem-*.idx`, directly in your temp folder.
  - The app loads it with full validation and deletes it.
  - The helper's location can't be redirected in release builds.
- **Untrusted input is parsed safely.** MFT records and index files are bounds-checked in safe Rust; `unsafe` is limited to small, commented Windows calls.
- **UI:** file names are escaped before display, and a Content Security Policy allows only the app's own files.
- **Network:**
  - Xylem's code contains **no network code**. No HTTP client is compiled for Windows; Tauri's optional HTTP crates are for other platforms only.
  - *Honest note:* the window is drawn by Microsoft **WebView2**, which was measured opening HTTPS connections to Microsoft for its own services. Your file data never goes there. See `docs/DEVLOG.md` for the measurements and the options being considered.

## Project layout

```
xylem/
├─ crates/
│  ├─ core/        Index, store, view, classify (+ rules.json), report, format
│  ├─ scan-walk/   folder-walk scanner (Windows FFI in ffi.rs)
│  ├─ scan-mft/    MFT reader: volume.rs, record.rs, scan.rs
│  ├─ cli/         xylem.exe (also the admin helper)
│  ├─ bench/       xylem-bench.exe: gen | run | time | verify | modes
│  └─ app/         Tauri app: src/ (Rust) + ui/ (HTML/CSS/JS)
├─ xylem-ui-prototype/   standalone design prototype for the next UI (open index.html)
├─ tools/          benchmark + UI-test scripts
└─ docs/           PRD, TECHSTACK, DEVLOG (history), HANDOFF (start here)
```

## Testing

```powershell
cargo test --workspace                     # unit + integration tests
cargo clippy --workspace --all-targets     # kept at zero warnings
.\target\release\xylem-bench.exe verify "C:\Program Files"   # exact match vs a plain std::fs walk
.\tools\admin-timing.ps1                   # MFT timing with per-phase trace (admin shell)
.\tools\admin-bench.ps1                    # full admin benchmark incl. WizTree (admin shell)
```

The tests cover:
- Torn, junk and hostile MFT records; run lists; boot sectors.
- Hardlinks, orphans, stale parents.
- Junction loops and Unicode names on real folders.
- Index save/load and corruption rejection.
- Classification, protection and category totals.
- The admin helper's path guard.
- Recycle-Bin delete (an opt-in test).

## Roadmap

| | |
|---|---|
| ✅ 1–3 | Folder walk, MFT reader, desktop app with sunburst, treemap and admin helper |
| ✅ 4 | Categories, by-category view, notes and safety badges, Recycle-Bin delete |
| ⏭ 5 | Instant reopen (index cache + USN journal), "what changed" snapshots |
| Later | Show every name of hardlinked files; installer into Program Files + code signing; Explorer context menu; new UI (see `xylem-ui-prototype/`) |

*Xylem was called "Drishti" until 10 Nov 2026.*
