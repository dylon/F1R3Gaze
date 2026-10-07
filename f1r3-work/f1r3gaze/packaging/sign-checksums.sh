#!/usr/bin/env bash
# Write dist/SHA256SUMS and, when a key is available, a detached ASCII-armoured
# signature dist/SHA256SUMS.asc. Linux packages are verified this way:
#   gpg --verify SHA256SUMS.asc SHA256SUMS && sha256sum -c SHA256SUMS
# Env: GPG_PRIVATE_KEY (armoured secret key), GPG_PASSPHRASE (optional).
set -euo pipefail
cd "${1:-dist}"
rm -f SHA256SUMS SHA256SUMS.asc
checksums=$(mktemp)
gpg_home=''
trap 'rm -f "$checksums"; [[ -z "$gpg_home" ]] || rm -rf "$gpg_home"' EXIT
find . -maxdepth 1 -type f ! -name SHA256SUMS ! -name SHA256SUMS.asc -print0 |
  LC_ALL=C sort -z | xargs -0 -r sha256sum > "$checksums"
[[ -s "$checksums" ]] || { echo "no release artifacts to checksum" >&2; exit 2; }
mv "$checksums" SHA256SUMS
chmod 644 SHA256SUMS
if [ -n "${GPG_PRIVATE_KEY:-}" ]; then
  gpg_home=$(mktemp -d)
  GNUPGHOME=$gpg_home
  export GNUPGHOME
  printf '%s' "$GPG_PRIVATE_KEY" | gpg --batch --import 2>/dev/null
  gpg --batch --yes --pinentry-mode loopback --passphrase "${GPG_PASSPHRASE:-}" \
      --armor --detach-sign --output SHA256SUMS.asc SHA256SUMS
  echo "signed SHA256SUMS"
else
  echo "GPG_PRIVATE_KEY not set: SHA256SUMS is unsigned" >&2
fi
