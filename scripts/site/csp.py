#!/usr/bin/env python3
"""Keep each page's CSP admitting its inline code by hash, never 'unsafe-inline'.

usage: scripts/site/csp.py --check|--write <page.html>...

For every inline <script> (one with no src) and every <style> element, the
SHA-256 of its exact text goes into script-src or style-src; the rest of the
policy is left as written. --check fails when a page's policy lists anything
else, admits 'unsafe-inline', or the page carries a style="" attribute, which
no hash can admit. --write rewrites the two lists in place.

The page is read with the standard library's HTML tokenizer, not patterns:
a browser accepts <STYLE>, </style >, <div/style=...> and data-src as
written, and the inventory must see what the browser sees.
"""

from __future__ import annotations

import base64
import hashlib
import re
import sys
from html.parser import HTMLParser
from pathlib import Path

# Only to rewrite the policy text in place; the tokenizer decides which meta
# is the policy and that there is exactly one.
META = re.compile(r'(<meta\s+http-equiv="Content-Security-Policy"\s+content=")([^"]*)(")', re.I)


class Inventory(HTMLParser):
    """Inline scripts, style elements, style attributes, stylesheets and CSP metas."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=False)
        self.scripts: list[str] = []
        self.styles: list[str] = []
        self.style_attributes: list[str] = []
        self.stylesheet = False
        self.policies: list[str] = []
        self.ambiguous: list[str] = []
        self.foreign = 0
        self.inline: str | None = None
        self.text = ""

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        names = {name for name, _ in attrs}
        if len(names) != len(attrs):
            # A browser keeps the first of two equal attributes; refuse rather
            # than guess which one a reader of this page would act on.
            self.ambiguous.append(f"<{tag}> repeats an attribute")
        values: dict[str, str] = {}
        for name, value in attrs:
            values.setdefault(name, value or "")
        if "style" in names:
            self.style_attributes.append(tag)
        if tag == "link" and "stylesheet" in values.get("rel", "").lower().split():
            self.stylesheet = True
        if tag == "meta" and values.get("http-equiv", "").lower() == "content-security-policy":
            self.policies.append(values.get("content", ""))
        if tag in ("svg", "math"):
            self.foreign += 1
        if tag in ("script", "style") and self.foreign:
            # Foreign content decodes entities in its text, so the hash a
            # browser computes is not the hash of the raw text read here.
            self.ambiguous.append(f"<{tag}> inside <svg> or <math> is parsed as foreign content")
        if (tag == "script" and "src" not in names) or tag == "style":
            self.inline, self.text = tag, ""

    def handle_startendtag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        # A browser ignores the slash on an HTML script or style and reads
        # what follows as its contents; the tokenizer here would not.
        if tag in ("script", "style"):
            self.ambiguous.append(f"<{tag}/> is not self-closing in HTML")
        self.handle_starttag(tag, attrs)
        self.inline = None

    def handle_data(self, data: str) -> None:
        if self.inline:
            self.text += data

    def handle_endtag(self, tag: str) -> None:
        if tag in ("svg", "math") and self.foreign:
            self.foreign -= 1
        if tag == self.inline:
            (self.scripts if tag == "script" else self.styles).append(self.text)
            self.inline = None


def inventory(page: str) -> Inventory:
    found = Inventory()
    found.feed(page)
    found.close()
    if found.inline:
        # A browser runs an element left open at the end of the page.
        found.ambiguous.append(f"<{found.inline}> is never closed")
    return found


def digest(text: str) -> str:
    return "'sha256-" + base64.b64encode(hashlib.sha256(text.encode()).digest()).decode() + "'"


def expected_policy(found: Inventory, policy: str) -> str:
    sources = {
        "script-src": [digest(body) for body in found.scripts],
        "style-src": (["'self'"] if found.stylesheet else []) + [digest(body) for body in found.styles],
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
        found = inventory(page)
        metas = list(META.finditer(page))
        if len(found.policies) != 1 or len(metas) != 1 or metas[0].group(2) != found.policies[0]:
            failures.append(f"{name}: needs exactly one Content-Security-Policy meta, written as http-equiv then content")
            continue
        match = metas[0]
        if found.ambiguous:
            failures.append(f"{name}: ambiguous markup: {found.ambiguous[0]}")
        if found.style_attributes:
            failures.append(
                f"{name}: a style=\"\" attribute on <{found.style_attributes[0]}> cannot be admitted by hash; use a class"
            )
        wanted = expected_policy(found, match.group(2))
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
