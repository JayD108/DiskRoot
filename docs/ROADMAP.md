# Xylem roadmap

**Written:** 10 Nov 2026, after commit `556491e`. Phases 1–4, the native app, the installer and the security features are done (see `DEVLOG.md`).

**Rule for every step:** never slower than today. The full C: scan must stay at ~1.2–1.5 s (MFT), build_tree at ≤ 0.21 s and classify at ≤ 0.17 s. Each step ends with tests, a DEVLOG/HANDOFF update and a commit.

---

## Track R: release readiness (runs alongside the phases; signing needs the user)

| Step | What | Done when |
|---|---|---|
| R1 | **Pick a signing route** (user decision; see "Code signing" below) | Route chosen and an account or application started |
| R2 | **Public GitHub repo + CI**: a GitHub Actions workflow that runs `cargo test`, `clippy` and `tools/release.ps1` on `windows-latest` and uploads `dist/` | Green CI on every push; release artifacts built by CI (SignPath requires this) |
| R3 | **Sign releases**: connect the chosen signer to CI or `release.ps1` | `signtool verify /pa` passes; the in-app shield shows "Signed"; Smart App Control lets the installer run |
| R4 | **Presentation polish**: screenshots in `docs/screenshots/`, a short demo GIF, a `SECURITY.md` (how to report issues), and a code-signing policy page if SignPath is used | README shows the real app |
| R5 | **Wider testing**: a second PC, an HDD, a non-English user name, a very full drive, and an exFAT/FAT32 USB stick (the folder-walk path) | Results recorded in the DEVLOG; bugs fixed |

## Phase 5: instant reopen + "what changed" (the biggest visible win)

**Status (10 Nov 2026):** 5.1, 5.3, 5.4 and 5.5 are done. **5.2 (USN catch-up) is deferred:** reading the USN journal needs admin rights, exactly like Fast scan. So a catch-up would still need the UAC prompt and would save only about 1 s over a full Fast scan (~1.5 s), and without admin it can't run at all. Revisit only if scans of much larger drives make Fast scan slow.

| Step | What | Detail |
|---|---|---|
| 5.1 | **Index cache** | After a scan, save the index (existing `XYLMIDX1` format, already length-checked and fuzzed) to `%LOCALAPPDATA%\Xylem\cache\`, keyed by volume serial number. On start, show the cached tree instantly, marked "from <time>". |
| 5.2 | **USN journal catch-up** | Store the volume's USN journal ID and position with the cache. On reopen, read only the changes since then (`FSCTL_READ_USN_JOURNAL`, admin) and patch the tree: create/delete/rename/size change. If the journal wrapped or its ID changed, rescan fully. The target is under 1 s to be current. |
| 5.3 | **Snapshots** | Keep the last N scans per drive (small: sizes per folder, not every file). |
| 5.4 | **"What changed" view** | A diff between two snapshots: which folders grew or shrank, by how much, and new big files. Shown as a list plus colour on the map (grew = warm, shrank = cool). |
| 5.5 | **Cache privacy options** | Settings: keep the cache (default on), or never store a cache. Plus "Clear cache now". The cache holds file names, so the privacy panel must say so. |

Risks: the USN journal needs admin (reuse the helper, which then also returns changes), and journal records lack sizes, so changed files are re-measured individually.

## Phase 6: memory and first-read speed

**Status (10 Nov 2026):** scan data for C: (1.9M items) is down from 204 MB to **143 MB** (6.2 done: sparse per-folder category sums, trimmed index columns). The whole app uses 213 MB after a fresh scan (was 278) and 211 MB reopened (was 248). The rest is the window: GPU driver, fonts, UI. Next: 6.1 (peak while scanning) and 6.3.

| Step | What | Target |
|---|---|---|
| 6.1 | Stream MFT chunks into the index instead of holding all parsed records | Peak RAM for C: under 150 MB (now ~255–272 MB) |
| 6.2 | Shrink the classification arrays (u8/u16 packing, drop per-file arrays not needed after stats) | |
| 6.3 | Investigate the first-raw-read cost (0.36–0.8 s, likely an NTFS metadata flush) | Measure first; keep only if it's faster |

## Phase 7: breadth (features WizTree users expect)

1. **Multi-select delete** (Ctrl/Shift-click), with one combined confirmation and the same warnings.
2. **File-type view**: bytes by extension, with drill-down to the files.
3. **Top files and search**: the largest files on the drive; search by name.
4. **Duplicate finder**: size first, then partial hash, then full hash. Read-only, opt-in; it reads file contents, so the privacy panel must say so.
5. **CSV export from the app** (the CLI already has it).
6. **Explorer context menu**: "Analyze with Xylem", installed by the installer and removed on uninstall.
7. **Hardlinks**: list all names of a hardlinked file.

## Then: 1.0 public release
Signed installer, CI builds, screenshots, testing on several PCs, and an honest README and status report.

---

## Code signing: where to read and which route fits

**Why it matters:** unsigned exes trigger SmartScreen warnings and can be blocked by Smart App Control. The in-app shield also shows "unsigned". `tools/release.ps1` already signs automatically once one of the `XYLEM_SIGN_*` variables is set.

| Route | Cost | Who can use it | Read |
|---|---|---|---|
| **SignPath Foundation** (free for open source) | Free | OSS projects: OSI license, public repo, MFA for the team, builds from CI. The certificate is issued to "SignPath Foundation", not to you. A community guide says dual licensing isn't allowed (Xylem is MIT OR Apache), so check the current terms first | https://signpath.org/ · terms: https://signpath.org/terms · product page: https://about.signpath.io/product/open-source |
| **Azure Artifact Signing** (formerly Trusted Signing) | $9.99/month (5,000 signatures) | Organizations in the US, Canada, EU and UK; **individuals only in the US or Canada** | Quickstart: https://learn.microsoft.com/en-us/azure/artifact-signing/quickstart |
| **OV certificate from a CA** (DigiCert, Sectigo, GlobalSign, or resellers) | Roughly a few hundred USD/year | Anyone with ID verification; the key must sit on a hardware token or cloud HSM | Microsoft's overview of all options: https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/code-signing-options |

**Recommendation:** start with **SignPath Foundation** if Xylem goes public as open source (it needs R2, CI). Use Azure Artifact Signing if you're eligible by country. Use an OV certificate if neither fits.

**Note:** no certificate gives an instant SmartScreen pass anymore. Reputation builds up as people download the signed files.
