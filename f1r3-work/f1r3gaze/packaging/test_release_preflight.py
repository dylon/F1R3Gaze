"""Publication must fail closed without every signing input."""

import unittest

from release_preflight import REQUIRED, check


class ReleasePreflightTests(unittest.TestCase):
    def test_each_required_credential_is_enforced(self) -> None:
        for kind, names in REQUIRED.items():
            configured = {name: "value" for name in names}
            if kind == "checksums":
                configured["GPG_EXPECTED_FINGERPRINT"] = "A" * 40
            check(kind, configured)
            for name in names:
                with self.subTest(kind=kind, missing=name):
                    partial = configured.copy()
                    del partial[name]
                    with self.assertRaisesRegex(ValueError, name):
                        check(kind, partial)

    def test_checksum_fingerprint_must_be_full(self) -> None:
        for value in ("", "12345678", "G" * 40, "A" * 39, "A" * 65):
            with (
                self.subTest(value=value),
                self.assertRaisesRegex(ValueError, "GPG_EXPECTED_FINGERPRINT"),
            ):
                check(
                    "checksums",
                    {
                        "GPG_PRIVATE_KEY": "key",
                        "GPG_PASSPHRASE": "passphrase",
                        "GPG_EXPECTED_FINGERPRINT": value,
                    },
                )

    def test_unknown_group_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "unknown"):
            check("optional", {})


if __name__ == "__main__":
    unittest.main()
