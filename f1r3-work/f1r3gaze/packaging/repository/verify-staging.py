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


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def archive_field(content: bytes, field: str) -> str:
    marker = f"%{field}%\n".encode()
    check(marker in content, f"Arch database is missing {field}")
    return content.split(marker, 1)[1].split(b"\n", 1)[0].decode()


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
            f"SHA256: {sha256(source)}\n".encode() in content,
            f"APT checksum mismatch: {name}",
        )
        for indexed in (packages, Path(f"{packages}.gz")):
            indexed_content = indexed.read_bytes()
            relative = indexed.relative_to(release.parent)
            check(
                f"{hashlib.sha256(indexed_content).hexdigest()} {len(indexed_content):16d} {relative}"
                in release_text,
                f"APT Release checksum mismatch: {relative}",
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
            check(stream is not None, f"Arch database description unreadable: {arch}")
            content = stream.read()
            check(
                archive_field(content, "FILENAME") == name,
                f"Arch database omits {name}",
            )
            check(
                archive_field(content, "SHA256SUM") == sha256(root / name),
                f"Arch checksum mismatch: {name}",
            )
            check(
                archive_field(content, "ARCH") == arch,
                f"Arch architecture mismatch: {name}",
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
            data = repomd.find("repo:data[@type='primary']", repository_ns)
            check(data is not None, f"RPM primary metadata missing: {channel}/{arch}")
            primary = data.find("repo:location", repository_ns)
            check(
                primary is not None and primary.get("href"),
                f"RPM primary index missing: {channel}/{arch}",
            )
            relative = Path(str(primary.get("href")))
            check(
                not relative.is_absolute() and ".." not in relative.parts,
                f"RPM primary index escapes repository: {relative}",
            )
            path = root / relative
            check(path.suffix == ".gz", f"RPM primary index is not gzip: {path}")
            compressed = path.read_bytes()
            checksum = data.find("repo:checksum", repository_ns)
            check(
                checksum is not None
                and checksum.get("type") == "sha256"
                and checksum.text == hashlib.sha256(compressed).hexdigest(),
                f"RPM primary index checksum mismatch: {channel}/{arch}",
            )
            size = data.find("repo:size", repository_ns)
            check(
                size is not None and size.text == str(len(compressed)),
                f"RPM primary index size mismatch: {channel}/{arch}",
            )
            raw = gzip.decompress(compressed)
            document = ElementTree.fromstring(raw)
            packages = document.findall("common:package", common_ns)
            check(len(packages) == 1, f"RPM primary package count: {channel}/{arch}")
            location = packages[0].find("common:location", common_ns)
            check(
                location is not None and location.get("href") == name,
                f"RPM primary index omits {name}",
            )
            package_checksum = packages[0].find("common:checksum", common_ns)
            check(
                package_checksum is not None
                and package_checksum.get("type") == "sha256"
                and package_checksum.text == sha256(root / name),
                f"RPM package checksum mismatch: {name}",
            )


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
