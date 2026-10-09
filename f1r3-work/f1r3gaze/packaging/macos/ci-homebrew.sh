#!/usr/bin/env bash
# Install the generated cask from a temporary local tap against the built DMG.
set -euo pipefail

[[ $# -ge 1 ]] || { echo "version required" >&2; exit 2; }
version=$1
dist=dist
if [[ $# -ge 2 ]]; then dist=$2; fi
dist=$(cd "$dist" && pwd)
[[ -s "$dist/F1R3Gaze-$version-macos-universal.dmg" ]] || {
  echo "Homebrew smoke test requires the built DMG" >&2
  exit 2
}
command -v brew >/dev/null || { echo "Homebrew is unavailable" >&2; exit 2; }

export HOMEBREW_NO_AUTO_UPDATE=1
export HOMEBREW_NO_ANALYTICS=1
export HOMEBREW_NO_INSTALL_CLEANUP=1

tap_name=f1r3fly/ci-gaze
cask_name="$tap_name/f1r3gaze"
tap_dir=$(mktemp -d "$RUNNER_TEMP/f1r3gaze-brew-tap.XXXXXX")
app=/Applications/F1R3Gaze.app
cli="$(brew --prefix)/bin/f1r3c"
tapped=0
cleanup() {
  if [[ "$tapped" == 1 ]]; then
    brew uninstall --cask "$cask_name" >/dev/null 2>&1 || true
    brew untap "$tap_name" >/dev/null 2>&1 || true
  fi
  rm -rf "$tap_dir"
}
trap cleanup EXIT

[[ ! -e "$app" && ! -L "$app" ]] || {
  echo "Homebrew smoke test requires a clean application destination" >&2
  exit 2
}
[[ ! -e "$cli" && ! -L "$cli" ]] || {
  echo "Homebrew smoke test requires a clean f1r3c link" >&2
  exit 2
}

python3 packaging/release_metadata.py homebrew --version "$version" \
  --dist "$dist" --base-url "file://$dist" \
  --out "$tap_dir/Casks/f1r3gaze.rb"
git -C "$tap_dir" init -q
git -C "$tap_dir" add Casks/f1r3gaze.rb
git -C "$tap_dir" -c user.name='F1R3Gaze CI' \
  -c user.email='ci@example.invalid' commit -q -m 'Add F1R3Gaze cask'

brew tap "$tap_name" "$tap_dir"
tapped=1
brew install --cask --require-sha "$cask_name"
app_version=$("$app/Contents/MacOS/f1r3gaze" --version)
[[ "$app_version" == "f1r3gaze $version" ]] || {
  echo "Homebrew app reported an unexpected version: $app_version" >&2
  exit 1
}
cli_version=$("$cli" --version)
[[ "$cli_version" == "f1r3c $version" ]] || {
  echo "Homebrew f1r3c reported an unexpected version: $cli_version" >&2
  exit 1
}
echo "Homebrew installed F1R3Gaze and f1r3c $version"

brew uninstall --cask "$cask_name"
[[ ! -e "$app" && ! -L "$app" ]] || {
  echo "Homebrew did not remove the application" >&2
  exit 1
}
[[ ! -e "$cli" && ! -L "$cli" ]] || {
  echo "Homebrew did not remove the f1r3c link" >&2
  exit 1
}
brew untap "$tap_name"
tapped=0
