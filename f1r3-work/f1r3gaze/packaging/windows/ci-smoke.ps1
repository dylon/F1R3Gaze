# Install, launch and remove a WiX MSI; launch the portable ZIP.
param([Parameter(Mandatory)][string]$Version, [string]$Dist = 'dist')
$ErrorActionPreference = 'Stop'
$distPath = (Resolve-Path $Dist).Path
$msi = Join-Path $distPath "F1R3Gaze-$Version-x64.msi"
$zip = Join-Path $distPath "f1r3gaze-$Version-windows-x64.zip"
$temp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$installLog = Join-Path $temp 'f1r3gaze-msi-install.log'
$removeLog = Join-Path $temp 'f1r3gaze-msi-remove.log'

try {
  foreach ($artifact in @($msi, $zip)) {
    if (-not (Test-Path $artifact) -or (Get-Item $artifact).Length -eq 0) { throw "Missing or empty $artifact" }
  }
  $portable = Join-Path $temp 'f1r3gaze-portable'
  Expand-Archive -LiteralPath $zip -DestinationPath $portable -Force
  foreach ($name in @('f1r3gaze.exe', 'f1r3c.exe', 'LICENSE')) {
    if (-not (Test-Path (Join-Path $portable $name))) { throw "Missing $name in ZIP" }
  }
  $got = & (Join-Path $portable 'f1r3gaze.exe') --version
  if ($LASTEXITCODE -or $got -ne "f1r3gaze $Version") { throw "Portable executable returned: $got" }
  $got = & (Join-Path $portable 'f1r3c.exe') --version
  if ($LASTEXITCODE -or $got -ne "f1r3c $Version") { throw "Portable compiler returned: $got" }

  $install = Start-Process msiexec.exe -ArgumentList @('/i', "`"$msi`"", '/qn', '/norestart', '/L*v', "`"$installLog`"") -Wait -PassThru
  if ($install.ExitCode -notin @(0, 3010)) { throw "MSI install failed: $($install.ExitCode)" }
  $installed = Join-Path $env:ProgramFiles 'F1R3Gaze'
  foreach ($name in @('f1r3gaze.exe', 'f1r3c.exe', 'LICENSE')) {
    if (-not (Test-Path (Join-Path $installed $name))) { throw "Missing installed $name" }
  }
  $got = & (Join-Path $installed 'f1r3gaze.exe') --version
  if ($LASTEXITCODE -or $got -ne "f1r3gaze $Version") { throw "Installed executable returned: $got" }
  $got = & (Join-Path $installed 'f1r3c.exe') --version
  if ($LASTEXITCODE -or $got -ne "f1r3c $Version") { throw "Installed compiler returned: $got" }
  foreach ($scheme in @('f1r3', 'f1r3h')) {
    if (-not (Test-Path "Registry::HKEY_LOCAL_MACHINE\Software\Classes\$scheme")) {
      throw "Missing $scheme URL registration"
    }
  }

  $remove = Start-Process msiexec.exe -ArgumentList @('/x', "`"$msi`"", '/qn', '/norestart', '/L*v', "`"$removeLog`"") -Wait -PassThru
  if ($remove.ExitCode -notin @(0, 3010)) { throw "MSI removal failed: $($remove.ExitCode)" }
  if (Test-Path (Join-Path $installed 'f1r3gaze.exe')) { throw 'MSI left the executable installed' }
} catch {
  Write-Host "::error::Windows installer smoke failure: $($_.Exception.Message)"
  if (Test-Path $installLog) { Get-Content -Tail 80 $installLog }
  if (Test-Path $removeLog) { Get-Content -Tail 80 $removeLog }
  throw
}
