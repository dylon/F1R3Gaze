"""Promotion must bind the catalog to actual bytes and reject unsafe trees."""

from __future__ import annotations

import argparse
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

import release_metadata
from repository.promotion_check import (
    check_promotion,
    directory_bytes,
    public_key_fingerprint,
    publication_key,
    verify_catalog,
    verify_checksum_manifest,
    verify_openpgp,
)


class PromotionCheckTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.dist = self.root / "dist"
        self.dist.mkdir()
        self.version = "0.1.0"
        for name in release_metadata.expected_artifact_names(self.version):
            (self.dist / name).write_bytes(name.encode())
        self.catalog = self.dist / "catalog.json"
        entries = release_metadata.artifacts(
            self.dist, self.version, "https://example.invalid/releases/v0.1.0"
        )
        release_metadata.generate_catalog(
            argparse.Namespace(version=self.version, out=self.catalog), entries
        )

    def test_complete_catalog_matches_release_bytes(self) -> None:
        verify_catalog(self.dist, self.catalog, self.version, 2_000_000)
        (self.dist / f"F1R3Gaze-{self.version}-x64.msi").write_bytes(b"changed")
        with self.assertRaisesRegex(ValueError, "catalog differs"):
            verify_catalog(self.dist, self.catalog, self.version, 2_000_000)

    def test_catalog_url_must_match_publication_url(self) -> None:
        verify_catalog(
            self.dist,
            self.catalog,
            self.version,
            2_000_000,
            "https://example.invalid/releases/v0.1.0",
        )
        with self.assertRaisesRegex(ValueError, "base URL differs"):
            verify_catalog(
                self.dist,
                self.catalog,
                self.version,
                2_000_000,
                "https://example.invalid/releases/v0.1.1",
            )

    def test_catalog_rejects_missing_extra_and_oversized_files(self) -> None:
        (self.dist / f"f1r3gaze_{self.version}_arm64.deb").unlink()
        with self.assertRaisesRegex(ValueError, "incomplete"):
            verify_catalog(self.dist, self.catalog, self.version, 2_000_000)
        (self.dist / f"f1r3gaze_{self.version}_arm64.deb").write_bytes(b"restored")
        (self.dist / "F1R3Gaze-0.1.0-extra.AppImage").write_bytes(b"unexpected")
        with self.assertRaisesRegex(ValueError, "unexpected release artifact"):
            verify_catalog(self.dist, self.catalog, self.version, 2_000_000)
        (self.dist / "F1R3Gaze-0.1.0-extra.AppImage").unlink()
        with self.assertRaisesRegex(ValueError, "catalog is too large"):
            verify_catalog(self.dist, self.catalog, self.version, 1)

    def test_repository_limit_and_escaping_symlink(self) -> None:
        tree = self.root / "tree"
        tree.mkdir()
        (tree / "index").write_bytes(b"large")
        with self.assertRaisesRegex(ValueError, "too large"):
            directory_bytes(tree, 1)
        (tree / "index").unlink()
        (tree / "escape").symlink_to(self.catalog)
        with self.assertRaisesRegex(ValueError, "escapes tree"):
            directory_bytes(tree, 100)

    def test_unsigned_or_incomplete_tree_cannot_be_finalized(self) -> None:
        tree = self.root / "tree"
        tree.mkdir()
        marker = tree / "UNSIGNED-STAGING"
        marker.write_text("unsigned\n")
        key = self.root / "missing-public-key.asc"
        with self.assertRaisesRegex(ValueError, "unsigned staging marker remains"):
            check_promotion(self.dist, tree, self.catalog, key, "0" * 40, self.version)
        with self.assertRaisesRegex(ValueError, "staged repository package matrix"):
            check_promotion(
                self.dist,
                tree,
                self.catalog,
                key,
                "0" * 40,
                self.version,
                finalize=True,
            )
        self.assertTrue(marker.is_file())

    def test_signature_verification_uses_the_expected_primary_key(self) -> None:
        home = self.root / "gnupg"
        home.mkdir(mode=0o700)
        old_home = os.environ.get("GNUPGHOME")
        os.environ["GNUPGHOME"] = str(home)
        self.addCleanup(self._restore_home, old_home)
        subprocess.run(
            [
                "gpg",
                "--batch",
                "--yes",
                "--pinentry-mode",
                "loopback",
                "--passphrase",
                "",
                "--quick-generate-key",
                "Promotion Test <promotion@example.invalid>",
                "rsa2048",
                "sign",
                "0",
            ],
            check=True,
            capture_output=True,
        )
        public = self.root / "public.asc"
        public.write_bytes(
            subprocess.run(
                ["gpg", "--batch", "--armor", "--export"],
                check=True,
                capture_output=True,
            ).stdout
        )
        fingerprint = public_key_fingerprint(public)
        tree = self.root / "tree"
        tree.mkdir()
        published = tree / "f1r3gaze-signing-key.asc"
        with self.assertRaisesRegex(ValueError, "published signing key is missing"):
            publication_key(tree, public, fingerprint)
        published.write_bytes(public.read_bytes())
        self.assertEqual(publication_key(tree, public, fingerprint), published)
        with self.assertRaisesRegex(
            ValueError, "trusted public key fingerprint mismatch"
        ):
            publication_key(tree, public, "0" * 40)
        published.unlink()
        published.symlink_to(public)
        with self.assertRaisesRegex(
            ValueError, "published signing key is missing or unsafe"
        ):
            publication_key(tree, public, fingerprint)
        published.unlink()
        published.write_text("not a public key\n")
        with self.assertRaisesRegex(ValueError, "gpg failed"):
            publication_key(tree, public, fingerprint)
        keyring = self.root / "public.gpg"
        subprocess.run(
            ["gpg", "--batch", "--dearmor", "--output", str(keyring), str(public)],
            check=True,
            capture_output=True,
        )
        signed = self.root / "Release"
        signed.write_bytes(b"signed release\n")
        signature = self.root / "Release.gpg"
        subprocess.run(
            [
                "gpg",
                "--batch",
                "--yes",
                "--armor",
                "--detach-sign",
                "--output",
                str(signature),
                str(signed),
            ],
            check=True,
            capture_output=True,
        )
        verify_openpgp(keyring, fingerprint, signature, signed)
        with self.assertRaisesRegex(ValueError, "different signing key"):
            verify_openpgp(keyring, "0" * 40, signature, signed)
        signed.write_bytes(b"changed\n")
        with self.assertRaisesRegex(ValueError, "gpgv failed"):
            verify_openpgp(keyring, fingerprint, signature, signed)

        checksums = self.dist / "SHA256SUMS"
        checksums.write_text(
            "".join(
                f"{release_metadata.digest(path)}  ./{path.name}\n"
                for path in sorted(self.dist.iterdir())
                if path.is_file()
            )
        )
        subprocess.run(
            [
                "gpg",
                "--batch",
                "--yes",
                "--armor",
                "--detach-sign",
                "--output",
                str(self.dist / "SHA256SUMS.asc"),
                str(checksums),
            ],
            check=True,
            capture_output=True,
        )
        verify_checksum_manifest(self.dist, keyring, fingerprint)
        (self.dist / "catalog.json").write_bytes(b"tampered")
        with self.assertRaisesRegex(ValueError, "signed SHA256SUMS differs"):
            verify_checksum_manifest(self.dist, keyring, fingerprint)

    @staticmethod
    def _restore_home(previous: str | None) -> None:
        if previous is None:
            os.environ.pop("GNUPGHOME", None)
        else:
            os.environ["GNUPGHOME"] = previous


if __name__ == "__main__":
    unittest.main()
