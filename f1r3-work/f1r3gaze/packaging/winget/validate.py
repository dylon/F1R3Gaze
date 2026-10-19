"""Validate a generated WinGet manifest set against pinned Microsoft schemas."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from urllib.parse import unquote, urlsplit

import jsonschema
import yaml

SCHEMA_VERSION = "1.12.0"
SCHEMA_DIR = Path(__file__).parent / "schemas"
IDENTIFIER = "F1R3FLY.F1R3Gaze"


class UniqueKeyLoader(yaml.SafeLoader):
    """Reject duplicate keys that a normal YAML loader silently overwrites."""


def construct_mapping(loader: UniqueKeyLoader, node: yaml.MappingNode) -> dict:
    loader.flatten_mapping(node)
    result = {}
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node)
        if key in result:
            raise ValueError(f"duplicate YAML key: {key}")
        result[key] = loader.construct_object(value_node)
    return result


UniqueKeyLoader.add_constructor(
    yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, construct_mapping
)


def validate_manifest_set(manifest_dir: Path, dist: Path | None = None) -> None:
    version = manifest_dir.name
    if manifest_dir.parent.name != IDENTIFIER:
        raise ValueError(f"manifest parent must be {IDENTIFIER}")
    names = {
        "version": f"{IDENTIFIER}.yaml",
        "installer": f"{IDENTIFIER}.installer.yaml",
        "defaultLocale": f"{IDENTIFIER}.locale.en-US.yaml",
    }
    actual = {path.name for path in manifest_dir.iterdir() if path.is_file()}
    if actual != set(names.values()):
        raise ValueError(f"manifest files mismatch: {sorted(actual)}")
    documents = {}
    for kind, name in names.items():
        document = yaml.load(
            (manifest_dir / name).read_text(encoding="utf-8"), Loader=UniqueKeyLoader
        )
        schema = json.loads(
            (SCHEMA_DIR / f"manifest.{kind}.{SCHEMA_VERSION}.json").read_text(
                encoding="utf-8"
            )
        )
        jsonschema.Draft7Validator.check_schema(schema)
        errors = sorted(
            jsonschema.Draft7Validator(schema).iter_errors(document),
            key=lambda error: list(map(str, error.path)),
        )
        if errors:
            raise ValueError(f"{name}: {errors[0].message}")
        if (
            document["PackageIdentifier"] != IDENTIFIER
            or document["PackageVersion"] != version
        ):
            raise ValueError(f"{name}: identifier or version differs from directory")
        if document["ManifestVersion"] != SCHEMA_VERSION:
            raise ValueError(f"{name}: unsupported manifest schema version")
        documents[kind] = document
    if (
        documents["version"]["DefaultLocale"]
        != documents["defaultLocale"]["PackageLocale"]
    ):
        raise ValueError("default locale differs from version manifest")
    installers = documents["installer"]["Installers"]
    if len(installers) != 1 or installers[0]["Architecture"] != "x64":
        raise ValueError("expected one Windows x64 MSI installer")
    url = urlsplit(installers[0]["InstallerUrl"])
    expected_name = f"F1R3Gaze-{version}-x64.msi"
    if url.scheme != "https" or unquote(url.path).rsplit("/", 1)[-1] != expected_name:
        raise ValueError("installer URL must use HTTPS and name the release MSI")
    if dist is not None:
        msi = dist / expected_name
        actual_hash = hashlib.sha256(msi.read_bytes()).hexdigest().upper()
        if installers[0]["InstallerSha256"] != actual_hash:
            raise ValueError(f"installer digest mismatch: {msi}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest_dir", type=Path)
    parser.add_argument("--dist", type=Path)
    args = parser.parse_args()
    validate_manifest_set(args.manifest_dir, args.dist)
    print(f"WinGet manifests validated for {args.manifest_dir.name}")


if __name__ == "__main__":
    main()
