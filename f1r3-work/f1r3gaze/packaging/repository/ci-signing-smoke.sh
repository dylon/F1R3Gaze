#!/usr/bin/env bash
# Exercise the production signing order on copies of real CI packages.
set -euo pipefail
dist=${1:?release package directory required}
version=${2:?stable version required}
here=$(cd "$(dirname "$0")" && pwd)
project=$(cd "$here/../.." && pwd)
dist=$(cd "$dist" && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export GNUPGHOME="$work/gnupg"
mkdir -m 700 "$GNUPGHOME"
gpg --batch --yes --pinentry-mode loopback --passphrase '' \
  --quick-generate-key 'F1R3Gaze CI <packaging-test@example.invalid>' rsa2048 sign 0
fingerprint=$(gpg --batch --with-colons --fingerprint --list-keys |
  awk -F: '$1 == "fpr" { print $10; exit }')
[[ "$fingerprint" =~ ^[[:xdigit:]]{40}$ ]] ||
  { echo "temporary signing key fingerprint is invalid" >&2; exit 2; }
gpg --batch --armor --export "$fingerprint" > "$work/public.asc"
cp -a "$dist" "$work/promotion-dist"
bash "$here/sign-rpms.sh" "$work/promotion-dist" "$version" "$fingerprint"
python3 "$here/stage.py" --dist "$work/promotion-dist" \
  --out "$work/promotion-tree" --version "$version" --require-complete
bash "$here/sign-metadata.sh" "$work/promotion-tree" "$fingerprint" "$version"
cd "$project"
python3 - "$work/promotion-dist" "$work/promotion-tree" "$work/public.asc" "$fingerprint" "$version" <<'PY'
import sys
from pathlib import Path
sys.path.insert(0, str(Path.cwd() / "packaging"))
from repository.promotion_check import verify_copies, verify_signatures

dist, tree, public = map(Path, sys.argv[1:4])
fingerprint, version = sys.argv[4:6]
verify_copies(dist, tree, version)
verify_signatures(tree, public, fingerprint, version)
assert (tree / "UNSIGNED-STAGING").is_file(), "ephemeral CI key must not finalize publication tree"
print(f"signed repository dry run passed for {version}")
PY
