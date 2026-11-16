# Progress — 0.14.0 (plans/018)

Branch `feat/0.14-measured-space` from `f9f806d`. Plan: `plans/018-measured-space-and-explain.md`.

## State

- [ ] T1 clone-aware sizing (worker A, worktree)
- [ ] T2 measured free space after apply (worker B, worktree)
- [ ] T3 `disk --warn-below` (worker B)
- [ ] T4 DerivedData workspace note (worker C, worktree)
- [ ] T5 `explain <path>` (worker C)
- [ ] T6 docs/version
- [ ] T7 gates and release

## Decisions

See plans/018 § Decisions.

## Next

Dispatch A, B, C in parallel; integrate in order A, B, C; run `verify.sh offline`.
