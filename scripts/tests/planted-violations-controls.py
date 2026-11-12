#!/usr/bin/env python3
"""Prove scripts/tests/planted-violations.py reports every planted fault.

The harness runs its mutation stage on isolated workers and no longer stops at
the first failing case, so a fault must be reported however the cases are
spread over the workers. This runs the real harness on two workers against a
doctored table built from cheap unit-test cases: one real case is the positive
control and must be caught; a no-op mutant, a mutant that does not compile, a
marker that never appears, and a worker that raises must each be reported as a
failure, by case name, with exit status 1.
"""

from __future__ import annotations

import contextlib
import importlib.util
import io
import os
import sys
from pathlib import Path

HARNESS_PATH = Path(__file__).resolve().parent / "planted-violations.py"
WORKER_FAULT = "planted worker fault"


def load_harness():
    # The filename holds a hyphen, so it cannot be imported by name.
    spec = importlib.util.spec_from_file_location("planted_violations", HARNESS_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main() -> int:
    harness = load_harness()
    real = {case.name: case for case in harness.CASES}
    caught = real["report/quotes-literal"]
    other = real["journal/bounded-error"]

    def variant(name: str, basis, **changes):
        fields = {
            "name": name,
            "relative_path": basis.relative_path,
            "before": basis.before,
            "after": basis.after,
            "tests": basis.tests,
            "marker": basis.marker,
            "target": basis.target,
        }
        fields.update(changes)
        return harness.Case(**fields)

    harness.CASES = (
        variant("control/no-op", caught, after="    // planted no-op mutant\n" + caught.before),
        variant(
            "control/does-not-compile",
            caught,
            after='    compile_error!("planted compile failure");\n' + caught.before,
        ),
        variant("control/wrong-marker", other, marker="PV control/never-printed"),
        variant("control/worker-fault", other),
        caught,
    )

    prove_case = harness.prove_case

    def faulty_prove_case(case, *arguments):
        if case.name == "control/worker-fault":
            raise RuntimeError(WORKER_FAULT)
        return prove_case(case, *arguments)

    harness.prove_case = faulty_prove_case
    os.environ["DEVTRIM_PV_JOBS"] = "2"
    os.environ.pop("DEVTRIM_PV_TEST_JOBS", None)
    sys.argv = [str(HARNESS_PATH)]
    stdout, stderr = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        try:
            status = harness.main()
        except SystemExit as stop:
            status = stop.code
    output, errors = stdout.getvalue(), stderr.getvalue()

    expectations = [
        ("exit status 1", status == 1),
        ("MISSED for the no-op mutant", "MISSED control/no-op: boundary violation survived" in errors),
        ("compile failure", "ERROR control/does-not-compile: mutant did not compile" in errors),
        ("marker mismatch", "ERROR control/wrong-marker: " in errors and "not at the named assertion" in errors),
        ("worker exception", f"ERROR control/worker-fault: worker failed: {WORKER_FAULT}" in errors),
        ("positive control caught", f"CAUGHT {caught.name}: " in output),
        ("no planted fault caught", "CAUGHT control/" not in output),
    ]
    missing = [label for label, held in expectations if not held]
    if missing:
        print(f"planted-violations-controls: exit status {status}", file=sys.stderr)
        print(f"--- stdout\n{output}--- stderr\n{errors}", file=sys.stderr)
        for label in missing:
            print(f"FAIL: {label}", file=sys.stderr)
        return 1
    print("planted-violations-controls: every planted fault reported, the real case caught")
    return 0


if __name__ == "__main__":
    sys.exit(main())
