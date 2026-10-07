#!/usr/bin/env bash
# Run inside the matching Fedora, Rocky Linux or openSUSE container.
set -euo pipefail
version=${1:?version required}
[[ $version =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] ||
  { echo "expected stable X.Y.Z version: $version" >&2; exit 2; }
IFS=. read -r major minor patch <<< "$version"
if ((patch > 0)); then
  older=$major.$minor.$((patch - 1))
elif ((minor > 0)); then
  older=$major.$((minor - 1)).1
elif ((major > 0)); then
  older=$((major - 1)).1.1
else
  echo 'version 0.0.0 has no earlier upgrade fixture' >&2
  exit 2
fi
# shellcheck source=/dev/null
source /etc/os-release
case "$ID" in
  fedora|rocky)
    if [[ "$ID" == fedora ]]; then suffix=fc${VERSION_ID%%.*}; else suffix=el${VERSION_ID%%.*}; fi
    dnf -y install git gcc gcc-c++ pkgconf-pkg-config python3 \
      fontconfig-devel libxkbcommon-devel vulkan-loader-devel rpm-build \
      tar gzip perl-core make
    command -v curl >/dev/null || dnf -y install curl-minimal
    ;;
  opensuse-leap)
    suffix=opensuse${VERSION_ID%%.*}
    zypper --non-interactive refresh
    zypper --non-interactive install curl git gcc gcc-c++ pkg-config python3 \
      fontconfig-devel libxkbcommon-devel vulkan-devel rpm-build \
      tar gzip perl make
    ;;
  *) echo "unsupported RPM CI image: $ID" >&2; exit 2 ;;
esac
curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs |
  sh -s -- -y --profile minimal --default-toolchain 1.95.0
# shellcheck source=/dev/null
source "$HOME/.cargo/env"
cargo build --locked --release -p gaze-shell -p f1r3c
packaging/linux/build.sh --format rpm --version "$version" --out dist
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
args=(--define "_topdir $work" --define "gaze_version $older"
  --define "gaze_bin $(pwd)/target/release" --define "gaze_project $(pwd)"
  --define "dist .$suffix" --define 'debug_package %{nil}')
if [[ "$ID" == opensuse-leap ]]; then args+=(--define 'gaze_suse 1'); fi
rpmbuild -bb packaging/linux/rpm/f1r3gaze.spec "${args[@]}"
older_package=$work/RPMS/$(uname -m)/f1r3gaze-$older-1.$suffix.$(uname -m).rpm
test -s "$older_package"
if [[ "$ID" == opensuse-leap ]]; then
  zypper --non-interactive --no-gpg-checks install "$older_package"
  [[ "$(rpm -q --qf '%{VERSION}' f1r3gaze)" == "$older" ]]
  zypper --non-interactive --no-gpg-checks install dist/*.rpm
else
  dnf -y install "$older_package"
  [[ "$(rpm -q --qf '%{VERSION}' f1r3gaze)" == "$older" ]]
  dnf -y install dist/*.rpm
fi
[[ "$(rpm -q --qf '%{VERSION}' f1r3gaze)" == "$version" ]]
[[ "$(f1r3gaze --version)" == "f1r3gaze $version" ]]
test -x /usr/bin/f1r3c
if [[ "$ID" == opensuse-leap ]]; then
  zypper --non-interactive remove f1r3gaze
else
  dnf -y remove f1r3gaze
fi
! test -e /usr/bin/f1r3gaze
