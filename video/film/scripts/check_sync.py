#!/usr/bin/env python3
"""Verify that the delivered audio lands on the frames timing.json names.

usage: check_sync.py <timing.json> <video.mp4> [--sfx <dir>]

Picture side: every scene boundary must sit on the beat grid and carry a swish
cue on the same frame; every click and hit cue must sit on the grid's eighth
notes. Audio side: decodes the MP4's audio (honouring its edit list, so encoder
priming counts); every kick beat, every click or hit cue and the final chord
must have its strongest attack within ±50 ms less than one video frame away.
A click or hit placed on a kick would pass on the kick alone, so with --sfx
(the folder holding music.py's sfx-click.wav, sfx-hit.wav and sfx-swish.wav,
one stem per cue kind, the same samples the mix adds) every click and hit must
also attack within one frame of its cue in its own stem, at least a tenth as
strongly as that stem's strongest attack, and every swish must sound in the
140 ms before its cut at no less than -40 dBFS. The delivered file's timing is anchored by the
kicks and final chord above; the stem proves each cue exists where it is placed.
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


def rms_dbfs(samples: array.array, start: float, end: float) -> float:
    lo = max(0, int(start * RATE) + WINDOW)
    hi = min(len(samples), int(end * RATE) + WINDOW)
    if hi <= lo:
        return -math.inf
    power = sum(s * s for s in samples[lo:hi]) / (hi - lo)
    return 10 * math.log10(power / 32768**2) if power else -math.inf


def check_cues(timing: dict, sfx_dir: str, failures: list[str]) -> int:
    fps = timing["fps"]
    stems = {}
    for kind in sorted({c["kind"] for c in timing["cues"]}):
        samples = decode(str(Path(sfx_dir) / f"sfx-{kind}.wav"))
        strength = flux(samples)
        # A stem left silent still has a floor, so a missing cue reads as missing.
        stems[kind] = (samples, strength, max(max(strength) / 10, 0.05))
    checked = 0
    for cue in timing["cues"]:
        t = cue["frame"] / fps
        samples, strength, floor = stems[cue["kind"]]
        checked += 1
        if cue["kind"] == "swish":
            level = rms_dbfs(samples, t - 0.14, t)
            if level < -40:
                failures.append(f"swish before frame {cue['frame']} is silent in the effects ({level:.1f} dBFS)")
            continue
        lo = max(1, int((t - 0.05) * RATE / HOP))
        hi = min(len(strength) - 1, int((t + 0.05) * RATE / HOP))
        best = max(range(lo, hi + 1), key=lambda i: strength[i])
        delta = best * HOP / RATE - t
        if strength[best] < floor:
            failures.append(f"{cue['kind']} at frame {cue['frame']} is missing from the effects")
        elif abs(delta) > 1 / fps:
            failures.append(f"{cue['kind']} at frame {cue['frame']} sounds {delta * 1000:+.1f} ms away in the effects")
    return checked


def main() -> int:
    args = sys.argv[1:]
    sfx_path = None
    if len(args) == 4 and args[2] == "--sfx":
        sfx_path = args[3]
        args = args[:2]
    if len(args) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    sys.argv[1:] = args
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
    if sfx_path:
        print(f"check_sync.py: {check_cues(timing, sfx_path, failures)} cues checked in the effects alone")
    for failure in failures:
        print(f"FAIL {failure}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
