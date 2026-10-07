#!/usr/bin/env bash
# Source-level checks available on a Linux development host.
set -euo pipefail
project=$(cd "$(dirname "$0")/.." && pwd)
command -v shellcheck >/dev/null || { echo "shellcheck is required" >&2; exit 2; }
while IFS= read -r -d '' script; do
  bash -n "$script"
  shellcheck "$script"
done < <(find "$project/packaging" -name '*.sh' -print0)
python3 -m unittest discover -s "$project/packaging" -p 'test_*.py'
python3 - "$project" <<'PY'
import plistlib
import sys
from pathlib import Path
from xml.etree import ElementTree

project = Path(sys.argv[1])
for source in (
    "packaging/macos/Info.plist",
    "packaging/macos/entitlements.plist",
    "packaging/windows/f1r3gaze.wxs",
    "packaging/windows/organization.wxs.in",
    "packaging/macos/organization-distribution.xml.in",
):
    ElementTree.parse(project / source)
with (project / "packaging/macos/Info.plist").open("rb") as stream:
    info = plistlib.load(stream)
assert info["CFBundleExecutable"] == "f1r3gaze"
assert len(info["CFBundleURLTypes"]) == 1
assert info["CFBundleURLTypes"][0]["CFBundleURLSchemes"] == ["f1r3", "f1r3h"]
assert info["CFBundleURLTypes"][0]["CFBundleURLRole"] == "Viewer"
tree = ElementTree.parse(project / "packaging/windows/f1r3gaze.wxs")
sources = [
    element.attrib.get("Source", "")
    for element in tree.iter()
    if element.tag.endswith("File")
]
assert any("f1r3gaze.exe" in source for source in sources), sources
assert any("f1r3c.exe" in source for source in sources), sources
assert all("embers" not in source.lower() and "f1r3node" not in source.lower()
           for source in sources), sources
print("installer source metadata is valid")
PY
