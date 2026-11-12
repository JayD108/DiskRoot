# Contributing to Xylem

Thanks for helping. Xylem is a small, careful tool: it reads your drive fast, explains what's on it, and changes files only when you confirm. Every contribution keeps it that way.

## The rules every change keeps

1. **Offline.** No network code, no telemetry, no accounts, no update checks. CI fails if an HTTP/TLS crate appears in `Cargo.lock`.
2. **Read-only scanning.** A scan never writes to the scanned drive.
3. **Careful changes.** Anything that changes files happens only after the user confirms, is re-checked on disk first, and can be undone where possible. Data is verified, never assumed.
4. **Least privilege.** The window never runs as admin.
5. **Fast.** Scans never get slower. Every feature is measured before it's merged.
6. **Open source only.** MIT/Apache-2.0 for Xylem's code; dependencies must have an OSI-approved licence (`cargo deny check` enforces it).
7. **Honest claims.** Numbers in the docs come from measurements, with the method. "Fastest in our tests", never "fastest in the world".

## Call for compression research 🔬

Xylem compresses folders in place with **Windows' own file compression** (WOF: XPRESS4K/8K/16K and LZX), driven by its own parallel engine (`crates/session/src/compress.rs`). Files stay where they are and open as usual, and Uncompress undoes it. It's already much faster than `compact.exe`, but we want **more space saved, in less time, with the same safety**. If you know compression, we'd love your help.

What we've measured so far is in [docs/BENCHMARKS.md](docs/BENCHMARKS.md#compression) (per file type: savings and speed of every WOF algorithm, plus zstd and lz4 for comparison), and the full list of open problems is in [docs/HELP-WANTED.md](docs/HELP-WANTED.md). In short:

| Problem | Why it matters | Ideas welcome |
|---|---|---|
| **Better estimates before compressing** | Users decide from the estimate. Today it's a per-type table from measurements | Sampling 64 chunks predicts the real saving within 2.6 points in ~10 ms ([measured](docs/BENCHMARKS.md#predicting-the-saving-before-compressing)): wire it into the app |
| **Choosing the algorithm per file** | LZX saves more but is several times slower; on some data it's barely better than XPRESS16K | A "Smart" level: pick per file from a sample, so most of LZX's saving costs little more time than Fast |
| **Data that WOF can't shrink much** | Float arrays (`.npy`) ~15%, model weights (`.pt`) and columnar files (`.parquet`) 6–8%, HDF5 2% | Lossless transforms (byte-plane splitting for floats), or a format-aware archive mode |
| **A new transparent codec** | zstd saves more than any WOF algorithm on most data, but Windows only reads WOF formats transparently | Anything that stays transparent without a kernel driver, or a safe, verified archive-and-restore design |
| **Throughput** | Large folders take minutes | I/O ordering, thread counts per disk type (SSD/HDD), avoiding work on files that won't shrink |

### How to contribute research

1. **Open an issue** with the *Compression research* template: the idea, the expected gain and how you'll measure it.
2. **Measure on copies only.** The probe in [`tools/compress-probe`](tools/compress-probe) does this for you: `cargo run --release -- <scratch folder> <files...>` copies each file, measures every algorithm and the predictor, checks the data, and deletes the copy. Never run experiments on files you can't lose. Copy test data to a scratch folder, compress the copy, check the bytes read back are identical (hash before and after), undo, and record:
   - file type and size,
   - space saved (size on disk before and after, from `GetCompressedFileSizeW`),
   - time (single thread and with the engine's thread count),
   - that the data is identical after compressing and after undo.
3. **Send a pull request** with the code and the numbers. Keep new dependencies OSI-licensed and small, and explain any `unsafe`.

What we won't merge: anything that deletes or replaces originals without a verified copy, anything that needs network access, kernel drivers, or claims that weren't measured.

## Other ways to help

- **Test on your PC** (different disks, languages, file systems) and report what looked wrong.
- **Rules:** add known folders to `crates/core/src/rules.json` with a plain-English note and a safety label (see the `_doc` block at the top).
- **Docs and translations of notes.**

## Building and testing

```
cargo build --release -p xylem          # the app: target\release\xylem.exe
cargo test --workspace                  # unit, integration and built-in fuzz tests
cargo clippy --workspace --all-targets -- -D warnings
.\tools\full-test.ps1                   # everything, incl. security checks (15-25 min)
```

More in [docs/FEATURES-AND-DEVELOPMENT.md](docs/FEATURES-AND-DEVELOPMENT.md#9-build-from-source).

## Pull requests

- One topic per pull request, with tests for new behaviour.
- `cargo test`, `cargo clippy -- -D warnings` and `cargo deny check` pass.
- User-visible changes update the README feature list and `docs/FEATURES-AND-DEVELOPMENT.md`.
- Security problems: please report privately ([SECURITY.md](SECURITY.md)), not in an issue.

By contributing you agree your contribution is licensed under MIT OR Apache-2.0, like the rest of Xylem.
