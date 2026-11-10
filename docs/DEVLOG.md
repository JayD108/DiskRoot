# Dev log

A running record of what was asked, what was decided, and what was built and measured, so no context is lost between sessions. **Append a new dated section every session.** Newest entries go at the bottom.

Machine: Core Ultra 9 275HX (8 P-cores + 16 E-cores), NVMe SSD, Windows 11, C: = 2 TB NTFS with ~1.56 TB used and ~1.77M MFT records. Toolchain: Rust 1.99.1 (MSVC), Tauri 2.12.5, Node 24 (not used by the build).

---

## 2026-11-10: Session 1 (Phases 0 to 3)

### Planning
- **Ask:** read `PRD.md` and review it.
  - **Findings:** `FSCTL_ENUM_USN_DATA` returns no file sizes, correct sizes (A3) are underestimated, the admin model was missing, and a UI stack hadn't been chosen.
- **Ask:** deeper research plus a stack that beats every existing tool.
  - **Result:** `TECHSTACK.md`: a Rust core, a raw MFT reader in an elevated helper, a USN-journal index cache, and a Tauri 2 UI.
- **WizTree inspection:** v4.33 is installed. Its EULA forbids reverse engineering, so it is used **only as a black-box benchmark**.

### Phase 1: user-mode scanner
- Installed Rust with `winget install Rustlang.Rustup`. VS Build Tools (C++) were already present.
- Workspace crates: `core` (struct-of-arrays Index), `scan-walk` (parallel `NtQueryDirectoryFile`), `cli`, `bench`.
- First real C: scan: 15.4 s cold, 4.88 s warm, 138 MB RAM. Exact match against std::fs on `C:\Program Files`.
- **Snag:** Smart App Control sometimes blocks freshly built unsigned exes (os error 4551). Retrying worked, and later builds ran.

### Speed round 1 (folder walk)

| Change | Warm time |
|---|---|
| Baseline | 4.88 s |
| 12 threads | 4.25 s |
| `FileFullDirectoryInformation` (no 8.3 names) | 4.02 s |
| `NtOpenFile` relative to the parent handle + LIFO stack | 3.98 s |
| 8 threads (default) | 3.61 s |

More threads were slower: P/E cores plus NTFS contention.

### WizTree head-to-head (both writing a full CSV)

| Mode | WizTree | xylem |
|---|---|---|
| User | 26–30 s, 790 MB | 4.0–4.9 s, 117 MB |
| Admin | 15–18 s, 844 MB | 4.1–6.4 s, 165 MB (still folder walk at that point) |

Accuracy vs WizTree: 279/309 top-level folders exact. The 12 GB gap was System Volume Information plus NTFS metafiles.

### Phase 2: MFT reader (`crates/scan-mft`)
- Boot sector → `$MFT` record 0 → extents (with attribute-list support) → 4 MB unbuffered reads → bounds-checked record parsing with fixups → extension-record merge → parent links with sequence check → BFS.
- First version: 3.37 s. Time grew linearly with threads because **each raw-volume handle costs ~0.35 s**.
- **Fix: one shared overlapped handle**, which brought it to 1.15–1.27 s.
  - About 0.36 s of that is the first raw MFT read (likely NTFS flushing its cache, which can't be avoided).
  - Read+parse runs at ~3.2 GB/s, the SSD's limit.
- **Accuracy:** MFT on-disk total 1,557.98 GB vs Windows used space ~1,558.2 GB. The folder walk says 1,563.9 GB and WizTree 1,583 GB, because both count hardlinks once per name.
- Head-to-head (admin, CSV): **xylem 1.68–1.82 s, 290 MB vs WizTree 12.5–15.8 s, 844 MB**.

### Speed round 2
- Synthetic `build_tree` benchmark (1.77M records, random parents): 0.705 s → **0.209 s**.
  - BFS is now split into an id pass over the compact CSR plus a sequential column fill.
  - `Index::from_columns` and `with_capacity` were added.
- Real C: scan inside the app: 1.29 s.

### Phase 3: Tauri app (`crates/app`)
- Vanilla HTML/JS/CSS UI with no build step: Canvas 2D sunburst, squarified treemap, side list, breadcrumbs, tooltip.
- Rust sends only a pruned view: depth ≤ 6, items under 0.2% grouped, at most 120 children per folder.
- Admin fast scan: `ShellExecuteExW("runas")` runs `xylem.exe scan C: --mode mft --save-index <temp>`, then the app loads the validated index.
- Verified with window-only screenshots plus UI Automation (`tools/ui-*.ps1`): sunburst, treemap, the admin flow end to end (1.29 s), and the folder-only error toast.
- Fixed:
  - The full-ring seam line.
  - Sibling colors were too similar.
  - The view-switch accessibility role (tabs → toggle buttons).
  - The list's scroll position wasn't reset.

### Review pass (before Phase 4)
- **Security, elevated helper:**
  - `--save-index` uses `create_new` (no overwrite through planted links).
  - The path must be `xylem-*.idx` directly in `%TEMP%`.
  - The app uses a random file name and loads with a header-vs-file-length check.
  - The `XYLEM_HELPER` override works in debug builds only.
- **UI:**
  - A navigation sequence token, so stale responses can't overwrite newer ones.
  - The list says "+ N smaller items not listed" when cut at 400.
- Test scripts moved from the session scratchpad into `tools/`. The project is now a git repo, and `docs/` holds the PRD, tech stack, this log and HANDOFF.

## 2026-11-10: Session 2 (Phase 4 finish, rename, audit, UI prototype)

### Phase 4 finished
- Extension-classified files are now grouped **by folder**. Datasets showed "1014 more places"; it now shows real folders.
- File counts for categories and sub-types.
- Stats keep plain arrays for the common case, plus a map only for mixed places: 0.18 s → ~0.09 s.
- Verified in the app on a throwaway `%TEMP%\xylem-demo` project:
  - The delete dialog shows the path, size, *Dev stuff › node_modules*, a 🟢 badge and the note.
  - Confirming moved `node_modules` to the Recycle Bin and left `notes.txt` untouched.
- Fixed:
  - The header counts weren't live after a delete.
  - "1 files" plural.
  - Short `MRITTU~1` roots (now expanded with `canonicalize`).

### Renamed Drishti → Xylem
- Folder `Storage/xylem`, crates `xylem-*`.
- Exes `xylem.exe`, `xylem-app.exe`, `xylem-bench.exe`.
- Tauri identifier `com.xylem.app`.
- Helper files `xylem-*.idx`, env vars `XYLEM_TRACE` / `XYLEM_HELPER`, index magic `XYLMIDX1`.
- 35 tests pass.

### Re-verified end to end under the new name
- Walk: C: median 3.26 s; `verify` exact on Program Files.
- MFT in the app via UAC:
  - All of C: in category view, 95% classified.
  - The helper exited, and no `xylem-*.idx` was left behind.
- MFT timing (5 runs): median **1.48 s**. Read+parse 0.53 s (unchanged), build 0.11–0.25 s (faster), peak RAM **255 MB** (was 290), index 90 MB (was 126).
  - "extents" (the first raw read) is 0.71–0.80 s, up from 0.36. That code is unchanged; it's likely the NTFS metadata flush, which depends on how busy the disk is.
  - One in-app run showed 5.04 s right after a build. **Open question:** can that flush cost be reduced?

### Security audit facts (for the Security.md discussion)
- The volume is opened `read(true)` + NO_BUFFERING + OVERLAPPED. The MFT code has no write APIs.
- `unsafe` blocks: core 0, scan-walk 5, scan-mft 7, app 3 (all FFI).
- `reqwest` and `hyper` are in Cargo.lock as **non-Windows-only** Tauri dependencies. `cargo tree --target x86_64-pc-windows-msvc` shows nothing, so they aren't compiled on Windows.
- **Runtime network measurement:** the WebView2 browser process keeps **2 HTTPS connections to Microsoft** (2603:1046::/32, Azure). They stay even with `--disable-background-networking --disable-component-update`, which are confirmed in its command line. Xylem's code makes none.
- **WebView2 memory:** ~248 MB private across 6 processes; xylem-app itself ~9 MB.

### UI prototype (design.md)
- `xylem-ui-prototype/` (index.html, styles.css, app.js): standalone, no build, no network.
- The plant-tissue theme:
  - Stem cross-section sunburst with a pith and growth rings.
  - Unfurl animation once, and a "sap rising" scan bar.
  - Vessel texture done in pure CSS.
  - Inspector panel; consent and shield modals with corrected (honest) copy.
- Switches: `?nowelcome`, `?still`. Rendered with headless Edge to verify.

### README
- Rewritten to merge the naming story with the real architecture.
- Inaccurate claims from naming_and_understanding.md corrected: WebGL → Canvas 2D; FSCTL volume data → boot sector; Aho-Corasick → hashed rule lookup; "zero-allocation"; USN instant reopen is planned, not built; "1.7 s" was scan+export.

### Tooling note
- A screen-region capture once grabbed the user's browser. `ui-screenshot.ps1` now uses PrintWindow (app pixels only, no focus steal).

### Same day, later: UI direction v2 + native spike
- **User feedback on prototype v1:** "looks like AI-slop webpage, not a tool". Wants a legacy-styled + modern, clean, simple UI with Xylem DNA, a smaller size, and **both** a native UI and an optional web UI.
- **Prototype v2** (`xylem-ui-prototype/`, replaces v1; v1 is in git history):
  - A classic utility structure (menu bar, toolbar, tree-table with columns, map pane, legend, details pane, status bar) with a modern finish.
  - Botanical palette, moss-green accent, light and dark themes, no glows or gradients.
  - Working menus, expand/collapse, keyboard nav, filter, synced hover/selection, a fake scan dialog.
  - URL switches `?dark&category&treemap&about`.
  - Rendered and checked with headless Edge. Fixed: invisible bar fills, clipped details pane, dark-theme label contrast.
- **Native UI spike** (`spikes/native-egui`, eframe 0.37 + glow, LTO + strip):
  - **4.46 MB exe, 1 process, ~59 MB private memory, 0 network connections**, idle CPU ~1% of one core.
  - Compare the Tauri app: 8.6 MB + WebView2, 7 processes, ~258 MB, 2 Microsoft connections.
  - eframe 0.37 API: `App::ui(&mut self, ui, frame)`, `egui::Panel::top/bottom/left(..).show(ui, ..)`.
- **Proposal written:** `docs/UI-ARCHITECTURE.md` (native default + optional web add-on, shared `xylem-session` crate, one `xylem.exe` for UI + CLI + helper). Waiting for the user's go-ahead.

### Same day, later: native app built (user approved "go with native")
- **New `crates/session` (xylem-session):** the UI-agnostic engine API, extracted from the Tauri backend.
  - `Scanned` provides view, list, crumbs, category_view, `cat_children` (new, for the tree-table), info, and `prepare_delete` → `PendingDelete::recycle` → `apply_delete`.
  - Also holds `scan` / `scan_admin`, `elevate.rs`, `recycle.rs`, and **`helper.rs`**: the elevated helper is now `xylem.exe --helper-scan C: --save-index <file>`.
    - It accepts only that exact command line and keeps the old guards (`xylem-*.idx` directly in `%TEMP%`, `create_new`).
    - Exit codes 0/2/3/4 map to clear app errors.
- **New `crates/native` (`xylem.exe`, the default app):** eframe 0.37 (glow + accesskit), modules main/app/map/theme/settings.
  - Menu bar, toolbar, tree-table (virtualized rows, twisties, % bars, category column, context menu), map pane (sunburst meshes / squarified treemap, upright rotated labels, legend), details pane (safety + note + actions), status bar.
  - Modal progress and delete confirmation; worker threads for scans and deletes.
  - Settings in `%APPDATA%\Xylem\settings.json`; Segoe UI loaded from `C:\Windows\Fonts`; new growth-ring icon (`crates/native/assets`, also copied into the web add-on).
  - Accessible tree rows (`Role::TreeItem` + selected state), which also make UI Automation tests possible.
- **`crates/app` became the optional add-on `xylem-web.exe`** (identifier `com.xylem.web`), a thin layer over session. The folder couldn't be renamed (locked), so it stays `crates/app`.
- **CLI binary renamed `xylem-cli.exe`;** its `--save-index` was removed (the helper lives in `xylem.exe` now).
- **Release profile:** `lto = "fat"`, `strip = true`, `debug = false`. Sizes: **xylem.exe 6.78 MB**, xylem-web.exe 8.16 MB, xylem-cli.exe 0.82 MB, xylem-bench.exe 0.91 MB.
- **Verified:**
  - 40 tests, clippy clean.
  - Native app on the home folder (location + category views).
  - Delete end to end on `%TEMP%\xylem-demo3` (selected via accessibility → dialog → Recycle Bin; notes.txt kept).
  - **Admin fast scan via UAC:** the helper exited, 1 process left, 0 leftover idx; C: 1,495,619 files in 3.28 s (first-raw-read variance).
  - **Native with all of C: loaded: ~272 MB private, 1 process, 0 network connections, idle CPU 16 ms / 5 s.**
  - Web add-on smoke-tested on the shared engine.
- **Fixed during testing:**
  - The details pane was too short.
  - egui followed the Windows dark theme over the app's setting (`set_theme` + `set_visuals_of`).
  - Seam line on full rings; "1 files" plurals.
  - The user switched the app to dark mode and clicked a delete confirm while testing; settings persistence worked as designed.
- **Follow-ups:**
  - Port the v2 design into the web add-on's HTML.
  - Reduce scan-data RAM (index + classification ≈ 200+ MB for 1.8M items).
  - Investigate the first-raw-read flush cost.

### Same day, later: installer, signing hook, security features, web UI removed
User asked: "go for next step and also add the security measures ... and think of security measures that will enhance the tool's use case". Mid-way the user added: "drop web ui completely, only the exe, so it would be safer to present".

- **Installer:** `installer/xylem.iss` (Inno Setup 6.7.3, installed per-user via winget).
  - Per-machine install into `{autopf}\Xylem` (admin-only writable, which protects the Fast-scan helper); `/CURRENTUSER` allows a per-user test install.
  - Parts: app (fixed) and cli (default). Start Menu shortcut, uninstaller. Settings and the deletion log are kept on uninstall.
  - **Tested twice:** silent per-user install into %TEMP%, then the installed `xylem-cli --version`, then a silent uninstall. Nothing left behind (folder, shortcut, HKCU uninstall key).
- **`tools/release.ps1`:** build → stage → **optional signtool signing** → ISCC → portable zip → `SHA256SUMS.txt`, all in `dist/` (gitignored).
  - Signing turns on with `XYLEM_SIGN_THUMBPRINT`, `XYLEM_SIGN_PFX` (+ password) or `XYLEM_SIGN_DLIB` (+ metadata, for Azure Trusted Signing), and runs `signtool verify /pa` afterwards.
  - The timestamp server is the only network step.
  - Output today (unsigned): setup 4.40 MB, portable zip 3.33 MB, xylem.exe 6.90 MB, xylem-cli.exe 0.82 MB.
  - Windows PowerShell 5.1: `$ErrorActionPreference = "Stop"` turns cargo's stderr into errors, so the script checks exit codes.
- **Added `LICENSE-MIT` + `LICENSE-APACHE`.** The README claimed them, but the files were missing. The Apache text came from apache.org.
- **`crates/session/src/safety.rs`:**
  - `verify_before_recycle`: still exists, same file/folder type, link status unchanged, and no folder between the scan root and the item became a link. Tested with a real `mklink /J` swap.
  - `DeleteWarnings { cloud, git_repos, large }`:
    - Cloud: OneDrive env-var folders or CLOUD placeholders in the subtree.
    - Git: `.git` entries in the subtree.
    - Large: ≥ 10 GB or ≥ 50,000 files.
  - Deletion log as JSON lines in `%LOCALAPPDATA%\Xylem\deletions.log` (recycled / cancelled / refused), with UTC timestamps (no date crate).
  - `PendingDelete::recycle` now re-checks, recycles and logs. `prepare_delete` refuses node 0 (the scan root).
- **`crates/session/src/integrity.rs`:**
  - Program Files location check.
  - Authenticode via `WinVerifyTrust` (raw-dylib, no revocation, cache-only URLs, so it stays offline). Tested: Edge = Signed, the test binary = Unsigned.
  - Windows' own exes are catalog-signed, which this embedded-signature check reports as unsigned. That's irrelevant for our exe.
- **Native UI:**
  - First-run **Welcome** notice (`settings.seen_notice`; a command-line scan waits until it's closed).
  - Painted **shield button** and Help → Security & privacy, with a "This copy of Xylem" integrity section.
  - **File → Deletion history…** (table plus "Open log folder").
  - Delete dialog shows amber warnings and an **"I understand" tick** that the button requires.
  - Fixes: wrapped labels in a Grid made the modal taller than the window (now a stacked layout); every dialog has its own window id, so sizes don't carry over.
- **Fuzz tests:**
  - `crates/scan-mft/src/fuzz.rs`: mutated and noise records, run lists, boot sectors, random parent graphs through `build_tree`.
  - `crates/core/src/store.rs`: mutated index files.
  - Deterministic seeds; `XYLEM_FUZZ_ROUNDS` sets the length (2M rounds: 123 s, pass).
  - **Real bugs found and fixed:**
    1. A parent-link cycle → `from_columns` panic. Columns are now sized by the BFS-reached count.
    2. A boot sector could declare record sizes up to 2 GB or clusters up to 128 GB → limited to 512–4096 B sectors, ≤ 2 MB clusters, ≤ 64 KB records.
    3. u64 overflow in `compute_totals`, category sums, extension merge and `remove_subtree` (debug panic, silent wrap in release) → saturating arithmetic.
- **Speed unchanged:** build_tree 1.77M records 0.188 s (was ~0.21); classify 0.115 s + stats 0.096 s.
- **Web UI removed completely** (user's decision):
  - `crates/app` deleted and taken out of the workspace.
  - Tauri, wry, WebView2 and reqwest are gone from `Cargo.lock`.
  - Installer, release script, README and docs updated.
  - Before removal, its delete got the same warning confirmation, and trimming its Tauri permissions was tried. Neither could be UI-tested (UI Automation can't type into WebView2), which was one more reason to drop it.
- **Verified in the real app** (isolated `APPDATA`/`LOCALAPPDATA`; the user's settings and log untouched):
  - Welcome → Scan → shield panel (Location ! and Signature !, correct for a dev build).
  - Deleting `proj` containing `.git`: warning shown, button disabled until ticked, recycled, log line written, history lists it.
- **52 tests pass, clippy clean.**

### Same day, later: going public on GitHub, SignPath preparation
- **Repo:** https://github.com/JayD108/Xylem (public).
  - The commit author email was switched to the GitHub no-reply address before the first push, so the user's personal email isn't public (user's choice). The rewrite covered only local, unpushed commits.
- **Windows version resource** (`build.rs` + `winresource`) in xylem.exe and xylem-cli.exe: ProductName "Xylem", version 0.1.0, description, and the app icon embedded in the exe. SignPath enforces product name/version metadata.
- **GitHub Actions:**
  - `ci.yml`: test + `clippy -D warnings` on windows-latest.
  - `release.yml`: on a `v*` tag, install Inno Setup, test, run `tools/release.ps1`, and upload the unsigned installer, zip and checksums. The SignPath signing step is prepared but commented out until acceptance.
- **README "Code signing policy" section:**
  - SignPath attribution, with the application marked as pending.
  - Team roles (JayD108 for all of them).
  - SignPath's standard privacy statement ("will not transfer any information...").
  - Uninstall notes.
- **HANDOFF:** removed personal paths and the user-preferences section (those live in the assistant's local memory, not the public repo).

### Same day, later: SignPath Foundation prerequisites (user is applying today)
- **License audit** (`cargo metadata`): every dependency offers MIT/Apache/Zlib/BSD/ISC, except **egui's bundled fonts (Ubuntu Font Licence, not OSI-approved)**.
  - Fix: dropped eframe's `default_fonts` feature. `theme::install_fonts` now builds `FontDefinitions::empty()` from Windows fonts only, with fallbacks:
    - Proportional: Segoe UI → Tahoma → Arial
    - Semibold: seguisb → segoeuib → Tahoma Bold → Arial Bold
    - Monospace: Consolas → Courier New
    - Symbols: Segoe UI Symbol
  - xylem.exe 6.96 → 6.30 MB. Screenshot-checked: all text and the ▶ ⚡ ◎ ▦ symbols render.
- **Installer switched from Inno Setup to NSIS.** Inno Setup's license isn't OSI-listed; NSIS is zlib, with LZMA under CPL.
  - `installer/xylem.nsi`: per-machine into `$PROGRAMFILES64\Xylem`, MUI2 pages, components (app required, cli default, desktop shortcut optional).
  - HKLM uninstall entry; the uninstaller deletes only the files it installed (no recursive delete).
  - The installer also carries version info.
  - A downloaded portable NSIS couldn't be run locally (the sandbox blocks running downloaded code), so the installer is built and tested in GitHub Actions instead.
- **`tools/release.ps1` split into steps:** `-Step build` stages the exes; `-Step package -StageDir <dir>` builds installer + zip + checksums from given (e.g. SignPath-signed) exes; `all` does both.
- **`release.yml` rewritten:**
  - Tag/version check → test → build → upload exes → *SignPath signs exes* → NSIS package from those exes → **real silent install + uninstall test** (files, Start Menu shortcut, HKLM entry, installed CLI runs, nothing left behind) → upload installer → *SignPath signs installer* → checksums → GitHub Release (tags only).
  - The SignPath steps switch on automatically when the secret `SIGNPATH_API_TOKEN` and the variable `SIGNPATH_ORGANIZATION_ID` exist.
- **New docs:**
  - `docs/SIGNPATH.md`: eligibility checklist mapped to the repo, prepared application answers, setup after acceptance, artifact configuration XML for `exes` and `installer`, end-to-end flow.
  - `SECURITY.md`: private vulnerability reporting.
  - `.github/release-notes.md`: download-page text with the code signing policy and privacy statement.
- **Still on the user:** enable GitHub 2FA (and MFA on SignPath), submit the application, and later add the secret/variable. "Verifiable reputation" is SignPath's judgement and can't be guaranteed by code.

### Same day, later: README restructured
- The user asked for a short README with no comparisons: features, the problems solved, a picture, how it works and how to use, with details in separate files.
  - **README:** tagline, screenshot, the problem, features, a small how-it-works sketch, install, how to use, safety (3 lines), the required *Code signing policy* section, a links table, license.
  - **Split out:** `docs/HOW-IT-WORKS.md` (architecture, name, accuracy), `docs/SECURITY-AND-PRIVACY.md`, `docs/DEVELOPMENT.md` (build, CLI, tests, releases, layout).
  - The WizTree numbers stay only in `docs/STATUS-REPORT.md`.
- **Screenshots:** `docs/screenshots/xylem-location.png` (C:\Windows, light, sunburst, details pane with "Don't delete by hand") and `xylem-category.png` (dark, treemap by category). I used C:\Windows on purpose: a Program Files shot showed the user's installed apps.
- **Release dry run** (workflow_dispatch) passed: tests, build, NSIS installer **2.34 MB**, zip 3.07 MB, real install/uninstall test OK.
- **Not done yet:** tagging v0.1.0, which publishes the first public GitHub Release. The sandbox blocked it; the user must approve or run `git tag -a v0.1.0 -m "Xylem 0.1.0"` and `git push origin v0.1.0`.

### Same day, later: full security retest (user asked for a 1–10 score with solid reasons)
- **Retest:**
  - 53 tests pass.
  - `clippy -D warnings` clean.
  - **`cargo audit`: 0 known vulnerabilities in 342 crates.** Only 3 "unmaintained" notes, for Linux-only zbus crates that the Windows build doesn't compile.
  - **No network crates in the Windows build** (tokio appears only via the wasm target).
  - `unsafe` is limited to 23 small FFI sites in 7 files.
  - The PE has ASLR, high-entropy ASLR, DEP and /GS.
  - Read through the elevation code again.
- **Found and fixed: DLL planting.** The exe imported 7 DLLs that aren't KnownDLLs (dwmapi, uxtheme, opengl32, uiautomationcore, wintrust, bcryptprimitives, vcruntime140).
  - Proof: a junk `dwmapi.dll` next to a copy of xylem.exe made it hang on a "bad image" error, so Windows loaded it from the app folder. For a portable copy in Downloads, a planted DLL would run inside the **elevated** helper.
  - Fix: link flag `/DEPENDENTLOADFLAG:0x800` (static imports from System32 only) and `SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32)` first in `main` (`integrity::harden_dll_search`).
  - Verified: with all 7 junk DLLs planted, the helper runs normally (exit 2) and the full GUI starts and renders.
  - I tried a static CRT (`+crt-static`, removes the vcruntime140 dependency). Back-to-back benchmarks showed build_tree 0.17–0.19 s dynamic vs 0.22–0.24 s static, so it was reverted (speed rule). The load flag already forces vcruntime140 to come from System32.
- **Found and fixed: protection bypass by scanning a subfolder.** For a scan root like `C:\Windows\Fonts`, only the root's own last path component was matched against the rules, so nothing inside was "System > Windows", and nothing was locked.
  - Fix: the root's path and each of its ancestors are matched, deepest first.
  - Tests: unit tests for `C:\Windows\Fonts`, `System32\drivers` and lowercase/slash paths, a WinSxS subfolder keeping its specific rule, and a real-disk session test that scans `%WINDIR%\Fonts` and `System32\drivers` and checks every item refuses deletion.
- **CI:** added a `cargo audit` job.
- Speed after the fixes: build_tree 0.19 s.

### Same day, later: Phase 5 (instant reopen, snapshots, "what changed", privacy controls)
- **`crates/session/src/remember.rs`:**
  - Cache in `%LOCALAPPDATA%\Xylem\cache\<fnv(root)>.{idx,snap,prev.snap}`.
  - `Snapshot` (folder paths + sizes, own `XYLMSNP1` format, strict loader with a fuzz test).
  - `diff` (threshold max(1 MiB, 0.1% of total); drops folders whose change is ≥ 75% explained by one child; new/removed folders included).
  - `remember` (runs on a background thread after the scan is shown; atomic temp+rename writes; rotates `.snap` → `.prev.snap`; prunes to 8 roots).
  - `reopen`, `last_changes`, `forget_index` (after a delete), `forget_all`.
  - `age_text` gives "3 h ago" style ages (no time zone needed).
- **Engine:**
  - `Scanned.index` is now `Arc<Index>`, so the background save shares it. `apply_delete` uses `Arc::make_mut`, which copies only if a save is still running. `Index` derives Clone.
  - New `Scanned::remember_parts`, `Scanned::find_rel`, `session::reopen`.
- **App:**
  - Reopens the last path at start (job "Opening the remembered scan…").
  - A fresh scan triggers the background remember step plus a status line "Since the previous scan (…): +X".
  - **View → What changed since the previous scan…**: dialog with change/now/before/folder, "new"/"gone" marks; click a row to jump to the folder in the tree.
  - **File → Remember scans** (checkbox, default on) and **Forget remembered scans**.
  - Welcome and Security panels now disclose the cache honestly.
  - After a scan, the path box and `last_path` use the scan's own root (long path), so reopen finds it. Before this fix the 8.3 short path `MRITTU~1` missed the cache.
  - Blank command-line arguments are ignored. Before, an empty `""` opened a "Nothing to scan" dialog.
- **Verified in the real app** (isolated APPDATA/LOCALAPPDATA):
  - Small folder: scan → change files → restart reopens the remembered scan → rescan → "What changed" lists exactly `projects\app\node_modules +60 MB`, `old-cache gone −20 MB`, `downloads new +15 MB`.
  - **Whole C:** 1,606,795 files; cache 89 MB idx + 36 MB snap; **reopened in 0.37 s** (walk scan 3.7–5.1 s); memory 248 MB reopened vs 278 MB after a fresh scan.
- **Speed A/B:** the CLI from commit 090a6d1 vs today, same disk, alternating: old 4.70/5.56/4.88 s vs new 4.75/4.60/3.92 s. No regression. One GUI run showed 11.3 s; two re-runs gave 3.7 and 5.1 s (a cold cache after builds).
- 4 new tests (diff, new/removed, snapshot round-trip + fuzz, keys). All tests pass, clippy clean.
- Dark theme showing up in two test runs was traced to the setting being switched in a test window left open on screen (it can't be reproduced without interaction); not a bug.

### Same day, later: security test plan findings F-01…F-16
- The user supplied a clearance test plan (`Storage/security test.md`). Status per finding: `docs/SECURITY-STATUS.md`.
- **Fixed:**
  - F-01/F-03: NSIS has no directory page, `/D` is ignored, "Start Xylem" goes through explorer (un-elevated).
  - F-02: release builds refuse Fast scan outside Program Files.
  - F-04: absolute explorer path.
  - F-05: odd names and long paths are refused.
  - F-06: file-identity chain, including the scan root, recorded when the dialog opens and checked before deleting. The app now confirms the exact `PendingDelete` it armed.
  - F-07: refuses folders that contain protected places.
  - F-08: `real_path` plus a file-identity hard lock (works through subst/UNC/8.3).
  - F-10: warning for USB/network drives.
  - F-12: `GetDriveTypeW`, network drives skipped.
  - F-13: `visible()` in the delete dialog.
  - F-16: Actions pinned by SHA, Dependabot.
  - PA-05: loader validates names.
- **Open:** F-09 (TEMP by handle), F-11 (helper timeout/cancel), F-15 (OTS UAC decision), F-13 for the log/CSV, cargo-deny, then the VM test run.
- 61 tests, clippy clean.
- **Git:** at the user's request there is no Claude attribution. The `Co-Authored-By` trailers were stripped from all commits (local backup branch `backup-before-rewrite`). The user will delete the GitHub repo and push to a new one themselves.

### Same day, later: new repo, first release
- The user deleted the old GitHub repo and pushed the cleaned history to a new https://github.com/JayD108/Xylem (no Claude contributor).
- README: added an Acknowledgements section (Claude used as an AI assistant), as the user asked.
- **v0.1.0 published** by the release workflow from tag `v0.1.0` → commit 7d3097e: installer 2.50 MB, portable zip 3.28 MB, SHA256SUMS. Unsigned (signing after all phases). The workflow's real install/uninstall test passed; CI on main is green.
- An old local `v0.1.0` tag (pre-cleanup commit, never pushed) was replaced.
- Dependabot opened PRs for Actions major bumps (checkout 7, upload-artifact 7, signpath action 3). Not merged yet; review them (the signpath action input names may differ in v3).

### Same day, later: remaining security findings F-09, F-11, F-13, F-15, cargo-deny
- **F-09, by design change:** the elevated helper no longer writes any file.
  - `elevate.rs`: the app creates `\.\pipe\xylem-<pid>-<nanos>-<random>` (FIRST_PIPE_INSTANCE, REJECT_REMOTE_CLIENTS, inbound, default DACL) before ShellExecuteEx, reads it on a thread, and validates with `load_checked`.
  - `helper.rs`: `--helper-scan C: --pipe <name>`, strict name check, connects with SECURITY_SQOS_PRESENT|SECURITY_IDENTIFICATION, streams `Index::save`.
  - The old `--save-index` file mode is gone.
  - Handled the ERROR_NO_DATA race (a helper that writes and closes before ConnectNamedPipe), found by a flaky parallel test; 5/5 runs clean afterwards.
  - **Real elevated test** (user clicked UAC; debug build): C: 1,527,612 files via pipe in 4.06 s unoptimized, **0 `xylem-*.idx` files in TEMP**.
- **F-11:** wait in 200 ms slices; Cancel button in the Fast-scan dialog; 10-minute limit; TerminateProcess attempted.
- **F-15:** works across accounts with the pipe; documented as authorized by the admin's password.
- **F-13:** `visible()` also in the Deletion history. The log is JSON and the CSV quoted. OUT-02 (CSV formula injection) passes by design.
- **cargo-deny:** `deny.toml` (OSI licence allow-list, advisories, crates.io only) passes locally: advisories, bans, licenses and sources all ok. Added as a CI job.
- All findings F-01…F-16 are now fixed or decided (`docs/SECURITY-STATUS.md`). Remaining before 1.0: the VM clearance test run, 24 h cargo-fuzz, signing.

### Same day, later: Phase 6 memory (5.2 skipped)
- The user offered admin rights for 5.2 (USN catch-up). Not needed: every *user* would need admin for it, so it stays deferred.
- **Measured first.** New `Scanned::memory_report` (per-structure heap bytes; `Index::heap_columns`, `Children/Classification/CatStats::heap_bytes`) and an ignored test `memory_of_a_real_scan` (scans C:). Before: **204.3 MB for 1,909,532 nodes (112 B/node)**. The biggest items were `cats.dir_cats` 40 MB (a dense [u64; 10] per folder, Vec doubling) and `index.names` 60 MB reserved (doubling during the walk).
- **Fixes:**
  - `CatStats` keeps dense sums only while computing (exact capacity), then stores just the non-zero categories per folder in CSR form (`dc_start/dc_cat/dc_bytes`); `breakdown()` rebuilds the array.
  - `Index::shrink_to_fit`, called in `Scanned::new`.
- **After: 142.6 MB (78 B/node, −30%).** dir_cats 3.3 MB, names 40.5 MB.
- Speed: classify 0.15–0.20 s (unchanged), stats 0.10–0.12 s (was 0.12–0.15).
- Real app: **213 MB private after a fresh scan of C:** (was 278), **211 MB reopened** (was 248).
- 63 tests, clippy clean.

### 11 Nov 2026: Phase 6.1, peak memory while scanning
- Measured: CLI walk of C: peaks at ~160 MB. The app peaked at **375 MB** (settled at 212).
- The biggest temporary cost was the Phase 5 "remember" step:
  - `Snapshot::of` held an `Option<String>` per node (1.9M × 24 B) plus a second copy of every folder path.
  - `diff` made lowercase copies of ~300k paths for both snapshots and cloned every folder entry.
- **Fixes:**
  - `Snapshot::of` uses a u32 slot per node and stores each folder path once (exact capacity).
  - `diff` compares 64-bit FNV fingerprints of lowercased paths (`path_key`) instead of lowercase strings, borrows the snapshot strings, and clones only the ≤ 50 reported folders.
- **After:** app peak **282 MB** with remember on (was 375), 254 MB with it off; settled 212 MB. The rest of the peak is the scan plus view building (~40 MB above settled).
- 6.3 (first raw MFT read) needs admin tracing; left for the admin benchmark session.

### 11 Nov: Phase 7, largest files + file types
- Engine:
  - `Scanned::top_files(limit, query)`: one pass with a min-heap; case-insensitive name search.
  - `Scanned::file_types(limit)`: bytes and file count per lowercase extension.
  - `FileRow`/`TypeRow` types and a test.
- App: **View → Largest files & file types…** Two columns: the top 100 files with a search box (re-queries as you type) and coloured category dots; file types with size and count. Clicking a file jumps to it in the tree (switching to the location view if needed). Names go through `visible()`.
- Verified on C:\Windows (172k files): .msi installer caches 922 MB each, .dll 23.7 GB in 35,736 files.

### 11 Nov: Phase 7, CSV export in the app and an Explorer right-click entry
- The CSV writer moved from the CLI to `xylem_core::report::write_csv` (shared; skips items deleted through Xylem; the formula-injection note is documented). The CLI uses it; verified on C:\Windows\Fonts (535 rows).
- App: **File → Export to CSV** writes `Documents\Xylem\<root> <UTC time>.csv` on a worker thread (`Scanned::export_csv`), then shows it in Explorer.
- Installer: optional section **"Explorer right-click: Analyze with Xylem"**, writing HKLM `Software\Classes\{Directory, Directory\Background, Drive}\shell\Xylem` → `"xylem.exe" "%V"`. Removed on uninstall.
  - main.rs drops the stray `"` that `"C:\"` turns into under Windows' argument rules.
  - The CI install test now checks the entries are added and removed.
- 64 tests, clippy clean.

### 11 Nov: UI refinements from the user's feedback
- **Smoother scan start:** the progress modal repainted only every 120 ms (choppy slide). Now it repaints every frame while a job runs, and the pulse is eased back and forth (cosine) instead of a hard wrap. The modal has a fixed minimum size from the first frame. **Live count** for folder walks via a new `xylem_scan_walk::PROGRESS` atomic, updated once per folder (no measurable cost; C: scan 4.46 s), exposed as `session::scan_progress()`.
- **Drive strip** (new bottom panel above the status bar):
  - Shows "C:\ 1.54 TB used of 1.86 TB (83%) · 325 GB free" (`session::disk_space` via GetDiskFreeSpaceExW).
  - A bar with the 5 biggest top-level items (category colours, alternating shades), "Everything else", space the scan couldn't see (shown only if > 0.5%), and free space. Hover shows size and %; click jumps to the item.
  - For a folder scan it shows the folder's share of the drive. Refreshed after deletes.
- **Curved sunburst labels** (`map::curved_text`): each letter is placed and rotated along the ring; reads clockwise on the upper half and counter-clockwise on the lower half, so text is never upside down. Long names like "RESIDENT EVIL 4 BIOHAZARD RE4" now follow the curve.
- Map tooltip also shows the item's share of the whole scan.
- **Change column in the tree:**
  - `remember::previous_sizes(root)` loads the previous snapshot as `path_key → bytes` (~16 B per folder). It's loaded on the background remember/reopen threads.
  - Folder rows show `+25.0 MB` (amber) or `−30.0 MB` (green) when the change is ≥ 1 MB; a folder new since the last scan shows its whole size. The header reads "Change · Category".
  - Verified: games +25 MB, cache −30 MB, root −5 MB.
- **Start screen:** one card per drive (name, "324 GB free of 1.86 TB", used bar: accent / amber > 75% / red > 90%). Click to scan.
- **Ctrl+F** focuses the filter box (hint text says so).

### 11 Nov (night): Phase 7 finished, overnight fuzzing started
- **Coverage-guided fuzzing (cargo-fuzz + libFuzzer + AddressSanitizer):**
  - `fuzz/` crate with 6 targets: mft_record, run_list, boot_sector, mft_tree (many records → `build_tree`), index_file, snapshot_file. Entry points: `xylem_scan_mft::fuzzing` (doc-hidden).
  - Seeds come from `write_fuzz_seeds` (ignored test).
  - `tools/fuzz-overnight.ps1` finds the MSVC ASan runtime, runs all targets in parallel and writes `fuzz/logs/_status.txt`.
  - Started 01:06 for 10 h per target, ~1,500–44,000 runs/s each. No VM needed: fuzzing is non-destructive.
  - The laptop is Windows 11 Home, so there is no Hyper-V or Sandbox. Destructive VM tests are proposed for disposable GitHub runners.
- **Multi-select delete:**
  - Ctrl+click (or Space) marks rows; marked rows show an amber edge.
  - Delete, the context menu or the details-pane button "Move N marked…" opens one `ConfirmMany` dialog: every item is prepared and armed individually, refused ones are listed with reasons, items inside another marked folder are skipped, warnings are combined (ack required).
  - Recycles sequentially on a worker and applies each success.
- **Duplicate finder** (`session::dupes`, View → Find duplicate files…):
  - Opt-in, with an explanation (the only feature that reads contents).
  - Steps: same-size candidates from the scan (no cloud/link/deleted) → hardlinks collapsed by file ID → BLAKE3 of the first+last 64 KB → full BLAKE3. Online-only/offline/locked files are skipped.
  - Progress + Cancel. The kept copy is the "original-looking" one (`copy_score`). "Mark extra copies" feeds the multi-delete.
  - Verified end to end: 3 identical 12 MB files + a 1-byte-different decoy → 1 set found, 2 copies recycled, decoy and original kept, both logged.
- **Hardlink names:** `safety::hardlink_names` (FindFirstFileNameW, no admin, any scan mode). The details pane shows "· 3 names (hardlinks)" with the other names on hover and a note that deleting one frees nothing.
- New dependency: `blake3` (Apache-2.0); cargo-deny licences ok.
- **Full local test** (`tools/full-test.ps1`, report in `docs/TEST-REPORT.md`): **19/19 pass**.
  - Build, 67 tests in debug and release, clippy, cargo-deny, cargo-audit, no HTTP/TLS crates.
  - 2M-round built-in fuzz.
  - Speed: tree 0.157 s, classify 0.175 s, walk of C: 4.0–4.7 s.
  - Exact match vs std::fs.
  - Security: EL-01 (10 bad helper command lines), IN-06 (9 planted DLLs), BP-01 (planted explorer.exe), F-02, PV-01 (0 sockets), PA-06 (hostile settings), RO-01.
  - The two first-run failures were test bugs:
    - F-02 counted the user's installed Xylem; later the script's ⚡ was garbled (BOM-less script read as ANSI by PowerShell 5.1).
    - RO-01 compared folder timestamps, which Windows updates on any listing (proved with `dir`).
- New real-file test: decoys `report.`, `report `, `CON.txt`, `nul` (created via `\?\`) are refused; the real `report` is untouched.

### 11 Nov: the Xylem logo
- The user made the logo with Gemini: a leaf whose veins form growth rings (= the sunburst), one amber wedge (= a disk segment), and the stem as the xylem vessel.
- `tools/make-icons.py <master.png>` builds everything from it:
  - Square crop around the leaf, which also removes Gemini's sparkle watermark in the corner.
  - Standard padding (leaf at 86% of the tile) on the logo's own dark tile (#23241F), rounded corners, light sharpening ≤ 48 px.
  - Outputs: `docs/logo/xylem-logo-master.png` (1024) and `xylem-logo-512.png`; `crates/native/assets/icon.ico` (16/20/24/32/40/48/64/128/256), `icon-256.png`, `icon-64.rgba` (window icon), `logo-128.rgba` (in-app); `installer/welcome.bmp` (164×314 NSIS welcome/finish image).
- Used in: the exe icon (Explorer, taskbar, Start menu, right-click entry), the window title bar, the start screen (72 px), Help → About, the installer icon and welcome/finish pages, the README header, the GitHub release notes.
- Checked: the 16 px icon is still recognizable (green leaf + amber slice); the exe's embedded icon extracted via System.Drawing.

### 11 Nov: competitor benchmark
- Installed via winget (hash-verified): WinDirStat 2.9.3, TreeSize 9.9.1, SizeTrend 4.0 (Store). Built eDirStat 2.0.0 from crates.io source (nightly). WizTree 4.33 was already installed.
- `tools/bench-competitors.ps1` (user and admin modes): 1 warm-up + 3 runs, round-robin, wall-clock process time.
- **Results (medians):**
  - Admin scan: Xylem 1.07 s · eDirStat 2.41 · WizTree 4.42.
  - Admin scan + CSV: Xylem 1.59 · WinDirStat 8.09 · WizTree 16.57.
  - User scan: Xylem 4.29 · eDirStat 5.28 · WizTree 20.83.
  - User scan + CSV: Xylem 4.85 · WinDirStat 21.13 · WizTree 34.74.
- Completeness checked: item counts agree (eDirStat vs Xylem within a few dozen files; WinDirStat CSV 2.06M rows).
- **TreeSize's "3 s" was invalid:** `/NOGUI` produced no output and exited, and `/CSV` hung for 5 min even on a small folder (a hidden first-run/trial dialog?). Its rows were removed and it was excluded.
- SizeTrend: no CLI, not timed.
- Caveat: 6 fuzzer processes were running (6/24 threads).
- Written up in `docs/BENCHMARKS.md` and linked from the README and COMPETITORS.
