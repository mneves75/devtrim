# progress — comment prune and whole-project audit (target 0.13.1)

Branch `chore/comment-prune-audit` from `main` 79d50e4 (fetched 2026-11-14).
Read on restart: this file, `tests.json`, `git log --oneline main..`.

## Play-back

1. Find code comments that add nothing (restate the code, stale, noise) and
   prune them; keep comments that carry what code cannot (platform quirks,
   vendor facts, measured incidents, threat reasoning).
2. Deliver a high-level plan: of the comments that excuse a workaround, which
   are best removed by refactoring the code and which can simply be deleted.
3. Read the whole project; find what is broken, inconsistent, inefficient or no
   longer needed; fix all of it.
4. Verify (full suite, reviews, mneves-verify), then release: bump, changelog,
   docs, merge to main, tag beta then stable, Homebrew, live check, cleanup.
   Targets: "prod" = immutable GitHub release + Homebrew tap + landing page.
   "app-store": devtrim is a Rust CLI with no App Store target; none exists.
   No database exists, so the staging-database authorization is moot.

## Deeper version

A senior maintainer would treat the comment pass as a *standards* pass, not a
taste pass: `CODING_STANDARDS.md` S3 already defines the rule (a comment
carries only what code cannot), so the raw material is the code itself checked
against S3, plus objective classes (TODO markers, commented-out code, stale
identifier/path references). Evidence a prune is right: every gate that reads
source text (planted-violation anchors, release-policy pins, CSP hashes,
facts.json digests, AGENTS==CLAUDE) still passes, and the binary is unchanged
by a comment-only commit. For "broken", the evidence is a reproduction: a test
that fails before the fix. For "inefficient", a measurement. For "no longer
needed", zero callers/readers.

Restated task: prune S3-violating comments; write the refactor-vs-delete plan
for comments that excuse workarounds; fix only reproduced defects, measured
inefficiencies and provably dead code, each with a failing-first test where
behaviour changes; release 0.13.1 only if the packaged binary changes.

## Red team

| # | Failure cause | Trigger | Cost | Early experiment |
|---|---|---|---|---|
| 1 | Assumption: comments are inert | a pruned/moved line is a `planted-violations.py` `before=` anchor, a `release-policy.sh` pin, CSP-hashed inline code, or a facts.json-quoted page line | gate breaks late (CI / release) | after every edit batch run the anchor check (`.scratch/prune-audit/anchors.py`) + `csp.py --check` + `cmp AGENTS.md CLAUDE.md` — seconds |
| 2 | Unconsidered failure: "fix all" regresses a deletion-safety path | an audit "bug" that is intended fail-closed behaviour gets "fixed" | data loss on a user's disk | no finding becomes code without a reproduction; behaviour fixes need a failing test first, and a planted case where the branch is mutable; triage against SECURITY.md / CLAUDE.md conventions |
| 3 | Optimism: release done "because asked" | comment-only diff tagged as 0.13.1, or release run flaky under load ~250 | a no-op release, or a half-finished release chain | decide after triage: if the binary changes, release; if not, say so. Reuse known reruns (handoff: hdiutil busy, RustSec fetch) |

Simpler route: report-only. Rejected because the owner asked to fix.
Verdict: **ship**, with experiment 1 run by me after each edit batch.

## Plan

- [x] T0 baseline `verify.sh focused` on clean main (log: `.scratch/prune-audit/baseline-focused.log`).
- [x] T1 audit (5 read-only workers, disjoint files): comments (delete / refactor / keep), defects with repro, dead code, inefficiency. Accept: every finding has file:line + repro or evidence.
- [x] T2 triage: reproduce each defect; reject unreproduced; record verdicts in `tests.json` + below.
- [ ] T3 comment prune (S3 deletions). Accept: anchors + csp + cmp + fmt + clippy + build hash unchanged.
- [ ] T4 fixes, one commit each with failing-first test. Accept: test red before, green after; focused verify.
- [ ] T5 write comment refactor-vs-delete plan (`plans/017`).
- [ ] T6 full verification: `verify.sh offline`, MSRV, fuzz, PTY, planted violations; reviewer agent; cross-model review; mneves-verify.
- [ ] T7 release (if binary changed): bump, changelog/docs, security gate, PR+merge, beta, stable, Homebrew, verify-live, cleanup.

## Decisions

- progress.md/tests.json live at repo root during the branch; folded into `plans/017` before merge.
- Preserve CLI/JSON contracts (AGENTS.md declares no "no real users" status).

## Findings / triage

T0 baseline: `verify.sh focused` exit 0 on 79d50e4. Anchor check: 215/215.
Mechanical comment passes: 0 TODO/FIXME/HACK, 0 commented-out code, 0 stale
backticked paths/identifiers (17 hits all external vendor files).

FIX (behaviour; failing-first test each)
- J1 journal rotation runs at `Ctx::from_cli` for every command but dashboard/app-storage: read-only commands rename journal files (a `trash-empty --only-devtrim` preview can rotate away `.3` before reading it); the TUI never rotates in a long session. Fix: rotate in `journal::check_writable` (apply start), TUI preflights it too. Owner: main session.
- O1 `leftovers`: one unreadable `.supergoal` fails the category (S8 scope). O2 `installers`: unreadable Desktop/entry fails the category. O3 `toolchains`: bare `?` errors without path (S7). O4 `node_modules` lockfile note reads the repo root, not the package folder; ancestor message wrong. Owner: worker ops.
- U1 `scan` summary re-sorts top 5 by size then groups consecutive runs: project header repeats/undercounts. U2 interactive `analyze` exit status/warnings reflect only the last folder. U3 `status --watch` has no too-small screen (DESIGN.md). Owner: worker ui.
- S1 `trash_gate` messages say "Trash size" for the previewed subset. S2 nonexistent `--root` listed as a scan root. Owner: main.

DEAD / REDUNDANT (no behaviour change)
- app.rs:37 dead `&& !cli.json`; journal `flush()` before `sync_data`; leftovers `!prefix.is_empty()`; toolchains discarded `PathBuf`; tui iCloud filter special case; analyze unreachable device branch; dashboard unreachable owner branch; `analyze` PAGE_ROWS duplicate.
- KEPT on purpose: redundant re-checks inside safety paths (agents digest/current, simulators component) — defence in depth at the deletion boundary is not dead code.

COMMENTS: apply every audit A-list item (deletions + stale rewrites); B-list → plans/017.

DEFERRED to plans/017 with reason (pinned anchors / owner decision / unmeasured):
xcode DerivedData double walk, artifacts double walk, agents lsof once per home,
installers judge-once, docker disclosure-as-finding, relocated Homebrew cache,
Ctx::protect typed entry, Unicode-whitespace evidence, operation-prefix on all errors.

## Status log

- 6aadc23 J1 journal rotation only at apply start (CLI + TUI); tests red→green, TUI mutation check.
- ba04bc9 safety-core comments, trash_gate wording, dead `!cli.json`/`flush`.
- S2 (missing --root listed) REJECTED: dashboard preview commands carry named roots verbatim by contract.
- 956620e merged fix/ops-audit (O1–O4 + comments; main session corrected the lockfile label for workspaces and made a note never withhold a finding).
- Running: wt-ui (U1–U3 etc.), wt-docs (scripts/docs), wt-tests (tests/fuzz).
- Deferred with reason in plans/017: artifacts/xcode double walks (pinned anchors; unmeasurable at load ~250).
