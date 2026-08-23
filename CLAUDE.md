# AGENTS.md — devtrim

Rust CLI (edition 2024, MSRV 1.85, pinned release toolchain 1.98.0). Sync code,
no async runtime, minimal dependencies.

## Commands
- Format: `cargo fmt --all -- --check`
- Lint: `cargo clippy --locked --all-targets --all-features -- -D warnings`
- Deletion boundary: `ast-grep test --skip-snapshot-tests` then `ast-grep scan --config sgconfig.yml`
- Test: `cargo test --locked --all-targets --all-features`
- MSRV: `cargo +1.85.0 test --locked --all-targets --all-features`
- Audit: `cargo audit`
- Build: `cargo build --release --locked --target aarch64-apple-darwin`
- Site/manual: `python3 -m http.server 4173`, then open `/index.html` or `/MANUAL.html`

## Conventions
- Every cleanup category = one file in `src/ops/`, implementing the `Op` trait.
- Findings use typed actions. Confirmation flags may bypass a gate but MUST NOT add or widen actions.
- Apply consumes only exact previewed findings; never rescan for deletion targets after confirmation.
- Unknown activity, ownership, symlink resolution, or owner-command status fails closed.
- Filesystem findings retain an exact internal `PathBuf`; serialized display text is never parsed back into deletion authority.
- Only `safety::validate_path_for_deletion` creates `VerifiedTarget`; only the private sink in `src/ops/mod.rs` consumes it. Raw filesystem deletion anywhere else is a blocking ast-grep violation.
- Apply derives Trash versus permanent mode from the previewed typed `Action`, never from a runtime flag.
- Permanent deletion must be explicit in preview and danger scoring.
- Never add shell-string execution; use `Command::new` with fixed arg arrays only.
- Docker volumes are never pruned. Xcode Archives are never pruned.
- Whole worktrees are never deleted; `leftovers` is report-only.
- `--json` emits exactly one document; failed/partial operations return nonzero.
- User-facing strings are English; size values are estimated logical bytes.
- Keep CSP metadata intact in shipped HTML. Landing page is `index.html` + `styles.css`; demo media lives in `media/`.

## Release
1. Bump `Cargo.toml` and every public version reference.
2. Add the dated `CHANGELOG.md` section; update README, manual, site, security, and agent docs.
3. Run every command above; MSRV must execute and may never be skipped. Also run `bash -n scripts/release.sh`, `shellcheck scripts/release.sh`, and `actionlint`.
4. Run the local autoreview helper in local mode and inspect the final diff.
5. Commit and push a clean tree.
6. `scripts/release.sh <version>` reruns local gates, requires successful CI for the exact release commit, builds/verifies arm64, packages the full Apache-2.0 license, tags, and creates the GitHub release.
