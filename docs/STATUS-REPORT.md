# Xylem: status report

**Date:** 10 Nov 2026 · **Repo:** `Storage/xylem` (commit `090a6d1` and later)

All numbers were measured on one machine: Core Ultra 9 275HX, NVMe SSD, Windows 11. C: is 2 TB NTFS with ~1.53 TB used and ~1.8M MFT records. Anything not measured is marked as such.

---

## 1. Decision: one native app, no web UI

**Xylem ships as one native `xylem.exe` only.** The earlier web UI (Tauri + Microsoft WebView2) was removed on 10 Nov 2026. The measurements that drove the decision:

| | Native `xylem.exe` | Former web UI |
|---|---|---|
| Download | **6.9 MB exe, 4.4 MB installer** | 8.2 MB + WebView2 runtime |
| Processes | **1** | 7 (app + 6 WebView2) |
| Memory with all of C: loaded | **~272 MB** | ~272 MB + ~248 MB WebView2 |
| Network connections | **0 (measured)** | 2 HTTPS to Microsoft (WebView2's own services) |
| Can honestly say "offline"? | **Yes** | No |

Removing it also took the whole browser-engine and HTTP stack out of the dependency tree, so there is less code to trust and less to explain when presenting.

---

## 2. Where Xylem is ahead

### Measured against WizTree 4.33 (black box: run normally and timed, no reverse engineering)

| Measure | Xylem | WizTree 4.33 | Notes |
|---|---|---|---|
| Scan + CSV export of every file, admin | **1.7–1.8 s** | 12.5–15.8 s | Same work for both: a full CSV |
| Scan without admin (+ CSV) | **4.0–4.9 s** | 26–30 s | WizTree falls back to slow listing without admin |
| Peak RAM in that test | **165–290 MB** | 844 MB | |
| Total vs Windows' "used space" (1,558.2 GB) | **1,558.0 GB** | 1,583 GB | WizTree counts hardlinked files once per name; Xylem counts each file once |
| Full-drive MFT scan, no export | 1.2–1.5 s typical, up to ~3–5 s when the disk is busy | **not measured** | WizTree's pure scan time is unknown, so the export comparison is the fair one |

### Versus the wider field (from research; only WizTree was measured)

| Gap in today's tools (from the PRD) | Xylem today |
|---|---|
| Fast tools look dated; nice ones are slow, paid or Mac-only | ✅ MFT speed, with a clean, modern native UI |
| They show *where*, not *what* | ✅ **95% of bytes classified** into Dev stuff, AI & data, Apps & Games, System, Temp & caches… |
| No "is it safe to delete?" | ✅ Plain-English note + 🟢/🟡/🔴 label for known places; protected folders locked in the engine |
| Deleting is scary | ✅ Always to the Recycle Bin, with a confirmation; Windows warns if something is too big to recycle |
| The fastest tool is closed source and paid for work use | ✅ Open source (license MIT/Apache) and free |
| Sizes are wrong (hardlinks, cloud files, compression) | ✅ Hardlinks counted once (MFT), OneDrive placeholders ≈ 0, compressed and CompactOS sizes correct |
| One visualization only | ✅ Sunburst + treemap, grouped by location *or* by category |
| Admin tools run fully elevated | ✅ The window never runs as admin; only a helper does, for a few seconds, and then exits |
| Privacy | ✅ No network code; the native app was measured at 0 connections |

### Engineering wins that are hard to copy
1. **One shared, overlapped handle for MFT reads.** The first version used one handle per thread, and each cost ~0.35 s. Sharing one took the scan from 3.4 s to 1.2 s.
2. **Struct-of-arrays index** (~50 bytes per file) with a bottom-up size roll-up in one pass.
3. **Data-driven classifier** (`rules.json`): ~0.1–0.2 s for 1.8M files. New rules need no code changes.
4. **Hardened elevation:**
   - The helper accepts only one exact command line.
   - It writes only a *new* `xylem-*.idx` file directly inside `%TEMP%`, and never overwrites.
   - Its output is validated before loading.

---

## 3. Where Xylem is behind or unproven (honest list)

| Area | Status |
|---|---|
| **Installer and code signing** | ❌ None. Smart App Control can block unsigned builds, and the helper should live in Program Files. Needed before any public release |
| **Instant reopen (USN journal)** | ❌ Planned (Phase 5). Every launch rescans |
| **"What changed since last scan"** | ❌ Planned (Phase 5). No snapshots yet |
| **Duplicate finder, file-type view, search** | ❌ Not built. WizTree has extension and top-files views |
| **Hardlinks** | ⚠️ Counted correctly once, but the *other* names of a hardlinked file aren't listed yet |
| **Memory of scan data** | ⚠️ ~255–272 MB for 1.8M items. Could be streamed and shrunk |
| **First raw read of the MFT** | ⚠️ 0.36–0.8 s, sometimes more when the disk is busy (Windows flushes its cache). Not yet optimized |
| **Tested hardware** | ⚠️ One laptop, one NVMe drive. No HDD, other PCs, non-English names at scale, or very fragmented MFTs in the wild |
| **Platforms** | Windows only. Fast scan works on NTFS only (other drives use the folder walk) |
| **Security.md consent screen and shield** | ⏸ Designed in the prototype, waiting for your decision |
| **Multi-select delete, Explorer right-click menu, CSV export from the app** | ❌ Not built (the CLI has CSV export) |

---

## 4. What has been built (timeline)

| Step | Result |
|---|---|
| PRD review + research | Found flaws in the original plan: `FSCTL_ENUM_USN_DATA` has no sizes, no admin model, no UI choice. Wrote TECHSTACK |
| **Phase 1:** folder walk | Parallel `NtQueryDirectoryFile`, relative opens, 8 threads. C: in **~3–3.6 s without admin**, exact match against `std::fs` |
| **Phase 2:** MFT reader | Boot sector → $MFT runs → 4 MB unbuffered parallel reads → bounds-checked parsing → tree. **C: in ~1.2–1.5 s.** Total matches Windows' used space |
| Speed rounds | Tree build 0.70 → 0.21 s (synthetic); RAM 290 → 255 MB; classification 0.34 → ~0.1–0.17 s |
| **Phase 3:** desktop app | Tauri app with sunburst, treemap, list and the admin helper |
| Security review | Fixed a temp-file overwrite risk in the elevated helper; `create_new`, path guard, debug-only override, length-checked loading |
| **Phase 4:** meaning | Categories, by-category view, notes and safety labels, protected paths, Recycle-Bin delete |
| Rename | Drishti → **Xylem** everywhere |
| Audit | Proved read-only disk access and no network code; **found WebView2's Microsoft connections and ~248 MB of overhead** |
| Design | Prototype v1 (rejected: looked like a web page) → **v2 "classic utility, modern finish"** (approved) |
| **Native app** | Engine API `crates/session`, **native `xylem.exe` (egui)**; web UI removed |
| **Release + security** | Installer, signing hook, checksums; delete re-check, OneDrive/Git/large warnings, deletion history, integrity panel, first-run notice, fuzzing (3 bugs found and fixed) |
| Records | Git history, DEVLOG, HANDOFF, README, UI-ARCHITECTURE, this report |

**Quality today:** 52 automated tests, zero lint warnings. Tested end to end on real data: scans, both views, delete to the Recycle Bin, and the admin fast scan through the UAC prompt.

---

## 5. Recommended next steps (in order)
1. **Installer + code signing.** Install into Program Files with a signed exe. This closes the last elevation risk and stops Smart App Control blocks. It's required before sharing Xylem with anyone.
2. **Phase 5: instant reopen + "what changed".** Index cache + USN journal: the biggest visible win, which no free competitor has.
3. **Memory diet.** Stream MFT chunks into the index and shrink the classification arrays (target: under 150 MB for C:).
4. **Consent screen + shield** (after your decision on the Security.md wording).
5. **Breadth:** multi-select delete, file-type view, duplicate finder, an in-app CSV export, the Explorer context menu.
7. **Wider testing** on other PCs, HDDs and very large drives before the first public release.
