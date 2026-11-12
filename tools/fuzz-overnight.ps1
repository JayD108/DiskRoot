# Long coverage-guided fuzzing of DiskRoot's untrusted-input parsers (cargo-fuzz
# + libFuzzer + AddressSanitizer), all targets in parallel.
#
#   .\tools\fuzz-overnight.ps1 -Hours 10          # default 10 h per target
#   .\tools\fuzz-overnight.ps1 -Hours 24          # the release requirement
#
# Needs: rustup toolchain "nightly", cargo-fuzz, and the MSVC ASan runtime
# (clang_rt.asan_dynamic-x86_64.dll, part of the VS C++ build tools).
# Output: fuzz\logs\<target>.log, crashes in fuzz\artifacts\<target>\,
# the growing corpus in fuzz\corpus\<target>\ (seed it with
# `cargo test -p diskroot-scan-mft write_fuzz_seeds -- --ignored`).
param([double]$Hours = 10, [string[]]$Targets = @("mft_record", "run_list", "boot_sector", "mft_tree", "index_file", "snapshot_file"))
$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$asan = Get-ChildItem "${env:ProgramFiles(x86)}\Microsoft Visual Studio", "$env:ProgramFiles\Microsoft Visual Studio" -Recurse -Filter "clang_rt.asan_dynamic-x86_64.dll" -ErrorAction SilentlyContinue |
  Where-Object { $_.FullName -match 'Hostx64\\x64' } | Select-Object -First 1
if (-not $asan) { throw "ASan runtime not found (install the MSVC AddressSanitizer component)" }
$env:Path = "$($asan.DirectoryName);$env:Path;$env:USERPROFILE\.cargo\bin"
New-Item -ItemType Directory -Force fuzz\logs | Out-Null
$secs = [int]($Hours * 3600)
"started $(Get-Date -Format s), $Hours h per target: $($Targets -join ', ')" | Set-Content fuzz\logs\_status.txt

foreach ($t in $Targets) {
  & cargo +nightly fuzz build $t 2>&1 | Out-Null
}
$procs = foreach ($t in $Targets) {
  New-Item -ItemType Directory -Force "fuzz\corpus\$t", "fuzz\artifacts\$t" | Out-Null
  $maxLen = if ($t -eq "mft_tree") { 65536 } else { 4096 }
  $args = @("+nightly", "fuzz", "run", $t, "--", "-max_total_time=$secs", "-rss_limit_mb=2048", "-timeout=20", "-max_len=$maxLen", "-print_final_stats=1")
  Start-Process -FilePath "cargo" -ArgumentList $args -NoNewWindow -PassThru `
    -RedirectStandardError "fuzz\logs\$t.log" -RedirectStandardOutput "fuzz\logs\$t.out"
}
$procs | Wait-Process
$summary = foreach ($t in $Targets) {
  $crashes = @(Get-ChildItem "fuzz\artifacts\$t" -File -ErrorAction SilentlyContinue).Count
  $last = Select-String "fuzz\logs\$t.log" -Pattern "^Done \d+ runs|stat::number_of_executed_units" | Select-Object -Last 1
  "{0,-14} crashes/hangs: {1}   {2}" -f $t, $crashes, $(if ($last) { $last.Line } else { "(no final stats)" })
}
("finished $(Get-Date -Format s)", $summary) | Add-Content fuzz\logs\_status.txt
