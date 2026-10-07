# Build dist\F1R3Gaze-<ver>-x64.msi from built binaries, signing the
# executables and the installer when a certificate is provided.
#   packaging\windows\package.ps1 -Version 0.1.0 -Bin target\release
# Signing env (optional): WINDOWS_CERT_PFX (base64 .pfx), WINDOWS_CERT_PASSWORD,
# WINDOWS_TIMESTAMP_URL (default http://timestamp.digicert.com).
# Requires: the WiX v4 CLI (dotnet tool install --global wix) and signtool.
param([Parameter(Mandatory)][string]$Version, [Parameter(Mandatory)][string]$Bin,
      [string]$Dist = "dist", [switch]$ZipOnly)
$ErrorActionPreference = "Stop"
if ($Version -notmatch '^[0-9]+\.[0-9]+\.[0-9]+$') {
  throw "MSI releases require a stable numeric X.Y.Z version: $Version"
}
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$workspaceVersion = $null
$inWorkspacePackage = $false
foreach ($line in Get-Content (Join-Path $here "..\..\Cargo.toml")) {
  if ($line -match '^\[workspace\.package\]') { $inWorkspacePackage = $true; continue }
  if ($line -match '^\[') { $inWorkspacePackage = $false }
  if ($inWorkspacePackage -and $line -match '^version\s*=\s*"([^"]+)"') {
    $workspaceVersion = $Matches[1]
    break
  }
}
if ($Version -ne $workspaceVersion) {
  throw "Version $Version differs from Cargo workspace version $workspaceVersion"
}
$icons = Resolve-Path "$here\..\icons"
New-Item -ItemType Directory -Force $Dist | Out-Null
$Dist = Resolve-Path $Dist
$stage = Join-Path ([IO.Path]::GetTempPath()) ("f1r3gaze-" + [Guid]::NewGuid())
New-Item -ItemType Directory $stage | Out-Null
try {
if (-not $ZipOnly -and -not (Get-Command wix -ErrorAction SilentlyContinue)) { throw "WiX CLI is required" }
foreach ($name in @("f1r3gaze.exe", "f1r3c.exe")) {
  if (-not (Test-Path "$Bin\$name")) { throw "Missing executable: $Bin\$name" }
}
Copy-Item "$Bin\f1r3gaze.exe", "$Bin\f1r3c.exe" $stage
Copy-Item "$here\..\..\..\..\LICENSE" (Join-Path $stage "LICENSE")

$signtool = $null
if ($env:WINDOWS_CERT_PFX) {
  $pfx = Join-Path $stage "cert.pfx"
  [IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:WINDOWS_CERT_PFX))
  $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" |
    Sort-Object FullName -Descending | Select-Object -First 1
  if (-not $signtool) { throw "Windows SDK signtool.exe is required for signing" }
  $ts = if ($env:WINDOWS_TIMESTAMP_URL) { $env:WINDOWS_TIMESTAMP_URL } else { "http://timestamp.digicert.com" }
  function Sign($f) {
    & $signtool.FullName sign /fd sha256 /tr $ts /td sha256 /f $pfx /p $env:WINDOWS_CERT_PASSWORD /d "F1R3Gaze" $f
    if ($LASTEXITCODE) { throw "signing $f failed" }
  }
  Sign "$stage\f1r3gaze.exe"; Sign "$stage\f1r3c.exe"
} else {
  Write-Warning "WINDOWS_CERT_PFX not set: the installer is unsigned"
}

# Portable output can also be made from a cross-built PE binary on Linux.
Compress-Archive -Force -Path "$stage\f1r3gaze.exe", "$stage\f1r3c.exe", "$stage\LICENSE" -DestinationPath (Join-Path $Dist "f1r3gaze-$Version-windows-x64.zip")
if ($ZipOnly) {
  Write-Host "built portable Windows ZIP"
  return
}

$msi = Join-Path $Dist "F1R3Gaze-$Version-x64.msi"
wix build (Join-Path $here "f1r3gaze.wxs") -arch x64 -d "Version=$Version" -d "Bin=$stage" -d "Icons=$icons" -o $msi
if ($LASTEXITCODE) { throw "wix build failed" }
if ($signtool) { Sign $msi }

Write-Host "built $msi"
} finally {
  Remove-Item -Recurse -Force $stage -ErrorAction SilentlyContinue
}
