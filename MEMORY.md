# Project Memory

## Current state

devtrim 0.3.0 implementation and documentation are complete on `master`. The
typed deletion boundary, security scans, local release gates, independent
verification, and exact-commit CI passed. Tagging, GitHub Release creation, and
publication remain a separate explicitly authorized workflow.

## Decisions

- Physical removal accepts only a private `VerifiedTarget`; serialized paths
  are presentation-only.
- Owner-reported npm and Homebrew caches are authorized only inside exact
  program namespaces and revalidated at apply.
- The structural deletion rule has positive-control tests and runs in
  pre-commit, CI, and release validation.
- MSRV is a mandatory executed gate; absence of its toolchain is a failure.
- Pathname TOCTOU remains documented rather than overstated as solved.

## Next boundary

Do not tag or publish 0.3.0 until explicitly requested. When requested, run
`scripts/release.sh 0.3.0` from a clean, synchronized `master`.
