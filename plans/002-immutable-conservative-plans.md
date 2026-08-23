# Plan 002: Make cleanup plans immutable, conservative, and truthful

> **Executor instructions**: Apply only the reviewed candidate set. Unknown
> activity or ownership must fail closed. Run focused tests after each op.
>
> **Drift check**: `git diff --stat 8dbbd6c..HEAD -- src/report.rs src/ops/`

## Status

- **Priority**: P0
- **Effort**: M
- **Risk**: HIGH
- **Depends on**: `plans/001-safe-deletion-boundary.md`
- **Category**: bug / security / performance
- **Planned at**: commit `8dbbd6c`, 2026-08-23
- **Execution status**: DONE

## Why this matters

Several operations apply a broader or different action than the user previewed.
`node-modules` rescans after confirmation, simulator `--yolo` adds an unlisted
erase-all, and failed owner commands are reported as successful reclaimed work.
A destructive CLI must make preview and apply use the same explicit plan.

## Current state

- `src/report.rs:5-18`: `Finding.action` is an untyped display string.
- `src/ops/node_modules.rs:52-80`: scan aggregates paths behind one
  representative path; apply discovers targets again.
- `src/ops/node_modules.rs:149-160`: failed `git log` becomes `None`, which is
  interpreted as stale.
- `src/ops/node_modules.rs:106-120`: WalkDir records only top-level
  `node_modules` but still traverses `.git` and dependency trees.
- `src/ops/simulators.rs:72-80`: `--yolo` adds `simctl erase all` although the
  finding previews only `delete unavailable`.
- `src/ops/toolchains.rs:22-54`: missing/broken `swift-latest` can select every
  installed toolchain; other symlink references are not checked.
- `src/ops/leftovers.rs:13-70`: whole directories are eligible from naming
  patterns and an unsupported claim that a mission is complete.
- `src/ops/docker.rs:66-79` and `src/ops/caches.rs:79-105`: failures can still
  count bytes or return overall success.

## Scope

**In scope**: `src/report.rs`, all affected `src/ops/*.rs`, focused tests.

**Out of scope**: generic plan framework, async/concurrency, broad timeouts,
process monitoring, whole-worktree cleanup, whitelist UI, new cleanup targets.

## Steps

1. Replace command/display string parsing with the smallest typed action shape
   needed by existing ops (for example enum variants for Trash, Shred-effective
   rendering, Info/None, and fixed command+args). Never reconstruct argv by
   splitting display text.
2. Emit one `Finding` per exact `node_modules` target. Apply only those paths;
   do not call `find_node_modules` from apply. Recheck that the owning repo's
   activity remains stale immediately before deletion. A failed Git command or
   unreadable commit date must skip the target with an actionable reason.
3. Use `WalkDir::filter_entry` to avoid descending into `.git` and any found
   `node_modules`. Preserve nested-project discovery outside those trees.
4. Remove simulator erase-all from implicit `--yolo` behavior. `--yolo` may
   bypass a gate, not add an operation. Keep `delete unavailable` only unless a
   future explicit command is designed and previewed.
5. Make toolchain cleanup fail closed unless a valid existing preserved target
   is resolved. Inspect all symlinks in the Toolchains directory and preserve
   every referenced target. If no safe survivor is known, return no destructive
   findings and explain why.
6. Remove whole worktree/scratch-directory deletion and blanket
   `codex-runtimes` cleanup from this release. Keep only exact disposable
   subdirectories whose recovery contract is established, or make the category
   report-only.
7. Check every external command exit status. Count items and bytes only after
   success. Any failed apply action must make the command return nonzero; do not
   hide it in notes. Keep Docker volumes absent from argv and tests.
8. Compute actionable totals centrally, excluding `info`/`none` findings and
   Xcode Archives. Apply size escalation to the total plan, not independently
   per row. Use estimated/logical size wording unless allocated blocks are
   measured consistently.

## Test plan

- Active repo remains ineligible when Git succeeds.
- Missing/failing Git never makes a repo eligible.
- A `node_modules` created after preview is not applied.
- WalkDir does not descend into nested dependency trees.
- `--yolo` does not invoke `simctl erase all`.
- Missing/broken `swift-latest` yields no destructive plan.
- A second symlink preserves its target.
- Failed Docker/cache/simulator commands return failure and zero successful
  bytes/items.
- Xcode Archives never contribute to actionable total and are never applied.

Use fake executables in a temporary `PATH`; never invoke real Docker, npm, brew,
Git mutation, or `xcrun` in tests.

## Done criteria

- [x] Apply consumes only exact previewed targets.
- [x] Unknown ownership/activity fails closed.
- [x] No whole worktree or active runtime cache is blanket-deleted.
- [x] External-command failure is observable through exit status and summary.
- [x] Docker volumes and Xcode Archives remain invariantly excluded.
- [x] Node scan prunes dependency and `.git` traversal.
- [x] Format, Clippy, and all tests pass.

## STOP conditions

- A fix would require copying Mole GPL implementation.
- Preserving a public behavior requires deleting an unpreviewed path.
- A target's recovery/ownership contract cannot be proven; skip it and report.

## Maintenance notes

A confirmation bypass (`-y`/`--yolo`) may weaken confirmation only; it must never
change the candidate set or add an action.
