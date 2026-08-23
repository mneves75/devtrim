# Project Memory

## Current goal

Ship devtrim 0.3.0 code and documentation with the deletion boundary encoded in
the type system, all release gates executed, and the exact commit green in CI.

## Decisions

- Physical removal accepts only a private `VerifiedTarget`; serialized paths
  are presentation-only.
- Owner-reported npm and Homebrew caches are authorized only inside exact
  program namespaces and revalidated at apply.
- The structural deletion rule has positive-control tests and runs in
  pre-commit, CI, and release validation.
- MSRV is a mandatory executed gate; absence of its toolchain is a failure.
- Pathname TOCTOU remains documented rather than overstated as solved.

## Active state

Implementation and docs target 0.3.0. Publication, tag, and GitHub Release are
not part of the current push-only task.
