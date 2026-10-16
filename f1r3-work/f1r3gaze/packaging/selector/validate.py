#!/usr/bin/env python3
"""Reject unknown or untested product combinations before building a suite."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def validate(catalog: dict, selected: list[str]) -> None:
    if catalog.get("schema_version") != 1:
        raise ValueError("unsupported release catalog schema")
    components = {entry["id"]: entry for entry in catalog.get("components", [])}
    if len(components) != len(catalog.get("components", [])):
        raise ValueError("duplicate component ID")
    if not selected or len(selected) != len(set(selected)):
        raise ValueError("select one or more distinct systems")
    if set(selected) - components.keys():
        raise ValueError(f"unknown system(s): {sorted(set(selected) - components.keys())}")
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
