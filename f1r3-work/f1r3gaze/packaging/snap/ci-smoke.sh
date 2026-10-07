#!/usr/bin/env bash
# Install, launch and remove an unsigned Snapcraft artifact on a CI runner.
set -euo pipefail
package=${1:?snap file required}
version=${2:?version required}
repo=$(cd "$(dirname "$0")/../../../.." && pwd)
if [[ ! -f "$package" && -f "$repo/$package" ]]; then
  package="$repo/$package"
fi
test -s "$package"
sudo snap install --dangerous "$package"
[[ "$(snap run f1r3gaze.f1r3gaze --version)" == "f1r3gaze $version" ]]
[[ "$(snap run f1r3gaze.f1r3c --version)" == "f1r3c $version" ]]
sudo snap remove --purge f1r3gaze
! snap list f1r3gaze >/dev/null 2>&1
