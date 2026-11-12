# Security and privacy

[← Back to README](../README.md) · To report a vulnerability, see [SECURITY.md](../SECURITY.md).

## In one table

| DiskRoot… | |
|---|---|
| reads file **names, sizes, attributes and dates** | Yes, that's what a scan is |
| reads what's **inside** your files | Only when you start the duplicate finder or Compress, on your PC. It also reads a few small files about your PC: Steam's and Epic's game lists (for "Belongs to") and your own rules file |
| connects to the **internet** | Never. No network code at all; measured with 0 connections |
| collects **telemetry** or needs an account | No |
| **changes** anything | Only when you confirm: moving items to the Recycle Bin, compressing a folder (undoable), or one of Windows' own tools you start from it. Ways to save space only suggests |
| runs **as admin** | The window doesn't need it: started with "Run as administrator", it warns, and its own scans stay folder walks. Only a short-lived helper, after the Windows prompt, reads the raw drive (Fast scan) or compacts a virtual disk |
| runs **in the background** or at startup | Only if you turn on scheduled scans: then Windows' Task Scheduler starts a normal scan (no admin, no window) every 30 min or more, which exits when done. Live updates and alerts stop when you close DiskRoot |
| keeps **data on your PC** | Settings, remembered scans, a deletion log (see the end of this page). Nothing leaves your PC |

## What DiskRoot reads
- **Scanning** reads names, sizes, attributes, last-modified dates, created dates (when a file arrived on this PC) and, on local NTFS drives, file IDs (to count hardlinked files once). Folders are opened for listing only and drives read-only; the scanning code contains no write calls. Last-opened dates aren't read. After a scan, DiskRoot saves it to its own folder (`%LOCALAPPDATA%\DiskRoot\cache`, lz4-compressed: about 45 MB for a 1.5 TB drive), which counts as SSD writes.
- **Scheduled scans** (off until you turn them on) are the same read-only folder walk, started by a Task Scheduler task that runs as you with least privilege. They never ask for admin, never run Fast scan, and write only DiskRoot's own files (the remembered scan and a small log in `%LOCALAPPDATA%\DiskRoot`). The task is removed when you turn them off or uninstall DiskRoot. Their optional warnings are a local Windows notification (no network), at most once a day per reason.
- **Drive health** asks each disk for its own reports (model, health log, TRIM support) through a handle opened with no access rights: it can't read or write any data on the disk, and needs no admin. One small note per drive per day (counters only: data written/read, wear %, hours, temperature, errors) is kept for the writes-per-day chart.
- **Who's writing** reads Windows' list of running programs with their write counters (one system call; no program is opened or touched), each disk's write counter, and how long since the last key or mouse input (to tell idle from active). Nothing is kept except, for scheduled scans, the names and rates of the top 5 writing programs of each 30-s sample in the scan log.
- **Ways to save space** works only from the scan in memory: it opens no file and changes nothing. Its buttons lead to the usual dialogs (Compress, the delete review, Windows' own tools).
- **The duplicate finder** (View → Find duplicate files, only when you start it) compares files of the same size by their contents. It skips online-only cloud files (reading them would download them) and treats hardlinks as one file.
- **Live updates** use Windows' change notifications for the scanned folder while the app is open. They read nothing else, and they stop when the app closes.
- **Belongs to** reads Windows' list of installed programs (install folders), Steam's `libraryfolders.vdf` and `appmanifest_*.acf` and the Epic Games Launcher's `.item` manifests, once at start, to name the program or game a folder belongs to. Nothing is stored.
- **Leftover hints** read the list of installed programs from the registry (the same list as Settings > Apps) and the names of running programs. Nothing is stored or sent.
- **The delete dialog** checks the list of running programs to warn about, say, Spotify running while you delete its cache.

## Admin rights

### Fast scan
1. The window creates a **private named pipe** first: nobody else can create one with that name, and only local programs can connect.
2. It starts `diskroot.exe --helper-scan C: --pipe <name>` through the Windows UAC prompt. The helper accepts only that exact command line and creates no window.
3. The helper opens the drive (read-only) and the pipe, then **drops its admin power before reading any drive data**: it removes every privilege, lowers itself to Low integrity (so it can't write files, registry keys or other programs), joins a job that forbids starting programs, and turns on Windows' code-loading protections. If it can't, it stops.
   - What this doesn't take away: **reading**. Windows keeps the helper in the Administrators group, and Low integrity blocks only writing. So in theory, a bug in the parser could read anything an admin can read. The helper already has the whole drive open for reading anyway. It can't write, start programs or load code, and it has no network code. A fully sandboxed reader process is on the research list.
4. It connects to the pipe in "identification" mode, so even a fake pipe could never act with its admin rights. It **writes nothing to disk**.
5. The window checks everything it receives with the same fuzz-tested loader it uses for its own files.
6. Fast scan can be cancelled; a helper that hangs is stopped after 10 minutes.

Other rules:
- **Only from an installed copy:** release builds use admin rights only from Program Files, which only administrators can change, never from a portable copy in Downloads.
- **Standard user + admin password:** works. The administrator who types the password authorizes reading every file *name* on the drive (never contents).

### Virtual-disk compaction
`diskroot.exe --helper-compact-vhdx <file>` (admin, after the UAC prompt) accepts only one existing `.vhdx` file outside Windows, refuses if anything has the disk open, and gives Windows' `diskpart` its commands through a pipe, so there's no script file another program could change. The disk is attached **read-only** while it's compacted. Release builds only do this from the installed copy.

## Deleting safely
- Always to the **Recycle Bin**, and only after you confirm.
- **Locked:** Windows, program folders, ProgramData, profile roots, the scanned folder itself, and any folder that contains a protected place. Windows and Program Files are also locked **by file identity**, so another spelling of the path (UNC, `subst`, short 8.3 names, junctions) doesn't get around it.
- **Refused names:** names ending in a dot or space, device names (`CON`, `NUL`, …) and paths of 260+ characters, which Windows could silently turn into a different item. Use Explorer for those.
- **Checked right before the move:**
  - When the dialog opens, DiskRoot records the file identity (volume + file ID) of the item and of every folder up to the scan root.
  - When you confirm, it re-opens those folders, compares identities, and **keeps them open so nothing can rename or swap them** until the Recycle Bin move ends.
  - The item must still exist and be the same type, and no folder on its path may have become a link since the scan.
- **Warnings that need a tick:** OneDrive folders (deleting also deletes in the cloud and on your other devices), Git repositories inside (unpushed work), very large deletes (10 GB+ or 50,000+ files), USB and network drives (may have no Recycle Bin), and a program using the place that's running right now.
- **"Mark all"** only marks places a "safe to clear" rule matched directly, never a folder that merely holds files of some type. Leftover app-data hints are never marked.
- **Hidden characters** in names (such as right-to-left overrides) are shown visibly in the delete dialog and the history.
- **History:** every delete (recycled, cancelled or refused) is logged in `%LOCALAPPDATA%\DiskRoot\deletions.log` (File → Deletion history).

## Windows' own tools
Some cleanups are done by Windows, not by DiskRoot, and only when you click them:
- **DISM** component cleanup (WinSxS) asks for admin through the Windows prompt.
- **Folder compression** is done by DiskRoot itself with Windows' own file compression, without admin. It opens each file read-only (no other program can change it meanwhile), never deletes, moves or copies anything, checks that every file really is inside the folder you chose, and skips files in use. Windows refuses files you aren't allowed to change. Never on protected or cloud folders or whole drives. Uncompress undoes it.
- **Settings pages** (Storage, System Protection) just open.
- All tools are started by their **full path in System32** and **never through `cmd.exe`**, so no argument is interpreted by a shell.
- The **PowerShell cleanup plan** is a file DiskRoot writes for you; DiskRoot never runs it. It's a dry run unless you add `-Run`, re-checks every item, refuses links, writes paths so PowerShell can't interpret them, and only uses the Recycle Bin.

## Your own rules
`%APPDATA%\DiskRoot\rules.json` can add categories, sub-types and notes. It **can't** set protection, safety labels or actions (such fields are refused), and its rules never apply where a built-in rule matches or inside a protected place. Size and count are limited.

## Robust parsing
- MFT records, run lists, boot sectors, index files and change notifications are parsed with bounds checks in safe Rust. `unsafe` is limited to small, commented Windows calls.
- **Fuzzing:** built-in fuzz tests run on every test run (2 million rounds in the full test), and coverage-guided fuzzing (cargo-fuzz with AddressSanitizer) ran overnight on 6 targets. Fuzzing found and fixed three real bugs: a parent-link cycle in a corrupt MFT crashed the tree build; sizes near 2⁶⁴ overflowed the totals (they now saturate); a corrupt boot sector could request huge buffers (the geometry is now limited to real NTFS values).

## DLL planting defence
- `diskroot.exe` loads every DLL it needs from `System32` only: the `/DEPENDENTLOADFLAG:0x800` link flag covers static imports, and `SetDefaultDllDirectories` runs first thing in `main`.
- So a malicious DLL saved next to a portable copy (for example in Downloads) is never loaded, including by the admin helpers. Tested with planted DLLs in the full test.

## Integrity check
The shield button (or Help → Security & privacy) shows whether this copy runs from Program Files, and whether it carries a valid Authenticode signature (checked with Windows' `WinVerifyTrust`, offline, without a revocation lookup).

## Security review and testing
The code was reviewed against a 16-finding security test plan; all findings are fixed or documented. Every release is checked by a 19-step local test, including:
- the helper refusing malformed command lines;
- planted DLLs never being loaded;
- a planted `explorer.exe` never being run;
- Fast scan refused from a non-installed copy;
- **zero network sockets** during a full scan;
- scanning changing nothing in the scanned folder.

In CI, GitHub Actions are pinned to commit SHAs, and Dependabot, `cargo audit` and `cargo deny` (OSI licences only, crates.io only) run on every push.

Still to do before 1.0: destructive tests on disposable machines, 24-hour fuzzing, and code signing.

## Your data on disk
| File | Contents | How to remove it |
|---|---|---|
| `%APPDATA%\DiskRoot\settings.json` | Theme, view choices, size alerts, last path | Delete the file |
| `%APPDATA%\DiskRoot\rules.json` | Your custom rules, if you made any | Delete the file |
| `%LOCALAPPDATA%\DiskRoot\cache\` | Names and sizes of your last scans (up to 8 drives or folders), for instant reopen and "what changed"; a size history of folders of 1 MB or more (`.hist`, at most 16 MB per drive or folder); paths and sizes of the biggest log files of the last 8 scans (`.logs.json`) | File → Forget remembered scans; View → Size history → Clear history; File → Remember scans turns both off |
| `%LOCALAPPDATA%\DiskRoot\health.jsonl` | One line per drive per day: model, data written/read, wear %, powered-on hours, temperature, errors | Delete the file |
| `%LOCALAPPDATA%\DiskRoot\scheduled-scans.jsonl`, `warnings.json` | Scheduled scans only: one line per background scan (time, duration, memory, change, warning shown), and when each warning was last shown | Delete the files |
| `%LOCALAPPDATA%\DiskRoot\deletions.log` | What you deleted with DiskRoot | Delete the file |
| `Documents\DiskRoot\` | CSV exports and cleanup scripts you asked for | Delete the folder |

Remembered scans are loaded with the same checked, fuzz-tested loader as the admin helper's result: a changed or damaged file is rejected, never trusted, and deleting always re-checks the real disk. The uninstaller leaves these files in place; delete the folders to remove them.
