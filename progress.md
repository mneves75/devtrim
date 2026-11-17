# Progress — 0.15.0 (plans/019, IN RELEASE)

Plan [plans/019-overview-and-explorer.md](plans/019-overview-and-explorer.md), branch
`plan/019-overview-explorer` from 9ce4edc; inspired by `govijr/fix-storage` f354e20 and
`morajabi/poo` 1893c39, nothing copied. T1–T8 and the site redesign are merged on the
branch (59c8878, fcac80d, d019dc0, 8927c1c, b360a20) with docs and the 0.15.0 bump
(2968e7d) and real frames (f45d7bb, 9c099a0). Status per task: `tests.json`.

Remaining: offline gate on the final commit, reviewer, mneves-verify, security gate on the
merged tree before the merge (Pages deploys on merge), then beta, production and Homebrew.
Open owner decision: free-space history (new state file) as plan 020, yes or no.

Decisions: `init.sh` not added, since `scripts/verify.sh` already sets up and runs every
check. The overview frame is cropped to the operations column so the demo machine's disk
numbers never reach the site.

# Progress — 0.14.0 (plans/018)

Shipped 2026-11-16: main 26ed547 (tree 9205907), immutable v0.14.0-beta1 and
v0.14.0 (zip sha256 e0f01326…), Homebrew tap 20fc13a, `/opt/homebrew/bin/devtrim`
0.14.0 sole. Security gate: staging and production WAIVED (GHSA-g3hv-75wg-m249,
expires 2026-11-30); records in `.scratch/security-gate/26ed547…/`.

## Decisions

- 5b least-regret ordering dropped: no rebuild-cost data; largest-first is a documented convention.
- Consent, danger and totals use allocated blocks; the APFS private size is a displayed lower
  bound only (`frees_at_least_bytes`), trusted only on APFS volumes with no snapshot.
- Clone-family credit dropped: an edited clone outside the tree can still hold the shared blocks.

## Follow-ups

- Remove the unused clone fields from `crates/apfs-size` (touches the only `unsafe` call).
- CORE-PARSE-1 (lsof omitted-name hypothesis) stays needs-validation, pre-existing.
- GHSA-g3hv-75wg-m249 waiver expires 2026-11-30.
