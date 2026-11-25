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


def runnable_tests() -> list[str]:
    """The full paths of what `cargo test` runs, asked of the test harness
    itself: a test compiled out (a `cfg`, a file no module declares) is never
    listed, and an `#[ignore]`d one is listed again under `--ignored`."""

    def listed(*extra: str) -> set[str]:
        command = ["cargo", "test", "--locked", "--all-targets", "--all-features", "--quiet", "--", "--list", *extra]
        result = subprocess.run(command, capture_output=True, text=True)
        if result.returncode != 0:
            raise ValueError(f"`{' '.join(command)}` failed:\n{result.stderr[-2000:]}")
        return [line.removesuffix(": test") for line in result.stdout.splitlines() if line.endswith(": test")]

    ignored = set(listed("--ignored"))
    return [path for path in listed() if path not in ignored]


def test_problem(catcher: str, tests: list[str]) -> str | None:
    """A catcher names exactly one test that runs. A bare name defined twice
    must be qualified, or a namesake still running would hide a copy that no
    longer does."""
    matches = [path for path in tests if path == catcher or path.endswith(f"::{catcher}")]
    if not matches:
        return "is neither a planted case nor a test `cargo test` runs"
    if len(matches) > 1:
        return f"matches {len(matches)} tests `cargo test` runs; name it by its full path"
    if "::" not in catcher:
        pattern = re.compile(rf"\bfn\s+{re.escape(catcher)}\s*[<(]")
        sources = [path for root in (Path("src"), Path("tests")) for path in root.rglob("*.rs")]
        defined = sum(len(pattern.findall(path.read_text(encoding="utf-8"))) for path in sources)
        if defined > 1:
            return f"is defined {defined} times but `cargo test` runs one; name it by its full path"
    return None


def tagged(version: str) -> bool:
    result = subprocess.run(
        ["git", "rev-parse", "--verify", "--quiet", f"refs/tags/v{version}"], capture_output=True
    )
    return result.returncode == 0


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
    declared = declared_version()
    # The version Cargo.toml declares is in flight only until it is tagged.
    in_flight = {"Unreleased"} | ({declared} if not tagged(declared) else set())
    try:
        planted, tests = planted_cases(), runnable_tests()
    except ValueError as error:
        return [str(error)]
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
        problem = None if catcher in planted else test_problem(catcher, tests)
        if problem:
            failures.append(f"{label}: Catcher `{catcher}` {problem}")
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
