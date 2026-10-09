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
installed=0
installed_app=/Applications/F1R3Gaze.app
bundle_id="io.f1r3fly.f1r3gaze.sandboxci.run$(uuidgen | tr '[:upper:]' '[:lower:]' | tr -d '-')"
container="$HOME/Library/Containers/$bundle_id"
cleanup() {
  local code=$?
  if [[ -n "$server_pid" ]]; then kill "$server_pid" 2>/dev/null || true; fi
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
compiler_dir="$container/Data/Documents/compiler-smoke"
mkdir -p "$compiler_dir"
printf 'Nil\n' > "$compiler_dir/example.rho"
"$app/Contents/MacOS/f1r3c" compile "$compiler_dir/example.rho" -o "$compiler_dir/example.knf"
[[ -s "$compiler_dir/example.knf" ]] || { echo 'sandboxed f1r3c did not create its output' >&2; exit 1; }
"$app/Contents/MacOS/f1r3c" inspect "$compiler_dir/example.knf" | grep -Fxq 'Nil'
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
