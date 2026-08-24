# Project Memory

## Current state

devtrim 0.3.1 hardening is being prepared on `master`. `v0.3.0-beta2` proved
the hosted, immutable, attested exact-byte promotion path, but 0.3.1 supersedes
it with fail-closed measurement/config parsing and truthful secret-scan gates.

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

## Next boundary

Publish and verify `v0.3.1-beta1` from the clean exact-CI commit, then stop for
explicit production confirmation. Never promote or reuse `v0.3.0-beta1`.
