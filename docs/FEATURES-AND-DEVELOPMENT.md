# Features and development

[← Back to README](../README.md) · How the engine works inside: [HOW-IT-WORKS.md](HOW-IT-WORKS.md) · Safety details: [SECURITY-AND-PRIVACY.md](SECURITY-AND-PRIVACY.md)

This page explains every feature of Xylem 0.2: **what it does, how to use it, and what happens underneath.** The second half is for people who want to build or change Xylem.

**Contents**
1. [Scanning](#1-scanning)
2. [Seeing where the space goes](#2-seeing-where-the-space-goes)
3. [Understanding what things are](#3-understanding-what-things-are)
4. [Change over time](#4-change-over-time)
5. [Finding space](#5-finding-space)
6. [Cleaning up](#6-cleaning-up)
7. [Power tools](#7-power-tools)
8. [Settings, files and shortcuts](#8-settings-files-and-shortcuts)
9. [Build from source](#9-build-from-source)
10. [Command line](#10-command-line)
11. [Tests](#11-tests)
12. [Releases](#12-releases)
13. [Project layout](#13-project-layout)

---

## 1. Scanning

### Scan (F5): no admin needed
- **What it does:** lists every folder and file under the drive or folder in the path box, with sizes.
- **Use it:** type or pick a path (top-left), press **Scan** or F5. Works on any drive: NTFS, FAT32, exFAT, USB sticks, network shares.
- **Underneath:** 8 worker threads ask Windows for whole folder listings at once (`NtQueryDirectoryFile`), hundreds of entries per call, each with its size, attributes and last-modified time. Each folder is opened relative to its parent, so long paths don't matter. Links (junctions, symlinks) are recorded but never followed, so nothing is counted twice through a loop. A full 1.5 TB system drive takes about 4–5 s.
- **Limits:** folders your account can't read are counted as "unreadable" (shown in the status bar). Hardlinked files are counted once per name.

### Fast scan (Ctrl+F5): whole NTFS drives, with admin
- **What it does:** reads the drive's own file table directly. About 1 s for a whole 1.5 TB drive, and it sees everything, including system folders and space Windows uses for itself.
- **Use it:** pick a drive (C:\) and press **Fast scan**. Windows asks for admin rights; click Yes.
- **Underneath:**
  1. Xylem starts a short-lived copy of itself as admin (the *helper*), with only one job.
  2. The helper opens the drive for reading, opens a private pipe back to the window, and then **gives up its admin power** (no privileges, Low integrity, no child processes) before it reads any data.
  3. It reads the Master File Table in 4 MB blocks on several threads, turns ~2 million records into a tree, sends it through the pipe and exits.
  4. The window (never admin) checks everything it receives before using it.
- **Limits:** NTFS only. For safety, release builds only do this from an installed copy (Program Files), not from a portable copy in Downloads.

### Cancel and progress
- A running walk shows how many items it found so far. Fast scan has a **Cancel** button and a 10-minute limit.

---

## 2. Seeing where the space goes

### The tree-table
- **What it shows:** each folder's size on disk, a bar and percentage of its parent, the number of files, the category, and (after a rescan) how much it changed.
- **Use it:** click the triangle or press → to open a folder, ← to close it. Double-click opens too. **Ctrl+F** filters by name.
- **Underneath:** the tree only builds rows for open folders, and at most 2,000 children per folder (the rest are summed into "+ N smaller items"). So it stays instant even on folders with 100,000 files.

### The map: sunburst or treemap
- **Sunburst:** the current folder in the middle, each ring one level deeper, each slice as wide as its share of the space. Labels curve along the rings.
- **Treemap:** nested rectangles, area = size.
- **Use it:** click a slice to go into it, click the middle (or press Backspace) to go up. Hover for details. Switch in the toolbar or the View menu.
- **Underneath:** the engine sends the map a *pruned view*: about 6 levels deep, with anything under 0.2% merged into "N small items". The map never draws millions of shapes, and its layout is cached until the window size or the folder changes. It only redraws when you move the mouse or something changes.

### Group by location or by category
- **Location:** the folder tree as it is on disk.
- **Category:** Category → sub-type → the places holding those bytes, e.g. *Dev stuff → node_modules → every node_modules folder, biggest first.* The bytes always add up exactly.

### Colour by category or by age
- **Category colours:** Apps & Games, Media, Documents, Dev stuff, AI & data, System, Temp & caches, Downloads, Cloud-synced, Other.
- **Age colours (View → Colour by age):** warm for recently changed, cool for old. The buckets are this month, 1–6 months, 6–12 months, 1–2 years, 2–5 years, and older. A folder takes the colour of the **newest** thing inside it, so a cool folder means nothing in it changed for a long time. The legend shows how many bytes fall in each age.
- **Underneath:** both scanners already receive each file's last-modified time; Xylem keeps it as a day number in 2 bytes per item (about 4 MB for 2 million items).

### The drive strip and the start screen
- The strip at the bottom shows how full the drive is (used / total, %) and the 5 biggest items in the scan as coloured blocks; click one to jump there.
- The start screen shows every drive with a fill bar.

---

## 3. Understanding what things are

### Categories and notes
- **What it does:** every file gets a category and a sub-type (about 60 sub-types), and known places get a plain-English note, e.g. *"node_modules: JavaScript dependencies of a project. Safe to delete; 'npm install' brings them back."*
- **Underneath:** 119 folder/file rules, 92 file extensions and marker files, all in a data file (`rules.json`) compiled into the program. One pass over the whole tree classifies ~2 million items in about 0.15 s: a matching rule decides a folder, everything inside inherits it, deeper rules win.
- **Marker files:** a folder holding `CACHEDIR.TAG` (the standard mark that build tools such as Rust's cargo put in their output folders) is recognised as *Build output*.

### Safety labels
Every known place has one of:
- **Safe to clear:** caches and build output that programs recreate.
- **Check first:** probably fine, but it may hold something you want (Downloads, IDE data).
- **Don't delete by hand:** Windows, installed programs, restore points. Xylem refuses to delete these and points you to the right tool.
- **Unknown:** no rule yet. **Your rule:** classified by your own custom rule.

### Protected places
Windows, Program Files, ProgramData, user profile roots, the Recycle Bin, NTFS metadata and similar are **locked**: Xylem won't delete them or any folder that contains them. Windows and Program Files are also locked by their file identity, so another way of writing the path (a `subst` drive, a short 8.3 name, a junction) doesn't get around it.

### Custom rules (File → Custom rules)
- **What it does:** add your own categories, e.g. every `Projects\*\out` folder as *Dev stuff → My build output*, or `.blend` files as *Media → 3D scenes*.
- **Use it:** **Edit rules** creates `%APPDATA%\Xylem\rules.json` with an example and opens it in Notepad; **Reload** applies it to the current scan straight away.
- **Limits on purpose:** your rules can set a category, a sub-type and a note, nothing else. They never apply where a built-in rule matches or inside a protected place, so they can't unlock anything. Fields like `protect` or `safety` are refused with an error. At most 500 rules, 256 KB.

---

## 4. Change over time

### Instant reopen
- **What it does:** when you start Xylem, your last scan appears straight away (a whole 1.5 TB drive in about 0.3–0.4 s), labelled with its age. Scan again for fresh numbers.
- **Underneath:** after every scan, a background thread saves the scan (the same compact format the admin helper uses) and a list of folder sizes into `%LOCALAPPDATA%\Xylem\cache`. Up to 8 drives or folders are remembered. Scans saved by Xylem 0.1 still open (without dates).

### What changed (View → What changed since the previous scan)
- **What it does:** after a rescan, lists the folders that grew or shrank, e.g. `projects\app\node_modules +612 MB`, and shows a **Change** column in the tree.
- **Underneath:** the two folder-size lists are compared. Xylem reports the deepest folder where a change really happened (at least 1 MB or 0.1% of the total), not every parent above it.

### Live updates (View → Live updates, on by default)
- **What it does:** while Xylem is open, sizes follow what happens on disk: a file that grows or is deleted updates its folders within about a second. New files are counted in the status bar ("3 new since the scan · F5 to include").
- **Underneath:** Windows' change notifications (`ReadDirectoryChangesW`) for the scanned folder, no admin. Xylem re-measures only the paths Windows reports, at most once per second. If Windows reports more changes than it could list, Xylem says "press F5" instead of guessing.
- **Limits:** new items join the tree at the next scan. A changed file's size on disk is rounded to 4 KB until then.

### Size alerts (right-click a folder → Alert me if it grows by …)
- **What it does:** tells you when a folder grows by 1, 5, 10 or 50 GB. You get a message and the taskbar button flashes; the alert then re-arms at the new size.
- **Underneath:** checked after every scan and every live update while Xylem is open. **There's no background process**: when Xylem is closed, nothing runs. View → Size alerts lists and removes them.

---

## 5. Finding space

### Largest files & file types (View menu)
- The 100 biggest files on the drive, with a search box, and sizes per file extension. Click a file to show it in the tree. One pass over the scan, nothing is read from disk.

### Leftovers (View → Leftovers)
Things that are often left behind, biggest first:
- **Installers, archives and disk images** (`.exe .msi .zip .7z .iso`…) inside Downloads folders, with their age.
- **Windows upgrade leftovers:** `Windows.old`, `$WINDOWS.~BT`, `$Windows.~WS` and similar. Windows removes these itself, so Xylem offers **Open Storage settings** instead of deleting them.
- **Possibly left over app data:** big folders in AppData or ProgramData that no installed or running program matches and that haven't changed for 90+ days. **These are hints only:** the evidence is shown, and they're never marked or labelled safe, because AppData can hold settings or saved games.
- **Mark the downloads for review** marks only the Downloads items, for one confirmation.

### Duplicate finder (View → Find duplicate files)
- **What it does:** finds identical files and how much space the extra copies use. **Mark extra copies** keeps the original-looking copy of each set (not named "copy" or "(1)", not in a backup or Downloads folder) and marks the rest.
- **Underneath:** the only feature that reads file contents, and only when you start it:
  1. Files of the same size (from the scan; no file opened yet).
  2. Hardlinks (one file with several names) are recognised and never counted as copies.
  3. A quick BLAKE3 hash of the first and last 64 KB.
  4. A full BLAKE3 hash only for files still matching.
- Online-only cloud files are skipped (reading them would download them).

### CSV export and Explorer integration
- **File → Export to CSV** writes every file and folder to `Documents\Xylem`.
- The installer adds **Analyze with Xylem** to a folder's right-click menu in Explorer.

---

## 6. Cleaning up

### Move to Recycle Bin
- **Use it:** select an item and press **Move to Recycle Bin…** or Delete. A dialog shows what will be freed and any warnings.
- **Underneath, in order:**
  1. **Refused right away:** protected places, folders containing them, the scan root, and names Windows could silently change into another file (ending in a dot or space, device names like `CON`, paths of 260+ characters).
  2. **When the dialog opens:** Xylem records the file identity (volume + file ID) of the item and of every folder up to the scan root.
  3. **When you confirm:** it re-opens every folder on the way and checks those identities again, and keeps the folders open so nothing can rename or swap them, until Windows has moved the item to the Recycle Bin.
  4. **Logged:** recycled, cancelled or refused, with size and time (File → Deletion history).
- **Warnings that need a tick:** OneDrive folders (deleting also deletes in the cloud), Git repositories inside (unpushed work), very large deletes (10 GB+ or 50,000+ files), USB or network drives (may have no Recycle Bin), and **a running program that uses the place** (e.g. Spotify for its cache).

### Several at once
- **Ctrl+click** or **Space** marks items; **Delete** reviews all marked items in one confirmation. Each item gets the full checks above, and items inside another marked folder are skipped.

### Mark all (for sub-types)
- **What it does:** on a sub-type in the Category view (right-click, or the button in the details pane), e.g. *node_modules* or *Build output*, marks **every place a "safe to clear" rule matched**, then you review them in one dialog.
- **Underneath:** it only marks folders where a safe rule matched the folder itself. It never marks a folder that merely holds files of a type, so marking *Video* can never mark your Desktop.

### Windows' own cleanups
Some places are cleaned by Windows, not by deleting files. The details pane offers a button for each:

| Place | Button | What runs |
|---|---|---|
| `C:\Windows\WinSxS` | Clean up with Windows… | `Dism.exe /Online /Cleanup-Image /StartComponentCleanup` (admin, in its own window; Xylem waits and reports) |
| System Volume Information | System Protection… | Windows' System Protection settings (restore points) |
| `Windows.old`, upgrade files | Open Storage settings… | Settings → System → Storage |

---

## 7. Power tools

All opt-in, and all done by Windows' own tools.

### Export a cleanup plan to PowerShell (File menu)
- **What it does:** turns the marked items into a script, e.g. to clean another PC or to run later: `Documents\Xylem\cleanup <time>.ps1`.
- **Underneath:** without `-Run` it only lists what it would do. With `-Run`, each item goes to the Recycle Bin after a check: it must still exist, be the same type, and neither it nor any folder above it may be a link. Paths are written so PowerShell never interprets them, and unusual names are left out and listed. **Xylem itself never runs the script.**

### Compress (folders)
- **What it does:** shrinks files on disk while they stay usable as normal (same place, names and dates; Windows unpacks them as they're read). Good for programs, games, code, logs and documents you mostly read. Photos, videos, archives and most model weights barely shrink.
- **Use it:** select a folder → **Compress…** (details pane or right-click). Pick **Fast** (default) or **Maximum**; it shows what will be packed, what is left alone and a rough estimate. A real progress bar shows speed, time left and space saved, with **Stop**. **Uncompress** undoes it.
- **Underneath:** Xylem's own engine (`crates/session/src/compress.rs`) drives Windows' file compression directly (the format `compact.exe /EXE` writes): Fast = XPRESS16K, Maximum = LZX, several files at once (cores − 2, at most 16). It skips packed formats, data that looks random, files of one cluster, and files that are already compressed, sparse, links or cloud placeholders. Each file is opened read-only and locked against changes; files in use are skipped; Windows refuses files you may not change. No admin. Not offered for protected places, cloud folders or whole drives; NTFS only. A file that's changed later is stored uncompressed again. Measured on 763 MB of mixed data: compact.exe LZX 30.9 s; engine Fast 1.7 s, Maximum 6.1 s (same savings as compact.exe LZX).

### Ways to save space
- **What it does:** one window with two lists, worked out from the scan in well under a second (no file is opened):
  - **Compress:** folders and big files that haven't changed or arrived for 1 month, 3 months, 6 months (default) or a year and would save at least 100 MB, biggest saving first. The highest such folder is listed, not everything inside it. Each row shows the rough saving with Fast, the size on disk and the age of the newest file; **Compress…** opens the usual dialog with the suggested level (Maximum for things untouched for a year, otherwise Fast).
  - **Other ways:** caches and temporary files, build folders (`node_modules`, `target`, venvs) in projects not changed for 6 months, AI model stores, the Recycle Bin, the hibernation file, previous Windows and upgrade files, restore points, big files that won't compress and haven't changed for a year (move them), WSL/Docker disks (Compact…), OneDrive files kept on the PC, likely folder copies, installers in Downloads, duplicates, WinSxS and Compact OS. Each says how; some open the right tool or let you mark the safe places for the usual delete review.
- **Use it:** **Save space** in the toolbar, or **View → Ways to save space…**. Nothing is changed or deleted from this window.
- **Underneath:** `crates/session/src/savings.rs`. "Not changed" is when the newest file inside last changed **or arrived on this PC** (the later of last-modified and created): a download or unpacked dataset keeps its author's modified date, often years older than the download. Folders' own dates are ignored, and Windows doesn't keep reliable last-opened dates, so Xylem never says "unused". Both scanners read the arrival date at no extra cost; saved scans gained it in index format v3 (older ones still load, without it). Uses the same per-file rules as the Compress dialog, so both agree. A folder named "cache" inside your own files is never offered as safe. Estimates come from measured ratios where we have them (LZX: `.npy` 12.7%, `.npz` 13.9%, `.pt` 6.9%, `.edf` 37.8%, `.parquet` 11.8%); other files assume 50% (40% for programs).

### Similar folders
- **What it does:** lists folders that look like copies (same size, file count and names inside; contents aren't compared) and folders with similar names (same name ignoring case, separators, "- Copy" and versions anywhere; name families like `APALA`, `APALA_dev` next to each other).
- **Use it:** **View → Similar folders…**; click a folder to show it in the tree. Xylem never deletes from this view.
- **Underneath:** `crates/session/src/similar.rs`, on the scan in memory (about 0.03 s for a 1.7M-item drive). Windows, Program Files, AppData and package/build caches are not looked into.

### Compact a WSL or Docker virtual disk (`.vhdx` files)
- **What it does:** WSL and Docker disks grow but never shrink by themselves. Compacting gives the unused space back.
- **Use it:** select the `.vhdx` file → **Compact virtual disk…**. The dialog guides you: free space inside first, stop WSL (**Shut down WSL** button) or quit Docker, then **Compact (admin)**.
- **Underneath:** Xylem first checks nothing has the disk open. Then an admin copy of Xylem (`--helper-compact-vhdx`) checks the path again and gives Windows' `diskpart` its commands through a pipe: attach the disk **read-only**, compact, detach. It reports the size before and after.

---

## 8. Settings, files and shortcuts

| Key | Action |
|---|---|
| F5 / Ctrl+F5 | Scan / Fast scan |
| ↑ ↓ | Move in the tree |
| → ← / Enter | Open / close a folder (← on a closed item goes to its parent) |
| Backspace | Map: up one level |
| Delete | Move to Recycle Bin (the marked items if any are marked) |
| Space / Ctrl+click | Mark / unmark |
| Ctrl+F | Filter the tree by name |

| File | What's in it |
|---|---|
| `%APPDATA%\Xylem\settings.json` | Theme, view choices, alerts, last path |
| `%APPDATA%\Xylem\rules.json` | Your custom rules (if you made any) |
| `%LOCALAPPDATA%\Xylem\cache\` | Remembered scans (File → Forget remembered scans deletes them) |
| `%LOCALAPPDATA%\Xylem\deletions.log` | Deletion history |
| `Documents\Xylem\` | CSV exports and cleanup scripts you asked for |

Nothing else is written, and nothing leaves your PC.

---

## 9. Build from source
Needs Windows 10/11, Rust (stable, MSVC toolchain) and the Visual Studio C++ build tools.

```powershell
git clone https://github.com/JayD108/Xylem.git
cd Xylem
cargo build --release --workspace                  # ~2-3 min (fat LTO)
.\target\release\xylem.exe                         # the app
.\target\release\xylem-cli.exe scan C: --top 20    # command line
```

## 10. Command line
`xylem-cli.exe scan <drive or folder>` with options:
- `--mode auto|mft|walk`: MFT needs an admin shell.
- `--by disk|size`: sort by size on disk or logical size.
- `--top N`, `--json`, `--export all.csv`, `--threads N`.

## 11. Tests
```powershell
cargo test --workspace                                       # ~90 tests: unit, integration, built-in fuzz
cargo clippy --workspace --all-targets -- -D warnings
.\target\release\xylem-bench.exe verify "C:\Program Files"   # exact match against a plain std::fs walk
.\tools\full-test.ps1                                        # 19 checks: build, tests, fuzz, speed, accuracy, security
.\tools\fuzz-overnight.ps1 -Hours 10                         # coverage-guided fuzzing (nightly + cargo-fuzz)
```

What the tests cover:
- **Parsers:** torn, junk and hostile MFT records, run lists and boot sectors; index files (also old versions); notification buffers.
- **Tree:** hardlinks, orphans, stale parents, parent cycles, junction loops, Unicode names, dates.
- **Rules:** categories, markers, protection, custom rules that try to unlock protection.
- **Deleting:** decoy names on real files, junction swaps after confirming, pinned folders, warnings.
- **Admin helper:** malformed command lines, the pipe hand-off, the lockdown (a child process proves it can't write or start programs).
- **Live updates, leftovers, app-data hints, compression and the cleanup script.**

Opt-in tests (`-- --ignored`) touch real things: a full C: scan for memory, the real elevated Fast scan (one UAC prompt), `compact.exe` on a temp folder. `XYLEM_FUZZ_ROUNDS=2000000` runs the built-in fuzz tests longer.

## 12. Releases
- **Official releases:** push a tag matching `Cargo.toml`'s version (`git tag v0.2.0 && git push origin v0.2.0`). `.github/workflows/release.yml` then tests and builds, signs through SignPath once that's set up, builds the NSIS installer and the portable zip, runs a real install/uninstall test, and publishes a GitHub Release with checksums.
- **Local build:** `.\tools\release.ps1` (needs NSIS) creates the same files in `dist\`.

## 13. Project layout
```
crates/
  core/        the data model: Index (+ store.rs file format), view.rs (map shapes),
               classify.rs + rules.json (categories), report.rs (top-N, CSV), format.rs
  scan-walk/   folder-walk scanner (no admin)
  scan-mft/    MFT reader: volume.rs (raw reads), record.rs (parsing), scan.rs (tree)
  session/     the engine API the app calls (lib.rs: Scanned) and everything around it:
                 helper.rs, elevate.rs, lockdown.rs   admin helper, pipe, dropping rights
                 safety.rs, integrity.rs, recycle.rs  delete checks, locks, Recycle Bin
                 remember.rs                          instant reopen, what changed
                 dupes.rs                             duplicate finder
                 watch.rs                             live updates
                 tools.rs, power.rs                   Windows' own tools, power tools
                 orphans.rs                           leftover app-data hints
  native/      xylem.exe: the app (app.rs, map.rs, theme.rs, settings.rs) + helper modes
  cli/         xylem-cli.exe
  bench/       xylem-bench.exe: gen | run | time | verify | modes
fuzz/          coverage-guided fuzz targets (cargo-fuzz)
installer/     NSIS script (xylem.nsi) and its welcome image
.github/       CI and the release pipeline
tools/         release, full test, benchmark, fuzzing, icon and UI-test scripts
docs/          documentation, logo, screenshots
```
