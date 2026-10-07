#!/usr/bin/env bash
# Run inside the matching Fedora, Rocky Linux or openSUSE container.
set -euo pipefail
version=${1:?version required}
# shellcheck source=/dev/null
source /etc/os-release
case "$ID" in
  fedora|rocky)
    dnf -y install curl git gcc gcc-c++ pkgconf-pkg-config \
      fontconfig-devel libxkbcommon-devel vulkan-loader-devel rpm-build \
      tar gzip perl-core make
    ;;
  opensuse-leap)
    zypper --non-interactive refresh
    zypper --non-interactive install curl git gcc gcc-c++ pkg-config \
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
if [[ "$ID" == opensuse-leap ]]; then
  zypper --non-interactive --no-gpg-checks install dist/*.rpm
else
  dnf -y install dist/*.rpm
fi
[[ "$(f1r3gaze --version)" == "f1r3gaze $version" ]]
test -x /usr/bin/f1r3c
if [[ "$ID" == opensuse-leap ]]; then
  zypper --non-interactive remove f1r3gaze
else
  dnf -y remove f1r3gaze
fi
! test -e /usr/bin/f1r3gaze
