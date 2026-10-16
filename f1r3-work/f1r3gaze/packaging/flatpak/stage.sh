#!/usr/bin/env bash
# Prepare the small input tree shared by sandbox package builders.
set -euo pipefail
bin=${1:?binary directory required}
out=${2:?output directory required}
here=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$out/icons"
for name in f1r3gaze f1r3c; do
  install -m755 "$bin/$name" "$out/$name"
done
install -m644 "$here/linux/f1r3gaze.desktop" "$out/f1r3gaze.desktop"
sed -i 's/^Icon=f1r3gaze$/Icon=io.f1r3fly.F1R3Gaze/' "$out/f1r3gaze.desktop"
for size in 16 32 48 64 128 256 512; do
  install -m644 "$here/icons/$size.png" "$out/icons/$size.png"
done
install -m644 "$here/icons/f1r3gaze.svg" "$out/icons/f1r3gaze.svg"
install -m644 "$here/../../../LICENSE" "$out/LICENSE"
