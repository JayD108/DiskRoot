<div align="center">

<img src="docs/logo/xylem-logo-512.png" alt="Xylem logo: a leaf whose veins form growth rings, with one amber slice like a disk-usage chart" width="140">

# Xylem

**See what's filling your drive, what each thing is, and whether it's safe to delete.**

A fast, free and open-source disk space analyzer for Windows 10 and 11.

</div>

![Xylem scanning C:\Windows: tree-table, sunburst map and the details pane explaining that Windows files must not be deleted by hand](docs/screenshots/xylem-location.png)

## The problem it solves

When your drive fills up, you need three answers:
1. **Where** is the space going?
2. **What** is it: a game, `node_modules`, an AI model cache, Windows itself?
3. **Is it safe to delete**, and how?

Most tools only answer the first one, and the friendly ones are slow on big drives. Xylem answers all three, in seconds.

## Features

- **Fast:** maps a whole NTFS drive in about 1–2 seconds by reading its file table directly (read-only). Without admin it still works, by walking folders.
- **Meaning, not just sizes:** every file is sorted into categories such as *Apps & Games, Dev stuff, AI & data, Media, System, Temp & caches*. You can browse by location or by category.
- **Plain-English advice:** known folders come with a note and a label: *safe to delete*, *check first* or *don't delete by hand*.
- **Your drive at a glance:** a strip at the bottom shows used and free space and your 5 biggest space users. The start screen shows every drive with a fill bar.
- **Instant reopen and "what changed":** your last scan opens in under a second (a whole 1.5 TB drive in ~0.4 s). After a rescan, Xylem points at the exact folders that grew or shrank, both in a list and in a Change column right in the tree.
- **Clear pictures:** a tree-table with size bars, next to a sunburst or a treemap. Light and dark themes, full keyboard navigation.
- **Similar folders:** finds folders that look like copies of each other, and projects with similar names (`project`, `project_v2`, `project - Copy`).
- **Find what to clean:** the largest files (with search), sizes by file type, and an opt-in duplicate finder that keeps the original-looking copy.
- **Clean up with confidence:** "Mark all" for things that are safe to clear (every `node_modules`, Rust `target`, cache), a Leftovers view (old installers and archives in Downloads, Windows upgrade leftovers, app data of uninstalled programs), and buttons for Windows' own cleanups (DISM, System Protection, Storage settings). Add your own categories with custom rules.
- **See it happen:** colour by age (what hasn't changed in years), live updates while Xylem is open, and alerts when a folder grows past a limit.
- **Power tools, all opt-in:** export a cleanup plan as a PowerShell script to review, compress folders in place with Xylem's fast compression engine (Windows' own file compression, many files at once, with real progress and undo), and shrink WSL/Docker virtual disks with a guided, checked compaction.
- **Careful deleting:** only to the Recycle Bin, only after you confirm. Windows and system folders are locked. You get warnings for OneDrive folders, Git repositories and very large deletes, and a history of everything you deleted. Mark several items (Ctrl+click) to remove them in one go. Hardlinked files are flagged, because deleting one name frees nothing.
- **Private:** no network connections, no accounts, no telemetry. One small program (~8 MB).

![Dark theme, treemap grouped by category](docs/screenshots/xylem-category.png)

## How it works

```
 You ──► xylem.exe (normal user) ──► Scan: walks folders, no admin needed
                │
                └── Fast scan ──► Windows asks for admin ──► a short-lived helper reads the
                                  drive's file table (read-only), hands the result back, exits
```

The window itself never runs as admin. More detail: [docs/HOW-IT-WORKS.md](docs/HOW-IT-WORKS.md).

## Install

Download **`xylem-setup-<version>.exe`** from [Releases](https://github.com/JayD108/Xylem/releases) and run it. It installs into Program Files and adds a normal uninstaller (Settings → Apps). For no install, use **`xylem-<version>-portable.zip`**: unzip it and run `xylem.exe`.

## How to use

1. Pick a drive or folder in the box at the top-left.
2. Press **Scan** (F5), or **Fast scan** (Ctrl+F5) for a whole drive (Windows asks for permission).
3. Expand folders in the list, or click the map. Switch **Location / Category** and **Sunburst / Treemap** in the toolbar.
4. Select anything to see what it is and whether it's safe to delete in the details pane at the bottom.
5. Next time, Xylem reopens your last scan instantly. Press **Scan** again, then **View → What changed** to see what grew.
6. **Move to Recycle Bin…** (or the Delete key) asks before doing anything.

Every feature, explained with what happens underneath: [docs/FEATURES-AND-DEVELOPMENT.md](docs/FEATURES-AND-DEVELOPMENT.md). There's also a command-line tool, `xylem-cli.exe`.

## Safety and privacy

Scanning only reads your disk, and Xylem never goes online. It doesn't open your files' contents, except when you start the duplicate finder, which compares files on your PC. It changes files only when you confirm: moving them to the Recycle Bin, or the opt-in power tools (NTFS compression, virtual-disk compaction), which use Windows' own tools. The shield button in the app shows exactly what it does. Full details: [docs/SECURITY-AND-PRIVACY.md](docs/SECURITY-AND-PRIVACY.md). To report a problem privately, see [SECURITY.md](SECURITY.md).

## Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by [SignPath Foundation](https://signpath.org/) *(application pending; until approved, releases are unsigned and the app says so)*.

- **Builds:** every release file is built by GitHub Actions from this repository, and each signing request is approved by hand. Nothing built on a personal computer is signed.
- **Team roles:** committers and reviewers: [JayD108](https://github.com/JayD108). Approvers: [JayD108](https://github.com/JayD108).
- **Privacy policy:** this program will not transfer any information to other networked systems unless specifically requested by the user or the person installing or operating it.

## More

| | |
|---|---|
| [How it works](docs/HOW-IT-WORKS.md) | Under the hood, step by step: reading the drive, the data model, categories, the map, reopen, live updates, deleting, accuracy |
| [Security and privacy](docs/SECURITY-AND-PRIVACY.md) | What Xylem reads and never does, admin rights, safe delete, Windows' own tools, your data on disk |
| [Features and development](docs/FEATURES-AND-DEVELOPMENT.md) | Every feature: what it does, how to use it, what happens underneath. Building, tests, project layout |
| [Roadmap](docs/ROADMAP.md) | Releases, planned features and what's next |
| [Benchmarks](docs/BENCHMARKS.md) | Speed against WizTree, WinDirStat and eDirStat, with the method |

## Acknowledgements

Claude (by Anthropic) was used as an AI assistant during the development of this project: for code, testing, security review and documentation. All work is reviewed and maintained by the project owner.

## License

MIT or Apache-2.0, at your option ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).
