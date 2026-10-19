#!/usr/bin/env python3
"""Stage unsigned package-manager indexes for a separate publication repository."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import re
import shutil
import subprocess
from datetime import datetime, timezone
from pathlib import Path

VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
RPM_CHANNELS = ("fc43", "fc44", "el9", "el10", "opensuse16")


def require_complete_packages(dist: Path, version: str) -> None:
    """Require every supported repository package before release staging."""
    if not VERSION.fullmatch(version):
        raise ValueError("version must be stable X.Y.Z")
    expected = {f"f1r3gaze_{version}_{arch}.deb" for arch in ("amd64", "arm64")}
    expected.update(
        f"f1r3gaze-{version}-1-{arch}.pkg.tar.zst" for arch in ("x86_64", "aarch64")
    )
    expected.update(
        f"f1r3gaze-{version}-1.{channel}.{arch}.rpm"
        for channel in RPM_CHANNELS
        for arch in ("x86_64", "aarch64")
    )
    actual = {
        path.name
        for path in dist.iterdir()
        if path.is_file() and path.name.endswith((".deb", ".pkg.tar.zst", ".rpm"))
    }
    if missing := sorted(expected - actual):
        raise ValueError("repository packages are incomplete: " + ", ".join(missing))
    if unexpected := sorted(actual - expected):
        raise ValueError("unexpected repository packages: " + ", ".join(unexpected))


def require_tool(name: str) -> None:
    if shutil.which(name) is None:
        raise RuntimeError(f"{name} is required for staged repository metadata")


def copy_packages(files: list[Path], destination: Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    for source in files:
        shutil.copy2(source, destination / source.name)


def apt_index(files: list[Path], root: Path) -> None:
    if not files:
        return
    require_tool("dpkg-scanpackages")
    pool = root / "pool/main/f/f1r3gaze"
    copy_packages(files, pool)
    architectures = sorted(
        {file.name.rsplit("_", 1)[-1].removesuffix(".deb") for file in files}
    )
    indexed: list[Path] = []
    for arch in architectures:
        dest = root / "dists/stable/main" / f"binary-{arch}"
        dest.mkdir(parents=True, exist_ok=True)
        result = subprocess.run(
            ["dpkg-scanpackages", "--arch", arch, "pool/main/f/f1r3gaze"],
            cwd=root,
            check=True,
            capture_output=True,
        )
        packages = dest / "Packages"
        packages.write_bytes(result.stdout)
        (dest / "Packages.gz").write_bytes(gzip.compress(result.stdout, mtime=0))
        indexed.extend((packages, dest / "Packages.gz"))
    release = root / "dists/stable/Release"
    lines = [
        "Origin: F1R3FLY.io",
        "Label: F1R3Gaze",
        "Suite: stable",
        "Codename: stable",
        f"Date: {datetime.now(timezone.utc):%a, %d %b %Y %H:%M:%S +0000}",
        f"Architectures: {' '.join(architectures)}",
        "Components: main",
        "Description: F1R3Gaze package repository",
        "SHA256:",
    ]
    for path in indexed:
        content = path.read_bytes()
        relative = path.relative_to(release.parent)
        lines.append(
            f" {hashlib.sha256(content).hexdigest()} {len(content):16d} {relative}"
        )
    release.write_text("\n".join(lines) + "\n", encoding="utf-8")


def arch_index(files: list[Path], root: Path) -> None:
    if not files:
        return
    require_tool("repo-add")
    by_arch: dict[str, list[Path]] = {}
    for file in files:
        arch = file.name.removesuffix(".pkg.tar.zst").rsplit("-", 1)[-1]
        by_arch.setdefault(arch, []).append(file)
    for arch, packages in by_arch.items():
        dest = root / arch
        copy_packages(packages, dest)
        subprocess.run(
            ["repo-add", "f1r3gaze.db.tar.gz", *[file.name for file in packages]],
            cwd=dest,
            check=True,
        )


def rpm_index(files: list[Path], root: Path) -> None:
    if not files:
        return
    require_tool("createrepo_c")
    destinations: set[Path] = set()
    for file in files:
        parts = file.name.removesuffix(".rpm").split(".")
        if len(parts) < 3:
            raise ValueError(f"RPM lacks distro channel: {file.name}")
        channel, arch = parts[-2:]
        if not channel.startswith(("fc", "el", "opensuse")):
            raise ValueError(f"unrecognized RPM channel: {file.name}")
        dest = root / channel / arch
        copy_packages([file], dest)
        destinations.add(dest)
    for dest in sorted(destinations):
        # Pin gzip for older RPM clients and for deterministic verification;
        # createrepo_c's default varies by distribution and version.
        subprocess.run(
            [
                "createrepo_c",
                "--compress-type",
                "gz",
                "--general-compress-type",
                "gz",
                ".",
            ],
            cwd=dest,
            check=True,
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--limit-bytes", type=int, default=900_000_000)
    parser.add_argument(
        "--version", help="stable release version, required with --require-complete"
    )
    parser.add_argument("--require-complete", action="store_true")
    args = parser.parse_args()
    if not args.dist.is_dir():
        parser.error(f"artifact directory does not exist: {args.dist}")
    if args.out.exists():
        parser.error(f"output must be a fresh directory: {args.out}")
    if args.require_complete:
        if not args.version:
            parser.error("--version is required with --require-complete")
        try:
            require_complete_packages(args.dist, args.version)
        except ValueError as error:
            parser.error(str(error))
    if not any(
        next(args.dist.glob(pattern), None) is not None
        for pattern in ("*.deb", "*.pkg.tar.zst", "*.rpm")
    ):
        parser.error("no repository packages found")
    args.out.mkdir(parents=True)
    try:
        apt_index(sorted(args.dist.glob("*.deb")), args.out / "apt")
        arch_index(sorted(args.dist.glob("*.pkg.tar.zst")), args.out / "arch")
        rpm_index(sorted(args.dist.glob("*.rpm")), args.out / "rpm")
        size = sum(
            path.stat().st_size for path in args.out.rglob("*") if path.is_file()
        )
        if size > args.limit_bytes:
            raise ValueError(
                f"repository tree is too large: {size} > {args.limit_bytes}"
            )
        (args.out / "UNSIGNED-STAGING").write_text(
            "Repository metadata must be signed and verified before publication.\n",
            encoding="utf-8",
        )
        print(f"staged {args.out} ({size} bytes, unsigned)")
    except Exception:
        shutil.rmtree(args.out)
        raise


if __name__ == "__main__":
    main()
