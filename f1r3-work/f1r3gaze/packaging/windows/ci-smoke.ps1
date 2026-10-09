# Install an older WiX MSI, upgrade, exercise URL handoff, remove, and launch ZIP.
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
$startMenu = Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'F1R3Gaze.lnk'
$windowProcess = $null
$session = $null
$windowStdout = Join-Path $temp 'f1r3gaze-msi-window-stdout.log'
$windowStderr = Join-Path $temp 'f1r3gaze-msi-window-stderr.log'

function RegisteredProducts {
  @(Get-ItemProperty 'Registry::HKEY_LOCAL_MACHINE\Software\Microsoft\Windows\CurrentVersion\Uninstall\*' -ErrorAction SilentlyContinue |
    Where-Object { $_.DisplayName -eq 'F1R3Gaze' })
}

function Assert-RegisteredVersion([string]$Expected) {
  $products = @(RegisteredProducts)
  if ($products.Count -ne 1 -or $products[0].DisplayVersion -ne $Expected) {
    throw "Expected one F1R3Gaze $Expected registration, found: $($products.DisplayVersion -join ', ')"
  }
  if ($products[0].Publisher -ne 'F1R3FLY.io') {
    throw "F1R3Gaze Add/Remove Programs publisher is incorrect: $($products[0].Publisher)"
  }
}

function Wait-SmokeProcess([System.Diagnostics.Process]$Process, [string]$Operation, [int]$Seconds) {
  if (-not $Process.WaitForExit($Seconds * 1000)) {
    Stop-Process -Id $Process.Id -Force -ErrorAction SilentlyContinue
    [void]$Process.WaitForExit(5000)
    throw "$Operation exceeded $Seconds seconds"
  }
  return $Process.ExitCode
}

function Install-Msi([string]$Path, [string]$Log) {
  Write-Host "Installing $Path (log: $Log)"
  $process = Start-Process msiexec.exe -ArgumentList @('/i', "`"$Path`"", '/qn', '/norestart', '/L*v', "`"$Log`"") -PassThru
  $exitCode = Wait-SmokeProcess $process "MSI install $Path" 180
  if ($exitCode -notin @(0, 3010)) { throw "MSI install failed ($Path): $exitCode" }
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
  if (-not (Test-Path $startMenu)) { throw "MSI did not create its Start menu shortcut: $startMenu" }
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

  $handoffProfile = Join-Path $scratch 'url-profile'
  $windowProcess = Start-Process -FilePath (Join-Path $installed 'f1r3gaze.exe') -ArgumentList @('--profile', "`"$handoffProfile`"") -PassThru -RedirectStandardOutput $windowStdout -RedirectStandardError $windowStderr
  $endpoint = Join-Path $handoffProfile 'runtime\url-handoff.json'
  $ready = $false
  for ($attempt = 0; $attempt -lt 100; $attempt++) {
    if (Test-Path $endpoint) { $ready = $true; break }
    if ($windowProcess.HasExited) { throw "Installed browser exited before opening its URL handoff endpoint: $($windowProcess.ExitCode)" }
    Start-Sleep -Milliseconds 100
  }
  if (-not $ready) { throw 'Installed browser did not open its URL handoff endpoint' }
  $link = 'f1r3://abcd/ci-smoke'
  Write-Host 'Delivering URL to the running browser'
  $second = Start-Process -FilePath (Join-Path $installed 'f1r3gaze.exe') -ArgumentList @('--profile', "`"$handoffProfile`"", $link) -PassThru
  $handoffExitCode = Wait-SmokeProcess $second 'Running-instance URL handoff' 30
  if ($handoffExitCode -ne 0) { throw "Running-instance URL handoff failed: $handoffExitCode" }
  $session = Join-Path $handoffProfile 'state\session.json'
  $delivered = $false
  for ($attempt = 0; $attempt -lt 300; $attempt++) {
    if ((Test-Path $session) -and ((Get-Content -Raw $session) -match [regex]::Escape($link))) {
      $delivered = $true
      break
    }
    if ($windowProcess.HasExited) { throw "Installed browser exited before saving the delivered URL: $($windowProcess.ExitCode)" }
    Start-Sleep -Milliseconds 100
  }
  if (-not $delivered) { throw 'Installed browser did not save the delivered URL in its session' }
  Stop-Process -Id $windowProcess.Id -Force
  if (-not $windowProcess.WaitForExit(5000)) { throw 'Installed browser did not stop after termination' }
  $windowProcess = $null

  Write-Host "Removing $msi (log: $removeLog)"
  $remove = Start-Process msiexec.exe -ArgumentList @('/x', "`"$msi`"", '/qn', '/norestart', '/L*v', "`"$removeLog`"") -PassThru
  $removeExitCode = Wait-SmokeProcess $remove 'MSI removal' 180
  if ($removeExitCode -notin @(0, 3010)) { throw "MSI removal failed: $removeExitCode" }
  if (Test-Path (Join-Path $installed 'f1r3gaze.exe')) { throw 'MSI left the executable installed' }
  if (Test-Path $startMenu) { throw 'MSI left the Start menu shortcut installed' }
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
  if (Test-Path $windowStderr) { Get-Content -Tail 80 $windowStderr }
  if ($session -and (Test-Path $session)) { Write-Host "Session snapshot: $(Get-Content -Raw $session)" }
  throw
} finally {
  if ($windowProcess -and -not $windowProcess.HasExited) {
    Stop-Process -Id $windowProcess.Id -Force -ErrorAction SilentlyContinue
    [void]$windowProcess.WaitForExit(5000)
  }
  Remove-Item -Recurse -Force $scratch -ErrorAction SilentlyContinue
}
