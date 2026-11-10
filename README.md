# Drishti: a fast, visual disk analyzer for Windows

*(Drishti is a working name.)* Drishti answers the questions people ask when their drive is full: **where** is the space going, **what** is it, and *(soon)* **what changed**. It scans a whole drive in about a second and draws the result as an interactive sunburst or treemap.

| On the author's laptop (C:, 1.56 TB used, 1.77M files) | Drishti | WizTree 4.33 |
|---|---|---|
| Full scan + CSV export, as admin | **1.7–1.8 s** | 12.5–15.8 s |
| Peak RAM | **290 MB** | 844 MB |
| Scan without admin | **~3.6 s** | 26–30 s |
| Total vs Windows' own "used space" (1,558.2 GB) | **1,558.0 GB** | 1,583 GB (counts hardlinks repeatedly) |

WizTree was used only as a black box: run normally and timed. Its license forbids reverse engineering, and none was done.

---

## 1. Quick start

```powershell
cd "C:\Users\Mrittunjay Dubey\Storage\drishti"
cargo build --release --workspace

.\target\release\drishti-app.exe                  # the desktop app
.\target\release\drishti.exe scan C: --top 20     # command line
```

In the app, pick a drive and press **Scan**. For a whole NTFS drive, press **Fast scan (admin)**: Windows asks for permission once, then the scan takes about a second.

The command line also offers:
- `--mode auto|mft|walk`
- `--by disk|size`
- `--json`
- `--export all.csv`
- `--threads N`

## 2. The big picture

```
                    ┌──────────────── drishti-app.exe (normal user) ────────────────┐
 You ── clicks ───► │  UI (HTML/JS, Canvas)  ◄── small JSON views ──  Rust backend   │
                    │   sunburst · treemap · list                    holds the Index │
                    └───────────────▲────────────────────────────────────┬──────────┘
                                    │ loads index file                   │ "Fast scan"
                                    │                                    ▼ (UAC prompt)
                    ┌───────────────┴─── drishti.exe (admin, short-lived) ──────────┐
                    │  MFT reader: raw disk ─► parse records ─► tree ─► save index   │
                    └────────────────────────────────────────────────────────────────┘
 Without admin:  the app itself runs the folder-walk scanner (no prompt).
```

There are three steps, each in its own crate:
1. **Scan:** read the file system as fast as possible into a compact tree (`scan-walk`, `scan-mft`).
2. **Model:** store the tree flat and small, with sizes rolled up into every folder (`core`).
3. **Show:** send the UI only what's visible right now, and draw it (`core::view` plus `app`).

## 3. How scanning works

### 3a. Folder walk (`crates/scan-walk`): works everywhere, no admin needed
This is the classic approach: list a folder, then list each of its subfolders.

Three tricks make it fast:
- **Big batches:** `NtQueryDirectoryFile` returns hundreds of entries per call, with size and size on disk included, so there's no extra call per file. It uses the info class without legacy 8.3 short names, which are slow to look up.
- **Relative opens:** each subfolder is opened *relative to its parent's already-open handle* (`NtOpenFile` + `RootDirectory`), so Windows never re-parses the full path from `C:\`.
- **8 worker threads with a depth-first work stack.** More threads were measured to be *slower* on this CPU (8 fast + 16 efficiency cores, plus NTFS lock contention).

Only workers touch the disk. A single builder thread owns the tree, so it needs no locks. Symlinks and junctions are never followed, so there are no loops or double counting. OneDrive folders are still scanned.

### 3b. MFT reader (`crates/scan-mft`): the fast path, needs admin and NTFS
NTFS keeps a table called the **Master File Table (MFT)**: one 1 KB record per file, holding its name, parent folder and sizes. Reading that table front to back is much faster than asking Windows about each folder.

1. **Boot sector** (first 512 bytes of the drive): cluster size, record size, and where the MFT starts.
2. **Record 0 is the MFT's own entry.** Its *run list* says where every fragment of the MFT sits on disk. If the MFT is very fragmented, extra fragments are listed in "extension records", and those are followed too.
3. **Read** the MFT in 4 MB chunks with several threads through **one** shared overlapped volume handle, bypassing the system cache.
   - A handle per thread cost ~0.35 s *each*, which was the biggest lesson of Phase 2.
4. **Parse** each record, all bounds-checked; corrupt or torn records are skipped, never trusted:
   - Apply the *fixup*: NTFS stamps a check value at the end of every 512 bytes.
   - `$FILE_NAME`: name and parent. When there are several names, the Win32 name wins over the DOS 8.3 one.
   - `$DATA`: size, and size on disk, including compressed, sparse and CompactOS (`WofCompressedData`) files.
   - `$STANDARD_INFORMATION`: attributes. `$REPARSE_POINT`: links and cloud placeholders.
5. **Fold extension records** into their base record. Big files can spread their attributes over several records.
6. **Link children to parents.** A link is accepted only if the parent is a folder *and* its sequence number matches, so stale links to deleted folders are rejected.
7. **Build the tree breadth-first from record 5** (the root): first a pass over compact integer arrays to assign ids, then a sequential fill of the columns.

Each file record becomes exactly one entry, so a **hardlinked file is counted once**. That's why the total matches Windows' used space exactly, while tools that list every name count those files twice.

`--mode auto` uses the MFT for whole NTFS drives when it has admin rights, and otherwise falls back to the folder walk and says why.

## 4. The data model (`crates/core`)

`Index` is a **struct of arrays**, one array per field, indexed by a `u32` node id:

```
parent[]  size[]  alloc[]  files[]  flags[]  name_off[]  name_len[]   + one big names string
```

- **Far smaller than a tree of objects.** There are no per-node allocations or pointers: about 40–60 bytes per file.
- **One rule makes everything simple: a parent's id is always smaller than its children's.** Rolling sizes up into folders is then a single backwards loop (`compute_totals`).
- `size` is the logical size; `alloc` is the size on disk. The UI ranks by size on disk, because that's what deleting frees.
- `flags`: folder, link, cloud placeholder, compressed, sparse, unreadable.

Other pieces in `core`:
- `Index::from_columns` builds an index in bulk and **validates** the parent and name invariants instead of trusting them.
- `store.rs` saves and loads the index (binary). This is how the admin helper hands its result to the app. Loading is fully validated: it checks the header against the file length, the UTF-8 names, and the tree invariants.
- `view.rs`:
  - `Children` holds sorted child lists.
  - `build_view` is the **pruned view**: a few levels deep, with items under 0.2% merged into "N small items". The UI draws thousands of shapes, never millions.
- `report.rs` produces the top-N lists; `format.rs` turns numbers into "1.52 TB" and "1,234".

## 5. The app (`crates/app`)

**Backend (`src/main.rs`):** Tauri commands the UI calls:

| Command | What it does |
|---|---|
| `scan` | Scans a path |
| `scan_admin` | Runs the elevated helper |
| `view` | Returns the pruned diagram data |
| `list` | Returns ranked children, plus the total count |
| `crumbs` | Returns breadcrumbs |
| `reveal` | Shows an item in Explorer |
| `list_drives`, `initial_path` | Support start-up |

**Frontend (`ui/`):** plain HTML, CSS and JS with no build step and no external downloads.
- **Sunburst:** the center is the current folder; each ring is one level deeper. Angles are proportional to size on disk.
- **Treemap:** squarified layout (rectangles kept close to square), nested, with a header bar per folder.
- Hit-testing turns the mouse position into the item under it, which drives the hover highlight, tooltip, click-to-zoom and list sync.
- Colors: each top-level branch gets a hue, and deeper levels and siblings get small shade shifts.

**Admin helper flow (`src/elevate.rs`):**
1. The app picks a random name `%TEMP%\drishti-<pid>-<time>.idx`.
2. It runs `drishti.exe scan C: --mode mft --save-index <that file>` through `ShellExecuteExW("runas")`, which shows the UAC prompt, and waits.
3. It loads the file, checking its length against the header, then deletes it.

## 6. Accuracy rules

| Situation | What Drishti does |
|---|---|
| Hardlinks (one file, several names) | MFT mode counts it once. The folder walk can't tell, so it counts each name (like other tools). |
| Compressed / sparse / CompactOS files | Size on disk = bytes actually used |
| OneDrive "online-only" files | Size on disk ≈ 0; flagged `cloud` |
| Symlinks / junctions | Shown with a `link` badge, never followed |
| Tiny files stored inside the MFT | Size on disk = 0 |
| `pagefile.sys`, `hiberfil.sys`, `$MFT`, System Volume Information | Included (MFT mode); they are real space |

## 7. Security model
- **The app never runs as admin.** Only the short-lived helper does, and only after you approve the UAC prompt.
- **The helper writes only where it should:**
  - `--save-index` accepts only a `drishti-*.idx` file directly in your temp folder.
  - It creates the file *new* (`create_new`), so a planted file or link makes it fail rather than overwrite something.
- **The helper's location can't be redirected.** It's always `drishti.exe` next to the app; the `DRISHTI_HELPER` override exists only in debug builds.
  - Remaining gap: if the app folder is writable by normal users, someone could swap the helper. The fix is an installer into Program Files plus code signing.
- **All parsing of disk data and index files is bounds-checked** and validated in safe Rust, and corrupt input is rejected.
- **The UI:**
  - Escapes every file name before inserting it as HTML.
  - Runs under a strict Content Security Policy that only allows the app's own files.
  - Tauri exposes only the commands listed above.

## 8. Project layout

```
drishti/
├─ crates/
│  ├─ core/        Index, store (save/load), view (pruning), report, format, win (attribute flags)
│  ├─ scan-walk/   folder-walk scanner (Windows FFI in ffi.rs, logic in walker.rs)
│  ├─ scan-mft/    MFT reader: volume.rs (disk), record.rs (parsing), scan.rs (pipeline + tree)
│  ├─ cli/         drishti.exe (also the admin helper)
│  ├─ bench/       drishti-bench.exe: gen | run | time | verify | modes
│  └─ app/         Tauri app: src/ (Rust backend), ui/ (HTML/CSS/JS), icons/
├─ tools/          benchmark + UI-test scripts (see below)
└─ docs/           PRD.md, TECHSTACK.md, DEVLOG.md (history), HANDOFF.md (start here)
```

## 9. Testing and benchmarking

```powershell
cargo test --workspace                     # unit + integration tests
cargo clippy --workspace --all-targets     # lint (kept at zero warnings)
cargo test --release -p drishti-scan-mft -- --ignored --nocapture   # tree-build speed benchmark

.\target\release\drishti-bench.exe verify "C:\Program Files"   # exact match vs a plain std::fs walk
.\target\release\drishti-bench.exe run C: --runs 5             # timing
.\target\release\drishti-bench.exe modes C:                    # MFT vs walk, folder by folder (admin)
.\tools\admin-bench.ps1                                        # all admin benchmarks + WizTree (admin shell)
```

**What the tests cover:**
- Torn, junk and malicious MFT records; run lists; boot sectors.
- Hardlinks, orphans, stale parents, extension records.
- Junction loops and Unicode names on a real temp folder.
- Index save/load and corruption rejection; the view-pruning math.
- The admin helper's path guard.

**UI checks without touching the mouse:**
- `tools/ui-screenshot.ps1` launches the app and captures only its window.
- `tools/ui-click.ps1` and `tools/ui-setpath.ps1` press buttons and type paths through Windows UI Automation.

## 10. Roadmap

| Phase | Status |
|---|---|
| 1. Fast folder walk + bench harness | Done |
| 2. MFT reader | Done |
| 3. Desktop app (sunburst, treemap, list, admin helper) | Done |
| 4. Categories (Dev, AI models, Games, System…), "by category" view, safe delete | In progress |
| 5. Instant reopen (USN journal), "what changed" snapshots, safety badges | Planned |

Known limits:
- In MFT mode, the other names of a hardlinked file aren't listed yet.
- MFT-mode RAM is 290 MB.
- No installer or code signing yet.
