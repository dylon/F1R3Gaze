#!/usr/bin/env bash
# Fetch fixed upstream releases and verify their GitHub-published SHA-256 digests.
set -euo pipefail
out=${1:?output directory required}
arch=$(uname -m)
case "$arch" in
  x86_64)
    tool_sha=ed4ce84f0d9caff66f50bcca6ff6f35aae54ce8135408b3fa33abfc3cb384eb0
    runtime_sha=2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d ;;
  aarch64)
    tool_sha=f0837e7448a0c1e4e650a93bb3e85802546e60654ef287576f46c71c126a9158
    runtime_sha=00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444 ;;
  *) echo "unsupported AppImage architecture: $arch" >&2; exit 2 ;;
esac
mkdir -p "$out"
out=$(cd "$out" && pwd)
tool="$out/appimagetool-$arch.AppImage"
runtime="$out/runtime-$arch"
curl --fail --location --silent --show-error \
  "https://github.com/AppImage/appimagetool/releases/download/1.9.1/appimagetool-$arch.AppImage" \
  --output "$tool"
curl --fail --location --silent --show-error \
  "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-$arch" \
  --output "$runtime"
printf '%s  %s\n' "$tool_sha" "$tool" "$runtime_sha" "$runtime" | sha256sum --check
chmod 755 "$tool" "$runtime"
printf 'APPIMAGETOOL_IMAGE=%s\nAPPIMAGETOOL_RUNTIME_FILE=%s\n' "$tool" "$runtime"
