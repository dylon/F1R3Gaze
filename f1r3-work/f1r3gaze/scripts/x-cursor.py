#!/usr/bin/env python3
"""x-cursor.py: describe the cursor the X server is showing.

    scripts/x-cursor.py [--png PATH]

Prints one line, ``WIDTH HEIGHT XHOT YHOT OPAQUE SHA256``:

- OPAQUE is the number of sprite pixels with non-zero alpha. It is 0 when
  the cursor is hidden: the application set a blank cursor.
- SHA256 hashes the hotspot and the ARGB pixels, so two captures with equal
  hashes show the same cursor shape.

With ``--png PATH`` the sprite is also written as an RGBA PNG.

The probe uses XFixes ``GetCursorImage``, the sprite the server itself draws.
It therefore sees what a user sees: an application's named cursor in the
current theme, or the blank cursor of a hidden one. ui-snapshots.sh compares
these hashes with reference sprites captured in the same run, so the checks
do not depend on the cursor theme. Requires python-xlib.
"""

import argparse
import hashlib
import struct
import sys
import zlib

from Xlib import display


def unpremultiply(argb):
    """XFixes sprites are premultiplied ARGB32; PNG wants straight RGBA."""
    a = (argb >> 24) & 0xFF
    r = (argb >> 16) & 0xFF
    g = (argb >> 8) & 0xFF
    b = argb & 0xFF
    match a:
        case 0:
            return (0, 0, 0, 0)
        case 255:
            return (r, g, b, 255)
        case _:
            return (min(255, r * 255 // a), min(255, g * 255 // a), min(255, b * 255 // a), a)


def write_png(path, width, height, pixels):
    """A minimal PNG encoder: 8-bit RGBA, no filtering."""
    raw = bytearray()
    for y in range(height):
        raw.append(0)  # filter type: none
        for x in range(width):
            raw.extend(unpremultiply(pixels[y * width + x]))

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    with open(path, "wb") as out:
        out.write(b"\x89PNG\r\n\x1a\n")
        out.write(chunk(b"IHDR", header))
        out.write(chunk(b"IDAT", zlib.compress(bytes(raw), 9)))
        out.write(chunk(b"IEND", b""))


def main():
    parser = argparse.ArgumentParser(description="Describe the X server's current cursor.")
    parser.add_argument("--png", help="also write the sprite to this PNG file")
    args = parser.parse_args()

    conn = display.Display()
    if not conn.has_extension("XFIXES"):
        sys.exit("the X server has no XFIXES extension")
    # XFixes requests are only valid once the client has stated its version.
    conn.xfixes_query_version()
    image = conn.xfixes_get_cursor_image(conn.screen().root)
    pixels = list(image.cursor_image)
    opaque = sum(1 for argb in pixels if argb >> 24)
    digest = hashlib.sha256()
    digest.update(struct.pack(">HHHH", image.width, image.height, image.xhot, image.yhot))
    digest.update(struct.pack(">%dI" % len(pixels), *pixels))
    if args.png:
        write_png(args.png, image.width, image.height, pixels)
    print(image.width, image.height, image.xhot, image.yhot, opaque, digest.hexdigest())


if __name__ == "__main__":
    main()
