#!/usr/bin/env bash
# Build both native slices and package a universal DMG and component PKG.
# Usage: packaging/macos/build-local.sh VERSION [OUTPUT_DIR]
set -euo pipefail

version=${1:?stable X.Y.Z version required}
out=${2:-dist}
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo "version must be stable X.Y.Z: $version" >&2
  exit 2
}
[[ $(uname -s) == Darwin ]] || {
  echo 'the macOS DMG must be built on macOS (requires lipo, iconutil and hdiutil)' >&2
  exit 2
}

here=$(cd "$(dirname "$0")" && pwd)
project=$(cd "$here/../.." && pwd)
expected=$(awk '/^\[workspace.package\]/{section=1;next} /^\[/{section=0} section && /^version = / {gsub(/"/,"",$3);print $3;exit}' "$project/Cargo.toml")
[[ "$version" == "$expected" ]] || {
  echo "version $version differs from Cargo workspace version $expected" >&2
  exit 2
}
target_dir=${CARGO_TARGET_DIR:-target}
case "$target_dir" in
  /*) ;;
  *) target_dir="$project/$target_dir" ;;
esac

(
  cd "$project"
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build --locked --release --target "$target" \
      --target-dir "$target_dir" \
      -p gaze-shell -p f1r3c --features gaze-shell/os-keyring
  done
  DIST="$out" bash "$here/package.sh" "$version" \
    "$target_dir/aarch64-apple-darwin/release" \
    "$target_dir/x86_64-apple-darwin/release"
)
