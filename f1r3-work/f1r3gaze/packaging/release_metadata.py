#!/usr/bin/env python3
"""Create release metadata from built artifacts; never guess a digest or version."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path
from urllib.parse import quote

VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
ARTIFACTS = (
    (re.compile(r"f1r3gaze_(?P<v>[^_]+)_(?P<a>amd64|arm64)\.deb"), "linux", "deb"),
    (
        re.compile(r"f1r3gaze-(?P<v>.+)-1-(?P<a>x86_64|aarch64)\.pkg\.tar\.zst"),
        "linux",
        "arch",
    ),
    (
        re.compile(
            r"f1r3gaze-(?P<v>.+)-1\.(?P<c>fc[0-9]+|el[0-9]+|opensuse[0-9]+)\.(?P<a>x86_64|aarch64)\.rpm"
        ),
        "linux",
        "rpm",
    ),
    (
        re.compile(r"F1R3Gaze-(?P<v>.+)-(?P<a>x86_64|aarch64)\.AppImage"),
        "linux",
        "appimage",
    ),
    (
        re.compile(r"F1R3Gaze-(?P<v>.+)-(?P<a>x86_64|aarch64)\.flatpak"),
        "linux",
        "flatpak",
    ),
    (re.compile(r"f1r3gaze_(?P<v>[^_]+)_(?P<a>amd64|arm64)\.snap"), "linux", "snap"),
    (
        re.compile(r"f1r3gaze-(?P<v>.+)-linux-(?P<a>x86_64|aarch64)\.tar\.gz"),
        "linux",
        "tar",
    ),
    (re.compile(r"F1R3Gaze-(?P<v>.+)-macos-universal\.(?P<f>dmg|pkg)"), "macos", None),
    (re.compile(r"F1R3Gaze-(?P<v>.+)-x64\.msi"), "windows", "msi"),
    (re.compile(r"f1r3gaze-(?P<v>.+)-windows-x64\.zip"), "windows", "zip"),
)
SKIP = {"SHA256SUMS", "SHA256SUMS.asc", "catalog.json"}


def digest(path: Path) -> str:
    sha = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            sha.update(chunk)
    return sha.hexdigest()


def artifacts(dist: Path, version: str, base_url: str) -> list[dict[str, str | int]]:
    result: list[dict[str, str | int]] = []
    for path in sorted(dist.iterdir()):
        if not path.is_file() or path.name in SKIP:
            continue
        for pattern, platform, kind in ARTIFACTS:
            match = pattern.fullmatch(path.name)
            if match:
                if match["v"] != version:
                    raise ValueError(f"artifact version mismatch: {path.name}")
                groups = match.groupdict()
                result.append(
                    {
                        "name": path.name,
                        "platform": platform,
                        "architecture": groups.get("a") or "universal",
                        "format": kind or groups["f"],
                        "channel": groups.get("c") or "stable",
                        "bytes": path.stat().st_size,
                        "sha256": digest(path),
                        "url": f"{base_url.rstrip('/')}/{quote(path.name)}",
                    }
                )
                break
        else:
            raise ValueError(f"unexpected release artifact: {path.name}")
    if not result:
        raise ValueError(f"no release artifacts in {dist}")
    return result


def require_artifact(
    entries: list[dict[str, str | int]], name: str
) -> dict[str, str | int]:
    for entry in entries:
        if entry["name"] == name:
            return entry
    raise ValueError(f"required artifact is missing: {name}")


def require_complete_release(entries: list[dict[str, str | int]], version: str) -> None:
    expected = {f"f1r3gaze_{version}_{arch}.deb" for arch in ("amd64", "arm64")}
    expected.update(
        f"f1r3gaze-{version}-1-{arch}.pkg.tar.zst" for arch in ("x86_64", "aarch64")
    )
    expected.update(
        f"f1r3gaze-{version}-1.{channel}.{arch}.rpm"
        for channel in ("fc43", "fc44", "el9", "el10", "opensuse16")
        for arch in ("x86_64", "aarch64")
    )
    for arch in ("x86_64", "aarch64"):
        expected.add(f"f1r3gaze-{version}-linux-{arch}.tar.gz")
        expected.add(f"F1R3Gaze-{version}-{arch}.AppImage")
        expected.add(f"F1R3Gaze-{version}-{arch}.flatpak")
    for arch in ("amd64", "arm64"):
        expected.add(f"f1r3gaze_{version}_{arch}.snap")
    expected.update(
        {
            f"F1R3Gaze-{version}-macos-universal.dmg",
            f"F1R3Gaze-{version}-macos-universal.pkg",
            f"F1R3Gaze-{version}-x64.msi",
            f"f1r3gaze-{version}-windows-x64.zip",
        }
    )
    found = {str(entry["name"]) for entry in entries}
    if missing := sorted(expected - found):
        raise ValueError("release is incomplete: " + ", ".join(missing))


def write(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def generate_catalog(
    args: argparse.Namespace, entries: list[dict[str, str | int]]
) -> None:
    catalog = {
        "schema_version": 1,
        "release_version": args.version,
        "components": [
            {
                "id": "f1r3gaze",
                "version": args.version,
                "in_process_rspace": True,
                "optional_external_services": ["f1r3node"],
                "compatible_with": {},
            }
        ],
        "artifacts": entries,
    }
    write(args.out, json.dumps(catalog, indent=2, sort_keys=True) + "\n")


def generate_cask(
    args: argparse.Namespace, entries: list[dict[str, str | int]]
) -> None:
    entry = require_artifact(entries, f"F1R3Gaze-{args.version}-macos-universal.dmg")
    content = f'''cask "f1r3gaze" do
  version "{args.version}"
  sha256 "{entry["sha256"]}"

  url "{entry["url"]}"
  name "F1R3Gaze"
  desc "Browser with in-process f1r3lang execution"
  homepage "https://github.com/F1R3FLY-io/F1R3Gaze"

  app "F1R3Gaze.app"
  binary "#{{appdir}}/F1R3Gaze.app/Contents/MacOS/f1r3c"
end
'''
    write(args.out, content)


def generate_winget(
    args: argparse.Namespace, entries: list[dict[str, str | int]]
) -> None:
    entry = require_artifact(entries, f"F1R3Gaze-{args.version}-x64.msi")
    ident = "F1R3FLY.F1R3Gaze"
    schema = "1.12.0"
    root = args.out / ident / args.version
    write(
        root / f"{ident}.yaml",
        f"""# yaml-language-server: $schema=https://aka.ms/winget-manifest.version.{schema}.schema.json
PackageIdentifier: {ident}
PackageVersion: {args.version}
DefaultLocale: en-US
ManifestType: version
ManifestVersion: {schema}
""",
    )
    write(
        root / f"{ident}.installer.yaml",
        f"""# yaml-language-server: $schema=https://aka.ms/winget-manifest.installer.{schema}.schema.json
PackageIdentifier: {ident}
PackageVersion: {args.version}
Platform:
  - Windows.Desktop
MinimumOSVersion: 10.0.22000.0
InstallerType: msi
Scope: machine
UpgradeCode: '{{6B0E6C2A-3F1D-4C55-9E6B-F1A3C0DE2026}}'
Installers:
  - Architecture: x64
    InstallerUrl: {entry["url"]}
    InstallerSha256: {str(entry["sha256"]).upper()}
ManifestType: installer
ManifestVersion: {schema}
""",
    )
    write(
        root / f"{ident}.locale.en-US.yaml",
        f"""# yaml-language-server: $schema=https://aka.ms/winget-manifest.defaultlocale.{schema}.schema.json
PackageIdentifier: {ident}
PackageVersion: {args.version}
PackageLocale: en-US
Publisher: F1R3FLY.io
PackageName: F1R3Gaze
License: Apache-2.0
ShortDescription: Browser with in-process f1r3lang execution
PackageUrl: https://github.com/F1R3FLY-io/F1R3Gaze
ManifestType: defaultLocale
ManifestVersion: {schema}
""",
    )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("catalog", "homebrew", "winget"))
    parser.add_argument("--version", required=True)
    parser.add_argument("--dist", type=Path, default=Path("dist"))
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument(
        "--require-complete",
        action="store_true",
        help="reject a release lacking any platform or distro artifact",
    )
    args = parser.parse_args()
    if not VERSION.fullmatch(args.version):
        parser.error("version must be stable X.Y.Z")
    if not args.dist.is_dir():
        parser.error(f"artifact directory does not exist: {args.dist}")
    try:
        entries = artifacts(args.dist, args.version, args.base_url)
        if args.require_complete:
            require_complete_release(entries, args.version)
        {
            "catalog": generate_catalog,
            "homebrew": generate_cask,
            "winget": generate_winget,
        }[args.command](args, entries)
    except ValueError as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
