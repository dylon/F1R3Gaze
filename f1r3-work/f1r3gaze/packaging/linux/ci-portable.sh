#!/usr/bin/env bash
# Launch the portable tarball and AppImage on their native Linux runner.
set -euo pipefail
version=${1:?version required}
arch=$(uname -m)
archive="dist/f1r3gaze-$version-linux-$arch.tar.gz"
appimage="dist/F1R3Gaze-$version-$arch.AppImage"
[[ -s "$archive" && -s "$appimage" ]] || { echo "portable artifact missing or empty" >&2; exit 2; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
tar -tzf "$archive" > "$work/files"
grep -qx "f1r3gaze-$version/bin/f1r3gaze" "$work/files"
grep -qx "f1r3gaze-$version/bin/f1r3c" "$work/files"
if grep -Eiq 'f1r3node|embers' "$work/files"; then
  echo "unexpected product in portable tarball" >&2
  exit 2
fi
tar -xzf "$archive" -C "$work"
[[ "$("$work/f1r3gaze-$version/bin/f1r3gaze" --version)" == "f1r3gaze $version" ]]
[[ "$("$work/f1r3gaze-$version/bin/f1r3c" --version)" == "f1r3c $version" ]]
chmod +x "$appimage"
[[ "$("$appimage" --appimage-extract-and-run --version)" == "f1r3gaze $version" ]]
echo "portable Linux artifacts launched on $arch"
