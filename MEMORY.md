# Project Memory

## Current state

devtrim 0.3.2 is the immutable production release. It promotes the exact
attested `v0.3.2-beta1` arm64 archive from commit `1eb6218`. The uncommitted
0.4.0 release candidate adds the Ratatui interface and hardens deletion,
external-command, browser-content, dependency, and release boundaries. Its
arm64 build is installed locally at `~/.local/bin/devtrim`; this is not a
published stable release.

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
  already verified beta bytes manually, then fixes the workflow on `master`.
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
- The production landing page remains on the live v0.3.2 download during beta.
  Artifact-packaged README/manual describe v0.4.0; release validation therefore
  does not treat `index.html` as a beta artifact version surface.
- The demo-video dependency graph is a release gate and receives weekly npm
  Dependabot coverage; shipped inline scripts use exact CSP hashes.

## Next boundary

The current combined diff passed its final P3 autoreview/fix cycle, complete
Rust/MSRV/npm/workflow/security gates, real PTY and responsive browser E2E, and
an exact arm64 package rehearsal. The next boundary is a fresh independent
verifier, then commit/push, green exact-commit CI, and a new immutable beta.
Production still requires explicit confirmation after that beta. Fuzzing,
signing/notarization, and the documented pathname TOCTOU limitation remain
follow-ups rather than hidden claims.
