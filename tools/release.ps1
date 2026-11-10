# Builds a Xylem release into .\dist: binaries -> (optional) signing ->
# installer -> portable zip -> SHA-256 checksums.
#
#   .\tools\release.ps1                   # everything (local test build)
#   .\tools\release.ps1 -Step build       # only build + stage the exes in dist\stage
#   .\tools\release.ps1 -Step package -StageDir <dir>
#                                          # installer + zip + checksums from exes in <dir>
#                                          # (CI uses this with the exes SignPath signed)
#
# Official releases are built and signed in GitHub Actions
# (.github/workflows/release.yml) through SignPath, so signing there doesn't
# use this script's signtool step. For a local build with your own
# certificate, set ONE of:
#   $env:XYLEM_SIGN_THUMBPRINT = "<sha1 thumbprint>"     # cert in the Windows store / token
#   $env:XYLEM_SIGN_PFX = "C:\path\cert.pfx"; $env:XYLEM_SIGN_PFX_PASSWORD = "..."
#   $env:XYLEM_SIGN_DLIB = "...\Azure.CodeSigning.Dlib.dll"; $env:XYLEM_SIGN_DLIB_METADATA = "...\metadata.json"
# Optional: $env:XYLEM_TIMESTAMP_URL (default http://timestamp.digicert.com).
# Timestamping is the only step that goes online.
param(
  [ValidateSet("all", "build", "package")][string]$Step = "all",
  [string]$StageDir,
  [switch]$SkipBuild,
  [string]$Version
)
# Native tools (cargo, signtool, makensis) are checked by exit code: in
# Windows PowerShell 5.1, "Stop" would turn their normal stderr into errors.
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

# 1. Build + stage ------------------------------------------------------------
if ($Step -in "all", "build") {
  if (-not $SkipBuild) {
    Write-Host "== Building release binaries"
    cargo build --release -p xylem -p xylem-cli
    if ($LASTEXITCODE) { throw "cargo build failed" }
  }
  $StageDir = Join-Path $dist "stage"
  Remove-Item $StageDir -Recurse -Force -ErrorAction SilentlyContinue
  New-Item -ItemType Directory -Force $StageDir | Out-Null
  foreach ($b in $bins) {
    if (-not (Test-Path "target\release\$b")) { throw "missing target\release\$b" }
    Copy-Item "target\release\$b" $StageDir
  }
  Write-Host "== Staged binaries in $StageDir"
  if ($Step -eq "build") { return }
}

# 2. Package --------------------------------------------------------------------
if (-not $StageDir) { throw "-StageDir is required for -Step package" }
$StageDir = (Resolve-Path $StageDir).Path
foreach ($b in $bins) { if (-not (Test-Path (Join-Path $StageDir $b))) { throw "missing $b in $StageDir" } }

$signed = Sign-Files ($bins | ForEach-Object { Join-Path $StageDir $_ })
Write-Host ("== Local signing: " + $(if ($signed) { "signed" } else { "skipped (no certificate configured)" }))

$makensis = @($env:MAKENSIS, "${env:ProgramFiles(x86)}\NSIS\makensis.exe", "$env:ProgramFiles\NSIS\makensis.exe") |
  Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
if (-not $makensis) { throw "NSIS not found (choco install nsis / winget install NSIS.NSIS, or set MAKENSIS)" }
Write-Host "== Building installer"
& $makensis /V2 "/DVERSION=$Version" "/DSRCDIR=$StageDir" "/DOUTDIR=$dist" installer\xylem.nsi
if ($LASTEXITCODE) { throw "makensis failed" }
$setup = Join-Path $dist "xylem-setup-$Version.exe"
if ($signed) { Sign-Files @($setup) | Out-Null }

# Portable zip (same binaries) for people who don't want an installer.
$zip = Join-Path $dist "xylem-$Version-portable.zip"
Remove-Item $zip -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $StageDir "xylem.exe"), (Join-Path $StageDir "xylem-cli.exe"), README.md, LICENSE-MIT, LICENSE-APACHE -DestinationPath $zip

# 3. Checksums ------------------------------------------------------------------
$sums = Join-Path $dist "SHA256SUMS.txt"
$files = @($setup, $zip) + ($bins | ForEach-Object { Join-Path $StageDir $_ })
$files | ForEach-Object {
  "{0}  {1}" -f (Get-FileHash $_ -Algorithm SHA256).Hash.ToLower(), (Split-Path $_ -Leaf)
} | Set-Content -Encoding ascii $sums
Write-Host "== Done"
Get-ChildItem $dist -File | ForEach-Object { "{0}  {1:N2} MB" -f $_.Name, ($_.Length / 1MB) }
Get-Content $sums
