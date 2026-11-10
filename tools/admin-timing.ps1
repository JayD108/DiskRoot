# Usage (admin PowerShell, or via Start-Process -Verb RunAs):  .\tools\admin-timing.ps1
# Quick MFT speed check: 5 runs with the per-phase trace.
# Results: %TEMP%\xylem-tools\admin-timing.txt
$rel = Join-Path (Split-Path -Parent $PSScriptRoot) "target\release"
$dir = Join-Path $env:TEMP "xylem-tools"
New-Item -ItemType Directory -Force $dir | Out-Null
$out = Join-Path $dir "admin-timing.txt"
$env:XYLEM_TRACE = "1"
& {
    "== MFT, default threads, 5 runs"
    & "$rel\xylem-bench.exe" run C: --mode mft --runs 5 2>&1 | ForEach-Object { "$_" }
    "== MFT, 4 threads, 3 runs"
    & "$rel\xylem-bench.exe" run C: --mode mft --runs 3 --threads 4 2>&1 | ForEach-Object { "$_" }
} *>&1 | Out-File $out -Encoding utf8
"ALL DONE" | Out-File $out -Append -Encoding utf8
