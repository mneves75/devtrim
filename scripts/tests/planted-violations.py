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
# Every mutant is a full rebuild; a feature-eval mutant also relinks the
# binary and runs it against real fixtures, about a minute each.
TOTAL_DEADLINE_SECONDS = 3600
# Unit fixtures can also build and query many real Git repositories. Give
# them the same bounded budget as feature evals; a timeout is never proof.
TEST_TIMEOUT_SECONDS = 120

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
        name="review013/build-cwd-preview",
        relative_path="src/safety.rs",
        before="        if !path.is_absolute() {\n",
        after="        if path.as_os_str().is_empty() {\n",
        tests=("eval_unreadable_build_cwd_refuses_project_cleanup",),
        marker="PV eval/build-cwd-unreadable", target="eval_system",
    ),
    Case(
        name="review013/build-cwd-apply",
        relative_path="src/safety.rs",
        before="        if !path.is_absolute() {\n",
        after="        if path.as_os_str().is_empty() {\n",
        tests=("eval_build_cwd_that_becomes_unreadable_after_preview_refuses_apply",),
        marker="PV eval/build-cwd-drift", target="eval_system",
    ),
    Case(
        name="review013/trash-complete-owned",
        relative_path="src/ops/mod.rs",
        before="crate::journal::read_complete_history(&ctx.journal_path)",
        after="crate::journal::read_history(&ctx.journal_path, 1000)",
        tests=("eval_trash_owned_identity_beyond_display_limit_across_retained_generations",),
        marker="PV eval/trash-retained-owned", target="eval_safety",
    ),
    Case(
        name="review013/trash-complete-malformed",
        relative_path="src/ops/mod.rs",
        before="crate::journal::read_complete_history(&ctx.journal_path)",
        after="crate::journal::read_history(&ctx.journal_path, 1000)",
        tests=("eval_trash_malformed_retained_record_beyond_display_limit_refuses_everything",),
        marker="PV eval/trash-retained-malformed", target="eval_safety",
    ),
    Case(
        name="review013/trash-complete-budget",
        relative_path="src/journal.rs",
        before="            if total > MAX_COMPLETE_HISTORY_BYTES as u64 {\n",
        after="            if false {\n",
        tests=("eval_trash_retained_snapshot_over_resource_budget_refuses_everything",),
        marker="PV eval/trash-retained-budget", target="eval_safety",
    ),
    Case(
        name="review013/tui-scan-error",
        relative_path="src/tui.rs",
        before="            || self\n                .findings\n                .iter()\n                .any(|finding| finding.scan_error().is_some())\n",
        after="",
        tests=("tui::tests::scan_error_results_preserve_failure_across_every_route",),
        marker="PV tui/scan-error-results",
    ),
    Case(
        name="review013/status-unreadable-layout",
        relative_path="src/status.rs",
        before='        Err(error) => Err(error).context("cannot inspect /System/Volumes/Data"),\n',
        after="        Err(_) => Ok(SystemTool::RootFilesystem),\n",
        tests=("status::tests::disk_layout_selection_fails_closed_except_for_absence",),
        marker="PV status/disk-layout",
    ),
    Case(
        name="review013/status-nondirectory-layout",
        relative_path="src/status.rs",
        before='        Ok(false) => anyhow::bail!("/System/Volumes/Data is not a directory"),\n',
        after="        Ok(false) => Ok(SystemTool::RootFilesystem),\n",
        tests=("status::tests::disk_layout_selection_fails_closed_except_for_absence",),
        marker="PV status/disk-layout",
    ),
    Case(
        name="review013/status-data-failure",
        relative_path="src/status.rs",
        before="        Ok(tool) => read(&mut unavailable, tool, |output| parse_df(&output)),\n",
        after="        Ok(tool) => capture(tool).and_then(|output| parse_df(&output)).ok().or_else(|| read(&mut unavailable, SystemTool::RootFilesystem, |output| parse_df(&output))),\n",
        tests=("eval_status_data_failure_never_uses_sealed_root",),
        marker="PV eval/status-data-failure", target="eval_reports",
    ),
    Case(
        name="review013/artifacts-dependency-root",
        relative_path="src/ops/artifacts.rs",
        before="    if has_node_modules_ancestor(root)\n        || root\n",
        after="    if root\n",
        tests=("eval_project_roots_below_dependency_namespaces_offer_nothing",),
        marker="PV eval/excluded-roots-dependencies", target="evals",
    ),
    Case(
        name="review013/node-modules-dependency-root",
        relative_path="src/ops/node_modules.rs",
        before="    if has_node_modules_ancestor(root)\n        || root\n",
        after="    if root\n",
        tests=("eval_project_roots_below_dependency_namespaces_offer_nothing",),
        marker="PV eval/excluded-roots-dependencies", target="evals",
    ),
    Case(
        name="review013/artifacts-git-root",
        relative_path="src/ops/artifacts.rs",
        before="    if has_node_modules_ancestor(root)\n        || root\n            .components()\n            .any(|component| is_git_metadata_name(component.as_os_str()))\n",
        after="    if has_node_modules_ancestor(root)\n",
        tests=("eval_project_roots_below_git_metadata_offer_nothing",),
        marker="PV eval/excluded-roots-git", target="evals",
    ),
    Case(
        name="review013/node-modules-git-root",
        relative_path="src/ops/node_modules.rs",
        before="    if has_node_modules_ancestor(root)\n        || root\n            .components()\n            .any(|component| is_git_metadata_name(component.as_os_str()))\n",
        after="    if has_node_modules_ancestor(root)\n",
        tests=("eval_project_roots_below_git_metadata_offer_nothing",),
        marker="PV eval/excluded-roots-git", target="evals",
    ),
    Case(
        name="eval/report-leftovers",
        relative_path="src/ops/leftovers.rs",
        before="        && suffix.len() == 6\n",
        after="        && suffix.len() == 7\n",
        tests=("eval_leftovers_reports_exact_hints_and_sizes",),
        marker="PV eval/leftovers-hints", target="eval_reports",
    ),
    Case(
        name="eval/report-history-limit",
        relative_path="src/journal.rs",
        before="        HistoryScope::Tail(limit.clamp(1, 1000)),\n",
        after="        HistoryScope::Tail(limit.max(1000)),\n",
        tests=("eval_history_limit_selects_newest_results_across_rotation",),
        marker="PV eval/history-limit", target="eval_reports",
    ),
    Case(
        name="eval/report-status-unavailable",
        relative_path="src/status.rs",
        before="    Ok(if report.unavailable.is_empty() {\n",
        after="    Ok(if true {\n",
        tests=("eval_status_uses_resident_memory_and_names_unavailable_metrics",),
        marker="PV eval/status-unavailable", target="eval_reports",
    ),
    Case(
        name="eval/report-completions",
        relative_path="src/app.rs",
        before='    clap_complete::generate(shell, &mut command, "devtrim", &mut output);\n',
        after="    let _ = (shell, &mut command);\n",
        tests=("eval_generated_docs_expose_commands_and_refuse_json",),
        marker="PV eval/generated-commands", target="eval_reports",
    ),
    Case(
        name="eval/report-manpage",
        relative_path="src/app.rs",
        before="    clap_mangen::Man::new(cli::Cli::command()).render(&mut output)?;\n",
        after="",
        tests=("eval_generated_docs_expose_commands_and_refuse_json",),
        marker="PV eval/generated-commands", target="eval_reports",
    ),
    Case(
        name="eval/report-largest",
        relative_path="src/largest.rs",
        before="    let limit = top.unwrap_or(20).clamp(1, 100);",
        after="    let limit = 100; let _ = top;",
        tests=("eval_largest_ranks_fixture_totals_and_honors_top",),
        marker="PV eval/largest-ranking", target="eval_reports",
    ),
    Case(
        name="eval/report-icloud",
        relative_path="src/ops/icloud.rs",
        before="if logical < 100 * 1024 * 1024 {",
        after="if logical <= 100 * 1024 * 1024 {",
        tests=("eval_icloud_reports_large_logical_files_with_local_allocation",),
        marker="PV eval/icloud-threshold", target="eval_reports",
    ),
    Case(
        name="eval/report-uninstall",
        relative_path="src/uninstall.rs",
        before="    name == identifier\n",
        after="    name.starts_with(identifier)\n",
        tests=("eval_uninstall_attributes_only_exact_bundle_identifier",),
        marker="PV eval/uninstall-identifier", target="eval_reports",
    ),
    Case(
        name="eval/report-scan-summary",
        relative_path="src/report.rs",
        before="const SCAN_LARGEST: usize = 5;",
        after="const SCAN_LARGEST: usize = 6;",
        tests=("eval_scan_human_summarizes_large_categories_and_all_lists_every_target",),
        marker="PV eval/scan-summary", target="eval_reports",
    ),
    Case(
        name="eval/report-scan-all",
        relative_path="src/report.rs",
        before="if all || part.len() <= SCAN_FULL_LISTING {",
        after="if part.len() <= SCAN_FULL_LISTING {",
        tests=("eval_scan_human_summarizes_large_categories_and_all_lists_every_target",),
        marker="PV eval/scan-all", target="eval_reports",
    ),
    Case(
        name="eval/report-status-memory",
        relative_path="src/status.rs",
        before="        .checked_add(wired)\n",
        after="        .checked_add(wired.saturating_add(inactive))\n",
        tests=("eval_status_uses_resident_memory_and_names_unavailable_metrics",),
        marker="PV eval/status-memory", target="eval_reports",
    ),
    Case(
        name="eval/report-analyze-partial",
        relative_path="src/analyze.rs",
        before="    Ok(if errors.is_empty() {\n",
        after="    Ok(if true {\n",
        tests=("eval_analyze_measures_children_and_discloses_partial_lower_bounds",),
        marker="PV eval/analyze-partial", target="eval_reports",
    ),
    Case(
        name="eval/safety-uv-busy",
        relative_path="src/safety.rs",
        before="    match rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive) {\n",
        after="    match Ok::<(), rustix::io::Errno>(()) {\n",
        tests=("eval_busy_uv_cache_is_kept_while_later_caches_are_removed",),
        marker="PV eval/uv-busy", target="eval_safety",
    ),
    Case(
        name="eval/safety-caches-continue",
        relative_path="src/ops/caches.rs",
        before="                outcome.fail(error);\n                continue;\n",
        after="                outcome.fail(error);\n                break;\n",
        tests=("eval_busy_uv_cache_is_kept_while_later_caches_are_removed",),
        marker="PV eval/caches-continue", target="eval_safety",
    ),
    Case(
        name="eval/safety-agents-continue",
        relative_path="src/ops/agents.rs",
        before="                outcome.fail(error);\n                continue;\n",
        after="                outcome.fail(error);\n                break;\n",
        tests=("eval_agents_continues_after_a_cache_holding_a_repository_is_refused",),
        marker="PV eval/agents-continue", target="eval_safety",
    ),
    Case(
        name="eval/safety-journal-symlink",
        relative_path="src/journal.rs",
        before="            flags | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,\n",
        after="            flags | OFlags::NONBLOCK | OFlags::CLOEXEC,\n",
        tests=("eval_a_symlinked_journal_refuses_apply_without_touching_its_destination",),
        marker="PV eval/journal-symlink", target="eval_safety",
    ),
    Case(
        name="eval/safety-noninteractive",
        relative_path="src/safety.rs",
        before='        bail!("non-interactive run: re-run with -y to confirm danger-{max_danger} operations");\n',
        after="        return Ok(());\n",
        tests=("eval_noninteractive_apply_requires_explicit_consent",),
        marker="PV eval/noninteractive-consent", target="eval_safety",
    ),
    Case(
        name="eval/safety-build-busy",
        relative_path="src/ops/project.rs",
        before="    process_cwds.iter().any(|cwd| cwd.starts_with(repo))\n",
        after="    let _ = (repo, process_cwds); false\n",
        tests=("eval_build_process_liveness_keeps_only_the_busy_repository",),
        marker="PV eval/build-busy", target="eval_safety",
    ),
    Case(
        name="sink/removal-root-device",
        relative_path="src/ops/mod.rs",
        before="    ensure_same_device(actual, parent_device, path)?;\n",
        after="    let _ = (actual, path);\n",
        tests=("ops::tests::both_removal_modes_refuse_a_root_on_a_foreign_parent_device",),
        marker="PV sink/removal-root-device",
    ),
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
        before="            if artifact_evidence(path)?.is_none() {\n",
        after="            if false {\n",
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
        before="            if tracks_files_under(&owner, path)? {\n",
        after="            if false && tracks_files_under(&owner, path)? {\n",
        tests=("ops::artifacts::tests::a_tree_its_repository_tracks_is_never_offered_or_removed",),
        marker="PV artifacts/tracked-apply",
    ),
    Case(
        name="node_modules/tracked-apply",
        relative_path="src/ops/node_modules.rs",
        before="            if tracks_files_under(&owner, path)? {\n",
        after="            if false && tracks_files_under(&owner, path)? {\n",
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
        before="                Some(Authored::ProgramKeypair(keypair)) => anyhow::bail!(\n",
        after="                Some(Authored::ProgramKeypair(keypair)) if false => anyhow::bail!(\n",
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
        before="                Some(Authored::Repository(marker)) => anyhow::bail!(\n",
        after="                Some(Authored::Repository(marker)) if false => anyhow::bail!(\n",
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
        before="            if let Some(output) = build_output_between(&owner, path)? {\n",
        after="            if let Some(output) = build_output_between(&owner, path)?.filter(|_| false) {\n",
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
        before="            if !has_manifest(path)? {\n",
        after="            if false && !has_manifest(path)? {\n",
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
        # Complete reads now refuse before the legacy errors-list check.
        # Swallowing that refusal must not look like an empty, healthy history.
        before="crate::journal::read_complete_history(&ctx.journal_path)",
        after="crate::journal::read_complete_history(&ctx.journal_path).or_else(|_| Ok::<_, anyhow::Error>(crate::journal::History { entries: Vec::new(), errors: Vec::new() }))",
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
        before="                Some(Authored::TerraformState(state)) => anyhow::bail!(\n                    \"refusing {}: it holds the Terraform state {}\",\n                    path.display(),\n                    state.display()\n                ),\n",
        after="                Some(Authored::TerraformState(_)) => {}\n",
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
        before="    let deletion_device = checked_removal_root(actual, expected, parent_identity.dev, &path)?;",
        after="    let expected = actual;\n    let deletion_device = checked_removal_root(actual, expected, parent_identity.dev, &path)?;",
        tests=("eval_apply_ignores_new_targets_and_refuses_swapped_ones_after_preview",),
        marker="PV eval/drift-swapped-target",
        target="evals",
    ),
    Case(
        name="eval/projects-plan",
        relative_path="src/ops/artifacts.rs",
        # Terraform state no longer keeps its build directory out of the plan.
        before="                        Some(Authored::TerraformState(_)) => {\n                            states_here += 1;\n                            continue;\n",
        after="                        Some(Authored::TerraformState(_)) => {\n                            states_here += 1;\n",
        tests=("eval_clean_artifacts_removes_only_corroborated_stale_output",),
        marker="PV eval/projects-plan",
        target="evals",
    ),
    Case(
        name="eval/projects-apply",
        relative_path="src/ops/node_modules.rs",
        # Apply reports each install removed while removing none.
        before="            match recheck(path).and_then(|()| apply_filesystem_finding(self.name(), finding, ctx)) {",
        after="            match recheck(path) {",
        tests=("eval_clean_node_modules_removes_only_stale_installs",),
        marker="PV eval/projects-apply",
        target="evals",
    ),
    Case(
        name="eval/hostile-repo",
        relative_path="src/ops/project.rs",
        # Git's fsmonitor is no longer disabled; `git ls-files` runs it.
        before="            \"core.fsmonitor=false\",\n",
        after="            \"devtrim.eval-mutant=false\",\n",
        tests=("eval_clean_node_modules_removes_only_stale_installs",),
        marker="PV eval/hostile-repo",
        target="evals",
    ),
    Case(
        name="eval/read-only",
        relative_path="src/journal.rs",
        # Opening any command creates devtrim's state folder if it is absent.
        before="    let location = match JournalLocation::open(path, ParentMode::Existing)? {\n        Some(location) => location,\n        None => return Ok(warnings),",
        after="    let location = match JournalLocation::open(path, ParentMode::Create)? {\n        Some(location) => location,\n        None => return Ok(warnings),",
        tests=("eval_read_only_commands_and_previews_change_nothing",),
        marker="PV eval/read-only",
        target="evals",
    ),
    Case(
        name="eval/xcode-age",
        relative_path="src/ops/xcode.rs",
        # A build tree touched within the window no longer counts as in use.
        before="    Ok((newest > cutoff).then_some(InUse::Recent))\n",
        after="    Ok((newest > cutoff && false).then_some(InUse::Recent))\n",
        tests=("eval_clean_xcode_removes_only_idle_build_trees_while_xcode_runs",),
        marker="PV eval/xcode-plan",
        target="evals",
    ),
    Case(
        name="eval/xcode-open-file",
        relative_path="src/ops/xcode.rs",
        # A file Xcode holds open no longer keeps its folder. Both checks go:
        # the by-identity one alone would still keep it, masking the mutant.
        before="        .any(|file| file.path.starts_with(folder) || file.path.starts_with(&canonical))\n        || held_open_by_identity(folder, &same_volume)?\n",
        after="        .any(|file| false && (file.path.starts_with(folder) || file.path.starts_with(&canonical)))\n        || (false && held_open_by_identity(folder, &same_volume)?)\n",
        tests=("eval_clean_xcode_removes_only_idle_build_trees_while_xcode_runs",),
        marker="PV eval/xcode-plan",
        target="evals",
    ),
    Case(
        name="eval/windows",
        relative_path="src/safety.rs",
        # Lowering the project window shortens retention with it.
        before="    let retain = retain.map_or(active.max(30), |days| days.max(1));",
        after="    let retain = retain.map_or(active, |days| days.max(1));",
        tests=("eval_the_project_window_never_shortens_retention",),
        marker="PV eval/windows",
        target="evals",
    ),
    Case(
        name="eval/toolchains-drift",
        relative_path="src/ops/toolchains.rs",
        # Apply no longer rechecks that a toolchain is still unreferenced.
        before="    if preserved.contains(&canonical) {\n        anyhow::bail!(\n            \"toolchain became referenced after preview: {}\",",
        after="    if preserved.is_empty() {\n        anyhow::bail!(\n            \"toolchain became referenced after preview: {}\",",
        tests=("eval_toolchains_apply_refuses_a_toolchain_referenced_after_preview",),
        marker="PV eval/toolchains-drift",
        target="eval_system",
    ),
    Case(
        name="eval/installers-drift",
        relative_path="src/ops/installers.rs",
        # Apply no longer rechecks an installer's age.
        before="if installer_details(target, &ctx.home, ctx.retain_days)?.is_none() {",
        after="if false {",
        tests=("eval_installers_apply_refuses_an_archive_modified_after_preview",),
        marker="PV eval/installers-drift",
        target="eval_system",
    ),
    Case(
        name="eval/simulators-recheck",
        relative_path="src/ops/simulators.rs",
        # Apply treats every listed device as still unavailable.
        before="match current.get(udid) {",
        after="match current.get(udid).map(|_| &false) {",
        tests=("eval_simulators_apply_refuses_a_device_that_changed_after_preview",),
        marker="PV eval/simulators-recheck",
        target="eval_system",
    ),
    Case(
        name="eval/docker-volumes",
        relative_path="src/report.rs",
        # The build-cache command prunes volumes instead.
        before="[\"--host\", host, \"builder\", \"prune\", \"-a\", \"-f\"]",
        after="[\"--host\", host, \"volume\", \"prune\", \"-a\", \"-f\"]",
        tests=("eval_clean_docker_prunes_images_and_build_cache_never_volumes",),
        marker="PV eval/docker-plan",
        target="eval_system",
    ),
    Case(
        name="eval/docker-down",
        relative_path="src/ops/docker.rs",
        # The daemon-down error stops naming the VM image and its size.
        before="produced{disclosed}\"",
        after="produced\"",
        tests=("eval_docker_daemon_down_names_the_vm_image_and_runs_no_prune",),
        marker="PV eval/docker-down",
        target="eval_system",
    ),
    Case(
        name="eval/optimize-refusal",
        relative_path="src/ops/optimize.rs",
        # Apply without an explicit task is no longer refused.
        before="            if apply {",
        after="            if apply && false {",
        tests=("eval_optimize_runs_exactly_the_selected_fixed_argv",),
        marker="PV eval/optimize-refusal",
        target="eval_system",
    ),
    Case(
        name="eval/trash-git-child",
        relative_path="src/ops/mod.rs",
        # A `.git` child of the Trash is offered like any other.
        before="if is_git_metadata_name(&entry.file_name()) {",
        after="if false {",
        tests=("eval_trash_empty_purges_exactly_the_previewed_direct_children",),
        marker="PV eval/trash-empty-plan",
        target="eval_system",
    ),
    Case(
        name="eval/trash-continue",
        relative_path="src/ops/mod.rs",
        # One refused Trash item stops every item after it.
        before="            outcome.fail(error);\n            continue;\n        }\n        outcome.record(finding, format!(\"permanently deleted {}\", finding.label));",
        after="            outcome.fail(error);\n            break;\n        }\n        outcome.record(finding, format!(\"permanently deleted {}\", finding.label));",
        tests=("eval_trash_empty_continues_past_an_item_holding_a_repository",),
        marker="PV eval/trash-empty-continue",
        target="eval_system",
    ),
    Case(
        name="eval/trash-only-devtrim",
        relative_path="src/ops/mod.rs",
        # A failed or interrupted Trash move still selects its item.
        before=".filter(|record| record.action == \"trash\" && record.status.as_deref() == Some(\"ok\"))",
        after=".filter(|record| record.action == \"trash\")",
        tests=("eval_trash_empty_only_devtrim_selects_only_journaled_identities",),
        marker="PV eval/trash-only-plan",
        target="eval_system",
    ),
    Case(
        name="eval/flags",
        relative_path="src/app.rs",
        # trash-empty accepts the permanent-deletion flag instead of rejecting it.
        before="Some(cli::Command::TrashEmpty { .. }) => (\"trash-empty\", true, true, true, false),",
        after="Some(cli::Command::TrashEmpty { .. }) => (\"trash-empty\", true, true, true, true),",
        tests=("eval_commands_reject_flags_they_cannot_honor_and_change_nothing",),
        marker="PV eval/flags-reject",
        target="eval_system",
    ),
    Case(
        name="eval/home-repo",
        relative_path="src/ops/project.rs",
        # A repository at or above the home folder owns the projects below it.
        before="    Ok(nearest_repo(path)?.filter(|repo| !crate::safety::holds_home(repo, home)))\n",
        after="    let _ = home;\n    nearest_repo(path)\n",
        tests=("eval_a_home_folder_repository_never_owns_the_projects_under_it",),
        marker="PV eval/home-repo",
        target="evals",
    ),
    Case(
        name="eval/apply-recheck-node-modules",
        relative_path="src/ops/node_modules.rs",
        # Only the preflight judges a finding; the removals that follow it
        # no longer recheck, so a repository active by then loses its output.
        before="            match recheck(path).and_then(|()| apply_filesystem_finding(self.name(), finding, ctx)) {",
        after="            match apply_filesystem_finding(self.name(), finding, ctx) {",
        tests=("eval_a_repository_that_becomes_active_during_apply_keeps_its_dependencies",),
        marker="PV eval/apply-recheck",
        target="evals",
    ),
    Case(
        name="eval/apply-recheck-artifacts",
        relative_path="src/ops/artifacts.rs",
        # Only the preflight judges a finding; the removals that follow it
        # no longer recheck, so a repository active by then loses its output.
        before="            match recheck(path).and_then(|()| apply_filesystem_finding(self.name(), finding, ctx)) {",
        after="            match apply_filesystem_finding(self.name(), finding, ctx) {",
        tests=("eval_a_repository_that_becomes_active_during_apply_keeps_its_build_output",),
        marker="PV eval/apply-recheck",
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
    # A command that never answers must be killed and reported as a timeout,
    # and a liveness probe that never answers must refuse, not read as exit 1.
    Case(
        name="process/command-timeout",
        relative_path="src/process.rs",
        before="                Ok(None) if Instant::now() >= deadline => {\n",
        after="                Ok(None) if false => {\n",
        tests=("process::tests::a_hung_command_is_killed_reaped_and_reported_as_a_timeout",),
        marker="PV process/timeout",
    ),
    Case(
        name="liveness/agent-processes",
        relative_path="src/safety.rs",
        before='const BUILD_PROCESS_PATTERN: &str = "node|npm|pnpm|yarn|bun|deno|cargo|rustc|go|python|python3|Python|gradle|java|xcodebuild|swift|swiftc|make|ninja|cmake|codex|claude|rust-analyzer|[0-9]+\\\\.[0-9]+\\\\.[0-9]+";' + "\n",
        after='const BUILD_PROCESS_PATTERN: &str = "node|npm|pnpm|yarn|bun|deno|cargo|rustc|go|python|python3|Python|gradle|java|xcodebuild|swift|swiftc|make|ninja|cmake";' + "\n",
        tests=("safety::tests::coding_agents_protect_the_repository_they_work_in",),
        marker="PV liveness/agent-processes",
    ),
    Case(
        name="liveness/probe-timeout",
        relative_path="src/safety.rs",
        before="    let output = probe_output(command, limit, probe)?;\n    parse_pgrep_pids(&output.stdout, output.status.code())\n",
        after="    let Ok(output) = probe_output(command, limit, probe) else {\n        return Ok(Vec::new());\n    };\n    parse_pgrep_pids(&output.stdout, output.status.code())\n",
        tests=("safety::tests::a_probe_that_cannot_finish_refuses_instead_of_reporting_nothing_running",),
        marker="PV liveness/probe-timeout",
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


def run_one_test(case: Case, binary: Path, test: str, target_dir: Path) -> subprocess.CompletedProcess:
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
    if len(sys.argv) > 2:
        fail("usage: planted-violations.py [PREFIX]")
    prefix = sys.argv[1] if len(sys.argv) == 2 else ""
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

    # Refactors can invalidate a binding without changing the guarded behavior.
    # Detect every stale binding before paying for builds and baseline fixtures.
    for case in cases:
        occurrences = pristine[case.relative_path].count(case.before)
        if occurrences != 1:
            fail(
                f"ERROR {case.name}: expected exactly one replacement site, "
                f"found {occurrences} — the guarded code moved or changed shape"
            )

    baselines = {}
    for target in sorted({case.target for case in cases}):
        baselines[target] = build_test_binary(workspace, target_dir, deadline, target)
        if baselines[target] is None:
            fail(f"ERROR: the unmutated copy did not compile ({target}); gate cannot run")

    # Every named test must pass before it can prove anything by failing.
    for case in cases:
        for test in case.tests:
            try:
                result = run_one_test(case, baselines[case.target], test, target_dir)
            except subprocess.TimeoutExpired:
                fail(f"ERROR {case.name}: baseline test timed out ({test})")
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
        path.write_text(original.replace(case.before, case.after, 1))

        mutant = build_test_binary(workspace, target_dir, deadline, case.target)
        path.write_text(original)
        if mutant is None:
            fail(f"ERROR {case.name}: mutant did not compile")

        for test in case.tests:
            try:
                result = run_one_test(case, mutant, test, target_dir)
            except subprocess.TimeoutExpired:
                fail(f"ERROR {case.name}: mutant test timed out ({test})")
            if result.returncode == 0:
                fail(f"MISSED {case.name}: boundary violation survived ({test})")
            combined = result.stdout + result.stderr
            if case.marker not in combined:
                fail(
                    f"ERROR {case.name}: {test} failed, but not at the named "
                    f"assertion '{case.marker}' — a different check refused, so "
                    f"this is not proof the boundary is covered\n{combined}"
                )
            location = re.search(r"((?:src|tests)/[^\s:]+:\d+)", combined)
            observation = (
                f"CAUGHT {case.name}: {test.rsplit('::', 1)[-1]}"
                f" at {location.group(1) if location else 'unknown'}"
            )
            caught.append(observation)
            print(observation, flush=True)

    print(f"planted-violations: {len(caught)} boundary/ies proven covered")
    return 0


if __name__ == "__main__":
    sys.exit(main())
