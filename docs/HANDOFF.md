# Handoff: start here next session

**Read this first, then the last section of `DEVLOG.md`. The plan is in `ROADMAP.md`.** Keep both up to date: after every milestone, add to `DEVLOG.md`, refresh this file, and commit.

## Where we are
- **Phases 1–4 are done.** Native app built, installer and security features added (2026-11-10). Renamed Drishti → Xylem.
- **The web UI was removed completely** (user's decision, 2026-11-10: "only the exe, so it's safer to present"). There is no Tauri/WebView2 anywhere now, and the dependency tree has no HTTP client.
- **Binaries:**
  - `xylem.exe`: the app. Also the elevated helper (`--helper-scan`).
  - `xylem-cli.exe`, `xylem-bench.exe`.
- **Engine API:** `crates/session`. Design reference: `xylem-ui-prototype/` (v2, HTML mock-up only).
- **Release:**
  - `tools/release.ps1` builds, optionally signs (`XYLEM_SIGN_*`), then makes the NSIS installer (`installer/xylem.nsi`, Program Files), the portable zip and `SHA256SUMS.txt` in `dist/`.
  - Current: setup 4.40 MB, xylem.exe 6.90 MB, **unsigned**.
- **Security:**
  - Delete re-check (type, links on the path).
  - OneDrive/Git/large warnings with a required tick.
  - Deletion log + history viewer.
  - Integrity panel (location + Authenticode), first-run notice, shield button.
  - Fuzz tests (they found 3 real bugs, all fixed).
- All 52 tests pass; clippy is clean.
- **Public repo:** https://github.com/JayD108/Xylem. CI is green, and the release pipeline dry run is green (NSIS installer 2.34 MB, real install/uninstall test in CI). SignPath prerequisites are in place: see `docs/SIGNPATH.md`.
- **v0.1.0 is published** (unsigned): https://github.com/JayD108/Xylem/releases/tag/v0.1.0. Old note, done: publish the first release. Run `git tag -a v0.1.0 -m "Xylem 0.1.0"` then `git push origin v0.1.0` (the user runs it or approves it). After that the user applies to SignPath.
- **Phase 5 (partly done):**
  - Done: instant reopen (C: 1.6M files in ~0.4 s), folder snapshots + "What changed" (View menu), and privacy controls (File → Remember scans / Forget remembered scans). Code: `crates/session/src/remember.rs`.
  - 5.2 (USN catch-up) deferred: it needs admin like Fast scan and saves only ~1 s. **Next: Phase 6 (memory).**
- **Security:** all findings F-01…F-16 fixed or decided (`docs/SECURITY-STATUS.md`); the admin helper writes nothing to disk (named pipe). Remaining before 1.0: the VM clearance test run, 24 h cargo-fuzz, signing.
- **Security retest (10 Nov):**
  - Score 8/10.
  - Fixed DLL planting and the Windows-subfolder lock bypass.
  - `cargo audit` is clean and runs in CI.
- **Open items:**
  - **Code signing needs the user's certificate.** Options: SignPath (free for OSS), Azure Trusted Signing, or a paid cert. Then set `XYLEM_SIGN_*` and run `tools/release.ps1`.
  - Scan-data RAM (~255–272 MB for C:).
  - First-raw-read cost.
  - Phase 5.

## Build and run
```powershell
cd <your clone of github.com/JayD108/Xylem>
$env:Path += ";$env:USERPROFILE\.cargo\bin"     # if cargo isn't found
cargo test --workspace
cargo clippy --workspace --all-targets
cargo build --release --workspace               # app ~2-3 min (fat LTO)
.\tools\release.ps1                               # dist\: setup.exe, portable zip, SHA256SUMS
.\target\release\xylem.exe            # the app
.\target\release\xylem-cli.exe scan C: --top 20   # admin shell => MFT mode
```

## Environment quirks
- **The dev shell is usually not elevated.** Admin tests run a script through `Start-Process powershell -Verb RunAs -Wait ...`, and the user clicks Yes on the UAC prompt. Batch several admin checks into one script (`tools/admin-bench.ps1`) to keep prompts to a minimum.
- **Smart App Control is ON.** Freshly built unsigned exes are sometimes blocked (os error 4551); retrying usually works.
- **Testing the UI without moving the mouse:** `tools/ui-screenshot.ps1` launches the app and captures only its window. `tools/ui-click.ps1` and `tools/ui-setpath.ps1` press buttons and type paths through UI Automation.
- **UI tests that open the app:** set `$env:APPDATA` / `$env:LOCALAPPDATA` to a temp folder first. That keeps the user's real settings and deletion log untouched. The first-run notice then shows; press "Got it" via `tools/ui-click.ps1`.


## Next steps
1. **Code signing via SignPath Foundation** (chosen 2026-11-10; repo public at github.com/JayD108/Xylem). After acceptance: add the `SIGNPATH_API_TOKEN` secret, fill in and uncomment the SignPath step in `.github/workflows/release.yml`, tag a release, approve the request, and re-test with Smart App Control on.
2. **Phase 5:**
   - Index cache plus USN journal (instant reopen).
   - Snapshots and diff ("what changed").
3. **Speed:** the first-raw-read "extents" cost (0.36–0.80 s).
4. **Later:**
   - Show the other names of hardlinked files.
   - Reduce MFT-mode RAM.
   - Explorer context menu.
   - Multi-select delete, in-app CSV export.
