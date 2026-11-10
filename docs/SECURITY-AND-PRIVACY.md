# Security and privacy

[← Back to README](../README.md) · To report a vulnerability, see [SECURITY.md](../SECURITY.md).

## What Xylem reads and doesn't do
- It reads **file names, sizes and attributes** only. It never opens or reads what's inside your files.
- It has **no network code**: no HTTP client and no browser engine anywhere in its dependencies. It was measured with 0 network connections.
- It changes nothing on your computer except when **you** delete something, and that goes to the Recycle Bin.

## Disk access
- Drives are opened **read-only** (`GENERIC_READ`, unbuffered). The scanning code contains no write calls.

## Admin rights
- The app window never runs as admin.
- **Fast scan** starts `xylem.exe --helper-scan C: --save-index <file>` through the Windows UAC prompt. The helper creates no window: it scans, writes its result and exits.
- It accepts only that exact command line.
- **Hardened handoff:**
  - The helper only creates a *new* file (`create_new`) named `xylem-*.idx` directly in your temp folder. A planted file or link makes it fail instead of overwriting.
  - The app validates the file fully before loading it, then deletes it.
- **Install location:** installed in Program Files, only administrators can replace the exe that Fast scan runs as admin.

## Deleting safely
- Always to the Recycle Bin, and only after you confirm.
- **Re-check right before the move:**
  - The item must still exist.
  - It must still be the same type (file or folder).
  - No folder on its path may have become a link (junction or symlink) since the scan.
  - Otherwise the delete is refused, so a swapped folder can't redirect it.
- **Locked:** Windows, program folders, profile roots and the scanned folder itself.
- **Warnings that need a tick:**
  - OneDrive folders: deleting also deletes them in the cloud and on your other devices.
  - Folders containing Git repositories: unpushed work.
  - Very large deletes: 10 GB or more, or 50,000+ files.
- **History:** every delete (recycled, cancelled or refused) is logged in `%LOCALAPPDATA%\Xylem\deletions.log`; see File → Deletion history.

## Integrity check
The shield button (or Help → Security & privacy) shows:
- whether this copy runs from Program Files;
- whether it carries a valid Authenticode signature. This uses Windows' `WinVerifyTrust`, offline: no revocation lookup.

## Robust parsing
- MFT records and index files are parsed with bounds checks in safe Rust; `unsafe` is limited to small, commented Windows calls.
- **Fuzz tests** feed the MFT parser, run lists, boot sector, tree builder and index loader hundreds of thousands of random and corrupted inputs on every test run (`XYLEM_FUZZ_ROUNDS` for longer runs; 2 million pass). They found and fixed three real bugs:
  - A parent-link cycle in a corrupt MFT crashed the tree build.
  - Sizes near 2⁶⁴ overflowed the totals; they now saturate.
  - A corrupt boot sector could request huge buffers; the geometry is now limited to real NTFS values.

## Your data on disk
- Settings: `%APPDATA%\Xylem\settings.json`.
- Deletion log: `%LOCALAPPDATA%\Xylem\deletions.log`.
- Both stay on your PC. The uninstaller leaves them in place; delete the folders to remove them.
