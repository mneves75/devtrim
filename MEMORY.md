# Project Memory

## Current state

devtrim 0.3.1 is the immutable production release. A 0.3.2 release candidate
is prepared on `master`; its next release step is an immutable, attested
`v0.3.2-beta1` followed by exact-byte production promotion after approval.

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

## Next boundary

Fuzz the path/config/size validators, then evaluate Apple signing and
notarization. The pathname TOCTOU assumption remains explicit.
