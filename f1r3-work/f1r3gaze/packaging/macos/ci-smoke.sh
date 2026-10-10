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
detach_dmg() {
  for ((attempt = 0; attempt < 60; attempt++)); do
    if hdiutil detach "$mount" >/dev/null 2>&1; then attached=0; return 0; fi
    # hdiutil can report Resource busy even when the image has just detached.
    if ! mount | grep -Fq " on $mount ("; then attached=0; return 0; fi
    sleep 0.5
  done
  hdiutil detach "$mount"
}
cleanup() {
  local code=$?
  if [[ "$attached" == 1 ]]; then detach_dmg || true; fi
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
detach_dmg
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
url_log="$RUNNER_TEMP/f1r3gaze-url-$(uname -m).log"
cleanup_app() {
  if [[ -n "$app_pid" ]]; then
    kill "$app_pid" 2>/dev/null || true
    for ((attempt = 0; attempt < 50; attempt++)); do
      kill -0 "$app_pid" 2>/dev/null || break
      sleep 0.1
    done
    if kill -0 "$app_pid" 2>/dev/null; then kill -KILL "$app_pid" 2>/dev/null || true; fi
  fi
  rm -rf "$profile"
}
trap cleanup_app EXIT
open --stderr "$url_log" -a "$installed" --args --profile "$profile"
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
open -u "$url"
delivered=0
for ((attempt = 0; attempt < 300; attempt++)); do
  if [[ -f "$profile/state/session.json" ]] && grep -Fq "$url" "$profile/state/session.json"; then
    delivered=1
    break
  fi
  kill -0 "$app_pid" 2>/dev/null || { echo 'macOS app exited before URL delivery' >&2; exit 1; }
  sleep 0.1
done
if [[ "$delivered" != 1 ]]; then
  if [[ -f "$profile/state/session.json" ]]; then cat "$profile/state/session.json" >&2; fi
  if [[ -f "$url_log" ]]; then tail -n 50 "$url_log" >&2; fi
  echo 'Launch Services URL was not saved in the running app session' >&2
  exit 1
fi
cleanup_app
trap - EXIT

# Check the cold-launch path separately: Launch Services opens the URL while
# starting the app, before the first browser surface has been created.
profile=$(mktemp -d "$RUNNER_TEMP/f1r3gaze-cold-url.XXXXXX")
app_pid=
trap cleanup_app EXIT
cold_url='f1r3://abcd/cold-start'
cold_log="$RUNNER_TEMP/f1r3gaze-cold-url-$(uname -m).log"
open --stderr "$cold_log" --env "F1R3GAZE_PROFILE=$profile" -u "$cold_url"
delivered=0
for ((attempt = 0; attempt < 300; attempt++)); do
  if [[ -f "$profile/state/session.json" ]] && grep -Fq "$cold_url" "$profile/state/session.json"; then
    delivered=1
    break
  fi
  app_pid=$(pgrep -f "$installed/Contents/MacOS/f1r3gaze" | head -n 1 || true)
  if [[ -n "$app_pid" ]]; then
    kill -0 "$app_pid" 2>/dev/null || { echo 'macOS app exited before cold URL delivery' >&2; exit 1; }
  fi
  sleep 0.1
done
if [[ "$delivered" != 1 ]]; then
  if [[ -f "$profile/state/session.json" ]]; then cat "$profile/state/session.json" >&2; fi
  if [[ -f "$cold_log" ]]; then tail -n 50 "$cold_log" >&2; fi
  echo 'Launch Services cold URL was not saved in the app session' >&2
  exit 1
fi
cleanup_app
trap - EXIT
