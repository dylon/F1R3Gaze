# Compare winit's live Win32 appearance with the current user's app setting.
$ErrorActionPreference = 'Stop'
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
$light = Get-ItemPropertyValue -Path $key -Name AppsUseLightTheme -ErrorAction SilentlyContinue
$expected = if ($null -ne $light -and $light -eq 0) { 'dark' } else { 'light' }
cargo run --locked --release -p gaze-shell --features gaze-shell/os-keyring --example native_system_theme -- $expected
if ($LASTEXITCODE) { throw "native system appearance probe failed (expected $expected)" }
