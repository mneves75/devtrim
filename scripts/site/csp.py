#!/usr/bin/env python3
"""Keep each page's CSP admitting its inline code by hash, never 'unsafe-inline'.

usage: scripts/site/csp.py --check|--write <page.html>...

For every inline <script> (one with no src) and every <style> element, the
SHA-256 of its exact text goes into script-src or style-src; the rest of the
policy is left as written. --check fails when a page's policy lists anything
else, admits 'unsafe-inline', or the page carries a style="" attribute, which
no hash can admit. --write rewrites the two lists in place.
"""

from __future__ import annotations

import base64
import hashlib
import re
import sys
from pathlib import Path

# HTML tag and attribute names are case-insensitive, so every pattern is too:
# a browser enforces the policy on <STYLE> as on <style>.
META = re.compile(r'(<meta http-equiv="Content-Security-Policy" content=")([^"]*)(")', re.I)
SCRIPT = re.compile(r"<script(?![^>]*\bsrc=)[^>]*>(.*?)</script>", re.S | re.I)
STYLE = re.compile(r"<style[^>]*>(.*?)</style>", re.S | re.I)
STYLE_ATTRIBUTE = re.compile(r"<[a-zA-Z][^>]*\sstyle\s*=", re.S | re.I)
STYLESHEET = re.compile(r'<link[^>]*rel="stylesheet"', re.I)


def digest(text: str) -> str:
    return "'sha256-" + base64.b64encode(hashlib.sha256(text.encode()).digest()).decode() + "'"


def expected_policy(page: str, policy: str) -> str:
    sources = {
        "script-src": [digest(body) for body in SCRIPT.findall(page)],
        "style-src": (["'self'"] if STYLESHEET.search(page) else [])
        + [digest(body) for body in STYLE.findall(page)],
    }
    directives = [part.strip() for part in policy.split(";") if part.strip()]
    rebuilt = []
    for directive in directives:
        name = directive.split()[0]
        if name in sources:
            values = sources.pop(name) or ["'none'"]
            rebuilt.append(" ".join([name, *values]))
        else:
            rebuilt.append(directive)
    rebuilt.extend(" ".join([name, *values]) for name, values in sources.items() if values)
    return "; ".join(rebuilt)


def main() -> int:
    if len(sys.argv) < 3 or sys.argv[1] not in ("--check", "--write"):
        print(__doc__.strip().splitlines()[2], file=sys.stderr)
        return 2
    write = sys.argv[1] == "--write"
    failures = []
    for name in sys.argv[2:]:
        path = Path(name)
        page = path.read_text()
        match = META.search(page)
        if not match:
            failures.append(f"{name}: no Content-Security-Policy meta")
            continue
        if STYLE_ATTRIBUTE.search(page):
            failures.append(f"{name}: a style=\"\" attribute cannot be admitted by hash; use a class")
        wanted = expected_policy(page, match.group(2))
        if "'unsafe-inline'" in wanted:
            failures.append(f"{name}: the policy admits 'unsafe-inline'")
        if wanted == match.group(2):
            continue
        if write:
            path.write_text(page[: match.start(2)] + wanted + page[match.end(2) :])
            print(f"{name}: policy updated")
        else:
            failures.append(f"{name}: stale CSP\n  served:   {match.group(2)}\n  expected: {wanted}")
    for failure in failures:
        print(f"csp: {failure}", file=sys.stderr)
    if failures:
        return 1
    if not write:
        print(f"csp: {len(sys.argv) - 2} page(s) admit exactly their inline code by hash")
    return 0


if __name__ == "__main__":
    sys.exit(main())
