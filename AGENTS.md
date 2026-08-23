# AGENTS.md — devtrim

Rust CLI (edition 2024). Sync code, no async runtime. Minimal deps.

## Commands
- Build: `cargo build --release`
- Test: `cargo test`
- Manual: open `MANUAL.html`

## Conventions
- Every cleanup category = one file in `src/ops/`, implementing the `Op` trait.
- Findings carry a danger score (1–10); apply-time gates live in `src/safety.rs`.
- Filesystem deletions MUST go through `ops::remove_path` (protected-path check + Trash-first).
- Never add shell-string execution; use `Command::new` with arg arrays only.
- Docker volumes are never pruned. Xcode Archives are never pruned.
- User-facing strings in English; docs single-file HTML with CSP meta intact.

## Release
- `scripts/release.sh <version>` builds release binary, zips with SHA256SUMS,
  tags `v<version>`, creates the GitHub release from CHANGELOG.md notes.
