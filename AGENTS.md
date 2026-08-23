# AGENTS.md — devtrim

Rust CLI (edition 2024). Sync code, no async runtime. Minimal deps.

## Commands
- Build: `cargo build --release --locked`
- Test: `cargo test`
- Site/manual: `python3 -m http.server 4173`, then open `/index.html` or `/MANUAL.html`

## Conventions
- Every cleanup category = one file in `src/ops/`, implementing the `Op` trait.
- Findings carry a danger score (1–10); apply-time gates live in `src/safety.rs`.
- Filesystem deletions MUST go through `ops::remove_path` (protected-path check + Trash-first).
- Never add shell-string execution; use `Command::new` with arg arrays only.
- Docker volumes are never pruned. Xcode Archives are never pruned.
- User-facing strings in English; keep CSP metadata intact in shipped HTML.
- Landing page: `index.html` + `styles.css`; demo media lives in `media/`.

## Release
1. Bump `Cargo.toml` and every public version reference.
2. Add the dated `CHANGELOG.md` section; update README and agent docs.
3. Run tests, commit, and push a clean tree.
4. `scripts/release.sh <version>` builds the locked release, zips it with SHA256SUMS, tags `v<version>`, and creates the GitHub release from changelog notes.
