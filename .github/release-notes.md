<img src="https://raw.githubusercontent.com/JayD108/Xylem/main/docs/logo/xylem-logo-512.png" alt="Xylem" width="96">

**Xylem {{VERSION}}**: a fast disk space analyzer for Windows 10/11. It shows what fills your drive, what each thing is, and whether it's safe to delete.

## New in 0.2
- **Instant reopen and "what changed":** your last scan opens in under a second; after a rescan, the folders that grew or shrank are listed and shown in a Change column.
- **Clean up with confidence:** "Mark all" for safe-to-clear places (`node_modules`, Rust `target`, caches), a Leftovers view (old installers, archives and disk images in Downloads, Windows upgrade leftovers, possibly orphaned app data), custom rules, and Windows' own DISM / System Protection / Storage cleanups.
- **See it happen:** colour by age (last modified), live updates while the app is open, size alerts.
- **Power tools (opt-in):** export marked items as a PowerShell cleanup script (dry run by default), NTFS compression with compact.exe, guided WSL/Docker virtual-disk compaction.
- **Finding space:** largest files with search, file types, duplicate finder, hardlink names, CSV export, Explorer right-click.
- **Safer:** the elevated helper drops its admin rights before reading drive data; deletes keep the folder chain locked until the Recycle Bin move ends; a running app that uses a cache is pointed out before deleting it.
- **Lighter:** scan data for 1.9M items 204 → 154 MB (including the new last-modified dates).

## What it does
- Scans a whole NTFS drive in about 1–2 seconds by reading its Master File Table, read-only, after the Windows admin prompt. Without admin it walks folders (no prompt).
- Tree-table with a sunburst or treemap. Groups by location *or* by category (apps, media, dev stuff, AI & data, system, temp & caches…).
- Explains known folders in plain English with a safety label, and locks Windows and system folders.
- Deletes only to the Recycle Bin, after you confirm. It re-checks the item on disk first and warns about OneDrive folders, Git repositories and very large deletes. A deletion history is kept on your PC.
- **No network connections, no telemetry, no accounts.**

## Download
- **`xylem-setup-{{VERSION}}.exe`**: installer (recommended). Installs into Program Files, which protects the admin helper used by Fast scan. Includes an uninstaller (Settings → Apps).
- **`xylem-{{VERSION}}-portable.zip`**: no install. Unzip and run `xylem.exe`.

Signature: {{SIGNED}}

## Code signing policy
Free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by [SignPath Foundation](https://signpath.org/) *(pending approval)*.

Every file in this release was built by GitHub Actions from the tagged commit of [JayD108/Xylem](https://github.com/JayD108/Xylem). Team roles and the privacy policy are on the [project page](https://github.com/JayD108/Xylem#code-signing-policy).

**Privacy:** this program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it.
