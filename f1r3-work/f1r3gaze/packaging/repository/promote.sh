#!/usr/bin/env bash
# Produce a complete signed release and repository in a fresh output directory.
# The private key and passphrase enter through GPG_PRIVATE_KEY and GPG_PASSPHRASE;
# neither is copied into the output. The expected full primary fingerprint is
# supplied separately by the release owner.
set -euo pipefail

usage() {
  echo 'usage: promote.sh --dist DIR --out DIR --version X.Y.Z --base-url HTTPS_URL --fingerprint FULL_FPR' >&2
  exit 2
}

source_dist=''
out=''
version=''
base_url=''
fingerprint=''
while (($#)); do
  case "$1" in
    --dist) (($# >= 2)) || usage; source_dist=$2; shift 2 ;;
    --out) (($# >= 2)) || usage; out=$2; shift 2 ;;
    --version) (($# >= 2)) || usage; version=$2; shift 2 ;;
    --base-url) (($# >= 2)) || usage; base_url=$2; shift 2 ;;
    --fingerprint) (($# >= 2)) || usage; fingerprint=$2; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n $source_dist && -n $out && -n $version && -n $base_url && -n $fingerprint ]] || usage
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo 'version must be stable X.Y.Z' >&2; exit 2; }
fingerprint=$(printf '%s' "$fingerprint" | tr '[:lower:]' '[:upper:]')
[[ $fingerprint =~ ^([0-9A-F]{40}|[0-9A-F]{64})$ ]] ||
  { echo 'fingerprint must be the full primary fingerprint' >&2; exit 2; }
[[ $base_url == https://* ]] || { echo 'release base URL must use HTTPS' >&2; exit 2; }
[[ -n ${GPG_PRIVATE_KEY:-} && -n ${GPG_PASSPHRASE:-} ]] ||
  { echo 'a protected GPG_PRIVATE_KEY and GPG_PASSPHRASE are required' >&2; exit 2; }
for tool in gpg gpgconf rpmkeys rpmsign repo-add dpkg-scanpackages createrepo_c python3 sha256sum; do
  command -v "$tool" >/dev/null || { echo "$tool is required" >&2; exit 2; }
done
source_dist=$(cd "$source_dist" && pwd -P)
while IFS= read -r -d '' artifact; do
  [[ -f $artifact && ! -L $artifact ]] ||
    { echo "release source must contain only regular artifact files: $artifact" >&2; exit 2; }
done < <(find "$source_dist" -mindepth 1 -maxdepth 1 -print0)
out_parent=$(cd "$(dirname "$out")" && pwd -P)
out=$out_parent/$(basename "$out")
[[ ! -e $out && ! -L $out ]] || { echo "output already exists: $out" >&2; exit 2; }
case "$out_parent/" in
  "$source_dist/"*) echo 'output may not be inside the source artifact directory' >&2; exit 2 ;;
esac

here=$(cd "$(dirname "$0")" && pwd -P)
packaging=$(cd "$here/.." && pwd -P)
private=$(mktemp -d)
staging=''
cleanup() {
  if [[ -n ${GNUPGHOME:-} && $GNUPGHOME == "$private/gnupg" ]]; then
    gpgconf --kill gpg-agent >/dev/null 2>&1 || true
  fi
  rm -rf -- "$private"
  [[ -z $staging ]] || rm -rf -- "$staging"
}
trap cleanup EXIT
staging=$(mktemp -d "$out_parent/.f1r3gaze-promotion.XXXXXXXX")
export GNUPGHOME=$private/gnupg
mkdir -m 700 "$GNUPGHOME"
printf 'allow-preset-passphrase\nmax-cache-ttl 7200\ndefault-cache-ttl 7200\n' > "$GNUPGHOME/gpg-agent.conf"
gpgconf --launch gpg-agent
printf '%s' "$GPG_PRIVATE_KEY" | gpg --batch --import >/dev/null 2>&1
actual=$(gpg --batch --with-colons --fingerprint --list-secret-keys |
  awk -F: '$1 == "sec" { count++; want = 1; next }
            want && $1 == "fpr" { print toupper($10); want = 0 }
            END { if (count != 1) exit 3 }') ||
  { echo 'private key import must contain exactly one primary key' >&2; exit 2; }
[[ $actual == "$fingerprint" ]] ||
  { echo 'private key differs from the configured primary fingerprint' >&2; exit 2; }
gpg --batch --armor --export "$fingerprint" > "$private/public.asc"
test -s "$private/public.asc"

preset=$(gpgconf --list-dirs libexecdir)/gpg-preset-passphrase
[[ -x $preset ]] || { echo 'gpg-preset-passphrase is required for protected signing' >&2; exit 2; }
mapfile -t grips < <(gpg --batch --with-colons --with-keygrip --list-secret-keys "$fingerprint" |
  awk -F: '$1 == "grp" && $10 != "" { print $10 }')
((${#grips[@]} > 0)) || { echo 'signing key has no keygrip' >&2; exit 2; }
for grip in "${grips[@]}"; do
  printf '%s' "$GPG_PASSPHRASE" | "$preset" --preset "$grip"
done
printf '%s\n' "$version" > "$private/probe"
gpg --batch --yes --pinentry-mode error --local-user "$fingerprint" \
  --detach-sign --output "$private/probe.sig" "$private/probe"
gpg --batch --verify "$private/probe.sig" "$private/probe"

cp -a "$source_dist" "$staging/dist"
bash "$here/sign-rpms.sh" "$staging/dist" "$version" "$fingerprint"
python3 "$here/stage.py" --dist "$staging/dist" \
  --out "$staging/repository" --version "$version" --require-complete
bash "$here/sign-metadata.sh" "$staging/repository" "$fingerprint" "$version"
python3 "$packaging/release_metadata.py" catalog --dist "$staging/dist" \
  --version "$version" --base-url "$base_url" \
  --out "$staging/dist/catalog.json" --require-complete
GPG_SIGNING_KEY_ID=$fingerprint GPG_EXPECTED_FINGERPRINT=$fingerprint \
  bash "$packaging/sign-checksums.sh" "$staging/dist"
python3 "$here/promotion_check.py" --dist "$staging/dist" \
  --tree "$staging/repository" --catalog "$staging/dist/catalog.json" \
  --public-key "$private/public.asc" --fingerprint "$fingerprint" \
  --version "$version" --finalize
python3 "$here/promotion_check.py" --dist "$staging/dist" \
  --tree "$staging/repository" --catalog "$staging/dist/catalog.json" \
  --public-key "$private/public.asc" --fingerprint "$fingerprint" \
  --version "$version"

chmod 755 "$staging"
mv -- "$staging" "$out"
echo "signed release and repository are ready in $out"
