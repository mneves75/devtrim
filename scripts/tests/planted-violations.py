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
        before="                authorize(target, ctx, release_context)?;\n",
        after="                let _ = release_context;\n",
        tests=(
            "ops::agents::tests::the_retired_claude_trees_can_never_become_roots_again",
        ),
        marker="PV agents/apply-namespace",
    ),
    Case(
        name="agents/codex-current-executable",
        relative_path="src/ops/agents.rs",
        before='        || !codex_executable(&path.join("bin/codex-code-mode-host"))?\n',
        after="",
        tests=(
            "ops::agents::tests::codex_releases_fail_closed_without_a_valid_current_link_or_install_lock",
        ),
        marker="PV agents/codex-current-executable",
    ),
    # Preview reads each owning repository's activity with git, and git will
    # run programs a hostile `.git/config` names. Each hardening flag closes one
    # path and is proven separately, against a fixture that arms exactly it.
    Case(
        name="git/signature-program",
        relative_path="src/ops/project.rs",
        before='            "-c",\n            "log.showSignature=false",\n            "--no-optional-locks",\n            "--no-lazy-fetch",\n            "--no-pager",\n            "log",\n            "--no-show-signature",\n',
        after='            "--no-optional-locks",\n            "--no-lazy-fetch",\n            "--no-pager",\n            "log",\n',
        tests=("ops::project::tests::activity_probe_never_runs_a_repository_configured_signature_program",),
        marker="PV git/signature-program",
    ),
    Case(
        name="git/lazy-fetch-transport",
        relative_path="src/ops/project.rs",
        before='            "--no-lazy-fetch",\n',
        after="",
        tests=("ops::project::tests::activity_probe_never_lazily_fetches_through_a_repository_configured_transport",),
        marker="PV git/lazy-fetch-transport",
    ),
    Case(
        name="git/reflog-activity",
        relative_path="src/ops/project.rs",
        before="    if reflog.is_empty() {\n",
        after="    if !reflog.is_empty() || reflog.is_empty() {\n",
        tests=("ops::project::tests::a_fresh_checkout_of_an_old_commit_is_active_not_stale",),
        marker="PV git/reflog-activity",
    ),
    Case(
        name="git/head-commit",
        relative_path="src/ops/project.rs",
        before="    Ok(commit.max(iso_date(entry, root)?))\n",
        after="    let _ = commit;\n    iso_date(entry, root)\n",
        tests=("ops::project::tests::a_recent_head_commit_counts_even_when_the_reflog_does_not_name_it",),
        marker="PV git/head-commit",
    ),
    Case(
        name="sink/rename-no-replace",
        relative_path="src/ops/mod.rs",
        before="    rustix::fs::renameat_with(dir, from, dir, to, rustix::fs::RenameFlags::NOREPLACE)\n",
        after="    dir.rename(from, dir, to)\n",
        tests=("ops::tests::quarantine_rename_never_replaces_an_occupied_name",),
        marker="PV sink/rename-no-replace",
    ),
    Case(
        name="liveness/lsof-escape",
        relative_path="src/safety.rs",
        before="decode_lsof_name(path)?",
        after="path.to_vec()",
        tests=("safety::tests::lsof_cwd_names_are_decoded_or_refused_never_taken_literally",),
        marker="PV liveness/lsof-escape",
    ),
    Case(
        name="liveness/lsof-unreported-running",
        relative_path="src/safety.rs",
        before="unreported.intersection(&running).next()",
        after="None::<&u32>",
        tests=("safety::tests::lsof_exit_one_passes_only_when_every_unreported_process_is_gone",),
        marker="PV liveness/lsof-unreported-running",
    ),
    Case(
        name="liveness/lsof-successor",
        relative_path="src/safety.rs",
        before="!later.complete || later.reported != successors",
        after="false",
        tests=("safety::tests::a_build_process_that_started_during_the_probe_is_looked_up_once",),
        marker="PV liveness/lsof-successor",
    ),
    Case(
        name="evidence/agents-codex-releases",
        relative_path="src/ops/agents.rs",
        before='const CODEX_RELEASES: DeletionEntry = DeletionEntry {\n    label: "Codex standalone release",\n    relative: ".codex/packages/standalone/releases",\n    evidence: "Owner source: OpenAI\'s standalone installer at commit \\\n               0a2eb4696c26ac33204bcd255721ab30220a4774 \\\n               (`scripts/install/install.sh`) writes each release to \\\n               `releases/<version>-<target>`, points `current` at one, and \\\n               serializes itself on `install.lock` with macOS `lockf(1)`, \\\n               which is BSD `flock(2)`. It removes only its own `.staging.*` \\\n               directories, never an older release. Observed 2026-09-23: six \\\n               old releases (1.68 GiB) beside `current`; moving them to Trash \\\n               left the current release, sessions, auth and configuration \\\n               working. The vendor does not promise removal is safe while an \\\n               older binary still runs, so a release any process is executing \\\n               is refused.",\n};\n',
        after='const CODEX_RELEASES: DeletionEntry = DeletionEntry {\n    label: "Codex standalone release",\n    relative: ".codex/packages/standalone/releases",\n    evidence: "\\u{a0}",\n};\n',
        tests=("ops::agents::tests::every_agent_entry_carries_evidence",),
        marker="PV evidence/agents-codex-releases",
    ),
    Case(
        name="agents/codex-running-release",
        relative_path="src/ops/agents.rs",
        before="        if executes(&mappings, target)? {\n",
        after="        if false {\n",
        tests=(
            "ops::agents::tests::a_release_a_process_still_executes_is_neither_offered_nor_removed",
        ),
        marker="PV agents/codex-running-release",
    ),
    Case(
        name="liveness/lsof-mapping-names",
        relative_path="src/safety.rs",
        before='    }\n    if awaiting_name {\n        bail!("lsof reported an executable mapping without a name");',
        after='    }\n    if false {\n        bail!("lsof reported an executable mapping without a name");',
        tests=("safety::tests::executable_mappings_refuse_any_mapping_they_cannot_name",),
        marker="PV liveness/lsof-mapping-names",
    ),
    Case(
        name="xcode/non-directory-target",
        relative_path="src/ops/xcode.rs",
        before="if !metadata.file_type().is_dir() {",
        after="if false {",
        tests=("ops::xcode::tests::apply_refuses_a_non_directory_xcode_support_child",),
        marker="PV xcode/non-directory-target",
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
        # The caller decides whether a build failure is an error; either way the
        # reason must be visible, or an unproven case cannot be diagnosed.
        print("\n".join(result.stderr.splitlines()[-20:]), file=sys.stderr)
        return None
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
