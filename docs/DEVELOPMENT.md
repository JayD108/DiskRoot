# Development

[← Back to README](../README.md)

## Build from source
Needs Windows 10/11, Rust (stable, MSVC toolchain) and the Visual Studio C++ build tools.

```powershell
git clone https://github.com/JayD108/Xylem.git
cd Xylem
cargo build --release --workspace
.\target\release\xylem.exe                         # the app
.\target\release\xylem-cli.exe scan C: --top 20    # command line (Fast scan mode when run as admin)
```

## Command line
`xylem-cli.exe scan <drive or folder>` with options:
- `--mode auto|mft|walk`
- `--by disk|size`
- `--top N`
- `--json`
- `--export all.csv`
- `--threads N`

## Tests
```powershell
cargo test --workspace                          # unit, integration and fuzz tests
cargo clippy --workspace --all-targets -- -D warnings
.\target\release\xylem-bench.exe verify "C:\Program Files"   # exact match against a plain std::fs walk
.\tools\admin-timing.ps1                        # Fast scan timing per phase (admin shell)
```

The tests cover:
- Torn, junk and hostile MFT records, run lists and boot sectors.
- Hardlinks, orphans and stale parents.
- Junction loops and Unicode names.
- Index save/load and corruption.
- Categories, protection and totals.
- The admin helper's path guard.
- The delete re-check against a real junction swap.
- OneDrive/Git warnings and signature checks.

`XYLEM_FUZZ_ROUNDS=2000000` runs the fuzz tests longer.

## Releases
- **Official releases:** push a tag matching `Cargo.toml`'s version (`git tag v0.1.0 && git push origin v0.1.0`). `.github/workflows/release.yml` then:
  - tests and builds;
  - signs through SignPath once the repository secret `SIGNPATH_API_TOKEN` and variable `SIGNPATH_ORGANIZATION_ID` exist;
  - builds the NSIS installer and portable zip;
  - runs a real install/uninstall test;
  - publishes a GitHub Release with checksums.
- **Local build:** `.\tools\release.ps1` (needs NSIS) creates the same files in `dist\`.

## Project layout
```
crates/
  core/        Index, store, view, classify (+ rules.json), report, format
  scan-walk/   folder-walk scanner
  scan-mft/    MFT reader: volume.rs, record.rs, scan.rs, fuzz.rs
  session/     engine API (lib.rs); elevated helper (helper.rs, elevate.rs, lockdown.rs);
               safety.rs, integrity.rs, recycle.rs; remember.rs (reopen, what changed);
               dupes.rs; watch.rs (live updates); tools.rs + power.rs (Windows' own
               tools); orphans.rs (leftover app-data hints)
  native/      xylem.exe: the app (egui) + helper mode
  cli/         xylem-cli.exe
  bench/       xylem-bench.exe: gen | run | time | verify | modes
fuzz/          coverage-guided fuzz targets (cargo-fuzz)
installer/     NSIS script (xylem.nsi) and its welcome image
.github/       CI and the release pipeline
tools/         release, full test, benchmark, fuzzing, icon and UI-test scripts
docs/          user and project documentation, logo, screenshots
```
