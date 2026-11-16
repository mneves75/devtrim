# 018 — Measure what an apply frees, size APFS clones, explain a path

Status: SHIPPED in 0.14.0 (main 26ed547, tag v0.14.0, tap 20fc13a). Base `f9f806d` (0.13.1 production).

Origin: comparison with `morajabi/poo` 1893c39 and `mattt/plonge` faa19c6
(2026-11-16). Only ideas backed by a measurement on this machine are built.

## Evidence

- pnpm installs files as APFS clones of its store. devtrim 0.13.1 previews
  `megasena-analyser-webapp/node_modules` at 1.371 GiB; its APFS private size
  (`ATTR_CMNEXT_PRIVATESIZE`, measured by plonge) is 0.078 GiB. odiarioneutro:
  1.255 vs 0.571 GiB. npm control (psicologia-app): equal.
- Deleting a 3 GiB clone (`cp -c`) freed 1 MiB of `df` free space; deleting a
  3 GiB plain copy freed 3081 MiB. `du`/`st_blocks` counted both as 3 GiB.
- `src/report.rs` records an observed apply: "25 GB reclaimed, `df` unchanged".
- 2 of 3 DerivedData folders on this Mac (3.7 GB) name a `WorkspacePath` that no
  longer exists (deleted agent worktrees); `info.plist` is XML.
- No Rust crate wraps `ATTR_CMNEXT_PRIVATESIZE`; `libc` 0.2.191 (already in the
  lock) exposes `getattrlist` and the constant, which needs `unsafe` FFI.

## Decisions

- Clone-aware sizing lives in a new workspace crate `crates/apfs-size` holding
  the only `unsafe` (one read-only `getattrlist` call). `devtrim` keeps
  `unsafe_code = "forbid"`. Rejected: lifting `forbid` in devtrim (weakens a
  documented guarantee for the deleting crate); keeping `st_blocks` and only
  disclosing (leaves the 17x overstatement in the typed confirmation).
- Measured free space is reported beside the estimate, labelled as observed,
  never as freed by devtrim. Read failure is reported in the summary, not an
  apply error.
- DerivedData workspace evidence is a note only; it grants no authority and
  changes no liveness check (folders are already offered when idle).
- `explain` is read-only: one fresh scan, protection verdict, covering finding.
- Rejected: poo's least-regret ordering (devtrim has no rebuild-cost data; the
  "largest first" ordering is a documented convention), agent auto-clean,
  schedules, hooks that delete, self-update, user rules that widen deletion,
  worktree removal, plonge's tool `clear` commands (all conflict with
  `CLAUDE.md § Conventions`).

## Tasks

| # | Task | Owner | Acceptance check |
|---|------|-------|------------------|
| T1 | `crates/apfs-size` + clone-aware `DiskUsage` (walks and installers) | worker A | unit test: a tree holding only a clone of an outside file measures ~0; a tree holding a file and its clone measures one copy; plain file unchanged; `verify.sh focused` green |
| T2 | Free space before/after every CLI and TUI apply (human + JSON `home_volume`) | worker B | eval: apply JSON carries `home_volume.free_before/free_after`; human summary line present; unreadable volume gives `error` without changing exit status |
| T3 | `devtrim disk [--warn-below SIZE]` | worker B | evals: prints free space; warns only below threshold; bad size refused exit 2 |
| T4 | DerivedData note naming the workspace, "workspace no longer exists" when gone | worker C | unit test with XML plist, gone vs present, binary/garbled plist gives no claim |
| T5 | `devtrim explain <path>` | worker C | evals: protected path names its rule; a stale node_modules names its category finding; unrelated path says nothing offers it; refuses mutation flags |
| T6 | Docs, version 0.14.0, changelog, SECURITY.md, manual, site | main | `verify.sh offline`, CSP check, release policy |
| T7 | Gates: reviewer, autoreview, mneves-verify, mneves-security; release | main | gate records under `.scratch/security-gate/` |

## Outcome

Review found that the private size undercounts what removal frees (an edited
clone still shares extents; clones trashed side by side read 0), which shrank
`trash-empty --confirm`. Sizes stayed allocated blocks; the private size ships
only as the display-only lower bound `frees_at_least_bytes`, with no clone-family
credit. Follow-up: `crates/apfs-size` still reads clone id, refcount and shared
bytes that nothing uses.
