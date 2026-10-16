#!/usr/bin/env python3
"""Check that Windows installer inputs are PE32+ binaries for x86_64."""

from __future__ import annotations

import argparse
import struct
from pathlib import Path


def verify_pe_x64(path: Path) -> None:
    with path.open("rb") as stream:
        header = stream.read(64)
        if len(header) != 64 or header[:2] != b"MZ":
            raise ValueError(f"{path}: missing DOS header")
        pe_offset = struct.unpack_from("<I", header, 0x3C)[0]
        if pe_offset < 64 or pe_offset > 4 * 1024 * 1024:
            raise ValueError(f"{path}: invalid PE offset")
        stream.seek(pe_offset)
        pe_header = stream.read(26)
    if len(pe_header) != 26 or pe_header[:4] != b"PE\0\0":
        raise ValueError(f"{path}: missing PE signature")
    machine = struct.unpack_from("<H", pe_header, 4)[0]
    magic = struct.unpack_from("<H", pe_header, 24)[0]
    if machine != 0x8664 or magic != 0x20B:
        raise ValueError(f"{path}: expected x86_64 PE32+, got {machine:#x}/{magic:#x}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("paths", nargs="+", type=Path)
    args = parser.parse_args()
    for path in args.paths:
        try:
            verify_pe_x64(path)
        except (OSError, ValueError) as error:
            parser.error(str(error))
        print(f"{path}: x86_64 PE32+")


if __name__ == "__main__":
    main()
