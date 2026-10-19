#!/usr/bin/env python3
"""Reject unknown or untested product combinations before building a suite."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def validate(catalog: dict, selected: list[str]) -> None:
    if catalog.get("schema_version") != 1:
        raise ValueError("unsupported release catalog schema")
    entries = catalog.get("components")
    if (
        not isinstance(entries, list)
        or len(entries) != 1
        or not isinstance(entries[0], dict)
        or entries[0].get("id") != "f1r3gaze"
    ):
        raise ValueError("version 1 release catalogs support only F1R3Gaze")
    components = {"f1r3gaze": entries[0]}
    if not selected or len(selected) != len(set(selected)):
        raise ValueError("select one or more distinct systems")
    if set(selected) - components.keys():
        raise ValueError(
            f"unknown system(s): {sorted(set(selected) - components.keys())}"
        )
    for left_id in selected:
        left = components[left_id]
        for right_id in selected:
            if left_id == right_id:
                continue
            right = components[right_id]
            tested = left.get("compatible_with", {}).get(right_id, [])
            if right["version"] not in tested:
                raise ValueError(
                    f"untested combination: {left_id} {left['version']} with "
                    f"{right_id} {right['version']}"
                )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--catalog", type=Path, required=True)
    parser.add_argument("systems", nargs="+")
    args = parser.parse_args()
    validate(json.loads(args.catalog.read_text(encoding="utf-8")), args.systems)
    print("selection has explicit tested compatibility")


if __name__ == "__main__":
    main()
