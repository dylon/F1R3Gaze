"""Exercise release promotion metadata without publishing or signing anything."""

from __future__ import annotations

import argparse
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import release_metadata
from selector.validate import validate
from winget.validate import validate_manifest_set


class ReleaseMetadataTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.dist = self.root / "dist"
        self.dist.mkdir()
        self.url = "https://example.invalid/releases/v0.1.0"
        for name in (
            "F1R3Gaze-0.1.0-macos-universal.dmg",
            "F1R3Gaze-0.1.0-x64.msi",
            "f1r3gaze_0.1.0_amd64.deb",
        ):
            (self.dist / name).write_bytes(name.encode())

    def entries(self) -> list[dict[str, str | int]]:
        return release_metadata.artifacts(self.dist, "0.1.0", self.url)

    def test_catalog_hashes_actual_bytes_and_selector_accepts_gaze(self) -> None:
        entries = self.entries()
        self.assertEqual(
            next(entry for entry in entries if entry["format"] == "dmg")["sha256"],
            hashlib.sha256(b"F1R3Gaze-0.1.0-macos-universal.dmg").hexdigest(),
        )
        args = argparse.Namespace(version="0.1.0", out=self.root / "catalog.json")
        release_metadata.generate_catalog(args, entries)
        catalog = json.loads(args.out.read_text())
        self.assertEqual(
            catalog["components"][0]["optional_external_services"], ["f1r3node"]
        )
        validate(catalog, ["f1r3gaze"])
        with self.assertRaisesRegex(ValueError, "unknown system"):
            validate(catalog, ["f1r3gaze", "embers"])

    def test_package_manager_manifests_use_msi_and_dmg_digest(self) -> None:
        entries = self.entries()
        cask = self.root / "Casks/f1r3gaze.rb"
        release_metadata.generate_cask(
            argparse.Namespace(version="0.1.0", out=cask), entries
        )
        self.assertIn(
            hashlib.sha256(b"F1R3Gaze-0.1.0-macos-universal.dmg").hexdigest(),
            cask.read_text(),
        )
        winget_root = self.root / "winget"
        release_metadata.generate_winget(
            argparse.Namespace(version="0.1.0", out=winget_root), entries
        )
        validate_manifest_set(winget_root / "F1R3FLY.F1R3Gaze/0.1.0", self.dist)
        installer = (
            winget_root / "F1R3FLY.F1R3Gaze/0.1.0/F1R3FLY.F1R3Gaze.installer.yaml"
        ).read_text()
        self.assertIn(
            hashlib.sha256(b"F1R3Gaze-0.1.0-x64.msi").hexdigest().upper(),
            installer,
        )
        self.assertIn("MinimumOSVersion: 10.0.22000.0", installer)

    def test_winget_validation_rejects_tampered_digest_and_duplicate_keys(self) -> None:
        root = self.root / "winget"
        release_metadata.generate_winget(
            argparse.Namespace(version="0.1.0", out=root), self.entries()
        )
        manifest_dir = root / "F1R3FLY.F1R3Gaze/0.1.0"
        installer = manifest_dir / "F1R3FLY.F1R3Gaze.installer.yaml"
        installer.write_text(
            installer.read_text().replace("Architecture: x64", "Architecture: arm64")
        )
        with self.assertRaisesRegex(ValueError, "Windows x64 MSI"):
            validate_manifest_set(manifest_dir, self.dist)
        release_metadata.generate_winget(
            argparse.Namespace(version="0.1.0", out=root), self.entries()
        )
        (self.dist / "F1R3Gaze-0.1.0-x64.msi").write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "installer digest mismatch"):
            validate_manifest_set(manifest_dir, self.dist)
        version_manifest = manifest_dir / "F1R3FLY.F1R3Gaze.yaml"
        version_manifest.write_text(
            version_manifest.read_text() + "PackageVersion: 0.1.0\n"
        )
        with self.assertRaisesRegex(ValueError, "duplicate YAML key"):
            validate_manifest_set(manifest_dir, self.dist)

    def test_sandbox_bundles_keep_their_native_architecture_and_digest(self) -> None:
        names = (
            "F1R3Gaze-0.1.0-aarch64.flatpak",
            "f1r3gaze_0.1.0_arm64.snap",
        )
        for name in names:
            (self.dist / name).write_bytes(name.encode())
        entries = {entry["name"]: entry for entry in self.entries()}
        self.assertEqual(entries[names[0]]["format"], "flatpak")
        self.assertEqual(entries[names[0]]["architecture"], "aarch64")
        self.assertEqual(entries[names[1]]["format"], "snap")
        self.assertEqual(entries[names[1]]["architecture"], "arm64")
        for name in names:
            self.assertEqual(
                entries[name]["sha256"], hashlib.sha256(name.encode()).hexdigest()
            )

    def test_rejects_mixed_versions_and_incomplete_release(self) -> None:
        with self.assertRaisesRegex(ValueError, "incomplete"):
            release_metadata.require_complete_release(self.entries(), "0.1.0")
        (self.dist / "f1r3gaze_0.2.0_arm64.deb").write_bytes(b"wrong")
        with self.assertRaisesRegex(ValueError, "version mismatch"):
            self.entries()


if __name__ == "__main__":
    unittest.main()
