# Builds a Xylem release: binaries -> (optional) code signing -> installer
# -> SHA-256 checksums, all into .\dist.
#
#   .\tools\release.ps1                       # unsigned build (for testing)
#   .\tools\release.ps1 -SkipBuild            # reuse target\release
#
# Signing is automatic once a certificate is configured. Set ONE of:
#   $env:XYLEM_SIGN_THUMBPRINT = "<sha1 thumbprint>"   # cert in your Windows cert store
#                                                      # (also works for a USB token / cloud HSM)
#   $env:XYLEM_SIGN_PFX = "C:\path\cert.pfx"; $env:XYLEM_SIGN_PFX_PASSWORD = "..."
#   $env:XYLEM_SIGN_DLIB = "C:\...\Azure.CodeSigning.Dlib.dll"
#   $env:XYLEM_SIGN_DLIB_METADATA = "C:\...\metadata.json"   # Azure Trusted Signing
# Optional: $env:XYLEM_TIMESTAMP_URL (default http://timestamp.digicert.com).
# Timestamping needs the network; it keeps signatures valid after the
# certificate expires. It is the only step here that goes online.
param([switch]$SkipBuild, [string]$Version)
# Native tools (cargo, signtool, ISCC) are checked by exit code: in Windows
# PowerShell 5.1, "Stop" would turn their normal stderr output into errors.
$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$env:Path += ";$env:USERPROFILE\.cargo\bin"

if (-not $Version) {
  $Version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches[0].Groups[1].Value
}
$dist = Join-Path $root "dist"
New-Item -ItemType Directory -Force $dist | Out-Null
$bins = "xylem.exe", "xylem-cli.exe"

# 1. Build -------------------------------------------------------------------
if (-not $SkipBuild) {
  Write-Host "== Building release binaries"
  cargo build --release -p xylem -p xylem-cli
  if ($LASTEXITCODE) { throw "cargo build failed" }
}
foreach ($b in $bins) { if (-not (Test-Path "target\release\$b")) { throw "missing target\release\$b" } }

# 2. Sign (optional) -----------------------------------------------------------
function Find-SignTool {
  $kits = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
  $st = Get-ChildItem $kits -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match '\\x64\\' } | Sort-Object FullName -Descending | Select-Object -First 1
  if (-not $st) { throw "signtool.exe not found (install the Windows SDK)" }
  $st.FullName
}
function Sign-Files([string[]]$files) {
  $ts = if ($env:XYLEM_TIMESTAMP_URL) { $env:XYLEM_TIMESTAMP_URL } else { "http://timestamp.digicert.com" }
  $common = @("sign", "/fd", "SHA256", "/tr", $ts, "/td", "SHA256", "/d", "Xylem")
  if ($env:XYLEM_SIGN_THUMBPRINT) { $how = @("/sha1", $env:XYLEM_SIGN_THUMBPRINT) }
  elseif ($env:XYLEM_SIGN_PFX) { $how = @("/f", $env:XYLEM_SIGN_PFX, "/p", $env:XYLEM_SIGN_PFX_PASSWORD) }
  elseif ($env:XYLEM_SIGN_DLIB) { $how = @("/dlib", $env:XYLEM_SIGN_DLIB, "/dmdf", $env:XYLEM_SIGN_DLIB_METADATA) }
  else { return $false }
  $st = Find-SignTool
  & $st @common @how @files
  if ($LASTEXITCODE) { throw "signtool sign failed" }
  & $st verify /pa /q @files
  if ($LASTEXITCODE) { throw "signature verification failed" }
  return $true
}

$stage = Join-Path $dist "stage"
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $stage | Out-Null
foreach ($b in $bins) { Copy-Item "target\release\$b" $stage }
$signed = Sign-Files ($bins | ForEach-Object { Join-Path $stage $_ })
Write-Host ("== Binaries " + $(if ($signed) { "signed" } else { "NOT signed (no certificate configured)" }))

# 3. Installer ---------------------------------------------------------------
$iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe") |
  Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { throw "Inno Setup 6 not found (winget install JRSoftware.InnoSetup)" }
Write-Host "== Building installer"
& $iscc /Q "/DAppVersion=$Version" "/DSourceDir=$stage" "/DOutDir=$dist" installer\xylem.iss
if ($LASTEXITCODE) { throw "ISCC failed" }
$setup = Join-Path $dist "xylem-setup-$Version.exe"
if ($signed) { Sign-Files @($setup) | Out-Null }

# Portable zip (same binaries) for people who don't want an installer.
$zip = Join-Path $dist "xylem-$Version-portable.zip"
Remove-Item $zip -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $stage "xylem.exe"), (Join-Path $stage "xylem-cli.exe"), README.md, LICENSE-MIT, LICENSE-APACHE -DestinationPath $zip

# 4. Checksums ---------------------------------------------------------------
$sums = Join-Path $dist "SHA256SUMS.txt"
$files = @($setup, $zip) + ($bins | ForEach-Object { Join-Path $stage $_ })
$files | ForEach-Object {
  "{0}  {1}" -f (Get-FileHash $_ -Algorithm SHA256).Hash.ToLower(), (Split-Path $_ -Leaf)
} | Set-Content -Encoding ascii $sums
Write-Host "== Done"
Get-ChildItem $dist -File | Select-Object Name, @{n = "MB"; e = { [math]::Round($_.Length / 1MB, 2) } } | Format-Table -AutoSize
Get-Content $sums
