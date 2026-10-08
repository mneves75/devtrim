# devtrim claims matrix

Branch `feat/feature-evals`, base `01f5ef0` (v0.10.7). Maps each user-facing claim to the test that proves it, so gaps are visible. Read-only research: no cargo build or test was run for this file; every citation below was read in the source, and `Citation check` at the end records the mechanical check.

## How to read it

- Source doc: **R** = README.md, **M** = MANUAL.html, **A** = the `## Conventions` bullets of AGENTS.md (byte-identical to CLAUDE.md).
- Unit proof: `#[test]` in `src/**`, cited as `path:line fn_name` with `src/` omitted (so `ops/agents.rs:1943` is `src/ops/agents.rs`). Also `rule-tests/*.yml` (ast-grep rule tests) and `fuzz/fuzz_targets/*.rs`.
- Black-box proof: `cli.rs:LINE fn_name` is `tests/cli.rs` (real binary, disposable HOME, stub tools on PATH); `tui.py:LINE` and `read-only-views.py:LINE` are `scripts/tests/*.py` (real PTY).
- Planted case: the marker string `PV <name>` from `scripts/tests/planted-violations.py`.
- **Important limit on the Planted column.** The planted-violations gate builds with `cargo test --lib` only (`planted-violations.py:895 build_test_binary`), and every case names a unit test (`ops::`, `safety::`, `tui::`). A planted case therefore proves a *unit* test can fail. It says nothing about any `cli.rs` or PTY row. The `PV-E2E:` string at `cli.rs:874` is only an assertion message; no gate runs a mutant against it. No black-box test in this repository is proven able to fail.
- Gap: `none` = nothing proves the claim. `no black-box` = only unit tests prove it. `partial` = a proof exists but misses a stated branch (named after the colon). `ok` = unit and black-box (or static rule) proof exist for the stated claim. `contradicted` = a doc disagrees with the test.
- "Review only" in a cell means the claim rests on a human reading evidence or documentation (a deletion-list entry's evidence text, a source URL) that no gate can check.
- One row is one checkable property. Rows are not exhaustive within a long Conventions bullet; the clauses I did not split out are listed in the section as `not itemised`.

## Cross-cutting facts that shape every section

1. No `cli.rs` test completes a successful filesystem apply without `--shred`. Trash is the default mode, and no test anywhere reaches `trash::delete` successfully (`ops/mod.rs:418`); the Trash-mode tests refuse before it (`ops/mod.rs:1314`) or write the journal by hand (`ops/mod.rs:2468`). Every PTY apply also uses permanent mode (`tui.py:231`, `tui.py:260`, `tui.py:348`).
2. No test snapshots the filesystem around a preview or a read-only command, so "preview changes nothing" is asserted only by sentinel files in a few mutation tests.
3. The sandbox `pgrep` stub always exits 1 (`cli.rs:29`, in `Sandbox::new`), so "no build or Xcode process is running" is the only liveness state the black-box suite reaches, except where a test overrides it (`cli.rs:1449`, `cli.rs:2769`).
4. `git` is a stub in `cli.rs` except for `cli.rs:1266`. Staleness decisions therefore rest on canned dates; real-Git behaviour is proven by unit tests (`ops/project.rs:1049` and following).

---

## scan

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "read-only report: categories first, then the largest findings" | R Usage, A | report.rs:752 human_plan_groups_consecutive_findings_by_project | cli.rs:410 scan_leads_with_categories_and_lists_the_largest_until_all | none | ok |
| "a category whole up to eight findings, otherwise its five largest with a count of the rest" | A, M | none | cli.rs:410 (12 findings: five plus "7 more") | none | partial: the 8-or-fewer whole-list branch is untested |
| "scan --all lists every finding" | R Usage | none | cli.rs:410 (12 of 12) | none | ok |
| "scan --json one machine-readable envelope, always complete" | R Usage, A | none | cli.rs:395 empty_json_scan_is_one_document; cli.rs:410 | none | ok |
| "READ-ONLY reports and never changes anything" (scan changes nothing) | R TUI, M | none | none (no tree snapshot around a scan) | none | none |
| "scan --shred explicitly previews permanent actions" | R Usage, A | none | cli.rs:197 mutation_flags_remain_available_only_where_they_have_meaning (exit and operation only); shred plus danger 9 asserted for caches only at cli.rs:1899 | none | partial: scan itself never asserted to carry shred actions |
| Read-only commands "reject --apply, -y, --yolo, and --shred" (scan --apply) | R Usage, A | none | cli.rs:160 read_only_commands_reject_mutation_flags | none | ok |
| liveness and Git observations computed once per scan, failures memoized and re-yielded | A | none | cli.rs:2769 scan_runs_each_liveness_probe_once_and_git_once_per_repo | none | ok |
| "scan_all runs the ten categories concurrently ... byte-identical to a serial scan; a panicked scan thread becomes that category's error" | A | none | none | none | none |
| human preview begins with `info scan roots:` naming roots and origin | R | safety.rs:2023 the_roots_note_says_where_the_roots_came_from; tui.rs:2187 project_views_name_their_scan_roots | cli.rs:716 purge_scans_the_default_project_folders_and_names_them | none | ok |
| a failing category exits nonzero with the error in `errors`, other categories still reported | R JSON | none | cli.rs:2769 (failing-probe loop) | none | ok |

Not itemised: scan section ordering by size beyond cli.rs:410.

## purge

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "stale node_modules + build artifacts, by project, largest first" | R Usage, M | ops/purge.rs:116 projects_are_ordered_by_their_total_then_targets_by_size | cli.rs:1349 purge_previews_project_build_output_grouped_largest_project_first | none | ok |
| findings carry `project` "for grouping only, never deletion authority" | R JSON, M | report.rs:752 | cli.rs:716 (line 748); cli.rs:1349 (line 1372) | none | ok |
| "move that exact plan to Trash after confirmation" (applied by each finding's own category) | R Usage, A | ops/purge.rs:191 a_misrouted_finding_is_refused_by_the_category_that_receives_it | cli.rs:1406 purge_applies_each_finding_through_its_own_category (permanent mode) | PV purge/misroute-refused | partial: Trash mode unproven (cross-cutting 1) |
| a misrouted finding is "refused, never deleted" | A | ops/purge.rs:191 | none | PV purge/misroute-refused | no black-box |
| "never counts the same bytes twice" | R, A | ops/purge.rs:145 a_purge_plan_never_counts_the_same_bytes_twice; ops/node_modules.rs:1111 a_node_modules_inside_build_output_belongs_to_that_output | none | PV node_modules/build-output-walk | no black-box |
| "skips a directory holding files its repository tracks" | R | ops/artifacts.rs:1018 a_tree_its_repository_tracks_is_never_offered_or_removed; ops/node_modules.rs:1015 a_committed_node_modules_is_never_offered_or_removed | cli.rs:1082 purge_never_offers_a_build_directory_holding_tracked_files; cli.rs:1266 the_tracked_file_check_uses_real_git_and_ignores_ambient_pathspec_settings | PV project/tracked-files, PV artifacts/tracked-apply, PV node_modules/tracked-apply | ok |
| tracked check survives a failing Git query: "refuses rather than trusts" | A | ops/project.rs:1032 a_failed_git_query_names_gits_reason | cli.rs:1190 a_failing_tracked_file_check_refuses_rather_than_trusts | none | ok |
| skips "a build directory holding a Solana program keypair" | R, A | ops/artifacts.rs:1173 a_tree_holding_a_program_keypair_is_never_offered_or_removed | cli.rs:1234 purge_never_offers_a_build_directory_holding_a_program_keypair | PV artifacts/keypair-scan, PV artifacts/keypair-apply | ok |
| skips "a nested Git repository" in a build directory | R, A | ops/artifacts.rs:1228 a_tree_holding_a_git_repository_is_never_offered_or_removed | cli.rs:1527 project_targets_with_their_own_git_marker_are_rejected (marker at the target root only) | PV artifacts/nested-repository-scan, PV artifacts/nested-repository-apply | partial: black-box covers the root marker, not a repository deeper in the tree |
| "never matches ambiguous names such as build, dist, or coverage" | R, A | ops/artifacts.rs:732 corroboration_matrix_is_fail_closed | none | none | no black-box |
| "never touches a repository with recent Git activity" | R | ops/project.rs:1128 a_fresh_checkout_of_an_old_commit_is_active_not_stale; ops/project.rs:1146 a_recent_head_commit_counts_even_when_the_reflog_does_not_name_it | cli.rs:1994 node_modules_apply_refuses_repo_that_became_active (apply-time recheck, stub git); no preview-time recent-repo case | PV git/reflog-activity, PV git/head-commit | partial: preview skip of a recent repo has no black-box |
| purge "runs its node-modules and artifacts halves independently"; failures contained | R JSON | ops/node_modules.rs:1375 a_failing_repository_blocks_only_its_own_findings; ops/artifacts.rs:1432 an_unreadable_tree_refuses_its_repository_rather_than_offers | cli.rs:984 purge_contains_each_failure_to_where_it_happened | PV node_modules/repository-contained, PV artifacts/repository-contained | ok |
| purge accepts the cleanup flags clean accepts (`--shred`) | R | none | cli.rs:1439 purge_accepts_the_cleanup_flags_clean_accepts | none | ok |
| default project folders (`~/dev`, `~/.codex/worktrees`, ...) and `info scan roots:` line | R | safety.rs:1909 default_roots_are_the_conventional_project_folders_that_exist | cli.rs:716; cli.rs:765 purge_finds_dependencies_in_codex_worktrees | PV roots/default-home-link, PV roots/default-home-identity | ok |
| TUI `p` opens the project purge view; Space leaves a finding out | R TUI | tui.rs:2632 project_purge_is_reachable_from_the_menu | tui.py:348 verify_purge | PV tui/selection-plan | ok |

Not itemised: Mole parity statements (R/A cite Mole V1.56.0 behaviour; no test compares against Mole).

## clean caches

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "tool download caches (HF, uv, npm, brew, cargo, bun, gh, ...)" list is exactly what is offered | R Usage, M | ops/caches.rs:606 every_built_in_cache_carries_evidence (evidence present only) | cli.rs:2386 huggingface_cleanup_preserves_authentication_and_other_state | PV evidence/built-in-caches | partial: no test asserts the list contents; evidence correctness is review only |
| Hugging Face "targets only ~/.cache/huggingface/hub"; tokens and settings never authority | R, A | ops/caches.rs:827 huggingface_authority_accepts_only_model_cache | cli.rs:2386 | none | ok |
| closed list of exact `~/Library/Caches` subdirectories; "~/Library stays protected wholesale" | R, A | safety.rs:2265 managed_library_caches_are_exact_exceptions; ops/caches.rs:630 library_cache_authority_matches_the_protection_carve_out | cli.rs:1116 a_cache_rebuilt_only_by_a_command_names_that_command | PV evidence/library-caches | ok |
| JetBrains and deno cache dirs excluded; a listed name is not a prefix licence | A | safety.rs:2265 (still_protected list) | none | none | no black-box |
| pnpm entry is the metadata cache, never `~/Library/pnpm/store` | R, A | none (the store path is not in any assertion) | none | none | none |
| a cache rebuilt only by a command names that command, not "rebuilt on next use" | A | ops/caches.rs:618 every_command_rebuilt_cache_is_a_managed_library_cache | cli.rs:1116 | none | ok |
| Homebrew cache offered whole unless it holds a `<name>--git` clone, then its other direct children | R, A | ops/caches.rs:495 a_homebrew_cache_holding_a_git_clone_is_offered_around_it | cli.rs:915 a_homebrew_cache_reported_through_a_linked_home_is_still_offered (linked home only) | PV caches/brew-clone-scan, PV caches/brew-entry-apply, PV caches/brew-entry-repository, PV caches/brew-entry-direct-child | partial: the clone-skipping behaviour has no black-box |
| owner-reported cache paths outside the npm/Homebrew namespaces are refused; apply reasserts | R Protected paths, A | ops/caches.rs:407 owner_cache_roots_are_program_specific_and_normalized; ops/caches.rs:448 apply_reasserts_owner_namespace_and_preserves_sentinel; ops/caches.rs:841 standard_authority_rejects_forged_cache_subpath | cli.rs:1911 owner_reported_cache_outside_namespace_is_skipped | none | ok |
| owner probes (`brew --cache`, `npm config get cache`) run from HOME, not the invoking directory | A | ops/caches.rs:407 | cli.rs:1803 owner_cache_probes_run_from_home_not_the_invoking_directory | none | ok |
| uv cache removed only under uv's own lock; held lock refuses only that finding | R, A | ops/caches.rs:708 uv_cache_is_refused_while_a_uv_process_holds_its_lock | none | PV caches/uv-lock | no black-box |
| uv lock never created through a symlinked ancestor | A | ops/caches.rs:776 uv_lock_is_never_created_through_a_symlinked_ancestor | none | PV caches/ancestor-lock | no black-box |
| "caches apply continues past a refused finding", run still nonzero | R JSON, A | ops/caches.rs:655 a_refused_cache_does_not_block_the_rest_of_the_plan | none | none (caches is not in the planted set) | no black-box |
| empty apply emits a zero `summary` | R JSON | none | cli.rs:1727 empty_json_apply_includes_a_zero_summary | none | ok |
| `clean caches` moves to Trash by default; `--shred` makes it permanent | R Principles | none | cli.rs:1899 shred_is_explicit_in_preview (preview only); apply only with `--shred` (cli.rs:2386) | none | partial: Trash apply unproven (cross-cutting 1) |
| VS Code logs, `Application Support/Code`, and `~/Library/Logs` are "not covered at all" | R | safety.rs:2265 ("Library/Application Support" stays protected) | none | none | no black-box |

## clean node-modules

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "exact paths in Git repos with no recent activity" (stale scan finds the install) | R Usage, M | ops/node_modules.rs:590 prunes_dependency_and_git_trees | cli.rs:410 scan_leads_with_categories_and_lists_the_largest_until_all; cli.rs:588 config_tilde_root_is_expanded | none | ok |
| activity = newer of HEAD commit date and newest reflog entry, in UTC | R, A | ops/project.rs:1128, ops/project.rs:1146, ops/project.rs:1235 a_repository_without_reflogs_is_judged_by_its_commit_date, ops/project.rs:1186 activity_dates_are_read_in_utc_like_the_cutoff | none (cli.rs uses canned dates) | PV git/reflog-activity, PV git/head-commit, PV git/utc-dates | no black-box |
| Git queries "run nothing" a hostile `.git/config` names (signature program, lazy fetch) | R Principles, A | ops/project.rs:1049 activity_probe_never_runs_a_repository_configured_signature_program; ops/project.rs:1088 activity_probe_never_lazily_fetches_through_a_repository_configured_transport | cli.rs:1266 (ambient pathspec settings only) | PV git/signature-program, PV git/lazy-fetch-transport | partial: hooks and fsmonitor overrides have no test at all |
| unknown Git activity is not stale: query failure blocks only that repository | R Principles, A | ops/project.rs:985 git_failure_is_not_stale; ops/node_modules.rs:1375 a_failing_repository_blocks_only_its_own_findings | cli.rs:1471 project_git_probe_failure_blocks_only_its_own_repository | PV node_modules/repository-contained | ok |
| a failure after the owner is known blocks the whole repository | A | ops/node_modules.rs:1436 a_failure_after_the_owner_is_known_blocks_the_whole_repository | none | PV node_modules/owner-veto | no black-box |
| a folder the walk cannot read is reported as an error and nothing in it is offered | R Principles | ops/node_modules.rs:1312 an_unreadable_folder_is_reported_and_blocks_only_itself | cli.rs:1595 project_walk_errors_block_only_the_unreadable_folder | PV node_modules/unread-folders-reported | ok |
| liveness: refuses a repo that is the working directory of a running build | R Liveness, M | ops/node_modules.rs:965 active_build_cwd_skips_repo_and_refuses_apply; ops/project.rs:1245 repo_owns_equal_and_descendant_process_cwds_only | none (stub pgrep never reports a build) | PV liveness/lsof-escape, PV liveness/lsof-unreported-running, PV liveness/lsof-successor | no black-box |
| failed liveness probe fails the whole category, nonzero | R Fail closed | safety.rs:2486 lsof_exit_one_passes_only_when_every_unreported_process_is_gone | cli.rs:1449 project_cleanup_probe_failures_are_nonzero_json_errors | PV liveness/lsof-unreported-running | ok |
| apply recheck: repo "became active after preview" is refused | R Immutable plans, A | ops/node_modules.rs:965 | cli.rs:1994 node_modules_apply_refuses_repo_that_became_active | none | ok |
| apply preflights every finding before changing anything; a failed probe stops the whole plan untouched | R JSON, A | ops/node_modules.rs:917 apply_preflights_entire_batch_before_deleting_valid_targets | cli.rs:2029 project_apply_preflights_all_repo_probes_before_mutating | none | partial: no planted case for the preflight (only for continue-past-refusal) |
| then "continue past a finding the sink refuses", recording each failure | R JSON, A | ops/node_modules.rs:1074 a_refused_node_modules_does_not_block_the_rest_of_the_plan | none | PV node_modules/continue-past-refusal | no black-box |
| apply reasserts a real `node_modules` leaf: no symlink, symlinked ancestor/owner, `.git`, nested dependency tree, non-normal component | R Principles, A | ops/node_modules.rs:618 apply_target_shape_matches_scanner_authority; ops/node_modules.rs:769 apply_rejects_node_modules_with_symlinked_ancestor_before_deletion; ops/node_modules.rs:809 apply_rejects_node_modules_with_symlinked_owner_before_deletion; ops/node_modules.rs:847 apply_rejects_nearest_owner_below_outer_node_modules | cli.rs:1527 (target with its own `.git` marker) | none | partial: symlink and ancestor refusals have no black-box and no planted case |
| apply refuses a target with no Git owner | A | ops/node_modules.rs:674 apply_rejects_target_without_git_owner | none | none | no black-box |
| "a node_modules holding any file its repository tracks ... never offered", rechecked at apply | R, A | ops/node_modules.rs:1015 a_committed_node_modules_is_never_offered_or_removed | cli.rs:1082 | PV node_modules/tracked-apply, PV project/tracked-files, PV project/tracked-case-rename, PV project/tracked-unicode-case, PV project/tracked-ascii-alias | ok |
| an install needs `package.json` beside it (React Native `.app/assets/node_modules`, corepack `dist/node_modules` are not installs) | R, A | ops/node_modules.rs:1251 a_node_modules_without_a_manifest_beside_it_is_not_an_install | none (every fixture writes a manifest) | PV node_modules/manifest-scan, PV node_modules/manifest-apply | no black-box |
| a `node_modules` inside build output belongs to that output; never a finding; apply refuses | R, A | ops/node_modules.rs:1111 a_node_modules_inside_build_output_belongs_to_that_output | none | PV node_modules/build-output-walk, PV node_modules/build-output-scan, PV node_modules/build-output-apply | no black-box |
| linked worktree with a missing gitdir, and a repository with no commit yet, are skipped and named | R | ops/node_modules.rs:1190 an_orphaned_worktree_is_skipped_instead_of_failing_the_scan; ops/node_modules.rs:1341 a_repository_without_commits_is_skipped_and_named; ops/project.rs:681 an_unborn_branch_is_recognized_and_a_dangling_head_is_not; ops/project.rs:834 only_a_listing_without_a_commit_object_proves_no_history; ops/project.rs:860 orphaned_worktrees_are_judged_by_their_gitdir_bytes | none | PV project/unborn-branch, PV project/unborn-dangling, PV project/unborn-broken-refs, PV project/unborn-no-commit, PV project/unborn-complete-listing | no black-box |
| skip notes name the window and the repositories held | R | ops/node_modules.rs:1477 skip_notes_name_the_listed_repositories_and_the_window | none | none | no black-box |
| overlapping roots do not duplicate findings; two names for one folder scan once | R Config | ops/project.rs:954 normalizes_duplicate_and_descendant_roots | cli.rs:1559 overlapping_project_roots_do_not_duplicate_findings | none | ok |
| apply removes a valid stale install and leaves `.git` | R | ops/node_modules.rs:885 apply_deletes_valid_stale_nested_node_modules | cli.rs:1406 (through purge, permanent); tui.py:348 verify_purge | none | partial: Trash mode unproven |
| `active_days` window decides staleness | R Config | safety.rs:1984 retention_defaults_to_at_least_thirty_days | cli.rs:588 (sets active_days = 30 only) | none | partial: no test varies the window or tests the day boundary |

## clean artifacts

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "deletes a directory only when its name is on a closed list and its ecosystem corroborates it" (`target` beside `Cargo.toml`, `.venv` with `pyvenv.cfg`, `Pods` beside `Podfile`, ...) | R, A, M | ops/artifacts.rs:732 corroboration_matrix_is_fail_closed; ops/artifacts.rs:795 walker_prunes_git_node_modules_and_matched_artifacts | cli.rs:664 artifacts_target_scans_corroborated_stale_repo (`target` only, preview) | none | partial: black-box covers one ecosystem and no apply |
| `.cxx`, `.terragrunt-cache`, `.output` only beside their owner's file | R, A | ops/artifacts.rs:1391 newer_names_are_offered_only_beside_their_owners_file | none | PV artifacts/newer-names | no black-box |
| only a regular, valid `CACHEDIR.TAG` marks build output | A | ops/artifacts.rs:991 only_a_regular_cachedir_tag_marks_build_output | none | none | no black-box |
| ambiguous names (`build`, `dist`, `vendor`, `bin`, `obj`) never matched | R, A | ops/artifacts.rs:732 | none | none | no black-box |
| corroboration "re-verified at apply"; orphans refused | A | ops/artifacts.rs:877 apply_refuses_removed_corroboration_and_orphans | none | none | no black-box |
| refuses artifacts below every ASCII-case variant of `node_modules` | R | ops/artifacts.rs:834 apply_rejects_artifact_under_case_variant_node_modules_ancestor | none | none | no black-box |
| tracked files: never offered, incl. case, Unicode-case and ASCII-alias renames on APFS | R, A | ops/artifacts.rs:1018 a_tree_its_repository_tracks_is_never_offered_or_removed; ops/artifacts.rs:1072 a_tracked_tree_renamed_only_in_case_is_never_offered; ops/artifacts.rs:1105 a_tracked_tree_renamed_only_in_unicode_case_is_never_offered; ops/artifacts.rs:1144 a_tracked_tree_renamed_to_an_ascii_alias_is_never_offered | cli.rs:1082 (plain case only) | PV project/tracked-files, PV project/tracked-case-rename, PV project/tracked-unicode-case, PV project/tracked-ascii-alias, PV artifacts/tracked-apply | partial: renamed-spelling cases are unit only |
| a tree holding a Git repository below its root is never offered; rechecked at apply | R, A | ops/artifacts.rs:1228 a_tree_holding_a_git_repository_is_never_offered_or_removed | cli.rs:1527 (marker at root only) | PV artifacts/nested-repository-scan, PV artifacts/nested-repository-apply | partial: see purge row |
| a tree holding `*-keypair.json` never offered, name matched as the volume folds it (Kelvin, long s) | R, A | ops/artifacts.rs:1173 a_tree_holding_a_program_keypair_is_never_offered_or_removed; ops/artifacts.rs:1539 program_keypair_names_match_the_solana_suffix_as_the_volume_does | cli.rs:1234 (plain name through purge) | PV artifacts/keypair-scan, PV artifacts/keypair-apply, PV artifacts/keypair-alias | ok |
| a tree holding Terraform state (`*.tfstate`) never offered, scan and apply | R, A | ops/artifacts.rs:1327 a_tree_holding_terraform_state_is_never_offered_or_removed | none | PV artifacts/terraform-state-scan, PV artifacts/terraform-state-apply | no black-box |
| liveness: a repo owning a running build is refused at scan and apply | R | ops/artifacts.rs:927 active_build_cwd_skips_repo_and_refuses_apply | none | none | no black-box |
| probe failure is a nonzero JSON error | R | none | cli.rs:1449 | none | ok |
| apply preflights every finding; a failed recheck stops the plan untouched | R JSON, A | none | cli.rs:2075 artifact_apply_preflights_all_repo_probes_before_mutating | none | partial: no unit counterpart and no planted case |
| continues past a finding the sink refuses | R JSON, A | ops/artifacts.rs:1285 a_refused_artifact_does_not_block_the_rest_of_the_plan | none | PV artifacts/continue-past-refusal | no black-box |
| repository whose tree check fails offers nothing, reported as one error | R Fail closed, A | ops/artifacts.rs:1432 an_unreadable_tree_refuses_its_repository_rather_than_offers | cli.rs:1471, cli.rs:1595 | PV artifacts/repository-contained, PV artifacts/unread-folders-reported | ok |
| repository with no commit yet is skipped and named | R | ops/artifacts.rs:1505 a_repository_without_commits_is_skipped_and_named | none | PV project/unborn-no-commit | no black-box |
| a target holding its own `.git` marker is rejected | A | safety.rs:2042 deletion_validation_refuses_git_repository_and_worktree_roots | cli.rs:1527 | none | ok |

## clean simulators

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "delete exact previewed unavailable devices"; one finding per device, UDID bound | R Usage, M | ops/simulators.rs:402 unavailable_device_finding_uses_measured_size_and_escalated_danger; ops/simulators.rs:658 preview_never_authorizes_a_broad_simulator_delete | cli.rs:1948 unavailable_simulator_json_authorizes_only_the_previewed_device (argv log and journal) | none | ok |
| "simulator cleanup accepts only the previewed device UDID"; forged `erase all` or `delete unavailable` refused | R Typed command boundary, M | ops/simulators.rs:326 rejects_forged_actions_without_authority; ops/simulators.rs:524 a_mixed_plan_skips_the_disclosure_and_refuses_only_the_forgery | cli.rs:1948 (asserts no `erase all`, no `delete unavailable`) | none | ok |
| apply "rechecks that it is still unavailable" (refuses an available or vanished device) | R, M | none (no test supplies a second `simctl list` answer) | none (stub returns the same list) | none | none |
| working simulators are one report-only finding with total size and the three largest | R, A, M | ops/simulators.rs:429 available_simulator_data_is_disclosed_but_never_actionable; ops/simulators.rs:482 apply_skips_the_report_only_disclosure_instead_of_refusing_it; ops/simulators.rs:568 an_overflowing_size_total_drops_only_the_disclosure | none | none | no black-box |
| a missing or changed size field drops only the disclosure, never zero | A | ops/simulators.rs:596 a_changed_size_field_drops_only_the_disclosure; ops/simulators.rs:619 simulator_disclosure_is_omitted_when_any_size_is_unreported | none | none | no black-box |
| `clean simulators` rejects `--shred` | R, A, M | none | cli.rs:197 mutation_flags_remain_available_only_where_they_have_meaning | none | ok |
| malformed device id is an error; `xcrun` absent is "no findings" only for not-found | A | ops/simulators.rs:643 malformed_simulator_device_id_is_an_error; ops/simulators.rs:370 optional_probe_only_treats_not_found_as_absent | none | none | no black-box |
| the apply is journaled as the exact argv | R Journal | none | cli.rs:1948 (records[0].argv) | none | ok |

## clean xcode

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "exact DeviceSupport/DerivedData child directories"; never a file or symlink beside them | R, A, M | ops/xcode.rs:592 scan_offers_only_real_directories_as_xcode_support_children; ops/xcode.rs:629 apply_refuses_a_non_directory_xcode_support_child; ops/xcode.rs:775 direct_device_support_child_can_be_applied | cli.rs:1154 clean_xcode_cleans_derived_data_around_package_checkouts (directories only) | PV xcode/non-directory-target | partial: file and symlink refusal have no black-box |
| a forged nested DerivedData target is refused before liveness | A | ops/xcode.rs:1021 forged_nested_derived_data_target_is_rejected_before_liveness | none | none | no black-box |
| "Xcode Archives are never pruned"; visible but never actionable | R Principles, A, M | ops/xcode.rs:691 archives_are_never_applied_or_counted; ops/xcode.rs:732 forged_actionable_archive_is_rejected | none | none | no black-box |
| a DerivedData folder holding package checkouts is offered as its directories except `SourcePackages` | R, A | ops/xcode.rs:880 a_derived_data_folder_holding_package_checkouts_is_cleaned_around_them; ops/xcode.rs:982 apply_never_removes_the_package_checkouts | cli.rs:1154 (checkout `.git` survives, three folders removed) | PV xcode/package-checkouts-offered, PV xcode/package-checkouts-kept | ok |
| a DerivedData folder that is itself a repository is never split | A | ops/xcode.rs:936 a_derived_data_folder_that_is_a_repository_is_never_split | none | PV xcode/repository-folder-scan, PV xcode/repository-folder-apply | no black-box |
| while Xcode runs, only DerivedData untouched for the activity window, with no Xcode-held file, is offered; apply judges each again | R Liveness, M | ops/xcode.rs:542 scan_skips_derived_data_when_an_xcode_build_is_running_or_unknown; ops/xcode.rs:1077 while_xcode_runs_only_old_closed_derived_data_is_offered; ops/xcode.rs:1402 derived_data_apply_refuses_a_running_or_unknown_xcode_build | none (stub pgrep always reports no Xcode) | PV xcode/active-folders, PV xcode/active-apply, PV xcode/active-empty-folder | no black-box |
| open files matched by the folder's real path, device/inode identity, every `DerivedData` component, and the mount fallback name | A | ops/xcode.rs:1188 an_open_file_is_matched_under_the_folders_real_path; ops/xcode.rs:1221 an_open_file_is_matched_by_the_folders_identity; ops/xcode.rs:1267 the_identity_match_looks_past_an_earlier_derived_data_component; ops/xcode.rs:1306 a_mount_fallback_name_on_the_folders_volume_refuses_the_folder; safety.rs:2699 xcode_open_files_are_absolute_names_from_a_complete_listing | none | PV xcode/active-canonical, PV xcode/active-identity, PV xcode/active-identity-every-component, PV xcode/active-mount-fallback, PV liveness/xcode-open-files | no black-box |
| a folder whose use cannot be judged blocks only itself | A | ops/xcode.rs:1367 an_unjudgeable_derived_data_folder_blocks_only_itself | none | none | no black-box |
| apply "continues past a refused finding"; run still nonzero | R JSON, A | ops/xcode.rs:807 a_refused_derived_data_folder_does_not_block_the_rest_of_the_plan | none | PV xcode/continue-past-refusal | no black-box |
| "DerivedData liveness covers Xcode itself and the SWBBuildService/XCBBuildService processes" | R, A | safety.rs:2628 liveness_pgrep_matches_devtrims_own_ancestors | cli.rs:2769 (pgrep pattern line in the spawn log) | none | partial: the process-name list is asserted only as an argv string |

## clean docker

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "image prune -a + builder prune on the exact previewed local Unix-socket endpoint" | R Usage, M | ops/docker.rs:702 docker_context_must_resolve_to_an_absolute_local_socket | cli.rs:2145 failed_docker_prune_is_nonzero_with_truthful_zero_summary (journal argv `--host unix://... image prune -a -f`); cli.rs:2211 human_apply_prints_data_loss_warning_before_action (success, argv not logged) | none | partial: `builder prune` is never run by a black-box test |
| remote Docker contexts refused before any prune capability exists | R, M | ops/docker.rs:702 | cli.rs:2122 docker_remote_context_is_rejected_before_any_prune_capability_is_created | none | ok |
| "Docker volumes are never pruned" | R Principles, A, M | ops/docker.rs:530 rejects_forged_actions_without_authority (forged `volume prune` refused) | none (no test asserts the success-path argv holds no `volume`) | none | no black-box |
| forged command or path-less action is refused | R Typed command boundary | ops/docker.rs:530 | none | none | no black-box |
| `clean docker` rejects `--shred` | R, A, M | none | cli.rs:197 | none | ok |
| failed prune: nonzero, truthful zero summary, exit status and stderr in the error | A | none | cli.rs:2145; cli.rs:2197 failed_human_apply_does_not_print_a_success_summary | none | ok |
| host VM disk image (OrbStack, Docker Desktop) reported, never actionable, measured in allocated blocks | R, A, M | ops/docker.rs:431 allocated_bytes_measures_allocation_not_logical_length; ops/docker.rs:467 vm_disk_image_is_disclosed_but_never_actionable; ops/docker.rs:500 apply_skips_the_report_only_vm_disk_finding_instead_of_refusing_it | none | none | no black-box |
| image finding emitted even when `docker` is not installed | R, A | ops/docker.rs:582 absent_vm_disk_image_yields_no_finding (the absent case); ops/docker.rs:616 optional_probe_only_treats_not_found_as_absent | none | none | partial: the present-image-without-docker case has no test |
| installed `docker` with a down daemon fails the category and the error names the image and its size | R, A | ops/docker.rs:616 (not-found versus nonzero only) | none | none | none |
| build-cache estimate: "up to" when ACTIVE is 0, "at least" the reclaimable figure otherwise; only that row parses ACTIVE | R, A | ops/docker.rs:673 build_cache_estimate_matches_what_prune_all_removes; ops/docker.rs:659 an_unused_row_cannot_fail_the_category | none | none | no black-box |
| sizes parse strictly; malformed `system df` fails | A | ops/docker.rs:591 parses_docker_sizes; ops/docker.rs:601 rejects_ambiguous_or_out_of_range_docker_sizes; ops/docker.rs:648 rejects_malformed_docker_system_df_output; fuzz/fuzz_targets/docker_size.rs | none | none | no black-box |

## clean toolchains

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "only unreferenced swift.org toolchains": direct `.xctoolchain` directories not referenced by any verified symlink | R Usage, M | ops/toolchains.rs:256 preserves_every_symlink_target; ops/toolchains.rs:274 apply_accepts_only_direct_unreferenced_toolchain_directories | none | none | no black-box |
| "broken toolchain links" and a missing or invalid `swift-latest` block the scan | R Fail closed, M | ops/toolchains.rs:242 missing_or_broken_latest_fails_closed | cli.rs:639 broken_swift_latest_is_a_nonzero_json_error | none | ok |
| apply reasserts the exact direct-child shape; forged nested target refused | R Category authority, A | ops/toolchains.rs:307 apply_rejects_forged_nested_toolchain_target | none | none | no black-box |
| `clean toolchains` apply moves to Trash / removes the old toolchain | R Usage | none | none | none | none |
| Xcode's built-in chain unaffected | M | none | none | none | none |

## clean installers

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "stale installer archives": direct children of Downloads and Desktop with extension `dmg pkg mpkg iso xip`, ASCII-case-insensitive | R, A, M | ops/installers.rs:192 finds_stale_installers_and_skips_recent_and_foreign_extensions | none | none | no black-box |
| "untouched for longer than the configured retention window (`retain_days`)" | R, A | ops/installers.rs:235 installers_follow_the_retention_window_not_the_project_window | none | PV installers/retention-window | no black-box |
| M states the gate is "the active window" | M line 560 | ops/installers.rs:235 asserts `retain_days` | none | PV installers/retention-window | contradicted: MANUAL.html:560 says active window; README, AGENTS and the test say `retain_days` |
| scanning is non-recursive (bundled installers in extracted trees left alone) | R, A, M | ops/installers.rs:348 nested_installers_are_not_scanned | none | none | no black-box |
| `zip`, `tar` and other formats that can carry user data are never matched | R, A | ops/installers.rs:192 | none | none | no black-box |
| apply reasserts the whole shape: symlinks and targets outside the two directories refused | R, A | ops/installers.rs:274 apply_refuses_a_target_outside_the_authorized_directories; ops/installers.rs:325 a_symlinked_installer_is_refused | none | none | no black-box |
| an installer touched after preview is refused at apply | A | ops/installers.rs:301 an_installer_that_stopped_being_stale_after_preview_is_refused | none | none | no black-box |
| the preview shows the age it judged | R | none | none | none | none |
| parse error keeps the category in JSON | A | none | cli.rs:2756 installers_parse_errors_retain_the_category_in_json | none | ok |

## clean agents

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| regenerable caches offered unconditionally; authentication, config, memories, skills, plugins, `.claude.json` backups never | R, A | ops/agents.rs:1903 regenerable_caches_are_offered_and_credentials_are_not | cli.rs:2418 agent_cleanup_removes_only_stale_history_and_regenerable_caches | PV evidence/agents-regenerable | ok |
| session history offered only once the newest regular file in its subtree is older than `retain_days` | R, A | ops/agents.rs:1943 history_is_offered_only_after_the_active_window; ops/agents.rs:1982 history_follows_the_retention_window_not_the_project_window | cli.rs:2418 (stale session removed, live session survives) | PV agents/retention-window, PV evidence/agents-history | ok |
| shell snapshots are history, not cache: age-gated | R, A | ops/agents.rs:2182 shell_snapshots_are_age_gated_rather_than_offered_outright | cli.rs:2418 | none | ok |
| `~/.claude/projects` is never a root; retired Claude trees can never return | R, A | ops/agents.rs:2020 the_claude_projects_tree_is_never_a_candidate; ops/agents.rs:2082 the_retired_claude_trees_can_never_become_roots_again | cli.rs:2418 (not asserted) | PV agents/apply-namespace | no black-box |
| apply reasserts tier, exact depth, no symlink, and re-reads the age gate | R, A | ops/agents.rs:2266 apply_refuses_forged_targets_and_preserves_them; ops/agents.rs:2328 history_that_stopped_being_stale_after_preview_is_refused; ops/agents.rs:2353 a_symlinked_history_child_is_refused | none | PV agents/history-symlink | no black-box |
| "a session resumed after preview falls out of the plan"; apply continues past that finding | R, A | ops/agents.rs:2213 a_resumed_session_does_not_block_the_rest_of_the_plan | none | none (agents is not in the planted continue-past set) | no black-box |
| older Codex releases only: layout-v1 package, installer lock, `current` verified, canonical versions, manifest digests | R, A | ops/agents.rs:1131 codex_standalone_offers_only_an_obsolete_release; ops/agents.rs:1178 codex_releases_fail_closed_without_a_valid_current_link_or_install_lock; ops/agents.rs:1365 codex_release_scan_rejects_unknown_contents_and_symlinked_candidates; ops/agents.rs:1812 a_tampered_main_executable_is_not_a_vendor_package; ops/agents.rs:1833 codex_versions_must_be_canonical; fuzz/fuzz_targets/probe_parsers.rs | cli.rs:2418 (current, newer, unknown and extra-resource releases survive; old one removed) | PV agents/codex-current-executable | ok |
| a release a process still executes is neither offered nor removed; apply re-probes each release | R, A | ops/agents.rs:1541 a_release_a_process_still_executes_is_neither_offered_nor_removed; ops/agents.rs:1600 apply_rechecks_liveness_at_each_release_not_once_per_plan; ops/agents.rs:1764 a_release_a_real_process_executes_is_refused_through_lsof; ops/agents.rs:1628 a_failed_mapping_probe_refuses_release_preview_and_apply | cli.rs:2418 (running and unprobed previews) | PV agents/codex-running-release, PV liveness/lsof-mapping-names | ok |
| a busy installer refuses only release findings; version promoted after preview refused | A | ops/agents.rs:1284 codex_installer_lock_blocks_release_preview; ops/agents.rs:1323 busy_codex_installer_refuses_only_release_apply; ops/agents.rs:1411 codex_release_apply_refuses_a_version_promoted_after_preview | cli.rs:2418 (blocked preview, nonzero, caches still listed) | none | partial: apply-time busy and promotion cases are unit only |
| a FIFO at a vendor file name refuses instead of hanging | A | ops/agents.rs:1777 a_fifo_at_a_vendor_file_name_refuses_instead_of_hanging | none | none | no black-box |
| every `agents` entry carries evidence (regenerable, history, Codex releases) | A | ops/agents.rs:1880 every_agent_entry_carries_evidence | none | PV evidence/agents-regenerable, PV evidence/agents-history, PV evidence/agents-codex-releases | partial: presence checked, correctness is review only |
| cache note discloses active-session interruption; history note says content does not come back | R | none | cli.rs:2418 (cache note asserted near line 2689) | none | partial: history note wording unasserted |
| the release authority names the installer root | A | ops/agents.rs:1862 the_release_authority_names_the_installer_root | none | none | no black-box |

## clean leftovers

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "report-only hints; never deletes worktrees" | R Usage, A, M | ops/leftovers.rs:111 scratch_pattern_is_only_a_hint | cli.rs:160 read_only_commands_reject_mutation_flags (`clean leftovers --apply` rejected) | none | partial: no test asserts the findings are non-actionable |
| lists possible agent scratch and `.supergoal` paths | M | ops/leftovers.rs:111 | none | none | no black-box |
| walk errors are not silently flattened (nonzero JSON) | A | none | cli.rs:1638 leftovers_walk_errors_are_not_silently_flattened | none | ok |
| scans only the scan roots (default project folders) | R | safety.rs:1909 | cli.rs:716 (largest, not leftovers) | none | partial: no leftovers-specific roots case |

## icloud

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "large iCloud Drive files and local allocation"; files at or above 100 MB, recursive | R Usage, M | ops/icloud.rs:78 missing_file_measurement_fails_closed | cli.rs:1668 icloud_recursively_reports_large_files_without_inferring_upload_status (exactly 100 MiB) | none | partial: no below-threshold negative control |
| does "not indicate iCloud upload status" | M | none | cli.rs:1668 (note text) | none | ok |
| traversal errors are nonzero JSON errors; unreadable subtree yields no findings | R Fail closed | none | cli.rs:1699 icloud_traversal_errors_are_nonzero_json_errors | none | ok |
| rejects mutation flags | R Usage | none | cli.rs:160 (`icloud --yolo`) | none | ok |
| symlinks not followed | (implicit in code `follow_links(false)`) | none | none | none | none |

## trash-empty (+ --only-devtrim)

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "Every mutation, including trash-empty, requires --apply" | R Principles | none | cli.rs:1741 trash_empty_requires_apply | none | ok |
| previews each current top-level Trash item as an exact target | R, M | ops/mod.rs:2258 purge_trash_consumes_only_exact_previewed_children | cli.rs:807 trash_empty_only_devtrim_leaves_everyone_elses_items (both items listed) | PV sink/trash-grant-root | ok |
| `--confirm=<gb>` required and measured over the exact set, not the whole Trash | R, A | safety.rs:2340 trash_acknowledgment_measures_the_plan_not_the_whole_trash | cli.rs:1753 trash_empty_yolo_still_requires_size_acknowledgment | none | partial: the plus-or-minus 2 GB tolerance (`trash_gate` at safety.rs:1243) is never tested at its edge |
| `--yolo` skips consent but never the size acknowledgment; `-y` alone is not enough for a danger-9 purge | R, A, M | safety.rs:2376 yes_does_not_bypass_critical_typed_confirmation | cli.rs:1772 trash_empty_apply_confirms_like_every_other_mutation (positive control with `--yolo`) | none | ok |
| the exact set is shown again at `--apply` before approval | R | none | none (human output at apply is not asserted) | none | none |
| anything moved to Trash after confirmation remains | R | ops/mod.rs:2258 | none | none | no black-box |
| `trash-empty` rejects `--shred` ("its preview is already permanent") | R Usage, A, M | none | none (cli.rs:197 covers docker and simulators only) | none | none |
| a direct item named as an ASCII-case variant of `.git` is warned about and left | R, A | ops/mod.rs:2281 trash_preview_skips_git_metadata_item_without_blocking_other_items | none | PV trash/continue-past-refusal | no black-box |
| apply continues past an item the sink refuses (a trashed project still holding its repository) | R JSON, A | ops/mod.rs:2339 a_refused_trash_item_does_not_block_the_rest_of_the_purge; ops/mod.rs:2381 trash_empty_purges_a_trashed_uv_cache_but_not_a_trashed_repository | none | PV trash/continue-past-refusal | no black-box |
| a forged target outside the Trash is refused; symlinked Trash root refused | A | ops/mod.rs:2431 purge_trash_rejects_a_forged_target_outside_trash; ops/mod.rs:2241 purge_trash_refuses_symlinked_root_and_preserves_sentinel; safety.rs:2329 refuses_symlinked_trash | none | none | no black-box |
| `--only-devtrim`: only items matching a successful journaled `trash` move (device, inode, birth time) are offered | R, A | ops/mod.rs:1698 only_items_devtrim_moved_to_the_trash_are_offered; journal.rs:1027 a_trash_record_carries_the_moved_items_identity_and_nothing_else_may | cli.rs:807 (hand-written journal with identity) | PV trash/only-devtrim-identity | partial: real move-then-purge round trip untested (cross-cutting 1) |
| the sink journals each Trash move's identity | R, A | ops/mod.rs:1767 the_sink_journals_the_identity_of_what_it_moves_to_the_trash | none | PV trash/journal-identity | no black-box |
| unreadable journal refuses the narrowed purge and offers nothing | R, A | ops/mod.rs:1698 | cli.rs:807 (message tag `PV-E2E` at line 874, unwired) | PV trash/incomplete-history | ok |
| only `trash-empty` takes `--only-devtrim` | A | none | cli.rs:807 (`scan --only-devtrim` exits 2) | none | ok |
| a Trash-held uv cache can be purged under the Trash marker grant, a repository cannot | R, A | ops/mod.rs:1800 the_trash_grant_covers_only_items_directly_in_the_trash; ops/mod.rs:2381 | none | PV sink/trash-grant-root | no black-box |
| permanent purge can delete a FIFO | A | ops/mod.rs:2411 permanent_trash_purge_deletes_a_fifo | none | none | no black-box |
| purge of a plain Trash item actually removes it | R Usage | ops/mod.rs:2258 | cli.rs:807 (applied, `--only-devtrim`); cli.rs:1772 (sentinel removed by `--yolo`) | none | ok |

## history / journal

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "recent journaled applies"; `--json` is one document with `entries` and `errors` | R, M | journal.rs:1438 history_returns_newest_results_and_orphans_first_with_limit | cli.rs:2233 history_json_is_one_document_and_skips_malformed_lines | none | ok |
| `--limit N` bounds output | M | journal.rs:1438 | none (`--limit` is never passed) | none | partial: black-box lacks the flag |
| missing history is empty and successful | R | none | cli.rs:2259 missing_history_is_empty_and_successful | none | ok |
| an attempt with no result is "interrupted" | R, M | journal.rs:1373 orphan_attempt_is_interrupted_and_malformed_lines_are_aggregated | cli.rs:2272 history_human_marks_orphan_attempt_interrupted | none | ok |
| unreadable or partial history: errors AND nonzero | A | journal.rs:1136 history_rejects_an_oversized_line | cli.rs:2289 unreadable_history_is_one_error_document_and_nonzero; cli.rs:2233 | none | ok |
| `$XDG_STATE_HOME` honored only when absolute | R | none | cli.rs:2304 history_uses_only_an_absolute_xdg_state_home | none | ok |
| history is read-only: creates no lock file, waits for guarded applies | R | journal.rs:1303 history_does_not_create_a_lock_file; journal.rs:1321 history_waits_for_an_active_attempt_before_snapshotting | none | none | no black-box |
| bounded tail scan (100k lines, tiny malformed lines cannot force unbounded work) | R | journal.rs:1151 history_ignores_more_than_100k_old_lines_when_the_tail_satisfies_the_limit; journal.rs:1170 malformed_tiny_lines_cannot_force_an_unbounded_tail_scan; journal.rs:1106 history_reads_the_newest_entry_from_each_oversized_sparse_generation | none | none | no black-box |
| legacy records pair across generations | R | journal.rs:1080 legacy_duplicate_results_pair_with_the_nearest_attempt; journal.rs:1531 history_pairs_records_across_rotation_generations | none | none | no black-box |
| write-ahead: attempt before deletion, result after, unwritable journal blocks apply | R, A | ops/mod.rs:2503 journal_records_refused_deletion_as_error; ops/mod.rs:2539 journal_write_failure_aborts_before_deletion; journal.rs:1393 finish_keeps_successful_result_when_journal_write_fails | cli.rs:1948 (attempt and ok result), cli.rs:2145 (attempt and error result), cli.rs:1406 | none | partial: unwritable-journal block has no black-box |
| symlinked journal path components refused | R | journal.rs:1184 journal_paths_never_follow_symlinks; journal.rs:1260 journal_parent_components_never_follow_symlinks | none | none | no black-box |
| rotation (10 MiB, keep 3) once, never mid-pair, never truncation | R, A | journal.rs:1463 rotation_shifts_files_and_clobbers_the_oldest_generation; journal.rs:1491 held_lock_skips_rotation_with_a_warning; journal.rs:1512 persistent_unlocked_lock_file_does_not_block_rotation; journal.rs:1549 attempt_guard_keeps_a_pair_in_one_rotation_generation | cli.rs:508 oversized_journal_rotates_once_when_context_opens | none | ok |
| concurrent writers lose no record; a tail without newline is terminated | A | journal.rs:1659 concurrent_writers_preserve_every_record; journal.rs:1229 a_record_after_a_truncated_line_starts_on_its_own_line | none | none | no black-box |
| `history` and `completions` do not depend on config (a malformed config cannot block them) | (comment in app.rs run) | none | none (cli.rs:539 uses `scan`) | none | none |

## analyze

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "interactive read-only disk explorer (never deletes)" | R, A, M | analyze.rs:884 a_file_is_never_descended_into | cli.rs:160 (`analyze --apply/--shred/--yolo/-y` rejected); read-only-views.py:116 exercise | none | partial: nothing asserts the tree is untouched |
| `analyze <dir> --json` one-shot breakdown | R Usage | analyze.rs:768 measures_each_child_and_ranks_largest_first | none | none | no black-box |
| symlinks reported at their own size, not followed; swapped-for-symlink refused on descend | R, A | analyze.rs:794 a_symlinked_child_is_not_followed; analyze.rs:900 descend_refuses_an_entry_swapped_for_a_symlink_after_listing; analyze.rs:993 resolve_root_refuses_a_symlink_and_a_file | none | none | no black-box |
| a different device is never measured or entered | R, A | analyze.rs:941 descend_refuses_a_directory_on_another_device | none | none | no black-box |
| unreadable subtrees disclosed as `(partial)` lower bounds; JSON lists each in `errors` and exits nonzero | R, A | analyze.rs:972 lower_bounds_are_reported_as_errors | none (exit status unasserted anywhere) | none | none |
| measures on a worker thread; cancel on leaving a directory; UI never blocks | R | analyze.rs:822 cancellation_stops_measurement | read-only-views.py:116 exercise (60-row and 2048-entry directories, quit latency recorded) | none | partial: the script returns latencies and asserts only that views appear and quit |
| navigation never leaves the starting root | A | analyze.rs:872 navigation_never_leaves_the_starting_root | none | none | no black-box |
| long list keeps selection visible after resize; bar proportional; cursor follows late reorder | R | analyze.rs:732 selected_entry_stays_visible_in_a_long_list_after_resize; analyze.rs:836 bar_is_proportional_and_bounded; analyze.rs:847 the_cursor_follows_its_entry_when_late_results_reorder_the_list | read-only-views.py:116 | none | ok |

## status

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "read-only machine vitals and a health score" through fixed-argv tools with fail-closed parsers | R, A, M | status.rs:1146 parses_load_average; status.rs:1154 load_average_fails_closed_on_malformed_input; status.rs:1213 vm_stat_fails_closed_on_malformed_input; status.rs:1244 df_fails_closed_on_malformed_input; status.rs:1322 netstat_fails_closed_without_link_rows | cli.rs:1835 status_reads_system_tools_in_the_c_locale | none | partial: black-box checks one field |
| tools run in the C locale (pt_BR decimal comma) | M, A | none | cli.rs:1835 | none | ok |
| "Memory used is active + wired + compressed" | R, A | status.rs:1188 vm_stat_uses_the_declared_page_size | none | none | no black-box |
| an unreadable metric is `null` with a reason in `unavailable`, never zero | R JSON, A | status.rs:1251 parses_battery_and_tolerates_a_machine_without_one; status.rs:1489 a_failed_battery_probe_is_not_reported_as_having_no_battery | none | none | no black-box |
| the process exits nonzero whenever `unavailable` is not empty | R JSON | none | read-only-views.py:116 exercise (`status --watch` with empty PATH exits 1; not `--json`) | none | partial: the one-shot and JSON exit status are unasserted |
| health score names every input it could not read | R, A | status.rs:1378 health_names_every_missing_input_instead_of_scoring_over_gaps; status.rs:1387 health_deducts_for_real_pressure; status.rs:1419 health_bands_are_distinguishable_without_any_colour | none | none | no black-box |
| netstat rows indexed from the end, exact keys, undocumented width refused | A | status.rs:1306 netstat_counts_each_interface_once_across_both_link_row_widths; status.rs:1328 netstat_refuses_a_link_row_of_undocumented_width_instead_of_skipping_it | none | none | no black-box |
| `status --watch` is a live dashboard, rejects `--json`, quits on `q` even with a stalled probe | R | status.rs:1433 watch_rows_hold_fixed_positions_whatever_is_readable | read-only-views.py:116 exercise (resize and stalled `sysctl` quit) | none | partial: the `--json` rejection is untested |
| `status` rejects mutation flags | R, A | none | none (cli.rs:160 omits `status`) | none | none |
| processes, uptime, thermal, boot time fail closed | A | status.rs:1177 boot_time_fails_closed_when_it_is_in_the_future_or_malformed; status.rs:1338 parses_processes_and_respects_the_limit; status.rs:1351 processes_fail_closed_when_none_parse; status.rs:1271 thermal_reports_no_recorded_limit_as_nominal | none | none | no black-box |

## uninstall

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "paths named for an app's bundle id (report-only)" | R Usage, M | uninstall.rs:507 located_paths_are_reported_and_never_actionable | none (the command is never run by any black-box test) | none | no black-box |
| matches the exact `CFBundleIdentifier`, never a prefix or display name | R, A, M | uninstall.rs:400 identifiers_match_exactly_and_never_by_prefix; uninstall.rs:427 bundle_identifiers_are_validated_as_filenames | none | none | no black-box |
| resolution refuses ambiguity and paths outside `/Applications` and `~/Applications` | M | uninstall.rs:464 resolution_refuses_ambiguity_and_paths_outside_the_roots | none | none | no black-box |
| a symlinked bundle is refused | A | uninstall.rs:495 a_symlinked_bundle_is_refused | none | none | no black-box |
| a running bundle is detected by executable path prefix | M | uninstall.rs:444 a_running_bundle_is_detected_by_executable_path_prefix | none | none | no black-box |
| group containers deliberately omitted; product-named data invisible (disclosed) | A | none | none | none | none |
| `uninstall` does not delete (no deletion path) | R "Where devtrim stops", A | uninstall.rs:507; safety.rs:1809 library_managed_namespaces_are_exact_exceptions | none (no `uninstall --apply` rejection test) | none | partial: flag rejection untested |
| `uninstall` rejects `--apply`, `-y`, `--yolo`, `--shred` | R, A | none | none | none | none |

## optimize

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "preview macOS maintenance tasks": three typed commands with fixed arguments | R, A, M | ops/optimize.rs:213 every_task_is_previewed_as_a_typed_command; ops/optimize.rs:237 task_invocations_are_fully_fixed | none | none | no black-box |
| `--apply` requires an explicit `--task` | R, A, M | ops/optimize.rs:207 applying_without_a_task_selection_is_refused | none (`--task` never passed) | none | no black-box |
| `--task quicklook\|fonts\|launch-services` narrows the plan; unknown names rejected | R Usage, M | ops/optimize.rs:171 selecting_tasks_narrows_the_plan_and_rejects_unknown_names | none | none | no black-box |
| root-requiring or hours-long tasks (`mdutil`, `periodic`, `purge`, DNS flush) are out of the catalog | R, A | ops/optimize.rs:250 root_requiring_tasks_stay_out_of_the_catalog | none | none | no black-box |
| apply refuses a finding whose displayed action was altered | R Typed command boundary | ops/optimize.rs:264 apply_refuses_a_finding_whose_action_was_altered | none | none | no black-box |
| `optimize` stays outside `ops::all()` so `scan` never lists maintenance commands | A | none | cli.rs:395 (empty scan, no `optimize`) | none | partial: only the empty-HOME scan |
| `optimize` rejects `--shred` | R, A | none | none | none | none |
| optimize apply runs the typed command and journals it | R | none | none (the simulator test at cli.rs:1948 covers the same sink, not this op) | none | none |

## largest

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "read-only: biggest directories under scan roots" (`Action::Info`, never deletion authority) | R, A, M | largest.rs:149 ranks_depth_one_and_two_totals_and_clamps_top | cli.rs:456 largest_json_is_one_read_only_document | none | ok |
| depth at most 2; `--top N` clamps (default 20) | M | largest.rs:149 | none (`--top` never passed) | none | partial: flag unexercised end to end |
| overlapping roots count each file once | R | largest.rs:108 overlapping_roots_count_each_file_once | cli.rs:1559 (project commands) | none | ok |
| unreadable entries disclosed as lower bounds; errors AND nonzero | R, A, M | none | cli.rs:477 largest_unreadable_entry_is_disclosed_and_nonzero | none | ok |
| rejects mutation flags | R | none | cli.rs:160 | none | ok |
| names its roots and their origin in human mode | R | safety.rs:2023 | cli.rs:716 (the `largest --root` call asserts `scan roots: ... (from --root)`) | none | ok |
| "no TUI entry" | A | tui.rs:1724 every_cleanup_target_is_reachable_from_the_menu | none | none | partial: negative claim not asserted |

## completions / manpage

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| `completions <bash\|zsh\|fish>` prints a script to stdout | R Usage, M | none | cli.rs:2339 zsh_completions_are_printed_to_piped_stdout | none | partial: bash and fish unexercised |
| unsupported shells rejected | R | none | cli.rs:2349 completions_reject_unsupported_shells | none | ok |
| `manpage` prints roff | R Usage, M | none | cli.rs:2359 manpage_is_printed_to_piped_stdout | none | ok |
| `--json` on either returns the standard error envelope, nonzero | R JSON, A | none | cli.rs:2369 generated_docs_reject_json_with_one_error_document | none | ok |
| they reject mutation flags | R | none | cli.rs:160 | none | ok |
| generated output matches the command definition (no stale flags) | (implied by M) | app.rs:609 every_clean_target_maps_to_its_own_operation_name | none | none | partial: no snapshot of the generated script |
| installed by Homebrew with the binary | M | none | none (release-policy / Homebrew scripts, not part of this matrix) | none | none |

## tui

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| bare `devtrim` opens the TUI only with interactive stdin and stdout; piped prints help and exits nonzero | R, A, M | none | cli.rs:115 no_arguments_in_a_non_terminal_prints_help_and_does_not_start_tui; cli.rs:126 explicit_tui_requires_an_interactive_terminal | none | ok |
| `devtrim tui` rejects `--apply -y --yolo --shred --json` | R, A | none | cli.rs:138 tui_rejects_cli_confirmation_bypasses (`--apply --yolo`); cli.rs:147 tui_json_flag_error_names_the_tui_operation; cli.rs:286 | none | partial: `-y` and `--shred` individually unasserted |
| menu, `?` help overlay, Esc cancel, quit, terminal restored | R | tui.rs:1927 help_overlay_opens_and_closes_without_reaching_the_screen_beneath; tui.rs:2422 control_c_quits_from_every_screen; tui.rs:2027 menu_supports_vim_navigation_and_direct_numbers | tui.py:210 verify_menu | none | ok |
| input queued while a scan or apply blocked is discarded; keys before the plan shows never approve | A | none | tui.py:231 verify_type_ahead (with positive control) | none | no unit |
| selection only narrows; approval covers exactly the selected findings; changing selection invalidates approval | R, A | tui.rs:2643 deselected_findings_never_reach_the_approved_plan; tui.rs:2665 changing_the_selection_invalidates_an_earlier_approval; tui.rs:2678 confirmation_strength_is_recomputed_for_the_selected_subset | tui.py:260 verify_selection | PV tui/selection-plan | ok |
| approval must match the current plan and operation; read-only forged plan fails closed | R Safety model, A | tui.rs:2233 approval_must_match_current_plan_and_operation; tui.rs:2303 forged_read_only_plan_fails_closed; tui.rs:2284 approval_is_invalidated_when_shred_mode_changes | none | none | no black-box |
| ordinary actions need `y`; critical plans need the typed size; Trash purge needs `PURGE <gb>` | R | tui.rs:2044 low_danger_apply_requires_explicit_yes; tui.rs:2075 permanent_apply_rejects_mismatched_typed_size; tui.rs:2096 trash_purge_requires_exact_phrase | tui.py:231 (typed size `0` reaches the apply) | none | partial: `PURGE <gb>` and `y` unit only |
| below 64x18 the interface blocks operation input; only quit remains | R | tui.rs:2522 small_terminal_blocks_hidden_confirmation_input; tui.rs:3542 small_terminal_fails_visibly_without_rendering_the_menu | tui.py:376 verify_minimum_size (exactly 64x18, not below) | none | partial: the below-minimum block has no PTY |
| detail pane grows to show the highlighted finding whole; says how many lines it hides; Enter shows the full view; Esc returns | R, A | tui.rs:3086 the_detail_pane_grows_to_show_the_highlighted_finding_whole; tui.rs:3130 a_terminal_too_short_for_the_details_says_how_many_lines_it_hides; tui.rs:3163 enter_shows_the_highlighted_finding_whole_and_esc_returns_to_the_same_plan | tui.py:376 verify_minimum_size | none | ok |
| every action key explains itself; scan view names "b, then 2 (caches)" | R, A | tui.rs:2761 a_read_only_scan_says_where_each_finding_can_be_acted_on; tui.rs:2850 keys_that_cannot_act_in_a_cleanup_view_say_why; tui.rs:2832 a_read_only_report_says_nothing_there_applies | tui.py:289 verify_action_keys_explain_themselves | none | ok |
| footer shows only valid keys and fits whole at 64x18; `?` reference fits at the minimum | A | tui.rs:2942 every_results_footer_fits_the_minimum_width; tui.rs:2987 every_outcome_status_fits_the_minimum_footer; tui.rs:1971 help_overlay_shows_every_binding_at_the_minimum_size; tui.rs:3493 the_footer_offers_selection_only_when_something_can_be_selected | tui.py:210 (help at the default size) | none | partial: min-size help not in a PTY |
| `?` refused on the confirmation screen | A | tui.rs:2016 help_overlay_never_opens_over_a_confirmation | none | none | no black-box |
| badges READ-ONLY / PREVIEW / PERMANENT explained by the pane | R | tui.rs:2997 the_menu_explains_the_highlighted_badge | none | none | no black-box |
| NO_COLOR degrades every token to a modifier; danger ladder stays ordered; no `Color::` literal in `src/tui.rs` | R, A | theme.rs:155 monochrome_theme_never_sets_a_color; theme.rs:184 monochrome_danger_ladder_stays_ordered_and_distinct; tui.rs:1777 monochrome_theme_leaves_no_colored_cell_on_any_screen; tui.rs:1899 colored_theme_still_paints_the_results_screen | none | none | no black-box |
| risk labels written as text as well as color | R | tui.rs:2431 rendered_menu_and_warning_have_non_color_labels | none | none | no black-box |
| findings, errors and scanner warnings rendered with terminal escapes neutralised | R, A | tui.rs:2459 rendered_errors_escape_terminal_controls; tui.rs:2470 rendered_findings_and_scan_warnings_escape_terminal_controls | none | none | no black-box |
| a partial apply does not claim it stopped when it continued; outcomes scroll | R | tui.rs:2349 a_partial_apply_does_not_claim_it_stopped_when_it_continued; tui.rs:2586 partial_apply_error_is_reachable_by_scrolling | none | none | no black-box |
| scanner diagnostics captured and retained in TUI state | A | tui.rs:2553 captured_scanner_diagnostics_are_visible_in_results | none | none | no black-box |
| Trash preview filters protected items before approval | R | tui.rs:2137 trash_preview_filters_protected_items_before_approval | none | none | no black-box |
| TUI rows wrap losslessly by grapheme; mode survives the minimum width | A | tui.rs:3239 a_row_keeps_whole_graphemes_at_the_end_of_a_path; tui.rs:3288 wrapping_is_lossless_and_keeps_every_row_within_the_width; tui.rs:3477 the_mode_survives_the_minimum_width_on_a_large_plan | tui.py:376 | none | ok |

## config (roots / protect / active_days / retain_days)

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| malformed config is an error, never a silent fallback | R Config, M | fuzz/fuzz_targets/config_parse.rs | cli.rs:539 malformed_config_fails_closed_with_json | none | ok |
| unknown fields rejected | R, M | fuzz/fuzz_targets/config_parse.rs | cli.rs:549 unknown_config_field_fails_closed_with_json | none | ok |
| `roots` expands `~`; explicit `--root` replaces configured roots, which replace the defaults | R, M | safety.rs:1961 explicit_roots_replace_the_defaults | cli.rs:588 config_tilde_root_is_expanded | none | partial: precedence of `--root` over config roots has no black-box |
| an explicit root that does not exist is warned about, not silently scanned | R | none | cli.rs:1860 a_missing_explicit_root_is_warned_not_silently_empty | none | ok |
| a default folder linked to the home folder or above it is skipped with a warning, by spelling and by identity | R, A | safety.rs:1909 default_roots_are_the_conventional_project_folders_that_exist; safety.rs:1997 the_home_folder_is_recognized_under_any_spelling | none | PV roots/default-home-link, PV roots/default-home-identity | no black-box |
| `protect` entries expand `~`, must be absolute; relative entries are an error | R, M | safety.rs:1879 configured_protect_expands_tilde_and_rejects_relative_entries | cli.rs:570 relative_protect_entry_fails_closed_with_json; cli.rs:616 config_tilde_protect_filters_preview_with_diagnostic | none | ok |
| a protected target is refused even if a scanner offers it; previews filter it out with a diagnostic | R, M | ops/mod.rs:2561 preview_filter_drops_only_protected_actionable_findings; ops/mod.rs:2503 journal_records_refused_deletion_as_error | cli.rs:616 (preview filter only) | none | partial: the sink-side refusal has no black-box |
| matching is Unicode-normalization-insensitive and ASCII-case-insensitive; symlinked entries also protect their resolved location; deleting an ancestor of an entry is refused | R | safety.rs:2145 configured_protect_refuses_ancestors_and_matches_symlinked_entries; safety.rs:2179 configured_protect_matches_across_unicode_normalization_forms; safety.rs:2218 configured_protect_refuses_literal_case_variant_and_children | none | none | no black-box |
| a protect alias that drifts after preview fails closed | A | safety.rs:2094 deletion_validation_fails_closed_when_protect_alias_drifts | none | none | no black-box |
| an entry that does not resolve to an existing path "warns loudly" | R | safety.rs:1895 configured_protect_existing_entry_warns_nothing (the control only) | none | none | partial: the warning itself has no positive test |
| `is_config_protected` never panics and is deny-only on arbitrary input | A | fuzz/fuzz_targets/validate_path.rs | none | none | no black-box |
| `active_days` (0 means 1): a repository is active when HEAD commit or newest reflog entry is inside it | R | safety.rs:1984 retention_defaults_to_at_least_thirty_days (zero becomes one) | cli.rs:588 (value set, never varied) | PV git/reflog-activity, PV git/head-commit | partial: no black-box varies the window or the day boundary |
| `retain_days` gates agent history and installers; unset it is `active_days` or 30, whichever is longer | R, A | safety.rs:1984; ops/agents.rs:1982 history_follows_the_retention_window_not_the_project_window; ops/installers.rs:235 installers_follow_the_retention_window_not_the_project_window | none (no `retain_days` in any `cli.rs` config) | PV agents/retention-window, PV installers/retention-window | no black-box |
| a build or dev server whose cwd is in the repository protects it at any window | R | ops/node_modules.rs:965 | none | PV liveness/lsof-escape | no black-box |
| `history`, `completions`, `manpage` work with a malformed config | (app.rs comment) | none | none | none | none |

## global flags / --json contract

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "Every --json invocation emits exactly one JSON document and failures return nonzero" | R Principles, A | report.rs:665 finding_preserves_json_text_and_escapes_only_for_terminals | cli.rs:257 run_time_json_errors_name_the_operation_that_failed; cli.rs:286; cli.rs:303; cli.rs:343; cli.rs:371 json_detection_requires_an_exact_flag | none | ok |
| clap parse errors with `--json` become one document naming the derived operation | A | none | cli.rs:303 clap_parse_errors_with_json_are_one_document_with_the_derived_operation | none | ok |
| `--help --json` and `--version --json` are one nonzero error document; `--version` without `--json` keeps clap output | A | none | cli.rs:343; cli.rs:384 version_without_json_preserves_clap_output | none | ok |
| parse errors escape terminal controls quoted from argv | A | report.rs:793 human_command_action_escapes_controls_without_changing_json | cli.rs:234 parse_errors_escape_terminal_controls_from_argv | none | ok |
| envelope fields `operation`, `applied`, `findings`, `errors`; applied adds `summary` | R JSON | report.rs:714 totals_count_error_entries_apart_from_findings | cli.rs:1727; cli.rs:395 | none | ok |
| actions are typed (`trash`, `shred`, `command`, `info`, `none`), never shell strings | R, A, M | ops/docker.rs:530 rejects_forged_actions_without_authority (forged command refused); report.rs:665 | cli.rs:1948 (command), cli.rs:1899 (shred), cli.rs:456 (info) | none | partial: no black-box emits a `trash` or `none` action |
| `summary.bytes_trashed_estimate` is the part still on disk; summaries never call Trash moves reclaimed | R, A | ops/mod.rs:2321 apply_outcome_counts_trashed_bytes_apart_from_freed_ones; report.rs:686 summary_headline_never_calls_trashed_bytes_reclaimed; report.rs:654 actionable_bytes_saturates_instead_of_wrapping; ops/mod.rs:2456 apply_outcome_size_saturates_instead_of_wrapping | none (every black-box apply is `--shred`) | none | no black-box |
| Trash-first: "Filesystem deletions go to macOS Trash"; "~17.8 GB moved to Trash; it is freed once the Trash is emptied" | R Principles | none (no test reaches `trash::delete` successfully) | none | none | none |
| a failed target: summary keeps the work that succeeded, `errors` names each failure, nonzero | R JSON | ops/mod.rs:2339; ops/xcode.rs:807 | cli.rs:2145; cli.rs:2197 failed_human_apply_does_not_print_a_success_summary | none | ok |
| capability-scoped flags: commands reject mutation flags they cannot honor | R, A | none | cli.rs:160 read_only_commands_reject_mutation_flags; cli.rs:197 mutation_flags_remain_available_only_where_they_have_meaning | none | partial: `trash-empty --shred`, `optimize --shred`, `status`, `uninstall`, `purge` and `tui -y --shred` cases are absent |
| non-TTY apply needs `-y` or `--yolo`; danger 9 or more needs typed confirmation, `--yolo` skips confirmation only | R, M | safety.rs:2376 yes_does_not_bypass_critical_typed_confirmation | cli.rs:1772 (trash-empty, danger 9 only) | none | partial: the danger 1-8 non-interactive refusal on a `clean` category has no test |
| size escalation: more than 1 GB raises danger to 5, 10 GB to 7, 50 GB to 8 | M | safety.rs:2370 aggregate_size_escalates (11 GB and 51 GB only) | none | none | partial: the 1 GB step and every boundary are untested |
| every human apply prints the data-loss notice; JSON does not | R, A | none | cli.rs:2211 human_apply_prints_data_loss_warning_before_action (docker only); cli.rs:2145 (no warning in JSON failure) | none | partial: one category |
| `--help` states that apply flags accept data-loss risk | R | none | cli.rs:2222 help_states_that_apply_flags_accept_data_loss_risk | none | ok |
| terminal output of actions, findings, errors and notes is escaped at the sink; JSON unmodified | R Safety model, A | report.rs:665; report.rs:793 | cli.rs:234 | none | ok |
| `-y` acknowledges risk and bypasses y/N; `--yolo` skips prompts but never operation acknowledgments | R | safety.rs:2376 | cli.rs:1753; cli.rs:1772 | none | ok |
| an existing preview/scan never adds an operation absent from the preview | R | ops/mod.rs:2258 | cli.rs:1948 (argv log) | none | partial: shown only for simulators and Trash |
| `--shred` raises danger to critical (9) and previews `shred` | R | none | cli.rs:1899 shred_is_explicit_in_preview | none | ok |
| every clean target maps to its own operation name | A | app.rs:609 every_clean_target_maps_to_its_own_operation_name | cli.rs:303 | none | ok |

## deletion sink, identity and --shred

| Claim | Source | Unit proof | Black-box proof | Planted case | Gap? |
|---|---|---|---|---|---|
| "Only safety::validate_path_for_deletion creates VerifiedTarget; only the private sink consumes it"; raw filesystem deletion elsewhere is a blocking ast-grep violation | A, R | rule-tests/no-direct-filesystem-delete-test.yml; rule-tests/no-unowned-filesystem-delete-in-owner-module-test.yml | none | none | ok (static rule tests; not run by cargo) |
| "Never add shell-string execution" | A | rule-tests/no-shell-invocation-test.yml | none | none | ok (static) |
| device/inode (plus generation) recorded at preview and rechecked through an open parent handle; a swapped target is refused | R Identity-verified, A | ops/mod.rs:1846 preview_and_handle_relative_identity_include_the_same_generation; ops/mod.rs:2016 shared_sink_refuses_directory_identity_swap; ops/mod.rs:2046 shared_sink_refuses_file_swap_to_symlink; ops/mod.rs:2189 shared_sink_refuses_finding_without_preview_identity | none | none | no black-box |
| exact non-UTF-8 target identity honoured | A | ops/mod.rs:1981 shared_sink_uses_exact_non_utf8_target_identity; safety.rs:1839 validation_preserves_arbitrary_non_utf8_leaf_identity | none | none | no black-box |
| foreign devices rejected before Trash or permanent mutation | R, A | ops/mod.rs:1818 same_device_preflight_refuses_a_foreign_directory_before_deletion | none | none | no black-box |
| Git repository/worktree markers at any depth refused, in any ASCII case | R, A | ops/mod.rs:1314 trash_sink_refuses_nested_git_worktree_before_mutation; ops/mod.rs:1346 permanent_sink_refuses_nested_git_worktree_before_deletion; ops/mod.rs:1284 permanent_sink_rechecks_git_marker_after_validation; safety.rs:2042 deletion_validation_refuses_git_repository_and_worktree_roots; safety.rs:2065 deletion_validation_refuses_git_metadata_case_variants | cli.rs:1527 (root marker via two categories) | none | partial: deep-marker refusal at the sink is unit only |
| only uv's empty `.git` in `sdists-v<N>` under a tagged cache root is tolerated, and only with uv's lock or the Trash grant | R, A | ops/mod.rs:1504 deletion_accepts_the_empty_git_marker_uv_writes_into_its_cache; ops/mod.rs:1521 a_uv_cache_without_uv_s_lock_keeps_its_marker_refused; ops/mod.rs:1542 a_uv_lock_grants_only_its_own_root; ops/mod.rs:1566 an_unreadable_cachedir_tag_only_withholds_the_uv_exception; ops/mod.rs:1605 only_the_exact_uv_marker_shape_is_tolerated | none | PV sink/uv-marker-nonempty, PV sink/uv-marker-case, PV sink/uv-marker-untagged, PV sink/uv-marker-special-file, PV sink/uv-marker-depth, PV sink/uv-marker-other-bucket, PV sink/uv-marker-grant, PV sink/uv-grant-root | no black-box |
| permanent deletion quarantines with a no-replace rename, drives recursion through open handles, restores on mismatch | R, A | ops/mod.rs:1242 permanent_sink_quarantines_and_deletes_a_verified_normal_path; ops/mod.rs:1867 quarantine_identity_mismatch_restores_the_original_name; ops/mod.rs:1903 quarantine_rename_never_replaces_an_occupied_name; ops/mod.rs:1937 quarantine_restore_failure_preserves_both_names_and_reports_quarantine_path; ops/mod.rs:1389 a_restored_permanent_refusal_names_the_original_target; ops/mod.rs:2086 concurrent_namespace_mutation_never_destroys_a_bystander | cli.rs:1154, cli.rs:1406, cli.rs:2386 (successful permanent removal only) | PV sink/rename-no-replace | partial: refusal and race paths have no black-box |
| system roots, user secrets, home root, Trash root and symlinked ancestors are protected | R Protected physical paths, M | safety.rs:1783 protected_system_roots_and_descendants; safety.rs:1796 protected_user_roots_and_descendants; safety.rs:1828 cleaned_parent_aliases_to_user_secrets_are_protected; safety.rs:1858 protects_case_variant_aliases; safety.rs:2314 rejects_symlinked_ancestor; ops/mod.rs:2227 shared_sink_rejects_symlinked_ancestor; fuzz/fuzz_targets/validate_path.rs; fuzz/fuzz_targets/clean_path.rs | cli.rs:1911 (owner cache outside the namespace) | none | partial: direct refusal of `/System`, `~/.ssh` and friends has no black-box |
| `~/Library` managed exceptions are exact | R, M | safety.rs:1809 library_managed_namespaces_are_exact_exceptions | cli.rs:1116 | none | ok |
| the sink rejects missing targets and non-filesystem actions | A | ops/mod.rs:2204 shared_sink_rejects_missing_targets_and_non_filesystem_actions | none | none | no black-box |
| `--shred` is permanent and previewed as such; Trash vs permanent derives from the previewed typed Action, not a runtime flag | R, A | none | cli.rs:1899; cli.rs:2386 | none | partial: no test shows `--shred` absent at apply cannot make a Trash action permanent |
| the sink verifies identity and refuses a missing preview identity | R | ops/mod.rs:2189 | none | none | no black-box |
| directory size measurement fails closed on unreadable content; does not follow a root symlink | R Measurement | safety.rs:2404 directory_size_fails_closed_on_unreadable_content; safety.rs:2422 directory_size_does_not_follow_a_root_symlink; safety.rs:2764 size_lookup_errors_are_not_reported_as_empty_paths | cli.rs:477 (largest) | none | partial: apply-blocking on incomplete size has no black-box |
| fixtures cannot leak a stale repository when a test panics | A | ops/mod.rs:1196 a_fixture_is_removed_even_when_its_test_panics | none | none | ok (meta) |
| liveness parsers fail closed on malformed `pgrep`/`lsof` output | A | safety.rs:2439 parses_build_process_probe_outputs; safety.rs:2486; safety.rs:2541 a_build_process_that_started_during_the_probe_is_looked_up_once; safety.rs:2594 real_lsof_race_with_an_exited_process_is_resolved_not_refused; safety.rs:2657 executable_mappings_refuse_any_mapping_they_cannot_name; safety.rs:2731 lsof_cwd_names_are_decoded_or_refused_never_taken_literally; fuzz/fuzz_targets/probe_parsers.rs | cli.rs:1449; cli.rs:2769 | PV liveness/lsof-escape, PV liveness/lsof-unreported-running, PV liveness/lsof-successor, PV liveness/lsof-mapping-names | ok |

---

## Gaps (ranked)

Ranking rule: inside the mutation and safety tier, `none` above `no black-box`; then report-only features; then cosmetic. Each item names the row(s) above.

### Tier 1: mutation or safety claims with no proof at all (Gap = none)

1. **Trash-first is unproven end to end.** No test reaches `trash::delete` (`ops/mod.rs:418`). Every black-box and PTY apply uses permanent mode. Consequences: the default mode, `bytes_trashed_estimate`, the "moved to Trash; freed once the Trash is emptied" message, and the sink journaling Trash identity are proven only in pieces. `trash_empty_only_devtrim` (`cli.rs:807`) writes its journal record by hand.
2. **`trash-empty --shred` and `optimize --shred` rejection.** README, AGENTS and MANUAL state `trash-empty` rejects `--shred`. `incompatible_flags` (`app.rs:74 incompatible_flags`) encodes it; no test exercises it. Neither do the `status`, `uninstall` and `purge` rows of the same table.
3. **Simulator apply recheck.** "rechecks that it is still unavailable" (the `became available after preview` and `vanished after preview` refusals, `ops/simulators.rs:266`) is claimed in README and MANUAL; no test feeds a second `simctl list` answer.
4. **Docker daemon-down error naming the VM image and its size** (README, AGENTS). Only the not-found versus nonzero split is tested (`ops/docker.rs:616`).
5. **`scan_all` concurrency contract** (AGENTS): byte-identical to serial, and a panicked thread becomes that category's error. No test.
6. **pnpm store is never offered** (README, AGENTS). Only a comment and an evidence string carry it.
7. **Preview and read-only commands change nothing.** No tree snapshot around any preview, `scan`, `status`, `uninstall`, `analyze`, `largest`.
8. **`toolchains` and `optimize` apply**, the Xcode-built-in-chain claim, and the installers "preview shows the age" claim: no test of the successful apply or message.
9. **Other `none` rows**: `trash-empty` re-shows the exact set at `--apply`; `analyze` nonzero exit on a partial; `status` rejects mutation flags; `history`/`completions`/`manpage` work with a malformed config.

### Tier 2: mutation or safety claims proven by unit tests only (Gap = no black-box)

10. **Protect refusal at the sink, apply side** (`cli.rs:616` proves only the preview filter), identity drift and swap (`ops/mod.rs:2016`, `ops/mod.rs:2046`), foreign-device refusal (`ops/mod.rs:1818`), symlinked-ancestor refusal.
11. **Xcode liveness while Xcode runs.** The sandbox `pgrep` stub never reports Xcode (`cli.rs:29`, in `Sandbox::new`), so the guard, the open-file matching (the `xcode/active` planted cases, `PV liveness/xcode-open-files`) and the apply-time re-judging have no black-box.
12. **Build-process liveness for `node-modules` and `artifacts`** (a running `node`/`cargo` in the repo): unit only (`ops/node_modules.rs:965`, `ops/artifacts.rs:927`). Only the probe-failure branch is black-box.
13. **Candidate-exclusion rules**: node_modules without a manifest, inside build output, orphaned or unborn repositories, Terraform state, case-variant ancestors, newer artifact names, `CACHEDIR.TAG` regularity, symlinked owner/ancestor. All unit plus planted, none black-box.
14. **Hostile-repository inertness**: signature program and lazy fetch are unit-tested (and planted); hooks and fsmonitor overrides have no test at all; no black-box runs a hostile `.git/config`.
15. **Apply paths of `installers`, `toolchains`, and the `agents` busy-installer and promotion cases**: unit only.
16. **`retain_days` and `active_days` end to end.** No `cli.rs` config sets `retain_days`; no test varies `active_days` or probes the day boundary; the preview-time "recent repository is skipped" case has no black-box (`cli.rs:1994` is apply-time only).
17. **Continue-past-refusal for `caches`, `agents`** has no planted case and no black-box (only `xcode`, `trash-empty`, `artifacts`, `node-modules` are planted). The preflight-all-before-mutating behaviour of `node-modules` and `artifacts` also has no planted case.
18. **uv cache lock refusal and ancestor-lock** (`PV caches/uv-lock`, `PV caches/ancestor-lock`) and the uv marker exception: unit and planted only.
19. **Docker `builder prune` and the "never volumes" success path.** The success black-box (`cli.rs:2211`) logs no argv and its stub offers only an Images row.
20. **Danger and consent arithmetic**: the MANUAL escalation table's 1 GB step; the plus-or-minus 2 GB `--confirm` tolerance; the non-interactive refusal of a danger 1-8 `clean` apply (only trash-empty reaches it).
21. **Journal**: unwritable-journal block and symlinked-path refusal have no black-box.

### Tier 3: structural weakness of the proof itself

22. **The planted gate covers only `--lib` unit tests** (`planted-violations.py:895 build_test_binary`). No black-box or PTY test is proven able to fail. The `PV-E2E` tag at `cli.rs:874` is not wired to any gate.
23. **Evidence entries** (the three `evidence` planted cases) prove only that a blank non-ASCII evidence string fails the loop. Whether an entry's owner, contents and vendor citation actually support deletion is review only.
24. **Documentation contradiction**: `MANUAL.html:560` says installers need "the active window"; README, AGENTS and `ops/installers.rs:235` say `retain_days`. Do not edit MANUAL.html in this task; fix in the evals change.

### Tier 4: report-only features

25. **`uninstall`**: not run by any black-box test; exact-identifier matching, ambiguity, symlinked bundle, running bundle and group-container omission are unit-only or unproven.
26. **`analyze --json` one-shot** and its nonzero-on-partial exit; **`status`** exit status on unavailable metrics (JSON and one-shot) and the memory-used formula end to end; **`largest --top`**; **`history --limit`**; **`icloud`** below-threshold exclusion; **`leftovers`** positive findings and non-actionability; **`completions bash|fish`**.
27. **`optimize` black-box**: `--task`, `--apply` refusal without a task, and the journal.

### Tier 5: cosmetic and TUI presentation

28. TUI layout, footer, wrap, badge and escape tests are unit only (`tui.rs`); the PTY covers menu, type-ahead, selection, action-key text, purge and the 64x18 detail pane (`tui.py`). Not in a PTY: below-minimum blocking, `PURGE <gb>`, `?` at 64x18, NO_COLOR, partial-apply scrolling.
29. `tui -y` and `tui --shred` rejection individually; `--version` and help text details.

## Citation check

Run after writing this file: every `path:LINE name` for a Rust test was matched against `tests.txt` (scratchpad) built from `#[test]` + the following `fn` line over `src/**/*.rs` and `tests/cli.rs`; every `PV <marker>` was matched against the `marker=` fields in `scripts/tests/planted-violations.py`; every `tui.py:` and `read-only-views.py:` `def` name and line was matched against the file. Result: all cited Rust tests, planted markers and PTY functions resolve. Citations that are intentionally not tests: `cli.rs:29`, `cli.rs:874`, `ops/mod.rs:418`, `ops/simulators.rs:266`, `safety.rs:1243`, `app.rs:74`, `planted-violations.py:895`.
