# Progress — 0.14.0 (plans/018)

Branch `feat/0.14-measured-space` from `f9f806d`. Plan: `plans/018-measured-space-and-explain.md`.

## State (2026-11-16)

- [x] T2 measured free space after apply — worker B, merged (bb02fd1)
- [x] T3 `disk --warn-below` — worker B, merged
- [x] T4 DerivedData workspace note — worker C, merged
- [x] T5 `explain <path>` — worker C, merged
- [ ] T1 clone-aware sizing — worker A branch `worktree-agent-a461409c566741beb`, NOT merged; rework running:
  snapshot fallback, memory (563 MB peak in `largest`), frames.py clone fixture, page.html sentence
- [x] T6 docs for T2–T5 (01d1caf), film facts regenerated (77f2cfa); CHANGELOG 0.14.0 drafted, uncommitted
- [ ] T6 docs for T1, version bump
- [ ] T7 gates and release

## Decisions

- 5b least-regret ordering dropped: no rebuild-cost data; largest-first is a documented convention.
- Private size is trusted only on APFS (HFS+ reports 0: worker A probe) and only on a volume with no
  snapshot (`diskutil apfs listSnapshots -plist` returns `<array/>`), else st_blocks. Reason: the
  private size excludes snapshot-held bytes (getattrlist(2)); with Time Machine local snapshots every
  old file would read ~0. Snapshot behaviour itself is untested on this Mac (Data volume has none).
- Kept private size over a clone-family-only rule: probe showed a modified clone leaves its family
  (new id, refcount 1) yet still shares blocks; only the private size counts it right.
- Fallback if T1 cannot be finished: ship 0.14.0 without T1 and drop its changelog entry.

## Next

Review A's rework, merge, docs for T1, `verify.sh offline`, reviewer + autoreview + mneves-verify,
security gate, bump 0.14.0, release beta then production.
