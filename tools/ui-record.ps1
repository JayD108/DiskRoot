# Records a real Fast scan, frame by frame, for the README demo.
#
#   .\tools\ui-record.ps1 -OutDir $env:TEMP\xylem-rec
#   python tools\make-demo-gif.py $env:TEMP\xylem-rec docs\screenshots\fast-scan.gif
#
# - Starts the INSTALLED Xylem (Fast scan only runs from Program Files) with
#   settings in a temp folder: your own settings, remembered scans and
#   deletion log are never touched.
# - Presses "Fast scan" through UI Automation (the mouse isn't moved). You
#   click Yes on the Windows prompt; the secure desktop can't be captured.
# - Captures ONLY Xylem's own window with PrintWindow (never the screen) and
#   logs when the admin helper runs, so the GIF maker can cut the time spent
#   on the Windows prompt and show real scan time.
# - Frames are kept IN MEMORY (shrunk to GIF size) and written only after the
#   scan: writing them during the scan competed with Fast scan for the disk
#   and made it ~4x slower. Run it on an idle PC (no fuzzing, no downloads).
param(
  [string]$OutDir = (Join-Path $env:TEMP "xylem-rec"),
  [string]$Exe = "C:\Program Files\Xylem\xylem.exe",
  [int]$Seconds = 40
)
Add-Type -AssemblyName System.Drawing, UIAutomationClient, UIAutomationTypes
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class RecWin {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
"@
[RecWin]::SetProcessDPIAware() | Out-Null
if (-not (Test-Path $Exe)) { "Not installed: $Exe"; exit 1 }

Remove-Item -Recurse -Force $OutDir -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $OutDir | Out-Null
$prof = Join-Path $OutDir "profile"
New-Item -ItemType Directory -Force (Join-Path $prof "roaming\Xylem"), (Join-Path $prof "local") | Out-Null
# Category grouping: the tree shows categories, not folder names.
[IO.File]::WriteAllText((Join-Path $prof "roaming\Xylem\settings.json"),
  '{"seen_notice":true,"remember_scans":false,"live_updates":false,"by_category":true,"last_path":"C:\\"}')

$before = @(Get-Process xylem -ErrorAction SilentlyContinue | ForEach-Object Id)
$env:APPDATA = Join-Path $prof "roaming"
$env:LOCALAPPDATA = Join-Path $prof "local"
$p = Start-Process -FilePath $Exe -PassThru
$deadline = (Get-Date).AddSeconds(20)
do {
  Start-Sleep -Milliseconds 200
  $p.Refresh(); $h = $p.MainWindowHandle
  $r = New-Object RecWin+RECT
  if ($h -ne 0) { [RecWin]::GetWindowRect($h, [ref]$r) | Out-Null }
} while (($h -eq 0 -or ($r.R - $r.L) -lt 400) -and (Get-Date) -lt $deadline)
if ($h -eq 0) { "no window"; exit 1 }
Start-Sleep -Milliseconds 1500

$frames = New-Object System.Collections.Generic.List[object]
function Grab([string]$path) {
  [RecWin]::GetWindowRect($h, [ref]$r) | Out-Null
  $w = $r.R - $r.L; $hh = $r.B - $r.T
  $bmp = New-Object System.Drawing.Bitmap $w, $hh
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $hdc = $g.GetHdc()
  [RecWin]::PrintWindow($h, $hdc, 2) | Out-Null
  $g.ReleaseHdc($hdc); $g.Dispose()
  # Keep a GIF-sized copy in memory; nothing touches the disk until the end.
  $sw2 = 1100; $sh2 = [int]($hh * $sw2 / $w)
  $small = New-Object System.Drawing.Bitmap $sw2, $sh2
  $g2 = [System.Drawing.Graphics]::FromImage($small)
  $g2.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
  $g2.DrawImage($bmp, 0, 0, $sw2, $sh2); $g2.Dispose(); $bmp.Dispose()
  $frames.Add(@($path, $small))
}

$sw = [Diagnostics.Stopwatch]::StartNew()
$log = New-Object System.Collections.Generic.List[string]
$log.Add("frame,ms,helper")
$n = 0
# A few frames of the start screen first.
for ($i = 0; $i -lt 4; $i++) { Grab (Join-Path $OutDir ("f{0:D4}.png" -f $n)); $log.Add("$n,$($sw.ElapsedMilliseconds),0"); $n++ }

$root = [System.Windows.Automation.AutomationElement]::FromHandle($h)
# Match by "Fast scan" (the button's label also has an icon character).
$all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
$btn = $all | Where-Object { $_.Current.Name -like "*Fast scan" } | Select-Object -First 1
if (-not $btn) { "Fast scan button not found"; exit 1 }
$pat = $btn.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
$pat.Invoke()
"Click Yes on the Windows prompt..."

$seenHelper = $false; $helperGoneAt = $null
while ($sw.Elapsed.TotalSeconds -lt $Seconds) {
  $others = @(Get-Process xylem -ErrorAction SilentlyContinue | Where-Object { $_.Id -ne $p.Id -and $before -notcontains $_.Id })
  $helper = [int]($others.Count -gt 0)
  if ($helper) { $seenHelper = $true }
  if ($seenHelper -and -not $helper -and -not $helperGoneAt) { $helperGoneAt = $sw.ElapsedMilliseconds }
  Grab (Join-Path $OutDir ("f{0:D4}.png" -f $n))
  $log.Add("$n,$($sw.ElapsedMilliseconds),$helper"); $n++
  if ($helperGoneAt -and ($sw.ElapsedMilliseconds - $helperGoneAt) -gt 3500) { break }
  # Waiting on the Windows prompt: a frame every 0.5 s is plenty.
  if (-not $seenHelper) { Start-Sleep -Milliseconds 500 }
}
foreach ($f in $frames) { $f[1].Save($f[0], [System.Drawing.Imaging.ImageFormat]::Png); $f[1].Dispose() }
$log | Set-Content -Encoding ascii (Join-Path $OutDir "frames.csv")
Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
"Saved $n frames to $OutDir (helper seen: $seenHelper)"
