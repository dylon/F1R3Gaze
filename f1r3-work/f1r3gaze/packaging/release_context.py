"""Resolve the release repository and reject mismatched source or tag versions."""

from __future__ import annotations

import argparse
import os
import re
import subprocess
from pathlib import Path
from urllib.parse import urlparse

STABLE_VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)")
PROJECT = Path(__file__).resolve().parent.parent


def version_number(value: str) -> str:
    version = value.removeprefix("v")
    if not STABLE_VERSION.fullmatch(version):
        raise ValueError(f"expected stable X.Y.Z release version: {value}")
    return version


def workspace_version(manifest: Path = PROJECT / "Cargo.toml") -> str:
    in_workspace_package = False
    found: list[str] = []
    for line in manifest.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped.startswith("["):
            in_workspace_package = stripped == "[workspace.package]"
        elif in_workspace_package:
            match = re.fullmatch(r'\s*version\s*=\s*"([^"]+)"\s*(?:#.*)?', line)
            if match:
                found.append(match[1])
    if len(found) != 1:
        raise ValueError(f"expected one [workspace.package] version in {manifest}")
    return version_number(found[0])


def check_source(
    value: str, tag_name: str | None = None, manifest: Path = PROJECT / "Cargo.toml"
) -> str:
    version = version_number(value)
    source_version = workspace_version(manifest)
    if version != source_version:
        raise ValueError(
            f"release version {version} differs from source version {source_version}"
        )
    if tag_name is not None and tag_name != f"v{version}":
        raise ValueError(f"release tag {tag_name} must equal v{version}")
    return version


def repository_from_remote(remote_url: str) -> str:
    if match := re.fullmatch(r"(?:[^/@:]+@)?([A-Za-z0-9.-]+):([^/][^:]*)", remote_url):
        host, path = match.groups()
    else:
        parsed = urlparse(remote_url)
        if parsed.scheme not in {"https", "http", "ssh", "git"} or not parsed.hostname:
            raise ValueError(
                f"remote cannot identify a GitHub release repository: {remote_url}"
            )
        host, path = parsed.hostname, parsed.path.lstrip("/")
    path = path.removesuffix(".git").strip("/")
    if len(path.split("/")) != 2 or not all(
        re.fullmatch(r"[A-Za-z0-9_.-]+", part) for part in path.split("/")
    ):
        raise ValueError(f"remote must identify OWNER/REPO: {remote_url}")
    return f"https://{host}/{path}"


def validate_repository_url(repository_url: str) -> str:
    parsed = urlparse(repository_url)
    path = parsed.path.strip("/")
    if (
        parsed.scheme != "https"
        or not parsed.hostname
        or parsed.username
        or parsed.password
        or parsed.query
        or parsed.fragment
        or len(path.split("/")) != 2
        or not all(re.fullmatch(r"[A-Za-z0-9_.-]+", part) for part in path.split("/"))
    ):
        raise ValueError(
            f"expected HTTPS repository URL with OWNER/REPO: {repository_url}"
        )
    return f"https://{parsed.netloc}/{path}"


def git(*args: str) -> str:
    result = subprocess.run(["git", *args], check=True, capture_output=True, text=True)
    return result.stdout.strip()


def publishing_remote_url(remote: str) -> str:
    urls = git("remote", "get-url", "--push", "--all", remote).splitlines()
    if len(set(urls)) != 1:
        raise ValueError(
            f"release remote {remote} must have exactly one push destination"
        )
    return urls[0]


def release_url(
    value: str, repository_url: str | None = None, remote: str | None = None
) -> str:
    version = version_number(value)
    if repository_url is None:
        server, repository = (
            os.environ.get("GITHUB_SERVER_URL"),
            os.environ.get("GITHUB_REPOSITORY"),
        )
        if remote is None and server and repository:
            repository_url = f"{server.rstrip('/')}/{repository}"
        else:
            repository_url = repository_from_remote(
                publishing_remote_url(remote or "origin")
            )
    return f"{validate_repository_url(repository_url)}/releases/download/v{version}"


def check_publication_tag(
    value: str, remote: str = "origin", require_existing: bool = False
) -> None:
    version = version_number(value)
    tag = f"refs/tags/v{version}"
    refs = git("ls-remote", "--tags", publishing_remote_url(remote), tag, f"{tag}^{{}}")
    entries = dict(line.split("\t", 1)[::-1] for line in refs.splitlines() if line)
    commit = entries.get(f"{tag}^{{}}", entries.get(tag))
    if require_existing and commit is None:
        raise ValueError(f"{tag} is missing from {remote}")
    head = git("rev-parse", "HEAD")
    if commit is not None and commit != head:
        raise ValueError(
            f"{tag} on {remote} points to {commit}, not checked-out commit {head}"
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    source = commands.add_parser("check-source")
    source.add_argument("version")
    source.add_argument("--tag-name")
    tag = commands.add_parser("check-publication-tag")
    tag.add_argument("version")
    tag.add_argument("--remote", default="origin")
    tag.add_argument("--require-existing", action="store_true")
    url = commands.add_parser("url")
    url.add_argument("version")
    url.add_argument("--repository-url")
    url.add_argument("--remote")
    args = parser.parse_args()
    try:
        if args.command == "check-source":
            print(check_source(args.version, args.tag_name))
        elif args.command == "check-publication-tag":
            check_publication_tag(args.version, args.remote, args.require_existing)
            if args.require_existing:
                print("release tag matches checked-out commit")
            else:
                print(
                    "release tag matches checked-out commit or is available for creation"
                )
        else:
            print(release_url(args.version, args.repository_url, args.remote))
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        raise SystemExit(str(error)) from error


if __name__ == "__main__":
    main()
