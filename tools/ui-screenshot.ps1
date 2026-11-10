# Usage: .\tools\ui-screenshot.ps1 -Launch -Arg "C:\some\folder" -Out shot.png   (or -Pid0 <pid>)
#
# Captures ONLY the Xylem window's own pixels with PrintWindow
# (PW_RENDERFULLCONTENT), so it works even when the window is behind others,
# never copies anything else on screen, and never steals keyboard focus from
# whatever the user is doing. (An earlier version copied the screen region
# and could capture other apps; don't go back to that.)
param([string]$Out, [int]$Pid0, [switch]$Launch, [string]$Arg = "C:\Program Files", [int]$Wait = 6)
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
"@
[Win]::SetProcessDPIAware() | Out-Null
$exe = Join-Path (Split-Path -Parent $PSScriptRoot) "target\release\xylem-app.exe"
if ($Launch) {
  # Start minimized-to-background is not possible for a fresh window, but we
  # never call SetForegroundWindow, so focus stays where Windows puts it.
  $p = Start-Process -FilePath $exe -ArgumentList "`"$Arg`"" -PassThru
} else {
  $p = Get-Process -Id $Pid0
}
# Wait for a real-sized main window (the first handle can be a tiny stub).
$deadline = (Get-Date).AddSeconds(30)
do {
  Start-Sleep -Milliseconds 250
  $p.Refresh()
  $h = $p.MainWindowHandle
  $r = New-Object Win+RECT
  if ($h -ne 0) { [Win]::GetWindowRect($h, [ref]$r) | Out-Null }
} while (($h -eq 0 -or ($r.R - $r.L) -lt 400) -and (Get-Date) -lt $deadline)
if ($h -eq 0) { "no window"; exit 1 }
Start-Sleep -Seconds $Wait
[Win]::GetWindowRect($h, [ref]$r) | Out-Null
$w = $r.R - $r.L; $hh = $r.B - $r.T
$bmp = New-Object System.Drawing.Bitmap $w, $hh
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
$ok = [Win]::PrintWindow($h, $hdc, 2)   # 2 = PW_RENDERFULLCONTENT (needed for WebView2)
$g.ReleaseHdc($hdc)
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
"pid=$($p.Id) size=${w}x${hh} printwindow=$ok"
