# Project Memory

## Current state

devtrim 0.6.2 is the immutable production release. Production reused the exact
verified beta artifact, and the Homebrew tap plus the sole visible local
installation report 0.6.2. That release binds Docker cleanup to one validated
local Unix-socket endpoint, replaces aggregate simulator deletion with exact
rechecked UDIDs, extends same-device/nested-Git preflight to Trash directories,
and makes metadata and liveness uncertainty fail closed.

The 0.6.3 candidate completed local release verification and is ready for beta
staging; production remains 0.6.2. Its source/security review
closed eight evidence-backed gaps: terminal-safe complete command actions, TUI
protected-Trash filtering before approval, fail-closed present Swift aliases,
capability-scoped global flags (including command-only `--shred` rejection),
implicit-TUI JSON rejection, exact release-version declarations, and
category-specific apply authorization for Xcode/toolchain direct children.

On a controlled `node_modules` corpus, devtrim and Mole 1.52.0 both found the
same 20 stale trees and excluded all 5 recent controls. Under high machine load,
15 alternating samples averaged 0.481 s for devtrim and 5.800 s for Mole; this
is a narrow scanner-path comparison, not a whole-product performance claim.

## Decisions

- Physical removal accepts only a private `VerifiedTarget`; serialized paths
  are presentation-only.
- Owner-reported npm and Homebrew caches are authorized only inside exact
  program namespaces and revalidated at apply.
- The structural deletion rule has positive-control tests and runs in
  pre-commit, CI, and release validation.
- MSRV is a mandatory executed gate; absence of its toolchain is a failure.
- An invariant a machine can check becomes a gate, not prose. `CODING_STANDARDS.md`
  carries only what clippy and ast-grep cannot see, as citable `S<n>` rules, and
  states each gate's blind spot so review knows where its work actually is.
- Pathname TOCTOU remains documented rather than overstated as solved.
- A production release may consume only an immutable, attested beta artifact
  from the same dereferenced commit; production never rebuilds it.
- The production release script is the sole automatic Homebrew entrypoint. Its
  idempotent closeout re-verifies the immutable artifact, updates only the tap
  formula with a normal push, locks local validation to that commit, and proves
  the existing sole `/opt/homebrew/bin/devtrim` installation. Beta skips it.
- Actionable size measurement, Docker size parsing, and config schema parsing
  fail closed; partial or ambiguous inputs never become cleanup authority.
- A failed final-tag workflow never moves the tag. Recovery may publish the
  already verified beta bytes manually, then fixes the workflow on `main`.
- Every human apply states the data-loss risk. `-y` skips normal y/N only;
  `--yolo` skips interactive prompts, but operation-specific acknowledgments
  such as `trash-empty --confirm=<gb>` remain mandatory.
- Aggregated sizes saturate instead of wrapping, and measurement errors fail
  closed before they can lower a danger score or authorize mutation.
- The TUI is a presentation adapter over existing `Op` owners. A matching typed
  approval is required at apply time, and CLI bypass flags are rejected.
- Global mutation flags are capability-scoped and rejected when the selected
  command cannot honor them; command-only Docker and simulator cleanup reject
  filesystem-only `--shred`, so flags never become silent no-ops.
- Xcode and Swift toolchain apply reassert the scanner's exact direct-child
  category shape before passing a target to the shared deletion sink.
- Terminal escaping happens at the final human rendering sink for the complete
  action or message; structured JSON retains the original value.
- Ratatui 0.30.2/Crossterm 0.29 require MSRV 1.88. Default Ratatui features,
  including its optional layout cache, stay off; the graph resolves patched
  `lru 0.18.2` instead of affected `0.12.5`.
- Fixed protected path components are compared ASCII case-insensitively at the
  shared validation boundary, while component-aware matching keeps similarly
  prefixed names such as `/systematic` outside the protected set.
- Release builds run without write or OIDC authority. A separate publisher job
  receives the packaged artifact and alone owns attestation and release writes;
  retrying all jobs replaces only the intermediate handoff, while retrying the
  publisher refreshes remote release state before deciding whether to create.
- Structural deletion enforcement uses separate rules for ordinary Rust source
  and the single owner module; positive controls prove a forged second sink is
  rejected both inside and outside that module.
- `Finding::command` alone issues the closed `CommandAuthority` capability;
  Docker and simulator apply require that capability, its validated endpoint or
  UDID, and its exact serialized action to agree before execution.
- Historical immutable tags and releases are provenance records and stay
  intact. The earlier request to delete or rewrite them was rejected.
- The production landing page remains on the current stable download during a
  beta, then advances only after exact-byte production promotion. Artifact
  validation does not treat `index.html` as a beta package version surface.
- The demo-video dependency graph is a release gate and receives weekly npm
  Dependabot coverage; shipped inline scripts use exact CSP hashes.
- Journal paths are opened component-by-component without following symlinks;
  apply writers serialize each synced record and keep rotation coordination
  across attempt/result, while `history` creates no state, waits for active
  guarded attempts, pairs legacy records across generations, and bounds input.
- Permanent recursive deletion performs a complete same-device/Git-marker
  preflight before the first removal, then repeats the checks while consuming
  the quarantined tree through directory handles.
- Hosted release credentials never share a job with repository or dependency
  execution. The local pre-tag phase is provenance-only; hosted read-only jobs
  produce the handoff consumed by the no-checkout publisher. After immutable
  publication, the local Homebrew closeout uses the authenticated tap boundary
  and scrubs token environment variables from install/test execution.

## Next boundary

0.5.0, 0.6.0, and 0.6.1 each passed independent security review plus structured
P3 autoreview, and the 0.6.2 local candidate passed the same review boundary,
with every verified release-scope finding fixed before publication; full
Rust/MSRV/npm/workflow/security gates, bounded fuzz runs, real PTY TUI passes,
exact-commit CI, staged-binary smoke tests, and exact-byte production promotion
also passed. Notarization
remains blocked on notarytool credentials (Developer ID cert exists locally;
runbook in SECURITY.md). Residual path-based windows (parent resolution,
Trash, single-entry unlink) are documented, not hidden.
