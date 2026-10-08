#!/usr/bin/env bash
# Sign a complete repository staged from already-signed release RPMs.
# Usage: sign-metadata.sh TREE KEY_ID VERSION
# The UNSIGNED-STAGING marker stays until promotion_check.py verifies the
# complete catalog, staged bytes and signatures, then finalizes the tree.
set -euo pipefail
tree=${1:?staged repository directory required}
key=${2:?OpenPGP key ID required}
[[ -d "$tree" && -f "$tree/UNSIGNED-STAGING" ]] ||
  { echo "expected an unsigned staged repository tree: $tree" >&2; exit 2; }
command -v gpg >/dev/null || { echo "gpg is required" >&2; exit 2; }
for tool in rpmkeys python3; do
  command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 2; }
done
tree=$(cd "$tree" && pwd)
gpg --batch --list-secret-keys "$key" >/dev/null ||
  { echo "secret key is unavailable: $key" >&2; exit 2; }
here=$(cd "$(dirname "$0")" && pwd)
version=${3:?stable version required}
python3 "$here/verify-staging.py" --tree "$tree" --version "$version"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
gpg --batch --armor --export "$key" > "$work/public.asc"
test -s "$work/public.asc"
mkdir "$work/rpmdb"
rpmkeys --dbpath "$work/rpmdb" --import "$work/public.asc"
while IFS= read -r -d '' package; do
  rpmkeys --dbpath "$work/rpmdb" --define '_pkgverify_level all' --checksig "$package"
done < <(find "$tree/rpm" -type f -name '*.rpm' -print0)

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
echo "signed repository metadata; run promotion_check.py before publication"
