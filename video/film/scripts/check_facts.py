#!/usr/bin/env python3
"""Prove every line the film shows comes verbatim from facts.json.

usage: check_facts.py <facts.json> <copy.json> <src-dir>...

copy.json maps each on-screen line id to {"text", "fact"}; the text must be a
substring of that fact's text, exactly as written. The source directories are
then scanned: every `line("id")` call must name an id in copy.json, and every
id in copy.json must appear in the source as a quoted string (a `line()` call
or a literal list the source maps through `line()`). ESLint's
react/jsx-no-literals (video/eslint.config.mjs) refuses any string written
straight into the film's JSX, so nothing bypasses copy.json. Exits 1 on any
failure.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

LINE_CALL = re.compile(r"\bline\(\s*\"([A-Za-z0-9_.-]+)\"\s*\)")
QUOTED = re.compile(r"\"([A-Za-z0-9_.-]+)\"")


def main() -> int:
    if len(sys.argv) < 4:
        print(__doc__, file=sys.stderr)
        return 2
    facts = {f["id"]: f for f in json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))}
    copy = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
    failures = []
    for key, entry in copy.items():
        fact = facts.get(entry.get("fact", ""))
        if fact is None:
            failures.append(f"{key}: cites unknown fact {entry.get('fact')!r}")
        elif entry["text"] not in fact["text"]:
            failures.append(f"{key}: {entry['text']!r} is not in {fact['id']} {fact['text']!r}")
    used = set()
    for root in sys.argv[3:]:
        for path in sorted(Path(root).rglob("*.tsx")):
            source = path.read_text(encoding="utf-8")
            used.update(m.group(1) for m in QUOTED.finditer(source) if m.group(1) in copy)
            for match in LINE_CALL.finditer(source):
                if match.group(1) not in copy:
                    failures.append(f"{path}: line({match.group(1)!r}) is not in copy.json")
    for key in sorted(set(copy) - used):
        failures.append(f"{key}: in copy.json but never shown")
    for failure in failures:
        print(f"FAIL {failure}")
    print(f"check_facts.py: {len(copy)} lines, {len(used)} used, {len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
