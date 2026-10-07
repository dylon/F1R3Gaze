#!/usr/bin/env bash
# Run as root inside an Arch/Arch Linux ARM base-devel container.
set -euo pipefail
version=${1:?version required}
project=$(pwd -P)
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
pacman -U --noconfirm /tmp/gaze-arch-output/f1r3gaze-*.pkg.tar.zst
[[ "$(f1r3gaze --version)" == "f1r3gaze $version" ]]
test -x /usr/bin/f1r3c
pacman -R --noconfirm f1r3gaze
! test -e /usr/bin/f1r3gaze
