## What this changes

<!-- One or two sentences: what it does for someone using DiskRoot. Link the issue if there is one ("Fixes #123"). -->

## How you checked it

<!-- What you ran, what you saw. For anything that touches speed, memory or SSD writes: the numbers before and after, and how you measured them (see docs/BENCHMARKS.md#method). -->

## Screenshots (UI changes)

<!-- Before / after. Please scan C:\Windows (or a test folder), not your own files, so no personal names end up in the PR. -->

## Checklist

- [ ] `cargo test --workspace` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean
- [ ] Only the lines this change needs are touched (please don't reformat unrelated code)
- [ ] Still **offline**: no network code, telemetry or update checks
- [ ] Scanning still **only reads**; anything that changes files asks first and is re-checked on disk
- [ ] No admin needed for anything new (raw drive access stays in the locked-down helper)
- [ ] Claims in docs or UI text are measured, with the method; nothing like "fastest in the world"
- [ ] New dependencies (if any) have an OSI-approved licence and pass `cargo deny check`
- [ ] Docs updated where behaviour changed (README, docs/FEATURES-AND-DEVELOPMENT.md, docs/CLI.md)

By contributing you agree that your work is licensed under MIT OR Apache-2.0, like the rest of DiskRoot, and to follow the [Code of Conduct](../CODE_OF_CONDUCT.md).
