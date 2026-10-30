#!/usr/bin/env python3
"""Keep each page's CSP admitting its inline code by hash, never 'unsafe-inline'.

usage: scripts/site/csp.py --check|--write <page.html>...

For every inline <script> (one with no src) and every <style> element, the
SHA-256 of its exact text goes into script-src or style-src; the rest of the
policy is left as written. --check fails when a page's policy lists anything
else, admits 'unsafe-inline', or the page carries a style="" attribute, which
no hash can admit. --write rewrites the two lists in place.

The page is read with the standard library's HTML tokenizer, not patterns,
and must stay inside the simple subset these pages are written in: only the
elements they use, every one closed in order, no event-handler or srcdoc
attribute, no "<!--" or "</" inside a script or style, each ending with its
exact end tag, no NUL, and the policy meta preceded by nothing but the doctype,
<html>, <head>, other metas and whitespace, so the browser cannot have left
the head before it. Anything else
is refused rather than modelled; a real browser load stays the ground truth.
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


# The elements index.html and MANUAL.html use. Others (svg, math, template,
# iframe, noscript, textarea, ...) change how the browser parses what follows.
ELEMENTS = frozenset(
    """a article b body button code dd details div dl dt em figcaption figure
    footer h1 h2 h3 head header html i input kbd label li link main meta nav
    ol output p pre script section span strong style summary table tbody td
    th thead title tr track ul video""".split()
)
VOID = frozenset({"input", "link", "meta", "track"})
# All that may precede the policy meta: none of it can make the browser close
# the head, so the meta is in the head the browser builds and is enforced.
PREFIX = frozenset({"html", "head", "meta"})
HTML_SPACE = " \t\n\f\r"


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
        self.open: list[str] = []
        self.policy_seen = False
        self.inline: str | None = None
        self.external = False
        self.text = ""

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        if tag not in ELEMENTS:
            self.ambiguous.append(f"<{tag}> is outside the elements this check reads")
        if not self.policy_seen and tag not in PREFIX:
            self.before_policy(f"<{tag}>")
        names = {name for name, _ in attrs}
        if len(names) != len(attrs):
            # A browser keeps the first of two equal attributes; refuse rather
            # than guess which one a reader of this page would act on.
            self.ambiguous.append(f"<{tag}> repeats an attribute")
        for name in names:
            if name == "srcdoc" or name.startswith("on"):
                self.ambiguous.append(f"<{tag}> carries {name}, which holds code or a page")
        values: dict[str, str] = {}
        for name, value in attrs:
            values.setdefault(name, value or "")
        if "style" in names:
            self.style_attributes.append(tag)
        if tag == "link" and "stylesheet" in values.get("rel", "").lower().split():
            self.stylesheet = True
        if tag == "meta" and values.get("http-equiv", "").lower() == "content-security-policy":
            if self.open != ["html", "head"]:
                self.ambiguous.append("the policy meta is not a direct child of <head>")
            self.policies.append(values.get("content", ""))
            self.policy_seen = True
        if tag not in VOID:
            self.open.append(tag)
        if tag in ("script", "style"):
            # A script with src is read too: its text drives the tokenizer.
            self.inline, self.text, self.external = tag, "", tag == "script" and "src" in names

    def handle_startendtag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        # A browser ignores the slash on an HTML element that is not void and
        # reads what follows as its contents; the tokenizer here would not.
        if tag not in VOID:
            self.ambiguous.append(f"<{tag}/> is not self-closing in HTML")
        self.handle_starttag(tag, attrs)

    def handle_data(self, data: str) -> None:
        if self.inline:
            self.text += data
        elif not self.policy_seen and data.strip(HTML_SPACE):
            self.before_policy("text")

    def handle_entityref(self, name: str) -> None:
        if not self.policy_seen:
            self.before_policy(f"&{name};")

    def handle_charref(self, name: str) -> None:
        if not self.policy_seen:
            self.before_policy(f"&#{name};")

    def handle_comment(self, data: str) -> None:
        if not self.policy_seen:
            self.before_policy("a comment")

    def handle_pi(self, data: str) -> None:
        self.ambiguous.append("a processing instruction is not HTML")

    def unknown_decl(self, data: str) -> None:
        self.ambiguous.append(f"<![{data[:20]}...]> is not HTML")

    def before_policy(self, what: str) -> None:
        self.ambiguous.append(f"{what} comes before the policy meta")

    def handle_endtag(self, tag: str) -> None:
        if not self.policy_seen:
            self.before_policy(f"</{tag}>")
        if self.open and self.open[-1] == tag:
            self.open.pop()
        else:
            # A browser repairs mis-nesting by tree-construction rules this
            # check does not model.
            self.ambiguous.append(f"</{tag}> does not close the element open there")
        if tag == self.inline:
            # "<!--" starts the browser's escaped script states, and parsers
            # differ on which "</..." ends raw text; with neither inside, the
            # first "</" is the end tag for every tokenizer.
            for marker in ("<!--", "</"):
                if marker in self.text:
                    self.ambiguous.append(f'"{marker}" inside a <{tag}>')
            if not self.external:
                (self.scripts if tag == "script" else self.styles).append(self.text)
            self.inline = None


# Python versions disagree with each other and with browsers on end tags
# such as "</ script>" or "</script/x>", so a script or style may end only
# with its exact end tag.
LOOSE_END_TAG = re.compile(r"</(?:[\s/]+(?:script|style)|(?:script|style)(?!>))", re.I)


def inventory(page: str) -> Inventory:
    found = Inventory()
    loose = LOOSE_END_TAG.search(page)
    if loose:
        found.ambiguous.append(f"{loose.group(0)!r} is not written as </script> or </style>")
    if "\x00" in page:
        # The browser replaces NUL before tokenizing, so it hashes other text.
        found.ambiguous.append("a NUL character")
    found.feed(page)
    found.close()
    if found.open:
        # A browser runs a script or style left open at the end of the page.
        found.ambiguous.append(f"<{found.open[-1]}> is never closed")
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
