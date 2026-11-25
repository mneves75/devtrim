#!/usr/bin/env python3
"""Check docs/incidents.md: every row's version, commit and catcher must exist.

usage: audit-incidents.py [--ledger PATH]

Run from the repository root; it needs full Git history. The ledger defaults
to docs/incidents.md, and the controls pass a planted copy.
"""

import re
import subprocess
import sys
from pathlib import Path

HEADER = ["ID", "Shipped", "Class", "Commit", "Catcher"]
NO_COMMIT = "—"


def rows(ledger: str) -> list[list[str]]:
    body = ledger.split("\n## Ledger\n", 1)
    if len(body) != 2:
        raise ValueError("no `## Ledger` section")
    table = [line for line in body[1].splitlines() if line.startswith("|")]
    if len(table) < 3:
        raise ValueError("the ledger table has no rows")
    cells = [[cell.strip() for cell in line.strip().strip("|").split("|")] for line in table]
    if cells[0] != HEADER:
        raise ValueError(f"header must be {' | '.join(HEADER)}")
    if not all(re.fullmatch(r":?-+:?", cell) for cell in cells[1]) or len(cells[1]) != len(HEADER):
        raise ValueError("second line must separate the header")
    return cells[2:]


def changelog_versions() -> set[str]:
    text = Path("CHANGELOG.md").read_text(encoding="utf-8")
    return set(re.findall(r"^## \[([^\]]+)\]", text, re.MULTILINE))


def declared_version() -> str:
    text = Path("Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'^version = "([^"]+)"', text, re.MULTILINE)
    if not match:
        raise ValueError("Cargo.toml declares no version")
    return match.group(1)


def planted_cases() -> set[str]:
    text = Path("scripts/tests/planted-violations.py").read_text(encoding="utf-8")
    return set(re.findall(r'^\s*name="([^"]+)",$', text, re.MULTILINE))


def rust_tests() -> set[str]:
    names = set()
    for path in [*Path("src").rglob("*.rs"), *Path("tests").rglob("*.rs")]:
        text = path.read_text(encoding="utf-8")
        names.update(re.findall(r"#\[test\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))? )?fn ([a-z0-9_]+)\(", text))
    return names


def reachable(commit: str) -> bool:
    if not re.fullmatch(r"[0-9a-f]{7,40}", commit):
        return False
    resolved = subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", f"{commit}^{{commit}}"],
        capture_output=True,
        text=True,
    )
    if resolved.returncode != 0:
        return False
    ancestor = subprocess.run(
        ["git", "merge-base", "--is-ancestor", resolved.stdout.strip(), "HEAD"],
        capture_output=True,
    )
    return ancestor.returncode == 0


def audit(ledger: str) -> list[str]:
    failures = []
    try:
        table = rows(ledger)
    except ValueError as error:
        return [str(error)]
    versions = changelog_versions()
    in_flight = {"Unreleased", declared_version()}
    catchers = planted_cases() | rust_tests()
    seen = set()
    for number, cells in enumerate(table, start=1):
        if len(cells) != len(HEADER):
            failures.append(f"row {number}: {len(cells)} cells, expected {len(HEADER)}")
            continue
        identifier, shipped, kind, commit, catcher = cells
        label = identifier or f"row {number}"
        if not re.fullmatch(r"D-\d{2,}", identifier):
            failures.append(f"{label}: ID must look like D-01")
        elif identifier in seen:
            failures.append(f"{label}: duplicate ID")
        seen.add(identifier)
        if shipped not in versions:
            failures.append(f"{label}: Shipped `{shipped}` is no CHANGELOG.md version")
        if not kind:
            failures.append(f"{label}: Class is empty")
        if commit == NO_COMMIT:
            if shipped not in in_flight:
                failures.append(f"{label}: `{NO_COMMIT}` is allowed only for an in-flight release, not {shipped}")
        elif not reachable(commit):
            failures.append(f"{label}: Commit `{commit}` is not a commit reachable from HEAD")
        if catcher not in catchers:
            failures.append(f"{label}: Catcher `{catcher}` is neither a planted case nor a #[test] fn")
    return failures


def main() -> int:
    args = sys.argv[1:]
    ledger = Path("docs/incidents.md")
    if args[:1] == ["--ledger"] and len(args) == 2:
        ledger = Path(args[1])
    elif args:
        print(__doc__.strip().splitlines()[2], file=sys.stderr)
        return 2
    failures = audit(ledger.read_text(encoding="utf-8"))
    for failure in failures:
        print(f"FAIL {failure}")
    if failures:
        return 1
    print(f"audit-incidents: {len(rows(ledger.read_text(encoding='utf-8')))} incident(s), each with a live catcher")
    return 0


if __name__ == "__main__":
    sys.exit(main())
