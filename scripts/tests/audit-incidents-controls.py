#!/usr/bin/env python3
"""Prove audit-incidents.py refuses each kind of broken ledger row.

Each control plants one defect in a copy of docs/incidents.md and requires the
audit to exit 1 naming it; the unmodified ledger must pass. The first control
replays the real drift the ledger exists for: 33fe4e2's VM-disk test, removed
in 0.11.0 without anything noticing.
"""

import importlib.util
import subprocess
import sys
import tempfile
from pathlib import Path

AUDIT = Path(__file__).with_name("audit-incidents.py")
LEDGER = Path("docs/incidents.md").read_text(encoding="utf-8")
D01 = "| 33fe4e2 | eval_clean_docker_prunes_images_and_build_cache_never_volumes |"
D07 = "| D-07 | 0.15.0 |"

CONTROLS = [
    (
        "removed catcher (0.11.0 drift)",
        (D01, "| 33fe4e2 | apply_skips_the_report_only_vm_disk_finding_instead_of_refusing_it |"),
        "is neither a planted case nor a #[test] fn",
    ),
    ("unknown planted case", ("| git/signature-program |", "| git/signature-programs |"), "neither a planted case"),
    ("ignored test", ("| cf7962b | journal-stop/model |", "| cf7962b | concurrent_writer_child |"), "neither a planted case"),
    ("helper fn, not a test", ("| every_lockfile_pins_the_version_cargo_toml_declares |", "| locked_version |"), "neither a planted case"),
    ("duplicate ID", (D07, "| D-06 | 0.15.0 |"), "duplicate ID"),
    ("malformed ID", (D07, "| I-7 | 0.15.0 |"), "ID must look like D-01"),
    ("unknown version", (D07, "| D-07 | 0.15.9 |"), "is no CHANGELOG.md version"),
    ("missing commit", ("| cf7962b |", "| 0000000 |"), "not a commit reachable from HEAD"),
    ("not a commit", ("| cf7962b |", "| main |"), "not a commit reachable from HEAD"),
    ("no commit on a shipped row", ("| cf7962b |", "| — |"), "allowed only for an in-flight release"),
    ("missing cell", ("| cf7962b | journal-stop/model |", "| journal-stop/model |"), "cells, expected 5"),
    (
        "empty class",
        ("| A stop between the stop check and uv's lock left `<cache>/.lock` with no journal record |", "|  |"),
        "Class is empty",
    ),
    ("renamed header", ("| ID | Shipped |", "| Id | Shipped |"), "header must be"),
    ("no ledger section", ("\n## Ledger\n", "\n## Rows\n"), "no `## Ledger` section"),
]


def run(ledger: str) -> subprocess.CompletedProcess:
    with tempfile.NamedTemporaryFile("w", suffix=".md", delete=False, encoding="utf-8") as handle:
        handle.write(ledger)
        path = Path(handle.name)
    try:
        return subprocess.run(
            [sys.executable, "-B", str(AUDIT), "--ledger", str(path)],
            capture_output=True,
            text=True,
        )
    finally:
        path.unlink()


# Catcher shapes `cargo test` never runs, beside two it does.
FIXTURE = """
#[test]
fn runs() {}
#[ignore]
#[test]
fn ignored_first() {}
#[test]
#[ignore = "helper"]
fn ignored_after() {}
#[cfg(any())]
#[test]
fn compiled_out() {}
/*
#[test]
fn commented_out() {}
*/
// #[test] fn line_commented() {}
#[cfg(target_os = "macos")]
#[test]
fn on_macos() {}
"""


def fixture_failures() -> int:
    spec = importlib.util.spec_from_file_location("audit_incidents", AUDIT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    with tempfile.TemporaryDirectory() as root:
        Path(root, "lib.rs").write_text(FIXTURE, encoding="utf-8")
        found = module.rust_tests((Path(root),))
    if found != {"runs", "on_macos"}:
        print(f"FAIL non-running catchers: expected only runs and on_macos, got {sorted(found)}")
        return 1
    print("REFUSED ignored, compiled-out and commented catchers")
    return 0


def main() -> int:
    failures = 0
    failures += fixture_failures()
    original = run(LEDGER)
    if original.returncode != 0:
        print(f"FAIL the unmodified ledger must pass:\n{original.stdout}{original.stderr}")
        failures += 1
    for name, (before, after), expected in CONTROLS:
        if LEDGER.count(before) != 1:
            print(f"FAIL {name}: anchor {before!r} must occur exactly once")
            failures += 1
            continue
        result = run(LEDGER.replace(before, after))
        if result.returncode != 1 or expected not in result.stdout:
            print(f"FAIL {name}: expected exit 1 naming {expected!r}, got {result.returncode}:\n{result.stdout}{result.stderr}")
            failures += 1
        else:
            print(f"REFUSED {name}")
    if failures:
        return 1
    print(f"audit-incidents-controls: {len(CONTROLS)} planted defect(s) refused")
    return 0


if __name__ == "__main__":
    sys.exit(main())
