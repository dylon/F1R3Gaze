#!/usr/bin/env bash
# Sign the RPMs in a promotion copy of dist before creating repository indexes.
# Usage: sign-rpms.sh DIST VERSION KEY_ID
set -euo pipefail
dist=${1:?artifact directory required}
version=${2:?stable version required}
key=${3:?OpenPGP key ID required}
here=$(cd "$(dirname "$0")" && pwd)
[[ -d "$dist" ]] || { echo "artifact directory does not exist: $dist" >&2; exit 2; }
for tool in gpg rpmsign rpmkeys python3; do
  command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 2; }
done
dist=$(cd "$dist" && pwd)
python3 - "$here" "$dist" "$version" <<'PY'
import sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from stage import require_complete_packages
require_complete_packages(Path(sys.argv[2]), sys.argv[3])
PY
gpg --batch --list-secret-keys "$key" >/dev/null ||
  { echo "secret key is unavailable: $key" >&2; exit 2; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
gpg --batch --armor --export "$key" > "$work/public.asc"
test -s "$work/public.asc"
mkdir "$work/rpmdb"
rpmkeys --dbpath "$work/rpmdb" --import "$work/public.asc"
for package in "$dist"/*.rpm; do
  rpmsign --addsign --define '_openpgp_sign gpg' --key-id "$key" "$package"
  rpmkeys --dbpath "$work/rpmdb" --define '_pkgverify_level all' --checksig "$package"
done
echo "signed and verified release RPMs; stage repository indexes from these exact bytes"
