# Handoff: start here next session

**Read this file first, then the last sections of `DEVLOG.md` (full history, newest at the bottom).** The plan is in `ROADMAP.md` and `FEATURE-PROPOSALS.md`. After every milestone: add to `DEVLOG.md`, refresh this file, commit, push.

_Last updated: 11 Nov 2026, ~03:45, after commit 3b5fcd6._

## Where we are (one screen)
- **Product:** Xylem, a native Windows disk space analyzer (Rust + egui). One `xylem.exe` (the app, which is also its own elevated Fast-scan helper), plus `xylem-cli.exe` and `xylem-bench.exe`. Open source (MIT OR Apache-2.0), offline, no telemetry.
- **Repo:** https://github.com/JayD108/Xylem (public; recreated by the owner on 10 Nov). CI (tests, cargo-audit, cargo-deny) is green; the release pipeline is green.
- **Released:** v0.1.0 (unsigned) on GitHub Releases. The current `main` is far ahead of it: next release **v0.2.0**.
- **Phases done:** 1–4 (scanners, MFT reader, app, categories, safe delete) · 5 (instant reopen, snapshots, "what changed"; 5.2 USN catch-up deliberately skipped) · 6 (memory: scan data 204 → 143 MB, app peak 375 → 282 MB) · 7 (largest files + search, file types, CSV export, Explorer right-click, multi-select delete, duplicate finder, hardlink names) · the UI round (drive strip with top 5, curved sunburst labels, Change column, drive cards, smooth progress, Ctrl+F) · the logo everywhere.
- **Quality:** 67 tests · clippy `-D warnings` clean · `tools/full-test.ps1` **19/19** (`docs/TEST-REPORT.md`) · all security findings F-01…F-16 fixed or decided (`docs/SECURITY-STATUS.md`).
- **Benchmarks** (`docs/BENCHMARKS.md`): Xylem is fastest in every test. With admin: scan 1.07 s (eDirStat 2.41, WizTree 4.42); scan + CSV 1.59 s (WinDirStat 8.09, WizTree 16.57). Without admin: 4.29 s / 4.85 s. TreeSize and SizeTrend couldn't be timed (TreeSize's CLI didn't scan; SizeTrend has no CLI).
- **Competitors** (`docs/COMPETITORS.md`): eDirStat (closest technically) and SizeTrend (closest in purpose). Fast MFT scanning is now common. Honest positioning: "the open-source disk analyzer that tells you what's safe to delete and what changed, at full speed, offline".

## Open threads (pick up here)
1. **Overnight fuzzing**: started 11 Nov 01:06, 10 h per target, ends ~11:07. Check `fuzz\logs\_status.txt` and `fuzz\artifacts\` (gitignored). At 03:37: ~1.24 billion runs, **0 crashes**. If anything crashed: reproduce with `cargo +nightly fuzz run <target> fuzz/artifacts/<target>/<file>`, fix, add a regression test.
2. **Next work: Phase 8, "Clean up with confidence"** (`docs/FEATURE-PROPOSALS.md`): dependency purge ("Mark all" + Rust `target` detection via `CACHEDIR.TAG`), leftovers view, more cache rules, custom glob rules (can't unlock protection), DISM / System Protection buttons via Windows' own tools. Waiting for the owner's go-ahead; recommended first item: dependency purge.
3. **Re-run benchmarks on an idle PC** after fuzzing ends (`tools/bench-competitors.ps1 -Mode user`, then `-Mode admin` elevated). Add TreeSize if the owner opens it once to clear its first-run/trial dialog.
4. **VM-style destructive tests** (other users, crafted VHDX images, junction races) from the owner's test plan (`Storage/security test.md`). The laptop is Windows 11 Home (no Hyper-V/Sandbox), so the plan is a workflow on disposable GitHub Windows runners.
5. **Admin session with the owner** (a couple of UAC clicks): Fast-scan memory peak, first-raw-read timing (Phase 6.3).
6. **Release v0.2.0** (tag `v0.2.0` after bumping `version` in `Cargo.toml`; the tag must match), then **apply to SignPath** (`docs/SIGNPATH.md`; enable the secret/variable after acceptance).
7. **Dependabot PRs** for major action bumps (checkout 7, upload-artifact 7, signpath action 3) are open: **don't merge** without checking (the signpath v3 inputs may differ).

## Build, test, run
```powershell
cd <your clone of github.com/JayD108/Xylem>
$env:Path += ";$env:USERPROFILE\.cargo\bin"
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --workspace                 # ~2-3 min (fat LTO)
.\target\release\xylem.exe                        # the app
.\tools\full-test.ps1                             # full local test (~20 min) -> test-evidence\REPORT.md
.\tools\bench-competitors.ps1 -Mode user          # benchmark (admin mode from an elevated shell)
.\tools\fuzz-overnight.ps1 -Hours 10              # coverage-guided fuzzing (nightly + cargo-fuzz + MSVC ASan)
.\tools\release.ps1                               # local installer build (needs NSIS); CI does this on tags
python tools\make-icons.py <logo.png>             # regenerate every icon/logo file
```

## Where things are
- `crates/core`: index, store (index file format), classify + `rules.json`, view, report (CSV).
- `crates/scan-mft`: MFT reader (`record.rs`, `volume.rs`, `scan.rs`, `fuzz.rs`, `fuzzing` entry points).
- `crates/scan-walk`: folder walker (NtQueryDirectoryFile, live `PROGRESS` counter).
- `crates/session`: engine API (`lib.rs`), elevated helper (`helper.rs` + `elevate.rs`, named pipe), `safety.rs` (re-check, identity chain, hard locks, warnings, deletion log, hardlink names, `visible()`), `integrity.rs` (signature, Program Files gate, DLL hardening), `remember.rs` (cache, snapshots, diff), `dupes.rs`, `recycle.rs`.
- `crates/native`: the app (`app.rs` UI, `map.rs` sunburst/treemap incl. curved labels, `theme.rs`, `settings.rs`), `assets/` icons.
- `fuzz/`: cargo-fuzz targets (6). `installer/xylem.nsi` + `welcome.bmp`. `.github/workflows/` (ci.yml, release.yml), `release-notes.md`, `dependabot.yml`. `deny.toml`.
- Docs: README (short; no comparisons there), HOW-IT-WORKS, SECURITY-AND-PRIVACY, SECURITY-STATUS, DEVELOPMENT, ROADMAP, FEATURE-PROPOSALS, BENCHMARKS, COMPETITORS, TEST-REPORT, SIGNPATH, DEVLOG, `logo/`.

## Environment quirks
- **The dev shell isn't elevated.** Admin runs go through `Start-Process powershell -Verb RunAs -Wait ...`; the owner clicks Yes. Batch admin checks into one script.
- **Smart App Control is on;** unsigned fresh builds are occasionally blocked; retrying usually works.
- **UI testing without touching the mouse:** `tools/ui-screenshot.ps1` (PrintWindow: only the app's own window, never screen-region capture), `tools/ui-click.ps1`, `tools/ui-setpath.ps1`. Point `APPDATA`/`LOCALAPPDATA` at a temp folder so the owner's settings, deletion log and remembered scans stay untouched. Screenshots meant for the public repo should show `C:\Windows`, not personal folders.
- **Windows PowerShell 5.1:** save scripts with non-ASCII text (e.g. ⚡) **with a BOM**; native tools' stderr + `Stop` makes false errors (check exit codes instead).
- The owner may have the **installed Xylem open** (`C:\Program Files\Xylem`). Don't close it, and don't count it in tests.
- Background jobs/fuzzers: check `fuzz\logs` before running speed benchmarks (they use CPU).

## Release and signing
- CI release pipeline: tag → test → build → (SignPath signs the exes) → NSIS installer → real install/uninstall test → (SignPath signs the installer) → checksums → GitHub Release. SignPath steps switch on when `SIGNPATH_API_TOKEN` + `SIGNPATH_ORGANIZATION_ID` exist.
- Signing is deferred by the owner until all phases are done.
