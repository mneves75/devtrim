# Project Memory

## Current state

devtrim 0.5.0 is the immutable production release (commit `053c9fd`), promoting
the exact attested `v0.5.0-beta1` arm64 archive; stable and beta bytes match
(`dc58f370…`). 0.5.0 closed the Mole-audit gaps: `clean artifacts`, `protect`
config, write-ahead journal + `history`, liveness guards, completions/manpage,
lib/bin split with five cargo-fuzz targets, Homebrew tap
`mneves75/homebrew-devtrim`. The 0.5.1 Unreleased line is open.

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

The 0.5.0 release passed an independent security review plus two P3 autoreview
rounds (13 real findings fixed pre-release, incl. protect NFC/NFD and
ancestor/symlink bypasses and GIT_DIR stale-gate poisoning), full
Rust/MSRV/npm/workflow/security gates, five bounded fuzz runs, a real PTY TUI
pass, exact-commit CI, staged-binary smoke tests, and exact-byte production
promotion. Apple signing/notarization (softened by brew distribution) and the
documented pathname TOCTOU limitation remain explicit follow-ups. The demo
video still shows the v0.4.0 menu (no Build artifacts entry) — refresh it in
0.5.x. Journal rotation is unbounded by design for now.
