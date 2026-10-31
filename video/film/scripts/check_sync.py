#!/usr/bin/env python3
"""Verify that the delivered audio lands on the frames timing.json names.

usage: check_sync.py <timing.json> <video.mp4>

Picture side: every scene boundary must sit on the beat grid and carry a swish
cue on the same frame; every click and hit cue must sit on the grid's eighth
notes. Audio side: decodes the MP4's audio (honouring its edit list, so encoder
priming counts); every kick beat, every click or hit cue and the final chord
must have its strongest attack within ±50 ms less than one video frame away.
Exits 1 on any miss.
"""

from __future__ import annotations

import array
import json
import math
import subprocess
import sys
from pathlib import Path

RATE = 48_000
HOP = 48  # 1 ms
WINDOW = 384  # 8 ms


def decode(path: str) -> array.array:
    raw = subprocess.run(
        ["ffmpeg", "-hide_banner", "-loglevel", "error", "-i", path, "-vn", "-ac", "1", "-ar", str(RATE), "-f", "s16le", "-"],
        capture_output=True, timeout=300, check=True,
    ).stdout
    # One window of silence first, so an attack on frame 0 has a quiet window before it.
    samples = array.array("h", [0] * WINDOW)
    samples.frombytes(raw)
    return samples


def flux(samples: array.array) -> list[float]:
    """Onset strength per 1 ms hop: the positive change of an 8 ms RMS envelope, in log terms.

    The window spans a whole cycle of the kick's lowest pitch, so zero crossings
    do not read as new attacks.
    """
    squares = [0.0]
    for s in samples:
        squares.append(squares[-1] + s * s)
    envelope = []
    for start in range(0, len(samples) - WINDOW, HOP):
        envelope.append(math.log10((squares[start + WINDOW] - squares[start]) / WINDOW + 1.0))
    return [0.0] + [max(0.0, envelope[i] - envelope[i - 1]) for i in range(1, len(envelope))]


def strongest_attack(strength: list[float], t: float, window: float = 0.05) -> float:
    """Time of the strongest attack within ±window of t."""
    lo = max(1, int((t - window) * RATE / HOP))
    hi = min(len(strength) - 1, int((t + window) * RATE / HOP))
    best = max(range(lo, hi + 1), key=lambda i: strength[i])
    # The envelope rises fastest in the hop whose window has just taken the attack in,
    # so the attack sits at that window's end, not its start (measured: start reads 6 ms early).
    # decode() prefixes WINDOW samples of silence, so the window's end is best * HOP in film time.
    return best * HOP / RATE


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    timing = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    fps, bpm = timing["fps"], timing["bpm"]
    beat_frames = fps * 60 / bpm
    frame = 1 / fps
    failures = []
    cue_frames = {(c["frame"], c["kind"]) for c in timing["cues"]}
    for scene in timing["scenes"][1:]:
        if scene["from"] % beat_frames:
            failures.append(f"scene {scene['id']} starts at frame {scene['from']}, off the beat grid")
        if (scene["from"], "swish") not in cue_frames:
            failures.append(f"scene {scene['id']} starts at frame {scene['from']} with no swish cue")
    for cue in timing["cues"]:
        if cue["kind"] in ("click", "hit") and cue["frame"] % (beat_frames / 2):
            failures.append(f"{cue['kind']} at frame {cue['frame']} is off the eighth-note grid")
    expected = [c["frame"] / fps for c in timing["cues"] if c["kind"] in ("click", "hit")]
    beat = 60 / bpm
    for section in timing["music"]["sections"]:
        if "kick" in section["voices"]:
            expected += [b * beat for b in range(section["from"], section["to"])]
    final = timing["music"].get("final")
    if final:
        expected.append(final["beat"] * beat)
    strength = flux(decode(sys.argv[2]))
    worst = 0.0
    for t in sorted(expected):
        delta = strongest_attack(strength, t) - t
        worst = max(worst, abs(delta))
        if abs(delta) > frame:
            failures.append(f"strongest attack near {t:.3f} s is {delta * 1000:+.1f} ms away")
    print(f"check_sync.py: {len(expected)} audio events, worst offset {worst * 1000:.1f} ms (limit {frame * 1000:.1f} ms)")
    for failure in failures:
        print(f"FAIL {failure}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
