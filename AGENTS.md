# AGENTS.md — devtrim

Rust CLI (edition 2024, MSRV 1.88, pinned release toolchain 1.98.0). Sync code,
no async runtime, minimal dependencies. Ratatui 0.30 uses Crossterm 0.29 with
default features disabled; do not enable its optional layout cache without a new audit.

## Commands
- Format: `cargo fmt --all -- --check`
- Lint: `cargo clippy --locked --all-targets --all-features -- -D warnings`
- Deletion boundary: `ast-grep test --skip-snapshot-tests` then `ast-grep scan --config sgconfig.yml`
- Test: `cargo test --locked --all-targets --all-features`
- MSRV: `rustup run 1.88.0 cargo test --locked --all-targets --all-features`
- Audit: `cargo audit`
- Video: `cd video && npm ci && npm audit --package-lock-only --audit-level=low && npm run lint && npm run format:check && npm run build`
- Build: `cargo build --release --locked --target aarch64-apple-darwin`
- Site/manual: `python3 -m http.server 4173`, then open `/index.html` or `/MANUAL.html`

## Conventions
- Every cleanup category = one file in `src/ops/`, implementing the `Op` trait.
- Findings use typed actions. Confirmation flags may bypass a gate but MUST NOT add or widen actions.
- Apply consumes only exact previewed findings; never rescan for deletion targets after confirmation.
- Every human apply prints the data-loss notice; every interactive mutation confirms regardless of danger. `-y` skips y/N only; `--yolo` skips all interactive prompts. Operation-specific acknowledgments such as `trash-empty --confirm=<gb>` still apply; JSON remains machine-only.
- Unknown activity, ownership, symlink resolution, owner-command status, configuration fields, or size measurement fails closed.
- Filesystem findings retain an exact internal `PathBuf`; serialized display text is never parsed back into deletion authority.
- Only `safety::validate_path_for_deletion` creates `VerifiedTarget`; only the private sink in `src/ops/mod.rs` consumes it. Raw filesystem deletion anywhere else is a blocking ast-grep violation.
- Apply derives Trash versus permanent mode from the previewed typed `Action`, never from a runtime flag.
- Permanent deletion must be explicit in preview and danger scoring.
- Never add shell-string execution; use `Command::new` with fixed arg arrays only.
- Only `Finding::command` creates command authority from the closed `CommandAuthority` enum; apply verifies that capability and its serialized `Action` before fixed-argument execution.
- Docker volumes are never pruned. Xcode Archives are never pruned.
- Whole worktrees are never deleted; `leftovers` is report-only.
- `--json` emits exactly one document; failed/partial operations return nonzero.
- User-facing strings are English; size values are estimated logical bytes.
- `src/tui.rs` is a presentation adapter over existing `Op` scan/apply owners. It must not duplicate scanners, deletion logic, or danger policy. TUI apply requires a matching typed approval; CLI bypass flags never pre-authorize it.
- Scanner diagnostics go through `Ctx`: explicit CLI commands may render stderr, while the TUI captures, escapes, and retains them in its own state.
- Bare `devtrim` opens the TUI only with interactive stdin and stdout. Non-TTY automation uses explicit subcommands; `--json` remains exactly one document.
- Keep CSP metadata intact in shipped HTML. Landing page is `index.html` + `styles.css`; demo media lives in `media/`.

## Release
1. Bump `Cargo.toml` and every version reference packaged with the artifact.
2. Add the dated `CHANGELOG.md` section; update README, manual, security, and agent docs. Keep the production landing page on the live stable version during beta staging.
3. Run every command above; MSRV must execute and may never be skipped. Also run a real PTY TUI cancel flow against a disposable home, `bash -n scripts/release.sh`, `shellcheck scripts/release.sh`, `actionlint`, Gitleaks, and TruffleHog.
4. Run the local autoreview helper in local mode and inspect the final diff.
5. Commit and push a clean tree.
6. GitHub immutable releases must be enabled. Stage with `scripts/release.sh <version>-beta<N>`; every retry uses a new `N`. The script reruns local gates, requires successful exact-commit CI, and pushes an annotated tag.
7. The hosted release workflow builds/verifies arm64, packages the full Apache-2.0 license, signs artifact provenance, publishes an immutable prerelease, and verifies the remote asset.
8. After staging verification, promote the same commit with `scripts/release.sh <version>`. Production reuses the exact highest verified beta artifact and checksum without rebuilding. After verification, update the production landing page and open the next patch `Unreleased` section.

## Apple Platforms
- For Swift or iOS/iPadOS 26 code, consult `/Applications/Xcode.app/Contents/PlugIns/IDEIntelligenceChat.framework/Versions/A/Resources/AdditionalDocumentation`.
- For Swift or iOS/iPadOS 27 beta code, consult `/Applications/Xcode-beta.app/Contents/PlugIns/IDEIntelligenceChat.framework/Versions/A/Resources/AdditionalDocumentation`.
- For Swift/SwiftUI work, use `~/dev/Skills/XCODE_AGENT_SKILLS` plus the SwiftUI Expert, Swift Concurrency, and Xcode Build Optimization skills. These are not applicable to the current Rust source.
