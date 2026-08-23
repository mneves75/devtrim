# Plan 004: Add release-blocking tests, CI, and artifact safeguards

> **Executor instructions**: This plan establishes verification, not product
> breadth. Keep CI small and pin third-party actions to immutable commit SHAs.
>
> **Drift check**: `git diff --stat 8dbbd6c..HEAD -- .github Cargo.toml Cargo.lock rust-toolchain.toml scripts/release.sh LICENSE tests src`

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: plans 001–003
- **Category**: tests / security / dx
- **Planned at**: commit `8dbbd6c`, 2026-08-23
- **Execution status**: DONE; local gates pass. Rust 1.85 MSRV runs in CI (no local rustup), and `scripts/release.sh` refuses to tag without a successful exact-commit CI run.

## Why this matters

Only six helper tests exist; destructive contracts are untested. Format and
strict Clippy currently fail, while the release script does not run either gate
or tests. The release can package stale files or mislabel a host binary as arm64,
and the distributed `LICENSE` is only a URL rather than the Apache-2.0 text.

## Current state

- `cargo test --locked`: six tests pass.
- `cargo fmt --all -- --check`: fails.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: fails.
- `cargo audit --json`: zero vulnerabilities across 97 locked dependencies.
- `scripts/release.sh:16-36`: builds host target, reuses `dist/`, runs no quality
  gates, pushes tag before creating the GitHub release.
- No `.github/workflows/`, Dependabot config, or pinned toolchain exists.
- `LICENSE` contains only a link.

## Scope

**In scope**: focused integration/unit tests, formatting/lint fixes, one CI
workflow, Dependabot, exact Rust toolchain pin, release script hardening, full
Apache-2.0 license text.

**Out of scope**: Homebrew publication, Intel package unless explicitly proven
in this plan, code signing/notarization, SBOM infrastructure beyond an optional
release artifact, self-update, deployment automation outside GitHub Releases.

## Steps

1. Format the code and resolve every strict Clippy finding without broad
   refactors. Remove unused Clap features if confirmed unused.
2. Add focused destructive-contract tests from plans 001–003. Each regression
   must be proven red against the old behavior or otherwise demonstrate that it
   exercises the changed branch. Keep fixtures stdlib-only.
3. Pin the project toolchain in `rust-toolchain.toml` to the current stable
   Rust used for release, with `rustfmt` and `clippy`. Keep `rust-version =
   "1.85"` as the declared MSRV unless tests prove a higher requirement.
4. Add a macOS GitHub Actions workflow for format, strict Clippy, locked tests,
   locked release build, and `cargo audit`. Pin every action by commit SHA and
   set least-privilege permissions. Add weekly Dependabot for Cargo and Actions.
5. Harden `scripts/release.sh`:
   - validate semantic version, clean tree, branch/remote state, changelog, and
     every public version reference;
   - run format, strict Clippy, tests, audit, and locked explicit-target build;
   - delete/recreate only the exact ignored versioned staging path and archive;
   - verify architecture with `file`, smoke-test that binary, list archive
     contents, and verify checksum;
   - do not force tags or overwrite releases;
   - fail with recovery instructions if a remote tag or release step fails.
6. Replace `LICENSE` with the complete Apache License 2.0 text.
7. Run `actionlint`, `shellcheck scripts/release.sh`, and `bash -n` locally.

## Test and verification commands

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo +1.85.0 test --locked --all-targets --all-features
cargo audit
cargo build --release --locked --target aarch64-apple-darwin
file target/aarch64-apple-darwin/release/devtrim
bash -n scripts/release.sh
shellcheck scripts/release.sh
actionlint
```

All must exit 0. If Rust 1.85 cannot resolve the existing locked graph under
resolver 3, stop and report the exact dependency/MSRV conflict rather than
silently raising MSRV.

## Done criteria

- [ ] Format, strict Clippy, tests, MSRV test, and audit pass (MSRV pending exact-commit CI).
- [x] CI runs the same gates on macOS with pinned actions.
- [x] Release script cannot reuse stale package contents or mislabel architecture.
- [x] Full Apache-2.0 license text is packaged.
- [x] Destructive safety regressions have automated coverage.
- [x] `git status --short` shows only intended files.

## STOP conditions

- A GitHub Action cannot be pinned to a verified immutable SHA.
- MSRV failure requires an unapproved version-policy change.
- Release hardening needs credentials or performs any push/tag/release during
  implementation; publication remains parent-owned and separately confirmed.

## Maintenance notes

Keep the release script and CI gates aligned. Any future destructive op must add
at least one regression test at its safety boundary before release.
