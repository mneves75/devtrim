# Progress — 0.15.0 (plans/019, TODO)

Plan written 2026-11-17 on branch `plan/019-overview-explorer` from base 9ce4edc:
[plans/019-overview-and-explorer.md](plans/019-overview-and-explorer.md). Inspired by
`govijr/fix-storage` f354e20 and `morajabi/poo` 1893c39; nothing implemented yet.
Next: T1 + T3 (menu volume line and category sizes), layer 1 first. Open owner
decision: free-space history (new state file) as plan 020, yes or no.

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
