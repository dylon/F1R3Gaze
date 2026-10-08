"""Reject a production release before packaging if signing credentials are absent."""

from __future__ import annotations

import os
import re
import sys

REQUIRED = {
    "macos": (
        "MACOS_CERT_P12",
        "MACOS_CERT_PASSWORD",
        "MACOS_SIGN_IDENTITY",
        "MACOS_INSTALLER_CERT_P12",
        "MACOS_INSTALLER_CERT_PASSWORD",
        "MACOS_INSTALLER_SIGN_IDENTITY",
        "APPLE_ID",
        "APPLE_TEAM_ID",
        "APPLE_APP_PASSWORD",
    ),
    "windows": ("WINDOWS_CERT_PFX", "WINDOWS_CERT_PASSWORD"),
    "checksums": (
        "GPG_PRIVATE_KEY",
        "GPG_PASSPHRASE",
        "GPG_EXPECTED_FINGERPRINT",
    ),
}


def check(kind: str, environment: dict[str, str]) -> None:
    if kind not in REQUIRED:
        raise ValueError(f"unknown signing credential group: {kind}")
    missing = [name for name in REQUIRED[kind] if not environment.get(name)]
    if missing:
        raise ValueError(f"{kind} release signing requires: {', '.join(missing)}")
    if kind == "checksums" and not re.fullmatch(
        r"(?:[0-9a-fA-F]{40}|[0-9a-fA-F]{64})",
        environment["GPG_EXPECTED_FINGERPRINT"],
    ):
        raise ValueError("GPG_EXPECTED_FINGERPRINT must be a full primary fingerprint")


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: release_preflight.py macos|windows|checksums")
    try:
        check(sys.argv[1], dict(os.environ))
    except ValueError as error:
        raise SystemExit(str(error)) from error
    print(f"{sys.argv[1]} release signing credentials are configured")


if __name__ == "__main__":
    main()
