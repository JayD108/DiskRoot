# Security policy

## Reporting a vulnerability
Please **don't open a public issue** for security problems. Report them privately through GitHub: **Security → Report a vulnerability** on https://github.com/JayD108/DiskRoot (private vulnerability reporting).

Include the DiskRoot version, your Windows version, and steps to reproduce. You'll get a reply as soon as possible. Fixes are released through the normal signed release pipeline.

## Scope
Especially interesting:
- Anything that makes DiskRoot write to or modify a disk while scanning.
- Ways to make the elevated helper (`diskroot.exe --helper-scan`) do anything other than scan and write its one result file.
- Ways to make a delete hit a different item than the one confirmed (for example, via links or junctions).
- Ways to make Compress change file contents, work outside the chosen folder, or leave data that doesn't read back identical.
- Crashes or hangs caused by crafted file systems or index files.
- Any network traffic from the app.

## Supported versions
Only the latest release gets fixes.

## Verifying downloads
Each GitHub Release lists SHA-256 checksums (`SHA256SUMS.txt`). Once signing is active, `diskroot.exe`, `droot.exe` and the installer carry an Authenticode signature. The app shows its own signature status under Help → Security & privacy.
