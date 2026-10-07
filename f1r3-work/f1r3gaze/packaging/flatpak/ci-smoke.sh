#!/usr/bin/env bash
# Build, install, launch and remove a native Flatpak bundle on a CI runner.
set -euo pipefail
version=${1:?version required}
here=$(cd "$(dirname "$0")" && pwd)
project=$(cd "$here/../.." && pwd)
arch=$(uname -m)
case "$arch" in
  x86_64|aarch64) ;;
  *) echo "unsupported Flatpak host architecture: $arch" >&2; exit 2 ;;
esac
cd "$project"
input=dist/flatpak-input
build=dist/flatpak-build
repo=dist/flatpak-repo
bundle="dist/F1R3Gaze-$version-$arch.flatpak"
[[ ! -e "$input" && ! -e "$build" && ! -e "$repo" && ! -e "$bundle" ]] ||
  { echo "Flatpak build output already exists" >&2; exit 2; }
"$here/stage.sh" target/release "$input"
flatpak remote-add --user --if-not-exists flathub \
  https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak-builder --user --force-clean --install-deps-from=flathub \
  --repo="$repo" "$build" "$here/io.f1r3fly.F1R3Gaze.json"
flatpak build-bundle "$repo" "$bundle" io.f1r3fly.F1R3Gaze \
  --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo
test -s "$bundle"
flatpak --user install --noninteractive -y "$bundle"
[[ "$(flatpak run --user io.f1r3fly.F1R3Gaze --version)" == "f1r3gaze $version" ]]
[[ "$(flatpak run --user --command=f1r3c io.f1r3fly.F1R3Gaze --version)" == "f1r3c $version" ]]
flatpak --user uninstall --noninteractive -y io.f1r3fly.F1R3Gaze
! flatpak --user info io.f1r3fly.F1R3Gaze >/dev/null 2>&1
