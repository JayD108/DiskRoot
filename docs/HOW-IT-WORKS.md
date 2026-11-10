# How Xylem works

[← Back to README](../README.md)

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

## The name

> In botany, **xylem** is the vascular tissue that pulls water up from the deepest roots to the canopy.

| Botany | Xylem |
|---|---|
| **Roots** reach into the soil | The helper drops straight into the drive: the **boot sector**, then the raw **Master File Table**, not the folder APIs. |
| **Vessels** carry sap upward | Every file is one row in flat arrays. Sizes **roll up** from children to parents in a single backwards loop. |
| **Growth rings** record structure | The sunburst is a stem cross-section: the middle is the current folder, and each ring is one level deeper. |
| **The canopy** opens to the light | Only what's visible reaches the UI: a pruned view of a few thousand shapes, never millions. |

## 1. Roots: two scanners

**MFT reader (`crates/scan-mft`)**, for admin on NTFS:
1. Read the **boot sector**: cluster size, record size, and where the MFT starts.
2. Read **record 0** (the MFT's own entry). Its run list says where each MFT fragment lives. Very fragmented MFTs are followed through extension records.
3. Read the MFT in **4 MB unbuffered chunks** on several threads through **one shared overlapped handle**.
4. **Parse** each record with bounds checks. The fixup (torn-write check) is verified, and corrupt records are skipped.
5. **Fold** extension records into their base record. **Link** each file to its parent, checking sequence numbers so stale links are rejected. Build the tree breadth-first.

Each file record becomes exactly one entry, so a hardlinked file is counted **once**, and the total matches Windows' used space.

**Folder walk (`crates/scan-walk`)**, without admin, on any drive:
- `NtQueryDirectoryFile` returns hundreds of entries with sizes per call.
- Each folder is opened relative to its parent's handle.
- 8 workers share a depth-first stack.
- Symlinks and junctions are never followed.

## 2. Vessels: the data model (`crates/core`)

`Index` is a struct of arrays indexed by a `u32` id: `parent[] size[] alloc[] files[] flags[] name_off[] name_len[]`, plus one string arena (~50 bytes per file).
- The rule **parent id < child id** makes the size roll-up one backwards loop.
- `store.rs` saves and loads the index, checking lengths, UTF-8 and the tree invariants.
- `view.rs` sends the UI a pruned view: about 6 levels, with tiny items merged into "N small items".

## 3. Meaning: categories (`crates/core/src/classify.rs` + `rules.json`)

- **Rules are data, not code.** A folder or file pattern (`node_modules`, `/Windows/WinSxS`, `AppData/Local/Temp`…) gives a category, a sub-type, a plain-English note, a safety level and a protection level.
- **One pass in id order:** a match decides a node, everything inside inherits it, and deeper matches win. About 0.1–0.2 s for 1.8 million files.
- **Extension rules** (`.mp4`, `.gguf`…) refine files inside general places such as Documents, or anywhere for strong rules (model files).
- **By-category view:** Category → Sub-type → the places (folders) holding those bytes.

## 4. Canopy: the app

- **Engine API (`crates/session`):** one `Scanned` value answers every UI question: `view`, `list`, `crumbs`, `category_view`, `cat_children`, `info`, `prepare_delete` → `recycle` → `apply_delete`.
- **The app (`crates/native`, `xylem.exe`):**
  - egui on OpenGL: one process, no browser engine, and it redraws only on input.
  - Layout: menu bar, toolbar, tree-table, map, details pane and status bar.
  - Fonts come from Windows (Segoe UI, Consolas); nothing is bundled.
  - Scans and deletes run on worker threads.
- **Design reference:** `xylem-ui-prototype/` is the HTML mock-up the look was taken from (not shipped).

## Accuracy

| Situation | What Xylem does |
|---|---|
| Hardlinks | Fast scan counts them once. The folder walk can't tell, so it counts each name. |
| Compressed / sparse / CompactOS files | Size on disk = bytes actually used |
| OneDrive online-only files | Size on disk ≈ 0, tagged as cloud |
| Symlinks / junctions | Tagged as links, never followed |
| `pagefile.sys`, `hiberfil.sys`, `$MFT`, System Volume Information | Included in Fast scan; they're real space |
