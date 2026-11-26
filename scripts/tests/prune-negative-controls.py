"""Run the test-pruning negative controls on isolated harness copies."""

import importlib.util
from pathlib import Path


spec = importlib.util.spec_from_file_location(
    "planted_violations", Path(__file__).with_name("planted-violations.py")
)
harness = importlib.util.module_from_spec(spec)
spec.loader.exec_module(harness)
Case = harness.Case

diagnostic = '    error_finding(\n        format!("{subject}: unreadable folders"),'
effect_words = '''            Self::Trash => "to Trash",
            Self::Permanent => "PERMANENT",
            Self::ReadOnly => "read-only",
'''
swapped_effect_words = '''            Self::Trash => "read-only",
            Self::Permanent => "PERMANENT",
            Self::ReadOnly => "to Trash",
'''
confirmation_size = (
    '            "{verb} {count} {noun}{plural} ({}){tail}",\n'
    "            report::gb(bytes)\n"
)

harness.CASES = (
    Case(
        name="prune/leftovers-duplicate-path",
        relative_path="src/ops/project.rs",
        before=diagnostic,
        after='    if subject == "leftovers" {\n'
        '        named = format!("{named}, {named}");\n'
        "    }\n" + diagnostic,
        tests=("eval_leftovers_report_every_folder_past_an_unreadable_one",),
        marker="the unreadable folder must be named exactly once:",
        target="eval_system",
    ),
    Case(
        name="prune/toolchains-duplicate-error",
        relative_path="src/ops/toolchains.rs",
        before='            findings.push(unread_folders_finding("Swift toolchains", &unread));\n',
        after='            findings.push(unread_folders_finding("Swift toolchains", &unread));\n'
        '            findings.push(unread_folders_finding("Swift toolchains", &unread));\n',
        tests=("eval_toolchains_list_every_finding_past_an_unreadable_toolchain",),
        marker="eval/toolchains-unreadable: expected one error:",
        target="eval_system",
    ),
    Case(
        name="prune/simulators-duplicate-error",
        relative_path="src/ops/simulators.rs",
        before='        findings.push(unread_folders_finding("simulator devices", &unread));\n',
        after='        findings.push(unread_folders_finding("simulator devices", &unread));\n'
        '        findings.push(unread_folders_finding("simulator devices", &unread));\n',
        tests=("eval_simulators_list_every_device_past_an_unreadable_one",),
        marker="eval/simulators-unreadable: expected one error:",
        target="eval_system",
    ),
    Case(
        name="prune/xcode-hidden-archive-name",
        relative_path="src/ops/project.rs",
        before=diagnostic,
        after='    if subject == "Xcode support folders" {\n'
        '        named = "path unavailable".to_owned();\n'
        "    }\n" + diagnostic,
        tests=("eval_an_unreadable_archive_never_withholds_device_support",),
        marker="the unreadable archive must be named:",
        target="eval_system",
    ),
    Case(
        name="prune/agents-hidden-readable-neighbors",
        relative_path="src/ops/agents.rs",
        before='            findings.push(unread_folders_finding("agent caches and history", &unread));',
        after="            findings.retain(|finding| finding.target().is_none());\n"
        '            findings.push(unread_folders_finding("agent caches and history", &unread));',
        tests=("ops::agents::tests::an_unreadable_history_folder_withholds_only_itself",),
        marker="left == right",
    ),
    Case(
        name="prune/agents-changed-diagnostic",
        relative_path="src/ops/agents.rs",
        before='unread_folders_finding("agent caches and history", &unread)',
        after='unread_folders_finding("agent files", &unread)',
        tests=("eval_clean_agents_removes_only_regenerable_caches_and_stale_history",),
        marker="eval/agents-unreadable: the unreadable folder must be named:",
        target="eval_system",
    ),
    Case(
        name="prune/trash-refusal-without-reason",
        relative_path="src/app.rs",
        before="trash-empty --only-devtrim is preview-only: devtrim cannot yet prove",
        after="trash-empty --only-devtrim is restricted: devtrim cannot yet prove",
        tests=("eval_trash_empty_only_devtrim_selects_only_journaled_identities",),
        marker="the refusal must say why:",
        target="eval_system",
    ),
    Case(
        name="prune/installers-stops-at-refusal",
        relative_path="src/ops/installers.rs",
        before="                outcome.fail(error);\n                continue;",
        after="                outcome.fail(error);\n                break;",
        tests=("review_installers_refused_first_archive_does_not_stop_the_second",),
        marker="review/installers-order: the archive after the refusal was kept:",
        target="eval_review",
    ),
    Case(
        name="prune/installers-missing-partial-free-space",
        relative_path="src/app.rs",
        before="    match report::home_volume_line(summary) {",
        after='    match report::home_volume_line(summary).filter(|_| summary.op != "installers") {',
        tests=("review_installers_refused_first_archive_does_not_stop_the_second",),
        marker="the partial summary must report measured free space:",
        target="eval_review",
    ),
    Case(
        name="prune/home-effect-words-swapped",
        relative_path="src/tui.rs",
        before=effect_words,
        after=swapped_effect_words,
        tests=("tui::tests::every_home_row_names_its_effect_in_words_and_every_operation_fits",),
        marker="tui/home-row-effect: expected",
    ),
    Case(
        name="prune/result-effect-words-swapped",
        relative_path="src/tui.rs",
        before=effect_words,
        after=swapped_effect_words,
        tests=("tui::tests::every_result_row_ends_with_its_danger_and_effect_whole",),
        marker="PV tui/row-keeps-effect",
    ),
    Case(
        name="prune/confirmation-selected-bytes-overstated",
        relative_path="src/tui.rs",
        before=confirmation_size,
        after=confirmation_size.replace("report::gb(bytes)", "report::gb(bytes.saturating_add(1 << 30))"),
        tests=("tui::tests::the_confirmation_names_how_much_of_the_preview_it_covers",),
        marker="the confirmation must report the selected total:",
    ),
    Case(
        name="prune/confirmation-permanent-bytes-overstated",
        relative_path="src/tui.rs",
        before=confirmation_size,
        after=confirmation_size.replace(
            "report::gb(bytes)",
            'report::gb(if verb == "permanently deletes" {\n'
            "                bytes.saturating_add(1 << 30)\n"
            "            } else {\n"
            "                bytes\n"
            "            })",
        ),
        tests=("tui::tests::the_confirmation_leads_with_the_effect_and_says_whether_it_can_be_undone",),
        marker="Permanently deletes 1 item (2.0 GB). missing:",
    ),
)

if __name__ == "__main__":
    if harness.main() != 0:
        raise SystemExit(2)
    print("Expected red result: every temporary mutant failed its intended assertion.")
    raise SystemExit(1)
