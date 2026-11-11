# Usage: .\tools\ui-key.ps1 -Pid0 <pid> -Key 0x28      (0x28 = Down, 0x26 = Up, 0x0D = Enter)
#
# Posts a key press to the Xylem window's own message queue, so the real
# keyboard focus and the mouse are never touched. For UI checks where a
# tree row has to be selected (tree rows aren't reachable by ui-click).
param([int]$Pid0, [int]$Key = 0x28, [int]$Times = 1)
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class K { [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint msg, IntPtr w, IntPtr l); }
"@
$h = (Get-Process -Id $Pid0).MainWindowHandle
for ($i = 0; $i -lt $Times; $i++) {
  [K]::PostMessageW($h, 0x0100, [IntPtr]$Key, [IntPtr]1) | Out-Null          # WM_KEYDOWN
  Start-Sleep -Milliseconds 60
  [K]::PostMessageW($h, 0x0101, [IntPtr]$Key, [IntPtr]0xC0000001) | Out-Null # WM_KEYUP
  Start-Sleep -Milliseconds 120
}
"sent key $Key x$Times"
