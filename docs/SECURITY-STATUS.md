# Security findings: status

Findings from the security clearance test plan (code review of commit `e6691a3`), and what has been done about each. "Fixed" means fixed in code with a regression test where one can be automated. The full test plan (VM tests, Procmon evidence, 24 h fuzzing) still has to be run before 1.0.

| ID | Sev | Finding | Status |
|---|---|---|---|
| F-01 | 🔴 | Installer lets the user pick a writable install folder | **Fixed.** No directory page, and `/D=` is ignored: always `Program Files\Xylem` |
| F-02 | 🔴 | Fast scan elevates from any folder (portable/Downloads) | **Fixed.** Release builds refuse Fast scan unless the exe runs from Program Files (`integrity::fast_scan_allowed`). Scan without admin still works everywhere |
| F-03 | 🔴 | Installer's "Start Xylem" runs the app as admin | **Fixed.** The finish page launches it through `explorer.exe` (un-elevated). To verify: IN-04 in a VM |
| F-04 | 🔴 | `explorer.exe` launched by bare name (planting) | **Fixed.** Absolute path from `GetSystemWindowsDirectoryW` |
| F-05 | 🟠 | Trailing dot/space and device names can make Windows delete another item | **Fixed by refusing:** such names and paths ≥ 260 characters aren't deleted by Xylem (`safety::unusual_name`); use Explorer for them. Test: `delete_refuses_swapped_items_and_odd_names` |
| F-06 | 🟠 | Re-check skips the scan root; no file identity check | **Fixed.** When the dialog opens, the volume serial + file index of the item and every folder up to and including the scan root are recorded and compared right before deleting (`identity_chain`/`check_identity`). Test: a folder swapped after confirm is refused. A swap since the *scan* (not since confirm) is still caught only by the type/link checks |
| F-07 | 🟠 | Deleting a parent of a protected place was allowed | **Fixed.** `prepare_delete` refuses if anything inside matches a protecting rule (`protected_here`) |
| F-08 | 🟠 | Locks bypassed by other path spellings (UNC, `\\?\`, subst, 8.3, junction) | **Fixed in two layers:** (1) the scan root is resolved to its real path (`real_path`); (2) a spelling-independent **hard lock** compares file identities with the real Windows and Program Files folders (and ProgramData/Users themselves). Tests: 8.3 name, `subst` drive |
| F-09 | 🟡 | Elevated helper trusts the user's TEMP (junction swap / TEMP redirection) | **Fixed by removing the file.** The helper now writes nothing to disk: it sends the index through a private named pipe the app creates first (`FILE_FLAG_FIRST_PIPE_INSTANCE`, local clients only), connecting with `SECURITY_IDENTIFICATION` so the pipe server can't impersonate it. Tested: pipe round-trip, no duplicate pipe, real elevated Fast scan of C: with 0 files in TEMP |
| F-10 | 🟡 | "Only to the Recycle Bin" isn't absolute on USB/network drives | **Fixed.** The delete dialog warns "May be deleted permanently" (needs a tick) on removable/network drives; docs updated |
| F-11 | 🟡 | Helper wait has no timeout | **Fixed.** The app waits in 200 ms slices: a **Cancel** button in the Fast-scan dialog, plus a 10-minute limit. On either it tries to end the helper and stops waiting |
| F-12 | 🟡 | Drive probing at startup touches network drives | **Fixed.** `drives()` uses `GetDriveTypeW` and skips network drives |
| F-13 | 🟡 | Display spoofing (U+202E, control/zero-width characters) | **Fixed:** the delete dialog and Deletion history show hidden characters as `<U+202E>` (`safety::visible`); the log file is JSON (control characters escaped); the CSV quotes every path |
| F-14 | ⚪ | Resident data of tiny files passes through the MFT parser | **By design:** never stored (the index holds names and sizes only). PV-05 canary test to run |
| F-15 | ⚪ | Standard user + admin password (OTS UAC) | **Decided and documented:** with the pipe it now works across accounts. An administrator who types their password authorizes reading every file *name* on the drive (never contents), the same as any admin tool. A standard user who doesn't know an admin password can't use Fast scan |
| F-16 | ⚪ | Actions pinned by tag; no Dependabot | **Fixed:** all Actions pinned to commit SHAs, Dependabot for actions + cargo, `cargo audit` and **`cargo deny`** (licences OSI-only, advisories, crates.io-only sources; `deny.toml`) in CI. Signing pending (SignPath, after all phases) |
| OUT-02 | — | CSV formula injection | **Pass by design:** every exported path starts with the drive (`C:\…`), never `=`/`+`/`-`/`@`; fields are quoted |
| PA-05 | — | Index loader accepted names with `\`, `/`, `:` or `..` | **Fixed** in `Index::load` with a test |

Also from the 10 Nov retest: DLL planting (fixed, `/DEPENDENTLOADFLAG` + `SetDefaultDllDirectories`), Windows lock lost when scanning inside it (fixed).
