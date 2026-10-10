#!/usr/bin/env bash
# Build a separate sandboxed app and productbuild package from the universal DMG.
# Usage: store-package.sh VERSION DIST OUTPUT_DIR BUNDLE_ID
# Without Store credentials this produces an ad hoc signed app and unsigned
# package for native CI. A submission build requires all MACOS_STORE_* inputs.
set -euo pipefail

version=${1:?version required}
dist=${2:?DMG directory required}
output=${3:?output directory required}
bundle_id=${4:?App Store bundle identifier required}
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "invalid version: $version" >&2; exit 2; }
[[ "$bundle_id" =~ ^[A-Za-z0-9-]+(\.[A-Za-z0-9-]+)+$ ]] || { echo "invalid bundle ID: $bundle_id" >&2; exit 2; }
dist=$(cd "$dist" && pwd)
mkdir -p "$output"
output=$(cd "$output" && pwd)
dmg="$dist/F1R3Gaze-$version-macos-universal.dmg"
app_output="$output/F1R3Gaze.app"
pkg_output="$output/F1R3Gaze-$version-macos-app-store.pkg"
[[ -s "$dmg" ]] || { echo "missing universal DMG: $dmg" >&2; exit 2; }
[[ ! -e "$app_output" && ! -e "$pkg_output" ]] || { echo "Store outputs already exist in $output" >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/f1r3gaze-store-package.XXXXXX")
mount="$work/mount"
mkdir "$mount"
attached=0
keychain=
cleanup() {
  local code=$?
  if [[ "$attached" == 1 ]]; then hdiutil detach "$mount" >/dev/null || true; fi
  if [[ -n "$keychain" ]]; then security delete-keychain "$keychain" >/dev/null 2>&1 || true; fi
  rm -rf "$work"
  exit "$code"
}
trap cleanup EXIT

signed=0
for name in MACOS_STORE_APP_CERT_P12 MACOS_STORE_APP_CERT_PASSWORD MACOS_STORE_APP_SIGN_IDENTITY \
            MACOS_STORE_INSTALLER_CERT_P12 MACOS_STORE_INSTALLER_CERT_PASSWORD \
            MACOS_STORE_INSTALLER_SIGN_IDENTITY MACOS_STORE_PROFILE MACOS_STORE_TEAM_ID; do
  if [[ -n "${!name:-}" ]]; then signed=1; fi
done
if [[ "$signed" == 1 ]]; then
  for name in MACOS_STORE_APP_CERT_P12 MACOS_STORE_APP_CERT_PASSWORD MACOS_STORE_APP_SIGN_IDENTITY \
              MACOS_STORE_INSTALLER_CERT_P12 MACOS_STORE_INSTALLER_CERT_PASSWORD \
              MACOS_STORE_INSTALLER_SIGN_IDENTITY MACOS_STORE_PROFILE MACOS_STORE_TEAM_ID; do
    [[ -n "${!name:-}" ]] || { echo "Store signing requires $name" >&2; exit 2; }
  done
  [[ -s "$MACOS_STORE_PROFILE" ]] || { echo "missing Store provisioning profile: $MACOS_STORE_PROFILE" >&2; exit 2; }
fi

hdiutil attach -nobrowse -readonly -mountpoint "$mount" "$dmg" >/dev/null
attached=1
app="$work/F1R3Gaze.app"
ditto "$mount/F1R3Gaze.app" "$app"
hdiutil detach "$mount" >/dev/null
attached=0

python3 - "$app/Contents/Info.plist" "$version" "$bundle_id" <<'PY'
import plistlib
import sys

path, version, bundle_id = sys.argv[1:]
with open(path, "rb") as stream:
    info = plistlib.load(stream)
if info["CFBundleShortVersionString"] != version or info["CFBundleVersion"] != version:
    raise SystemExit("DMG app version does not match the Store package version")
if info["CFBundleExecutable"] != "f1r3gaze":
    raise SystemExit("unexpected Store app executable")
info["CFBundleIdentifier"] = bundle_id
schemes = ["f1r3", "f1r3h"]
if [item["CFBundleURLSchemes"] for item in info["CFBundleURLTypes"]] != [[s] for s in schemes]:
    raise SystemExit("unexpected URL scheme declarations")
for item, scheme in zip(info["CFBundleURLTypes"], schemes):
    item["CFBundleURLName"] = f"{bundle_id}.{scheme}"
with open(path, "wb") as stream:
    plistlib.dump(info, stream)
PY

if [[ "$signed" == 1 ]]; then
  security cms -D -i "$MACOS_STORE_PROFILE" > "$work/profile.plist"
  python3 - "$work/profile.plist" "$MACOS_STORE_TEAM_ID" "$bundle_id" <<'PY'
import datetime
import plistlib
import sys

path, team_id, bundle_id = sys.argv[1:]
with open(path, "rb") as stream:
    profile = plistlib.load(stream)
if team_id not in profile.get("TeamIdentifier", []):
    raise SystemExit("Store provisioning profile has the wrong team")
entitlements = profile.get("Entitlements", {})
app_id = entitlements.get("com.apple.application-identifier") or entitlements.get("application-identifier")
if app_id != f"{team_id}.{bundle_id}":
    raise SystemExit("Store provisioning profile has the wrong application ID")
if profile["ExpirationDate"] <= datetime.datetime.now(datetime.timezone.utc).replace(tzinfo=None):
    raise SystemExit("Store provisioning profile has expired")
PY
  cp "$MACOS_STORE_PROFILE" "$app/Contents/embedded.provisionprofile"
fi

python3 - "$app" "$signed" <<'PY'
from pathlib import Path
import sys

app = Path(sys.argv[1])
expected = {
    "Contents/Info.plist", "Contents/MacOS/f1r3gaze", "Contents/MacOS/f1r3c",
    "Contents/Resources/LICENSE", "Contents/Resources/f1r3gaze.icns",
    "Contents/_CodeSignature/CodeResources",
}
if sys.argv[2] == "1":
    expected.add("Contents/embedded.provisionprofile")
actual = {str(path.relative_to(app)) for path in app.rglob("*") if path.is_file()}
if actual - expected or expected - actual - {"Contents/_CodeSignature/CodeResources"}:
    raise SystemExit(f"unexpected Store app payload: {sorted(actual)}")
PY

if [[ "$signed" == 1 ]]; then
  keychain="$work/store.keychain-db"
  keychain_password=$(uuidgen)
  security create-keychain -p "$keychain_password" "$keychain"
  security set-keychain-settings -lut 21600 "$keychain"
  security unlock-keychain -p "$keychain_password" "$keychain"
  printf '%s' "$MACOS_STORE_APP_CERT_P12" | base64 -D > "$work/app.p12"
  printf '%s' "$MACOS_STORE_INSTALLER_CERT_P12" | base64 -D > "$work/installer.p12"
  security import "$work/app.p12" -k "$keychain" -P "$MACOS_STORE_APP_CERT_PASSWORD" -T /usr/bin/codesign
  security import "$work/installer.p12" -k "$keychain" -P "$MACOS_STORE_INSTALLER_CERT_PASSWORD" -T /usr/bin/productbuild
  security set-key-partition-list -S apple-tool:,apple: -s -k "$keychain_password" "$keychain" >/dev/null
  codesign --force --timestamp --options runtime --identifier "$bundle_id.f1r3c" \
    --entitlements "$here/store-helper-entitlements.plist" --keychain "$keychain" \
    --sign "$MACOS_STORE_APP_SIGN_IDENTITY" "$app/Contents/MacOS/f1r3c"
  codesign --force --timestamp --options runtime --identifier "$bundle_id" \
    --entitlements "$here/store-app-entitlements.plist" --keychain "$keychain" \
    --sign "$MACOS_STORE_APP_SIGN_IDENTITY" "$app"
else
  codesign --force --sign - --options runtime --identifier "$bundle_id.f1r3c" \
    --entitlements "$here/store-helper-entitlements.plist" "$app/Contents/MacOS/f1r3c"
  codesign --force --sign - --options runtime --identifier "$bundle_id" \
    --entitlements "$here/store-app-entitlements.plist" "$app"
fi
codesign --verify --deep --strict --verbose=2 "$app"
if [[ "$signed" == 1 ]]; then
  codesign -dv --verbose=4 "$app" 2> "$work/signature-details"
  grep -Fxq "TeamIdentifier=$MACOS_STORE_TEAM_ID" "$work/signature-details" || {
    echo "Store app signature has the wrong team" >&2
    exit 2
  }
fi

pkg="$work/F1R3Gaze-$version-macos-app-store.pkg"
if [[ "$signed" == 1 ]]; then
  productbuild --sign "$MACOS_STORE_INSTALLER_SIGN_IDENTITY" --keychain "$keychain" \
    --component "$app" /Applications "$pkg"
  pkgutil --check-signature "$pkg" >/dev/null
else
  productbuild --component "$app" /Applications "$pkg"
fi
pkgutil --payload-files "$pkg" > "$work/payload-files"
grep -q 'MacOS/f1r3gaze' "$work/payload-files"
grep -q 'MacOS/f1r3c' "$work/payload-files"
if grep -Eiq 'f1r3node|embers' "$work/payload-files"; then
  echo 'unexpected product in Store package' >&2
  exit 2
fi
[[ -s "$pkg" ]] || { echo "Store package is empty" >&2; exit 2; }
ditto "$app" "$app_output"
mv "$pkg" "$pkg_output"
echo "built $app_output and $pkg_output"
