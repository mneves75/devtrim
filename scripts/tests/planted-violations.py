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
        target: str = "lib",
    ) -> None:
        self.name = name
        self.relative_path = relative_path
        self.before = before
        self.after = after
        self.tests = tests
        self.marker = marker
        # "lib" selects a unit test; any other value names an integration test
        # file in tests/, whose binary runs the freshly rebuilt devtrim.
        self.target = target


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
        before="                authorize(target, ctx, release_context, mappings)?;\n",
        after="                let _ = (release_context, mappings);\n",
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
        # Either setting alone suppresses the display, so both go together.
        before='        .args([\n            "-c",\n            "log.showSignature=false",\n            "log",\n            "--no-show-signature",\n            "-1",\n        ])\n',
        after='        .args(["log", "-1"])\n',
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
    # uv's own empty `.git` is the one nested Git marker the sink tolerates.
    # Each condition of that exception is broken on its own, and the matching
    # shape must then be refused by name; a mutant that tolerated a real
    # worktree pointer, a case variant, an untagged root, or a marker at the
    # wrong depth or in the wrong bucket would otherwise pass unnoticed.
    Case(
        name="sink/uv-marker-empty",
        relative_path="src/ops/mod.rs",
        before="            && metadata.st_size == 0,\n",
        after="            && metadata.st_size >= 0,\n",
        tests=("ops::tests::only_the_exact_uv_marker_shape_is_tolerated",),
        marker="PV sink/uv-marker-nonempty",
    ),
    Case(
        name="sink/uv-marker-spelling",
        relative_path="src/ops/mod.rs",
        before='fn is_uv_source_distribution_marker(dir: &cap_std::fs::Dir, name: &OsStr) -> Result<bool> {\n    if name.as_bytes() != b".git" {\n',
        after='fn is_uv_source_distribution_marker(dir: &cap_std::fs::Dir, name: &OsStr) -> Result<bool> {\n    if !is_git_metadata_name(name) {\n',
        tests=("ops::tests::only_the_exact_uv_marker_shape_is_tolerated",),
        marker="PV sink/uv-marker-case",
    ),
    # Both deletion paths build their marker rules through `MarkerRules::new`,
    # so one reading of the tag serves them both, and one case breaks it.
    Case(
        name="sink/uv-marker-tag",
        relative_path="src/ops/mod.rs",
        before="            uv_exception: markers.uv() && has_cachedir_tag(dir),\n",
        after="            uv_exception: markers.uv(),\n",
        tests=("ops::tests::only_the_exact_uv_marker_shape_is_tolerated",),
        marker="PV sink/uv-marker-untagged: Trash preflight",
    ),
    # A directory or symlink already fails the size test on APFS, so only a
    # zero-length FIFO shows the file-type test doing work of its own.
    Case(
        name="sink/uv-marker-regular-file",
        relative_path="src/ops/mod.rs",
        before="        rustix::fs::FileType::from_raw_mode(metadata.st_mode) == rustix::fs::FileType::RegularFile\n",
        after="        rustix::fs::FileType::from_raw_mode(metadata.st_mode) != rustix::fs::FileType::Directory\n",
        tests=("ops::tests::only_the_exact_uv_marker_shape_is_tolerated",),
        marker="PV sink/uv-marker-special-file",
    ),
    Case(
        name="sink/uv-marker-depth",
        relative_path="src/ops/mod.rs",
        before="    if rules.uv_exception && depth == 1 && uv_bucket {\n",
        after="    if rules.uv_exception && depth >= 1 && uv_bucket {\n",
        tests=("ops::tests::only_the_exact_uv_marker_shape_is_tolerated",),
        marker="PV sink/uv-marker-depth",
    ),
    Case(
        name="sink/uv-marker-bucket",
        relative_path="src/ops/mod.rs",
        before="    if rules.uv_exception && depth == 1 && uv_bucket {\n",
        after="    if rules.uv_exception && depth == 1 && (uv_bucket || !uv_bucket) {\n",
        tests=("ops::tests::only_the_exact_uv_marker_shape_is_tolerated",),
        marker="PV sink/uv-marker-other-bucket",
    ),
    Case(
        name="sink/uv-marker-grant",
        relative_path="src/ops/mod.rs",
        before="            uv_exception: markers.uv() && has_cachedir_tag(dir),\n",
        after="            uv_exception: has_cachedir_tag(dir),\n",
        tests=("ops::tests::a_uv_cache_without_uv_s_lock_keeps_its_marker_refused",),
        marker="PV sink/uv-marker-grant",
    ),
    Case(
        name="sink/uv-grant-root",
        relative_path="src/safety.rs",
        before="        if lock.root != self.path {\n",
        after="        if false {\n",
        tests=("ops::tests::a_uv_lock_grants_only_its_own_root",),
        marker="PV sink/uv-grant-root",
    ),
    Case(
        name="caches/uv-lock",
        relative_path="src/safety.rs",
        before="    match rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive) {\n",
        after="    match Ok::<(), rustix::io::Errno>(()) {\n",
        tests=("ops::caches::tests::uv_cache_is_refused_while_a_uv_process_holds_its_lock",),
        marker="PV caches/uv-lock",
    ),
    Case(
        name="caches/uv-lock-ancestor",
        relative_path="src/safety.rs",
        before="    if resolved != root {\n",
        after="    if false {\n",
        tests=("ops::caches::tests::uv_lock_is_never_created_through_a_symlinked_ancestor",),
        marker="PV caches/ancestor-lock",
    ),
    # `purge` routes by leaf name and relies on the receiving category to
    # refuse a shape it does not own; the repository in the test is stale, so
    # without this check the misrouted source directory would be deleted.
    Case(
        name="purge/misroute-corroboration",
        relative_path="src/ops/artifacts.rs",
        before="                if artifact_evidence(path)?.is_none() {\n",
        after="                if false {\n",
        tests=("ops::purge::tests::a_misrouted_finding_is_refused_by_the_category_that_receives_it",),
        marker="PV purge/misroute-refused",
    ),
    Case(
        name="trash/continue-past-refusal",
        relative_path="src/ops/mod.rs",
        before="            outcome.fail(error);\n            continue;\n        }\n        outcome.record(finding, format!(\"permanently deleted {}\", finding.label));\n",
        after="            outcome.fail(error);\n            break;\n        }\n        outcome.record(finding, format!(\"permanently deleted {}\", finding.label));\n",
        tests=("ops::tests::a_refused_trash_item_does_not_block_the_rest_of_the_purge",),
        marker="PV trash/continue-past-refusal",
    ),
    Case(
        name="xcode/continue-past-refusal",
        relative_path="src/ops/xcode.rs",
        before="                Err(error) => outcome.fail(error),\n",
        after="                Err(error) => {\n                    outcome.fail(error);\n                    break;\n                }\n",
        tests=("ops::xcode::tests::a_refused_derived_data_folder_does_not_block_the_rest_of_the_plan",),
        marker="PV xcode/continue-past-refusal",
    ),
    Case(
        name="tui/selection-plan",
        relative_path="src/tui.rs",
        before="            .filter(|(index, _)| !self.excluded.contains(index))\n",
        after="            .filter(|(index, _)| !self.excluded.contains(index) || true)\n",
        tests=("tui::tests::deselected_findings_never_reach_the_approved_plan",),
        marker="PV tui/selection-plan",
    ),
    Case(
        name="git/utc-dates",
        relative_path="src/ops/project.rs",
        before='        .env("TZ", "UTC0")\n',
        after="",
        tests=("ops::project::tests::activity_dates_are_read_in_utc_like_the_cutoff",),
        marker="PV git/utc-dates",
    ),
    # Mole V1.56.0 refuses a purge target holding files Git tracks or a
    # `*-keypair.json` (`lib/clean/project.sh`). Scan and apply each check on
    # their own, so each check is proven on its own.
    Case(
        name="project/tracked-files",
        relative_path="src/ops/project.rs",
        before="            .is_some_and(|identities| identities.contains(&(wanted.dev(), wanted.ino()))))\n",
        after="            .is_some_and(|_| false))\n",
        tests=(
            "ops::artifacts::tests::a_tree_its_repository_tracks_is_never_offered_or_removed",
            "ops::node_modules::tests::a_committed_node_modules_is_never_offered_or_removed",
        ),
        marker="PV project/tracked-files",
    ),
    # Trackedness is decided by the volume's identity of each tracked path's
    # ancestor, not by its spelling: comparing spellings instead must lose a
    # case-only rename, a Unicode case rename, and an ASCII alias alike.
    Case(
        name="project/tracked-case-spelling",
        relative_path="src/ops/project.rs",
        before="            .is_some_and(|identities| identities.contains(&(wanted.dev(), wanted.ino()))))\n",
        after="            .is_some_and(|_| self.listed.split(|byte| *byte == 0).any(|entry| std::path::Path::new(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(entry)).starts_with(relative))))\n",
        tests=("ops::artifacts::tests::a_tracked_tree_renamed_only_in_case_is_never_offered",),
        marker="PV project/tracked-case-rename",
    ),
    Case(
        name="project/tracked-unicode-spelling",
        relative_path="src/ops/project.rs",
        before="            .is_some_and(|identities| identities.contains(&(wanted.dev(), wanted.ino()))))\n",
        after="            .is_some_and(|_| self.listed.split(|byte| *byte == 0).any(|entry| std::path::Path::new(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(entry)).starts_with(relative))))\n",
        tests=("ops::artifacts::tests::a_tracked_tree_renamed_only_in_unicode_case_is_never_offered",),
        marker="PV project/tracked-unicode-case",
    ),
    Case(
        name="project/tracked-alias-spelling",
        relative_path="src/ops/project.rs",
        before="            .is_some_and(|identities| identities.contains(&(wanted.dev(), wanted.ino()))))\n",
        after="            .is_some_and(|_| self.listed.split(|byte| *byte == 0).any(|entry| std::path::Path::new(<std::ffi::OsStr as std::os::unix::ffi::OsStrExt>::from_bytes(entry)).starts_with(relative))))\n",
        tests=("ops::artifacts::tests::a_tracked_tree_renamed_to_an_ascii_alias_is_never_offered",),
        marker="PV project/tracked-ascii-alias",
    ),
    Case(
        name="artifacts/tracked-apply",
        relative_path="src/ops/artifacts.rs",
        before="                if tracks_files_under(&owner, path)? {\n",
        after="                if false && tracks_files_under(&owner, path)? {\n",
        tests=("ops::artifacts::tests::a_tree_its_repository_tracks_is_never_offered_or_removed",),
        marker="PV artifacts/tracked-apply",
    ),
    Case(
        name="node_modules/tracked-apply",
        relative_path="src/ops/node_modules.rs",
        before="                if tracks_files_under(&owner, path)? {\n",
        after="                if false && tracks_files_under(&owner, path)? {\n",
        tests=("ops::node_modules::tests::a_committed_node_modules_is_never_offered_or_removed",),
        marker="PV node_modules/tracked-apply",
    ),
    Case(
        name="artifacts/keypair-scan",
        relative_path="src/ops/artifacts.rs",
        before="                            keypairs_here += 1;\n                            continue;\n",
        after="                            keypairs_here += 1;\n",
        tests=("ops::artifacts::tests::a_tree_holding_a_program_keypair_is_never_offered_or_removed",),
        marker="PV artifacts/keypair-scan",
    ),
    Case(
        name="artifacts/keypair-apply",
        relative_path="src/ops/artifacts.rs",
        before="                    Some(Authored::ProgramKeypair(keypair)) => anyhow::bail!(\n",
        after="                    Some(Authored::ProgramKeypair(keypair)) if false => anyhow::bail!(\n",
        tests=("ops::artifacts::tests::a_tree_holding_a_program_keypair_is_never_offered_or_removed",),
        marker="PV artifacts/keypair-apply",
    ),
    # The keypair name matches as APFS folds it: a key under a spelling with
    # the Kelvin sign or the long s still opens by its usual name.
    Case(
        name="artifacts/keypair-alias-kelvin",
        relative_path="src/ops/artifacts.rs",
        before="                '\\u{212a}' => 'k',\n",
        after="",
        tests=("ops::artifacts::tests::program_keypair_names_match_the_solana_suffix_as_the_volume_does",),
        marker="PV artifacts/keypair-alias",
    ),
    Case(
        name="artifacts/keypair-alias-long-s",
        relative_path="src/ops/artifacts.rs",
        before="                '\\u{17f}' => 's',\n",
        after="",
        tests=("ops::artifacts::tests::program_keypair_names_match_the_solana_suffix_as_the_volume_does",),
        marker="PV artifacts/keypair-alias",
    ),
    # Mole also refuses a purge target holding a `.git` anywhere inside; the
    # scan skips such a tree and the apply preflight refuses it.
    Case(
        name="artifacts/nested-repository-scan",
        relative_path="src/ops/artifacts.rs",
        before="                            repositories_here += 1;\n                            continue;\n",
        after="                            repositories_here += 1;\n",
        tests=("ops::artifacts::tests::a_tree_holding_a_git_repository_is_never_offered_or_removed",),
        marker="PV artifacts/nested-repository-scan",
    ),
    Case(
        name="artifacts/nested-repository-apply",
        relative_path="src/ops/artifacts.rs",
        before="                    Some(Authored::Repository(marker)) => anyhow::bail!(\n",
        after="                    Some(Authored::Repository(marker)) if false => anyhow::bail!(\n",
        tests=("ops::artifacts::tests::a_tree_holding_a_git_repository_is_never_offered_or_removed",),
        marker="PV artifacts/nested-repository-apply",
    ),
    # A finding the sink refuses after preflight costs only itself.
    Case(
        name="artifacts/continue-past-refusal",
        relative_path="src/ops/artifacts.rs",
        before="                Err(error) => outcome.fail(error),\n",
        after="                Err(error) => {\n                    outcome.fail(error);\n                    break;\n                }\n",
        tests=("ops::artifacts::tests::a_refused_artifact_does_not_block_the_rest_of_the_plan",),
        marker="PV artifacts/continue-past-refusal",
    ),
    Case(
        name="node_modules/continue-past-refusal",
        relative_path="src/ops/node_modules.rs",
        before="                Err(error) => outcome.fail(error),\n",
        after="                Err(error) => {\n                    outcome.fail(error);\n                    break;\n                }\n",
        tests=("ops::node_modules::tests::a_refused_node_modules_does_not_block_the_rest_of_the_plan",),
        marker="PV node_modules/continue-past-refusal",
    ),
    # A node_modules inside build output (`.next/standalone`, a repository
    # left in a Cargo `target`) belongs to that output: the walk never enters
    # it, the scan refuses one its root lies in, and apply rechecks.
    Case(
        name="node_modules/build-output-walk",
        relative_path="src/ops/node_modules.rs",
        before="            Ok(true) => entries.skip_current_dir(),\n",
        after="            Ok(true) => {}\n",
        tests=("ops::node_modules::tests::a_node_modules_inside_build_output_belongs_to_that_output",),
        marker="PV node_modules/build-output-walk",
    ),
    Case(
        name="node_modules/build-output-scan",
        relative_path="src/ops/node_modules.rs",
        before="                    if build_output_between(&owner, &path)?.is_some() {\n",
        after="                    if false && build_output_between(&owner, &path)?.is_some() {\n",
        tests=("ops::node_modules::tests::a_node_modules_inside_build_output_belongs_to_that_output",),
        marker="PV node_modules/build-output-scan",
    ),
    Case(
        name="node_modules/build-output-apply",
        relative_path="src/ops/node_modules.rs",
        before="                if let Some(output) = build_output_between(&owner, path)? {\n",
        after="                if let Some(output) = build_output_between(&owner, path)?.filter(|_| false) {\n",
        tests=("ops::node_modules::tests::a_node_modules_inside_build_output_belongs_to_that_output",),
        marker="PV node_modules/build-output-apply",
    ),
    # A node_modules is an install only beside the package.json it was
    # installed from: an app bundle's `assets/node_modules` and a packaged
    # tool's `dist/node_modules` have none, and no install recreates them.
    Case(
        name="node_modules/manifest-scan",
        relative_path="src/ops/node_modules.rs",
        before="                    if !has_manifest(&path)? {\n",
        after="                    if false && !has_manifest(&path)? {\n",
        tests=("ops::node_modules::tests::a_node_modules_without_a_manifest_beside_it_is_not_an_install",),
        marker="PV node_modules/manifest-scan",
    ),
    Case(
        name="node_modules/manifest-apply",
        relative_path="src/ops/node_modules.rs",
        before="                if !has_manifest(path)? {\n",
        after="                if false && !has_manifest(path)? {\n",
        tests=("ops::node_modules::tests::a_node_modules_without_a_manifest_beside_it_is_not_an_install",),
        marker="PV node_modules/manifest-apply",
    ),
    # A repository whose checks fail offers nothing, not even a finding judged
    # before the failure; only the repository's error is reported.
    Case(
        name="node_modules/repository-contained",
        relative_path="src/ops/node_modules.rs",
        before="                Err(error) => findings.push(unjudged_finding(\"node_modules\", &owner, &error)),\n",
        after="                Err(error) => {\n                    findings.append(&mut judged);\n                    findings.push(unjudged_finding(\"node_modules\", &owner, &error));\n                }\n",
        tests=("ops::node_modules::tests::a_failing_repository_blocks_only_its_own_findings",),
        marker="PV node_modules/repository-contained",
    ),
    Case(
        name="artifacts/repository-contained",
        relative_path="src/ops/artifacts.rs",
        before="                Err(error) => findings.push(unjudged_finding(\"artifacts\", &owner, &error)),\n",
        after="                Err(error) => {\n                    findings.append(&mut judged);\n                    findings.push(unjudged_finding(\"artifacts\", &owner, &error));\n                }\n",
        tests=("ops::artifacts::tests::an_unreadable_tree_refuses_its_repository_rather_than_offers",),
        marker="PV artifacts/repository-contained",
    ),
    # A folder the walk cannot read is reported, never silently dropped from
    # the plan (CODING_STANDARDS.md S8).
    Case(
        name="node_modules/unread-folders-reported",
        relative_path="src/ops/node_modules.rs",
        before="            findings.push(unread_folders_finding(\"node_modules\", &unread));\n",
        after="            let _ = &unread;\n",
        tests=("ops::node_modules::tests::an_unreadable_folder_is_reported_and_blocks_only_itself",),
        marker="PV node_modules/unread-folders-reported",
    ),
    Case(
        name="artifacts/unread-folders-reported",
        relative_path="src/ops/artifacts.rs",
        before="            findings.push(unread_folders_finding(\"artifacts\", &unread));\n",
        after="            let _ = &unread;\n",
        tests=("ops::artifacts::tests::an_unreadable_folder_is_reported_and_blocks_only_itself",),
        marker="PV artifacts/unread-folders-reported",
    ),
    # Only Git's own unborn state is skipped: HEAD symbolic and resolving to
    # nothing. A HEAD naming a missing commit must keep its error.
    Case(
        name="project/unborn-branch",
        relative_path="src/ops/project.rs",
        before="    git_output(&[\"symbolic-ref\", \"-q\", \"HEAD\"]).is_some_and(|symbolic| symbolic.status.success())\n",
        after="    git_output(&[\"symbolic-ref\", \"-q\", \"HEAD\"]).is_some_and(|symbolic| !symbolic.status.success())\n",
        tests=("ops::project::tests::an_unborn_branch_is_recognized_and_a_dangling_head_is_not",),
        marker="PV project/unborn-branch",
    ),
    Case(
        name="project/unborn-dangling",
        relative_path="src/ops/project.rs",
        before="            .is_some_and(|references| references.status.success())\n",
        after="            .is_some_and(|_| true)\n",
        tests=("ops::project::tests::an_unborn_branch_is_recognized_and_a_dangling_head_is_not",),
        marker="PV project/unborn-dangling",
    ),
    Case(
        name="project/unborn-broken-refs",
        relative_path="src/ops/project.rs",
        before="        && git_output(&[\"refs\", \"verify\"]).is_some_and(|verified| verified.status.success())\n",
        after="        && git_output(&[\"refs\", \"verify\"]).is_some()\n",
        tests=("ops::project::tests::an_unborn_branch_is_recognized_and_a_dangling_head_is_not",),
        marker="PV project/unborn-broken-refs",
    ),
    Case(
        name="node_modules/owner-veto",
        relative_path="src/ops/node_modules.rs",
        before="            groups.remove(&owner);\n",
        after="            let _ = &owner;\n",
        tests=("ops::node_modules::tests::a_failure_after_the_owner_is_known_blocks_the_whole_repository",),
        marker="PV node_modules/owner-veto",
    ),
    Case(
        name="project/unborn-no-commit",
        relative_path="src/ops/project.rs",
        before="            listed.status.success() && listed.stderr.is_empty() && holds_no_commit(&listed.stdout)\n",
        after="            listed.status.success() && listed.stderr.is_empty()\n",
        tests=("ops::project::tests::an_unborn_branch_is_recognized_and_a_dangling_head_is_not",),
        marker="PV project/unborn-no-commit",
    ),
    Case(
        name="project/unborn-complete-listing",
        relative_path="src/ops/project.rs",
        before="            listed.status.success() && listed.stderr.is_empty() && holds_no_commit(&listed.stdout)\n",
        after="            listed.status.success() && holds_no_commit(&listed.stdout)\n",
        tests=("ops::project::tests::an_unborn_branch_is_recognized_and_a_dangling_head_is_not",),
        marker="PV project/unborn-complete-listing",
    ),
    # 0.10.6: the Homebrew split, the three newer names, the Trash identity
    # selection and per-folder DerivedData liveness.
    Case(
        name="caches/brew-clone-scan",
        relative_path="src/ops/caches.rs",
        before="        if file_type.is_dir() && has_git_marker(&entry.path())? {\n",
        after="        if false && file_type.is_dir() && has_git_marker(&entry.path())? {\n",
        tests=("ops::caches::tests::a_homebrew_cache_holding_a_git_clone_is_offered_around_it",),
        marker="PV caches/brew-clone-scan",
    ),
    Case(
        name="caches/brew-entry-apply",
        relative_path="src/ops/caches.rs",
        before="    if metadata.file_type().is_symlink() {\n        return Ok(false);\n    }\n    Ok(!(metadata.is_dir()",
        after="    if false {\n        return Ok(false);\n    }\n    Ok(!(metadata.is_dir()",
        tests=("ops::caches::tests::a_homebrew_cache_holding_a_git_clone_is_offered_around_it",),
        marker="PV caches/brew-entry-apply",
    ),
    Case(
        name="artifacts/newer-names",
        relative_path="src/ops/artifacts.rs",
        before="        Some(\".cxx\") => sibling_evidence(path, &[\"build.gradle\", \"build.gradle.kts\"]),\n",
        after="        Some(\".cxx\") => Ok(Some(\"directory name .cxx\".into())),\n",
        tests=("ops::artifacts::tests::newer_names_are_offered_only_beside_their_owners_file",),
        marker="PV artifacts/newer-names",
    ),
    Case(
        name="trash/only-devtrim-identity",
        relative_path="src/ops/mod.rs",
        before="        let ours = TrashedIdentity::of(path).is_some_and(|identity| moved.contains(&identity));\n",
        after="        let ours = TrashedIdentity::of(path).is_some() || moved.is_empty();\n",
        tests=("ops::tests::only_items_devtrim_moved_to_the_trash_are_offered",),
        marker="PV trash/only-devtrim-identity",
    ),
    Case(
        name="trash/journal-identity",
        relative_path="src/ops/mod.rs",
        before="        .with_trashed_identity(trashed),\n",
        after="        .with_trashed_identity(None.or(trashed.filter(|_| false))),\n",
        tests=("ops::tests::the_sink_journals_the_identity_of_what_it_moves_to_the_trash",),
        marker="PV trash/journal-identity",
    ),
    Case(
        name="xcode/active-open",
        relative_path="src/ops/xcode.rs",
        before="        return Ok(Some(InUse::Open));\n",
        after="        return Ok(None);\n",
        tests=("ops::xcode::tests::while_xcode_runs_only_old_closed_derived_data_is_offered",),
        marker="PV xcode/active-folders",
    ),
    Case(
        name="xcode/active-recent",
        relative_path="src/ops/xcode.rs",
        before="    Ok((newest > cutoff).then_some(InUse::Recent))\n",
        after="    Ok((newest > cutoff).then_some(InUse::Recent).filter(|_| false))\n",
        tests=("ops::xcode::tests::while_xcode_runs_only_old_closed_derived_data_is_offered",),
        marker="PV xcode/active-folders",
    ),
    Case(
        name="xcode/active-apply",
        relative_path="src/ops/xcode.rs",
        before="                            match derived_data_in_use(folder, activity, ctx.active_days)? {\n",
        after="                            match derived_data_in_use(folder, activity, ctx.active_days)?.filter(|_| false) {\n",
        tests=("ops::xcode::tests::while_xcode_runs_only_old_closed_derived_data_is_offered",),
        marker="PV xcode/active-apply",
    ),
    Case(
        name="xcode/active-empty-folder",
        relative_path="src/ops/xcode.rs",
        before="    if newest == std::time::UNIX_EPOCH {\n",
        after="    if false && newest == std::time::UNIX_EPOCH {\n",
        tests=("ops::xcode::tests::scan_skips_derived_data_when_an_xcode_build_is_running_or_unknown",),
        marker="PV xcode/active-empty-folder",
    ),
    # 0.10.6 review round: incomplete history, typed lsof names, the brew
    # entry's two halves and the canonical open-file match.
    Case(
        name="trash/incomplete-history",
        relative_path="src/ops/mod.rs",
        before="    if !history.errors.is_empty() {\n",
        after="    if false && !history.errors.is_empty() {\n",
        tests=("ops::tests::only_items_devtrim_moved_to_the_trash_are_offered",),
        marker="PV trash/incomplete-history",
    ),
    Case(
        name="liveness/xcode-open-files",
        relative_path="src/safety.rs",
        before="                if !name.starts_with(b\"/\") {\n                    bail!(\n                        \"lsof could not name an open {} file (`{}`), so whether it lies under DerivedData is unknown\",\n",
        after="                if false && !name.starts_with(b\"/\") {\n                    bail!(\n                        \"lsof could not name an open {} file (`{}`), so whether it lies under DerivedData is unknown\",\n",
        tests=("safety::tests::xcode_open_files_are_absolute_names_from_a_complete_listing",),
        marker="PV liveness/xcode-open-files",
    ),
    Case(
        name="caches/brew-entry-repository",
        relative_path="src/ops/caches.rs",
        before="    Ok(!(metadata.is_dir() && has_git_marker(target)?))\n",
        after="    Ok(true)\n",
        tests=("ops::caches::tests::a_homebrew_cache_holding_a_git_clone_is_offered_around_it",),
        marker="PV caches/brew-entry-repository",
    ),
    Case(
        name="caches/brew-entry-direct-child",
        relative_path="src/ops/caches.rs",
        before="    if !is_standard_brew_cache(parent, home) {\n",
        after="    if !is_eligible_owner_cache(\"brew\", parent, home) {\n",
        tests=("ops::caches::tests::a_homebrew_cache_holding_a_git_clone_is_offered_around_it",),
        marker="PV caches/brew-entry-direct-child",
    ),
    Case(
        name="xcode/active-canonical",
        relative_path="src/ops/xcode.rs",
        before="        .any(|file| file.path.starts_with(folder) || file.path.starts_with(&canonical))\n",
        after="        .any(|file| file.path.starts_with(folder) || canonical.as_os_str().is_empty())\n",
        tests=("ops::xcode::tests::an_open_file_is_matched_under_the_folders_real_path",),
        marker="PV xcode/active-canonical",
    ),
    # 0.10.6 final review: Terraform state, lsof's mount fallback, and the
    # open-file match by identity.
    Case(
        name="artifacts/terraform-state-scan",
        relative_path="src/ops/artifacts.rs",
        before="        if is_terraform_state_name(entry.file_name()) {\n",
        after="        if false && is_terraform_state_name(entry.file_name()) {\n",
        tests=("ops::artifacts::tests::a_tree_holding_terraform_state_is_never_offered_or_removed",),
        marker="PV artifacts/terraform-state-scan",
    ),
    Case(
        name="artifacts/terraform-state-apply",
        relative_path="src/ops/artifacts.rs",
        before="                    Some(Authored::TerraformState(state)) => anyhow::bail!(\n                        \"refusing {}: it holds the Terraform state {}\",\n                        path.display(),\n                        state.display()\n                    ),\n",
        after="                    Some(Authored::TerraformState(_)) => {}\n",
        tests=("ops::artifacts::tests::a_tree_holding_terraform_state_is_never_offered_or_removed",),
        marker="PV artifacts/terraform-state-apply",
    ),
    Case(
        name="xcode/active-identity",
        relative_path="src/ops/xcode.rs",
        before="        || held_open_by_identity(folder, &same_volume)?\n",
        after="        || (held_open_by_identity(folder, &same_volume)? && false)\n",
        tests=("ops::xcode::tests::an_open_file_is_matched_by_the_folders_identity",),
        marker="PV xcode/active-identity",
    ),
    Case(
        name="xcode/active-identity-every-component",
        relative_path="src/ops/xcode.rs",
        before="                Ok(_) => {}\n                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}\n",
        after="                Ok(_) => break,\n                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}\n",
        tests=("ops::xcode::tests::the_identity_match_looks_past_an_earlier_derived_data_component",),
        marker="PV xcode/active-identity-every-component",
    ),
    Case(
        name="xcode/active-mount-fallback",
        relative_path="src/ops/xcode.rs",
        before="                .ends_with(fallback.as_bytes())\n",
        after="                .ends_with(b\"\\0never\")\n",
        tests=("ops::xcode::tests::a_mount_fallback_name_on_the_folders_volume_refuses_the_folder",),
        marker="PV xcode/active-mount-fallback",
    ),
    Case(
        name="liveness/xcode-open-file-device",
        relative_path="src/safety.rs",
        before="                    .and_then(|dev| u64::from_str_radix(dev, 16).ok())\n",
        after="                    .and_then(|dev| u64::from_str_radix(dev, 16).ok())\n                    .or(Some(0))\n",
        tests=("safety::tests::xcode_open_files_are_absolute_names_from_a_complete_listing",),
        marker="PV liveness/xcode-open-files",
    ),
    # A default project folder linked to the home folder or above it would make
    # the whole home or disk a scan root nobody named.
    Case(
        name="roots/default-home-link",
        relative_path="src/safety.rs",
        before="                if holds_home(&root, home) {\n",
        after="                if false && holds_home(&root, home) {\n",
        tests=("safety::tests::default_roots_are_the_conventional_project_folders_that_exist",),
        marker="PV roots/default-home-link",
    ),
    Case(
        name="roots/default-home-identity",
        relative_path="src/safety.rs",
        before="            .is_ok_and(|candidate| (candidate.dev(), candidate.ino()) == identity)\n",
        after="            .is_ok_and(|candidate| false && (candidate.dev(), candidate.ino()) == identity)\n",
        tests=("safety::tests::the_home_folder_is_recognized_under_any_spelling",),
        marker="PV roots/default-home-identity",
    ),
    # Irreplaceable files follow `retain_days`, never the project window, so
    # lowering `active_days` to free build output cannot expose them.
    Case(
        name="agents/retention-window",
        relative_path="src/ops/agents.rs",
        before="history_details(&path, &ctx.home, ctx.retain_days)",
        after="history_details(&path, &ctx.home, ctx.active_days)",
        tests=("ops::agents::tests::history_follows_the_retention_window_not_the_project_window",),
        marker="PV agents/retention-window",
    ),
    Case(
        name="installers/retention-window",
        relative_path="src/ops/installers.rs",
        before="installer_details(&path, &ctx.home, ctx.retain_days)",
        after="installer_details(&path, &ctx.home, ctx.active_days)",
        tests=("ops::installers::tests::installers_follow_the_retention_window_not_the_project_window",),
        marker="PV installers/retention-window",
    ),
    Case(
        name="sink/trash-grant-root",
        relative_path="src/safety.rs",
        before="        if self.path.parent() != Some(trash) {\n",
        after="        if false && self.path.parent() != Some(trash) {\n",
        tests=("ops::tests::the_trash_grant_covers_only_items_directly_in_the_trash",),
        marker="PV sink/trash-grant-root",
    ),
    # DerivedData holding SwiftPM package checkouts is cleaned around them:
    # they are Git clones, never offered by the scan nor removed by apply.
    Case(
        name="xcode/package-checkouts-offered",
        relative_path="src/ops/xcode.rs",
        before="        if file_type.is_dir() && !is_source_packages(&entry.file_name()) {\n",
        after="        if file_type.is_dir() {\n",
        tests=("ops::xcode::tests::a_derived_data_folder_holding_package_checkouts_is_cleaned_around_them",),
        marker="PV xcode/package-checkouts-offered",
    ),
    Case(
        name="xcode/package-checkouts-kept",
        relative_path="src/ops/xcode.rs",
        before="    if in_folder && path.file_name().is_some_and(is_source_packages) {\n",
        after="    if false && in_folder && path.file_name().is_some_and(is_source_packages) {\n",
        tests=("ops::xcode::tests::apply_never_removes_the_package_checkouts",),
        marker="PV xcode/package-checkouts-kept",
    ),
    # A DerivedData folder that is itself a repository is never split: its
    # directories are that repository's worktree.
    Case(
        name="xcode/repository-folder-scan",
        relative_path="src/ops/xcode.rs",
        before="    Ok(holds_package_checkouts(folder)? && !has_git_marker(folder)?)\n",
        after="    Ok(holds_package_checkouts(folder)?)\n",
        tests=("ops::xcode::tests::a_derived_data_folder_that_is_a_repository_is_never_split",),
        marker="PV xcode/repository-folder-scan",
    ),
    Case(
        name="xcode/repository-folder-apply",
        relative_path="src/ops/xcode.rs",
        before="    Ok(folder_is_directory && is_package_folder(folder)?)\n",
        after="    Ok(folder_is_directory && holds_package_checkouts(folder)?)\n",
        tests=("ops::xcode::tests::a_derived_data_folder_that_is_a_repository_is_never_split",),
        marker="PV xcode/repository-folder-apply",
    ),
    # Feature evals (tests/evals.rs): each runs the rebuilt binary against a
    # planted fixture, so these prove the black-box assertions can fail.
    Case(
        name="eval/caches-plan",
        relative_path="src/ops/caches.rs",
        # Homebrew's Git clone stops counting, so the whole cache is offered.
        before="        if file_type.is_dir() && has_git_marker(&entry.path())? {\n",
        after="        if file_type.is_dir() && false && has_git_marker(&entry.path())? {\n",
        tests=("eval_clean_caches_removes_every_listed_cache_and_nothing_else",),
        marker="PV eval/caches-plan",
        target="evals",
    ),
    Case(
        name="eval/caches-apply",
        relative_path="src/ops/caches.rs",
        # Apply reports every cache removed while removing none of them.
        before="                apply_filesystem_finding(self.name(), finding, ctx)\n            })()",
        after="                Ok(())\n            })()",
        tests=("eval_clean_caches_removes_every_listed_cache_and_nothing_else",),
        marker="PV eval/caches-apply",
        target="evals",
    ),
    Case(
        name="eval/drift-new-target",
        relative_path="src/app.rs",
        # Apply rescans after consent (same permanent mode, so the mutant can
        # never reach Finder's Trash) instead of consuming the previewed plan.
        before="    let danger = safety::plan_danger(&findings);\n    if let Err(error) = safety::gate(danger, ctx, &findings) {\n        return command_error(operation.name(), false, &findings, ctx, error);\n    }\n",
        after="    let danger = safety::plan_danger(&findings);\n    if let Err(error) = safety::gate(danger, ctx, &findings) {\n        return command_error(operation.name(), false, &findings, ctx, error);\n    }\n    let mut findings = operation.scan(ctx, &ops::project::ScanObservations::default())?;\n    report::effective_actions(&mut findings, cli.shred);\n",
        tests=("eval_apply_ignores_new_targets_and_refuses_swapped_ones_after_preview",),
        marker="PV eval/drift-new-target",
        target="evals",
    ),
    Case(
        name="eval/drift-swapped-target",
        relative_path="src/ops/mod.rs",
        # The sink forgets the preview-time identity: it adopts whatever is
        # at the path now, which disarms both the pre-removal check and the
        # post-quarantine recheck that would otherwise mask this mutant.
        before="    if actual != expected {\n        anyhow::bail!(\"target identity changed after preview; refusing\");",
        after="    let expected = actual;\n    if actual != expected {\n        anyhow::bail!(\"target identity changed after preview; refusing\");",
        tests=("eval_apply_ignores_new_targets_and_refuses_swapped_ones_after_preview",),
        marker="PV eval/drift-swapped-target",
        target="evals",
    ),
    Case(
        name="eval/write-ahead",
        relative_path="src/journal.rs",
        # A failed attempt record no longer stops the mutation it precedes.
        before="    append_at(&location, &record)\n        .with_context(|| format!(\"cannot write apply journal: {}\", ctx.journal_path.display()))?;\n    Ok(JournalAttempt {",
        after="    let _ = append_at(&location, &record);\n    Ok(JournalAttempt {",
        tests=("eval_an_unwritable_journal_prevents_every_removal",),
        marker="PV eval/write-ahead",
        target="evals",
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


def build_test_binary(workspace: Path, target_dir: Path, deadline: float, target: str) -> Path:
    """Compile one test target and return the executable cargo produced.

    An integration test target also rebuilds the `devtrim` binary it runs
    (`CARGO_BIN_EXE_devtrim`), so a mutant is exercised through the real
    entry point, not a stale build.
    """
    remaining = int(max(1, deadline - time.monotonic()))
    selection = ["--lib"] if target == "lib" else ["--test", target]
    kind = ["lib"] if target == "lib" else ["test"]
    result = run(
        [
            "rustup", "run", TOOLCHAIN, "cargo", "test",
            "--locked", "--offline", *selection, "--all-features",
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
            built = message.get("target", {})
            if built.get("kind") == kind and (target == "lib" or built.get("name") == target):
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
    # An optional name prefix narrows the run while developing a case; the
    # gate runs with no argument, so every case.
    prefix = sys.argv[1] if len(sys.argv) > 1 else ""
    cases = [case for case in CASES if case.name.startswith(prefix)]
    if not cases:
        fail(f"ERROR: no case name starts with {prefix!r}")
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
        for case in cases
    }

    baselines = {}
    for target in sorted({case.target for case in cases}):
        baselines[target] = build_test_binary(workspace, target_dir, deadline, target)
        if baselines[target] is None:
            fail(f"ERROR: the unmutated copy did not compile ({target}); gate cannot run")

    # Every named test must pass before it can prove anything by failing.
    for case in cases:
        for test in case.tests:
            result = run_one_test(baselines[case.target], test, target_dir)
            if not selected_exactly_one(result.stdout):
                fail(f"ERROR {case.name}: expected exactly one test, ran 0 ({test})")
            if result.returncode != 0:
                fail(f"ERROR {case.name}: baseline test already fails ({test})")

    caught = []
    for case in cases:
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

        mutant = build_test_binary(workspace, target_dir, deadline, case.target)
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
            location = re.search(r"((?:src|tests)/[^\s:]+:\d+)", combined)
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
