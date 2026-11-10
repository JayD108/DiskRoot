<div align="center">

# xylem

**WizTree-class speed. Something nicer to look at. It tells you what things *are*.**

A Rust disk analyzer for Windows with a native interface (no browser engine). It reads the NTFS Master File Table directly, maps a 1.5 TB drive in about a second and a half, sorts every file into categories that mean something, and tells you what's safe to clean.

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
| App download / app memory with C: loaded | **6.8 MB**, one process, ~272 MB, 0 network connections | |

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
git clone https://github.com/JayD108/Xylem.git xylem
cd xylem
cargo build --release --workspace          # Rust 1.88+, MSVC build tools

.\target\release\xylem.exe                  # the app (native UI)
.\target\release\xylem-cli.exe scan C: --top 20   # command line (MFT mode when run as admin)
```

In the app, type or pick a drive and press **Scan** (F5), or **Fast scan** (Ctrl+F5) for whole NTFS drives. The tree-table is the main view: expand folders, use the arrow keys, filter by name. The map beside it follows your selection, and clicking a ring or tile selects it. Switch **Location / Category** and **Sunburst / Treemap** in the toolbar or the View menu. The details pane says what the selected item is and whether it's safe to remove. *Move to Recycle Bin…* (or Delete, or right-click) asks first.

**Install:** download `xylem-setup-<version>.exe` from [Releases](https://github.com/JayD108/Xylem/releases), or build it yourself with `tools\release.ps1` (needs NSIS). It installs into Program Files, which protects the admin helper (see below). A portable zip is built too. The command-line tool is an optional part of the install.

CLI options: `--mode auto|mft|walk`, `--by disk|size`, `--json`, `--export all.csv`, `--threads N`.

*Screenshots: add `docs/screenshots/sunburst.png`, `category.png` and `delete.png` before the first public release.*

## How it works

```
                    ┌──────────────── xylem.exe (normal user, one process) ─────────┐
 You ── clicks ───► │  native UI (egui)  ◄── calls ──  xylem-session (engine API)    │
                    │  tree-table · map · details     Index + rules + classification │
                    └───────────────▲────────────────────────────────────┬──────────┘
                                    │ loads + validates index file       │ Fast scan
                                    │                                    ▼ (UAC prompt)
                    ┌───────────────┴─ xylem.exe --helper-scan (admin, seconds) ─────┐
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

### 4. Canopy: the apps
- **Engine API (`crates/session`):** one `Scanned` value answers every UI question: `view`, `list`, `crumbs`, `category_view`, `cat_children`, `info`, `prepare_delete` → `recycle` → `apply_delete`. The UI never holds the full tree, and the engine can be tested without it.
- **The app (`crates/native`, `xylem.exe`):**
  - egui on OpenGL: one process, no browser engine, and it only redraws on input.
  - The layout is "classic utility, modern finish": menu bar, toolbar, tree-table, map, details pane and status bar.
  - Botanical palette, light and dark themes, Segoe UI loaded from Windows (not bundled), keyboard navigation, and accessible tree rows for screen readers.
  - Scans and deletes run on worker threads, so the window never freezes.
- **Design prototype (`xylem-ui-prototype/`):** the static HTML mock-up the app's look was taken from (design reference only; not shipped).
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
- **Admin only when asked, only briefly.** The app window never runs elevated. "Fast scan" starts `xylem.exe --helper-scan C: --save-index <file>` through the Windows UAC prompt. In that mode no window is created: it scans, writes its result, and exits (measured: no elevated process left behind). It accepts only that exact command line.
- **Hardened handoff:**
  - The helper only writes a *new* file (`create_new`, so a planted file or link makes it fail rather than overwrite) named `xylem-*.idx`, directly in your temp folder.
  - The app loads it with full validation and deletes it.
  - The helper's location can't be redirected in release builds.
- **Untrusted input is parsed safely.** MFT records and index files are bounds-checked in safe Rust; `unsafe` is limited to small, commented Windows calls.
- **Fuzz-tested parsers.** The MFT record parser, run lists, the boot sector, the tree builder and the index loader each get hundreds of thousands of random and mutated inputs on every `cargo test`. Set `XYLEM_FUZZ_ROUNDS` for longer runs; 2 million rounds pass. Fuzzing found and fixed three real bugs:
  - A parent-link cycle in a corrupt MFT crashed the tree build.
  - Sizes near 2⁶⁴ overflowed the totals; they now saturate.
  - A corrupt boot sector could ask for huge buffers; the geometry is now limited to real NTFS values.
- **Deleting safely:**
  - Always to the Recycle Bin, and only after you confirm.
  - Right before the move, the engine **re-checks the item on disk**: it must still exist, still be a file or folder as scanned, and no folder on its path may have become a link (junction/symlink) since the scan. Otherwise the delete is refused, so a swapped folder can't redirect it somewhere else.
  - Windows, program folders, profile roots and the scan root itself are locked in the engine.
- **Warnings that need a tick** before deleting:
  - *OneDrive* folders: deleting syncs to the cloud and your other devices.
  - Folders containing *Git repositories*: unpushed work.
  - *Very large* deletes: 10 GB or more, or 50,000+ files.
- **Deletion history.** Each delete (recycled, cancelled or refused) is logged on your PC in `%LOCALAPPDATA%\Xylem\deletions.log`, viewable under File → Deletion history.
- **Integrity check** (shield button / Help → Security & privacy):
  - Shows whether this copy runs from Program Files (only admins can change it) and whether it carries a valid Authenticode signature (Windows' `WinVerifyTrust`, offline: no revocation lookup).
  - Release builds are signed by `tools/release.ps1` once a certificate is configured. Until then they are honestly shown as *unsigned*.
- **First-run notice:** a plain list of what Xylem reads, what it never does, and how deleting works.
- **Network:**
  - Xylem contains **no network code** and no HTTP client or browser engine in its dependencies.
  - **Measured: 0 network connections**, one process.
  - The only online step anywhere is the signing timestamp in `tools/release.ps1`, on the developer's machine, never in the app.

## Code signing policy

Windows releases will be signed with a free certificate: **free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by [SignPath Foundation](https://signpath.org/).** *(The application is pending; until it's accepted, release builds are unsigned and the app says so in Help → Security & privacy.)*

- **Builds:** every signed file is built by GitHub Actions from this repository (`.github/workflows/release.yml`), and each signing request is approved by hand. Nothing built on a personal machine is signed. Details: [docs/SIGNPATH.md](docs/SIGNPATH.md).
- **Team roles:**
  - Committers and reviewers: [JayD108](https://github.com/JayD108)
  - Approvers (approve each signing request by hand): [JayD108](https://github.com/JayD108)
- **Privacy policy:** this program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it. Xylem has no network code at all; see *Security & privacy* above.
- **Uninstalling:** the installer adds a normal Windows uninstaller (Settings → Apps). The portable zip needs no uninstall: delete the folder. Settings (`%APPDATA%\Xylem`) and the deletion log (`%LOCALAPPDATA%\Xylem`) can be deleted by hand.

## Project layout

```
xylem/
├─ crates/
│  ├─ core/        Index, store, view, classify (+ rules.json), report, format
│  ├─ scan-walk/   folder-walk scanner (Windows FFI in ffi.rs)
│  ├─ scan-mft/    MFT reader: volume.rs, record.rs, scan.rs
│  ├─ session/     engine API used by the app; elevated helper; Recycle Bin;
│  │               safety.rs (re-check, warnings, deletion log); integrity.rs (signature)
│  ├─ native/      xylem.exe: the app (egui) + helper mode
│  ├─ cli/         xylem-cli.exe: command line
│  └─ bench/       xylem-bench.exe: gen | run | time | verify | modes
├─ xylem-ui-prototype/   standalone design reference (open index.html)
├─ spikes/        throwaway experiments (native UI size spike)
├─ installer/      NSIS installer script (xylem.nsi)
├─ .github/        CI (tests, clippy) and the release pipeline (build → sign → package → test install → publish)
├─ tools/          release.ps1 (build → sign → installer → checksums), benchmark + UI-test scripts
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
- The delete re-check against a real junction swap; Git/OneDrive warnings.
- Signature checks (signed Edge vs unsigned test binary).
- Fuzz tests (`fuzz.rs` in scan-mft, `store.rs` in core).

## Roadmap

| | |
|---|---|
| ✅ 1–3 | Folder walk, MFT reader, desktop app with sunburst, treemap and admin helper |
| ✅ 4 | Categories, by-category view, notes and safety badges, Recycle-Bin delete |
| ✅ UI | Native app `xylem.exe` on a separate engine API (an earlier web UI was removed: one process, no browser engine, nothing online) |
| ✅ Release | Installer (Program Files, uninstaller, optional parts), portable zip, checksums, signing hook; delete re-check, warnings, history, integrity check, fuzzing |
| ⏭ 5 | Instant reopen (index cache + USN journal), "what changed" snapshots |
| Later | Show every name of hardlinked files; lower scan-data RAM; a real code-signing certificate; Explorer context menu |

*Xylem was called "Drishti" until 10 Nov 2026.*
