#!/usr/bin/env bash
# Package previously built, native F1R3Gaze binaries.
set -euo pipefail

usage() {
  echo "usage: $0 --format deb|arch|rpm|tar|appimage --version X.Y.Z [--bin DIR] [--out DIR]" >&2
}

format='' version='' bin='' out=dist
while (($#)); do
  case "$1" in
    --format|--version|--bin|--out)
      (($# >= 2)) || { usage; exit 2; }
      case "$1" in
        --format) format=$2 ;;
        --version) version=$2 ;;
        --bin) bin=$2 ;;
        --out) out=$2 ;;
      esac
      shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage; exit 2 ;;
  esac
done
[[ -n "$format" && -n "$version" ]] || { usage; exit 2; }
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] ||
  { echo "invalid version: $version" >&2; exit 2; }

here=$(cd "$(dirname "$0")" && pwd)
project=$(cd "$here/../.." && pwd)
license=$(cd "$project/../.." && pwd)/LICENSE
expected=$(awk '/^\[workspace.package\]/{section=1;next} /^\[/{section=0} section && /^version = / {gsub(/"/,"",$3);print $3;exit}' "$project/Cargo.toml")
[[ "$version" == "$expected" ]] ||
  { echo "version $version differs from Cargo workspace version $expected" >&2; exit 2; }
bin=$(cd "${bin:-$project/target/release}" && pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)
for name in f1r3gaze f1r3c; do
  [[ -f "$bin/$name" && -x "$bin/$name" ]] ||
    { echo "missing executable: $bin/$name" >&2; exit 2; }
done
[[ -f "$license" ]] || { echo "missing license: $license" >&2; exit 2; }

arch=$(uname -m)
case "$arch" in
  x86_64) deb_arch=amd64; elf_machine='Advanced Micro Devices X86-64' ;;
  aarch64) deb_arch=arm64; elf_machine='AArch64' ;;
  *) echo "unsupported host architecture: $arch" >&2; exit 2 ;;
esac
for name in f1r3gaze f1r3c; do
  machine=$(readelf -h "$bin/$name" | awk -F: '/Machine:/{gsub(/^[ \t]+/,"",$2);print $2}')
  [[ "$machine" == "$elf_machine" ]] ||
    { echo "$name targets $machine, host is $arch" >&2; exit 2; }
done

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
stage() {
  local prefix=$1 size
  install -Dm755 "$bin/f1r3gaze" "$prefix/bin/f1r3gaze"
  install -Dm755 "$bin/f1r3c" "$prefix/bin/f1r3c"
  install -Dm644 "$here/f1r3gaze.desktop" "$prefix/share/applications/f1r3gaze.desktop"
  install -Dm644 "$license" "$prefix/share/licenses/f1r3gaze/LICENSE"
  for size in 16 32 48 64 128 256 512; do
    install -Dm644 "$here/../icons/$size.png" "$prefix/share/icons/hicolor/${size}x${size}/apps/f1r3gaze.png"
  done
  install -Dm644 "$here/../icons/f1r3gaze.svg" "$prefix/share/icons/hicolor/scalable/apps/f1r3gaze.svg"
}

case "$format" in
  deb)
    command -v dpkg-deb >/dev/null || { echo "dpkg-deb is required" >&2; exit 2; }
    root=$work/deb
    stage "$root/usr"
    mkdir -p "$root/DEBIAN"
    size=$(du -sk "$root/usr" | cut -f1)
    cat > "$root/DEBIAN/control" <<EOF
Package: f1r3gaze
Version: $version
Section: web
Priority: optional
Architecture: $deb_arch
Installed-Size: $size
Depends: libc6 (>= 2.35), libgcc-s1, libfontconfig1, libxkbcommon0, libvulkan1 | libgl1
Maintainer: F1R3FLY.io <engineering@f1r3fly.io>
Homepage: https://github.com/F1R3FLY-io/F1R3Gaze
Description: F1R3Gaze browser and f1r3lang compiler
 F1R3Gaze renders sites with its in-process RSpace. F1R3Node is an
 optional, separately installed service; Embers is not bundled.
EOF
    dpkg-deb --root-owner-group --build "$root" "$out/f1r3gaze_${version}_${deb_arch}.deb"
    ;;
  arch)
    command -v makepkg >/dev/null || { echo "makepkg is required" >&2; exit 2; }
    mkdir -p "$work/arch"
    cp "$here/arch/PKGBUILD" "$work/arch/PKGBUILD"
    cp /etc/makepkg.conf "$work/arch/makepkg.conf"
    # Arch Linux ARM images can still default to xz; releases use one
    # predictable pacman artifact name on both architectures.
    printf "\nPKGEXT='.pkg.tar.zst'\n" >> "$work/arch/makepkg.conf"
    (
      cd "$work/arch"
      F1R3GAZE_BIN_DIR="$bin" F1R3GAZE_PROJECT_DIR="$project" \
        F1R3GAZE_VERSION="$version" PKGDEST="$out" \
        SRCDEST="$work/arch" SRCPKGDEST="$work/arch" BUILDDIR="$work/arch" LOGDEST="$work/arch" \
        makepkg --config "$work/arch/makepkg.conf" --nodeps --noconfirm --force
    )
    ;;
  rpm)
    command -v rpmbuild >/dev/null || { echo "rpmbuild is required" >&2; exit 2; }
    # Own the distro suffix rather than relying on image-specific RPM macros.
    # shellcheck source=/dev/null
    source /etc/os-release
    case "$ID" in
      fedora) rpm_channel="fc${VERSION_ID%%.*}" ;;
      rocky) rpm_channel="el${VERSION_ID%%.*}" ;;
      opensuse-leap) rpm_channel="opensuse${VERSION_ID%%.*}"; rpm_suse=1 ;;
      *) echo "unsupported RPM host: $ID" >&2; exit 2 ;;
    esac
    mkdir -p "$work/rpm/BUILD" "$work/rpm/BUILDROOT" "$work/rpm/RPMS" "$work/rpm/SOURCES" "$work/rpm/SPECS" "$work/rpm/SRPMS"
    rpm_args=(--define "_topdir $work/rpm" --define "gaze_version $version"
      --define "gaze_bin $bin" --define "gaze_project $project"
      --define "dist .$rpm_channel" --define "debug_package %{nil}")
    if [[ ${rpm_suse:-0} -eq 1 ]]; then
      rpm_args+=(--define "gaze_suse 1")
    fi
    rpmbuild -bb "$here/rpm/f1r3gaze.spec" "${rpm_args[@]}"
    find "$work/rpm/RPMS" -name '*.rpm' -exec cp -v {} "$out/" \;
    ;;
  tar)
    root=$work/f1r3gaze-$version
    stage "$root"
    tar -C "$work" -czf "$out/f1r3gaze-$version-linux-$arch.tar.gz" "f1r3gaze-$version"
    ;;
  appimage)
    tool=${APPIMAGETOOL:-$(command -v appimagetool || true)}
    [[ -n "$tool" && -x "$tool" ]] ||
      { echo "appimagetool is required; set APPIMAGETOOL to a pinned executable" >&2; exit 2; }
    root=$work/F1R3Gaze.AppDir
    stage "$root/usr"
    cp "$here/f1r3gaze.desktop" "$root/f1r3gaze.desktop"
    cp "$here/../icons/256.png" "$root/f1r3gaze.png"
    cat > "$root/AppRun" <<'EOF'
#!/bin/sh
root=$(dirname "$(readlink -f "$0")")
exec "$root/usr/bin/f1r3gaze" "$@"
EOF
    chmod 755 "$root/AppRun"
    ARCH="$arch" "$tool" --no-appstream "$root" "$out/F1R3Gaze-$version-$arch.AppImage"
    ;;
  *) echo "unsupported format: $format" >&2; exit 2 ;;
esac
