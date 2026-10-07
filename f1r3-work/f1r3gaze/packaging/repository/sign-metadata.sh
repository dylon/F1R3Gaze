#!/usr/bin/env bash
# Sign a staged repository tree with an already-provisioned OpenPGP key.
# Usage: sign-metadata.sh TREE KEY_ID
# The UNSIGNED-STAGING marker stays in place until install/upgrade testing
# and RPM package signing have been completed for production publication.
set -euo pipefail
tree=${1:?staged repository directory required}
key=${2:?OpenPGP key ID required}
[[ -d "$tree" && -f "$tree/UNSIGNED-STAGING" ]] ||
  { echo "expected an unsigned staged repository tree: $tree" >&2; exit 2; }
command -v gpg >/dev/null || { echo "gpg is required" >&2; exit 2; }
tree=$(cd "$tree" && pwd)
gpg --batch --list-secret-keys "$key" >/dev/null ||
  { echo "secret key is unavailable: $key" >&2; exit 2; }

sign_detached() {
  local input=$1 output=$2
  gpg --batch --yes --armor --local-user "$key" --detach-sign \
    --output "$output" "$input"
  gpg --batch --verify "$output" "$input"
}

if [[ -f "$tree/apt/dists/stable/Release" ]]; then
  release=$tree/apt/dists/stable/Release
  sign_detached "$release" "$release.gpg"
  gpg --batch --yes --armor --local-user "$key" --clearsign \
    --output "${release%Release}InRelease" "$release"
  gpg --batch --verify "${release%Release}InRelease"
fi
while IFS= read -r -d '' database; do
  sign_detached "$database" "${database%.tar.gz}.sig"
done < <(find "$tree/arch" -type f -name '*.db.tar.gz' -print0 2>/dev/null)
while IFS= read -r -d '' package; do
  sign_detached "$package" "$package.sig"
done < <(find "$tree/arch" -type f -name '*.pkg.tar.zst' -print0 2>/dev/null)
while IFS= read -r -d '' metadata; do
  sign_detached "$metadata" "$metadata.asc"
done < <(find "$tree/rpm" -type f -name 'repomd.xml' -print0 2>/dev/null)
echo "signed available repository metadata; production readiness is still pending"
