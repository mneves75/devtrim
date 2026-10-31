#!/usr/bin/env python3
"""Compute WCAG contrast for every text colour pair the film declares.

usage: check_contrast.py <theme.json> [--min 4.5] [--max-colours 6]

theme.json holds "colours" (name -> #rrggbb) and "text" (style name ->
[foreground, background]). Every text pair must reach --min; the palette may
hold at most --max-colours colours. Exits 1 on any failure.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def luminance(hex_colour: str) -> float:
    channels = [int(hex_colour[i : i + 2], 16) / 255 for i in (1, 3, 5)]
    linear = [c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4 for c in channels]
    return 0.2126 * linear[0] + 0.7152 * linear[1] + 0.0722 * linear[2]


def ratio(a: str, b: str) -> float:
    la, lb = sorted((luminance(a), luminance(b)), reverse=True)
    return (la + 0.05) / (lb + 0.05)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("theme")
    parser.add_argument("--min", type=float, default=4.5)
    parser.add_argument("--max-colours", type=int, default=6)
    options = parser.parse_args()
    theme = json.loads(Path(options.theme).read_text(encoding="utf-8"))
    colours = theme["colours"]
    failed = 0
    count_ok = len(colours) <= options.max_colours
    print(f"{'PASS' if count_ok else 'FAIL'} palette: {len(colours)} colours (max {options.max_colours})")
    failed += not count_ok
    for style, (fg, bg) in theme["text"].items():
        value = ratio(colours[fg], colours[bg])
        ok = value >= options.min
        failed += not ok
        print(f"{'PASS' if ok else 'FAIL'} {style}: {fg} on {bg} = {value:.2f}:1")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
