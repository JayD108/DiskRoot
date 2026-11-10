# Usage (admin PowerShell for -Admin): .\tools\compare-wiztree.ps1 [-Admin] [-Runs 3]
# Results: %TEMP%\xylem-tools\compare-<mode>.txt
param([switch]$Admin, [int]$Runs = 3)
# Times WizTree and xylem the same way: wall clock from process start to
# exit, plus peak RAM, sampled every 20 ms. One warm-up run each first.

$dir     = Join-Path $env:TEMP "xylem-tools"
New-Item -ItemType Directory -Force $dir | Out-Null
$mode    = if ($Admin) { "admin" } else { "user" }
$out     = Join-Path $dir "compare-$mode.txt"
$wiztree = "C:\Program Files\WizTree\WizTree64.exe"
$xylem = Join-Path (Split-Path -Parent $PSScriptRoot) "target\release\xylem-cli.exe"
$wtCsv   = Join-Path $dir "wt-$mode.csv"
$drOut   = Join-Path $dir "xylem-$mode.txt"
$adminArg = if ($Admin) { "/admin=1" } else { "/admin=0" }

function Measure-Run($exe, $argList, $stdout) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    if ($stdout) {
        $p = Start-Process -FilePath $exe -ArgumentList $argList -PassThru -NoNewWindow -RedirectStandardOutput $stdout
    } else {
        $p = Start-Process -FilePath $exe -ArgumentList $argList -PassThru
    }
    $peak = 0
    while (-not $p.HasExited) {
        try { $p.Refresh(); if ($p.PeakWorkingSet64 -gt $peak) { $peak = $p.PeakWorkingSet64 } } catch {}
        Start-Sleep -Milliseconds 20
    }
    $sw.Stop()
    [pscustomobject]@{ Seconds = [math]::Round($sw.Elapsed.TotalSeconds, 2); PeakMB = [math]::Round($peak / 1MB) }
}

$elevated = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
"mode=$mode elevated=$elevated runs=$Runs" | Out-File $out -Encoding utf8

$tools = @(
    @{ Name = "WizTree"; Exe = $wiztree; Args = @("C:", "/export=`"$wtCsv`"", $adminArg); Out = $null },
    @{ Name = "xylem"; Exe = $xylem; Args = @("scan", "C:", "--top", "50", "--export", "`"$(Join-Path $dir "dr-$mode.csv")`""); Out = $drOut }
)
foreach ($t in $tools) { $null = Measure-Run $t.Exe $t.Args $t.Out }   # warm-up
for ($i = 1; $i -le $Runs; $i++) {
    foreach ($t in $tools) {
        $r = Measure-Run $t.Exe $t.Args $t.Out
        "{0,-8} run {1}: {2,6} s  peak {3,5} MB" -f $t.Name, $i, $r.Seconds, $r.PeakMB | Out-File $out -Append -Encoding utf8
    }
}
"done" | Out-File $out -Append -Encoding utf8
