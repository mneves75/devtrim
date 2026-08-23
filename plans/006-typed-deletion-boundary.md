# Plan 006: Make invalid deletion targets unrepresentable

## Status

- **Priority**: P0
- **Risk**: HIGH
- **Execution status**: DONE
- **Release**: 0.3.0

## Problem

v0.2.1 repaired physical-path validation, but the shared deletion function still
accepted an ordinary path. Every future operation therefore had to remember to
call the validator correctly. The same review found that owner-reported npm and
Homebrew cache paths accepted arbitrary hidden directories and unresolved
parent components under the home directory.

A safe-deletion tool cannot leave its central invariant as a convention. The
deletion sink must make an unvalidated target unrepresentable.

## Decision

`validate_path_for_deletion` now returns a private `VerifiedTarget`.
Only the private sink in `src/ops/mod.rs` accepts that type. Findings preserve
the exact internal `PathBuf`; their serialized string is presentation only.
Apply derives Trash versus permanent removal from the typed previewed action.

The boundary is enforced twice:

1. deterministic and property-based tests exercise protected roots, managed
   exceptions, parent aliases, owner namespaces, non-UTF-8 identity, forged
   command actions, and partial failures;
2. a positive-control ast-grep rule blocks raw filesystem deletion outside the
   shared sink in pre-commit, CI, and the release script.

## Alternatives considered

1. Port Mole's Bats suite — rejected: roughly 48k lines and GPL-3.0 do not fit
   this Apache-2.0 Rust CLI.
2. Descriptor-relative `openat` deletion — rejected for this release: the core
   fix is refusing ambiguous authority, not racing more effectively.
3. Transactional apply with a rollback journal — rejected: Trash already owns
   recovery for normal deletion.
4. Persist a cross-invocation plan — rejected: showing the plan before the
   prompt fixes the real interaction defect without adding state.
5. Property/fuzz testing — selected in bounded form with deterministic
   `proptest`; full long-running fuzzing remains the strongest deferred test.
6. A user whitelist — rejected: it adds a knob instead of repairing authority.
7. Raise MSRV to 1.98 — rejected: CI can prove 1.85, so unverifiable locally is
   not evidence that 1.85 is unsupported.
8. Rewrite as 0.3.0 — rejected as implementation strategy: the boundary can be
   fixed centrally without replacing working operations. The minor version is
   used because the internal contract and release gates materially changed.
9. Signing and notarization — deferred; important distribution work, unrelated
   to the deletion-boundary defect.
10. Avoid delegation — rejected: independent adversarial review found the
    owner-cache escape and lossy identity path that a single implementation
    pass had missed.

## Advisor evidence

- The Rust API Guidelines recommend newtypes for static distinctions and APIs
  that make invalid states hard to express:
  <https://rust-lang.github.io/api-guidelines/type-safety.html>
- The Rust Fuzz Book documents structured generation and property-based
  strategies for inputs that must satisfy or challenge invariants:
  <https://rust-fuzz.github.io/book/cargo-fuzz/structure-aware-fuzzing.html>
- `cap-std` demonstrates the stronger capability-oriented filesystem direction
  for a future descriptor-relative design:
  <https://docs.rs/crate/cap-std/latest>
- ast-grep's official rule testing supports invalid examples as positive
  controls, so a clean scan is evidence that a rule known to fire found nothing:
  <https://ast-grep.github.io/guide/test-rule.html>

## Five-year review

The likely next improvement is to bind validation to an opened parent directory
and perform descriptor-relative removal, closing the documented pathname TOCTOU
window. That is a larger capability-filesystem change and should ship only with
real adversarial filesystem tests. A continuously fuzzed validator corpus and
signed/notarized artifacts are the other credible next investments.

## Proof

```bash
ast-grep test --skip-snapshot-tests
ast-grep scan --config sgconfig.yml
cargo test --locked --all-targets --all-features
cargo +1.85.0 test --locked --all-targets --all-features
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo audit
cargo build --release --locked --target aarch64-apple-darwin
```
