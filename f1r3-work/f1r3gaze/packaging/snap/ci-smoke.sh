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

# SNAP_USER_COMMON is writable under strict confinement across revisions.
mkdir -p "$HOME/snap/f1r3gaze/common"
work=$(mktemp -d "$HOME/snap/f1r3gaze/common/f1r3gaze-smoke.XXXXXX")
server_pid=
cleanup() {
  if [[ -n "$server_pid" ]]; then kill "$server_pid" 2>/dev/null || true; fi
  rm -rf "$work"
}
trap cleanup EXIT
if ! newtab=$(snap run f1r3gaze.f1r3gaze \
  --profile "$work/profile" --headless gaze://newtab --timeout 5); then
  echo "Snap built-in page failed: $newtab" >&2; exit 1
fi
[[ "$newtab" == *'stage:  Running'* && "$newtab" == *'<h1>F1R3Gaze</h1>'* ]] || {
  echo "Snap could not run its built-in page: $newtab" >&2; exit 1;
}
mkdir "$work/site"
printf '<html><head><title>CI sandbox page</title></head><body><h1>network-ready</h1></body></html>\n' > "$work/site/index.html"
python3 -u "$(dirname "$0")/../linux/serve-smoke-page.py" "$work/site" "$work/port" &
server_pid=$!
for ((attempt = 0; attempt < 100; attempt++)); do
  [[ -s "$work/port" ]] && break
  kill -0 "$server_pid" 2>/dev/null || { echo 'Snap smoke HTTP server exited' >&2; exit 1; }
  sleep 0.1
done
[[ -s "$work/port" ]] || { echo 'Snap smoke HTTP server did not start' >&2; exit 1; }
if ! page=$(snap run f1r3gaze.f1r3gaze \
  --profile "$work/profile" --headless "http://127.0.0.1:$(cat "$work/port")/" --timeout 5); then
  echo "Snap local page failed: $page" >&2; exit 1
fi
[[ "$page" == *'stage:  Static'* && "$page" == *'<h1>network-ready</h1>'* ]] || {
  echo "Snap could not fetch the local page: $page" >&2; exit 1;
}
printf 'Nil\n' > "$work/example.rho"
snap run f1r3gaze.f1r3c compile "$work/example.rho" -o "$work/example.knf"
test -s "$work/example.knf"
snap run f1r3gaze.f1r3c inspect "$work/example.knf" | grep -Fxq 'Nil'

timeout -k 5s 90s xvfb-run -a bash "$(dirname "$0")/../linux/ci-window.sh" \
  snap run f1r3gaze.f1r3gaze --profile "$work/window-profile" gaze://newtab

sudo snap remove --purge f1r3gaze
! snap list f1r3gaze >/dev/null 2>&1
