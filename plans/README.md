# Implementation Plans

Generated on 2026-08-23 from devtrim commit `8dbbd6c`, after comparison with
`tw93/mole` commit `9f2a9a3` and independent reviews by Claude Opus 5 and
GPT-5.6 Sol. Plans 001–007 intentionally hardened devtrim's narrow cleanup
product without importing Mole's broader uninstall, optimize, sudo, or
app-protection scope. Plan 008 is the pre-release audit of the later original
Ratatui interface and its shared safety/release boundaries.

## Execution order & status

| Plan | Title | Priority | Effort | Depends on | Status |
|------|-------|----------|--------|------------|--------|
| 001 | Make every destructive path physically safe and explicitly applied | P0 | M | — | DONE |
| 002 | Make cleanup plans immutable, conservative, and truthful | P0 | M | 001 | DONE |
| 003 | Stabilize JSON, config, errors, and exit status | P1 | M | 002 | DONE |
| 004 | Add release-blocking tests, CI, and artifact safeguards | P1 | M | 001–003 | DONE (MSRV runs in CI; `scripts/release.sh` requires a green exact-commit run) |
| 005 | Release devtrim 0.2.1 with synchronized documentation | P1 | S | 004 | DONE |
| 006 | Make invalid deletion targets unrepresentable | P0 | M | 001–005 | DONE |
| 007 | Build attested betas and promote exact artifacts | P0 | M | 006 | DONE (immutable attested beta verified; production promotion remains an explicit release decision) |
| 008 | Harden the 0.4.0 release boundary | P0 | M | 006–007 | IN PROGRESS |

Status values: TODO | IN PROGRESS | DONE | BLOCKED | REJECTED

## Dependency notes

- 001 owns the shared deletion boundary and must land before op-specific fixes.
- 002 changes finding/apply behavior; 003 must serialize the final contracts.
- 004 locks all safety contracts in tests and release gates.
- 005 only starts after every local gate and independent review is clean.
- 006 replaces the deletion-owner convention with a private verified-target capability and structural enforcement.

## Review record

Independent fresh-context reviews (Claude Opus 5 and GPT-5.6 Sol) plus P3
`autoreview` closeout passes ran against this work. Accepted findings were
fixed; the rejected ones are listed below with their reason.

## Findings considered and rejected

- Mole feature parity: rejected; `PRODUCT.md` defines a narrower developer-disk
  hygiene tool, not an all-in-one Mac maintenance suite.
- Porting Mole code/tests: rejected; Mole is GPL-3.0 while devtrim is Apache-2.0.
  Reuse principles only.
- Async runtime, plugin system, generic cleanup DSL, transaction journal, sudo,
  and app-protection databases: rejected as unnecessary complexity.
- Whole-worktree deletion by naming or branch heuristics: rejected permanently;
  worktree staleness is not safely decidable.
- Whitelists, operation history, broad command timeouts, Homebrew distribution,
  Intel artifacts, signing, and notarization: deferred until measured user or
  distribution needs justify the added surface.
- "CI cannot run the arm64 binary on `macos-14`": rejected; GitHub documents
  `macos-14` as an arm64 (M1) runner.
- "Apply must consume a plan saved by an earlier invocation": rejected; a CLI
  without a persisted plan file cannot promise that. The real gap — confirming
  before the plan was displayed — was fixed instead.
- Byte-reproducible archive metadata: deferred; checksums plus exact-commit CI
  evidence already bind the artifact to a reviewed commit.
