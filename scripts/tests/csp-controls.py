#!/usr/bin/env python3
"""Prove scripts/site/csp.py --check can fail.

The shipped pages must pass (positive control). Copies with a stale style hash,
an inline style element or style attribute spelled any way a browser accepts
(upper case, whitespace before an end tag's bracket, an attribute after a
slash, a style inside SVG), an inline script whose only "src" is data-src, a
self-closing script or style and a repeated attribute (each read one way by the
browser and another by a naive reader), any script or style once SVG or MathML
has opened (parsed as foreign content, even past a stray end tag), an element
left unclosed at the end, markup outside the subset csp.py reads (an unlisted
element such as iframe or template, srcdoc or an event handler, a mis-nested end
tag, anything before the policy meta but the doctype, <html>, <head>, other
metas and whitespace, the policy meta outside <head>, "<!--" or "</" in any
script or style, a NUL), a second policy meta, and an 'unsafe-inline' source
must each fail.
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


def check(page: Path, mode: str = "--check") -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, "-B", str(CHECK), mode, str(page)],
        capture_output=True,
        text=True,
        timeout=60,
    )


# Plants rewritten with --write before --check, so a stale hash cannot be
# what refuses them.
REWRITTEN = {"double-escaped-script", "double-escaped-src-script", "nul-in-style", "end-tag-like-text-in-script"}


def hashed_in_svg(manual: str, opening: str = "<svg>") -> str:
    """A style inside SVG whose raw-text hash is admitted by the policy."""
    body = "p::after { content: '&amp;'; }"
    digest = "'sha256-" + base64.b64encode(hashlib.sha256(body.encode()).digest()).decode() + "'"
    page = manual.replace("</body>", f"{opening}<style>{body}</style></svg></body>", 1)
    # Appended last, where csp.py --write would put the element's hash.
    return re.sub(r"(style-src [^;\"]*)", lambda match: f"{match.group(1)} {digest}", page, count=1)


def csp_moved(manual: str, opening: str, closing: str) -> str:
    """The page with its policy meta moved out of <head> into the body."""
    meta = re.search(r'<meta http-equiv="Content-Security-Policy"[^>]*>', manual).group(0)
    return manual.replace(meta, "", 1).replace("</body>", f"{opening}{meta}{closing}</body>", 1)


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
        # The browser ignores an unmatched end tag and stays inside SVG.
        "hashed-style-after-stray-end-tag": hashed_in_svg(manual, "<svg></math>"),
        "unclosed-style-at-end": manual + "<style>p { color: red; }",
        "self-closing-style": manual.replace("</body>", "<style/>p { color: red; }</style></body>", 1),
        "self-closing-script": manual.replace("</body>", "<script/>window.x = 1;</script></body>", 1),
        "duplicate-rel-attribute": manual.replace("</head>", '<link rel="stylesheet" rel="icon" href="x.css"></head>', 1),
        # The subset rule: only the elements these pages use, closed in
        # order, no srcdoc or event handler, the policy meta in <head>.
        "policy-meta-in-template": csp_moved(manual, "<template>", "</template>"),
        "policy-meta-in-noscript": csp_moved(manual, "<noscript>", "</noscript>"),
        "policy-meta-in-body": csp_moved(manual, "<div>", "</div>"),
        # A body element or text in the head makes the browser close the
        # head there, so the policy meta after it lands in the body.
        "body-element-before-policy": manual.replace(
            '<meta http-equiv="Content-Security-Policy"', '<div></div><meta http-equiv="Content-Security-Policy"', 1
        ),
        "text-before-policy": manual.replace(
            '<meta http-equiv="Content-Security-Policy"', 'x<meta http-equiv="Content-Security-Policy"', 1
        ),
        "entity-before-policy": manual.replace(
            '<meta http-equiv="Content-Security-Policy"', '&nbsp;<meta http-equiv="Content-Security-Policy"', 1
        ),
        # With scripting on, a browser reads noscript as raw text, so the end
        # tag inside the attribute closes it and the div closes the head.
        "noscript-raw-text-escape": manual.replace(
            '<meta http-equiv="Content-Security-Policy"',
            '<noscript><link href="</noscript><div></div>"></noscript><meta http-equiv="Content-Security-Policy"',
            1,
        ),
        "body-before-head": manual.replace("<head>", "<body></body><head>", 1),
        # "<!--" in script text enters the browser's escaped states, where
        # a later </script> may not end the script where the tokenizer says.
        "double-escaped-script": manual.replace(
            "</body>", "<script><!--<script></script><style></script></style></body>", 1
        ),
        "double-escaped-src-script": manual.replace(
            "</body>",
            '<script src=""><!--<script></script><style></script><script>window.x=1;</script></style></body>',
            1,
        ),
        # The browser turns NUL into U+FFFD before tokenizing, so the text it
        # hashes is not the text read here.
        # Parsers differ on which "</..." ends raw text.
        "end-tag-like-text-in-script": manual.replace("</body>", "<script>x = '</script/x>';</script></body>", 1),
        "nul-in-style": manual.replace("</body>", "<style>p::after { content: '\x00'; }</style></body>", 1),
        "srcdoc-iframe": manual.replace(
            "</body>", '<iframe srcdoc="<style>p{color:red}</style>"></iframe></body>', 1
        ),
        "bare-iframe": manual.replace("</body>", "<iframe></iframe></body>", 1),
        "unlisted-element": manual.replace("</body>", "<aside>x</aside></body>", 1),
        "misnested-end-tag": manual.replace("</body>", "<p><span>x</p></span></body>", 1),
        "event-handler-attribute": manual.replace("</body>", '<div onclick="x()">x</div></body>', 1),
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
            if label in REWRITTEN:
                # The attack hashes what csp.py reads; only a refusal stops it.
                check(page, "--write")
            if check(page).returncode == 0:
                failures.append(f"{label}: csp.py --check accepted a page it must refuse")
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
