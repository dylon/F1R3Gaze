#!/usr/bin/env bash
# On Linux, cross-build PE executables and package the portable Windows ZIP.
# Native Windows CI uses package.ps1 without -ZipOnly to build the MSI too.
set -euo pipefail
version=${1:?stable X.Y.Z version required}
out=${2:-dist}
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  { echo "version must be stable X.Y.Z" >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
project=$(cd "$here/../.." && pwd)
expected=$(awk '/^\[workspace.package\]/{section=1;next} /^\[/{section=0} section && /^version = / {gsub(/"/,"",$3);print $3;exit}' "$project/Cargo.toml")
[[ "$version" == "$expected" ]] ||
  { echo "version $version differs from Cargo workspace version $expected" >&2; exit 2; }
command -v cargo-xwin >/dev/null || { echo "cargo-xwin is required" >&2; exit 2; }
command -v pwsh >/dev/null || { echo "PowerShell is required" >&2; exit 2; }
target_dir=${CARGO_TARGET_DIR:-$project/target}
(
  cd "$project"
  cargo xwin build --locked --release --target x86_64-pc-windows-msvc \
    --target-dir "$target_dir" -p gaze-shell -p f1r3c \
    --features gaze-shell/os-keyring
  python3 "$here/../verify_binary.py" \
    "$target_dir/x86_64-pc-windows-msvc/release/f1r3gaze.exe" \
    "$target_dir/x86_64-pc-windows-msvc/release/f1r3c.exe"
  pwsh -NoProfile -File "$here/package.ps1" -Version "$version" \
    -Bin "$target_dir/x86_64-pc-windows-msvc/release" -Dist "$out" -ZipOnly
)
