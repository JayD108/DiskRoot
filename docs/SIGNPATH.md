# Code signing with SignPath Foundation

How Xylem gets free code signing from [SignPath Foundation](https://signpath.org/), and how the pieces in this repo fit together.

Official rules: https://signpath.org/terms. Apply: https://signpath.org/apply.

---

## 1. Eligibility checklist (SignPath Foundation conditions → Xylem)

| Condition | Status | Where |
|---|---|---|
| OSI-approved license, no *commercial* dual licensing | ✅ MIT OR Apache-2.0, both OSI-approved and free, the usual Rust dual license | `LICENSE-MIT`, `LICENSE-APACHE` |
| No proprietary components | ✅ All Rust dependencies are MIT/Apache/Zlib/BSD/ISC (audited with `cargo metadata`). egui's bundled fonts (one was under the non-OSI Ubuntu Font Licence) were **removed**; the app uses fonts already in Windows. The installer is NSIS (zlib license; LZMA under CPL) | `Cargo.toml`, `installer/xylem.nsi` |
| Free of malware / unwanted software | ✅ No network code, no telemetry, no bundled offers, nothing runs at startup | `docs/SECURITY-AND-PRIVACY.md` |
| Actively maintained, already released | ✅ Public repo with history; v0.1.0 published on GitHub Releases (unsigned) | Releases page |
| Functionality described on the download page | ✅ Release notes describe what it does | `.github/release-notes.md` |
| Own project; team owns the repository | ✅ JayD108 owns the repo and writes the code | — |
| Built from source in a verifiable way (CI) | ✅ Every release file is built by GitHub Actions from the tagged commit | `.github/workflows/release.yml` |
| No hacking tools / no security circumvention | ✅ A disk-space analyzer. It reads the file table read-only, with admin only after the Windows UAC prompt | `docs/HOW-IT-WORKS.md` |
| No system changes without warnings | ✅ The app changes nothing on its own. Deleting needs confirmation and goes to the Recycle Bin. The installer only adds its folder, a shortcut and an uninstall entry | — |
| Uninstallation provided | ✅ NSIS uninstaller (Settings → Apps); tested in CI on every release | `release.yml` "Test install and uninstall" |
| Privacy policy or "no transfer" statement | ✅ "This program will not transfer any information…" | README *Code signing policy*, release notes |
| Code signing policy section on home + download page | ✅ README section *Code signing policy*; also in every release's notes | README, release notes |
| Team roles named | ✅ Committers/reviewers/approvers: JayD108 | README |
| **MFA for all team members on GitHub and SignPath** | ⚠️ **You must turn this on**: GitHub → Settings → Password and authentication → Two-factor authentication. Also use MFA on your SignPath account | — |
| Product name/version metadata consistent | ✅ The exes carry ProductName "Xylem" and the workspace version; the installer carries the same. The release workflow fails if the tag ≠ `Cargo.toml` version | `crates/*/build.rs`, `installer/xylem.nsi`, `release.yml` |
| "Verifiable reputation" for executables | ⚠️ **Not something code can fix.** Xylem is new. Stars, users, downloads, issues and outside mentions help; SignPath decides | — |

---

## 2. Applying (what to fill in)

Suggested answers for the application form. The form's exact fields may differ; adapt as needed.

- **Project name:** Xylem
- **Repository:** https://github.com/JayD108/Xylem
- **Homepage / download page:** https://github.com/JayD108/Xylem (README) and https://github.com/JayD108/Xylem/releases
- **License:** MIT OR Apache-2.0 (the standard Rust dual license; both OSI-approved, no commercial licensing)
- **Description:** Xylem is a fast, open-source disk space analyzer for Windows. It reads the NTFS Master File Table (read-only, after the UAC prompt) to map a whole drive in about 1–2 seconds. It shows a tree-table with a sunburst or treemap, explains what folders are and whether they are safe to delete, and deletes only to the Recycle Bin with re-checks and warnings. It has no network code and no telemetry.
- **Files to sign:** `xylem.exe` (app; also its own elevated helper), `xylem-cli.exe` (command line), `xylem-setup-<version>.exe` (NSIS installer)
- **Build system:** GitHub Actions (`.github/workflows/release.yml`), windows-latest, Rust stable, NSIS
- **Team:** JayD108 (author, reviewer, approver)
- **Why signing:** Smart App Control and SmartScreen block or warn about unsigned exes. Xylem asks for admin rights for its fast scan, so users should be able to verify the publisher.

---

## 3. After acceptance: switching signing on

SignPath will create the project, or you create it in their web app. Then:

1. **In SignPath:**
   - Project slug: `Xylem` (or note the slug they give you).
   - Signing policy: `release-signing` (or note the slug), with the GitHub trusted build system and origin verification enabled.
   - Two **artifact configurations** (below), with the slugs `exes` and `installer`.
   - Create an API token for a CI user that is allowed to submit signing requests.
2. **In GitHub** (repo → Settings → Secrets and variables → Actions):
   - Secret `SIGNPATH_API_TOKEN` = the API token.
   - Variable `SIGNPATH_ORGANIZATION_ID` = your SignPath organization ID.
   - Optional variables `SIGNPATH_PROJECT_SLUG` and `SIGNPATH_POLICY_SLUG`, if they differ from `Xylem` / `release-signing`.
3. **Release:** bump `version` in `Cargo.toml`, commit, then `git tag v<version>` and `git push origin v<version>`. The workflow pauses at each signing step until you **approve the request in SignPath**, then publishes a signed release.

Nothing else changes: the workflow turns signing on as soon as the secret and the variable exist.

### Artifact configuration `exes`
GitHub uploads the two exes as one zip artifact.
```xml
<?xml version="1.0" encoding="utf-8"?>
<artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
  <zip-file>
    <pe-file path="xylem.exe" product-name="Xylem">
      <authenticode-sign />
    </pe-file>
    <pe-file path="xylem-cli.exe" product-name="Xylem">
      <authenticode-sign />
    </pe-file>
  </zip-file>
</artifact-configuration>
```

### Artifact configuration `installer`
```xml
<?xml version="1.0" encoding="utf-8"?>
<artifact-configuration xmlns="http://signpath.io/artifact-configuration/v1">
  <zip-file>
    <pe-file path="xylem-setup-*.exe" product-name="Xylem">
      <authenticode-sign />
    </pe-file>
  </zip-file>
</artifact-configuration>
```

---

## 4. How it works end to end

```
git tag v0.2.0 ─► GitHub Actions (windows-latest)
   test ─► build exes ─► upload "exes" artifact ─► SignPath signs (you approve)
   ─► NSIS installer + zip built FROM the signed exes ─► real install/uninstall test
   ─► upload installer ─► SignPath signs (you approve) ─► SHA-256 checksums
   ─► GitHub Release with notes (code signing policy + privacy statement)
```

- SignPath only signs artifacts that GitHub Actions built from this repository (origin verification). Nothing from a personal PC gets signed.
- The certificate is issued to **SignPath Foundation**, so Windows shows "SignPath Foundation" as the publisher.
- The in-app shield (Help → Security & privacy) then shows **Signed** for installed copies.
