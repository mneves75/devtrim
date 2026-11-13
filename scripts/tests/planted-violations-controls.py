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
import subprocess
import sys
import time
from pathlib import Path

HARNESS_PATH = Path(__file__).resolve().parent / "planted-violations.py"
WORKER_FAULT = "planted worker fault"


def load_harness():
    # The filename holds a hyphen, so it cannot be imported by name.
    spec = importlib.util.spec_from_file_location("planted_violations", HARNESS_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def timed_out_child_releases_its_worker(harness) -> bool:
    """A process that left the killed group still holds the output pipes.

    Waiting for those pipes to close would hang the worker for as long as that
    process lives, so the harness must only reap the child it killed.
    """
    holder = "import time; time.sleep(40)"
    command = [
        sys.executable,
        "-c",
        "import subprocess, sys, time; "
        f"subprocess.Popen([sys.executable, '-c', {holder!r}], start_new_session=True); "
        "time.sleep(120)",
    ]
    started = time.monotonic()
    try:
        harness.SUPERVISOR.run(command, HARNESS_PATH.parent, HARNESS_PATH.parent, 5)
    except subprocess.TimeoutExpired:
        return time.monotonic() - started < 20
    return False


def run_harness(harness, cases, jobs: int) -> tuple[int, str, str]:
    harness.CASES = tuple(cases)
    os.environ["DEVTRIM_PV_JOBS"] = str(jobs)
    os.environ.pop("DEVTRIM_PV_TEST_JOBS", None)
    sys.argv = [str(HARNESS_PATH)]
    stdout, stderr = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        try:
            status = harness.main()
        except SystemExit as stop:
            status = stop.code
    return status, stdout.getvalue(), stderr.getvalue()


def main() -> int:
    harness = load_harness()
    if not timed_out_child_releases_its_worker(harness):
        print("FAIL: a timed-out child kept its worker waiting on a pipe", file=sys.stderr)
        return 1
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

    def install_faults(module) -> None:
        # Bound to this module's own prove_case: each run has its own supervisor.
        prove_case = module.prove_case

        def faulty_prove_case(case, worker, *arguments):
            if case.name == "control/worker-fault":
                raise RuntimeError(WORKER_FAULT)
            if case.name == "control/restore-fault":
                # The restoration failed: the worker's copy keeps the mutant.
                path = worker.workspace / case.relative_path
                path.write_text(path.read_text().replace(case.before, case.after, 1))
                raise OSError("planted restoration failure")
            return prove_case(case, worker, *arguments)

        module.prove_case = faulty_prove_case

    install_faults(harness)
    status, output, errors = run_harness(
        harness,
        (
            variant("control/no-op", caught, after="    // planted no-op mutant\n" + caught.before),
            variant(
                "control/does-not-compile",
                caught,
                after='    compile_error!("planted compile failure");\n' + caught.before,
            ),
            variant("control/wrong-marker", other, marker="PV control/never-printed"),
            variant("control/worker-fault", other),
            caught,
        ),
        jobs=2,
    )
    expectations = [
        ("exit status 1", status == 1),
        ("MISSED for the no-op mutant", "MISSED control/no-op: boundary violation survived" in errors),
        ("compile failure", "ERROR control/does-not-compile: mutant did not compile" in errors),
        ("marker mismatch", "ERROR control/wrong-marker: " in errors and "not at the named assertion" in errors),
        ("worker exception", f"ERROR control/worker-fault: worker failed: {WORKER_FAULT}" in errors),
        ("positive control caught", f"CAUGHT {caught.name}: " in output),
        ("no planted fault caught", "CAUGHT control/" not in output),
    ]

    # A worker whose copy may still hold a mutant must take no further case:
    # a no-op in another file, judged by the leaked mutant's test, would pass.
    harness = load_harness()
    install_faults(harness)
    status, leak_output, leak_errors = run_harness(
        harness,
        (
            variant("control/restore-fault", caught),
            variant(
                "control/after-leak",
                other,
                after="    // planted no-op mutant\n" + other.before,
                tests=caught.tests,
                marker=caught.marker,
            ),
        ),
        jobs=1,
    )
    output, errors = output + leak_output, errors + leak_errors
    expectations += [
        ("exit status 1 after a failed restoration", status == 1),
        ("restoration failure reported", "ERROR control/restore-fault: worker failed" in leak_errors),
        ("no case proven on a contaminated copy", "CAUGHT control/after-leak" not in leak_output),
    ]

    # A worker that dies while publishing a result is a failure even when
    # every result was stored first.
    harness = load_harness()
    record = harness.MutationStage.record

    def failing_record(stage, index, outcome):
        record(stage, index, outcome)
        raise OSError("planted publication failure")

    harness.MutationStage.record = failing_record
    status, publish_output, publish_errors = run_harness(harness, (caught,), jobs=1)
    output, errors = output + publish_output, errors + publish_errors
    expectations += [
        ("exit status 1 after a publication failure", status == 1),
        ("publication failure reported", "planted publication failure" in publish_errors),
    ]

    missing = [label for label, held in expectations if not held]
    if missing:
        print(f"--- stdout\n{output}--- stderr\n{errors}", file=sys.stderr)
        for label in missing:
            print(f"FAIL: {label}", file=sys.stderr)
        return 1
    print("planted-violations-controls: every planted fault reported, the real case caught")
    return 0


if __name__ == "__main__":
    sys.exit(main())
