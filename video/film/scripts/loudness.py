#!/usr/bin/env python3
"""Bring a WAV to the delivery loudness and prove it: about -14 LUFS, true peak <= -1 dBTP.

usage: loudness.py <in.wav> <out.wav> [--target -14] [--peak -1]

Two-pass ffmpeg loudnorm (measure, then apply the measured values linearly),
48 kHz, then an independent ebur128 measurement of the output. Exits 1 when the
output misses the target by more than 1 LU or its true peak exceeds the limit.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys


def run(args: list[str]) -> str:
    result = subprocess.run(args, capture_output=True, text=True, timeout=300)
    if result.returncode != 0:
        raise SystemExit(f"loudness.py: {' '.join(args[:3])} failed:\n{result.stderr[-2000:]}")
    return result.stderr


def measure(path: str) -> tuple[float, float]:
    """Integrated loudness (LUFS) and true peak (dBTP), from ffmpeg's ebur128 summary."""
    log = run(["ffmpeg", "-hide_banner", "-nostats", "-i", path, "-af", "ebur128=peak=true", "-f", "null", "-"])
    summary = log[log.rfind("Summary:") :]
    integrated = float(re.search(r"I:\s+(-?[\d.]+) LUFS", summary).group(1))
    peak = float(re.search(r"Peak:\s+(-?[\d.]+) dBFS", summary).group(1))
    return integrated, peak


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("source")
    parser.add_argument("output")
    parser.add_argument("--target", type=float, default=-14.0)
    parser.add_argument("--peak", type=float, default=-1.0)
    options = parser.parse_args()
    # Aim a little under the ceiling: true-peak estimation differs between passes.
    ceiling = options.peak - 0.5
    first = run([
        "ffmpeg", "-hide_banner", "-nostats", "-i", options.source,
        "-af", f"loudnorm=I={options.target}:TP={ceiling}:LRA=11:print_format=json", "-f", "null", "-",
    ])
    stats = json.loads(first[first.rfind("{") : first.rfind("}") + 1])
    second = (
        f"loudnorm=I={options.target}:TP={ceiling}:LRA=11:linear=true"
        f":measured_I={stats['input_i']}:measured_TP={stats['input_tp']}"
        f":measured_LRA={stats['input_lra']}:measured_thresh={stats['input_thresh']}"
        f":offset={stats['target_offset']}"
    )
    run(["ffmpeg", "-hide_banner", "-nostats", "-y", "-i", options.source, "-af", second, "-ar", "48000", "-c:a", "pcm_s16le", options.output])
    before = measure(options.source)
    after = measure(options.output)
    print(f"loudness.py: before {before[0]:.1f} LUFS / {before[1]:.1f} dBTP, after {after[0]:.1f} LUFS / {after[1]:.1f} dBTP")
    if abs(after[0] - options.target) > 1.0 or after[1] > options.peak:
        print(f"loudness.py: FAIL target {options.target} LUFS ±1, true peak <= {options.peak} dBTP", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
