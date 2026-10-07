#!/usr/bin/env python3
"""Verify that each staged repository indexes its expected release package."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import tarfile
from pathlib import Path
from xml.etree import ElementTree

from stage import RPM_CHANNELS, VERSION


def check(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def verify_apt(tree: Path, version: str) -> None:
    release = tree / "apt/dists/stable/Release"
    release_text = release.read_text(encoding="utf-8")
    check(
        "Architectures: amd64 arm64\n" in release_text, "APT Release architecture list"
    )
    for arch in ("amd64", "arm64"):
        name = f"f1r3gaze_{version}_{arch}.deb"
        source = tree / "apt/pool/main/f/f1r3gaze" / name
        packages = tree / f"apt/dists/stable/main/binary-{arch}/Packages"
        content = packages.read_bytes()
        check(
            gzip.decompress(Path(f"{packages}.gz").read_bytes()) == content,
            f"APT gzip mismatch: {arch}",
        )
        check(
            f"Filename: pool/main/f/f1r3gaze/{name}\n".encode() in content,
            f"APT index omits {name}",
        )
        check(
            f"SHA256: {hashlib.sha256(source.read_bytes()).hexdigest()}\n".encode()
            in content,
            f"APT checksum mismatch: {name}",
        )
        relative = packages.relative_to(release.parent)
        check(
            f"{hashlib.sha256(content).hexdigest()} {len(content):16d} {relative}"
            in release_text,
            f"APT Release omits {relative}",
        )


def verify_arch(tree: Path, version: str) -> None:
    for arch in ("x86_64", "aarch64"):
        name = f"f1r3gaze-{version}-1-{arch}.pkg.tar.zst"
        root = tree / "arch" / arch
        check((root / name).is_file(), f"Arch package missing: {name}")
        database = root / "f1r3gaze.db.tar.gz"
        with tarfile.open(database, "r:gz") as archive:
            descriptions = [
                member for member in archive if member.name.endswith("/desc")
            ]
            check(len(descriptions) == 1, f"Arch database entries: {arch}")
            stream = archive.extractfile(descriptions[0])
            check(
                stream is not None and name.encode() in stream.read(),
                f"Arch database omits {name}",
            )


def verify_rpm(tree: Path, version: str) -> None:
    repository_ns = {"repo": "http://linux.duke.edu/metadata/repo"}
    common_ns = {"common": "http://linux.duke.edu/metadata/common"}
    for channel in RPM_CHANNELS:
        for arch in ("x86_64", "aarch64"):
            name = f"f1r3gaze-{version}-1.{channel}.{arch}.rpm"
            root = tree / "rpm" / channel / arch
            check((root / name).is_file(), f"RPM package missing: {name}")
            repomd = ElementTree.parse(root / "repodata/repomd.xml")
            primary = repomd.find(
                "repo:data[@type='primary']/repo:location", repository_ns
            )
            check(
                primary is not None and primary.get("href"),
                f"RPM primary index missing: {channel}/{arch}",
            )
            path = root / str(primary.get("href"))
            raw = path.read_bytes()
            if path.suffix == ".gz":
                raw = gzip.decompress(raw)
            document = ElementTree.fromstring(raw)
            locations = [
                element.get("href")
                for element in document.findall(
                    "common:package/common:location", common_ns
                )
            ]
            check(locations == [name], f"RPM primary index omits {name}: {locations}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tree", type=Path, required=True)
    parser.add_argument("--version", required=True)
    args = parser.parse_args()
    check(VERSION.fullmatch(args.version) is not None, "version must be stable X.Y.Z")
    check((args.tree / "UNSIGNED-STAGING").is_file(), "staging marker missing")
    # Validate the staged copies, not just the downloaded input artifacts.
    for root, pattern in (
        ("apt", "*.deb"),
        ("arch", "*.pkg.tar.zst"),
        ("rpm", "*.rpm"),
    ):
        check(any((args.tree / root).rglob(pattern)), f"staged {root} packages missing")
    verify_apt(args.tree, args.version)
    verify_arch(args.tree, args.version)
    verify_rpm(args.tree, args.version)
    print(f"repository indexes verified for {args.version}")


if __name__ == "__main__":
    main()
