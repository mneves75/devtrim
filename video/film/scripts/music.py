#!/usr/bin/env python3
"""Synthesize the film's music and sound effects from timing.json, in code.

usage: music.py <timing.json> <out-dir>

Writes <out-dir>/music.wav (music only) and <out-dir>/mix.wav (music plus one
swish per scene change and one click per keystroke, each placed on the exact
frame timing.json names). Both are 48 kHz stereo 16-bit with peaks at -3 dBFS;
loudness.py brings them to the delivery target. Standard library only, fixed
seed, so the same timing.json always yields the same bytes.

timing.json fields read: fps, bpm, durationFrames, music.chords (one chord per
bar, each a list of MIDI notes), music.sections (beat ranges and which voices
play), music.final (beat, chord), cues (frame, kind: swish | click | hit).
"""

from __future__ import annotations

import json
import math
import random
import struct
import sys
import wave
from pathlib import Path

RATE = 48_000
TAU = 2 * math.pi


def hz(note: float) -> float:
    return 440.0 * 2 ** ((note - 69) / 12)


class Track:
    def __init__(self, seconds: float) -> None:
        self.left = [0.0] * int(seconds * RATE)
        self.right = [0.0] * int(seconds * RATE)

    def add(self, start: float, samples: list[float], gain: float, pan: float = 0.0) -> None:
        """Mix mono samples in at `start` seconds; pan -1 (left) .. 1 (right), equal power."""
        angle = (pan + 1) * math.pi / 4
        gl, gr = gain * math.cos(angle), gain * math.sin(angle)
        offset = int(round(start * RATE))
        end = min(len(self.left), offset + len(samples))
        for i in range(max(0, offset), end):
            s = samples[i - offset]
            self.left[i] += s * gl
            self.right[i] += s * gr

    def mixed_with(self, other: "Track") -> "Track":
        out = Track(0)
        out.left = [a + b for a, b in zip(self.left, other.left)]
        out.right = [a + b for a, b in zip(self.right, other.right)]
        return out


def lowpass(samples: list[float], cutoff: float) -> list[float]:
    alpha = 1 - math.exp(-TAU * cutoff / RATE)
    out, y = [], 0.0
    for s in samples:
        y += alpha * (s - y)
        out.append(y)
    return out


def kick() -> list[float]:
    n = int(0.32 * RATE)
    out, phase = [], 0.0
    for i in range(n):
        t = i / RATE
        phase += TAU * (46 + 74 * math.exp(-t * 28)) / RATE
        out.append(math.sin(phase) * math.exp(-t * 9) * (1 - math.exp(-t * 900)))
    return out


def hat(rng: random.Random, decay: float) -> list[float]:
    n = int(0.09 * RATE)
    noise = [rng.uniform(-1, 1) for _ in range(n)]
    low = lowpass(noise, 6000)
    return [(a - b) * math.exp(-(i / RATE) * decay) for i, (a, b) in enumerate(zip(noise, low))]


def pluck(note: float, seconds: float) -> list[float]:
    f = hz(note)
    n = int(seconds * RATE)
    return [
        (math.sin(TAU * f * i / RATE) + 0.35 * math.sin(2 * TAU * f * i / RATE))
        * math.exp(-(i / RATE) * 7)
        * (1 - math.exp(-(i / RATE) * 400))
        for i in range(n)
    ]


def bass(note: float, seconds: float) -> list[float]:
    f = hz(note)
    n = int(seconds * RATE)
    raw = [
        sum(math.sin(k * TAU * f * i / RATE) / k for k in (1, 2, 3, 4))
        * min(1.0, i / (0.004 * RATE))
        * math.exp(-(i / RATE) * 3.5)
        for i in range(n)
    ]
    return lowpass(raw, 420)


def pad(notes: list[int], seconds: float) -> list[float]:
    n = int(seconds * RATE)
    fade = 0.12 * RATE
    out = [0.0] * n
    for note in notes:
        for detune in (-0.06, 0.06):
            f = hz(note + detune)
            for i in range(n):
                out[i] += math.sin(TAU * f * i / RATE)
    voices = 2 * len(notes)
    return [s / voices * min(1.0, i / fade, (n - i) / fade) for i, s in enumerate(out)]


def swish(rng: random.Random) -> list[float]:
    n = int(0.28 * RATE)
    out, y = [], 0.0
    for i in range(n):
        t = i / n
        cutoff = 900 + 5200 * math.sin(math.pi * t)
        alpha = 1 - math.exp(-TAU * cutoff / RATE)
        y += alpha * (rng.uniform(-1, 1) - y)
        out.append(y * math.sin(math.pi * t) ** 2)
    return out


def click() -> list[float]:
    n = int(0.03 * RATE)
    return [math.sin(TAU * 2400 * i / RATE) * math.exp(-(i / RATE) * 260) for i in range(n)]


def hit() -> list[float]:
    body = kick()
    return [s * 1.0 for s in body] + [0.0] * int(0.2 * RATE)


def peak_normalize(track: Track, dbfs: float) -> None:
    peak = max(max(map(abs, track.left)), max(map(abs, track.right)), 1e-9)
    gain = 10 ** (dbfs / 20) / peak
    track.left = [s * gain for s in track.left]
    track.right = [s * gain for s in track.right]


def write(track: Track, path: Path) -> None:
    with wave.open(str(path), "wb") as out:
        out.setnchannels(2)
        out.setsampwidth(2)
        out.setframerate(RATE)
        frames = bytearray()
        for a, b in zip(track.left, track.right):
            frames += struct.pack("<hh", int(max(-1, min(1, a)) * 32767), int(max(-1, min(1, b)) * 32767))
        out.writeframes(bytes(frames))


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2
    timing = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    out_dir = Path(sys.argv[2])
    out_dir.mkdir(parents=True, exist_ok=True)
    fps, bpm = timing["fps"], timing["bpm"]
    seconds = timing["durationFrames"] / fps
    beat = 60 / bpm
    chords = timing["music"]["chords"]
    rng = random.Random(7)

    music = Track(seconds)
    kick_s, hat_s, hat_open = kick(), hat(rng, 90), hat(rng, 30)
    for section in timing["music"]["sections"]:
        voices = set(section["voices"])
        if "pad" in voices:
            # One pad per run of the same chord (chords change every four beats).
            b = section["from"]
            while b < section["to"]:
                end = min(section["to"], (b // 4 + 1) * 4)
                music.add(b * beat, pad(chords[(b // 4) % len(chords)], (end - b) * beat + 0.05), 0.34)
                b = end
        for b in range(section["from"], section["to"]):
            t = b * beat
            chord = chords[(b // 4) % len(chords)]
            root = chord[0]
            if "kick" in voices:
                music.add(t, kick_s, 0.7)
            if "hat" in voices:
                music.add(t + beat / 2, hat_open if b % 4 == 3 else hat_s, 0.22, pan=0.25)
            if "bass" in voices:
                music.add(t, bass(root - 12, beat * 0.45), 0.42)
                music.add(t + beat / 2, bass(root - 12, beat * 0.4), 0.30)
            if "arp" in voices:
                tones = [chord[1], chord[2], chord[1] + 12, chord[2]]
                for j in range(2):
                    k = (b % 4) * 2 + j
                    music.add(t + j * beat / 2, pluck(tones[k % 4] + 12, 0.35), 0.13, pan=-0.3 + 0.6 * (k % 2))
    final = timing["music"].get("final")
    if final:
        start = final["beat"] * beat
        music.add(start, pad(final["chord"], seconds - start), 0.5)
        music.add(start, kick_s, 0.8)

    sfx = Track(seconds)
    # One stem per kind as well: check_sync.py --sfx judges each cue on a stem where
    # no other cue (a swish under a hit on the same cut) or music can mask it.
    stems = {kind: Track(seconds) for kind in ("swish", "click", "hit")}
    swish_s, click_s, hit_s = swish(rng), click(), hit()
    for cue in timing["cues"]:
        t = cue["frame"] / fps
        if cue["kind"] == "swish":
            # Centre the swish's loudest moment on the cut.
            placed = (t - 0.14, swish_s, 0.30)
        elif cue["kind"] == "click":
            placed = (t, click_s, 0.32)
        elif cue["kind"] == "hit":
            placed = (t, hit_s, 0.5)
        else:
            raise SystemExit(f"unknown cue kind {cue['kind']!r} at frame {cue['frame']}")
        sfx.add(*placed, pan=0.1 if cue["kind"] == "click" else 0.0)
        stems[cue["kind"]].add(*placed)

    mix = music.mixed_with(sfx)
    peak_normalize(music, -3)
    peak_normalize(mix, -3)
    write(music, out_dir / "music.wav")
    write(mix, out_dir / "mix.wav")
    # The effects alone, one stem per kind: check_sync.py --sfx proves each cue sounds
    # on its frame, which the mix cannot show where a cue shares its frame with a kick.
    for kind, stem in stems.items():
        write(stem, out_dir / f"sfx-{kind}.wav")
    print(f"music.py: {seconds:.2f} s at {bpm} BPM, {len(timing['cues'])} cues -> {out_dir}/music.wav, mix.wav, sfx-*.wav")
    return 0


if __name__ == "__main__":
    sys.exit(main())
