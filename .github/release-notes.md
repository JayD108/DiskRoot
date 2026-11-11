<img src="https://raw.githubusercontent.com/JayD108/Xylem/main/docs/logo/xylem-logo-512.png" alt="Xylem" width="96">

**Xylem {{VERSION}}**: a fast disk space analyzer for Windows 10/11. It shows what fills your drive, what each thing is, and whether it's safe to delete.

## New in 0.3
- **Fast compression, built in:** right-click any folder → **Compress…**. Xylem now runs Windows' own file compression itself, on many files at once, instead of through `compact.exe`: about 15× faster at the new **Fast** level and 5× faster at **Maximum** in our tests (763 MB of mixed data). Files stay where they are and open as usual. It skips files that won't shrink, shows real progress with speed and time left, can be stopped, and **Uncompress** undoes it.
- **Similar folders** (View menu): finds folders that look like copies of each other, and projects with similar names (`project`, `project_v2`, `project - Copy`), with how much each group takes.
- **Fixed: compressed files were counted as almost 0 bytes** by the normal (no-admin) scan. Folders compressed with Windows' file compression (compact /EXE, CompactOS, Xylem's Compress) now show their real size on disk. Fast scan was not affected.
- **Fixed: "What changed" could compare a normal scan with a Fast scan** as if your files had changed. Xylem now remembers how each scan was made and says so when the two differ; it also shows how far apart the scans were.
- Clearer text about what needs admin rights and what Xylem can change; percentages no longer round a part up to 100%.

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
