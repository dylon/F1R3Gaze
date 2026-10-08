"""Exercise release checksum signing with a real, passphrase-protected key."""

from __future__ import annotations

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("sign-checksums.sh")


class SignChecksumsTests(unittest.TestCase):
    def test_signer_must_match_the_expected_primary_fingerprint(self) -> None:
        with tempfile.TemporaryDirectory(
            prefix="f1r3gaze-sign-checksums-"
        ) as temporary:
            root = Path(temporary)
            home = root / "gpg"
            home.mkdir(mode=0o700)
            dist = root / "dist"
            dist.mkdir()
            (dist / "artifact.txt").write_text("release artifact\n", encoding="utf-8")
            passphrase = "temporary-test-passphrase"
            gpg = ["gpg", "--homedir", str(home), "--batch", "--yes"]
            subprocess.run(
                [
                    *gpg,
                    "--pinentry-mode",
                    "loopback",
                    "--passphrase",
                    passphrase,
                    "--quick-generate-key",
                    "Release Test <release@example.invalid>",
                    "rsa2048",
                    "sign",
                    "0",
                ],
                check=True,
                capture_output=True,
            )
            keys = subprocess.run(
                [*gpg, "--with-colons", "--fingerprint", "--list-keys"],
                check=True,
                capture_output=True,
                text=True,
            ).stdout
            fingerprint = next(
                line.split(":")[9]
                for line in keys.splitlines()
                if line.startswith("fpr:")
            )
            secret = subprocess.run(
                [
                    *gpg,
                    "--pinentry-mode",
                    "loopback",
                    "--passphrase",
                    passphrase,
                    "--armor",
                    "--export-secret-keys",
                    fingerprint,
                ],
                check=True,
                capture_output=True,
                text=True,
            ).stdout
            environment = os.environ.copy()
            environment.pop("GPG_SIGNING_KEY_ID", None)
            environment.update(
                GPG_PRIVATE_KEY=secret,
                GPG_PASSPHRASE=passphrase,
                GPG_EXPECTED_FINGERPRINT=fingerprint,
            )
            signed = subprocess.run(
                ["bash", str(SCRIPT), str(dist)],
                env=environment,
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(signed.returncode, 0, signed.stderr)
            self.assertTrue((dist / "SHA256SUMS.asc").is_file())
            subprocess.run(
                [
                    *gpg,
                    "--verify",
                    str(dist / "SHA256SUMS.asc"),
                    str(dist / "SHA256SUMS"),
                ],
                check=True,
                capture_output=True,
            )

            environment["GPG_EXPECTED_FINGERPRINT"] = "0" * len(fingerprint)
            rejected = subprocess.run(
                ["bash", str(SCRIPT), str(dist)],
                env=environment,
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(rejected.returncode, 0)
            self.assertIn("differs from expected primary fingerprint", rejected.stderr)
            self.assertFalse((dist / "SHA256SUMS.asc").exists())


if __name__ == "__main__":
    unittest.main()
