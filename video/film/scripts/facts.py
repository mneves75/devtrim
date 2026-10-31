#!/usr/bin/env python3
"""Pull every sentence the film may quote from the product's own pages into facts.json.

usage: facts.py <site-url> <page.html>... > facts.json
       facts.py --check facts.json <site-url> <page.html>...

Each fact is one block of visible text (a heading, paragraph, list item,
table cell, caption or one line of a preformatted terminal frame) with the
link to the nearest anchor above it, so every line on screen traces back to
the page that says it. Its id is "f" plus the first 8 hex digits of the
text's SHA-256. Scripts and styles are skipped; whitespace is collapsed.
--check regenerates the facts and exits 1 when the given facts.json differs, so
copy is never checked against a snapshot of pages that have since changed.
"""

from __future__ import annotations

import hashlib
import json
import sys
from html.parser import HTMLParser
from pathlib import Path

BLOCKS = {"h1", "h2", "h3", "h4", "p", "li", "dt", "dd", "td", "th", "figcaption", "summary", "title"}
SKIP = {"script", "style"}


class Collector(HTMLParser):
    def __init__(self, link: str) -> None:
        super().__init__(convert_charrefs=True)
        self.link = link
        self.anchor = ""
        self.stack: list[str] = []
        self.buffer: list[str] = []
        self.pre_depth = 0
        self.skip_depth = 0
        self.facts: list[dict[str, str]] = []

    def flush(self) -> None:
        text = "".join(self.buffer)
        self.buffer = []
        if self.pre_depth:
            lines = text.split("\n")
        else:
            lines = [" ".join(text.split())]
        for line in lines:
            line = line.rstrip() if self.pre_depth else line.strip()
            if line.strip():
                self.facts.append({"text": line, "link": f"{self.link}#{self.anchor}" if self.anchor else self.link})

    def handle_starttag(self, tag: str, attrs: list[tuple[str, str | None]]) -> None:
        identifier = dict(attrs).get("id")
        if identifier:
            self.anchor = identifier
        if tag in SKIP:
            self.skip_depth += 1
        elif tag == "pre":
            self.flush()
            self.pre_depth += 1
        elif tag in BLOCKS and not self.pre_depth:
            self.flush()
        elif tag == "br":
            self.buffer.append("\n" if self.pre_depth else " ")

    def handle_endtag(self, tag: str) -> None:
        if tag in SKIP:
            self.skip_depth = max(0, self.skip_depth - 1)
        elif tag == "pre":
            self.flush()
            self.pre_depth = max(0, self.pre_depth - 1)
        elif tag in BLOCKS and not self.pre_depth:
            self.flush()

    def handle_data(self, data: str) -> None:
        if not self.skip_depth:
            self.buffer.append(data)


def main() -> int:
    args = sys.argv[1:]
    check = None
    if args[:1] == ["--check"]:
        if len(args) < 2:
            print(__doc__, file=sys.stderr)
            return 2
        check, args = Path(args[1]), args[2:]
    if len(args) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    site = args[0].rstrip("/")
    facts = []
    for page in args[1:]:
        name = Path(page).name
        collector = Collector(f"{site}/" if name == "index.html" else f"{site}/{name}")
        collector.feed(Path(page).read_text(encoding="utf-8"))
        collector.close()
        collector.flush()
        facts.extend(collector.facts)
    # The site's own address is shown on the end card; its evidence is the page itself.
    facts.append({"text": site.split("://", 1)[-1], "link": f"{site}/"})
    seen = set()
    unique = []
    for fact in facts:
        if fact["text"] not in seen:
            seen.add(fact["text"])
            # The id is a digest of the text, so editing one page line never renumbers the rest.
            unique.append({"id": "f" + hashlib.sha256(fact["text"].encode()).hexdigest()[:8], **fact})
    if check is not None:
        committed = json.loads(check.read_text(encoding="utf-8"))
        if committed == unique:
            print(f"facts.py: {check} matches the pages ({len(unique)} facts)")
            return 0
        old, new = {f["text"] for f in committed}, {f["text"] for f in unique}
        for text in sorted(old - new):
            print(f"FAIL stale fact, no longer on the pages: {text}")
        for text in sorted(new - old):
            print(f"FAIL new page text missing from {check.name}: {text}")
        if old == new:
            print(f"FAIL {check.name} differs from the pages in order, ids or links")
        print(f"facts.py: {check} is stale; regenerate it with facts.py")
        return 1
    json.dump(unique, sys.stdout, ensure_ascii=False, indent=1)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
