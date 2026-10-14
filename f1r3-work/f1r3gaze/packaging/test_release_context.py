"""Release identity must follow the checked-out source and publishing origin."""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from release_context import (
    check_publication_tag,
    check_source,
    release_url,
    repository_from_remote,
)


class ReleaseContextTests(unittest.TestCase):
    def test_source_and_tag_must_match_requested_version(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            manifest = Path(directory) / "Cargo.toml"
            manifest.write_text('[workspace.package]\nversion = "0.1.1"\n')
            self.assertEqual(check_source("0.1.1", "v0.1.1", manifest), "0.1.1")
            with self.assertRaisesRegex(ValueError, "differs from source"):
                check_source("0.1.2", manifest=manifest)
            with self.assertRaisesRegex(ValueError, "must equal"):
                check_source("0.1.1", "v0.1.0", manifest)
            with self.assertRaisesRegex(ValueError, "stable"):
                check_source("0.1.1-rc1", manifest=manifest)

    def test_release_url_uses_selected_repository(self) -> None:
        cases = {
            "git@github.com:dylon/F1R3Gaze.git": "https://github.com/dylon/F1R3Gaze",
            "ssh://git@github.com/F1R3FLY-io/F1R3Gaze.git": "https://github.com/F1R3FLY-io/F1R3Gaze",
            "https://git.example.com/team/F1R3Gaze.git": "https://git.example.com/team/F1R3Gaze",
        }
        for remote, expected in cases.items():
            with self.subTest(remote=remote):
                self.assertEqual(repository_from_remote(remote), expected)
        with patch.dict(
            os.environ,
            {
                "GITHUB_SERVER_URL": "https://github.com",
                "GITHUB_REPOSITORY": "dylon/F1R3Gaze",
            },
        ):
            self.assertEqual(
                release_url("v0.1.1"),
                "https://github.com/dylon/F1R3Gaze/releases/download/v0.1.1",
            )
            with patch(
                "release_context.git",
                return_value="git@github.com:F1R3FLY-io/F1R3Gaze.git",
            ):
                self.assertEqual(
                    release_url("0.1.1", remote="origin"),
                    "https://github.com/F1R3FLY-io/F1R3Gaze/releases/download/v0.1.1",
                )
        self.assertEqual(
            release_url("0.1.1", "https://github.com/F1R3FLY-io/F1R3Gaze"),
            "https://github.com/F1R3FLY-io/F1R3Gaze/releases/download/v0.1.1",
        )
        for invalid in (
            "file:///tmp/repo",
            "https://github.com/owner",
            "http://github.com/a/b",
        ):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                release_url("0.1.1", invalid)

    def test_existing_remote_tag_cannot_retarget_publication(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            remote = root / "remote.git"
            clone = root / "clone"
            subprocess.run(
                ["git", "init", "--bare", str(remote)], check=True, capture_output=True
            )
            subprocess.run(["git", "init", str(clone)], check=True, capture_output=True)

            def git(*args: str) -> None:
                subprocess.run(
                    ["git", "-C", str(clone), "-c", "core.fsmonitor=false", *args],
                    check=True,
                    capture_output=True,
                )

            git("config", "user.name", "Release Test")
            git("config", "user.email", "release@example.invalid")
            git("remote", "add", "origin", str(remote))
            (clone / "file").write_text("first")
            git("add", "file")
            git("commit", "-m", "first")
            git("tag", "-a", "v0.1.1", "-m", "release")
            git("push", "origin", "v0.1.1")
            with patch("release_context.git") as command:
                command.side_effect = lambda *args: subprocess.run(
                    ["git", "-C", str(clone), "-c", "core.fsmonitor=false", *args],
                    check=True,
                    capture_output=True,
                    text=True,
                ).stdout.strip()
                check_publication_tag("0.1.1", require_existing=True)
                check_publication_tag("0.1.2")
                with self.assertRaisesRegex(ValueError, "missing"):
                    check_publication_tag("0.1.2", require_existing=True)
                (clone / "file").write_text("second")
                git("commit", "-am", "second")
                with self.assertRaisesRegex(ValueError, "not checked-out commit"):
                    check_publication_tag("0.1.1")


if __name__ == "__main__":
    unittest.main()
