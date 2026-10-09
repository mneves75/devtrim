#!/usr/bin/env python3
"""Prove scripts/perf/pageload.mjs can fail.

A page-load gate that passes a blank, broken or slow page proves nothing. This
serves seven control pages from a temporary folder and requires: a healthy
page passes; a page that blocks its load past the budget fails the budget
(exit 1); a page that paints after the old fixed observation delay reports
its late paint and fails the budget (exit 1); a missing image, a
Content-Security-Policy violation and a page that paints nothing are broken
samples (exit 2).

usage: scripts/tests/pageload-controls.py <chrome>
"""

import json
import os
import socket
import subprocess
import sys
import tempfile
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "perf" / "pageload.mjs"
PAGES = {
    "good.html": "<!doctype html><meta charset=utf-8><title>good</title><p>Hello</p>",
    # Blocks the parser, and so the load event, for 400 ms.
    "slow.html": (
        "<!doctype html><meta charset=utf-8><title>slow</title><p>Slow</p>"
        "<script>const end = performance.now() + 400; while (performance.now() < end) {}</script>"
    ),
    # Load finishes before any content can paint; waiting 250 ms cannot see LCP.
    "latepaint.html": (
        "<!doctype html><meta charset=utf-8><title>late paint</title>"
        '<p hidden id="late-target">Hello after load</p><script>'
        "addEventListener('load', () => setTimeout(() => {"
        "document.querySelector('p').hidden = false; }, 600));</script>"
    ),
    # An initial paragraph must not hide a larger LCP candidate painted later.
    "largerpaint.html": (
        '<!doctype html><meta charset=utf-8><title>larger paint</title><p id="initial-target">Small</p>'
        '<p hidden id="larger-target" style="font-size:48px">A much larger contentful paint</p><script>'
        "addEventListener('load', () => setTimeout(() => {"
        "document.querySelector('[hidden]').hidden = false; }, 200));</script>"
    ),
    "broken.html": (
        "<!doctype html><meta charset=utf-8><title>broken</title><p>Broken</p>"
        '<img src="missing.png" alt="" width="10" height="10">'
    ),
    "csp.html": (
        "<!doctype html><meta charset=utf-8>"
        "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'\">"
        "<title>csp</title><p>Blocked</p><script>document.title = 'ran';</script>"
    ),
    "blank.html": "<!doctype html><meta charset=utf-8><title>blank</title>",
}
EXPECTED = {
    "good.html": 0,
    "slow.html": 1,
    "latepaint.html": 1,
    "largerpaint.html": 1,
    "broken.html": 2,
    "csp.html": 2,
    "blank.html": 2,
}
BUDGET_MS = "200"


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__, file=sys.stderr)
        return 2
    chrome = sys.argv[1]
    with tempfile.TemporaryDirectory(prefix="devtrim-pageload-controls-") as folder:
        home = Path(folder, "home")
        profiles = Path(folder, "profiles")
        home.mkdir()
        profiles.mkdir()
        environment = dict(os.environ, TMPDIR=str(profiles))
        for name, html in PAGES.items():
            Path(folder, name).write_text(html)
        port = free_port()
        server = subprocess.Popen(
            [sys.executable, "-m", "http.server", str(port), "--bind", "127.0.0.1"],
            cwd=folder,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        try:
            failures = []
            for name, executable in (
                ("missing-browser", str(Path(folder, "missing-chrome"))),
                ("exited-browser", "/usr/bin/true"),
            ):
                result = subprocess.run(
                    ["node", str(SCRIPT), executable, f"http://127.0.0.1:{port}/", "1", BUDGET_MS, "good.html"],
                    capture_output=True, text=True, timeout=30,
                    env=dict(environment, HOME=str(home)),
                )
                passed = result.returncode == 2 and not list(profiles.iterdir())
                print(f"{'ok' if passed else 'FAIL'} {name}: exit {result.returncode}, expected 2; profiles removed={not list(profiles.iterdir())}", flush=True)
                if not passed:
                    failures.append(name)
                    print(result.stderr[-2000:], file=sys.stderr)
            for page, expected in EXPECTED.items():
                result = subprocess.run(
                    ["node", str(SCRIPT), chrome, f"http://127.0.0.1:{port}/", "3", BUDGET_MS, page],
                    capture_output=True,
                    text=True,
                    timeout=180,
                    env=environment,
                )
                late_paint_observed = True
                if page in ("latepaint.html", "largerpaint.html") and result.returncode == expected:
                    try:
                        report = json.loads(result.stdout)
                    except json.JSONDecodeError:
                        report = []
                    late_paint_observed = len(report) == 2 and all(
                        len(view["samples"]) == 3
                        and all(
                            sample["lcp"] >= (600 if page == "latepaint.html" else 200)
                            and sample.get("lcp_element_id") == (
                                "late-target" if page == "latepaint.html" else "larger-target"
                            )
                            for sample in view["samples"]
                        )
                        for view in report
                    )
                passed = result.returncode == expected and late_paint_observed
                verdict = "ok" if passed else "FAIL"
                detail = f", intended paint observed={late_paint_observed}" if page in ("latepaint.html", "largerpaint.html") else ""
                print(f"{verdict} {page}: exit {result.returncode}, expected {expected}{detail}", flush=True)
                if not passed:
                    failures.append(page)
                    print(result.stdout[-2000:] + result.stderr[-2000:], file=sys.stderr)
        finally:
            server.terminate()
            server.wait()
    if failures:
        print(f"pageload-controls: {len(failures)} control(s) failed", file=sys.stderr)
        return 1
    print("pageload-controls: the gate passes a healthy page and rejects every bad one")
    return 0


if __name__ == "__main__":
    sys.exit(main())
