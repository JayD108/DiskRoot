# Handoff: start here next session

**Read this first, then the last section of `DEVLOG.md`.** Keep both up to date: after every milestone, add to `DEVLOG.md`, refresh this file, and commit.

## Where we are
- Phases 1–3 are done and Phase 4 is in progress. See "Next steps" below.
- **Working:**
  - The CLI (`drishti`): folder walk plus the MFT reader, CSV/JSON export, `--save-index`.
  - The bench harness (`drishti-bench`).
  - The Tauri app (`drishti-app`): sunburst, treemap, list, and the admin fast scan through the helper.
- Every test passes and `cargo clippy --workspace --all-targets` is clean.

## Build and run
```powershell
cd "C:\Users\Mrittunjay Dubey\Storage\drishti"
$env:Path += ";$env:USERPROFILE\.cargo\bin"     # if cargo isn't found
cargo test --workspace
cargo clippy --workspace --all-targets
cargo build --release --workspace               # app ~1.5 min, first time ~3 min
.\target\release\drishti-app.exe
.\target\release\drishti.exe scan C: --top 20   # admin shell => MFT mode
```

## Environment quirks
- **Claude's shell is not elevated.** Admin tests run a script through `Start-Process powershell -Verb RunAs -Wait ...`, and the user clicks Yes on the UAC prompt. Batch several admin checks into one script (`tools/admin-bench.ps1`) to keep prompts to a minimum.
- **Smart App Control is ON.** Freshly built unsigned exes are sometimes blocked (os error 4551); retrying usually works.
- **Testing the UI without moving the mouse:** `tools/ui-screenshot.ps1` launches the app and captures only its window. `tools/ui-click.ps1` and `tools/ui-setpath.ps1` press buttons and type paths through UI Automation.
- The home folder `C:\Users\Mrittunjay Dubey` is itself a git repo. This project has its **own** repo in `drishti/`.

## User preferences
- Wants short, plain answers. Is a CS engineer who is newer to Windows and Android internals.
- Wants everything recorded (`DEVLOG.md`, `HANDOFF.md`, git commits) so no context is lost.
- Never ship anything that depends on reverse-engineering WizTree.

## Next steps
1. **Phase 4 (MVP finish):** C1 classification, C2 "by category" view, D3 safe delete to the Recycle Bin with protected paths locked.
2. **Phase 5:**
   - Index cache plus USN journal (instant reopen).
   - Snapshots and diff ("what changed").
   - Safety badges and notes (D1/D2).
3. **Later:**
   - Show the other names of hardlinked files.
   - Reduce MFT-mode RAM (290 MB).
   - Installer into Program Files, plus code signing.
   - Explorer context menu.
