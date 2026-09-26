#!/usr/bin/env python3
"""Synthesize the brag soundtrack: A minor, 120 BPM (one beat = 15 video
frames), music and in-key sound effects sharing one reverb space."""
import sys
import numpy as np
from scipy.io import wavfile
from scipy.signal import butter, sosfilt, fftconvolve

SR = 48000
FPS = 30
DUR = 645 / FPS
N = int(DUR * SR)
rng = np.random.default_rng(7)

def fr(frame):  # video frame -> seconds
    return frame / FPS

def hz(midi):
    return 440.0 * 2 ** ((midi - 69) / 12)

def lp(x, cut, order=2):
    return sosfilt(butter(order, cut, "low", fs=SR, output="sos"), x)

def hp(x, cut, order=2):
    return sosfilt(butter(order, cut, "high", fs=SR, output="sos"), x)

def bp(x, lo, hi, order=2):
    return sosfilt(butter(order, [lo, hi], "band", fs=SR, output="sos"), x)

def place(bus, sig, t):
    i = int(t * SR)
    if i >= len(bus):
        return
    j = min(len(bus), i + len(sig))
    bus[i:j] += sig[: j - i]

def env(n, a, d, sustain=0.0, curve=5.0):
    t = np.arange(n) / SR
    att = np.clip(t / max(a, 1e-4), 0, 1)
    dec = sustain + (1 - sustain) * np.exp(-curve * np.clip(t - a, 0, None) / max(d, 1e-4))
    return att * dec

def tone(f, dur, wave="sine", detune=0.0):
    t = np.arange(int(dur * SR)) / SR
    ph = 2 * np.pi * f * t
    if wave == "sine":
        return np.sin(ph)
    if wave == "tri":
        return 2 / np.pi * np.arcsin(np.sin(ph))
    if wave == "saw":
        out = np.zeros_like(t)
        for d in (-detune, 0, detune):
            out += 2 * ((f * (1 + d) * t) % 1.0) - 1
        return out / 3
    raise ValueError(wave)

BEAT = 0.5
MUSIC_IN = fr(75)      # reveal: the groove starts
GROOVE = fr(165)       # terminal: hats + bass pulse
BREAK = fr(452)        # stat scene: drums thin out
OUTRO = fr(544)

pad = np.zeros(N); bass = np.zeros(N); drums = np.zeros(N); sfx = np.zeros(N)

# ---- pads: Am9 | Fmaj7 | Cmaj7(add9) | G6, 2 bars each from the reveal ----
chords = [[57, 60, 64, 67, 71], [53, 57, 60, 64, 69], [48, 55, 59, 62, 64], [55, 59, 62, 64, 67]]
roots = [45, 41, 48, 43]
bar = 4 * BEAT
# pre-hook swell: Am, filtered dark, rising
swell_len = MUSIC_IN + 0.3
sw = np.zeros(int(swell_len * SR))
for m in chords[0][:4]:
    sw += tone(hz(m), swell_len, "saw", 0.004)
cut = np.linspace(300, 1400, len(sw))
# time-varying lowpass approximated by crossfading two filtered copies
sw = lp(sw, 350) * (1 - np.linspace(0, 1, len(sw))) + lp(sw, 1500) * np.linspace(0, 1, len(sw))
sw *= np.linspace(0, 1, len(sw)) ** 1.6 * 0.5
place(pad, sw, 0)

t = MUSIC_IN
k = 0
while t < DUR:
    ci = k % 4
    if t >= OUTRO:
        ci = 0
    length = 2 * bar if t < OUTRO else DUR - t
    c = np.zeros(int((length + 0.6) * SR))
    for m in chords[ci]:
        c += tone(hz(m), length + 0.6, "saw", 0.005)
    c = lp(c, 1800 if t >= GROOVE else 1200)
    c *= env(len(c), 0.25, length * 0.9, sustain=0.55, curve=1.2)
    c[-int(0.6 * SR):] *= np.linspace(1, 0, int(0.6 * SR))
    place(pad, c * 0.42, t)
    if t >= OUTRO:
        break
    # bass
    r = hz(roots[ci] - 12)
    if t < GROOVE:
        b = tone(r, 2 * bar, "sine") * env(int(2 * bar * SR), 0.02, 2 * bar, sustain=0.6, curve=1)
        place(bass, b * 0.9, t)
    else:
        for e in range(16):
            tt = t + e * BEAT / 2
            if tt >= OUTRO:
                break
            n = tone(r, 0.24, "sine") + 0.25 * tone(r * 2, 0.24, "tri")
            n *= env(len(n), 0.005, 0.2, curve=4)
            place(bass, n * (0.85 if e % 2 == 0 else 0.55), tt)
    t += 2 * bar
    k += 1

# final bass note under the outro chord
fb = tone(hz(33), DUR - OUTRO, "sine") * env(int((DUR - OUTRO) * SR), 0.01, DUR - OUTRO, sustain=0.3, curve=2)
place(bass, fb * 0.9, OUTRO)

# ---- drums ----
def kick():
    n = int(0.35 * SR); tt = np.arange(n) / SR
    f = 45 + 75 * np.exp(-tt * 28)
    s = np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-tt * 9)
    s += lp(rng.standard_normal(n), 3000) * np.exp(-tt * 180) * 0.15
    return s

def hat(open_=False):
    n = int((0.12 if open_ else 0.04) * SR)
    s = hp(rng.standard_normal(n), 7000) * env(n, 0.001, 0.03 if not open_ else 0.08)
    return s

def snap():
    n = int(0.18 * SR)
    s = bp(rng.standard_normal(n), 1200, 5000) * env(n, 0.001, 0.07)
    s += tone(hz(57), 0.18, "tri") * env(n, 0.001, 0.05) * 0.3
    return s

duck = np.ones(N)
t = MUSIC_IN
beat = 0
while t < OUTRO - 1e-6:
    in_break = BREAK <= t < BREAK + 2 * BEAT
    if not in_break:
        place(drums, kick() * 0.95, t)
        i = int(t * SR); L = int(0.3 * SR)
        seg = 1 - 0.45 * np.exp(-np.arange(L) / SR * 10)
        duck[i:i + L] = np.minimum(duck[i:i + L], seg[: len(duck[i:i + L])])
    if t >= GROOVE and not in_break:
        place(drums, hat() * 0.16, t + BEAT / 2)
        place(drums, hat() * 0.07, t + BEAT / 4)
        place(drums, hat() * 0.07, t + 3 * BEAT / 4)
        if beat % 2 == 1:
            place(drums, snap() * 0.22, t)
    t += BEAT
    beat += 1

# ---- sound effects, in A minor, sent to the same room ----
def blip(midi, level, dur=0.18):
    s = tone(hz(midi), dur, "sine") + 0.3 * tone(hz(midi) * 2, dur, "tri")
    return s * env(len(s), 0.003, dur * 0.35, curve=5) * level

def click(level, thump=45):
    n = int(0.06 * SR)
    s = hp(rng.standard_normal(n), 2500) * env(n, 0.0005, 0.012)
    s += tone(hz(thump), 0.06, "sine") * env(n, 0.001, 0.03) * 0.8
    return s * level

def whoosh(t_end, dur=0.55, level=0.35):
    n = int(dur * SR)
    s = rng.standard_normal(n)
    lo = lp(s, 900); hi = lp(s, 5000)
    ramp = np.linspace(0, 1, n)
    s = (lo * (1 - ramp) + hi * ramp) * ramp ** 2.2
    s[-int(0.04 * SR):] *= np.linspace(1, 0, int(0.04 * SR))
    place(sfx, hp(s, 200) * level, t_end - dur)

def bell(midi, level):
    s = tone(hz(midi), 1.2, "sine") + 0.35 * tone(hz(midi) * 2.76, 1.2, "sine") * env(int(1.2 * SR), 0.001, 0.15)
    return s * env(len(s), 0.002, 0.6, curve=4) * level

# hook: riser into the reveal, a tick when the meter locks at 97
riser_len = MUSIC_IN
r = rng.standard_normal(int(riser_len * SR))
r = bp(r, 400, 6000) * np.linspace(0, 1, len(r)) ** 3
place(sfx, r * 0.1, 0)
place(sfx, blip(81, 0.35, 0.25), fr(40))
place(sfx, blip(76, 0.18, 0.25), fr(40))
# reveal impact
n = int(1.2 * SR); tt = np.arange(n) / SR
boom = np.sin(2 * np.pi * np.cumsum(38 + 60 * np.exp(-tt * 8)) / SR) * np.exp(-tt * 3.2)
place(sfx, boom * 0.7, MUSIC_IN)
# tagline words
for i, m in enumerate([69, 72, 74, 76]):
    place(sfx, blip(m + 12, 0.07, 0.12), fr(96 + i * 4))
# into the terminal
whoosh(fr(168))
for i, m in enumerate([69, 72, 74, 76, 79, 81]):
    place(sfx, blip(m + 12, 0.09, 0.14), fr(188 + i * 5))
place(sfx, click(0.5), fr(264))           # j
place(sfx, click(0.65, 40), fr(286))      # Space
place(sfx, blip(64, 0.12, 0.2), fr(286))
place(sfx, click(0.5), fr(346))           # a
place(sfx, bell(76, 0.16), fr(349))       # confirm modal
place(sfx, bell(81, 0.13), fr(353))
whoosh(fr(458))
# the counter rolling 1,660 -> 53: ticks, easing like the numbers
ticks = [478 + 26 * x for x in (0, .12, .24, .34, .43, .52, .6, .68, .76, .84, .92)]
for i, f in enumerate(ticks):
    place(sfx, blip(88 - (i % 5) * 2, 0.045, 0.06), fr(f))
place(sfx, blip(81, 0.2, 0.3), fr(504))
place(sfx, blip(88, 0.12, 0.3), fr(504))
whoosh(fr(546), 0.5, 0.3)
# outro typing: soft keys
for f in range(558, 584, 3):
    place(sfx, click(0.12, 57), fr(f))

# ---- room: one shared reverb ----
ir_len = int(1.6 * SR)
ir = rng.standard_normal(ir_len) * np.exp(-np.arange(ir_len) / SR * 3.2)
ir = lp(ir, 5000); ir /= np.sqrt(np.sum(ir ** 2))
def room(x, wet):
    return x + wet * fftconvolve(x, ir)[: len(x)]

pad_bus = room(pad * duck, 0.35)
sfx_bus = room(sfx, 0.3)
music = pad_bus * 0.55 + lp(bass, 400) * 0.6 + room(drums, 0.12) * 0.75
mix = music + sfx_bus * 0.55

# tail fade and gentle glue
fade = int(1.3 * SR)
mix[-fade:] *= np.linspace(1, 0, fade) ** 1.5
mix[: int(0.02 * SR)] *= np.linspace(0, 1, int(0.02 * SR))
mix = np.tanh(mix / np.max(np.abs(mix)) * 1.4) / np.tanh(1.4)
stereo = np.stack([mix, mix], axis=1)
# a touch of width: delay the reverb-heavy pad slightly on the right
wide = np.roll(pad_bus, int(0.011 * SR)) * 0.08
stereo[:, 1] += wide / (np.max(np.abs(pad_bus)) + 1e-9) * 0.2
stereo /= np.max(np.abs(stereo)) * 1.05
wavfile.write(sys.argv[1], SR, (stereo * 32767).astype(np.int16))
print("wrote", sys.argv[1], f"{DUR:.3f}s")
