#!/usr/bin/env bash
# Write dist/SHA256SUMS and, when a key is available, a detached ASCII-armoured
# signature dist/SHA256SUMS.asc. Linux packages are verified this way:
#   gpg --verify SHA256SUMS.asc SHA256SUMS && sha256sum -c SHA256SUMS
# Env: GPG_SIGNING_KEY_ID for an existing key, or GPG_PRIVATE_KEY
# (armoured secret key) and optional GPG_PASSPHRASE. Production releases
# supply GPG_EXPECTED_FINGERPRINT from an independently reviewed setting.
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
verify_expected_signer() {
  [[ -n "${GPG_EXPECTED_FINGERPRINT:-}" ]] || return 0
  local expected actual
  local -a selector=()
  expected=$(printf '%s' "$GPG_EXPECTED_FINGERPRINT" | tr '[:lower:]' '[:upper:]')
  [[ $expected =~ ^([0-9A-F]{40}|[0-9A-F]{64})$ ]] ||
    { echo "expected signing fingerprint is not full hexadecimal" >&2; exit 2; }
  [[ -z "${1:-}" ]] || selector=("$1")
  actual=$(gpg --batch --with-colons --fingerprint --list-secret-keys "${selector[@]}" |
    awk -F: '$1 == "sec" { count++; want = 1; next }
              want && $1 == "fpr" { print toupper($10); want = 0 }
              END { if (count != 1) exit 3 }') ||
    { echo "expected exactly one signing key" >&2; exit 2; }
  [[ $actual == "$expected" ]] ||
    { echo "signing key differs from expected primary fingerprint" >&2; exit 2; }
}
if [ -n "${GPG_SIGNING_KEY_ID:-}" ]; then
  verify_expected_signer "$GPG_SIGNING_KEY_ID"
  gpg --batch --yes --armor --local-user "$GPG_SIGNING_KEY_ID" \
      --detach-sign --output SHA256SUMS.asc SHA256SUMS
  echo "signed SHA256SUMS with existing key"
elif [ -n "${GPG_PRIVATE_KEY:-}" ]; then
  gpg_home=$(mktemp -d)
  GNUPGHOME=$gpg_home
  export GNUPGHOME
  printf '%s' "$GPG_PRIVATE_KEY" | gpg --batch --import 2>/dev/null
  verify_expected_signer
  printf '%s\n' "${GPG_PASSPHRASE:-}" | gpg --batch --yes \
      --pinentry-mode loopback --passphrase-fd 0 --armor \
      --detach-sign --output SHA256SUMS.asc SHA256SUMS
  echo "signed SHA256SUMS"
else
  echo "GPG_PRIVATE_KEY not set: SHA256SUMS is unsigned" >&2
fi
if [[ -f SHA256SUMS.asc ]]; then
  gpg --batch --verify SHA256SUMS.asc SHA256SUMS
fi
sha256sum -c SHA256SUMS
