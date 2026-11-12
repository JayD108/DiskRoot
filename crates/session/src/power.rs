//! Phase 10 "power tools", all opt-in and all done by Windows' own tools:
//!
//! - [`powershell_plan`]: a cleanup script for review and running elsewhere.
//!   DiskRoot writes it and never runs it.
//! - Folder compression lives in [`crate::compress`] (DiskRoot's own engine
//!   driving Windows' file compression directly).
//! - [`compact_vhdx`]: shrinks a WSL/Docker virtual disk with Windows'
//!   `diskpart` behind the UAC prompt, only while nothing uses the disk.

use std::path::Path;

use crate::tools;

/// Paths that can't be written into a PowerShell single-quoted string or a
/// command line safely: control characters, double quotes (never valid in
/// Windows names), and the "smart" quotes PowerShell also treats as quotes.
fn unsafe_path(p: &str) -> Option<&'static str> {
    if p.chars().any(char::is_control) {
        Some("contains control characters")
    } else if p.contains(['"', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}']) {
        Some("contains quote characters")
    } else {
        None
    }
}

/// One item for [`powershell_plan`].
pub struct PlanItem {
    pub path: String,
    pub dir: bool,
    pub bytes: u64,
}

/// A PowerShell script that moves `items` to the Recycle Bin, re-checking
/// each one first: it must still exist, be the same type, and neither it
/// nor any folder above it may be a link (junction/symlink). Without `-Run`
/// it only prints what it would do. Items with unusual characters are left
/// out and listed in the script's header.
pub fn powershell_plan(items: &[PlanItem], scan_root: &str, when: &str) -> String {
    let mut ok = Vec::new();
    let mut skipped = Vec::new();
    for it in items {
        match unsafe_path(&it.path) {
            None => ok.push(it),
            Some(why) => skipped.push(format!("#   {} ({why})", crate::safety::visible(&it.path))),
        }
    }
    let total: u64 = ok.iter().map(|i| i.bytes).sum();
    let mut s = String::new();
    s.push_str("# DiskRoot cleanup plan\n");
    s.push_str(&format!("# Made {when} from a scan of {}\n", crate::safety::visible(scan_root)));
    s.push_str(&format!("# {} items, about {} on disk.\n#\n", ok.len(), diskroot_core::format::bytes(total)));
    s.push_str("# Read it first. Without -Run it only shows what it would do:\n");
    s.push_str("#   powershell -ExecutionPolicy Bypass -File <this file>\n");
    s.push_str("#   powershell -ExecutionPolicy Bypass -File <this file> -Run\n");
    s.push_str("# Everything goes to the Recycle Bin. An item is skipped if it's gone, changed type,\n");
    s.push_str("# or it (or a folder above it) is now a link. DiskRoot itself never runs this script.\n");
    if !skipped.is_empty() {
        s.push_str("#\n# Left out (unusual characters; delete these by hand if you want):\n");
        for x in &skipped {
            s.push_str(x);
            s.push('\n');
        }
    }
    s.push_str("\nparam([switch]$Run)\n$ErrorActionPreference = 'Stop'\nAdd-Type -AssemblyName Microsoft.VisualBasic\n$items = @(\n");
    for it in &ok {
        // Single-quoted: nothing is expanded; a ' is written as ''.
        let q = it.path.replace('\'', "''");
        s.push_str(&format!("    @{{ Path = '{q}'; Dir = ${}; Bytes = {} }}\n", if it.dir { "true" } else { "false" }, it.bytes));
    }
    s.push_str(
        r#")
$done = 0; $skipped = 0
foreach ($it in $items) {
    $p = $it.Path
    $i = Get-Item -LiteralPath $p -Force -ErrorAction SilentlyContinue
    if (-not $i) { "skip (gone):         $p"; $skipped++; continue }
    if ($i.PSIsContainer -ne $it.Dir) { "skip (type changed): $p"; $skipped++; continue }
    $c = $i; $link = $false
    while ($c) {
        if ($c.Attributes -band [IO.FileAttributes]::ReparsePoint) { $link = $true; break }
        $c = if ($c.PSIsContainer) { $c.Parent } else { $c.Directory }
    }
    if ($link) { "skip (now a link):   $p"; $skipped++; continue }
    if (-not $Run) { "would recycle:       $p"; continue }
    try {
        if ($it.Dir) {
            [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteDirectory($p, 'OnlyErrorDialogs', 'SendToRecycleBin')
        } else {
            [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile($p, 'OnlyErrorDialogs', 'SendToRecycleBin')
        }
        "recycled:            $p"; $done++
    } catch { "failed:              $p ($($_.Exception.Message))"; $skipped++ }
}
if ($Run) { "Done: $done recycled, $skipped skipped." } else { "Dry run: nothing was changed. Add -Run to recycle." }
"#,
    );
    s
}

/// True if a program has the file open (WSL or Docker running): an
/// exclusive open is refused then.
pub fn in_use(path: &Path) -> bool {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().read(true).share_mode(0).open(path).is_err()
}

/// `wsl --shutdown` (no admin): stops every WSL distro so its disk is free.
pub fn wsl_shutdown() -> Result<(), String> {
    match tools::run_system_tool("wsl.exe", "--shutdown")? {
        0 => Ok(()),
        c => Err(format!("wsl --shutdown finished with code {c}")),
    }
}

/// Checks that `path` is a virtual disk DiskRoot may hand to diskpart.
pub fn check_vhdx(path: &Path) -> Result<(), String> {
    let s = path.to_string_lossy();
    if let Some(why) = unsafe_path(&s) {
        return Err(format!("The path {why}."));
    }
    if !path.is_absolute() || !s.to_ascii_lowercase().ends_with(".vhdx") {
        return Err("Only .vhdx virtual disks can be compacted.".into());
    }
    let m = std::fs::symlink_metadata(path).map_err(|e| format!("Can't read {s}: {e}"))?;
    if !m.is_file() || m.file_type().is_symlink() {
        return Err("That isn't a plain file.".into());
    }
    let win = crate::integrity::windows_dir().to_string_lossy().to_ascii_lowercase();
    if s.to_ascii_lowercase().starts_with(&win) {
        return Err("DiskRoot won't touch virtual disks inside Windows.".into());
    }
    Ok(())
}

/// The diskpart script for compacting `path` read-only-attached.
pub fn diskpart_script(path: &Path) -> String {
    format!("select vdisk file=\"{}\"\nattach vdisk readonly\ncompact vdisk\ndetach vdisk\nexit\n", path.display())
}

/// Exit codes of `diskroot.exe --helper-compact-vhdx <path>`.
pub const COMPACT_OK: i32 = 0;
pub const COMPACT_BAD_ARGS: i32 = 2;
pub const COMPACT_IN_USE: i32 = 6;
pub const COMPACT_FAILED: i32 = 7;

/// Elevated side: `diskroot.exe --helper-compact-vhdx <path>`. Re-checks the
/// path and that nothing uses the disk, then feeds the script to diskpart
/// through a pipe (no script file that another program could swap).
pub fn run_if_compact_helper(args: &[String]) -> Option<i32> {
    if args.get(1).map(String::as_str) != Some("--helper-compact-vhdx") {
        return None;
    }
    let (Some(p), 3) = (args.get(2), args.len()) else { return Some(COMPACT_BAD_ARGS) };
    let path = Path::new(p);
    if check_vhdx(path).is_err() {
        return Some(COMPACT_BAD_ARGS);
    }
    if in_use(path) {
        return Some(COMPACT_IN_USE);
    }
    use std::io::Write;
    use std::process::{Command, Stdio};
    let child = Command::new(tools::system32("diskpart.exe")).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    let Ok(mut child) = child else { return Some(COMPACT_FAILED) };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(diskpart_script(path).as_bytes());
    }
    Some(match child.wait() {
        Ok(s) if s.success() => COMPACT_OK,
        _ => COMPACT_FAILED,
    })
}

/// App side: asks Windows for admin rights and runs the helper above.
/// Blocks for as long as diskpart works (minutes for big disks).
pub fn compact_vhdx(path: &Path) -> Result<String, String> {
    check_vhdx(path)?;
    if in_use(path) {
        return Err("The virtual disk is in use. Stop WSL (wsl --shutdown) or quit Docker Desktop first.".into());
    }
    // Release builds only elevate an installed copy (same rule as Fast scan).
    #[cfg(not(debug_assertions))]
    if !crate::integrity::fast_scan_allowed() {
        return Err("Compacting needs admin rights, so it only works from an installed copy of DiskRoot.".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.with_file_name("diskroot.exe");
    let before = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let code = tools::run_system_tool_admin(&exe, &format!("--helper-compact-vhdx \"{}\"", path.display()))?;
    let after = std::fs::metadata(path).map(|m| m.len()).unwrap_or(before);
    match code as i32 {
        COMPACT_OK => Ok(format!(
            "Done: {} → {} ({} freed).",
            diskroot_core::format::bytes(before),
            diskroot_core::format::bytes(after),
            diskroot_core::format::bytes(before.saturating_sub(after))
        )),
        COMPACT_IN_USE => Err("The virtual disk was in use when the compaction was about to start. Nothing was changed.".into()),
        COMPACT_BAD_ARGS => Err("The helper refused the path.".into()),
        c => Err(format!("diskpart couldn't compact the disk (code {c}). Nothing else was changed.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_quotes_paths_and_leaves_out_odd_ones() {
        let items = vec![
            PlanItem { path: r"C:\Users\me\it's mine\node_modules".into(), dir: true, bytes: 10 },
            PlanItem { path: "C:\\Users\\me\\a\u{2019}b".into(), dir: false, bytes: 1 },
            PlanItem { path: "C:\\x\ny".into(), dir: false, bytes: 1 },
            PlanItem { path: r"C:\Users\me\$env:TEMP x.zip".into(), dir: false, bytes: 5 },
        ];
        let s = powershell_plan(&items, r"C:\", "2026-11-11");
        assert!(s.contains(r"Path = 'C:\Users\me\it''s mine\node_modules'; Dir = $true"));
        assert!(s.contains(r"Path = 'C:\Users\me\$env:TEMP x.zip'"), "single quotes: $ isn't expanded");
        assert!(s.contains("# Left out"));
        assert_eq!(s.matches("@{ Path").count(), 2, "odd paths aren't in the list");
        assert!(s.contains("param([switch]$Run)") && s.contains("SendToRecycleBin"));
        assert!(!s.contains("Remove-Item"), "never a permanent delete");
    }

    #[test]
    fn vhdx_checks() {
        assert!(check_vhdx(Path::new("relative.vhdx")).is_err());
        assert!(check_vhdx(Path::new(r"C:\x\disk.vhd")).is_err());
        assert!(check_vhdx(Path::new(r"C:\Windows\a.vhdx")).is_err());
        let dir = std::env::temp_dir().join(format!("diskroot-vhdx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("ext4.vhdx");
        std::fs::write(&f, b"not really a disk").unwrap();
        assert!(check_vhdx(&f).is_ok());
        assert!(!in_use(&f));
        let held = std::fs::File::open(&f).unwrap();
        assert!(in_use(&f), "an open handle makes it in use");
        drop(held);
        assert!(diskpart_script(&f).starts_with("select vdisk file=\""));
        assert!(diskpart_script(&f).contains("attach vdisk readonly"));
        let _ = std::fs::remove_dir_all(&dir);
        // The elevated helper refuses anything but one valid path.
        let a = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(run_if_compact_helper(&a(&["x", "--other"])), None);
        assert_eq!(run_if_compact_helper(&a(&["x", "--helper-compact-vhdx"])), Some(COMPACT_BAD_ARGS));
        assert_eq!(run_if_compact_helper(&a(&["x", "--helper-compact-vhdx", r"C:\a.vhdx", "extra"])), Some(COMPACT_BAD_ARGS));
        assert_eq!(run_if_compact_helper(&a(&["x", "--helper-compact-vhdx", r"C:\nope\missing.vhdx"])), Some(COMPACT_BAD_ARGS));
    }

}
