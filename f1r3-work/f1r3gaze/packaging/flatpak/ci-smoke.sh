#!/usr/bin/env bash
# Build, install, launch and remove a native Flatpak bundle on a CI runner.
set -euo pipefail
version=${1:?version required}
here=$(cd "$(dirname "$0")" && pwd)
project=$(cd "$here/../.." && pwd)
arch=$(uname -m)
case "$arch" in
  x86_64|aarch64) ;;
  *) echo "unsupported Flatpak host architecture: $arch" >&2; exit 2 ;;
esac
cd "$project"
input=dist/flatpak-input
build=dist/flatpak-build
repo=dist/flatpak-repo
bundle="dist/F1R3Gaze-$version-$arch.flatpak"
[[ ! -e "$input" && ! -e "$build" && ! -e "$repo" && ! -e "$bundle" ]] ||
  { echo "Flatpak build output already exists" >&2; exit 2; }
"$here/stage.sh" target/release "$input"
flatpak remote-add --user --if-not-exists flathub \
  https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak-builder --user --force-clean --install-deps-from=flathub \
  --repo="$repo" "$build" "$here/io.f1r3fly.F1R3Gaze.json"
flatpak build-bundle "$repo" "$bundle" io.f1r3fly.F1R3Gaze \
  --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo
test -s "$bundle"
flatpak --user install --noninteractive -y "$bundle"
[[ "$(flatpak run --user io.f1r3fly.F1R3Gaze --version)" == "f1r3gaze $version" ]]
[[ "$(flatpak run --user --command=f1r3c io.f1r3fly.F1R3Gaze --version)" == "f1r3c $version" ]]

# Exercise the installed application and compiler inside the actual sandbox.
work=$(mktemp -d "$HOME/f1r3gaze-flatpak-smoke.XXXXXX")
server_pid=
cleanup() {
  if [[ -n "$server_pid" ]]; then kill "$server_pid" 2>/dev/null || true; fi
  rm -rf "$work"
}
trap cleanup EXIT
if ! newtab=$(flatpak run --user io.f1r3fly.F1R3Gaze \
  --profile "$work/profile" --headless gaze://newtab --timeout 5); then
  echo "Flatpak built-in page failed: $newtab" >&2; exit 1
fi
[[ "$newtab" == *'stage:  Running'* && "$newtab" == *'<h1>F1R3Gaze</h1>'* ]] || {
  echo "Flatpak could not run its built-in page: $newtab" >&2; exit 1;
}
mkdir "$work/site"
printf '<html><head><title>CI sandbox page</title></head><body><h1>network-ready</h1></body></html>\n' > "$work/site/index.html"
python3 -u "$here/../linux/serve-smoke-page.py" "$work/site" "$work/port" &
server_pid=$!
for ((attempt = 0; attempt < 100; attempt++)); do
  [[ -s "$work/port" ]] && break
  kill -0 "$server_pid" 2>/dev/null || { echo 'Flatpak smoke HTTP server exited' >&2; exit 1; }
  sleep 0.1
done
[[ -s "$work/port" ]] || { echo 'Flatpak smoke HTTP server did not start' >&2; exit 1; }
if ! page=$(flatpak run --user io.f1r3fly.F1R3Gaze \
  --profile "$work/profile" --headless "http://127.0.0.1:$(cat "$work/port")/" --timeout 5); then
  echo "Flatpak local page failed: $page" >&2; exit 1
fi
[[ "$page" == *'stage:  Static'* && "$page" == *'<h1>network-ready</h1>'* ]] || {
  echo "Flatpak could not fetch the local page: $page" >&2; exit 1;
}
printf 'Nil\n' > "$work/example.rho"
flatpak run --user --command=f1r3c io.f1r3fly.F1R3Gaze \
  compile "$work/example.rho" -o "$work/example.knf"
test -s "$work/example.knf"
flatpak run --user --command=f1r3c io.f1r3fly.F1R3Gaze \
  inspect "$work/example.knf" | grep -Fxq 'Nil'

flatpak --user uninstall --noninteractive -y io.f1r3fly.F1R3Gaze
! flatpak --user info io.f1r3fly.F1R3Gaze >/dev/null 2>&1
