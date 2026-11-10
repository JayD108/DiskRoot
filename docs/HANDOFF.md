# Handoff: start here next session

**Read this first, then the last section of `DEVLOG.md`.** Keep both up to date: after every milestone, add to `DEVLOG.md`, refresh this file, and commit.

## Where we are
- **Phases 1–4 are done.** The project was **renamed Drishti → Xylem** on 2026-11-10 (folder `Storage/xylem`).
- **Working:**
  - The CLI (`xylem`): folder walk plus the MFT reader, CSV/JSON export, `--save-index`.
  - The bench harness (`xylem-bench`).
  - The Tauri app (`xylem-app`): sunburst, treemap, list, admin fast scan through the helper, By location / By category, notes and safety badges, Recycle-Bin delete.
- `xylem-ui-prototype/` holds the proposed new UI design (static demo, open `index.html`). It is **not** wired into the app yet.
- Every test passes and `cargo clippy --workspace --all-targets` is clean.
- **Waiting on the user** (details at the end of DEVLOG session 2):
  1. **Security.md:** consent screen and shield. Discuss before implementing, and keep the copy honest (WebView2 does contact Microsoft).
  2. **UI plan in `docs/UI-ARCHITECTURE.md`:** a native egui UI by default (spike: 4.46 MB, ~59 MB, 0 network) plus an optional Tauri web UI add-on, both on a new `xylem-session` crate.
  3. **Approve prototype v2** (`xylem-ui-prototype/`, "classic utility, modern finish"), then build the native UI to match it.

## Build and run
```powershell
cd "C:\Users\Mrittunjay Dubey\Storage\xylem"
$env:Path += ";$env:USERPROFILE\.cargo\bin"     # if cargo isn't found
cargo test --workspace
cargo clippy --workspace --all-targets
cargo build --release --workspace               # app ~1.5 min, first time ~3 min
.\target\release\xylem-app.exe
.\target\release\xylem.exe scan C: --top 20   # admin shell => MFT mode
```

## Environment quirks
- **Claude's shell is not elevated.** Admin tests run a script through `Start-Process powershell -Verb RunAs -Wait ...`, and the user clicks Yes on the UAC prompt. Batch several admin checks into one script (`tools/admin-bench.ps1`) to keep prompts to a minimum.
- **Smart App Control is ON.** Freshly built unsigned exes are sometimes blocked (os error 4551); retrying usually works.
- **Testing the UI without moving the mouse:** `tools/ui-screenshot.ps1` launches the app and captures only its window. `tools/ui-click.ps1` and `tools/ui-setpath.ps1` press buttons and type paths through UI Automation.
- The home folder `C:\Users\Mrittunjay Dubey` is itself a git repo. This project has its **own** repo in `xylem/`.

## User preferences
- Wants short, plain answers. Is a CS engineer who is newer to Windows and Android internals.
- Wants everything recorded (`DEVLOG.md`, `HANDOFF.md`, git commits) so no context is lost.
- Never ship anything that depends on reverse-engineering WizTree.

## Next steps
1. **After the user's decisions above:** port the prototype design and add the consent screen and shield.
2. **Phase 5:**
   - Index cache plus USN journal (instant reopen).
   - Snapshots and diff ("what changed").
3. **Speed:** investigate the first-raw-read "extents" cost (0.36–0.80 s, likely the NTFS metadata flush).
4. **Later:**
   - Show the other names of hardlinked files.
   - Reduce MFT-mode RAM (now ~255 MB).
   - Installer into Program Files, plus code signing.
   - Explorer context menu.
