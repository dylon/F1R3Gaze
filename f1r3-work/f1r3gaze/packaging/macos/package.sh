#!/usr/bin/env bash
# Build a universal app, DMG and component PKG. Signing and notarisation are
# enabled only when the respective identities and credentials are supplied.
# Usage: packaging/macos/package.sh <version> <arm64 bin dir> <x86_64 bin dir>
# Signing env (all optional; without them the DMG is unsigned):
#   MACOS_CERT_P12      base64 of a "Developer ID Application" .p12
#   MACOS_CERT_PASSWORD its password
#   MACOS_SIGN_IDENTITY e.g. "Developer ID Application: F1R3FLY.io (TEAMID)"
#   MACOS_INSTALLER_CERT_P12      base64 of a "Developer ID Installer" .p12
#   MACOS_INSTALLER_CERT_PASSWORD its password
#   MACOS_INSTALLER_SIGN_IDENTITY the Installer identity
# Notarisation env (optional): APPLE_ID, APPLE_TEAM_ID, APPLE_APP_PASSWORD
set -euo pipefail
VER="${1:?version}"; ARM="${2:?arm64 bin dir}"; X86="${3:?x86_64 bin dir}"
[[ "$VER" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  { echo "macOS package version must be stable X.Y.Z: $VER" >&2; exit 2; }
HERE="$(cd "$(dirname "$0")" && pwd)"; ICONS="$HERE/../icons"
PROJECT="$(cd "$HERE/../.." && pwd)"
EXPECTED="$(awk '/^\[workspace.package\]/{section=1;next} /^\[/{section=0} section && /^version = / {gsub(/"/,"",$3);print $3;exit}' "$PROJECT/Cargo.toml")"
[[ "$VER" == "$EXPECTED" ]] ||
  { echo "version $VER differs from Cargo workspace version $EXPECTED" >&2; exit 2; }
OUT="${DIST:-dist}"; mkdir -p "$OUT"; OUT="$(cd "$OUT" && pwd)"
WORK="$(mktemp -d)"
cleanup() {
  local code=$?
  rm -rf "$WORK"
  if [[ -n "${KC:-}" ]]; then security delete-keychain "$KC" 2>/dev/null || true; fi
  exit "$code"
}
trap cleanup EXIT
for arch_dir in "$ARM" "$X86"; do
  for binary in f1r3gaze f1r3c; do
    [[ -f "$arch_dir/$binary" ]] || { echo "missing $arch_dir/$binary" >&2; exit 2; }
  done
done

APP="$WORK/stage/F1R3Gaze.app"; C="$APP/Contents"
mkdir -p "$C/MacOS" "$C/Resources"
lipo -create "$ARM/f1r3gaze" "$X86/f1r3gaze" -output "$C/MacOS/f1r3gaze"
lipo -create "$ARM/f1r3c" "$X86/f1r3c" -output "$C/MacOS/f1r3c"
for binary in f1r3gaze f1r3c; do
  lipo "$C/MacOS/$binary" -verify_arch arm64 x86_64
done
sed -e "s/__VERSION__/$VER/g" -e "s/__BUNDLE_VERSION__/$VER/g" \
  "$HERE/Info.plist" > "$C/Info.plist"

IS="$WORK/f1r3gaze.iconset"; mkdir -p "$IS"
for s in 16 32 128 256 512; do
  cp "$ICONS/$s.png" "$IS/icon_${s}x${s}.png"
  cp "$ICONS/$((s * 2)).png" "$IS/icon_${s}x${s}@2x.png" 2>/dev/null || cp "$ICONS/1024.png" "$IS/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$IS" -o "$C/Resources/f1r3gaze.icns"
cp "$HERE/../../../../LICENSE" "$C/Resources/LICENSE"

# Assert the complete unsigned payload before signing adds _CodeSignature.
actual="$(cd "$APP" && find Contents -type f | LC_ALL=C sort)"
expected="$(printf '%s\n' Contents/Info.plist Contents/MacOS/f1r3c Contents/MacOS/f1r3gaze Contents/Resources/LICENSE Contents/Resources/f1r3gaze.icns | LC_ALL=C sort)"
[[ "$actual" == "$expected" ]] ||
  { echo "unexpected macOS app payload:" >&2; printf '%s\n' "$actual" >&2; exit 2; }

SIGN=""
if [ -n "${MACOS_CERT_P12:-}" ]; then
  KC="$WORK/build.keychain-db"; KP="$(uuidgen)"
  security create-keychain -p "$KP" "$KC"
  security set-keychain-settings -lut 21600 "$KC"
  security unlock-keychain -p "$KP" "$KC"
  printf '%s' "$MACOS_CERT_P12" | base64 -D > "$WORK/cert.p12"
  security import "$WORK/cert.p12" -k "$KC" -P "${MACOS_CERT_PASSWORD:-}" -T /usr/bin/codesign
  if [[ -n "${MACOS_INSTALLER_CERT_P12:-}" ]]; then
    printf '%s' "$MACOS_INSTALLER_CERT_P12" | base64 -D > "$WORK/installer-cert.p12"
    security import "$WORK/installer-cert.p12" -k "$KC" -P "${MACOS_INSTALLER_CERT_PASSWORD:-}" -T /usr/bin/pkgbuild
  fi
  security set-key-partition-list -S apple-tool:,apple: -s -k "$KP" "$KC" >/dev/null
  SIGN="${MACOS_SIGN_IDENTITY:?MACOS_SIGN_IDENTITY must name the certificate}"
  cs() { codesign --force --timestamp --options runtime --entitlements "$HERE/entitlements.plist" --keychain "$KC" --sign "$SIGN" "$@"; }
  cs "$C/MacOS/f1r3c"
  cs "$C/MacOS/f1r3gaze"
  cs "$APP"
  codesign --verify --deep --strict --verbose=2 "$APP"
else
  echo "MACOS_CERT_P12 not set: the app is unsigned" >&2
fi

ln -s /Applications "$WORK/stage/Applications"
DMG="$OUT/F1R3Gaze-$VER-macos-universal.dmg"
for attempt in 1 2 3 4; do
  if hdiutil create -volname "F1R3Gaze $VER" -srcfolder "$WORK/stage" -ov -format UDZO "$DMG" \
    >/dev/null 2>"$WORK/hdiutil-create.err"; then
    break
  fi
  cat "$WORK/hdiutil-create.err" >&2
  if ! grep -q 'Resource busy' "$WORK/hdiutil-create.err" || [[ "$attempt" -eq 4 ]]; then
    exit 1
  fi
  sleep "$((attempt * 5))"
done
hdiutil verify "$DMG" >/dev/null
if [[ -n "$SIGN" ]]; then
  codesign --force --timestamp --identifier io.f1r3fly.f1r3gaze.dmg --keychain "$KC" --sign "$SIGN" "$DMG"
fi

PKG="$OUT/F1R3Gaze-$VER-macos-universal.pkg"
PKG_SIGNED=0
if [[ -n "${MACOS_INSTALLER_CERT_P12:-}" ]]; then
  [[ -n "$SIGN" ]] || { echo "Installer signing requires application signing" >&2; exit 2; }
  pkgbuild --component "$APP" --install-location /Applications \
    --identifier io.f1r3fly.f1r3gaze --version "$VER" \
    --sign "${MACOS_INSTALLER_SIGN_IDENTITY:?Installer identity required}" --keychain "$KC" "$PKG"
  PKG_SIGNED=1
else
  pkgbuild --component "$APP" --install-location /Applications \
    --identifier io.f1r3fly.f1r3gaze --version "$VER" "$PKG"
fi
[[ -s "$DMG" && -s "$PKG" ]] || { echo "macOS package output is missing or empty" >&2; exit 2; }
pkgutil --payload-files "$PKG" > "$WORK/pkg-files"
grep -q 'MacOS/f1r3gaze' "$WORK/pkg-files"
grep -q 'MacOS/f1r3c' "$WORK/pkg-files"
if grep -Eiq 'f1r3node|embers' "$WORK/pkg-files"; then
  echo "unexpected product in macOS component PKG" >&2
  exit 2
fi
pkgutil --check-signature "$PKG" >/dev/null 2>&1 || [[ "$PKG_SIGNED" -eq 0 ]]

if [ -n "$SIGN" ] && [ -n "${APPLE_ID:-}" ]; then
  [[ "$PKG_SIGNED" -eq 1 ]] || { echo "notarisation requires an Installer-signed PKG" >&2; exit 2; }
  for artifact in "$DMG" "$PKG"; do
    xcrun notarytool submit "$artifact" --apple-id "$APPLE_ID" \
      --team-id "${APPLE_TEAM_ID:?Apple team ID required}" \
      --password "${APPLE_APP_PASSWORD:?Apple app password required}" --wait
    xcrun stapler staple "$artifact"
  done
  spctl --assess --type open --context context:primary-signature --verbose "$DMG"
else
  echo "notarisation skipped (needs signing and APPLE_ID)" >&2
fi
echo "built $DMG and $PKG"
