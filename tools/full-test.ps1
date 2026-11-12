# Full local test of Xylem: build checks, all tests, long built-in fuzzing,
# speed, accuracy, and the security checks from the clearance plan that
# are safe to run on a normal PC (scratch folders in %TEMP% only).
#
#   .\tools\full-test.ps1            # writes test-evidence\REPORT.md
#
# Needs: cargo, cargo-deny, cargo-audit. Takes ~15-25 minutes. Opens app
# windows briefly (and one Explorer window for the planted-explorer test).
# Never touches real user data; every app run uses throwaway settings.
$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$env:Path += ";$env:USERPROFILE\.cargo\bin"
$ev = Join-Path $root "test-evidence"
Remove-Item $ev -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory $ev | Out-Null
$report = Join-Path $ev "REPORT.md"
$results = New-Object System.Collections.Generic.List[object]
function Rec($id, $name, $pass, $detail) {
  $results.Add([pscustomobject]@{ Id = $id; Name = $name; Result = $(if ($pass) { "PASS" } else { "FAIL" }); Detail = $detail })
  Write-Host ("[{0}] {1}: {2}  {3}" -f $(if ($pass) { "PASS" } else { "FAIL" }), $id, $name, $detail)
}
function Sum-Tests($lines) { (($lines | Select-String "test result: ok\. (\d+) passed" | ForEach-Object { [int]$_.Matches[0].Groups[1].Value }) | Measure-Object -Sum).Sum }
function Fails($lines) { @($lines | Select-String "test result: FAILED|panicked at").Count }
function Iso([string]$name) {
  $d = Join-Path $env:TEMP "xylem-ft-$name"
  Remove-Item $d -Recurse -Force -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force "$d\r\Xylem", "$d\l" | Out-Null
  '{"seen_notice":true,"remember_scans":false}' | Out-File -Encoding ascii "$d\r\Xylem\settings.json"
  $env:APPDATA = "$d\r"; $env:LOCALAPPDATA = "$d\l"
  $d
}

# --- A. Build, lint, supply chain ---------------------------------------------
$o = cargo build --release -p xylem -p xylem-cli -p xylem-bench 2>&1
Rec "A-01" "Release build" ($LASTEXITCODE -eq 0) ""
$o = cargo test --workspace 2>&1; $o | Out-File "$ev\tests-debug.txt"
Rec "A-02" "All tests (debug: overflow checks on)" ((Fails $o) -eq 0 -and $LASTEXITCODE -eq 0) "$(Sum-Tests $o) passed"
$o = cargo test --workspace --release 2>&1; $o | Out-File "$ev\tests-release.txt"
Rec "A-03" "All tests (release)" ((Fails $o) -eq 0 -and $LASTEXITCODE -eq 0) "$(Sum-Tests $o) passed"
$o = cargo clippy --workspace --all-targets -- -D warnings 2>&1; $o | Out-File "$ev\clippy.txt"
Rec "A-04" "Clippy, warnings as errors" ($LASTEXITCODE -eq 0) "$(@($o | Select-String '^(warning|error)').Count) findings"
$o = cargo deny check 2>&1; $o | Out-File "$ev\deny.txt"
Rec "A-05" "cargo-deny (licences OSI-only, advisories, sources)" ($LASTEXITCODE -eq 0) (($o | Select-String "advisories|licenses" | Select-Object -Last 1).Line)
$o = cargo audit 2>&1; $o | Out-File "$ev\audit.txt"
Rec "A-06" "cargo-audit (known vulnerabilities)" ($LASTEXITCODE -eq 0) "$(@($o | Select-String '^Crate:').Count) notices (unmaintained only = OK)"
$net = Select-String Cargo.lock -Pattern '^name = "(reqwest|hyper|ureq|curl|isahc|surf|attohttpc|native-tls|rustls)"'
Rec "A-07" "No HTTP/TLS client crates (PV-04)" (-not $net) ""

# --- B. Long built-in fuzzing ---------------------------------------------------
$env:XYLEM_FUZZ_ROUNDS = "2000000"
$t = Measure-Command { $o = cargo test --release -p xylem-scan-mft -p xylem-core fuzz 2>&1 }
Remove-Item Env:XYLEM_FUZZ_ROUNDS
$o | Out-File "$ev\fuzz-2M.txt"
Rec "B-01" "Built-in fuzz, 2M rounds per parser (PA-01)" ((Fails $o) -eq 0) ("{0:N0} s" -f $t.TotalSeconds)

# --- C. Speed and accuracy -------------------------------------------------------
$o = cargo test --release -p xylem-scan-mft -- --ignored --nocapture build_speed 2>&1
$bt = ($o | Select-String "best ([\d.]+) s").Matches[0].Groups[1].Value
Rec "C-01" "Tree build, 1.77M records (target <= 0.25 s)" ([double]$bt -le 0.25) "$bt s"
$o = cargo test --release -p xylem-core -- --ignored --nocapture classify_speed 2>&1
$cl = ($o | Select-String "classify ([\d.]+) s").Matches[0].Groups[1].Value
Rec "C-02" "Classify 1.8M items (target <= 0.25 s)" ([double]$cl -le 0.25) "$cl s"
$times = foreach ($i in 1..2) { (Measure-Command { .\target\release\xylem-cli.exe scan C: --mode walk --top 1 | Out-Null }).TotalSeconds }
Rec "C-03" "Folder walk of all of C: (target <= 8 s)" (($times | Measure-Object -Minimum).Minimum -le 8) (($times | ForEach-Object { "{0:N2} s" -f $_ }) -join ", ")
$o = .\target\release\xylem-bench.exe verify "C:\Program Files" 2>&1; $o | Out-File "$ev\verify.txt"
Rec "C-04" "Exact match with a plain std::fs walk (C:\Program Files)" (($o -join "`n") -match "match|OK|identical" -and ($o -join "`n") -notmatch "MISMATCH|differ") (($o | Select-Object -Last 1))

# --- D. Security checks (safe locally) ------------------------------------------
$exe = (Resolve-Path .\target\release\xylem.exe).Path
# EL-01: only the exact helper command line is accepted.
$bad = @(
  @("--helper-scan"),
  @("--helper-scan", "C:"),
  @("--helper-scan", "C:", "--pipe"),
  @("--helper-scan", "C:", "--pipe", "\\.\pipe\xylem-1", "extra"),
  @("--helper-scan", "C:\Users", "--pipe", "\\.\pipe\xylem-1"),
  @("--helper-scan", "1:", "--pipe", "\\.\pipe\xylem-1"),
  @("--helper-scan", "C:", "--pipe", "\\evil\pipe\xylem-1"),
  @("--helper-scan", "C:", "--pipe", "\\.\pipe\other"),
  @("--helper-scan", "C:", "--save-index", "$env:TEMP\xylem-1.idx"),
  @("--helper-scan", "C:", "--pipe", ("\\.\pipe\xylem-" + ("a" * 200)))
)
$ok = $true
foreach ($a in $bad) {
  $p = Start-Process $exe -ArgumentList $a -PassThru -WindowStyle Hidden
  if (-not $p.WaitForExit(10000)) { Stop-Process -Id $p.Id -Force; $ok = $false } elseif ($p.ExitCode -ne 2) { $ok = $false }
}
Rec "EL-01" "Helper refuses every malformed command line (exit 2)" $ok "$($bad.Count) forms"

# IN-06: DLL planting next to a portable copy.
$dd = Iso "dll"
Copy-Item $exe $dd
foreach ($dll in "dwmapi.dll", "uxtheme.dll", "OPENGL32.dll", "uiautomationcore.dll", "wintrust.dll", "bcryptprimitives.dll", "VCRUNTIME140.dll", "version.dll", "userenv.dll") { Set-Content "$dd\$dll" "not a dll" }
$p = Start-Process "$dd\xylem.exe" -ArgumentList "--helper-scan", "bad" -PassThru -WindowStyle Hidden
$helperOk = $p.WaitForExit(10000) -and $p.ExitCode -eq 2
if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force }
$p = Start-Process "$dd\xylem.exe" -PassThru
Start-Sleep 5; $p.Refresh()
$guiOk = -not $p.HasExited -and $p.MainWindowTitle -like "Xylem*"
if (-not $p.HasExited) { Stop-Process -Id $p.Id }
Rec "IN-06" "Planted junk DLLs are never loaded (helper + window)" ($helperOk -and $guiOk) "9 DLLs planted"

# BP-01: planted explorer.exe next to a portable copy.
$bd = Iso "bp"
Copy-Item $exe $bd
$marker = Join-Path $env:TEMP "xylem-planted-explorer-ran.txt"
Remove-Item $marker -ErrorAction SilentlyContinue
$src = Join-Path $bd "marker.rs"
"fn main() { let _ = std::fs::write(std::env::temp_dir().join(`"xylem-planted-explorer-ran.txt`"), b`"ran`"); }" | Out-File -Encoding ascii $src
& rustc -O -o "$bd\explorer.exe" $src 2>&1 | Out-Null
New-Item -ItemType Directory -Force "$bd\data" | Out-Null
Set-Content "$bd\data\file.txt" "x"
$p = Start-Process "$bd\xylem.exe" -ArgumentList "`"$bd\data`"" -PassThru
Start-Sleep 4
& "$root\tools\ui-click.ps1" -Pid0 $p.Id -Name "file.txt" | Out-Null
Start-Sleep 1
& "$root\tools\ui-click.ps1" -Pid0 $p.Id -Name "Show in Explorer" | Out-Null
Start-Sleep 3
Stop-Process -Id $p.Id -ErrorAction SilentlyContinue
Rec "BP-01" "Planted explorer.exe is not run by 'Show in Explorer'" ((Test-Path "$bd\explorer.exe") -and -not (Test-Path $marker)) "marker exe built: $(Test-Path "$bd\explorer.exe")"

# F-02: Fast scan refused outside Program Files (release build).
$fd = Iso "fast"
$p = Start-Process $exe -ArgumentList "C:\Windows\Fonts" -PassThru
Start-Sleep 4
& "$root\tools\ui-click.ps1" -Pid0 $p.Id -Name "$([char]0x26A1)  Fast scan" | Out-Null
Start-Sleep 2
& "$root\tools\ui-screenshot.ps1" -Pid0 $p.Id -Out "$ev\F-02-fast-scan-refused.png" -Wait 0 | Out-Null
# Only processes started from the test build count (an installed Xylem the
# user has open is not ours). The refusal text must be on screen.
$helpers = @(Get-Process xylem -ErrorAction SilentlyContinue | Where-Object { $_.Id -ne $p.Id -and $_.Path -eq $exe }).Count
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$win = [System.Windows.Automation.AutomationElement]::FromHandle($p.MainWindowHandle)
$texts = $win.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition) | ForEach-Object { $_.Current.Name }
$refused = [bool]($texts | Where-Object { $_ -like "*only works from an installed copy*" })
Stop-Process -Id $p.Id -ErrorAction SilentlyContinue
Rec "F-02" "Fast scan refused from a non-installed copy (message shown, no helper started)" ($refused -and $helpers -eq 0) "message: $refused, helpers: $helpers"

# PV-01: no network while scanning all of C: and using the app.
$nd = Iso "net"
$p = Start-Process $exe -ArgumentList "C:\" -PassThru
$conns = 0
for ($i = 0; $i -lt 16; $i++) {
  Start-Sleep 1
  $conns += @(Get-NetTCPConnection -OwningProcess $p.Id -ErrorAction SilentlyContinue).Count
  $conns += @(Get-NetUDPEndpoint -OwningProcess $p.Id -ErrorAction SilentlyContinue).Count
}
& "$root\tools\ui-screenshot.ps1" -Pid0 $p.Id -Out "$ev\PV-01-scan-of-C.png" -Wait 0 | Out-Null
Stop-Process -Id $p.Id -ErrorAction SilentlyContinue
Rec "PV-01" "Zero TCP/UDP sockets during a full scan of C:" ($conns -eq 0) "$conns socket observations in 16 s"

# PA-06: hostile settings file.
$sd = Iso "settings"
'{"dark": "yes", "last_path": "\\\\attacker.invalid\\share", "remember_scans": 1' + ("x" * 200000) | Out-File -Encoding ascii "$sd\r\Xylem\settings.json"
$p = Start-Process $exe -PassThru
Start-Sleep 5; $p.Refresh()
$alive = -not $p.HasExited
$smb = @(Get-NetTCPConnection -OwningProcess $p.Id -RemotePort 445 -ErrorAction SilentlyContinue).Count
& "$root\tools\ui-screenshot.ps1" -Pid0 $p.Id -Out "$ev\PA-06-hostile-settings.png" -Wait 0 | Out-Null
if ($alive) { Stop-Process -Id $p.Id }
Rec "PA-06" "Corrupt/huge settings: app starts with defaults, no SMB to a UNC last_path" ($alive -and $smb -eq 0) ""

# EL-08/RO: the scan never writes to the scanned tree.
$rd = Iso "ro"
New-Item -ItemType Directory -Force "$rd\data\a" | Out-Null
1..50 | ForEach-Object { Set-Content "$rd\data\a\f$_.txt" "x$_" }
# Files: name, size, last write. Folders: names only; Windows itself updates a
# folder's own timestamps lazily when *any* program lists it (checked with
# plain `dir`), so those aren't Xylem writes.
$snap = { Get-ChildItem "$rd\data" -Recurse -Force | ForEach-Object { if ($_.PSIsContainer) { "D|$($_.FullName)" } else { "F|$($_.FullName)|$($_.LastWriteTimeUtc.Ticks)|$($_.Length)" } } }
$before = & $snap
$p = Start-Process $exe -ArgumentList "`"$rd\data`"" -PassThru
Start-Sleep 4; Stop-Process -Id $p.Id -ErrorAction SilentlyContinue
$after = & $snap
Rec "RO-01" "Scanning changes nothing in the scanned folder" ((Compare-Object $before $after).Count -eq 0) "51 items compared"

# SC-01: a scheduled (headless) scan writes only Xylem's own files, keeps a
# size history, exits, and the CLI reads that history. Folders are compared
# by name only (Windows updates their own timestamps lazily, see RO-01).
$hd = Iso "hist"
$list = { Get-ChildItem "$hd\data" -Recurse -Force | ForEach-Object { if ($_.PSIsContainer) { "D|$($_.Name)" } else { "F|$($_.Name)|$($_.Length)|$($_.LastWriteTimeUtc.Ticks)" } } }
New-Item -ItemType Directory -Force "$hd\data\sub" | Out-Null
[IO.File]::WriteAllBytes("$hd\data\sub\a.bin", (New-Object byte[] 3000000))
@{ every_min = 30; roots = @("$hd\data"); on_battery = $true; warn_free_gb = 0 } | ConvertTo-Json | Out-File -Encoding utf8 "$hd\r\Xylem\schedule.json"
$p1 = Start-Process $exe -ArgumentList "--scheduled-scan" -PassThru -WindowStyle Hidden; $e1 = $p1.WaitForExit(60000) -and $p1.ExitCode -eq 0
[IO.File]::WriteAllBytes("$hd\data\sub\b.bin", (New-Object byte[] 5000000))
$beforeData = & $list
$p2 = Start-Process $exe -ArgumentList "--scheduled-scan" -PassThru -WindowStyle Hidden; $e2 = $p2.WaitForExit(60000) -and $p2.ExitCode -eq 0
$afterData = & $list
$untouched = (Compare-Object $beforeData $afterData).Count -eq 0
# Paths may come back in the long or the short (8.3) form of %TEMP%.
$written = @(Get-ChildItem "$hd\l", "$hd\r" -Recurse -File | ForEach-Object { $_.FullName })
$onlyOurs = @($written | Where-Object { $_ -notmatch '\\xylem-ft-hist\\(l|r)\\Xylem\\' }).Count -eq 0
$hj = .\target\release\xylem-cli.exe history "$hd\data" --folder sub --days 1 --json 2>&1 | Out-String
$h = $null; try { $h = $hj | ConvertFrom-Json } catch {}
$histOk = $h -and @($h.points).Count -eq 2 -and @($h.folder).Count -eq 2 -and @($h.grew.items | Where-Object { $_.path -eq "sub" }).Count -eq 1
Rec "SC-01" "Scheduled scan: exits, writes only Xylem's files, scanned folder untouched; CLI reads the size history" ($e1 -and $e2 -and $untouched -and $onlyOurs -and $histOk) "exit: $e1/$e2, untouched: $untouched, only ours: $onlyOurs, history: $histOk"

# Clean up scratch folders.
Get-ChildItem $env:TEMP -Directory -Filter "xylem-ft-*" | Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item $marker -ErrorAction SilentlyContinue

# --- Report -------------------------------------------------------------------
$pass = @($results | Where-Object Result -eq "PASS").Count
$lines = @("# Xylem full test: $(Get-Date -Format 'yyyy-MM-dd HH:mm')", "", "**$pass of $($results.Count) checks passed.** Commit $(git rev-parse --short HEAD).", "", "| ID | Check | Result | Detail |", "|---|---|---|---|")
$lines += $results | ForEach-Object { "| $($_.Id) | $($_.Name) | $($_.Result) | $($_.Detail) |" }
$lines | Set-Content -Encoding utf8 $report
Write-Host "`n$pass / $($results.Count) passed. Report: $report"
