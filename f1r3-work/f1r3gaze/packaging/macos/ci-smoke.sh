#!/usr/bin/env bash
# Verify a universal DMG and install its component PKG on a macOS runner.
set -euo pipefail
version=${1:?version required}
dist=${2:-dist}
dist=$(cd "$dist" && pwd)
dmg="$dist/F1R3Gaze-$version-macos-universal.dmg"
pkg="$dist/F1R3Gaze-$version-macos-universal.pkg"
[[ -s "$dmg" && -s "$pkg" ]] || { echo "macOS installer missing or empty" >&2; exit 2; }

hdiutil verify "$dmg"
mount=$(mktemp -d "$RUNNER_TEMP/f1r3gaze-dmg.XXXXXX")
attached=0
cleanup() {
  local code=$?
  if [[ "$attached" == 1 ]]; then hdiutil detach "$mount" || true; fi
  rmdir "$mount" 2>/dev/null || true
  exit "$code"
}
trap cleanup EXIT
hdiutil attach -nobrowse -readonly -mountpoint "$mount" "$dmg"
attached=1
app="$mount/F1R3Gaze.app"
test -x "$app/Contents/MacOS/f1r3gaze"
test -x "$app/Contents/MacOS/f1r3c"
lipo "$app/Contents/MacOS/f1r3gaze" -verify_arch arm64 x86_64
lipo "$app/Contents/MacOS/f1r3c" -verify_arch arm64 x86_64
[[ "$("$app/Contents/MacOS/f1r3gaze" --version)" == "f1r3gaze $version" ]]
[[ "$("$app/Contents/MacOS/f1r3c" --version)" == "f1r3c $version" ]]
if find "$app" \( -iname '*f1r3node*' -o -iname '*embers*' \) | grep -q .; then
  echo 'unexpected separate product in the F1R3Gaze app' >&2
  exit 2
fi
hdiutil detach "$mount"
rmdir "$mount"
trap - EXIT

sudo installer -pkg "$pkg" -target /
installed=/Applications/F1R3Gaze.app
test -x "$installed/Contents/MacOS/f1r3gaze"
test -x "$installed/Contents/MacOS/f1r3c"
[[ "$("$installed/Contents/MacOS/f1r3gaze" --version)" == "f1r3gaze $version" ]]
[[ "$("$installed/Contents/MacOS/f1r3c" --version)" == "f1r3c $version" ]]
pkgutil --pkg-info io.f1r3fly.f1r3gaze
