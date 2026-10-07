#!/usr/bin/env bash
# Exercise one Ubuntu/Debian image against the built native .deb.
set -euo pipefail
version=${1:?version required}
case "$(uname -m)" in
  x86_64) arch=amd64 ;;
  aarch64) arch=arm64 ;;
  *) echo "unsupported Debian CI architecture" >&2; exit 2 ;;
esac
package="/src/dist/f1r3gaze_${version}_${arch}.deb"
[[ -f "$package" ]] || { echo "missing $package" >&2; exit 2; }
apt-get update
DEBIAN_FRONTEND=noninteractive apt-get install -y "$package"
[[ "$(f1r3gaze --version)" == "f1r3gaze $version" ]]
test -x /usr/bin/f1r3c
DEBIAN_FRONTEND=noninteractive apt-get remove -y f1r3gaze
! test -e /usr/bin/f1r3gaze
