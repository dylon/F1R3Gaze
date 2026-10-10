#!/usr/bin/env bash
# Exercise installation, major-version upgrade and removal on a target image.
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
case "$(uname -m)" in
  x86_64) arch=amd64 ;;
  aarch64) arch=arm64 ;;
  *) echo "unsupported Debian CI architecture" >&2; exit 2 ;;
esac
package="/src/dist/f1r3gaze_${version}_${arch}.deb"
[[ -f "$package" ]] || { echo "missing $package" >&2; exit 2; }
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
dpkg-deb --raw-extract "$package" "$work/older-root"
sed -i "s/^Version: .*/Version: $older/" "$work/older-root/DEBIAN/control"
older_package=$work/f1r3gaze_${older}_${arch}.deb
dpkg-deb --root-owner-group --build "$work/older-root" "$older_package"
[[ $(dpkg-deb --field "$older_package" Version) == "$older" ]]
apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install -y "$older_package"
[[ $(dpkg-query -W -f='${Version}' f1r3gaze) == "$older" ]]
DEBIAN_FRONTEND=noninteractive apt-get install -y "$package"
[[ $(dpkg-query -W -f='${Version}' f1r3gaze) == "$version" ]]
[[ "$(f1r3gaze --version)" == "f1r3gaze $version" ]]
test -x /usr/bin/f1r3c
test -f /usr/share/applications/f1r3gaze.desktop
DEBIAN_FRONTEND=noninteractive apt-get remove -y f1r3gaze
! test -e /usr/bin/f1r3gaze
