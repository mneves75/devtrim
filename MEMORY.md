# Project Memory

## Current state

devtrim 0.3.0 implementation is on `master`. `v0.3.0-beta1` proved the arm64
archive but is not promotable because it was built locally and published as a
mutable, unattested prerelease. The next release commit adds a hosted attested
build and immutable exact-byte beta-to-production promotion.

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

## Next boundary

Enable immutable GitHub releases, publish and verify `v0.3.0-beta2` from the
next clean exact-CI commit, then stop for explicit production confirmation.
Never promote or reuse `v0.3.0-beta1`.
