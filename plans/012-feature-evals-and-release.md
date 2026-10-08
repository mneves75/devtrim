# 012 — Feature evals, security pass, page load, release 0.10.8

Branch `feat/feature-evals`, base `01f5ef0` (0.10.7). Owner request: prove every
feature does what it says, safely, with evals shown able to fail; audit security;
every page under 50 ms; release.

## What the review changed

Inputs: Claude's draft, Sol's independent plan (GPT-6.1 Sol), an adversarial
GPT-6 Astra review, and the expert-review pass.

- The oracle no longer claims "exact" without a spec. It compares an enumerated
  set of allowed changes: payload removals (whole pre-snapshot subtrees), journal
  appends (checked by their own assertions) and harness logs. Survivors keep
  type, mode, size, content hash, regular-file mtime, link target and inode.
  Directory mtimes and atime are excluded. Oracle controls prove it catches a
  modification, a creation, a same-content replacement, a symlink retarget and an
  extra deletion, and accepts a legitimate parent-directory change.
  It is called *observed net-state preservation within the fixture*.
- Preview→apply drift is tested inside one invocation (PTY confirmation barrier):
  after preview, plant a new eligible target, refresh an approved target's age,
  swap one, add a tracked file. New targets stay; changed ones are refused.
  Separate preview and apply commands cannot catch a rescan-after-confirm bug.
- `planted-violations.py` gains a test-target field so a mutant can select an
  integration test (`--test evals`) and is checked against the rebuilt binary.
  Each claimed boundary gets its own marker and positive control, not one
  generic marker per feature.
- Journal ordering is proven, not counted: an unwritable journal prevents the
  mutation; an attempt exists before a command stub runs (the stub checks it).
- Existing black-box tests that already apply (simulators, plain trash-empty,
  purge, xcode, agents, HF) are upgraded with the oracle rather than duplicated.
- The page-load script fails closed: finite timings, observed LCP, every asset
  decoded, no CSP violation, CDP deadlines, nonzero exit over budget; it is
  proven to reject a deliberately slow page and a broken asset.
- Trash via Finder, a real Docker daemon, real simulators and macOS maintenance
  tasks are native effects. Stubs prove devtrim's side; the native side is
  either run in a disposable environment or reported as not proven.
- The ledger is a tracked plan (this file plus `plans/012-claims.md`), not a
  scratch note.

## Phases

1. **Ledger.** Every README/MANUAL/Conventions claim → stable ID → existing proof
   → gap. Read-only inventory already running.
2. **Pilot.** One feature end to end (`clean caches`): oracle + its controls,
   in-invocation drift, journal ordering, a planted boundary through the
   extended runner. Expand only after the pilot proves the pipeline.
3. **Evals by gap.** Mutating features first (node-modules, artifacts, purge,
   xcode, toolchains, installers, agents, trash-empty, docker, simulators,
   optimize), then report-only (scan, largest, leftovers, icloud, uninstall,
   history, analyze, status) with fixture-derived values, not field presence.
   Each defect found: red test → smallest fix.
4. **Security.** Fresh-context `security-audit` over authority boundaries, process
   execution, parsers, journal, terminal output, release scripts, shipped HTML.
   Done: zero confirmed findings at medium or above after one fix and rerun.
5. **Page load.** Fix the gate, re-baseline, find the bottleneck by measurement,
   then optimize (hero PNG 1.3 MB and manual layout are the candidates).
6. **Review.** code-review skill, reviewer agent, mneves-verify; fix; rerun once.
7. **Release** per AGENTS.md § Release: 0.10.8, changelog, docs, fuzz lock,
   gates, beta1, download verification, prod, Homebrew, landing page, merge,
   delete branch.

## Alternatives considered

- Formal or `cargo-mutants` framework: deeper in places, unbounded cost, reports
  any failing test as a kill. Rejected.
- Premise challenge — "the suite already proves it; publish only the ledger":
  cheapest, honest, but leaves drift, journal ordering and seven commands
  unproven end to end. Rejected as the whole answer; kept as phase 1.

## Five years out

Stable claim IDs, small fixtures, recorded native limits, and boundary-named
mutants stay readable after the authors leave. A vendor change should break a
named assertion, not widen deletion silently.
