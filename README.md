<div align="center">

<img src="docs/logo/xylem-logo-512.png" alt="Xylem logo: a leaf whose veins form growth rings, with one amber slice like a disk-usage chart" width="140">

# Xylem

**See what's filling your drive, what each thing is, and whether it's safe to delete.**

A fast, free and open-source disk space analyzer for Windows 10 and 11.

**[⬇ Download Xylem for Windows](https://github.com/JayD108/Xylem/releases/latest/download/xylem-setup.exe)** · [portable zip](https://github.com/JayD108/Xylem/releases/latest/download/xylem-portable.zip) · [all releases](https://github.com/JayD108/Xylem/releases)

</div>

![Xylem scanning C:\Windows: tree-table, sunburst map and the details pane explaining that Windows files must not be deleted by hand](docs/screenshots/xylem-location.png)

## The problem it solves

When your drive fills up, you need three answers:
1. **Where** is the space going?
2. **What** is it: a game, `node_modules`, an AI model cache, Windows itself?
3. **Is it safe to delete**, and how?

Most tools only answer the first one, and the friendly ones are slow on big drives. Xylem answers all three, in seconds, and then shows you **every way to get space back**, with how much each one gives.

**Its promises:** offline (no network code at all) · read-only scanning · nothing changes without your OK · the window never runs as admin · fast · open source.

## Features

- **Fast:** maps a whole NTFS drive in about 1–2 seconds by reading its file table directly (read-only). Without admin it still works, by walking folders.
- **Meaning, not just sizes:** every file is sorted into categories such as *Apps & Games, Dev stuff, AI & data, Media, System, Temp & caches*. You can browse by location or by category.
- **Plain-English advice:** known folders come with a note and a label: *safe to delete*, *check first* or *don't delete by hand*.
- **Your drive at a glance:** a strip at the bottom shows used and free space and your 5 biggest space users. The start screen shows every drive with a fill bar.
- **Instant reopen and "what changed":** your last scan opens in under a second (a whole 1.5 TB drive in ~0.4 s). After a rescan, Xylem points at the exact folders that grew or shrank, both in a list and in a Change column right in the tree.
- **Clear pictures:** a tree-table with size bars, next to a sunburst or a treemap. Light and dark themes, full keyboard navigation.
- **Ways to save space:** the **Save space** button lists, biggest first:
  - **what to compress:** folders and big files whose files haven't changed *or arrived* for 1, 3, 6 or 12 months, with the expected saving (from real measurements per file type) and a one-click Compress;
  - **every other way:** caches and temp files, build folders in old projects, AI model stores, the Recycle Bin, the hibernation file, previous Windows, restore points, big files that won't compress (move them), WSL/Docker disks, OneDrive copies, folder copies, old installers, duplicates, and Compact OS. Each with its size and exactly how to do it.
- **Knows when data arrived:** downloads and unpacked datasets keep their author's old dates; Xylem also reads when each file arrived on your PC, so "not changed in 2 years" really means that.
- **Similar folders:** finds folders that look like copies of each other, and projects with similar names (`project`, `project_v2`, `project - Copy`).
- **Find what to clean:** the largest files (with search), sizes by file type, and an opt-in duplicate finder that keeps the original-looking copy.
- **Clean up with confidence:** "Mark all" for things that are safe to clear (every `node_modules`, Rust `target`, cache), a Leftovers view (old installers and archives in Downloads, Windows upgrade leftovers, app data of uninstalled programs), and buttons for Windows' own cleanups (DISM, System Protection, Storage settings). Add your own categories with custom rules.
- **See it happen:** colour by age (what hasn't changed in years), live updates while Xylem is open, and alerts when a folder grows past a limit.
- **Fast compression, built in:** right-click a folder → **Compress…**. Xylem's own engine drives Windows' file compression on many files at once (about 15× faster than `compact.exe` in our tests), skips files that won't shrink, shows real progress, can be stopped, and **Uncompress** undoes it. Files stay where they are and open as usual.
- **Power tools, all opt-in:** export a cleanup plan as a PowerShell script to review, and shrink WSL/Docker virtual disks with a guided, checked compaction.
- **Careful deleting:** only to the Recycle Bin, only after you confirm. Windows and system folders are locked. You get warnings for OneDrive folders, Git repositories and very large deletes, and a history of everything you deleted. Mark several items (Ctrl+click) to remove them in one go. Hardlinked files are flagged, because deleting one name frees nothing.
- **Light on your PC:** the status bar shows Xylem's own RAM and CPU; free its memory with one click (your scan stays saved), and scan gently so other programs stay fast.
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

1. **[Download `xylem-setup.exe`](https://github.com/JayD108/Xylem/releases/latest/download/xylem-setup.exe)** (always the newest version, about 3 MB).
2. Run it and click **Install**. Windows may say *"Windows protected your PC"* because releases aren't signed yet: click **More info → Run anyway**. You can check the file against `SHA256SUMS.txt` on the [release page](https://github.com/JayD108/Xylem/releases/latest).
3. Start **Xylem** from the Start menu, or right-click any folder → **Analyze with Xylem**.

One installer gives you everything:

| | |
|---|---|
| `xylem.exe` | The app, in Program Files (where only admins can change it, which keeps Fast scan safe) |
| `xylem-cli` | The command-line tool, **already on PATH**: open a new terminal and type `xylem-cli scan C:\` ([guide](docs/CLI.md)) |
| Start menu entry, right-click "Analyze with Xylem", optional desktop shortcut | Pick them on the components page |
| Uninstaller | Settings → Apps → Xylem. It removes exactly what it installed, including the PATH entry |

**No install?** Take the **[portable zip](https://github.com/JayD108/Xylem/releases/latest/download/xylem-portable.zip)**, unzip it anywhere and run `xylem.exe` (Fast scan needs the installed version). To use `xylem-cli` from any folder, add that folder to your PATH ([how](docs/CLI.md#2-make-xylem-cli-work-from-any-folder)).

Silent install for IT: `xylem-setup.exe /S` (uninstall: `"C:\Program Files\Xylem\uninstall.exe" /S`).

## How to use

1. Pick a drive or folder in the box at the top-left.
2. Press **Scan** (F5), or **Fast scan** (Ctrl+F5) for a whole drive (Windows asks for permission).
3. Expand folders in the list, or click the map. Switch **Location / Category** and **Sunburst / Treemap** in the toolbar.
4. Select anything to see what it is and whether it's safe to delete in the details pane at the bottom.
5. Press **Save space** to see what you could compress and every other way to get space back.
6. Next time, Xylem reopens your last scan instantly. Press **Scan** again, then **View → What changed** to see what grew.
7. **Move to Recycle Bin…** (or the Delete key) and **Compress…** ask before doing anything.

Every feature, explained with what happens underneath: [docs/FEATURES-AND-DEVELOPMENT.md](docs/FEATURES-AND-DEVELOPMENT.md). There's also a command-line tool, `xylem-cli.exe`: [docs/CLI.md](docs/CLI.md) shows how to set it up and every command.

## Safety and privacy

Scanning only reads your disk, and Xylem never goes online. It doesn't open your files' contents, except when you start the duplicate finder or Compress, on your PC. It changes files only when you confirm: moving them to the Recycle Bin, compressing a folder (undoable), or the opt-in power tools, which use Windows' own tools. Ways to save space only suggests. Before each release a full local test checks it end to end: no network connections, read-only scanning, the admin helper's limits, and fuzzing of every file parser. The shield button in the app shows exactly what it does. Full details: [docs/SECURITY-AND-PRIVACY.md](docs/SECURITY-AND-PRIVACY.md). To report a problem privately, see [SECURITY.md](SECURITY.md).

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
| [Command line](docs/CLI.md) | Set up `xylem-cli`, every command and option, JSON and CSV formats, script examples |
| [Benchmarks](docs/BENCHMARKS.md) | Speed against WizTree, WinDirStat and eDirStat, and compression measured per file type, with the method |
| [Contributing](CONTRIBUTING.md) | How to help, the rules every change keeps, and the **call for compression research** |
| [Help wanted](docs/HELP-WANTED.md) | Everything Xylem needs next: compression problems with measurements, known gaps, testing |

## Help make compression better

Xylem's compression engine is open for research. We've measured every Windows compression algorithm (and zstd/lz4 for comparison) on real data; text and programs save 50–70%, but float arrays save ~15% and model weights or columnar files only 6–8%. The full list of what we need is in [docs/HELP-WANTED.md](docs/HELP-WANTED.md). If you know compression, read the [call for compression research](CONTRIBUTING.md#call-for-compression-research-): better estimates, smarter per-file choices, transforms, or a new safe mode. Measure on copies, open an issue, send a pull request.

## Acknowledgements

Claude (by Anthropic) was used as an AI assistant during the development of this project: for code, testing, security review and documentation. All work is reviewed and maintained by the project owner.

## License

MIT or Apache-2.0, at your option ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).
