#!/usr/bin/env bash
# Install from a finalized, CI-only signed repository inside a distro image.
# The caller mounts the promotion-tree at /repo and this script read-only.
set -euo pipefail
version=${1:?stable version required}
repo=/repo
key=$repo/f1r3gaze-signing-key.asc
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  { echo "invalid version: $version" >&2; exit 2; }
[[ -s "$key" && ! -e "$repo/UNSIGNED-STAGING" ]] ||
  { echo "repository is not finalized with a public key" >&2; exit 2; }

# shellcheck source=/dev/null
source /etc/os-release
case "$ID" in
  debian|ubuntu)
    cat > /etc/apt/sources.list.d/f1r3gaze.sources <<EOF
Types: deb
URIs: file://$repo/apt
Suites: stable
Components: main
Signed-By: $key
EOF
    apt-get update
    DEBIAN_FRONTEND=noninteractive apt-get install -y f1r3gaze
    [[ $(dpkg-query -W -f='${Version}' f1r3gaze) == "$version" ]]
    ;;
  arch|archarm|archlinux)
    # Docker denies Landlock even though pacman supports it; this affects only
    # the disposable image, not the signed repository or customer guidance.
    sed -i '/^\[options\]/a DisableSandbox' /etc/pacman.conf
    pacman-key --init
    pacman-key --add "$key"
    fingerprint=$(gpg --batch --show-keys --with-colons --fingerprint "$key" |
      awk -F: '$1 == "fpr" {print $10; exit}')
    [[ -n $fingerprint ]]
    pacman-key --lsign-key "$fingerprint"
    # pacman expands $arch, not this shell.
    # shellcheck disable=SC2016
    printf '\n[f1r3gaze]\nSigLevel = Required DatabaseRequired\nServer = file://%s/arch/$arch\n' "$repo" >> /etc/pacman.conf
    pacman -Sy --noconfirm f1r3gaze 2>&1 | tee /tmp/f1r3gaze-pacman-install.log
    if grep -Fq 'signature format error' /tmp/f1r3gaze-pacman-install.log; then
      echo 'pacman reported an unsupported package signature format' >&2
      exit 1
    fi
    [[ $(pacman -Q f1r3gaze) == "f1r3gaze $version-1" ]]
    # Refresh only our signed files index; the image's upstream mirrors are
    # unrelated to this check and may be unavailable during a release.
    # shellcheck disable=SC2016
    cat > /tmp/f1r3gaze-files-pacman.conf <<EOF
[options]
Architecture = auto
SigLevel = Required DatabaseRequired
DisableSandbox
[f1r3gaze]
Server = file://$repo/arch/\$arch
EOF
    pacman --config /tmp/f1r3gaze-files-pacman.conf -Fy --noconfirm
    pacman --config /tmp/f1r3gaze-files-pacman.conf -Fl f1r3gaze |
      grep -F 'usr/bin/f1r3gaze'
    ;;
  fedora|rocky)
    if [[ $ID == fedora ]]; then channel=fc${VERSION_ID%%.*}; else channel=el${VERSION_ID%%.*}; fi
    cat > /etc/yum.repos.d/f1r3gaze.repo <<EOF
[f1r3gaze]
name=F1R3Gaze CI repository
baseurl=file://$repo/rpm/$channel/\$basearch
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=file://$key
EOF
    dnf -y install f1r3gaze
    [[ $(rpm -q --qf '%{VERSION}' f1r3gaze) == "$version" ]]
    ;;
  opensuse-leap)
    cat > /etc/zypp/repos.d/f1r3gaze.repo <<EOF
[f1r3gaze]
name=F1R3Gaze CI repository
baseurl=file://$repo/rpm/opensuse16/\$basearch
enabled=1
autorefresh=1
gpgcheck=1
repo_gpgcheck=1
pkg_gpgcheck=1
gpgkey=file://$key
EOF
    zypper --gpg-auto-import-keys --non-interactive refresh f1r3gaze
    zypper --non-interactive install f1r3gaze
    [[ $(rpm -q --qf '%{VERSION}' f1r3gaze) == "$version" ]]
    ;;
  *) echo "unsupported repository smoke distro: $ID" >&2; exit 2 ;;
esac

[[ $(f1r3gaze --version) == "f1r3gaze $version" ]]
test -x /usr/bin/f1r3c
case "$ID" in
  debian|ubuntu) DEBIAN_FRONTEND=noninteractive apt-get remove -y f1r3gaze ;;
  arch|archarm|archlinux) pacman -R --noconfirm f1r3gaze ;;
  fedora|rocky) dnf -y remove f1r3gaze ;;
  opensuse-leap) zypper --non-interactive remove f1r3gaze ;;
esac
test ! -e /usr/bin/f1r3gaze
echo "signed repository install and remove passed on $ID $(uname -m)"
