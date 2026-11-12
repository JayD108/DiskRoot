# Xylem command line (`xylem-cli`)

[← Back to README](../README.md)

`xylem-cli.exe` scans a drive or folder and lists what takes the space, as a table, as JSON, or as a full CSV. Like the app, it **only reads**: it never changes, moves or deletes anything, and it never goes online.

---

## 1. Get it

| You have | Where `xylem-cli.exe` is |
|---|---|
| The installer ([`xylem-setup.exe`](https://github.com/JayD108/Xylem/releases/latest/download/xylem-setup.exe)) | `C:\Program Files\Xylem\xylem-cli.exe`, **added to PATH by the installer** since 0.4.1 (component "Command-line tool (xylem-cli), on PATH", on by default). Open a new terminal and it works |
| The portable zip (`xylem-<version>-portable.zip`) | Next to `xylem.exe` wherever you unzipped it |

Downloads: [Releases](https://github.com/JayD108/Xylem/releases/latest).

## 2. Make `xylem-cli` work from any folder

**Installed with 0.4.1 or newer:** nothing to do. Open a *new* terminal (PATH is read when a window starts) and type `xylem-cli --version`.

**Portable zip, or installed 0.4.0 or older:** Windows only finds a program by name if its folder is on your **PATH**. Add it once (your account only, no admin needed). In PowerShell:

```powershell
$dir = "C:\Program Files\Xylem"   # or the folder you unzipped the portable version to
$p = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not (($p -split ';') -contains $dir)) { [Environment]::SetEnvironmentVariable("Path", ($p.TrimEnd(';') + ";" + $dir).TrimStart(';'), "User") }
```

Then **close PowerShell and open a new window** (PATH is read when a window starts) and check:

```powershell
xylem-cli --version
```

Without PATH, use the full path. In PowerShell, quote it and put `&` in front: `& "C:\Program Files\Xylem\xylem-cli.exe" scan C:\`

To undo, remove the folder from Settings → System → About → Advanced system settings → Environment Variables → *Path* (your user).

## 3. Commands

```
xylem-cli scan [OPTIONS] <PATH>     scan a drive or folder and report
xylem-cli history [OPTIONS] <PATH>  size history the app keeps of a drive or folder
xylem-cli --version                 print the version
xylem-cli --help                    list commands
xylem-cli scan --help               list scan options
```

### `scan` options

| Option | Meaning | Default |
|---|---|---|
| `<PATH>` | Drive or folder to scan: `C:\`, `D:`, `"C:\My Data"` (quote paths with spaces) | required |
| `--top <N>` | How many biggest folders and files to list | `50` |
| `--by disk` / `--by size` | Rank by **size on disk** (what you get back by deleting) or by **logical size** | `disk` |
| `--mode auto` / `mft` / `walk` | `auto`: read the drive's file table (MFT, ~1 s) when it's a whole NTFS drive and you're admin, otherwise walk folders. `mft`: only the file table. `walk`: only the folder walk | `auto` |
| `--threads <N>` | Worker threads for the folder walk | one per CPU core (4–32) |
| `--json` | Print JSON instead of tables | off |
| `--export <FILE>` | Also write **every** file and folder to a CSV file | off |

### `history` options

Reads the size history the app saves with every remembered scan (and every scheduled scan). It scans nothing and changes nothing.

| Option | Meaning | Default |
|---|---|---|
| `<PATH>` | A drive or folder the app has scanned: `C:`, `"C:\My Data"` | required |
| `--days <N>` | "What changed" compares the latest scan with the newest one at least N days old (or the oldest there is) | `7` |
| `--folder <REL>` | Also print one folder's size at each scan, as a path inside `<PATH>`, e.g. `Users\me\Downloads` | off |
| `--json` | Print JSON (points, forecast, what grew, folder series) | off |

Example (shortened; the numbers are made up):

```
> xylem-cli history C: --days 30
Size history of C:\ · 31 scans since 2026-10-13 · history files use 2.68 MB
  At the pace of the last 30 days (+1.00 GB a day), C: would be full in about 330 days.

Latest scans
  Date         When           Method          On disk  Drive free
  2026-11-12   just now       folder walk     1.52 TB      333 GB
  ...

What changed since the scan from 30 days ago (asked: last 30 days)
  +  4.77 GB     41.2 GB  Users\me\Downloads
```

## 4. Examples

```powershell
# The 20 biggest folders and files on C:
xylem-cli scan C:\ --top 20

# One folder (quotes because of the space)
xylem-cli scan "C:\Users\me\Downloads" --top 10

# Rank by real (logical) size instead of size on disk
xylem-cli scan D:\ --by size

# Everything into a spreadsheet (opens in Excel)
xylem-cli scan C:\ --export "$env:USERPROFILE\Desktop\c-drive.csv"

# JSON for scripts
xylem-cli scan C:\ --json > c.json

# Fast scan of a whole drive: open PowerShell as administrator first
xylem-cli scan C:\ --mode mft
```

### Use it from scripts

PowerShell:
```powershell
$r = xylem-cli scan C:\ --json --top 10 | ConvertFrom-Json
$r.top_folders | Select-Object path, @{n='GB';e={[math]::Round($_.on_disk/1GB,1)}}
```

Python:
```python
import json, subprocess
r = json.loads(subprocess.run(["xylem-cli", "scan", "C:\\", "--json", "--top", "10"], capture_output=True, text=True, check=True).stdout)
for f in r["top_folders"]:
    print(f'{f["on_disk"] / 2**30:6.1f} GB  {f["path"]}')
```

Find every folder over 10 GB in the CSV:
```powershell
Import-Csv c-drive.csv | Where-Object { $_.Kind -eq 'dir' -and [int64]$_.OnDisk -gt 10GB } | Sort-Object { [int64]$_.OnDisk } -Descending | Select-Object -First 20 Path, @{n='GB';e={[math]::Round([int64]$_.OnDisk/1GB,1)}}
```

## 5. Output formats

### Table (default)

```
Scanned C:\ in 8.43 s · mode: folder walk (MFT read needs administrator rights)
  1,677,869 files · 307,965 folders · 586 unreadable folders (run as administrator to include more)
  Total 1.55 TB · on disk 1.52 TB · peak RAM 122 MB

Top 5 folders (by size on disk)
   #    On disk       Size        Files  Path
   1     427 GB     432 GB      991,724  C:\Users
   ...
Top 5 files (by size on disk)
   #    On disk       Size  Path
   1    18.0 GB    18.0 GB  C:\pagefile.sys
```

Sizes use 1024-based units (1 GB = 1024³ bytes), like Windows Explorer.

### JSON (`--json`)

| Field | Meaning |
|---|---|
| `root` | What was scanned |
| `mode` | `MFT (direct read)`, `folder walk`, or `folder walk (<why the MFT wasn't used>)` |
| `elapsed_ms` | Scan time |
| `files`, `folders` | Counts |
| `unreadable_folders` | Folders Windows didn't let it list (run as admin to include more) |
| `size`, `on_disk` | Total logical size and size on disk, in bytes |
| `peak_memory` | Peak memory used, bytes |
| `top_folders`, `top_files` | Lists of `{ "path", "size", "on_disk", "files" }`, biggest first (`files` = files inside; 1 for a file) |

### CSV (`--export`)

One row per file and folder, UTF-8, with a header:

| Column | Meaning |
|---|---|
| `Path` | Full path (quoted) |
| `Size` | Logical size in bytes (folders: everything inside) |
| `OnDisk` | Size on disk in bytes (folders: everything inside) |
| `Files` | Files inside (1 for a file) |
| `Kind` | `dir` or `file` |

A whole C: drive is about 2 million rows (~300 MB; C:\Windows alone: 277,000 rows, 38 MB). Excel opens up to 1,048,576 rows; for bigger exports use PowerShell, Python or a database.

## 6. Admin, speed and accuracy

| | Without admin (`walk`) | As admin (`mft`) |
|---|---|---|
| Speed on a 1.5 TB drive | ~4–9 s | ~1–2 s |
| Folders Windows hides from you | Counted as unreadable | Included |
| Works on | Any drive or folder | Whole NTFS drives |
| Hardlinked files (one file, several names) | Counted once per name (totals can be higher) | Counted once |

## 7. Exit codes

| Code | Meaning |
|---|---|
| `0` | Done |
| `1` | The scan failed; the message says why (e.g. "The system cannot find the path specified", or "Access is denied" for `--mode mft` without admin) |
| `2` | Wrong command or option (the help is printed) |

## 8. What the CLI doesn't do (yet)

The CLI only reports. Categories, safety labels, **Save space**, **Compress**, delete, "what changed" between the last two scans and duplicates are in the app (`xylem.exe`); `history` reads what the app saved. Ideas for CLI commands for those are welcome: see [CONTRIBUTING.md](../CONTRIBUTING.md).
