#!/usr/bin/env python3
"""Planted controls for the demo film's checks in video/film/scripts, without Remotion or a network.

usage: python3 -B scripts/tests/film-controls.py

Synthesizes the music for a 4 s timing, normalises it, muxes it under a
synthetic ffmpeg picture and requires every check to pass; then plants one
violation per check and requires each to fail for its own reason: audio two
frames late, a cut off the beat, a missing click, a frame-0 kick over silence,
yuv444p, a variable frame rate, a contact sheet whose 0 s tile is not frame 0,
a misquoted line, a line never shown, a single-quoted unknown id, a page
edited since, hostile page text reaching the terminal raw, DEL and C1 left raw
in the extracted JSON, a faint colour pair, a seventh colour, and a build that
renders past a failing lint. Needs python3 and ffmpeg/ffprobe on PATH; writes
only to a temporary directory.
"""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import tempfile
from pathlib import Path

VIDEO = Path(__file__).resolve().parents[2] / "video"
SCRIPTS = VIDEO / "film" / "scripts"
PY = [sys.executable, "-B"]

TIMING = {
    "fps": 60, "bpm": 120, "beatFrames": 30, "durationFrames": 240, "width": 320, "height": 180,
    "scenes": [{"id": "a", "from": 0, "to": 120}, {"id": "b", "from": 120, "to": 240}],
    "cues": [{"frame": 120, "kind": "swish", "scene": "b"}, {"frame": 150, "kind": "click", "key": "x"}],
    "music": {
        "chords": [[57, 60, 64], [53, 57, 60]],
        "sections": [{"from": 0, "to": 6, "voices": ["pad", "kick", "hat", "bass", "arp"]}],
        "final": {"beat": 6, "chord": [57, 60, 64, 69]},
    },
}
THEME = {
    "colours": {"bg": "#0a0a0a", "ink": "#efefec", "accent": "#c8ff38"},
    "text": {"body": ["ink", "bg"], "accent": ["accent", "bg"]},
}
FACTS = [{"id": "f00000001", "text": "Preview first. Approve once.", "link": "https://example.test/"}]
COPY = {"head": {"text": "Approve once.", "fact": "f00000001"}}
SOURCE = 'export const Head = () => <h1>{line("head")}</h1>;\n'

failures: list[str] = []


def record(name: str, ok: bool, detail: str, dump: str = "") -> None:
    print(f"{'PASS' if ok else 'FAIL'} {name}: {detail}")
    if not ok:
        failures.append(name)
        if dump:
            print(dump)


def run(name: str, args: list[str], expect_ok: bool) -> None:
    result = subprocess.run(args, capture_output=True, text=True, timeout=600)
    ok = (result.returncode == 0) == expect_ok
    # The check's own last word, so a refusal can be seen to refuse for the planted reason.
    lines = (result.stdout + result.stderr).strip().splitlines()
    said = next((line for line in lines if line.startswith("FAIL")), lines[-1] if lines else "")
    detail = f"exit {result.returncode}: {said[:150]}" if ok else f"exit {result.returncode}, expected {'0' if expect_ok else 'nonzero'}"
    record(name, ok, detail, result.stdout[-1500:] + "\n" + result.stderr[-1500:])


def write_json(path: Path, value: object) -> Path:
    path.write_text(json.dumps(value), encoding="utf-8")
    return path


def ffmpeg(*args: str) -> None:
    subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", *args], check=True, timeout=300)


def media(root: Path) -> None:
    timing = write_json(root / "timing.json", TIMING)
    size = ["--width", "320", "--height", "180", "--fps", "60", "--seconds", "4"]

    run("music", PY + [str(SCRIPTS / "music.py"), str(timing), str(root)], True)
    run("loudness", PY + [str(SCRIPTS / "loudness.py"), str(root / "mix.wav"), str(root / "mix-norm.wav")], True)
    ffmpeg("-f", "lavfi", "-i", "testsrc2=size=320x180:rate=60:duration=4", "-pix_fmt", "yuv420p", "-c:v", "libx264", str(root / "picture.mp4"))
    mux = ["-map", "0:v:0", "-map", "1:a:0", "-c:v", "copy", "-c:a", "aac", "-ar", "48000", "-movflags", "+faststart", "-shortest"]
    ffmpeg("-i", str(root / "picture.mp4"), "-i", str(root / "mix-norm.wav"), *mux, str(root / "film.mp4"))
    run("media", PY + [str(SCRIPTS / "check_media.py"), str(root / "film.mp4"), *size, "--audio"], True)
    run("sync", PY + [str(SCRIPTS / "check_sync.py"), str(timing), str(root / "film.mp4"), "--sfx", str(root)], True)
    run("contact sheet", PY + [str(SCRIPTS / "contact.py"), str(root / "film.mp4"), str(root / "contact"), "--every", "0.5"], True)

    ffmpeg("-i", str(root / "mix-norm.wav"), "-af", "adelay=33:all=1", str(root / "late.wav"))
    ffmpeg("-i", str(root / "picture.mp4"), "-i", str(root / "late.wav"), *mux, str(root / "late.mp4"))
    run("sync refuses audio two frames (33 ms) late", PY + [str(SCRIPTS / "check_sync.py"), str(timing), str(root / "late.mp4")], False)
    off_grid = json.loads(json.dumps(TIMING))
    off_grid["scenes"][1]["from"] = 121
    run("sync refuses a cut off the beat", PY + [str(SCRIPTS / "check_sync.py"), str(write_json(root / "off.json", off_grid)), str(root / "film.mp4")], False)
    # The click sits on a kick: the mix alone would pass without it, its own stem does not.
    silent = root / "no-click"
    silent.mkdir()
    no_click = {**TIMING, "cues": [c for c in TIMING["cues"] if c["kind"] != "click"]}
    run("music without the click", PY + [str(SCRIPTS / "music.py"), str(write_json(silent / "timing.json", no_click)), str(silent)], True)
    run("sync refuses a missing click", PY + [str(SCRIPTS / "check_sync.py"), str(timing), str(root / "film.mp4"), "--sfx", str(silent)], False)
    # A kick expected on frame 0 over silence: the strongest hop of nothing is not an attack.
    only_kick = {"fps": 60, "bpm": 120, "scenes": [{"id": "a", "from": 0, "to": 240}], "cues": [],
                 "music": {"sections": [{"from": 0, "to": 1, "voices": ["kick"]}]}}
    ffmpeg("-i", str(root / "picture.mp4"), "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", *mux, "-t", "4", str(root / "hush.mp4"))
    run("sync refuses a frame-0 kick over silence",
        PY + [str(SCRIPTS / "check_sync.py"), str(write_json(root / "kick0.json", only_kick)), str(root / "hush.mp4")], False)

    ffmpeg("-f", "lavfi", "-i", "testsrc2=size=320x180:rate=60:duration=4", "-pix_fmt", "yuv444p", "-c:v", "libx264", "-movflags", "+faststart", str(root / "444.mp4"))
    run("media refuses yuv444p", PY + [str(SCRIPTS / "check_media.py"), str(root / "444.mp4"), *size], False)
    # Every other frame dropped after 2 s: the timestamp base still says 60.
    ffmpeg("-i", str(root / "film.mp4"), "-vf", "select=lt(t\\,2)+not(mod(n\\,2))", "-fps_mode", "vfr", "-c:v", "libx264",
           "-pix_fmt", "yuv420p", "-c:a", "copy", "-movflags", "+faststart", str(root / "vfr.mp4"))
    run("media refuses a variable frame rate", PY + [str(SCRIPTS / "check_media.py"), str(root / "vfr.mp4"), *size, "--audio"], False)

    # Frame 0 alone is white: the first tile of the sheet, stamped 0 s, must be that frame.
    ffmpeg("-f", "lavfi", "-i", "color=black:size=320x180:rate=60:duration=2",
           "-vf", "drawbox=c=white:t=fill:enable='eq(n,0)'", "-pix_fmt", "yuv420p", "-c:v", "libx264", str(root / "first.mp4"))
    run("contact sheet of the white frame", PY + [str(SCRIPTS / "contact.py"), str(root / "first.mp4"), str(root / "first"), "--every", "0.2"], True)
    tile = subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-i", str(root / "first-01.png"),
                           "-vf", "crop=100:60:150:100,scale=1:1", "-pix_fmt", "gray", "-f", "rawvideo", "-"],
                          capture_output=True, check=True, timeout=60).stdout
    record("contact sheet's 0 s tile is frame 0", bool(tile) and tile[0] > 200, f"luma {tile[0] if tile else None}")


def facts(root: Path) -> None:
    facts_json = write_json(root / "facts.json", FACTS)
    copy_json = write_json(root / "copy.json", COPY)
    trace = PY + [str(SCRIPTS / "check_facts.py"), str(facts_json)]
    src = root / "src"
    src.mkdir()
    (src / "Head.tsx").write_text(SOURCE, encoding="utf-8")
    run("facts", trace + [str(copy_json), str(src)], True)
    misquoted = {"head": {"text": "Approve twice.", "fact": "f00000001"}}
    run("facts refuse a misquote", trace + [str(write_json(root / "bad.json", misquoted)), str(src)], False)
    unshown = {**COPY, "tail": {"text": "Preview first.", "fact": "f00000001"}}
    run("facts refuse a line never shown", trace + [str(write_json(root / "unshown.json", unshown)), str(src)], False)
    single = root / "src-single"
    single.mkdir()
    (single / "Head.tsx").write_text(SOURCE.replace('"head"', "'head'"), encoding="utf-8")
    run("facts accept a single-quoted line()", trace + [str(copy_json), str(single)], True)
    (single / "Tail.tsx").write_text("export const Tail = () => <p>{line('tail')}</p>;\n", encoding="utf-8")
    run("facts refuse a single-quoted unknown id", trace + [str(copy_json), str(single)], False)
    # A line shown only from a .ts module (a literal list mapped through line()) is still shown.
    split = root / "src-split"
    split.mkdir()
    (split / "Head.tsx").write_text(SOURCE, encoding="utf-8")
    (split / "lines.ts").write_text('export const TAIL = ["tail"];\n', encoding="utf-8")
    run("facts count a line shown from a .ts module", trace + [str(root / "unshown.json"), str(split)], True)

    page = root / "index.html"
    page.write_text("<!doctype html><title>t</title><p>Preview first. Approve once.</p>\n", encoding="utf-8")
    made = subprocess.run(PY + [str(SCRIPTS / "facts.py"), "https://example.test/", str(page)], capture_output=True, text=True, check=True)
    (root / "made.json").write_text(made.stdout, encoding="utf-8")
    check = PY + [str(SCRIPTS / "facts.py"), "--check", str(root / "made.json"), "https://example.test/", str(page)]
    run("facts match their page", check, True)
    page.write_text("<!doctype html><title>t</title><p>Preview first. Approve twice.</p>\n", encoding="utf-8")
    run("facts refuse a page edited since", check, False)
    # Page text is untrusted: a stale or new line holding terminal controls is reported
    # escaped, never as raw ESC/BEL bytes the operator's terminal would act on.
    page.write_text("<!doctype html><title>t</title><p>New \x1b[2J\x1b[H line.</p>\n", encoding="utf-8")
    write_json(root / "hostile.json", [{"id": "f0", "text": "Old \x1b]0;TITLE\x07 line.", "link": ""}])
    hostile = subprocess.run(PY + [str(SCRIPTS / "facts.py"), "--check", str(root / "hostile.json"), "https://example.test/", str(page)],
                             capture_output=True, text=True, timeout=60)
    said = hostile.stdout + hostile.stderr
    escaped = hostile.returncode == 1 and "\x1b" not in said and "\x07" not in said and said.count("\\x1b") >= 3
    record("facts report hostile page text escaped", escaped, f"exit {hostile.returncode}", repr(said[-800:]))
    # The extracted JSON escapes DEL and C1 controls too (json escapes only C0), and the
    # loaded text keeps every character.
    page.write_text("<!doctype html><title>t</title><p>A\x7fB\x85C\x9b2JD</p>\n", encoding="utf-8")
    made = subprocess.run(PY + [str(SCRIPTS / "facts.py"), "https://example.test/", str(page)], capture_output=True, text=True, timeout=60)
    raw = [c for c in made.stdout if c == "\x7f" or "\x80" <= c <= "\x9f"]
    collapsed = " ".join("A\x7fB\x85C\x9b2JD".split())  # NEL is whitespace, so it collapses like a space
    kept = made.returncode == 0 and any(f["text"] == collapsed for f in json.loads(made.stdout))
    record("facts JSON escapes DEL and C1 and keeps the text", made.returncode == 0 and not raw and kept,
           f"exit {made.returncode}, raw {len(raw)}, kept {kept}")


def contrast(root: Path) -> None:
    run("contrast", PY + [str(SCRIPTS / "check_contrast.py"), str(write_json(root / "theme.json", THEME))], True)
    low = json.loads(json.dumps(THEME))
    low["colours"]["grey"] = "#444444"
    low["text"]["faint"] = ["grey", "bg"]
    run("contrast refuses a faint pair", PY + [str(SCRIPTS / "check_contrast.py"), str(write_json(root / "low.json", low))], False)
    many = json.loads(json.dumps(THEME))
    many["colours"].update({f"c{i}": "#ffffff" for i in range(4)})
    run("contrast refuses a seventh colour", PY + [str(SCRIPTS / "check_contrast.py"), str(write_json(root / "many.json", many))], False)


def build(root: Path) -> None:
    """build.py lints (react/jsx-no-literals among it) before it renders, against the real film.

    A stub npm stands in for `npm run lint`: a failing lint must stop the build there; a passing
    one must let it go on to the next step, where the build is stopped before it synthesizes music.
    """
    for lint_exit, name in ((1, "build stops at a failing lint"), (0, "build goes past a passing lint")):
        stub = root / f"bin-{lint_exit}"
        stub.mkdir()
        (stub / "npm").write_text(f'#!/bin/sh\n[ "$1 $2" = "run lint" ] && exit {lint_exit}\nexit 0\n', encoding="utf-8")
        (stub / "npm").chmod(0o755)
        env = {**os.environ, "PATH": f"{stub}:{os.environ['PATH']}"}
        proc = subprocess.Popen(PY + [str(SCRIPTS / "build.py"), "--out", str(root / f"build-{lint_exit}")], cwd=VIDEO, env=env,
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, start_new_session=True)
        said = []
        assert proc.stdout is not None
        for line in proc.stdout:
            said.append(line)
            if line.startswith("==> text contrast"):
                os.killpg(proc.pid, signal.SIGKILL)
                break
        proc.wait(timeout=120)
        text = "".join(said)
        linted = "==> lint" in text
        stopped = "lint failed" in text
        went_on = "==> text contrast" in text
        ok = linted and ((stopped and not went_on) if lint_exit else (went_on and not stopped))
        record(name, ok, f"linted {linted}, stopped {stopped}, went on {went_on}", text[-1500:])


def main() -> int:
    with tempfile.TemporaryDirectory(prefix="devtrim-film-controls-") as tmp:
        root = Path(tmp)
        for part in (media, facts, contrast, build):
            folder = root / part.__name__
            folder.mkdir()
            part(folder)
    print(f"film-controls.py: {len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
