#!/usr/bin/env bash
# Exercise a real NSOpenPanel Powerbox grant for a profile outside App Sandbox.
set -euo pipefail
version=${1:?version required}
dist=${2:-dist}
[[ "$(uname -s)" == Darwin ]] || { echo 'this smoke requires macOS' >&2; exit 2; }
: "${RUNNER_TEMP:?this smoke requires a disposable GitHub runner}"
command -v xcodegen >/dev/null || { echo 'xcodegen is required' >&2; exit 2; }
command -v xcodebuild >/dev/null || { echo 'Xcode is required' >&2; exit 2; }
dist=$(cd "$dist" && pwd)
dmg="$dist/F1R3Gaze-$version-macos-universal.dmg"
[[ -s "$dmg" ]] || { echo "missing universal DMG: $dmg" >&2; exit 2; }

here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d "$RUNNER_TEMP/f1r3gaze-store-native-panel.XXXXXX")
bundle_id="io.f1r3fly.f1r3gaze.sandboxci.xcui$(uuidgen | tr '[:upper:]' '[:lower:]' | tr -d '-')"
container="$HOME/Library/Containers/$bundle_id"
mount="$work/mount"
attached=0
app="$work/output/F1R3Gaze.app"
log="$RUNNER_TEMP/f1r3gaze-store-native-panel-$(uname -m).log"
cleanup() {
  local code=$?
  if [[ "$attached" == 1 ]]; then hdiutil detach "$mount" >/dev/null 2>&1 || true; fi
  pkill -f "$app/Contents/MacOS/f1r3gaze" 2>/dev/null || true
  rm -rf "$container" "$work"
  exit "$code"
}
trap cleanup EXIT
[[ ! -e "$container" ]] || { echo "test container already exists: $container" >&2; exit 2; }

mkdir "$mount"
hdiutil attach -nobrowse -readonly -mountpoint "$mount" "$dmg" >/dev/null
attached=1
source="$work/external-profile"
"$mount/F1R3Gaze.app/Contents/MacOS/f1r3gaze" --profile "$source" wallet list >/dev/null
[[ -s "$source/data/layout.json" ]] || { echo 'unsandboxed DMG did not initialize the source profile' >&2; exit 1; }
printf 'config from Developer ID DMG\n' > "$source/config/ci-import-sentinel"
printf 'state from Developer ID DMG\n' > "$source/state/ci-import-sentinel"
hdiutil detach "$mount" >/dev/null
attached=0

bash "$here/store-package.sh" "$version" "$dist" "$work/output" "$bundle_id"
target="$container/Data/Library/Application Support/io.f1r3fly.f1r3gaze"
[[ ! -e "$target" ]] || { echo 'Store target exists before first launch' >&2; exit 1; }
cp -R "$here/store-ui-test" "$work/ui-test"
python3 - "$app" "$source" "$target" "$work/ui-test/UITests/Fixture.swift" <<'PY'
import json
from pathlib import Path
import sys

app, source, target, output = sys.argv[1:]
Path(output).write_text(
    "enum Fixture {\n"
    f"    static let appPath = {json.dumps(app)}\n"
    f"    static let sourcePath = {json.dumps(source)}\n"
    f"    static let targetPath = {json.dumps(target)}\n"
    "}\n",
    encoding="utf-8",
)
PY

(cd "$work/ui-test" && xcodegen generate)
if ! xcodebuild test \
  -project "$work/ui-test/F1R3GazeStoreImportUITest.xcodeproj" \
  -scheme UITestHost -destination 'platform=macOS' \
  -derivedDataPath "$work/derived" -resultBundlePath "$work/results.xcresult" \
  -parallel-testing-enabled NO CODE_SIGNING_ALLOWED=NO > "$log" 2>&1; then
  tail -n 160 "$log" >&2
  exit 1
fi
cmp "$source/config/ci-import-sentinel" "$target/config/ci-import-sentinel"
cmp "$source/state/ci-import-sentinel" "$target/state/ci-import-sentinel"
echo "native macOS folder selection imported an external profile into $bundle_id"
