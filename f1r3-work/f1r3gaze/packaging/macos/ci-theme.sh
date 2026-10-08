#!/usr/bin/env bash
# Compare winit's live AppKit appearance with the runner's global preference.
set -euo pipefail
appearance=$(defaults read -g AppleInterfaceStyle 2>/dev/null || true)
case "$appearance" in
  Dark) expected=dark ;;
  *) expected=light ;;
esac
cargo run --locked --release -p gaze-shell --features gaze-shell/os-keyring \
  --example native_system_theme -- "$expected"
