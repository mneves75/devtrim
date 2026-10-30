#!/usr/bin/env python3
"""Keep each page's CSP admitting its inline code by hash, never 'unsafe-inline'.

usage: scripts/site/csp.py --check|--write <page.html>...

For every inline <script> (one with no src) and every <style> element, the
SHA-256 of its exact text goes into script-src or style-src; the rest of the
policy is left as written. --check fails when a page's policy lists anything
else, admits 'unsafe-inline', or the page carries a style="" attribute, which
no hash can admit. --write rewrites the two lists in place.

The page is read by a small tokenizer of its own, not the standard library's
HTML parser, whose behavior changes between Python versions. It accepts only
the markup these pages are written in, which it reads exactly as a browser
does: the doctype, comments with no "--" inside, plain end tags, start tags
with double-quoted attribute values free of "&", script and style text that
runs to its exact end tag with no "</" or "<!--" inside, and title text with
no "<" or "&". Only the elements these pages use are accepted, every one closed
in order, with no event-handler or srcdoc attribute and no NUL, and the policy
meta preceded by nothing but the doctype, <html>, <head>, other metas and
whitespace, so the browser cannot have left the head before it. Anything else
is refused rather than modelled; a real browser load stays the ground truth.
"""

from __future__ import annotations

import base64
import hashlib
import re
import sys
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
# Text that runs to the element's exact end tag: raw text, and title's RCDATA.
RAW = frozenset({"script", "style", "title"})
# All that may precede the policy meta: none of it can make the browser close
# the head, so the meta is in the head the browser builds and is enforced.
PREFIX = frozenset({"html", "head", "meta"})
HTML_SPACE = " \t\n\f\r"
SPACE = "[ \t\n\f\r]"
DOCTYPE = re.compile(r"<!doctype html>", re.I)
END_TAG = re.compile(r"</([a-zA-Z][a-zA-Z0-9]*)>")
START_TAG = re.compile(
    rf'<([a-zA-Z][a-zA-Z0-9]*)((?:{SPACE}+[a-zA-Z][a-zA-Z0-9-]*(?:="[^"]*")?)*){SPACE}*(/?)>'
)
ATTRIBUTE = re.compile(r'([a-zA-Z][a-zA-Z0-9-]*)(?:="([^"]*)")?')


class Inventory:
    """Inline scripts, style elements, style attributes, stylesheets and CSP metas."""

    def __init__(self) -> None:
        self.scripts: list[str] = []
        self.styles: list[str] = []
        self.style_attributes: list[str] = []
        self.stylesheet = False
        self.policies: list[str] = []
        self.ambiguous: list[str] = []
        self.open: list[str] = []


def inventory(page: str) -> Inventory:
    found = Inventory()
    refuse = found.ambiguous.append
    if "\x00" in page:
        # The browser replaces NUL before tokenizing, so it hashes other text.
        refuse("a NUL character")
    at = 0
    while at < len(page):
        policy_seen = bool(found.policies)
        lt = page.find("<", at)
        if not policy_seen and page[at : len(page) if lt == -1 else lt].strip(HTML_SPACE):
            refuse("text comes before the policy meta")
        if lt == -1:
            break
        at = lt
        if page.startswith("<!--", at):
            end = page.find("-->", at + 4)
            body = page[at + 4 : end]
            # A browser ends "<!-->" and "<!--->" at once and a comment at
            # "--!>", and Python's parsers have ended one at "-- >"; a comment
            # with no "--" inside ends at its first "-->" everywhere.
            if end == -1 or body.startswith((">", "->")) or "--" in body:
                refuse(f"the comment at {page[at : at + 20]!r} may end elsewhere in a browser")
                break
            if not policy_seen:
                refuse("a comment comes before the policy meta")
            at = end + 3
            continue
        doctype = DOCTYPE.match(page, at)
        if doctype:
            at = doctype.end()
            continue
        end_tag = END_TAG.match(page, at)
        if end_tag:
            name = end_tag.group(1).lower()
            if not policy_seen:
                refuse(f"</{name}> comes before the policy meta")
            if found.open and found.open[-1] == name:
                found.open.pop()
            else:
                # A browser repairs mis-nesting by tree-construction rules this
                # check does not model.
                refuse(f"</{name}> does not close the element open there")
            at = end_tag.end()
            continue
        start_tag = START_TAG.match(page, at)
        if start_tag is None:
            refuse(f"{page[at : at + 20]!r} is outside the markup this check reads")
            break
        name = start_tag.group(1).lower()
        pairs = [(key.lower(), value) for key, value in ATTRIBUTE.findall(start_tag.group(2))]
        names = {key for key, _ in pairs}
        values = dict(pairs)
        if name not in ELEMENTS:
            refuse(f"<{name}> is outside the elements this check reads")
        if not policy_seen and name not in PREFIX:
            refuse(f"<{name}> comes before the policy meta")
        if start_tag.group(3) and name not in VOID:
            # A browser ignores the slash on an element that is not void.
            refuse(f"<{name}/> is not self-closing in HTML")
        if len(names) != len(pairs):
            # A browser keeps the first of two equal attributes; refuse rather
            # than guess which one a reader of this page would act on.
            refuse(f"<{name}> repeats an attribute")
        for key, value in pairs:
            if key == "srcdoc" or key.startswith("on"):
                refuse(f"<{name}> carries {key}, which holds code or a page")
            if "&" in value:
                # A browser decodes character references in a value.
                refuse(f"<{name}> has a character reference in {key}")
        if "style" in names:
            found.style_attributes.append(name)
        if name == "link" and "stylesheet" in values.get("rel", "").lower().split():
            found.stylesheet = True
        if name == "meta" and values.get("http-equiv", "").lower() == "content-security-policy":
            if found.open != ["html", "head"]:
                refuse("the policy meta is not a direct child of <head>")
            found.policies.append(values.get("content", ""))
        at = start_tag.end()
        if name in RAW:
            close = re.compile(f"</{name}>", re.I).search(page, at)
            body = "" if close is None else page[at : close.start()]
            # With no "</" or "<!--" inside (the escaped script states), the
            # exact end tag is where a browser ends the text; title text also
            # decodes references, so it must not hold "<" or "&".
            if close is None or "</" in body or "<!--" in body or (name == "title" and ("<" in body or "&" in body)):
                refuse(f"<{name}> at {page[start_tag.start() : start_tag.start() + 20]!r} does not end where it seems")
                break
            if name == "style" or (name == "script" and "src" not in names):
                (found.scripts if name == "script" else found.styles).append(body)
            at = close.end()
        elif name not in VOID:
            found.open.append(name)
    if found.open:
        # A browser runs a script or style left open at the end of the page.
        refuse(f"<{found.open[-1]}> is never closed")
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
