# Usage: .\tools\ui-screenshot.ps1 -Launch -Arg "C:\some\folder" -Out shot.png   (or -Pid0 <pid>)
param([string]$Out, [int]$Pid0, [switch]$Launch, [string]$Arg = "C:\Program Files", [int]$Wait = 6)
# Captures only the Drishti app window (not the rest of the screen).
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int cmd);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
"@
[Win]::SetProcessDPIAware() | Out-Null
$exe = Join-Path (Split-Path -Parent $PSScriptRoot) "target\release\drishti-app.exe"
if ($Launch) {
  $p = Start-Process -FilePath $exe -ArgumentList "`"$Arg`"" -PassThru
} else {
  $p = Get-Process -Id $Pid0
}
$deadline = (Get-Date).AddSeconds(20)
while ($p.MainWindowHandle -eq 0 -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200; $p.Refresh() }
if ($p.MainWindowHandle -eq 0) { "no window"; exit 1 }
$h = $p.MainWindowHandle
[Win]::ShowWindow($h, 9) | Out-Null
[Win]::SetForegroundWindow($h) | Out-Null
Start-Sleep -Seconds $Wait
$r = New-Object Win+RECT
[Win]::GetWindowRect($h, [ref]$r) | Out-Null
$w = $r.R - $r.L; $hh = $r.B - $r.T
$bmp = New-Object System.Drawing.Bitmap $w, $hh
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.L, $r.T, 0, 0, $bmp.Size)
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
"pid=$($p.Id) size=${w}x${hh}"
