#!/usr/bin/env python3
"""Check a delivered MP4 against the brief: size, frame rate, duration, codecs, faststart.

usage: check_media.py <video.mp4> --width 1920 --height 1080 --fps 60 --seconds 24 [--audio]

Requires H.264 yuv420p video, the exact size and frame rate, a duration within
one frame of --seconds, the moov atom before mdat (+faststart) and, with
--audio, exactly one AAC 48 kHz stream; without it, no audio stream at all.
Prints one PASS/FAIL line per check and exits 1 on any failure.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from fractions import Fraction


def probe(path: str) -> dict:
    out = subprocess.run(
        ["ffprobe", "-v", "error", "-show_streams", "-show_format", "-of", "json", path],
        capture_output=True, text=True, timeout=60, check=True,
    )
    return json.loads(out.stdout)


def faststart(path: str) -> bool:
    """True when the top-level moov box comes before mdat."""
    with open(path, "rb") as handle:
        offset = 0
        while True:
            header = handle.read(8)
            if len(header) < 8:
                return False
            size = int.from_bytes(header[:4], "big")
            kind = header[4:8]
            if kind == b"moov":
                return True
            if kind == b"mdat":
                return False
            if size == 1:
                size = int.from_bytes(handle.read(8), "big")
                handle.seek(offset + size)
            elif size == 0:
                return False
            else:
                handle.seek(offset + size)
            offset += size


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("video")
    parser.add_argument("--width", type=int, required=True)
    parser.add_argument("--height", type=int, required=True)
    parser.add_argument("--fps", type=int, required=True)
    parser.add_argument("--seconds", type=float, required=True)
    parser.add_argument("--audio", action="store_true")
    options = parser.parse_args()
    info = probe(options.video)
    video = [s for s in info["streams"] if s["codec_type"] == "video"]
    audio = [s for s in info["streams"] if s["codec_type"] == "audio"]
    checks = []
    checks.append(("one video stream", len(video) == 1, f"{len(video)}"))
    if video:
        v = video[0]
        checks.append(("h264", v["codec_name"] == "h264", v["codec_name"]))
        checks.append(("yuv420p", v.get("pix_fmt") == "yuv420p", v.get("pix_fmt")))
        checks.append(("size", (v["width"], v["height"]) == (options.width, options.height), f"{v['width']}x{v['height']}"))
        rate = Fraction(v["r_frame_rate"])
        checks.append(("frame rate", rate == options.fps, str(rate)))
    total = float(info["format"]["duration"])
    checks.append(("duration", abs(total - options.seconds) <= 1 / options.fps + 1e-3, f"{total:.3f} s"))
    checks.append(("faststart", faststart(options.video), "moov before mdat"))
    if options.audio:
        checks.append(("one audio stream", len(audio) == 1, f"{len(audio)}"))
        if audio:
            a = audio[0]
            checks.append(("aac", a["codec_name"] == "aac", a["codec_name"]))
            checks.append(("48 kHz", a["sample_rate"] == "48000", a["sample_rate"]))
    else:
        checks.append(("no audio stream", not audio, f"{len(audio)}"))
    failed = 0
    for name, ok, seen in checks:
        print(f"{'PASS' if ok else 'FAIL'} {name}: {seen}")
        failed += not ok
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
