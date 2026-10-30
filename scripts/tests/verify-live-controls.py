#!/usr/bin/env python3
"""Prove scripts/verify-live.sh can fail.

Serves HEAD's site files from a loopback server and runs the site half of the
live proof against them: the untouched copy must pass (positive control), and
a tampered stylesheet, a page without its CSP meta and a missing page must
each fail, naming the file. No network beyond 127.0.0.1 is used.
"""

from __future__ import annotations

import functools
import http.server
import shutil
import subprocess
import sys
import tempfile
import threading
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "verify-live.sh"
FILES = ("index.html", "MANUAL.html", "styles.css")
CSP_META = '<meta http-equiv="Content-Security-Policy"'


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args: object) -> None:
        pass


def committed(name: str) -> bytes:
    return subprocess.run(
        ["git", "show", f"HEAD:{name}"], cwd=ROOT, check=True, capture_output=True
    ).stdout


def run_against(site: Path) -> subprocess.CompletedProcess[str]:
    handler = functools.partial(QuietHandler, directory=str(site))
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        port = server.server_address[1]
        return subprocess.run(
            ["bash", str(SCRIPT)],
            cwd=ROOT,
            env={
                "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
                "DEVTRIM_SITE_URL": f"http://127.0.0.1:{port}/",
                "DEVTRIM_SITE_REF": "HEAD",
            },
            capture_output=True,
            text=True,
            timeout=120,
        )
    finally:
        server.shutdown()
        server.server_close()


def fresh_site(base: Path, name: str) -> Path:
    site = base / name
    site.mkdir()
    for file in FILES:
        (site / file).write_bytes(committed(file))
    return site


def main() -> int:
    failures = []
    base = Path(tempfile.mkdtemp(prefix="devtrim-verify-live-controls."))
    try:
        result = run_against(fresh_site(base, "pristine"))
        if result.returncode != 0 or "verify-live: all checks passed" not in result.stdout:
            failures.append(f"positive control failed:\n{result.stdout}{result.stderr}")

        tampered = fresh_site(base, "tampered")
        (tampered / "styles.css").write_bytes(committed("styles.css") + b"\n/* drift */\n")
        no_csp = fresh_site(base, "no-csp")
        index = committed("index.html").decode()
        assert CSP_META in index, "HEAD's index.html has no CSP meta to remove"
        (no_csp / "index.html").write_text(index.replace(CSP_META, "<meta name=\"x\""))
        missing = fresh_site(base, "missing")
        (missing / "MANUAL.html").unlink()

        for site, needle in (
            (tampered, "FAIL http://127.0.0.1"),
            (no_csp, "serves no CSP meta"),
            (missing, "MANUAL.html answered HTTP 404"),
        ):
            result = run_against(site)
            if result.returncode == 0 or needle not in result.stdout:
                failures.append(
                    f"{site.name}: expected a failure naming '{needle}':\n{result.stdout}{result.stderr}"
                )
            elif site is tampered and "styles.css differs" not in result.stdout:
                failures.append(f"tampered: the drift was not attributed to styles.css:\n{result.stdout}")
    finally:
        shutil.rmtree(base)

    for failure in failures:
        print(f"verify-live-controls: {failure}", file=sys.stderr)
    if failures:
        return 1
    print("verify-live-controls: positive control passed; 3 planted drifts failed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
