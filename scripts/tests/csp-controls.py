#!/usr/bin/env python3
"""Prove scripts/site/csp.py --check can fail.

The shipped pages must pass (positive control). Copies with a stale style hash,
an inline style element or style attribute spelled in upper case (HTML tag and
attribute names are case-insensitive, so a browser enforces the policy on them),
and an 'unsafe-inline' source must each fail.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CHECK = ROOT / "scripts" / "site" / "csp.py"


def check(page: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-B", str(CHECK), "--check", str(page)],
        capture_output=True,
        text=True,
        timeout=60,
    )


def main() -> int:
    failures = []
    for name in ("index.html", "MANUAL.html"):
        result = check(ROOT / name)
        if result.returncode != 0:
            failures.append(f"positive control: {name} failed:\n{result.stderr}")

    manual = (ROOT / "MANUAL.html").read_text()
    assert "<style>" in manual and "<body" in manual and "style-src 'sha256-" in manual
    planted = {
        "stale-hash": manual.replace("style-src 'sha256-", "style-src 'sha256-A", 1),
        "uppercase-style-element": manual.replace(
            "</body>", "<STYLE>body { outline: 1px solid red; }</STYLE></body>", 1
        ),
        "uppercase-style-attribute": manual.replace("<body", '<body STYLE="color: red"', 1),
        "unsafe-inline": manual.replace("style-src 'sha256-", "style-src 'unsafe-inline' 'sha256-", 1),
    }
    base = Path(tempfile.mkdtemp(prefix="devtrim-csp-controls."))
    try:
        for label, text in planted.items():
            page = base / f"{label}.html"
            page.write_text(text)
            if check(page).returncode == 0:
                failures.append(f"{label}: csp.py --check accepted a page a browser would block")
    finally:
        shutil.rmtree(base)

    for failure in failures:
        print(f"csp-controls: {failure}", file=sys.stderr)
    if failures:
        return 1
    print(f"csp-controls: both pages pass; {len(planted)} planted policy breaks failed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
