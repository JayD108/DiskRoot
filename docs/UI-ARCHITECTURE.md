# UI architecture: native by default, web UI as an optional add-on

> **Update 10 Nov 2026:** the web UI (Tauri/WebView2) was removed. Xylem now ships only the native `xylem.exe`. Sections about the web UI are kept as history.

**Status:** approved and built (2026-11-10). `crates/session` + `crates/native` (`xylem.exe`) + `crates/app` (`xylem-web.exe`). Deviation: the CLI stays a separate `xylem-cli.exe`, because one Windows exe cannot be both a clean console tool and a window app.

## Goals
1. **Light on the machine.** A disk tool shouldn't need a browser engine.
2. **Truly offline.** Zero network connections, measurable.
3. **Looks like a tool, not a web page.** Classic utility structure with a modern finish (see `xylem-ui-prototype/`).
4. **Two interfaces, one engine.** Native UI by default; the web UI is optional for people who want it.

## Measurements (2026-11-10, same laptop)

| | Tauri app (WebView2) | Native spike (egui + OpenGL) |
|---|---|---|
| Executable | 8.6 MB + WebView2 runtime | **4.46 MB**, self-contained |
| Processes | 7 (app + 6 × msedgewebview2) | **1** |
| Private memory, small scan | ~258 MB (WebView2 ≈ 248 MB) | **~59 MB** |
| Network connections | 2 HTTPS to Microsoft (WebView2's own services) | **0** |
| Idle CPU (5 s) | n/a | 47 ms (~1% of one core) |

Spike: `spikes/native-egui` (eframe 0.37, `glow` backend, LTO + strip). It draws the sunburst as triangle-strip meshes, plus a menu bar, list and status bar.

## Shape

```
                 ┌──────────────────────────────────────────────┐
                 │ Engine crates (unchanged)                     │
                 │  core · scan-walk · scan-mft · classify/rules │
                 └───────────────────┬───────────────────────────┘
                                     │
                 ┌───────────────────▼───────────────────────────┐
                 │ xylem-session (new, UI-agnostic)               │
                 │  scan / scan_admin / view / list / crumbs /    │
                 │  category_view / info / delete / reveal        │
                 │  (moved out of crates/app/src/main.rs)         │
                 └──────────┬─────────────────────────┬──────────┘
                            │                         │
          ┌─────────────────▼──────────┐   ┌──────────▼─────────────────┐
          │ xylem.exe (DEFAULT)         │   │ xylem-web.exe (OPTIONAL)    │
          │ native UI (egui)            │   │ Tauri + WebView2            │
          │ + CLI subcommands           │   │ same design, HTML/CSS       │
          │ + the elevated scan helper  │   │ separate optional download  │
          └─────────────────────────────┘   └─────────────────────────────┘
```

- **One small exe does everything by default.**
  - With no arguments, `xylem.exe` opens the native window.
  - `xylem scan …` is the CLI.
  - The elevated helper is the same exe run as `xylem scan C: --mode mft --save-index …`.
  - This removes `xylem-app.exe` and the separate helper from the default install.
- **The web UI is an add-on.** `xylem-web.exe` is a separate, optional component ("Install the web interface: needs Microsoft WebView2"). It links the same `xylem-session`, so features and behavior are identical. It's useful for people who prefer it, and it's the place to try new visual ideas quickly.
- **Choosing.** *View → Interface → Native / Web* (shown only if the add-on is installed) saves the choice. `xylem.exe` then starts `xylem-web.exe` if the web UI was picked.

## One design system for both

The prototype's tokens (palette, type sizes, spacing, radii) live in one place, `docs/design-tokens.json`:
- They're turned into CSS variables for the web UI.
- They're turned into `egui::Visuals` and `Style` for the native UI: same colors, 1px hairlines, 4px radii, 24px rows, tabular numbers.
- The layout is the same in both: menu bar, toolbar, tree-table | map, details pane, status bar.

## Size reductions, in order of payoff
1. **Drop WebView2 from the default path:** −~200 MB RAM, −6 processes, zero network.
2. **Merge the app, CLI and helper into one exe:** one ~5 MB download instead of 8.6 + 0.9 MB.
3. **Release profile:** `lto = true`, `codegen-units = 1`, `panic = "abort"`, `strip = true` (the spike uses these).
4. **Lean features:** egui's `glow` backend rather than `wgpu` (smaller); only the fonts we use (Segoe UI from Windows, loaded at runtime, not bundled).
5. **Engine memory (separate work):** stream parsed MFT chunks into the index to cut the ~255 MB scan peak.

## Effort (estimate)
1. Extract `xylem-session` from the Tauri backend: small, mostly moving code.
2. Native UI to parity with the prototype (tree-table, map, details, menus, dialogs, delete flow): the main work, a few sessions.
3. Merge CLI, helper and UI into `xylem.exe`; keep `xylem-web.exe` building against the session crate.
4. Shared tokens, light/dark themes, keyboard navigation, accessibility (egui exposes AccessKit).
