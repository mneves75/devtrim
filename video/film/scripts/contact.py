#!/usr/bin/env python3
"""Tile frames of a video into contact sheets.

usage: contact.py <video.mp4> <out-prefix> [--every 0.2] [--per-sheet 20] [--width 384]

Pulls one frame every --every seconds (frame-accurate, first frame at 0 s) and
tiles them --per-sheet to a sheet, 5 columns wide, each tile stamped with its
timecode. Writes <out-prefix>-01.png, -02.png, ... and prints their paths.
"""

from __future__ import annotations

import argparse
import math
import subprocess
import sys
from pathlib import Path


def duration(path: str) -> float:
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1", path],
        capture_output=True, text=True, timeout=60, check=True,
    )
    return float(out.stdout.strip())


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("video")
    parser.add_argument("prefix")
    parser.add_argument("--every", type=float, default=0.2)
    parser.add_argument("--per-sheet", type=int, default=20)
    parser.add_argument("--width", type=int, default=384)
    options = parser.parse_args()
    total = duration(options.video)
    count = int(math.floor(total / options.every + 1e-6))
    columns = 5
    sheets = math.ceil(count / options.per_sheet)
    rows = math.ceil(options.per_sheet / columns)
    Path(options.prefix).parent.mkdir(parents=True, exist_ok=True)
    stamp = (
        "drawtext=text='%{pts\\:hms}':x=8:y=8:fontsize=20:fontcolor=white:box=1:boxcolor=black@0.7:boxborderw=4"
    )
    for sheet in range(sheets):
        start = sheet * options.per_sheet * options.every
        frames = min(options.per_sheet, count - sheet * options.per_sheet)
        out = f"{options.prefix}-{sheet + 1:02d}.png"
        subprocess.run(
            [
                "ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-ss", f"{start:.3f}", "-i", options.video,
                "-vf", f"fps=1/{options.every},setpts=PTS+{start}/TB,{stamp},scale={options.width}:-2,tile={columns}x{rows}",
                "-frames:v", "1", "-update", "1", out,
            ],
            check=True, timeout=300,
        )
        print(f"{out} ({frames} frames from {start:.1f} s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
