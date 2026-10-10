"""Release staging must reject missing or mixed-version repository packages."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from repository.stage import RPM_CHANNELS, require_complete_packages


class RepositoryStageTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.dist = Path(self.temporary.name)
        version = "0.1.0"
        names = [f"f1r3gaze_{version}_{arch}.deb" for arch in ("amd64", "arm64")]
        names += [
            f"f1r3gaze-{version}-1-{arch}.pkg.tar.zst" for arch in ("x86_64", "aarch64")
        ]
        names += [
            f"f1r3gaze-{version}-1.{channel}.{arch}.rpm"
            for channel in RPM_CHANNELS
            for arch in ("x86_64", "aarch64")
        ]
        for name in names:
            (self.dist / name).write_bytes(b"package fixture")

    def test_accepts_complete_matrix_alongside_other_release_artifacts(self) -> None:
        (self.dist / "F1R3Gaze-0.1.0-macos-universal.dmg").write_bytes(b"fixture")
        require_complete_packages(self.dist, "0.1.0")

    def test_rejects_missing_architecture(self) -> None:
        (self.dist / "f1r3gaze-0.1.0-1.fc44.aarch64.rpm").unlink()
        with self.assertRaisesRegex(ValueError, "fc44.aarch64.rpm"):
            require_complete_packages(self.dist, "0.1.0")

    def test_rejects_mixed_version_package(self) -> None:
        (self.dist / "f1r3gaze_0.2.0_amd64.deb").write_bytes(b"fixture")
        with self.assertRaisesRegex(ValueError, "unexpected repository packages"):
            require_complete_packages(self.dist, "0.1.0")

    def test_rejects_unstable_release_version(self) -> None:
        with self.assertRaisesRegex(ValueError, "stable X.Y.Z"):
            require_complete_packages(self.dist, "0.1.0-rc1")


if __name__ == "__main__":
    unittest.main()
