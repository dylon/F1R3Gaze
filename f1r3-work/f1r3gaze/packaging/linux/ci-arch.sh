#!/usr/bin/env bash
# Run as root inside an Arch/Arch Linux ARM base-devel container.
set -euo pipefail
version=${1:?version required}
[[ $version =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] ||
  { echo "expected stable X.Y.Z version: $version" >&2; exit 2; }
IFS=. read -r major minor patch <<< "$version"
if ((patch > 0)); then
  older=$major.$minor.$((patch - 1))
elif ((minor > 0)); then
  older=$major.$((minor - 1)).1
elif ((major > 0)); then
  older=$((major - 1)).1.1
else
  echo 'version 0.0.0 has no earlier upgrade fixture' >&2
  exit 2
fi
project=$(pwd -P)
arch=$(uname -m)
# The container runtime blocks Landlock, which pacman uses for downloads.
# This changes only the disposable CI image's pacman configuration.
sed -i '/^\[options\]/a DisableSandbox' /etc/pacman.conf
pacman -Sy --noconfirm
command -v zstd >/dev/null || pacman -S --noconfirm zstd
useradd --create-home --shell /bin/bash gaze-builder
mkdir -p /tmp/gaze-arch-output
chown gaze-builder:gaze-builder /tmp/gaze-arch-output
su gaze-builder -s /bin/bash -c \
  "cd '$project' && packaging/linux/build.sh --format arch --version '$version' --bin '$project/target/release' --out /tmp/gaze-arch-output"
mkdir -p "$project/dist"
cp /tmp/gaze-arch-output/f1r3gaze-*.pkg.tar.zst "$project/dist/"
mkdir -p /tmp/gaze-arch-older
cp packaging/linux/arch/PKGBUILD /tmp/gaze-arch-older/PKGBUILD
cp /etc/makepkg.conf /tmp/gaze-arch-older/makepkg.conf
printf "\nPKGEXT='.pkg.tar.zst'\n" >> /tmp/gaze-arch-older/makepkg.conf
chown -R gaze-builder:gaze-builder /tmp/gaze-arch-older
su gaze-builder -s /bin/bash -c \
  "cd /tmp/gaze-arch-older && F1R3GAZE_BIN_DIR='$project/target/release' F1R3GAZE_PROJECT_DIR='$project' F1R3GAZE_VERSION='$older' PKGDEST=/tmp/gaze-arch-older makepkg --config /tmp/gaze-arch-older/makepkg.conf --nodeps --noconfirm --force"
older_package=/tmp/gaze-arch-older/f1r3gaze-$older-1-$arch.pkg.tar.zst
package=/tmp/gaze-arch-output/f1r3gaze-$version-1-$arch.pkg.tar.zst
test -s "$older_package"
test -s "$package"
pacman -U --noconfirm "$older_package"
[[ "$(pacman -Q f1r3gaze)" == "f1r3gaze $older-1" ]]
pacman -U --noconfirm "$package"
[[ "$(pacman -Q f1r3gaze)" == "f1r3gaze $version-1" ]]
[[ "$(f1r3gaze --version)" == "f1r3gaze $version" ]]
test -x /usr/bin/f1r3c
pacman -R --noconfirm f1r3gaze
! test -e /usr/bin/f1r3gaze
