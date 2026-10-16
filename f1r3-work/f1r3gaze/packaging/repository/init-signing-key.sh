#!/usr/bin/env bash
# Create a passphrase-protected primary key and signing subkey outside Git.
# GnuPG's pinentry asks for the passphrase; this script never reads or stores it.
set -euo pipefail

usage() {
  echo 'usage: init-signing-key.sh --home ABSOLUTE_NEW_DIRECTORY --uid "Release Name <email>"' >&2
  exit 2
}

home=''
uid=''
while (($#)); do
  case "$1" in
    --home) (($# >= 2)) || usage; home=$2; shift 2 ;;
    --uid) (($# >= 2)) || usage; uid=$2; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n $home && -n $uid && $home == /* ]] || usage
[[ -t 0 && -t 1 ]] || {
  echo 'run this interactively so GnuPG can prompt for a passphrase' >&2
  exit 2
}
command -v gpg >/dev/null || { echo 'gpg is required' >&2; exit 2; }

here=$(cd "$(dirname "$0")" && pwd -P)
repo=$(git -C "$here" rev-parse --show-toplevel)
parent=$(cd "$(dirname "$home")" && pwd -P)
home=$parent/$(basename "$home")
case "$home" in
  "$repo"|"$repo"/*)
    echo 'the private key home must be outside the Git checkout' >&2
    exit 2 ;;
esac
[[ ! -e $home && ! -L $home ]] || {
  echo "refusing to reuse an existing path: $home" >&2
  exit 2
}

umask 077
mkdir -m 700 -- "$home"
GPG_TTY=$(tty)
export GPG_TTY
echo 'GnuPG will ask for a new passphrase through pinentry.'
gpg --homedir "$home" --quick-generate-key "$uid" rsa4096 cert 2y
fingerprint=$(gpg --homedir "$home" --batch --with-colons \
  --fingerprint --list-secret-keys |
  awk -F: '$1 == "sec" { count++; want = 1; next }
            want && $1 == "fpr" { print toupper($10); want = 0 }
            END { if (count != 1) exit 3 }') || {
  echo 'expected exactly one primary signing key' >&2
  exit 2
}
[[ $fingerprint =~ ^([0-9A-F]{40}|[0-9A-F]{64})$ ]] || {
  echo 'GnuPG returned an invalid primary fingerprint' >&2
  exit 2
}
gpg --homedir "$home" --quick-add-key "$fingerprint" rsa4096 sign 1y
gpg --homedir "$home" --batch --armor --export "$fingerprint" > "$home/public.asc"
test -s "$home/public.asc"
echo "Key home: $home"
echo "Public key: $home/public.asc"
echo "Primary fingerprint: $fingerprint"
echo 'Back up the protected primary key, signing subkey and passphrase separately.'
echo 'Export only the signing subkey to CI after the backup is verified.'
