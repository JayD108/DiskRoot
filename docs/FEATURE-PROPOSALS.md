# Feature proposals: review and plan

[← Back to README](../README.md) · Reviewed 11 Nov 2026 against the code (commit 68f59a9) and Xylem's constraints.

## The constraints every feature must keep
1. **Speed:** scans no slower (C: ≈ 1.1 s with admin, ≈ 4.3 s without), no idle CPU, memory not meaningfully higher.
2. **Read-only scanning:** scanning never writes to the scanned drive.
3. **Careful changes:** anything that changes files only happens after the user confirms, is re-checked on disk first, and is undoable where possible (Recycle Bin).
4. **Least privilege:** the window never runs as admin. Admin work goes through a minimal helper, or through Windows' own tools behind Windows' own UAC prompt.
5. **Offline and private:** no network; nothing runs at startup unless the user turns it on.
6. **OSI-licensed components only** (SignPath requirement).

## Verdicts

| # | Feature | Exists today? | Speed / constraint impact | Verdict |
|---|---|---|---|---|
| 1 | NTFS compression (compact.exe /EXE:LZX) | No (Xylem already *measures* compressed and CompactOS sizes correctly) | No scan impact. It **writes** to files, which is new for Xylem; compressing takes minutes and some apps run slower on compressed files | **Viable, opt-in.** Run Windows' own `compact.exe` (absolute path), user files only, never inside Windows/Program Files |
| 2 | Dependency purge (node_modules, Rust target, venv…) | **Partly.** Rules classify node_modules, venv/.venv, __pycache__, package caches; Category view lists every place under "Dev stuff"; multi-select delete exists | None: data is already in the scan | **Viable, top priority.** Add "Mark all" on a sub-type, Rust `target` detection, one confirmation |
| 3 | Cache clearing (Adobe, Spotify, Docker…) | **Partly.** Rules + notes for many caches; Docker marked "use `docker system prune`" | None (rules are data) | **Viable for plain caches.** Not for Docker or WSL (their data sits in virtual disks), which get guided steps |
| 4 | Live updates while open | No | A non-admin watcher (ReadDirectoryChangesW) has no scan cost; idle cost is tiny if updates are batched (≤ 1 per second). Using the USN journal instead would need admin the whole time → breaks constraint 4 | **Viable with ReadDirectoryChangesW,** not the USN journal |
| 5 | Background size alerts | No | Needs Xylem running in the background → only as an **opt-in** tray mode; never on by default (constraint 5) | **Viable later, opt-in.** First: alerts while the app is open (built on #4) |
| 6 | Custom categorization rules | No (rules are compiled in) | Glob and extension rules use the same fast path → no slowdown. **RegEx per item would slow scans of 2M items**, so not offered | **Viable: glob/extension only.** User rules can add categories and colours but can **never unlock** protected places |
| 7 | WSL2/Docker VHDX compaction | Detection only (vhdx files and Docker places are classified) | Needs admin, WSL/Docker stopped, and Optimize-VHD needs Hyper-V (**not on Windows Home**, including this laptop); `diskpart compact vdisk` on a disk in use risks corruption | **Viable as a guided action:** detect, explain, check WSL/Docker are stopped, then run Windows' own diskpart behind UAC with a final confirm |
| 8 | Space Shifter (move + junction) | No | High data-loss risk (partial copies, open files, permissions); junctions break some apps (Store apps, OneDrive, anti-cheat, some backups) | **Not now.** Revisit later with copy → verify → swap → keep the original in the Recycle Bin |
| 9 | Shadow copies / restore points | No (System Volume Information is shown as "Restore points", locked) | Reading sizes needs admin; deleting restore points is permanent | **Viable, light:** show how much System Protection uses and open Windows' own System Protection settings to manage it. No deleting by Xylem |
| 10 | WinSxS cleanup via DISM | Note only ("use Disk Cleanup") | DISM takes minutes and needs admin; it's Microsoft's supported method | **Viable:** button that starts Windows' DISM `/StartComponentCleanup` in a visible console behind UAC. The window stays un-elevated |
| 11 | Orphaned AppData detector | No | Reads the uninstall registry keys (milliseconds). **Matching folder names to apps is fuzzy → false positives**, and AppData holds settings and saves | **Viable as hints only:** amber "Possibly left over from <app>" with the evidence shown, never "safe", never auto-marked |
| 12 | Age heatmap (cold storage) | No: the index has no timestamps | Needs last-write time per item: +4 bytes/item as days (≈ +7.6 MB for C:) and a few % more parse work. "Last accessed" is unreliable on Windows (often not updated) | **Viable with last-modified only,** stored compactly; folders show their newest file |
| 13 | Export plan to PowerShell (.ps1) | No | No speed impact. Risk: a script run on *other* PCs where paths differ, and quoting mistakes | **Viable:** generated script re-checks every path, uses the Recycle Bin, refuses links, quotes safely, and is only written, never run by Xylem |
| 14 | Archive & replace (zip, verify, delete) | No | Heavy I/O; the original going to the Recycle Bin frees nothing until it's emptied; a zip crate adds dependencies | **Later.** Lower value than 1/2/12; needs careful verify-before-delete |
| 15 | Installer & ISO graveyard | **Partly:** Downloads rule, .iso/.msi extension rules, Largest files and File types | None | **Viable, cheap:** a "Leftovers" view (old installers/ISOs in Downloads, Windows.old, $WINDOWS.~BT, $Windows.~WS) + rules for those folders |

## Implementation plan

### Phase 8: "Clean up with confidence" (no speed cost; mostly data + UI)
1. **Dependency purge (#2):** "Mark all" on any sub-type row in the Category view, then the existing multi-delete. Detect Rust `target` folders by a `CACHEDIR.TAG` file inside, the standard marker Cargo and other tools write; it's already in the scan, so no extra I/O. Show "Recreate with: npm install / cargo build / pip install -r …".
2. **Leftovers view (#15):** View → Leftovers: installers/ISOs/archives in Downloads older than 30 days (needs #12's dates; until then, by size), Windows.old, $WINDOWS.~BT, $Windows.~WS, with the right route for each (Windows.old → Windows' "Previous Windows installation" cleanup).
3. **More cache rules (#3):** Adobe Media Cache, Spotify, Discord/Teams caches, browser caches, NVIDIA/AMD shader caches, npm/pip/NuGet/Gradle caches. Each gets a note and safety label; warn if the owning app is running.
4. **Custom rules (#6):** `%APPDATA%\Xylem\rules.json` (glob/extension, name, colour), validated on load, merged after the built-ins, **unable to change protection**. File → Edit custom rules opens the folder; reload without restarting.
5. **Windows-native cleanups (#10, #9):** in the details pane for WinSxS, "Clean up with Windows (DISM)…" (elevated console, explained first); for System Volume Information, "Open System Protection settings…" plus the used size where available.

### Phase 9: "See it happen" (small, measured costs)
6. **Age heatmap (#12):** last-write time per item as u32 days (scanners already see it: MFT `$STANDARD_INFORMATION`, walk `FileFullDirectoryInformation`). Rolled up as "newest inside". A third colour mode: Category / Location / **Age**. Budget: ≤ +8 MB and ≤ +5% scan time, measured with the benchmark script before merging.
7. **Live updates (#4):** a ReadDirectoryChangesW watcher on the scan root while the app is open. Changed paths are re-measured individually and sizes patched, at most one UI update per second. If the buffer overflows, it shows "Changes detected, press F5" instead of guessing.
8. **Alerts while open (#5a):** "Tell me if a folder grows more than N GB": a desktop notification from the watcher. The opt-in tray/background mode (#5b) comes only if users ask.
9. **Orphan hints (#11):** uninstall registry → app names + install locations → fuzzy match AppData folders; amber hint with the evidence; no auto-marking.

### Phase 10: "Power tools" (opt-in, careful)
10. **Export to PowerShell (#13).**
11. **NTFS compression (#1):** compact.exe, user folders only, a preview estimate first, a log entry, and undo (`compact /U`).
12. **VHDX compaction, guided (#7).**
13. Later / under review: **Archive & replace (#14)** and **Space Shifter (#8)**.

Every item ships with: tests, a measured benchmark (no scan regression), docs (README feature line + SECURITY-AND-PRIVACY where relevant), and a changelog entry in the release notes.
