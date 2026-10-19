#!/usr/bin/env bash
# Avoid a FUSE requirement on CI while preserving a pinned AppImage runtime.
set -euo pipefail
: "${APPIMAGETOOL_IMAGE:?set APPIMAGETOOL_IMAGE to the verified tool}"
: "${APPIMAGETOOL_RUNTIME_FILE:?set APPIMAGETOOL_RUNTIME_FILE to the verified runtime}"
exec "$APPIMAGETOOL_IMAGE" --appimage-extract-and-run --runtime-file "$APPIMAGETOOL_RUNTIME_FILE" "$@"
