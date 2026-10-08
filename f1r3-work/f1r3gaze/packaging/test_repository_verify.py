"""A staged index must bind the exact bytes customers will download."""

from __future__ import annotations

import gzip
import hashlib
import importlib.util
import io
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

REPOSITORY = Path(__file__).parent / "repository"
sys.path.insert(0, str(REPOSITORY))
spec = importlib.util.spec_from_file_location(
    "verify_staging", REPOSITORY / "verify-staging.py"
)
assert spec is not None and spec.loader is not None
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class RepositoryVerifyTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.version = "0.1.0"

    def test_apt_rejects_changed_compressed_index(self) -> None:
        lines = ["Architectures: amd64 arm64", "SHA256:"]
        for arch in ("amd64", "arm64"):
            name = f"f1r3gaze_{self.version}_{arch}.deb"
            source = self.root / "apt/pool/main/f/f1r3gaze" / name
            source.parent.mkdir(parents=True, exist_ok=True)
            source.write_bytes(name.encode())
            directory = self.root / f"apt/dists/stable/main/binary-{arch}"
            directory.mkdir(parents=True)
            packages = directory / "Packages"
            packages.write_text(
                f"Filename: pool/main/f/f1r3gaze/{name}\nSHA256: {sha(source.read_bytes())}\n"
            )
            compressed = directory / "Packages.gz"
            compressed.write_bytes(gzip.compress(packages.read_bytes(), mtime=0))
            for indexed in (packages, compressed):
                data = indexed.read_bytes()
                relative = indexed.relative_to(self.root / "apt/dists/stable")
                lines.append(f" {sha(data)} {len(data):16d} {relative}")
        release = self.root / "apt/dists/stable/Release"
        release.write_text("\n".join(lines) + "\n")
        verify.verify_apt(self.root, self.version)
        compressed.write_bytes(gzip.compress(b"tampered", mtime=0))
        with self.assertRaisesRegex(
            ValueError, "APT gzip mismatch|APT Release checksum mismatch"
        ):
            verify.verify_apt(self.root, self.version)

    def test_arch_rejects_changed_package(self) -> None:
        for arch in ("x86_64", "aarch64"):
            name = f"f1r3gaze-{self.version}-1-{arch}.pkg.tar.zst"
            directory = self.root / "arch" / arch
            directory.mkdir(parents=True)
            package = directory / name
            package.write_bytes(name.encode())
            description = f"%FILENAME%\n{name}\n\n%SHA256SUM%\n{sha(package.read_bytes())}\n\n%ARCH%\n{arch}\n".encode()
            with tarfile.open(directory / "f1r3gaze.db.tar.gz", "w:gz") as archive:
                entry = tarfile.TarInfo(f"f1r3gaze-{self.version}-1/desc")
                entry.size = len(description)
                archive.addfile(entry, io.BytesIO(description))
            with tarfile.open(directory / "f1r3gaze.files.tar.gz", "w:gz") as archive:
                entry = tarfile.TarInfo(f"f1r3gaze-{self.version}-1/desc")
                entry.size = len(description)
                archive.addfile(entry, io.BytesIO(description))
                listing = b"%FILES%\nusr/bin/f1r3gaze\nusr/bin/f1r3c\n"
                entry = tarfile.TarInfo(f"f1r3gaze-{self.version}-1/files")
                entry.size = len(listing)
                archive.addfile(entry, io.BytesIO(listing))
        verify.verify_arch(self.root, self.version)
        package.write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "Arch checksum mismatch"):
            verify.verify_arch(self.root, self.version)
        package.write_bytes(name.encode())
        files_index = directory / "f1r3gaze.files.tar.gz"
        with tarfile.open(files_index, "w:gz") as archive:
            entry = tarfile.TarInfo(f"f1r3gaze-{self.version}-1/desc")
            entry.size = len(description)
            archive.addfile(entry, io.BytesIO(description))
            listing = b"%FILES%\nusr/bin/f1r3gaze\n"
            entry = tarfile.TarInfo(f"f1r3gaze-{self.version}-1/files")
            entry.size = len(listing)
            archive.addfile(entry, io.BytesIO(listing))
        with self.assertRaisesRegex(ValueError, "Arch files index omits executable"):
            verify.verify_arch(self.root, self.version)

    def test_rpm_rejects_changed_package_and_primary_index(self) -> None:
        for channel in verify.RPM_CHANNELS:
            for arch in ("x86_64", "aarch64"):
                name = f"f1r3gaze-{self.version}-1.{channel}.{arch}.rpm"
                directory = self.root / "rpm" / channel / arch
                repodata = directory / "repodata"
                repodata.mkdir(parents=True)
                package = directory / name
                package.write_bytes(name.encode())
                primary = f'<metadata xmlns="http://linux.duke.edu/metadata/common"><package><checksum type="sha256">{sha(package.read_bytes())}</checksum><location href="{name}"/></package></metadata>'.encode()
                compressed = gzip.compress(primary, mtime=0)
                (repodata / "primary.xml.gz").write_bytes(compressed)
                repomd = f'<repomd xmlns="http://linux.duke.edu/metadata/repo"><data type="primary"><checksum type="sha256">{sha(compressed)}</checksum><location href="repodata/primary.xml.gz"/><size>{len(compressed)}</size></data></repomd>'
                (repodata / "repomd.xml").write_text(repomd)
        verify.verify_rpm(self.root, self.version)
        package.write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "RPM package checksum mismatch"):
            verify.verify_rpm(self.root, self.version)
        package.write_bytes(name.encode())
        index = repodata / "primary.xml.gz"
        index.write_bytes(index.read_bytes() + b"tampered")
        with self.assertRaisesRegex(ValueError, "RPM primary index checksum mismatch"):
            verify.verify_rpm(self.root, self.version)


if __name__ == "__main__":
    unittest.main()
