<img src="https://raw.githubusercontent.com/JayD108/Xylem/main/docs/logo/xylem-logo-512.png" alt="Xylem" width="96">

**Xylem {{VERSION}}**: a fast disk space analyzer for Windows 10/11. It shows what fills your drive, what each thing is, and whether it's safe to delete.

## New in 0.4
- **Ways to save space** (the new **Save space** button): two lists from your scan, biggest first.
  - **Compress:** folders and big files whose files haven't changed or arrived for 1, 3, 6 or 12 months, with the expected saving and a one-click **Compress…** (Maximum suggested for things untouched for a year).
  - **Other ways:** caches and temp files, build folders in old projects, AI model stores, Recycle Bin, hibernation file, previous Windows, restore points, big files that won't compress, WSL/Docker disks, OneDrive copies, folder copies, old installers, duplicates and Compact OS, each with its size and exactly how.
  Nothing is changed from this window.
- **Arrival dates:** downloads and unpacked datasets keep their authors' old dates. Xylem now also reads when each file arrived on your PC, so a dataset downloaded last month no longer looks "2 years old".
- **Honest compression estimates** from real measurements per file type (e.g. text ~55%, EEG `.edf` ~33%, `.npy` ~15%, `.parquet`/`.pt` ~6–8% with Fast) instead of a flat 50%.
- **Safer cache labels:** a folder named `cache` inside your own projects is now "check first", not "safe to clear"; app caches in AppData stay safe.
- **Fixed:** the drive strip and totals at the bottom now update after Compress/Uncompress.
- **Help wanted:** a call for compression research and a list of open problems: see CONTRIBUTING.md and docs/HELP-WANTED.md.

## In 0.3
- Fast built-in compression (right-click → Compress…, ~15× faster than `compact.exe` in our tests, undoable), Similar folders, and fixes for compressed files in the normal scan and for "What changed" across scan methods.

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
