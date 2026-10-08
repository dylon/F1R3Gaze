#!/usr/bin/env bash
# Recheck downloaded signed artifacts and build package-manager manifests.
set -euo pipefail
dist=${1:?signed release directory required}
tree=${2:?signed repository directory required}
version=${3:?stable version required}
base_url=${4:?release base URL required}
fingerprint=${5:?trusted primary fingerprint required}
metadata=${6:?fresh metadata output directory required}
[[ ! -e $metadata ]] || { echo "metadata output already exists: $metadata" >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
packaging=$(cd "$here/.." && pwd)
python3 "$here/promotion_check.py" --dist "$dist" \
  --tree "$tree" --catalog "$dist/catalog.json" \
  --public-key "$tree/f1r3gaze-signing-key.asc" \
  --fingerprint "$fingerprint" --version "$version"
python3 "$packaging/release_metadata.py" homebrew --version "$version" \
  --dist "$dist" --base-url "$base_url" \
  --out "$metadata/Casks/f1r3gaze.rb" --require-complete
python3 "$packaging/release_metadata.py" winget --version "$version" \
  --dist "$dist" --base-url "$base_url" \
  --out "$metadata/winget" --require-complete
python3 "$packaging/winget/validate.py" \
  "$metadata/winget/F1R3FLY.F1R3Gaze/$version" --dist "$dist"
