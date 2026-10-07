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
old_dir=
cleanup() {
  local code=$?
  if [[ "$attached" == 1 ]]; then hdiutil detach "$mount" || true; fi
  rmdir "$mount" 2>/dev/null || true
  if [[ -n "$old_dir" ]]; then rm -rf "$old_dir"; fi
  exit "$code"
}
trap cleanup EXIT
hdiutil attach -nobrowse -readonly -mountpoint "$mount" "$dmg"
attached=1
app="$mount/F1R3Gaze.app"
test -x "$app/Contents/MacOS/f1r3gaze"
test -x "$app/Contents/MacOS/f1r3c"
python3 - "$app/Contents/Info.plist" <<'PY'
import plistlib
import sys

with open(sys.argv[1], "rb") as stream:
    info = plistlib.load(stream)
assert [entry["CFBundleURLSchemes"] for entry in info["CFBundleURLTypes"]] == [["f1r3"], ["f1r3h"]]
assert all(entry["CFBundleTypeRole"] == "Editor" for entry in info["CFBundleURLTypes"])
PY
lipo "$app/Contents/MacOS/f1r3gaze" -verify_arch arm64 x86_64
lipo "$app/Contents/MacOS/f1r3c" -verify_arch arm64 x86_64
[[ "$("$app/Contents/MacOS/f1r3gaze" --version)" == "f1r3gaze $version" ]]
[[ "$("$app/Contents/MacOS/f1r3c" --version)" == "f1r3c $version" ]]
if find "$app" \( -iname '*f1r3node*' -o -iname '*embers*' \) | grep -q .; then
  echo 'unexpected separate product in the F1R3Gaze app' >&2
  exit 2
fi
# A lower-version component package exercises the macOS Installer upgrade
# transaction with this exact application payload.
IFS=. read -r major minor patch <<< "$version"
if ((patch > 0)); then
  older="$major.$minor.$((patch - 1))"
elif ((minor > 0)); then
  older="$major.$((minor - 1)).1"
elif ((major > 0)); then
  older="$((major - 1)).1.1"
else
  echo 'macOS package 0.0.0 has no earlier version for upgrade testing' >&2
  exit 2
fi
old_dir=$(mktemp -d "$RUNNER_TEMP/f1r3gaze-old-pkg.XXXXXX")
old_pkg="$old_dir/F1R3Gaze-$older.pkg"
pkgbuild --component "$app" --install-location /Applications \
  --identifier io.f1r3fly.f1r3gaze --version "$older" "$old_pkg"
sudo installer -pkg "$old_pkg" -target /
[[ "$(pkgutil --pkg-info io.f1r3fly.f1r3gaze | awk '/^version:/ {print $2}')" == "$older" ]]
hdiutil detach "$mount"
attached=0
rmdir "$mount"

sudo installer -pkg "$pkg" -target /
[[ "$(pkgutil --pkg-info io.f1r3fly.f1r3gaze | awk '/^version:/ {print $2}')" == "$version" ]]
rm -rf "$old_dir"
old_dir=
trap - EXIT
installed=/Applications/F1R3Gaze.app
test -x "$installed/Contents/MacOS/f1r3gaze"
test -x "$installed/Contents/MacOS/f1r3c"
[[ "$("$installed/Contents/MacOS/f1r3gaze" --version)" == "f1r3gaze $version" ]]
[[ "$("$installed/Contents/MacOS/f1r3c" --version)" == "f1r3c $version" ]]
pkgutil --pkg-info io.f1r3fly.f1r3gaze

# Launch Services must deliver a registered URL to the already running app.
profile=$(mktemp -d "$RUNNER_TEMP/f1r3gaze-url-profile.XXXXXX")
app_pid=
cleanup_app() {
  if [[ -n "$app_pid" ]]; then kill "$app_pid" 2>/dev/null || true; fi
  rm -rf "$profile"
}
trap cleanup_app EXIT
open -a "$installed" --args --profile "$profile"
for ((attempt = 0; attempt < 100; attempt++)); do
  app_pid=$(pgrep -f "$installed/Contents/MacOS/f1r3gaze" | head -n 1 || true)
  if [[ -n "$app_pid" && -f "$profile/runtime/instance.lock" ]]; then break; fi
  sleep 0.1
done
[[ -n "$app_pid" && -f "$profile/runtime/instance.lock" ]] || {
  echo 'installed macOS app did not start with the isolated profile' >&2
  exit 1
}
sleep 1
url='f1r3://abcd/ci-smoke'
open -a "$installed" "$url"
delivered=0
for ((attempt = 0; attempt < 100; attempt++)); do
  if [[ -f "$profile/state/session.json" ]] && grep -Fq "$url" "$profile/state/session.json"; then
    delivered=1
    break
  fi
  kill -0 "$app_pid" 2>/dev/null || { echo 'macOS app exited before URL delivery' >&2; exit 1; }
  sleep 0.1
done
[[ "$delivered" == 1 ]] || { echo 'Launch Services URL was not saved in the running app session' >&2; exit 1; }
cleanup_app
trap - EXIT
