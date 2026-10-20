#!/usr/bin/env python3
"""Check the rendered Leo icon ink in the 1280x800 UI snapshot scenes.

The layout test checks icon element boxes. This checks the painted glyphs,
which can still be offset by incorrect TrueType side bearings.
"""

import argparse
import subprocess
from pathlib import Path


WIDTH = 1280
HEIGHT = 800


def pixels(path):
    dimensions = subprocess.check_output(
        ["identify", "-format", "%w %h", str(path)], text=True
    )
    if dimensions != f"{WIDTH} {HEIGHT}":
        raise SystemExit(f"{path}: expected {WIDTH}x{HEIGHT}, got {dimensions}")
    rgb = subprocess.check_output(["magick", str(path), "-depth", "8", "rgb:-"])
    if len(rgb) != WIDTH * HEIGHT * 3:
        raise SystemExit(f"{path}: unexpected RGB data length")

    def get(x, y):
        offset = (y * WIDTH + x) * 3
        return rgb[offset], rgb[offset + 1], rgb[offset + 2]

    return get


def check(get, name, box, center, ink):
    left, top, right, bottom = box
    points = [
        (x, y)
        for y in range(top, bottom)
        for x in range(left, right)
        if ink(get(x, y))
    ]
    if len(points) < 6:
        raise AssertionError(f"{name}: icon ink is missing ({len(points)} pixels)")
    x0, x1 = min(x for x, _ in points), max(x for x, _ in points)
    y0, y1 = min(y for _, y in points), max(y for _, y in points)
    if x0 == left or x1 == right - 1 or y0 == top or y1 == bottom - 1:
        raise AssertionError(f"{name}: icon ink reaches the test crop edge")
    actual = ((x0 + x1) / 2, (y0 + y1) / 2)
    if max(abs(actual[0] - center[0]), abs(actual[1] - center[1])) > 1:
        raise AssertionError(f"{name}: ink center {actual}, button center {center}")
    print(f"{name}: center {actual} within 1 px of {center}")


def grayscale(color):
    return max(color) - min(color) < 10


def blue(color):
    return color[2] > color[0] + 25 and color[2] > color[1] + 15


def theme_scene(path, dark):
    get = pixels(path)
    neutral = (lambda c: grayscale(c) and c[0] > 100) if dark else (
        lambda c: grayscale(c) and c[0] < 160
    )
    name = "dark" if dark else "light"
    check(get, f"{name} toolbar", (8, 48, 39, 74), (23, 60), blue)
    check(get, f"{name} tab close", (184, 12, 199, 30), (191, 21), neutral)
    for label, y in (
        ("tabs", 106), ("history", 143), ("sites", 179),
        ("grants", 215), ("wallet", 251), ("console", 287),
    ):
        check(
            get, f"{name} rail {label}", (10, y - 12, 31, y + 13),
            (20.5, y), blue if label == "tabs" else neutral,
        )


def find_scene(path):
    get = pixels(path)
    neutral = lambda c: grayscale(c) and c[0] > 100
    for label, x in (("previous", 1193), ("next", 1219), ("close", 1245)):
        check(get, f"find {label}", (x - 10, 103, x + 11, 124), (x, 113), neutral)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("screenshots", type=Path, help="directory produced by ui-snapshots.sh")
    args = parser.parse_args()
    theme_scene(args.screenshots / "01-tabs-dark.png", dark=True)
    theme_scene(args.screenshots / "02-tabs-light.png", dark=False)
    find_scene(args.screenshots / "27-find-open.png")


if __name__ == "__main__":
    main()
