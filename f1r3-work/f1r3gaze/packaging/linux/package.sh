#!/usr/bin/env bash
# Compatibility wrapper for the three original Linux artifacts.
set -euo pipefail
version=${1:?version required}
bin=${2:?binary directory required}
here=$(cd "$(dirname "$0")" && pwd)
for format in deb tar appimage; do
  if [[ "$format" == appimage ]] && [[ -z "${APPIMAGETOOL:-}" ]] && ! command -v appimagetool >/dev/null; then
    echo "appimagetool unavailable; skipping AppImage" >&2
    continue
  fi
  "$here/build.sh" --format "$format" --version "$version" --bin "$bin" --out "${DIST:-dist}"
done
