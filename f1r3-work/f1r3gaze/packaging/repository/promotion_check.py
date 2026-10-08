"""Read-only release promotion gate, with an explicit signed-tree finalization step."""

from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from urllib.parse import quote, urlsplit, urlunsplit

PACKAGING = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PACKAGING))

import release_metadata
from repository.stage import RPM_CHANNELS, VERSION, require_complete_packages

FINGERPRINT = re.compile(r"(?:[0-9A-F]{40}|[0-9A-F]{64})")


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def command(*args: str) -> str:
    result = subprocess.run(args, capture_output=True, text=True, check=False)
    if result.returncode:
        raise ValueError(
            f"{args[0]} failed: {(result.stderr or result.stdout).strip()}"
        )
    return result.stdout


def require_tool(name: str) -> None:
    if shutil.which(name) is None:
        raise ValueError(f"{name} is required for signed promotion checks")


def directory_bytes(root: Path, limit: int) -> int:
    require(limit > 0, "size limit must be positive")
    require(root.is_dir(), f"repository tree is missing: {root}")
    resolved_root = root.resolve()
    size = 0
    for path in root.rglob("*"):
        if path.is_symlink():
            require(path.exists(), f"repository symlink is broken: {path}")
            require(
                path.resolve().is_relative_to(resolved_root),
                f"repository symlink escapes tree: {path}",
            )
        elif path.is_file():
            size += path.stat().st_size
            require(size <= limit, f"repository tree is too large: {size} > {limit}")
    return size


def verify_catalog(dist: Path, catalog: Path, version: str, limit: int) -> None:
    require(limit > 0, "catalog size limit must be positive")
    require(dist.is_dir(), f"artifact directory is missing: {dist}")
    require(catalog.is_file(), f"release catalog is missing: {catalog}")
    require(
        catalog.stat().st_size <= limit,
        f"release catalog is too large: {catalog.stat().st_size} > {limit}",
    )
    require(not catalog.is_symlink(), "release catalog may not be a symlink")
    for path in dist.iterdir():
        require(not path.is_symlink(), f"release artifact may not be a symlink: {path}")
    document = json.loads(catalog.read_text(encoding="utf-8"))
    require(document.get("schema_version") == 1, "unsupported release catalog schema")
    require(
        document.get("release_version") == version, "release catalog version mismatch"
    )
    components = document.get("components")
    require(
        isinstance(components, list) and len(components) == 1,
        "release catalog must contain only Gaze",
    )
    require(
        isinstance(components[0], dict)
        and components[0].get("id") == "f1r3gaze"
        and components[0].get("version") == version,
        "release catalog component mismatch",
    )
    entries = document.get("artifacts")
    require(isinstance(entries, list) and entries, "release catalog has no artifacts")
    first = entries[0]
    require(
        isinstance(first, dict)
        and isinstance(first.get("name"), str)
        and isinstance(first.get("url"), str),
        "release catalog entry is malformed",
    )
    url = urlsplit(first["url"])
    require(
        url.scheme == "https"
        and url.netloc
        and not url.username
        and not url.password
        and not url.query
        and not url.fragment,
        "release artifact URLs must use HTTPS without credentials or query strings",
    )
    require(
        url.path.rsplit("/", 1)[-1] == quote(first["name"]),
        "release artifact URL basename mismatch",
    )
    base_url = urlunsplit((url.scheme, url.netloc, url.path.rsplit("/", 1)[0], "", ""))
    expected = release_metadata.artifacts(dist, version, base_url)
    release_metadata.require_complete_release(expected, version)
    require(
        entries == expected,
        "release catalog differs from artifact names, sizes, URLs or SHA-256 digests",
    )


def repository_packages(version: str) -> list[tuple[str, Path]]:
    packages = [
        (f"f1r3gaze_{version}_{arch}.deb", Path("apt/pool/main/f/f1r3gaze"))
        for arch in ("amd64", "arm64")
    ]
    packages.extend(
        (f"f1r3gaze-{version}-1-{arch}.pkg.tar.zst", Path("arch") / arch)
        for arch in ("x86_64", "aarch64")
    )
    packages.extend(
        (f"f1r3gaze-{version}-1.{channel}.{arch}.rpm", Path("rpm") / channel / arch)
        for channel in RPM_CHANNELS
        for arch in ("x86_64", "aarch64")
    )
    return packages


def verify_copies(dist: Path, tree: Path, version: str) -> None:
    require_complete_packages(dist, version)
    expected = repository_packages(version)
    expected_paths = {relative / name for name, relative in expected}
    actual_paths = {
        path.relative_to(tree)
        for path in tree.rglob("*")
        if path.is_file() and path.name.endswith((".deb", ".pkg.tar.zst", ".rpm"))
    }
    require(
        actual_paths == expected_paths,
        "staged repository package matrix differs from release",
    )
    for name, relative in expected:
        source = dist / name
        staged = tree / relative / name
        require(staged.is_file(), f"staged package is missing: {staged}")
        require(
            source.stat().st_size == staged.stat().st_size
            and release_metadata.digest(source) == release_metadata.digest(staged),
            f"staged package differs from release artifact: {name}",
        )


def public_key_fingerprint(public_key: Path) -> str:
    require(public_key.is_file(), f"public key is missing: {public_key}")
    require_tool("gpg")
    output = command(
        "gpg",
        "--batch",
        "--show-keys",
        "--with-colons",
        "--fingerprint",
        str(public_key),
    )
    lines = [line.split(":") for line in output.splitlines()]
    require(
        sum(line[0] == "pub" for line in lines) == 1,
        "public key file must contain exactly one primary key",
    )
    fingerprints = [line[9].upper() for line in lines if line[0] == "fpr"]
    require(bool(fingerprints), "public key has no fingerprint")
    return fingerprints[0]


def verify_openpgp(
    keyring: Path,
    fingerprint: str,
    signature: Path,
    source: Path | None = None,
    output_file: Path | None = None,
) -> None:
    require_tool("gpgv")
    require(signature.is_file(), f"repository signature is missing: {signature}")
    arguments = ["gpgv", "--keyring", str(keyring), "--status-fd", "1"]
    if output_file is not None:
        arguments.extend(("--output", str(output_file)))
    arguments.append(str(signature))
    if source is not None:
        require(source.is_file(), f"signed metadata is missing: {source}")
        arguments.append(str(source))
    output = command(*arguments)
    valid = [
        line.split()
        for line in output.splitlines()
        if line.startswith("[GNUPG:] VALIDSIG ")
    ]
    require(
        any(fingerprint in (fields[2].upper(), fields[-1].upper()) for fields in valid),
        f"signature uses a different signing key: {signature}",
    )


def verify_checksum_manifest(dist: Path, keyring: Path, fingerprint: str) -> None:
    manifest = dist / "SHA256SUMS"
    verify_openpgp(keyring, fingerprint, dist / "SHA256SUMS.asc", manifest)
    expected = {
        path.name: release_metadata.digest(path)
        for path in dist.iterdir()
        if path.is_file() and path.name not in {"SHA256SUMS", "SHA256SUMS.asc"}
    }
    actual = {}
    for line in manifest.read_text(encoding="utf-8").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  \./([^/]+)", line)
        require(match is not None, f"malformed SHA256SUMS entry: {line}")
        name = match.group(2)
        require(name not in actual, f"duplicate SHA256SUMS entry: {name}")
        actual[name] = match.group(1)
    require(
        actual == expected,
        "signed SHA256SUMS differs from release artifacts or catalog",
    )


def verify_signatures(
    tree: Path,
    public_key: Path,
    fingerprint: str,
    version: str,
    dist: Path | None = None,
) -> None:
    require(
        FINGERPRINT.fullmatch(fingerprint) is not None,
        "expected fingerprint must be full hexadecimal",
    )
    require(
        public_key_fingerprint(public_key) == fingerprint,
        "public key fingerprint mismatch",
    )
    for tool in ("gpgv", "rpmkeys"):
        require_tool(tool)
    with tempfile.TemporaryDirectory(prefix="f1r3gaze-promotion-") as temporary:
        work = Path(temporary)
        keyring = work / "trusted.gpg"
        command(
            "gpg",
            "--batch",
            "--yes",
            "--dearmor",
            "--output",
            str(keyring),
            str(public_key),
        )
        release = tree / "apt/dists/stable/Release"
        verify_openpgp(keyring, fingerprint, Path(f"{release}.gpg"), release)
        cleartext = work / "inrelease-content"
        verify_openpgp(
            keyring, fingerprint, release.parent / "InRelease", output_file=cleartext
        )
        require(
            cleartext.read_bytes() == release.read_bytes(),
            "APT InRelease differs from Release",
        )
        for arch in ("x86_64", "aarch64"):
            root = tree / "arch" / arch
            database = root / "f1r3gaze.db.tar.gz"
            verify_openpgp(keyring, fingerprint, root / "f1r3gaze.db.sig", database)
            package = root / f"f1r3gaze-{version}-1-{arch}.pkg.tar.zst"
            verify_openpgp(keyring, fingerprint, Path(f"{package}.sig"), package)
        for channel in RPM_CHANNELS:
            for arch in ("x86_64", "aarch64"):
                root = tree / "rpm" / channel / arch
                repomd = root / "repodata/repomd.xml"
                verify_openpgp(keyring, fingerprint, Path(f"{repomd}.asc"), repomd)
        rpmdb = work / "rpmdb"
        rpmdb.mkdir()
        command("rpmkeys", "--dbpath", str(rpmdb), "--import", str(public_key))
        for package in sorted((tree / "rpm").rglob("*.rpm")):
            command(
                "rpmkeys",
                "--dbpath",
                str(rpmdb),
                "--define",
                "_pkgverify_level all",
                "--checksig",
                str(package),
            )
        if dist is not None:
            verify_checksum_manifest(dist, keyring, fingerprint)


def check_promotion(
    dist: Path,
    tree: Path,
    catalog: Path,
    public_key: Path,
    fingerprint: str,
    version: str,
    tree_limit: int = 900_000_000,
    catalog_limit: int = 2_000_000,
    finalize: bool = False,
) -> None:
    require(VERSION.fullmatch(version) is not None, "version must be stable X.Y.Z")
    directory_bytes(tree, tree_limit)
    marker = tree / "UNSIGNED-STAGING"
    require(finalize or not marker.exists(), "unsigned staging marker remains")
    verify_catalog(dist, catalog, version, catalog_limit)
    verify_copies(dist, tree, version)
    command(
        sys.executable,
        str(Path(__file__).with_name("verify-staging.py")),
        "--tree",
        str(tree),
        "--version",
        version,
        "--marker-state",
        "either" if finalize else "signed",
    )
    verify_signatures(tree, public_key, fingerprint.upper(), version, dist)
    if finalize and marker.exists():
        marker.unlink()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist", type=Path, required=True)
    parser.add_argument("--tree", type=Path, required=True)
    parser.add_argument("--catalog", type=Path, required=True)
    parser.add_argument("--public-key", type=Path, required=True)
    parser.add_argument("--fingerprint", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--tree-limit-bytes", type=int, default=900_000_000)
    parser.add_argument("--catalog-limit-bytes", type=int, default=2_000_000)
    parser.add_argument(
        "--finalize",
        action="store_true",
        help="remove UNSIGNED-STAGING only after every check passes",
    )
    args = parser.parse_args()
    try:
        check_promotion(
            args.dist,
            args.tree,
            args.catalog,
            args.public_key,
            args.fingerprint.upper(),
            args.version,
            args.tree_limit_bytes,
            args.catalog_limit_bytes,
            args.finalize,
        )
    except (OSError, ValueError, json.JSONDecodeError) as error:
        parser.error(str(error))
    print(f"signed repository is ready for separate publication: {args.tree}")


if __name__ == "__main__":
    main()
