# Plan 005: Release devtrim 0.2.1 with synchronized documentation

> **Executor instructions**: Prepare all local release metadata and validation.
> Do not push, tag, publish, or create a GitHub release; those external writes
> remain with the parent and require immediate confirmation.
>
> **Drift check**: `git diff --stat 8dbbd6c..HEAD -- Cargo.toml Cargo.lock CHANGELOG.md README.md MANUAL.html index.html AGENTS.md CLAUDE.md PRODUCT.md DESIGN.md scripts/release.sh`

## Status

- **Priority**: P1
- **Effort**: S
- **Risk**: MED
- **Depends on**: `plans/004-release-gates.md`
- **Category**: docs / release
- **Planned at**: commit `8dbbd6c`, 2026-08-23
- **Execution status**: IN PROGRESS; independent reviews and two autoreview passes are clean, all local gates pass, and the release commit is prepared. Publication follows CI.

## Why this matters

The release must describe actual behavior, not the earlier intended behavior.
Public version references, safety gates, JSON examples, config behavior, Trash
syntax, test claims, and artifact instructions must agree before publication.
The advisor panel recommends patch version `0.2.1` because this restores existing
safety and correctness promises without adding product scope.

## Current state

- Version `0.2.0` appears in Cargo metadata, README download/install links,
  `MANUAL.html`, and `index.html`.
- `CHANGELOG.md` claims integration coverage that does not exist.
- Manual shows invalid `devtrim clean trash-empty` / `--confirm` behavior and
  overstates allocated-block sizing.
- Agent docs omit format, Clippy, audit, and CI gates.

## Scope

**In scope**: `Cargo.toml`, regenerated `Cargo.lock`, `CHANGELOG.md`, `README.md`,
`MANUAL.html`, `index.html`, `AGENTS.md`, `CLAUDE.md`, and any security/release
doc introduced by plan 004.

**Out of scope**: visual redesign, demo rerender, new product features, release
publication, GitHub issue/PR creation.

## Steps

1. Bump package version to `0.2.1` and refresh `Cargo.lock` without unrelated
   dependency upgrades.
2. Add a dated Keep a Changelog section covering only shipped changes:
   physical deletion validation, apply/preview parity, fail-closed eligibility,
   truthful JSON/outcomes, tests/CI/release hardening, and docs corrections.
3. Update every public version/download reference. Use a deterministic grep to
   prove no stale current-version reference remains except historical changelog
   entries and old release links that are intentionally historical.
4. Correct user-facing commands and contracts: top-level `trash-empty`, exact
   confirmation flag, mandatory `--apply`, `-y`/`--yolo` semantics, logical or
   estimated size wording, valid single-document JSON examples, config-root
   behavior, and removed/deferred cleanup targets.
5. Update `AGENTS.md` and `CLAUDE.md` with exact release gates and the physical
   deletion/immutable-plan conventions. Keep both files identical.
6. Run all plan-004 gates plus a local static-site smoke check. Confirm CSP meta
   remains intact in both shipped HTML files.
7. Inspect the complete diff and run the required local autoreview helper in
   local mode. Address only verified in-scope findings and rerun affected gates.
8. Stage and create one conventional local commit. Do not push/tag/release.

## Verification

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo +1.85.0 test --locked --all-targets --all-features
cargo audit
cargo build --release --locked --target aarch64-apple-darwin
bash -n scripts/release.sh
shellcheck scripts/release.sh
actionlint
cmp -s AGENTS.md CLAUDE.md
rg -n '0\.2\.0|clean trash-empty|--confirm-gb|integration coverage' README.md MANUAL.html index.html AGENTS.md CLAUDE.md Cargo.toml
```

Expected: all commands pass; final grep returns only deliberately historical
changelog/release material or no matches in current docs.

## Done criteria

- [x] Version is `0.2.1` everywhere current.
- [x] Changelog and docs match tested behavior.
- [x] AGENTS/CLAUDE guidance is synchronized.
- [ ] Full validation and autoreview are clean or findings are dispositioned.
- [ ] One local commit exists and working tree is clean.
- [x] No push, tag, or release has occurred in this plan.

## STOP conditions

- Any required gate fails twice after a reasonable in-scope fix.
- A reviewer requests a product-scope expansion rather than a release blocker.
- Publication requires credentials or external writes; return control to parent.

## Maintenance notes

After the parent receives immediate confirmation, it may push the commit and run
`scripts/release.sh 0.2.1`, then verify the GitHub release assets/checksum.
