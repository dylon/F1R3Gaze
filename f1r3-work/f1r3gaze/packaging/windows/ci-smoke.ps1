# Install an older WiX MSI, upgrade to this release, remove it, and launch ZIP.
param([Parameter(Mandatory)][string]$Version, [string]$Dist = 'dist')
$ErrorActionPreference = 'Stop'
$distPath = (Resolve-Path $Dist).Path
$msi = Join-Path $distPath "F1R3Gaze-$Version-x64.msi"
$zip = Join-Path $distPath "f1r3gaze-$Version-windows-x64.zip"
$temp = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
$oldInstallLog = Join-Path $temp 'f1r3gaze-msi-old-install.log'
$upgradeLog = Join-Path $temp 'f1r3gaze-msi-upgrade.log'
$removeLog = Join-Path $temp 'f1r3gaze-msi-remove.log'
$scratch = Join-Path $temp ("f1r3gaze-smoke-" + [guid]::NewGuid().ToString('N'))
$portable = Join-Path $scratch 'portable'
$oldMsi = Join-Path $scratch 'older.msi'
$installed = Join-Path $env:ProgramFiles 'F1R3Gaze'

function RegisteredProducts {
  @(Get-ItemProperty 'Registry::HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue |
    Where-Object { $_.DisplayName -eq 'F1R3Gaze' })
}

function Assert-RegisteredVersion([string]$Expected) {
  $products = @(RegisteredProducts)
  if ($products.Count -ne 1 -or $products[0].DisplayVersion -ne $Expected) {
    throw "Expected one F1R3Gaze $Expected registration, found: $($products.DisplayVersion -join ', ')"
  }
}

function Install-Msi([string]$Path, [string]$Log) {
  $result = Start-Process msiexec.exe -ArgumentList @('/i', "`"$Path`"", '/qn', '/norestart', '/L*v', "`"$Log`"") -Wait -PassThru
  if ($result.ExitCode -notin @(0, 3010)) { throw "MSI install failed ($Path): $($result.ExitCode)" }
}

try {
  foreach ($artifact in @($msi, $zip)) {
    if (-not (Test-Path $artifact) -or (Get-Item $artifact).Length -eq 0) { throw "Missing or empty $artifact" }
  }
  New-Item -ItemType Directory -Path $scratch | Out-Null
  Expand-Archive -LiteralPath $zip -DestinationPath $portable -Force
  foreach ($name in @('f1r3gaze.exe', 'f1r3c.exe', 'LICENSE')) {
    if (-not (Test-Path (Join-Path $portable $name))) { throw "Missing $name in ZIP" }
  }
  $got = & (Join-Path $portable 'f1r3gaze.exe') --version
  if ($LASTEXITCODE -or $got -ne "f1r3gaze $Version") { throw "Portable executable returned: $got" }
  $got = & (Join-Path $portable 'f1r3c.exe') --version
  if ($LASTEXITCODE -or $got -ne "f1r3c $Version") { throw "Portable compiler returned: $got" }

  $parts = [int[]]$Version.Split('.')
  if ($parts[2] -gt 0) { $parts[2]-- }
  elseif ($parts[1] -gt 0) { $parts[1]--; $parts[2] = 1 }
  elseif ($parts[0] -gt 0) { $parts[0]--; $parts[1] = 1; $parts[2] = 1 }
  else { throw 'MSI 0.0.0 has no earlier version for upgrade testing' }
  $oldVersion = $parts -join '.'
  $icons = (Resolve-Path (Join-Path $PSScriptRoot '..\icons')).Path
  & wix build (Join-Path $PSScriptRoot 'f1r3gaze.wxs') -arch x64 -d "Version=$oldVersion" -d "Bin=$portable" -d "Icons=$icons" -o $oldMsi
  if ($LASTEXITCODE -or -not (Test-Path $oldMsi)) { throw "WiX could not build the $oldVersion upgrade fixture" }
  Install-Msi $oldMsi $oldInstallLog
  Assert-RegisteredVersion $oldVersion

  Install-Msi $msi $upgradeLog
  Assert-RegisteredVersion $Version
  foreach ($name in @('f1r3gaze.exe', 'f1r3c.exe', 'LICENSE')) {
    if (-not (Test-Path (Join-Path $installed $name))) { throw "Missing installed $name" }
  }
  $got = & (Join-Path $installed 'f1r3gaze.exe') --version
  if ($LASTEXITCODE -or $got -ne "f1r3gaze $Version") { throw "Installed executable returned: $got" }
  $got = & (Join-Path $installed 'f1r3c.exe') --version
  if ($LASTEXITCODE -or $got -ne "f1r3c $Version") { throw "Installed compiler returned: $got" }
  foreach ($scheme in @('f1r3', 'f1r3h')) {
    $key = Get-Item "Registry::HKEY_LOCAL_MACHINE\Software\Classes\$scheme\shell\open\command" -ErrorAction Stop
    $expectedCommand = '"{0}" "%1"' -f (Join-Path $installed 'f1r3gaze.exe')
    if ($key.GetValue('') -ne $expectedCommand) { throw "$scheme URL command is incorrect: $($key.GetValue(''))" }
  }

  $remove = Start-Process msiexec.exe -ArgumentList @('/x', "`"$msi`"", '/qn', '/norestart', '/L*v', "`"$removeLog`"") -Wait -PassThru
  if ($remove.ExitCode -notin @(0, 3010)) { throw "MSI removal failed: $($remove.ExitCode)" }
  if (Test-Path (Join-Path $installed 'f1r3gaze.exe')) { throw 'MSI left the executable installed' }
  if (@(RegisteredProducts).Count -ne 0) { throw 'MSI left a product registration installed' }
  foreach ($scheme in @('f1r3', 'f1r3h')) {
    if (Test-Path "Registry::HKEY_LOCAL_MACHINE\Software\Classes\$scheme") {
      throw "MSI left the $scheme URL registration installed"
    }
  }
} catch {
  Write-Host "::error::Windows installer smoke failure: $($_.Exception.Message)"
  if (Test-Path $oldInstallLog) { Get-Content -Tail 80 $oldInstallLog }
  if (Test-Path $upgradeLog) { Get-Content -Tail 80 $upgradeLog }
  if (Test-Path $removeLog) { Get-Content -Tail 80 $removeLog }
  throw
} finally {
  Remove-Item -Recurse -Force $scratch -ErrorAction SilentlyContinue
}
