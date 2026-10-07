#!/usr/bin/env bash
# Run as root inside an Arch/Arch Linux ARM base-devel container.
set -euo pipefail
version=${1:?version required}
useradd --create-home --shell /bin/bash gaze-builder
mkdir -p /tmp/gaze-arch-output
chown gaze-builder:gaze-builder /tmp/gaze-arch-output
su gaze-builder -s /bin/bash -c \
  "cd /src && packaging/linux/build.sh --format arch --version '$version' --bin /src/target/release --out /tmp/gaze-arch-output"
mkdir -p /src/dist
cp /tmp/gaze-arch-output/f1r3gaze-*.pkg.tar.zst /src/dist/
pacman -Sy --noconfirm
pacman -U --noconfirm /tmp/gaze-arch-output/f1r3gaze-*.pkg.tar.zst
[[ "$(f1r3gaze --version)" == "f1r3gaze $version" ]]
test -x /usr/bin/f1r3c
pacman -R --noconfirm f1r3gaze
! test -e /usr/bin/f1r3gaze
