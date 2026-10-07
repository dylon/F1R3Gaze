#!/usr/bin/env bash
# Prepare a self-contained snapcraft build context from native binaries.
set -euo pipefail
bin=${1:?binary directory required}
out=${2:?fresh output directory required}
version=${3:?version required}
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  { echo "Snap version must be stable X.Y.Z" >&2; exit 2; }
[[ ! -e "$out" ]] || { echo "output already exists: $out" >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
"$here/../flatpak/stage.sh" "$bin" "$out"
cp "$here/../linux/f1r3gaze.desktop" "$out/f1r3gaze.desktop"
mkdir -p "$out/snap"
sed "s/^version: '.*'/version: '$version'/" "$here/snapcraft.yaml" \
  > "$out/snap/snapcraft.yaml"
echo "staged snapcraft context at $out"
