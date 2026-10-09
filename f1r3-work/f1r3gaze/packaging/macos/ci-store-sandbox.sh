#!/usr/bin/env bash
# Re-sign the real DMG app as an isolated App Sandbox prototype and exercise it.
# This uses an ad hoc identity and a disposable bundle ID; it is not a Store
# submission artifact. The Developer ID DMG/PKG and Homebrew cask stay intact.
set -euo pipefail
version=${1:?version required}
dist=${2:-dist}
dist=$(cd "$dist" && pwd)
dmg="$dist/F1R3Gaze-$version-macos-universal.dmg"
[[ -s "$dmg" ]] || { echo "macOS DMG missing or empty: $dmg" >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/f1r3gaze-store-sandbox.XXXXXX")
mount="$work/mount"
mkdir "$mount"
attached=0
server_pid=
bundle_id="io.f1r3fly.f1r3gaze.sandboxci.run$(uuidgen | tr '[:upper:]' '[:lower:]' | tr -d '-')"
container="$HOME/Library/Containers/$bundle_id"
cleanup() {
  local code=$?
  if [[ -n "$server_pid" ]]; then kill "$server_pid" 2>/dev/null || true; fi
  if [[ "$attached" == 1 ]]; then hdiutil detach "$mount" >/dev/null || true; fi
  rm -rf "$work" "$container"
  exit "$code"
}
trap cleanup EXIT
[[ ! -e "$container" ]] || { echo "test container already exists: $container" >&2; exit 2; }

hdiutil attach -nobrowse -readonly -mountpoint "$mount" "$dmg" >/dev/null
attached=1
app="$work/F1R3Gaze.app"
ditto "$mount/F1R3Gaze.app" "$app"
hdiutil detach "$mount" >/dev/null
attached=0
plutil -replace CFBundleIdentifier -string "$bundle_id" "$app/Contents/Info.plist"

codesign --force --sign - --options runtime --identifier "$bundle_id.f1r3c" \
  --entitlements "$here/store-helper-entitlements.plist" "$app/Contents/MacOS/f1r3c"
codesign --force --sign - --options runtime --identifier "$bundle_id" \
  --entitlements "$here/store-app-entitlements.plist" "$app"
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
paths=$("$exe" paths)
support="$container/Data/Library/Application Support/io.f1r3fly.f1r3gaze"
[[ "$paths" == *"$support/config"* ]] || {
  echo "sandboxed Gaze did not locate its profile inside the container:" >&2
  printf '%s\n' "$paths" >&2
  exit 1
}
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
echo "sandboxed macOS app loaded built-in and network pages inside $bundle_id"
