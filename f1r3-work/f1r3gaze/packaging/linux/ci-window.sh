#!/usr/bin/env bash
# Called under xvfb-run to confirm the packaged app opens a real window.
set -euo pipefail
[[ $# -gt 0 ]] || { echo 'application command required' >&2; exit 2; }
command -v xdotool >/dev/null

# The CI host may advertise Wayland; the isolated display is X11. wgpu_context
# honors WGPU_BACKEND, so this also exercises Vulkan surface creation.
unset WAYLAND_DISPLAY WAYLAND_SOCKET
export XDG_SESSION_TYPE=x11 WGPU_BACKEND=vulkan
log=$(mktemp)
app_pid=
# shellcheck disable=SC2329 # Invoked by the EXIT trap.
cleanup() {
  if [[ -n "$app_pid" ]]; then
    kill "$app_pid" 2>/dev/null || true
    for ((attempt = 0; attempt < 50; attempt++)); do
      kill -0 "$app_pid" 2>/dev/null || break
      sleep 0.1
    done
    kill -KILL "$app_pid" 2>/dev/null || true
    wait "$app_pid" 2>/dev/null || true
  fi
  rm -f "$log"
}
trap cleanup EXIT
"$@" >"$log" 2>&1 &
app_pid=$!

for ((attempt = 0; attempt < 600; attempt++)); do
  if xdotool search --onlyvisible --name F1R3Gaze >/dev/null 2>&1; then
    echo 'F1R3Gaze opened a visible X11 window with the Vulkan backend selected'
    exit 0
  fi
  if ! kill -0 "$app_pid" 2>/dev/null; then
    echo 'F1R3Gaze exited before opening a window:' >&2
    cat "$log" >&2
    exit 1
  fi
  sleep 0.1
done
echo 'F1R3Gaze did not open a visible window within 60 seconds:' >&2
cat "$log" >&2
exit 1
