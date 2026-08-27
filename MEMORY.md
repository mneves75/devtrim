# Project Memory

## Current state

devtrim 0.6.0 is the immutable production release (commit `d05c0ed`), promoting
the exact attested `v0.6.0-beta1` archive (`e8028136…`). 0.6.0 anchored the
deletion sink (cap-std parent handle, preview-time device/inode, quarantine +
handle-bound recursive delete for permanent mode), added `largest`, journal
rotation under a rustix flock, explicit ad-hoc codesign in the release
workflow, and the refreshed demo video. The 0.6.1 Unreleased line is open.

## Decisions

- Physical removal accepts only a private `VerifiedTarget`; serialized paths
  are presentation-only.
- Owner-reported npm and Homebrew caches are authorized only inside exact
  program namespaces and revalidated at apply.
- The structural deletion rule has positive-control tests and runs in
  pre-commit, CI, and release validation.
- MSRV is a mandatory executed gate; absence of its toolchain is a failure.
- Pathname TOCTOU remains documented rather than overstated as solved.
- A production release may consume only an immutable, attested beta artifact
  from the same dereferenced commit; production never rebuilds it.
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
  Docker and simulator apply require that capability and its exact serialized
  action to agree before fixed-argument execution.
- Historical immutable tags and releases are provenance records and stay
  intact. The earlier request to delete or rewrite them was rejected.
- The production landing page remains on the current stable download during a
  beta, then advances only after exact-byte production promotion. Artifact
  validation does not treat `index.html` as a beta package version surface.
- The demo-video dependency graph is a release gate and receives weekly npm
  Dependabot coverage; shipped inline scripts use exact CSP hashes.

## Next boundary

0.5.0 and 0.6.0 each passed an independent security review plus structured P3
autoreview rounds (13 and 7 verified findings respectively, all fixed
pre-release with regression tests), full Rust/MSRV/npm/workflow/security
gates, bounded fuzz runs, real PTY TUI passes, exact-commit CI, staged-binary
smoke tests, and exact-byte production promotion. Notarization
remains blocked on notarytool credentials (Developer ID cert exists locally;
runbook in SECURITY.md). Residual path-based windows (parent resolution,
Trash, single-entry unlink) are documented, not hidden.
