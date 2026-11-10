# Usage: run from an ADMIN PowerShell:  .\tools\admin-bench.ps1
# MFT timing per thread count (with phase trace), MFT-vs-walk accuracy, and
# the WizTree head-to-head. Results: %TEMP%\xylem-tools\admin-bench.txt
$rel = Join-Path (Split-Path -Parent $PSScriptRoot) "target\release"
$dir = Join-Path $env:TEMP "xylem-tools"
New-Item -ItemType Directory -Force $dir | Out-Null
$out = Join-Path $dir "admin-bench.txt"
& {
    $env:XYLEM_TRACE = "1"
    foreach ($t in 2, 4, 8) {
        "== MFT, $t threads"
        & "$rel\xylem-bench.exe" run C: --mode mft --runs 3 --threads $t 2>&1 | ForEach-Object { "$_" }
    }
    Remove-Item Env:XYLEM_TRACE
    "`n== MFT vs folder walk"
    & "$rel\xylem-bench.exe" modes C: 2>&1 | ForEach-Object { "$_" }
} *>&1 | Out-File $out -Encoding utf8
& (Join-Path $PSScriptRoot "compare-wiztree.ps1") -Admin
Get-Content (Join-Path $dir "compare-admin.txt") | Out-File $out -Append -Encoding utf8
Remove-Item (Join-Path $dir "*.csv") -ErrorAction SilentlyContinue
"ALL DONE" | Out-File $out -Append -Encoding utf8
