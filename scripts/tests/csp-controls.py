#!/usr/bin/env python3
"""Prove scripts/site/csp.py --check can fail.

The shipped pages must pass (positive control). Copies with a stale style hash,
an inline style element or style attribute spelled any way a browser accepts
(upper case, whitespace before an end tag's bracket, an attribute after a
slash, a style inside SVG), an inline script whose only "src" is data-src, a
self-closing script or style and a repeated attribute (each read one way by the
browser and another by a naive reader), any script or style inside SVG or
MathML (parsed as foreign content), an element left unclosed at the end, a
second policy meta, and an 'unsafe-inline' source must each fail.
"""

from __future__ import annotations

import base64
import hashlib
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CHECK = Path(sys.argv[1]) if len(sys.argv) > 1 else ROOT / "scripts" / "site" / "csp.py"


def check(page: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-B", str(CHECK), "--check", str(page)],
        capture_output=True,
        text=True,
        timeout=60,
    )


def hashed_in_svg(manual: str) -> str:
    """A style inside SVG whose raw-text hash is admitted by the policy."""
    body = "p::after { content: '&amp;'; }"
    digest = "'sha256-" + base64.b64encode(hashlib.sha256(body.encode()).digest()).decode() + "'"
    page = manual.replace("</body>", f"<svg><style>{body}</style></svg></body>", 1)
    # Appended last, where csp.py --write would put the element's hash.
    return re.sub(r"(style-src [^;\"]*)", lambda match: f"{match.group(1)} {digest}", page, count=1)


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
        # Spellings the HTML tokenizer accepts and a pattern match would miss.
        "end-tag-whitespace": manual.replace("</body>", "<style>p { color: red; }</style ></body>", 1),
        "slash-separated-attribute": manual.replace("</body>", '<div/style="color: red">x</div></body>', 1),
        "data-src-inline-script": manual.replace("</body>", '<script data-SRC="unused">window.x = 1;</script></body>', 1),
        # Inside SVG a style is parsed as foreign content (entities decoded),
        # so its hash cannot be read off the raw text; any such style refuses.
        "style-in-svg": manual.replace("</body>", "<svg><style>p { color: red; }</style></svg></body>", 1),
        "hashed-style-in-svg": hashed_in_svg(manual),
        "unclosed-style-at-end": manual + "<style>p { color: red; }",
        "self-closing-style": manual.replace("</body>", "<style/>p { color: red; }</style></body>", 1),
        "self-closing-script": manual.replace("</body>", "<script/>window.x = 1;</script></body>", 1),
        "duplicate-rel-attribute": manual.replace("</head>", '<link rel="stylesheet" rel="icon" href="x.css"></head>', 1),
        "second-policy-meta": manual.replace(
            "<head>", '<head><meta http-equiv="content-security-policy" content="default-src *">', 1
        ),
    }
    base = Path(tempfile.mkdtemp(prefix="devtrim-csp-controls."))
    try:
        for label, text in planted.items():
            if text == manual:
                failures.append(f"{label}: the planted change did not apply")
                continue
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
