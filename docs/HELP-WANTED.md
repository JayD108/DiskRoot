# Help wanted: what would make Xylem better

[← Back to README](../README.md) · How to contribute: [CONTRIBUTING.md](../CONTRIBUTING.md)

This is the honest list of what Xylem needs next: open problems, known gaps and measured weak spots. Each item says why it matters, where the code is, and how to show it works. Pick one, open an issue saying you're on it, and send a pull request.

Every change keeps Xylem's rules: **offline, read-only scanning, nothing changes without the user's OK, never admin in the window, fast, OSI-licensed, honest numbers.**

---

## 1. Compression engine (biggest opportunity)

Xylem compresses folders in place with Windows' own file compression (WOF), driven by its own parallel engine ([`crates/session/src/compress.rs`](../crates/session/src/compress.rs)). Files stay usable and Uncompress undoes it. Here's what it achieves today on real data, measured on copies (E10, single thread, one file per type):

| File type | Fast (XPRESS16K) | Maximum (LZX) | zstd 3 (not transparent) |
|---|---|---|---|
| Text: `.jsonl`, `.csv` | 51–55% | 54–66% | 53–71% |
| Programs: `.dll` | 60% | 72% | 76% |
| Python pickles `.pkl` | 62% | 69% | 70% |
| EEG `.edf` | 33% | 38–42% | 38–43% |
| Token ids `.bin` | 26% | 36% | 36% |
| Float arrays `.npy` | 15% | 17% | 18% |
| `.parquet`, `.pt`, `.safetensors` | 6–8% | 7–8% | 7–9% |
| Satellite `.h5` | 2% | 7% | 9% (17% at zstd 9) |

Full table with speeds: [BENCHMARKS.md → Compression](BENCHMARKS.md#compression).

**What this tells us:**
- **Text, logs, CSV, code and programs** compress well already.
- **Scientific data** saves 2–42%: EEG a third, float arrays ~15%, HDF5 almost nothing with Fast; zstd finds a few points more.
- **Model weights** (`.pt`, `.safetensors`) and **columnar files** (`.parquet`) save under 10% with any codec. They are close to random already.
- **LZX vs XPRESS16K:** LZX adds 7–12 points on text, code and programs, but only 0.5–2 on data files, at 5–15× the time (one thread).

### Open problems

| # | Problem | Why | Where | Done when |
|---|---|---|---|---|
| C1 | **Sampled estimates.** Estimates come from a per-extension table. Sampling a few 16 KB chunks per file with XPRESS-Huffman (`RtlCompressBuffer`) predicts the real Fast saving within 2.6 points in ~10 ms per file (64 chunks; [measured](BENCHMARKS.md#predicting-the-saving-before-compressing)). **Wanted:** wire it into the Compress dialog and Save space (sample the biggest files, use the table for the rest) | Users decide from the estimate; a wrong one wastes their time | `compress.rs` (`ratio`, `verdict`), `savings.rs` | Estimates within ±3 points on the E10 set; under 50 ms per GB on SSD |
| C2 | **"Smart" level: pick XPRESS16K or LZX per file.** | LZX is worth it on text (+10 points), not on data (+0.5) | `compress.rs` `run` | Saves ≥ 90% of LZX's bytes in ≤ 2× Fast's time on a mixed folder |
| C3 | **Skip files that won't shrink, cheaply.** Today: extension list + entropy of the first 64 KB. Windows still spends time on files that end up saving < 2% | Throughput on big data folders | `compress.rs` | Fewer wasted bytes processed, no files wrongly skipped that would save ≥ 5% |
| C4 | **Float arrays and EEG.** Byte-plane shuffling (like Blosc) often doubles what codecs find in float data | `.npy`/`.edf`/`.h5` are big on research PCs | New: an opt-in archive mode (C6) | A measured, lossless gain on the E10 files |
| C5 | **I/O tuning per disk.** Thread count is cores − 2 for every disk | HDDs suffer from many parallel readers | `compress.rs` `default_threads` | Measured on an HDD and an SSD |
| C6 | **A verified archive mode for cold data** (zstd/7z-style, not transparent): pack → verify every byte → only then replace, with one-click restore | zstd 3 saves 5–15 points more than WOF on several types | Design first (see "Archive & replace" in [ROADMAP.md](ROADMAP.md)) | A design reviewed for data safety before any code |
| C7 | **A transparent codec beyond WOF** without a kernel driver | Windows reads only WOF formats transparently | Research | A proof of concept, or a clear "not possible" write-up |

How to measure: [CONTRIBUTING.md → How to contribute research](../CONTRIBUTING.md#how-to-contribute-research). The R&D probe used for the table is described in [BENCHMARKS.md](BENCHMARKS.md#compression).

---

## 2. Scanning and accuracy

| # | What | Why | Where |
|---|---|---|---|
| S1 | **Count hardlinks once in the folder walk.** The walk counts every name; on C:\Windows that's 46 GB vs 33 GB real (Fast scan is right) | Wrong totals without admin | `crates/scan-walk` |
| S2 | **Sandboxed reader for Fast scan.** After lockdown the admin helper could still *read* other drives (finding F-17) | Defence in depth if a parser bug is ever found | `crates/session/src/lockdown.rs`, `helper.rs` |
| S3 | **Test on more file systems and disks:** exFAT, FAT32, ReFS, HDDs, network drives, non-English names | Only NTFS SSDs are tested well | Reports welcome, no code needed |
| S4 | **A friendlier label for a drive root** in the details pane | "C:\" shows as Other › Unclassified | `app.rs` details pane |

## 3. Safety and security

| # | What | Why |
|---|---|---|
| X1 | **Destructive tests on throwaway machines** (crafted file systems, junction swaps during delete/compress) | They can't run on a personal PC; a VM or CI runner can |
| X2 | **24-hour fuzzing** of every parser (`fuzz/`, cargo-fuzz) | So far: 2 million rounds per parser per release, and overnight runs |
| X3 | **Code signing** (SignPath Foundation) | Unsigned releases trigger SmartScreen |

## 4. App and accessibility

| # | What | Why | Where |
|---|---|---|---|
| U1 | **Screen-reader access to the map** and to menu items | The sunburst/treemap is mouse-only; menu popups aren't exposed to UI Automation | `crates/native` (egui + AccessKit) |
| U2 | **Run Ways to save space off the UI thread** on huge drives | ~0.5 s on 1.7M files today; would grow on 10M | `app.rs` `open_savings` |
| U3 | **Translations** of the UI and the rule notes | English only | `rules.json`, `app.rs` |

## 5. Measurements anyone can help with

- **Benchmarks on other PCs** (idle machine): `tools/bench-competitors.ps1`, results into [BENCHMARKS.md](BENCHMARKS.md).
- **Compression numbers for more file types** (games, Office files, photos in RAW, VM images, source trees): run the probe on copies and send the table.

## 6. After 1.0 (needs a careful design first)

- **Archive & replace** (pack cold folders, verify, replace, restore).
- **Space Shifter** (move a folder to another drive and leave a junction), only with copy → verify → swap.

---

Found something not on this list? Open an issue. For security problems, report privately ([SECURITY.md](../SECURITY.md)).
