#!/usr/bin/env python3
"""Build the film end to end and check it: music, loudness, render, mux, checks, contact sheet.

usage: build.py [--out out/film] [--draft]

Run from video/. Reads film/timing.json, film/theme.json, film/copy.json and
film/facts.json; writes into --out:
  film.mp4          H.264 yuv420p + AAC 48 kHz, +faststart (the delivery)
  film-muted.mp4    the same picture with no audio stream
  music.wav         the music alone at delivery loudness
  contact-*.png     one frame every 0.2 s, 20 to a sheet (review)
  thumbs.png        a 15-frame sheet across the whole film (delivery)
--draft renders at half size for a quick look and skips the media size check.
Stops at the first failing step and prints which one.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
FILM = SCRIPTS.parent
SITE = "https://mneves75.github.io/devtrim/"
PAGES = [FILM.parents[1] / "index.html", FILM.parents[1] / "MANUAL.html"]


def step(name: str, args: list[str]) -> None:
    print(f"==> {name}", flush=True)
    result = subprocess.run(args)
    if result.returncode != 0:
        raise SystemExit(f"build.py: {name} failed (exit {result.returncode})")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", default="out/film")
    parser.add_argument("--draft", action="store_true")
    options = parser.parse_args()
    out = Path(options.out)
    out.mkdir(parents=True, exist_ok=True)
    timing = json.loads((FILM / "timing.json").read_text(encoding="utf-8"))
    seconds = timing["durationFrames"] / timing["fps"]
    py = [sys.executable, "-B"]

    step("facts fresh", py + [str(SCRIPTS / "facts.py"), "--check", str(FILM / "facts.json"), SITE, *map(str, PAGES)])
    step("facts trace", py + [str(SCRIPTS / "check_facts.py"), str(FILM / "facts.json"), str(FILM / "copy.json"), "src"])
    # ESLint's react/jsx-no-literals is what keeps every on-screen string in copy.json.
    step("lint", ["npm", "run", "lint"])
    step("text contrast", py + [str(SCRIPTS / "check_contrast.py"), str(FILM / "theme.json")])
    step("music", py + [str(SCRIPTS / "music.py"), str(FILM / "timing.json"), str(out)])
    step("loudness: mix", py + [str(SCRIPTS / "loudness.py"), str(out / "mix.wav"), str(out / "mix-norm.wav")])
    step("loudness: music", py + [str(SCRIPTS / "loudness.py"), str(out / "music.wav"), str(out / "music-norm.wav")])
    (out / "music-norm.wav").replace(out / "music.wav")
    render = ["npx", "remotion", "render", "DevtrimDemo", str(out / "picture.mp4"), "--muted", "--log=error", "--codec=h264", "--image-format=png", "--pixel-format=yuv420p", "--crf=18"]
    if options.draft:
        render.append("--scale=0.5")
    step("render", render)
    step("mux", [
        "ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", str(out / "picture.mp4"), "-i", str(out / "mix-norm.wav"),
        "-map", "0:v:0", "-map", "1:a:0", "-c:v", "copy", "-c:a", "aac", "-b:a", "192k", "-ar", "48000",
        "-movflags", "+faststart", "-shortest", str(out / "film.mp4"),
    ])
    step("muted copy", [
        "ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", str(out / "picture.mp4"),
        "-map", "0:v:0", "-c:v", "copy", "-an", "-movflags", "+faststart", str(out / "film-muted.mp4"),
    ])
    if not options.draft:
        size = ["--width", str(timing["width"]), "--height", str(timing["height"]), "--fps", str(timing["fps"]), "--seconds", str(seconds)]
        step("media: film", py + [str(SCRIPTS / "check_media.py"), str(out / "film.mp4"), *size, "--audio"])
        step("media: muted", py + [str(SCRIPTS / "check_media.py"), str(out / "film-muted.mp4"), *size])
    step("sync", py + [str(SCRIPTS / "check_sync.py"), str(FILM / "timing.json"), str(out / "film.mp4"), "--sfx", str(out)])
    step("contact sheets", py + [str(SCRIPTS / "contact.py"), str(out / "film.mp4"), str(out / "contact")])
    step("thumbnail sheet", py + [str(SCRIPTS / "contact.py"), str(out / "film.mp4"), str(out / "thumbs"), "--every", f"{seconds / 15:.4f}", "--per-sheet", "15"])
    for leftover in ("mix.wav", "mix-norm.wav", "sfx-swish.wav", "sfx-click.wav", "sfx-hit.wav", "picture.mp4"):
        (out / leftover).unlink(missing_ok=True)
    print(f"build.py: done -> {out}/film.mp4")
    return 0


if __name__ == "__main__":
    sys.exit(main())
