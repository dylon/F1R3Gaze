#!/usr/bin/env bash
# Build and exercise an isolated App Sandbox package with an ad hoc identity.
# The Developer ID DMG/PKG and Homebrew cask stay intact.
set -euo pipefail
version=${1:?version required}
dist=${2:-dist}
dist=$(cd "$dist" && pwd)
dmg="$dist/F1R3Gaze-$version-macos-universal.dmg"
[[ -s "$dmg" ]] || { echo "macOS DMG missing or empty: $dmg" >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/f1r3gaze-store-sandbox.XXXXXX")
server_pid=
dialog_pid=
original_attached=0
original_mount="$work/original-mount"
installed=0
installed_app=/Applications/F1R3Gaze.app
bundle_id="io.f1r3fly.f1r3gaze.sandboxci.run$(uuidgen | tr '[:upper:]' '[:lower:]' | tr -d '-')"
container="$HOME/Library/Containers/$bundle_id"
cleanup() {
  local code=$?
  if [[ -n "$server_pid" ]]; then kill "$server_pid" 2>/dev/null || true; fi
  if [[ -n "$dialog_pid" ]]; then kill "$dialog_pid" 2>/dev/null || true; fi
  if [[ "$original_attached" == 1 ]]; then hdiutil detach "$original_mount" >/dev/null || true; fi
  if [[ "$installed" == 1 ]]; then
    sudo rm -rf "$installed_app"
    sudo pkgutil --forget "$bundle_id" >/dev/null 2>&1 || true
  fi
  sudo rm -rf "$work"
  rm -rf "$container"
  exit "$code"
}
trap cleanup EXIT
[[ ! -e "$container" ]] || { echo "test container already exists: $container" >&2; exit 2; }
[[ ! -e "$installed_app" && ! -L "$installed_app" ]] || {
  echo "test installation would replace an existing app: $installed_app" >&2
  exit 2
}

mkdir "$original_mount"
hdiutil attach -nobrowse -readonly -mountpoint "$original_mount" "$dmg" >/dev/null
original_attached=1
printf 'Nil\n' > "$work/unsandboxed-example.rho"
original_compiler="$original_mount/F1R3Gaze.app/Contents/MacOS/f1r3c"
original_gaze="$original_mount/F1R3Gaze.app/Contents/MacOS/f1r3gaze"
"$original_compiler" compile "$work/unsandboxed-example.rho" -o "$work/unsandboxed-example.knf"
[[ -s "$work/unsandboxed-example.knf" ]] || { echo 'DMG f1r3c did not compile a source file' >&2; exit 1; }
"$original_compiler" inspect "$work/unsandboxed-example.knf" | grep -Fxq 'Nil'
old_home="$work/old-home"
mkdir "$old_home"
old_profile="$old_home/Library/Application Support/io.f1r3fly.f1r3gaze"
old_paths=$(HOME="$old_home" "$original_gaze" paths)
[[ "$old_paths" == *"$old_profile/config"* ]] || {
  echo 'unsandboxed DMG did not select the isolated default macOS profile' >&2
  printf '%s\n' "$old_paths" >&2
  exit 1
}
HOME="$old_home" "$original_gaze" --headless gaze://newtab --timeout 5 > "$work/old-profile.log" 2>&1
[[ -s "$old_profile/data/layout.json" ]] || {
  echo 'unsandboxed DMG did not create an existing profile' >&2
  cat "$work/old-profile.log" >&2
  exit 1
}
printf 'created by the unsandboxed DMG\n' > "$old_profile/config/import-sentinel"
hdiutil detach "$original_mount" >/dev/null
original_attached=0

bash "$here/store-package.sh" "$version" "$dist" "$work/output" "$bundle_id"
app="$work/output/F1R3Gaze.app"
pkg="$work/output/F1R3Gaze-$version-macos-app-store.pkg"
[[ -s "$pkg" ]] || { echo 'Store package is missing or empty' >&2; exit 1; }
codesign --verify --deep --strict --verbose=2 "$app"
codesign -d --entitlements :- "$app" > "$work/app-entitlements.plist" 2>/dev/null
codesign -d --entitlements :- "$app/Contents/MacOS/f1r3c" > "$work/helper-entitlements.plist" 2>/dev/null
python3 - "$work/app-entitlements.plist" "$work/helper-entitlements.plist" <<'PY'
import plistlib
import sys

with open(sys.argv[1], "rb") as stream:
    app = plistlib.load(stream)
with open(sys.argv[2], "rb") as stream:
    helper = plistlib.load(stream)
assert app == {
    "com.apple.security.app-sandbox": True,
    "com.apple.security.network.client": True,
    "com.apple.security.files.user-selected.read-write": True,
}, app
assert helper == {
    "com.apple.security.app-sandbox": True,
    "com.apple.security.inherit": True,
}, helper
PY

exe="$app/Contents/MacOS/f1r3gaze"
[[ "$("$exe" --version)" == "f1r3gaze $version" ]]
[[ "$("$app/Contents/MacOS/f1r3c" --version)" == "f1r3c $version" ]]
pkgutil --payload-files "$pkg" > "$work/pkg-payload"
grep -Fq 'MacOS/f1r3gaze' "$work/pkg-payload"
grep -Fq 'MacOS/f1r3c' "$work/pkg-payload"
paths=$("$exe" paths)
support="$container/Data/Library/Application Support/io.f1r3fly.f1r3gaze"
[[ "$paths" == *"$support/config"* ]] || {
  echo "sandboxed Gaze did not locate its profile inside the container:" >&2
  printf '%s\n' "$paths" >&2
  exit 1
}

# Drive the real first-launch native dialogs on the GitHub macOS desktop. A
# profile made by the unsandboxed DMG lives outside the Store container; only
# the folder picker can grant this sandboxed process access to it.
open --stderr "$work/dialog.log" -a "$app"
for ((attempt = 0; attempt < 100; attempt++)); do
  dialog_pid=$(pgrep -f "$exe" | head -n 1 || true)
  [[ -n "$dialog_pid" ]] && break
  sleep 0.2
done
[[ -n "$dialog_pid" ]] || { echo 'Launch Services did not start the Store app' >&2; exit 1; }
if ! osascript - "$dialog_pid" "$old_profile" <<'APPLESCRIPT'
on run arguments
  set appPid to item 1 of arguments as integer
  set sourcePath to item 2 of arguments
  tell application "System Events"
    set appProcess to missing value
    repeat 100 times
      try
        set appProcess to first process whose unix id is appPid
        exit repeat
      end try
      delay 0.2
    end repeat
    if appProcess is missing value then error "sandboxed app process did not appear"
    set frontmost of appProcess to true
    -- The first-launch AppKit alert belongs to this process, so require its
    -- explicit Yes button before operating the native folder panel.
    set answered to false
    repeat 150 times
      try
        if exists button "Yes" of window 1 of appProcess then
          click button "Yes" of window 1 of appProcess
          set answered to true
          exit repeat
        else if exists button "Yes" of sheet 1 of window 1 of appProcess then
          click button "Yes" of sheet 1 of window 1 of appProcess
          set answered to true
          exit repeat
        end if
      end try
      delay 0.2
    end repeat
    if not answered then
      set openWindows to name of every window of appProcess
      error "first-launch AppKit alert did not show a Yes button; windows: " & (openWindows as text)
    end if
    delay 1
    keystroke "g" using {command down, shift down}
    delay 0.5
    keystroke sourcePath
    key code 36
    delay 0.8
    key code 36
  end tell
end run
APPLESCRIPT
then
  cat "$work/dialog.log" >&2
  echo 'could not select the existing profile through the native folder panel' >&2
  exit 1
fi
imported=0
for ((attempt = 0; attempt < 200; attempt++)); do
  if [[ -s "$support/config/import-sentinel" ]]; then imported=1; break; fi
  kill -0 "$dialog_pid" 2>/dev/null || break
  sleep 0.2
done
if [[ "$imported" != 1 ]]; then
  cat "$work/dialog.log" >&2
  echo 'native folder selection did not import the unsandboxed profile' >&2
  exit 1
fi
cmp "$old_profile/config/import-sentinel" "$support/config/import-sentinel"
[[ -s "$old_profile/data/layout.json" ]] || { echo 'native import removed its source' >&2; exit 1; }
kill "$dialog_pid" 2>/dev/null || true
for ((attempt = 0; attempt < 50; attempt++)); do
  kill -0 "$dialog_pid" 2>/dev/null || break
  sleep 0.1
done
if kill -0 "$dialog_pid" 2>/dev/null; then kill -KILL "$dialog_pid" 2>/dev/null || true; fi
dialog_pid=
rm -rf "$support"

# Import a complete, user-selected profile before the default Store profile
# opens. The source lives inside the test container so the ad hoc sandbox can
# read it without an interactive Powerbox grant; the customer path uses the
# first-launch native folder panel for that grant.
import_source="$container/Data/Documents/import-source"
import_address=$("$exe" --profile "$import_source" wallet new 'CI imported profile')
printf 'config survives import\n' > "$import_source/config/import-sentinel"
printf 'state survives import\n' > "$import_source/state/import-sentinel"
"$exe" profile import "$import_source"
for relative in config/import-sentinel state/import-sentinel data/layout.json data/wallet/wallets.tsv; do
  cmp "$import_source/$relative" "$support/$relative" || {
    echo "Store profile import changed or omitted $relative" >&2
    exit 1
  }
done
"$exe" wallet list | grep -Fq "$import_address"
if "$exe" profile import "$import_source" > "$work/import-repeat.log" 2>&1; then
  echo 'Store profile import replaced an existing container profile' >&2
  exit 1
fi
[[ -s "$import_source/data/layout.json" ]] || {
  echo 'Store profile import removed its source profile' >&2
  exit 1
}

compiler_dir="$container/Data/Documents/compiler-smoke"
mkdir -p "$compiler_dir"
printf 'Nil\n' > "$compiler_dir/example.rho"
if ! "$exe" compiler compile "$compiler_dir/example.rho" \
    -o "$compiler_dir/example.knf" > "$work/compiler.log" 2>&1; then
  cat "$work/compiler.log" >&2
  echo 'sandboxed Gaze failed to launch its compiler to compile a source file' >&2
  exit 1
fi
[[ -s "$compiler_dir/example.knf" ]] || { echo 'sandboxed f1r3c did not create its output' >&2; exit 1; }
"$exe" compiler inspect "$compiler_dir/example.knf" | grep -Fxq 'Nil'
python3 - "$exe" "$work/headless.log" gaze://newtab <<'PY'
import subprocess
import sys

with open(sys.argv[2], "wb") as output:
    subprocess.run(
        [sys.argv[1], "--headless", sys.argv[3], "--timeout", "5"],
        stdout=output,
        stderr=subprocess.STDOUT,
        check=True,
        timeout=60,
    )
PY
[[ -s "$support/data/layout.json" ]] || {
  echo "sandboxed Gaze did not create its profile marker in the container" >&2
  cat "$work/headless.log" >&2
  exit 1
}
grep -Fq '<h1>F1R3Gaze</h1>' "$work/headless.log"

# A portable profile remains usable when its root is inside the app's
# container. Exercise the actual sandboxed binary, not just layout tests.
portable="$container/Data/Documents/portable-profile"
portable_paths=$("$exe" --profile "$portable" paths)
for class in config data state cache runtime; do
  grep -Fxq "$class"$'\t'"$portable/$class" <<< "$portable_paths" || {
    echo "sandboxed --profile placed $class outside the selected root" >&2
    printf '%s\n' "$portable_paths" >&2
    exit 1
  }
done
python3 - "$exe" "$portable" "$work/portable.log" <<'PY'
import subprocess
import sys

with open(sys.argv[3], "wb") as output:
    subprocess.run(
        [sys.argv[1], "--profile", sys.argv[2], "--headless", "gaze://newtab", "--timeout", "5"],
        stdout=output,
        stderr=subprocess.STDOUT,
        check=True,
        timeout=60,
    )
PY
[[ -s "$portable/data/layout.json" ]] || {
  echo "sandboxed --profile did not create its profile marker" >&2
  cat "$work/portable.log" >&2
  exit 1
}
grep -Fq '<h1>F1R3Gaze</h1>' "$work/portable.log"

# The Store app must be able to round-trip a wallet through a file it owns.
# Keep the private key out of CI logs and remove it before the container cleanup.
wallet_address=$("$exe" --profile "$portable" wallet new 'CI sandbox')
[[ -n "$wallet_address" ]] || { echo 'sandboxed wallet creation returned no address' >&2; exit 1; }
wallet_file="$portable/wallet-export.key"
"$exe" --profile "$portable" wallet export "$wallet_address" "$wallet_file" > "$work/wallet-export.log"
[[ -s "$wallet_file" ]] || { echo 'sandboxed wallet export did not write a key file' >&2; exit 1; }
"$exe" --profile "$portable" wallet remove "$wallet_address"
imported_address=$("$exe" --profile "$portable" wallet import "$wallet_file" 'CI imported')
[[ "$imported_address" == "$wallet_address" ]] || {
  echo 'sandboxed wallet import did not recover the exported address' >&2
  exit 1
}
"$exe" --profile "$portable" wallet list | grep -Fq "$wallet_address"
"$exe" --profile "$portable" wallet remove "$wallet_address"
rm -f "$wallet_file"

mkdir "$work/site"
printf '<html><head><title>CI sandbox page</title></head><body><h1>network-ready</h1></body></html>\n' > "$work/site/index.html"
python3 -u "$here/../linux/serve-smoke-page.py" "$work/site" "$work/port" > "$work/server.log" 2>&1 &
server_pid=$!
for ((attempt = 0; attempt < 300; attempt++)); do
  [[ -s "$work/port" ]] && break
  if ! kill -0 "$server_pid" 2>/dev/null; then
    echo 'sandbox smoke HTTP server exited' >&2
    cat "$work/server.log" >&2
    exit 1
  fi
  sleep 0.2
done
if [[ ! -s "$work/port" ]]; then
  echo 'sandbox smoke HTTP server did not start within 60 seconds' >&2
  cat "$work/server.log" >&2
  exit 1
fi
python3 - "$exe" "$work/network.log" "http://127.0.0.1:$(cat "$work/port")/" <<'PY'
import subprocess
import sys

with open(sys.argv[2], "wb") as output:
    subprocess.run(
        [sys.argv[1], "--headless", sys.argv[3], "--timeout", "5"],
        stdout=output,
        stderr=subprocess.STDOUT,
        check=True,
        timeout=60,
    )
PY
grep -Fq '<h1>network-ready</h1>' "$work/network.log"
# Installer relocates a bundle when it finds the same identifier elsewhere.
# Remove the inspected build copy so this exercises a fresh /Applications install.
rm -rf "$app"
installed=1
sudo installer -pkg "$pkg" -target /
[[ -x "$installed_app/Contents/MacOS/f1r3gaze" ]] || { echo 'Store package did not install Gaze' >&2; exit 1; }
[[ -x "$installed_app/Contents/MacOS/f1r3c" ]] || { echo 'Store package did not install f1r3c' >&2; exit 1; }
[[ "$("$installed_app/Contents/MacOS/f1r3gaze" --version)" == "f1r3gaze $version" ]]
"$installed_app/Contents/MacOS/f1r3gaze" compiler inspect "$compiler_dir/example.knf" | grep -Fxq 'Nil'
python3 - "$installed_app/Contents/MacOS/f1r3gaze" "$work/installed.log" <<'PY'
import subprocess
import sys

with open(sys.argv[2], "wb") as output:
    subprocess.run(
        [sys.argv[1], "--headless", "gaze://newtab", "--timeout", "5"],
        stdout=output,
        stderr=subprocess.STDOUT,
        check=True,
        timeout=60,
    )
PY
grep -Fq '<h1>F1R3Gaze</h1>' "$work/installed.log"
pkgutil --pkg-info "$bundle_id"
echo "sandboxed macOS package installed and loaded built-in and network pages inside $bundle_id"
