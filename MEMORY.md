# Project Memory

## Current state

devtrim 0.3.2 is the immutable production release. It promotes the exact
attested `v0.3.2-beta1` arm64 archive from commit `1eb6218`; 0.4.0 is the
active development line with an unreleased Ratatui interface.

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

## Next boundary

The 0.4.0 implementation has completed local review and release validation but
remains unreleased. Stage it only through a new immutable beta when explicitly
requested. Fuzzing, signing/notarization, and the documented pathname TOCTOU
limitation remain follow-ups rather than hidden claims.
