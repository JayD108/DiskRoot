# Fair speed benchmark: Xylem vs other disk analyzers on the same drive.
#
#   .\tools\bench-competitors.ps1 -Mode user    # normal user (folder walks)
#   .\tools\bench-competitors.ps1 -Mode admin   # run elevated (MFT / fast modes)
#
# Method: same PC, same drive, warm cache. Per tool and test: 1 warm-up run
# (not counted), then -Runs timed runs, tools interleaved round-robin. Time =
# wall clock from process start to exit (what a user waits for). Results are
# appended to bench-results\results.csv.
param([ValidateSet("user", "admin")][string]$Mode = "user", [string]$Drive = "C:", [int]$Runs = 3)
$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot
$out = Join-Path $root "bench-results"
New-Item -ItemType Directory -Force $out | Out-Null
$tmp = Join-Path $env:TEMP "xylem-bench-out"
New-Item -ItemType Directory -Force $tmp | Out-Null
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (($Mode -eq "admin") -ne $isAdmin) { throw "Mode $Mode but elevated=$isAdmin" }

$xylem = Join-Path $root "target\release\xylem-cli.exe"
$wiz = "C:\Program Files\WizTree\WizTree64.exe"
$wds = "C:\Program Files\WinDirStat\WinDirStat.exe"
$ts = "$env:USERPROFILE\AppData\Local\Programs\JAM Software\TreeSize\TreeSize.exe"
$ed = "$env:USERPROFILE\bench-tools\edirstat\bin\edirstat.exe"
$adm = if ($Mode -eq "admin") { "1" } else { "0" }
$xmode = if ($Mode -eq "admin") { "mft" } else { "walk" }
$d = "$Drive\"

# name, class, exe, args
$tests = @(
  @("Xylem", "scan", $xylem, @("scan", $Drive, "--mode", $xmode, "--top", "1")),
  @("WizTree 4.33", "scan", $wiz, @($Drive, "/export=`"$tmp\wiz-min.csv`"", "/exportfiles=0", "/exportmaxdepth=1", "/admin=$adm")),
  # TreeSize: its command line did not scan here (likely a first-run/trial dialog),
  # so it is excluded until that is resolved.
  @("eDirStat 2.0", "scan", $ed, @("--benchmark", $d)),
  @("Xylem", "scan+csv", $xylem, @("scan", $Drive, "--mode", $xmode, "--top", "1", "--export", "$tmp\xylem.csv")),
  @("WizTree 4.33", "scan+csv", $wiz, @($Drive, "/export=`"$tmp\wiz.csv`"", "/admin=$adm")),
  @("WinDirStat 2.9", "scan+csv", $wds, @("/SaveTo", "$tmp\wds.csv", $d))
)
$tests = $tests | Where-Object { Test-Path $_[2] }

function Run-One($t) {
  Get-ChildItem $tmp -File -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
  $sw = [Diagnostics.Stopwatch]::StartNew()
  $p = Start-Process -FilePath $t[2] -ArgumentList $t[3] -PassThru -WindowStyle Hidden `
    -RedirectStandardOutput "$tmp\stdout.txt" -RedirectStandardError "$tmp\stderr.txt"
  if (-not $p.WaitForExit(600000)) { Stop-Process -Id $p.Id -Force; return -1 }
  $sw.Stop()
  return $sw.Elapsed.TotalSeconds
}

$csv = Join-Path $out "results.csv"
if (-not (Test-Path $csv)) { "time,mode,class,tool,run,seconds" | Set-Content $csv }
Write-Host "warm-up..."
foreach ($t in $tests) { [void](Run-One $t) }
for ($r = 1; $r -le $Runs; $r++) {
  foreach ($t in $tests) {
    $s = Run-One $t
    "{0},{1},{2},{3},{4},{5:N3}" -f (Get-Date -Format s), $Mode, $t[1], $t[0], $r, $s | Add-Content $csv
    Write-Host ("{0,-6} {1,-9} {2,-15} run {3}: {4:N2} s" -f $Mode, $t[1], $t[0], $r, $s)
  }
}
# Sanity: how many rows did each full export contain?
foreach ($f in "xylem.csv", "wiz.csv", "wds.csv") {
  $p = Join-Path $tmp $f
  if (Test-Path $p) { "{0}: {1:N0} lines" -f $f, ((Get-Content $p -ReadCount 100000 | Measure-Object -Sum { $_.Count }).Sum) | Add-Content (Join-Path $out "export-sizes-$Mode.txt") }
}
Write-Host "done -> $csv"
