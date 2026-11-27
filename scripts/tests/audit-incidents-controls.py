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
        "is neither a planted case nor a test `cargo test` runs",
    ),
    ("unknown planted case", ("| git/signature-program |", "| git/signature-programs |"), "neither a planted case"),
    ("ignored test", ("| cf7962b | journal-stop/model |", "| cf7962b | concurrent_writer_child |"), "neither a planted case"),
    (
        "ambiguous partial test path",
        ("| cf7962b | journal-stop/model |", "| cf7962b | tests::active_build_cwd_skips_repo_and_refuses_apply |"),
        "name it by its full path",
    ),
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


def load_audit():
    spec = importlib.util.spec_from_file_location("audit_incidents", AUDIT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def refused(name: str, failures: list[str], expected: str) -> int:
    if not any(expected in failure for failure in failures):
        print(f"FAIL {name}: expected {expected!r}, got {failures}")
        return 1
    print(f"REFUSED {name}")
    return 0


def module_failures() -> int:
    """Defects a ledger edit alone cannot plant: a tagged declared version, a
    namesake compiled out beside one that runs, and a test listing that fails."""
    count = 0
    module = load_audit()
    module.declared_version = lambda: "0.17.1"
    planted = LEDGER.replace(D07, "| D-07 | 0.17.1 |").replace("| cf7962b |", "| — |")
    count += refused("tagged declared version", module.audit(planted), "allowed only for an in-flight release")

    module = load_audit()
    name = "active_build_cwd_skips_repo_and_refuses_apply"
    real = module.runnable_tests()
    namesakes = [path for path in real if path.endswith(f"::{name}")]
    module.runnable_tests = lambda: [path for path in real if path not in namesakes[1:]]
    planted = LEDGER.replace("| cf7962b | journal-stop/model |", f"| cf7962b | {name} |")
    count += refused("namesake compiled out", module.audit(planted), "name it by its full path")

    module = load_audit()
    real_run = module.subprocess.run

    class FailingCargo:
        @staticmethod
        def run(command, **options):
            if command[0] == "cargo":
                return subprocess.CompletedProcess(command, 101, "", "error: could not compile")
            return real_run(command, **options)

    module.subprocess = FailingCargo
    count += refused("test listing fails", module.audit(LEDGER), "failed")
    return count


MODULE_CONTROLS = 3


def main() -> int:
    failures = 0
    failures += module_failures()
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
    print(f"audit-incidents-controls: {len(CONTROLS) + MODULE_CONTROLS} planted defect(s) refused")
    return 0


if __name__ == "__main__":
    sys.exit(main())
