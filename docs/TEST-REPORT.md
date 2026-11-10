# Xylem full test: 2026-11-11 01:30

**19 of 19 checks passed.** Commit 5eedfe2.

| ID | Check | Result | Detail |
|---|---|---|---|
| A-01 | Release build | PASS |  |
| A-02 | All tests (debug: overflow checks on) | PASS | 67 passed |
| A-03 | All tests (release) | PASS | 67 passed |
| A-04 | Clippy, warnings as errors | PASS | 0 findings |
| A-05 | cargo-deny (licences OSI-only, advisories, sources) | PASS | advisories ok, bans ok, licenses ok, sources ok |
| A-06 | cargo-audit (known vulnerabilities) | PASS | 3 notices (unmaintained only = OK) |
| A-07 | No HTTP/TLS client crates (PV-04) | PASS |  |
| B-01 | Built-in fuzz, 2M rounds per parser (PA-01) | PASS | 68 s |
| C-01 | Tree build, 1.77M records (target <= 0.25 s) | PASS | 0.157 s |
| C-02 | Classify 1.8M items (target <= 0.25 s) | PASS | 0.175 s |
| C-03 | Folder walk of all of C: (target <= 8 s) | PASS | 4.73 s, 3.98 s |
| C-04 | Exact match with a plain std::fs walk (C:\Program Files) | PASS | MATCH |
| EL-01 | Helper refuses every malformed command line (exit 2) | PASS | 10 forms |
| IN-06 | Planted junk DLLs are never loaded (helper + window) | PASS | 9 DLLs planted |
| BP-01 | Planted explorer.exe is not run by 'Show in Explorer' | PASS | marker exe built: True |
| F-02 | Fast scan refused from a non-installed copy (message shown, no helper started) | PASS | message shown, no helper (re-checked after fixing the test script's text encoding) |
| PV-01 | Zero TCP/UDP sockets during a full scan of C: | PASS | 0 socket observations in 16 s |
| PA-06 | Corrupt/huge settings: app starts with defaults, no SMB to a UNC last_path | PASS |  |
| RO-01 | Scanning changes nothing in the scanned folder | PASS | 51 items compared |

## Notes
- First run: two false failures, both in the test, not in Xylem. **F-02**: the user's installed Xylem was open and was counted as a helper. **RO-01**: a folder's own timestamps changed, which Windows' own `dir` does too (NTFS updates directory times lazily when any program lists them); the check now compares files.
- F-02 in the second run: the button name with the ⚡ symbol was garbled because Windows PowerShell 5.1 read the BOM-less script as ANSI. Fixed; re-checked: message shown, no helper started.
- Overnight coverage-guided fuzzing (cargo-fuzz + ASan, 6 targets, 10 h) was running alongside: no crashes so far.
- Run it yourself: `.\tools\full-test.ps1` (≈ 20 min; writes test-evidence\REPORT.md).
