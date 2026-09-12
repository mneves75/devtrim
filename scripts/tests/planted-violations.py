#!/usr/bin/env python3
"""Prove that named safety assertions actually guard what they claim to guard.

A passing test says a boundary held. It does not say the test would notice if
the boundary were removed — devtrim has already shipped an assertion that could
not fail, because its fixture sat under a root retired earlier in the same work
and the scan returned before ever reaching the check under test. Nothing caught
that for several commits.

This gate breaks each guarded branch on a throwaway copy of the source and
requires the *named* assertion to fail. Two properties matter and both are
enforced: the mutant must compile, so a build error is never mistaken for
proof; and the failure must come from the tagged assertion, so an unrelated
refusal cannot stand in for the boundary.

Deliberately a fixed, named set rather than a mutation framework. `cargo-mutants`
reports a mutant as caught when *a* test fails, which is exactly the confusion
this exists to remove, and its cost is unbounded. Review still owns every
assertion these cases do not name.

Marker names must stay mutually non-prefixing: the check is a substring match,
so `PV evidence/agents` would also accept a failure tagged
`PV evidence/agents-history` and attribute it to the wrong guard.
"""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

TOOLCHAIN = "1.98.1"
TOTAL_DEADLINE_SECONDS = 600
TEST_TIMEOUT_SECONDS = 30

REPOSITORY = Path(__file__).resolve().parents[2]
SOURCE_INPUTS = ("src", "tests", "Cargo.toml", "Cargo.lock")


class Case:
    """One production branch, the edit that disables it, and the proof."""

    def __init__(
        self,
        name: str,
        relative_path: str,
        before: str,
        after: str,
        tests: tuple[str, ...],
        marker: str,
    ) -> None:
        self.name = name
        self.relative_path = relative_path
        self.before = before
        self.after = after
        self.tests = tests
        self.marker = marker


CASES = (
    Case(
        name="agents/history-symlink",
        relative_path="src/ops/agents.rs",
        # Deleting the symlink branch is NOT enough: the file-type check below
        # also rejects a link, so the test would stay green while the branch was
        # gone. The mutation must make a symlink positively eligible.
        before="    if file_type.is_symlink() {\n        return Ok(None);\n    }",
        after="    if file_type.is_symlink() {\n        return Ok(Some((400, 4)));\n    }",
        tests=("ops::agents::tests::a_symlinked_history_child_is_refused",),
        marker="PV agents/history-symlink",
    ),
    # The const assertion rejects empty and ASCII-whitespace evidence while
    # compiling, so the only thing the runtime loops can catch is a non-ASCII
    # blank. Each list gets its own case: they are three separate guards, and
    # deleting one loop must not be covered by another list's proof. The whole
    # entry is replaced, because splicing into a multi-line evidence string
    # leaves a dangling literal and the mutant fails to compile.
    Case(
        name='evidence/library-caches',
        relative_path='src/safety.rs',
        before='DeletionEntry {\n        label: "Playwright browser cache",\n        relative: "ms-playwright",\n        evidence: "Playwright\'s own docs describe this as the downloaded browser \\\n                   location, re-created by `npx playwright install`.",\n    },\n',
        after='DeletionEntry {\n        label: "Playwright browser cache",\n        relative: "ms-playwright",\n        evidence: "\\u{a0}",\n    },\n',
        tests=('safety::tests::every_managed_library_cache_carries_evidence',),
        marker="PV evidence/library-caches",
    ),
    Case(
        name='evidence/built-in-caches',
        relative_path='src/ops/caches.rs',
        before='DeletionEntry {\n        label: "huggingface model cache",\n        relative: ".cache/huggingface/hub",\n        evidence: "Model snapshots re-downloaded on next use. Scoped to `hub` \\\n                   so the sibling tokens and settings are never authority.",\n    },\n',
        after='DeletionEntry {\n        label: "huggingface model cache",\n        relative: ".cache/huggingface/hub",\n        evidence: "\\u{a0}",\n    },\n',
        tests=('ops::caches::tests::every_built_in_cache_carries_evidence',),
        marker="PV evidence/built-in-caches",
    ),
    Case(
        name='evidence/agents-regenerable',
        relative_path='src/ops/agents.rs',
        before='DeletionEntry {\n        label: "Claude Code metadata cache",\n        relative: ".claude/cache",\n        evidence: "Vendor-documented: the `.claude` directory reference lists \\\n                   `cache/changelog.md` as refreshed in the background. The \\\n                   observed siblings (`model-catalog/`, `my-closed-issues.json`) \\\n                   are re-fetched the same way.",\n    },\n',
        after='DeletionEntry {\n        label: "Claude Code metadata cache",\n        relative: ".claude/cache",\n        evidence: "\\u{a0}",\n    },\n',
        tests=('ops::agents::tests::every_agent_entry_carries_evidence',),
        marker="PV evidence/agents-regenerable",
    ),
    Case(
        name='evidence/agents-history',
        relative_path='src/ops/agents.rs',
        before='HistoryRoot {\n        label: "Claude Code shell snapshots",\n        relative: ".claude/shell-snapshots",\n        depth: 1,\n        evidence: "Vendor-documented: one snapshot per session, applied by the \\\n                   Bash tool to each command, and swept by Claude Code\'s own \\\n                   `cleanupPeriodDays` retention since v2.1.117 — so age is the \\\n                   vendor\'s own criterion here. Not rewritten if removed \\\n                   mid-session.",\n    },\n',
        after='HistoryRoot {\n        label: "Claude Code shell snapshots",\n        relative: ".claude/shell-snapshots",\n        depth: 1,\n        evidence: "\\u{a0}",\n    },\n',
        tests=('ops::agents::tests::every_agent_entry_carries_evidence',),
        marker="PV evidence/agents-history",
    ),
    Case(
        name="agents/apply-namespace",
        relative_path="src/ops/agents.rs",
        before="                authorize(target, ctx)?;\n",
        after="",
        tests=(
            "ops::agents::tests::the_retired_claude_trees_can_never_become_roots_again",
        ),
        marker="PV agents/apply-namespace",
    ),
)


def fail(message: str) -> None:
    print(message, file=sys.stderr)
    sys.exit(1)


def run(command: list[str], cwd: Path, env_target: Path, timeout: int):
    import os

    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = str(env_target)
    return subprocess.run(
        command,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def build_test_binary(workspace: Path, target_dir: Path, deadline: float) -> Path:
    """Compile the library tests and return the executable cargo produced."""
    remaining = int(max(1, deadline - time.monotonic()))
    result = run(
        [
            "rustup", "run", TOOLCHAIN, "cargo", "test",
            "--locked", "--offline", "--lib", "--all-features",
            "--no-run", "--message-format=json",
        ],
        workspace,
        target_dir,
        remaining,
    )
    if result.returncode != 0:
        return None  # caller decides whether a build failure is an error
    executable = None
    for line in result.stdout.splitlines():
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if message.get("reason") == "compiler-artifact" and message.get("executable"):
            if message.get("target", {}).get("kind") == ["lib"]:
                executable = message["executable"]
    return Path(executable) if executable else None


def run_one_test(binary: Path, test: str, target_dir: Path) -> subprocess.CompletedProcess:
    return run(
        [str(binary), "--exact", "--nocapture", test],
        REPOSITORY,
        target_dir,
        TEST_TIMEOUT_SECONDS,
    )


def selected_exactly_one(output: str) -> bool:
    match = re.search(r"running (\d+) test", output)
    return bool(match) and match.group(1) == "1"


def main() -> int:
    """Every exit path removes the scratch tree.

    `fail()` exits through `SystemExit` and a build can raise `TimeoutExpired`,
    so cleanup cannot live at the end of the happy path: each early exit would
    otherwise leave a full source copy and an `--all-features` debug build under
    `target/`. The likeliest failures — `--offline` without a fetched
    dependency, or the pinned toolchain missing — happen *after* the copy, in a
    gate developers run locally.
    """
    scratch = REPOSITORY / "target" / f"planted-violations-{int(time.time())}"
    try:
        return run_cases(scratch)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)


def run_cases(scratch: Path) -> int:
    deadline = time.monotonic() + TOTAL_DEADLINE_SECONDS
    workspace = scratch / "src-copy"
    target_dir = scratch / "cargo-target"
    workspace.mkdir(parents=True, exist_ok=True)

    for entry in SOURCE_INPUTS:
        source = REPOSITORY / entry
        destination = workspace / entry
        if source.is_dir():
            shutil.copytree(source, destination)
        else:
            shutil.copy2(source, destination)

    pristine = {
        case.relative_path: (workspace / case.relative_path).read_text()
        for case in CASES
    }

    baseline = build_test_binary(workspace, target_dir, deadline)
    if baseline is None:
        fail("ERROR: the unmutated copy did not compile; gate cannot run")

    # Every named test must pass before it can prove anything by failing.
    for case in CASES:
        for test in case.tests:
            result = run_one_test(baseline, test, target_dir)
            if not selected_exactly_one(result.stdout):
                fail(f"ERROR {case.name}: expected exactly one test, ran 0 ({test})")
            if result.returncode != 0:
                fail(f"ERROR {case.name}: baseline test already fails ({test})")

    caught = []
    for case in CASES:
        if time.monotonic() > deadline:
            fail(f"ERROR {case.name}: total deadline exceeded before mutation")

        path = workspace / case.relative_path
        original = pristine[case.relative_path]
        occurrences = original.count(case.before)
        if occurrences != 1:
            fail(
                f"ERROR {case.name}: expected exactly one replacement site, "
                f"found {occurrences} — the guarded code moved or changed shape"
            )
        path.write_text(original.replace(case.before, case.after, 1))

        mutant = build_test_binary(workspace, target_dir, deadline)
        path.write_text(original)
        if mutant is None:
            fail(f"ERROR {case.name}: mutant did not compile")

        for test in case.tests:
            try:
                result = run_one_test(mutant, test, target_dir)
            except subprocess.TimeoutExpired:
                fail(f"ERROR {case.name}: mutant test timed out ({test})")
            if result.returncode == 0:
                fail(f"MISSED {case.name}: boundary violation survived ({test})")
            combined = result.stdout + result.stderr
            if case.marker not in combined:
                fail(
                    f"ERROR {case.name}: {test} failed, but not at the named "
                    f"assertion '{case.marker}' — a different check refused, so "
                    f"this is not proof the boundary is covered"
                )
            location = re.search(r"(src/[^\s:]+:\d+)", combined)
            caught.append(
                f"CAUGHT {case.name}: {test.rsplit('::', 1)[-1]}"
                f" at {location.group(1) if location else 'unknown'}"
            )

    for line in caught:
        print(line)
    print(f"planted-violations: {len(caught)} boundary/ies proven covered")
    return 0


if __name__ == "__main__":
    sys.exit(main())
