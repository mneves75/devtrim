# AGENTS.md — devtrim

Rust CLI (edition 2024, MSRV 1.88, pinned release toolchain 1.98.0). Sync code,
no async runtime, minimal dependencies. Ratatui 0.30 uses Crossterm 0.29 with
default features disabled; do not enable its optional layout cache without a new audit.

## Commands
- Format: `cargo fmt --all -- --check`
- Lint: `cargo clippy --locked --all-targets --all-features -- -D warnings`
- Structural lint: `ast-grep test --skip-snapshot-tests` then `ast-grep scan --config sgconfig.yml`
- Test: `cargo test --locked --all-targets --all-features`
- MSRV: `rustup run 1.88.0 cargo test --locked --all-targets --all-features`
- Audit: `cargo audit`
- Fuzz (local release gate, nightly): with the nightly toolchain's bin dir first on PATH (`PATH="$(dirname "$(rustup which cargo --toolchain nightly)"):$HOME/.cargo/bin:$PATH"`), run `cargo fuzz run <target> -- -max_total_time=60` for each target in `fuzz/fuzz_targets/`. A standalone stable cargo earlier on PATH breaks cargo-fuzz's inner build.
- Video: `cd video && npm ci && npm audit --package-lock-only --audit-level=low && npm run lint && npm run format:check && npm run build`
- Build: `cargo build --release --locked --target aarch64-apple-darwin`
- Site/manual: `python3 -m http.server 4173`, then open `/index.html` or `/MANUAL.html`

## Conventions
- Every cleanup category = one file in `src/ops/`, implementing the `Op` trait.
- Findings use typed actions. Confirmation flags may bypass a gate but MUST NOT add or widen actions.
- Apply consumes only exact previewed findings; never rescan for deletion targets after confirmation.
- Every human apply prints the data-loss notice; every interactive mutation confirms regardless of danger. `-y` skips y/N only; `--yolo` skips all interactive prompts. Operation-specific acknowledgments such as `trash-empty --confirm=<gb>` still apply; JSON remains machine-only.
- Global flags are capability-scoped: commands reject mutation flags they cannot honor. `scan --shred` may change previewed actions; Docker and simulator cleanup reject `--shred` because they execute typed commands rather than filesystem deletion actions; `trash-empty` accepts apply/confirmation flags but rejects `--shred`; report-only commands accept no mutation flags.
- Unknown activity, ownership, symlink resolution, owner-command status, configuration fields, or size measurement fails closed.
- Filesystem findings retain an exact internal `PathBuf`; serialized display text is never parsed back into deletion authority.
- Only `safety::validate_path_for_deletion` creates `VerifiedTarget`; only the private sink in `src/ops/mod.rs` consumes it. Raw filesystem deletion anywhere else is a blocking ast-grep violation.
- Apply derives Trash versus permanent mode from the previewed typed `Action`, never from a runtime flag.
- Permanent deletion must be explicit in preview and danger scoring.
- Never add shell-string execution. Process execution uses a fixed program; a dynamic argument is allowed only when a closed typed authority validates and carries it.
- Only `Finding::command` creates command authority from the closed `CommandAuthority` enum; apply verifies that capability, its validated arguments, and its serialized `Action` before execution. Docker authority binds an absolute local Unix-socket endpoint; simulator authority binds one exact UDID.
- Docker volumes are never pruned. Xcode Archives are never pruned.
- Xcode and Swift toolchain apply must reassert the scanner's exact direct-child target shape before calling the shared deletion sink.
- Every directory deletion preflights device boundaries and nested Git repository/worktree markers before either Trash or permanent mutation.
- `artifacts` matches only its closed corroborated-name list plus valid `CACHEDIR.TAG` signatures; ambiguous names (`build`, `dist`, `out`, `vendor`, `bin`, `obj`, `coverage`) are never added. Corroboration is re-verified at apply.
- Every deletion and typed command writes a write-ahead journal record (attempt before, result after) via `src/journal.rs`, synced per record; an unwritable journal blocks apply. Rotation is shift-and-rename at journal-open time only, never truncation, never mid-apply. `history` is read-only, reads rotated files, and emits its own single JSON document.
- Findings capture preview-time device/inode identity; the sink refuses actionable filesystem findings whose identity is missing or drifted and verifies it through a cap-std parent-directory handle. Permanent deletion continues through that handle. Trash remains path-based after the identity check (no fd-anchored macOS Trash API) — keep that disclosure accurate.
- `largest` is report-only visibility: `Action::Info` findings, lenient traversal with disclosed skip counts, never deletion authority, no TUI entry.
- Demo video: edit `video/src/DevtrimDemo.tsx`, render with `npx remotion render DevtrimDemo ../media/demo.mp4 --overwrite --muted` from `video/`, and verify a menu frame plus a single video stream before shipping.
- Config `protect` entries are deny-only, enforced inside `validate_path_for_deletion`, and fail closed on malformed input. No flag may bypass them.
- Liveness probes (`build_process_cwds`, `xcodebuild_running`) use fixed argv `pgrep`/`lsof`; probe failure blocks the affected findings and surfaces as an error, never a silent pass.
- `completions` and `manpage` write plain stdout and refuse `--json` with the standard error envelope.
- AGENTS.md and CLAUDE.md must stay byte-identical (release gate compares them).
- Whole worktrees are never deleted; `leftovers` is report-only.
- `--json` emits exactly one document; failed/partial operations return nonzero.
- User-facing strings are English; size values are estimated logical bytes.
- `src/tui.rs` is a presentation adapter over existing `Op` scan/apply owners. It must not duplicate scanners, deletion logic, or danger policy. TUI apply requires a matching typed approval; CLI bypass flags never pre-authorize it.
- Scanner diagnostics go through `Ctx`: explicit CLI commands may render stderr, while the TUI captures, escapes, and retains them in its own state.
- Complete human-facing actions, findings, errors, and notes are terminal-escaped at their final rendering sink; JSON data remains unmodified.
- Bare `devtrim` opens the TUI only with interactive stdin and stdout. Non-TTY automation uses explicit subcommands; `--json` remains exactly one document.
- Keep CSP metadata intact in shipped HTML. Landing page is `index.html` + `styles.css`; demo media lives in `media/`.
- Code review reads `CODING_STANDARDS.md`. Every bullet in this section is a hard standard, citable as `CLAUDE.md § Conventions`.

## Release
1. Bump `Cargo.toml` and every version reference packaged with the artifact.
2. Add the dated `CHANGELOG.md` section; update README, manual, security, and agent docs. Keep the production landing page on the live stable version during beta staging.
3. Run every command above; MSRV must execute and may never be skipped. Also run a real PTY TUI cancel flow against a disposable home, shell syntax and ShellCheck for every script under the release policy, `actionlint`, Gitleaks, and TruffleHog.
4. Run the local autoreview helper in local mode and inspect the final diff.
5. Commit and push a clean tree.
6. GitHub immutable releases must be enabled. Stage with `scripts/release.sh <version>-beta<N>`; every retry uses a new `N`. The credential-bearing script performs provenance-only preflight and pushes an annotated tag; read-only hosted jobs rerun every gate before the no-checkout publisher receives release-write/OIDC authority.
7. The hosted release workflow builds/verifies arm64, packages the full Apache-2.0 license, signs artifact provenance, publishes an immutable prerelease, and verifies the remote asset.
8. After staging verification, promote the same commit with `scripts/release.sh <version>`. Production reuses the exact highest verified beta artifact and checksum without rebuilding, then automatically runs the idempotent Homebrew closeout. That helper re-verifies release provenance, changes only the tap formula, pushes normally, locks validation to the published tap commit, upgrades/tests the existing `/opt/homebrew` installation, and requires it to be the sole visible `devtrim`. Beta never invokes it; a failed closeout resumes with `scripts/update-homebrew.sh <version>` without moving the tag or release.
9. After verification, update the production landing page and open the next patch `Unreleased` section.

## Apple Platforms
- For Swift or iOS/iPadOS 26 code, consult `/Applications/Xcode.app/Contents/PlugIns/IDEIntelligenceChat.framework/Versions/A/Resources/AdditionalDocumentation`.
- For Swift or iOS/iPadOS 27 beta code, consult `/Applications/Xcode-beta.app/Contents/PlugIns/IDEIntelligenceChat.framework/Versions/A/Resources/AdditionalDocumentation`.
- For Swift/SwiftUI work, use `~/dev/Skills/XCODE_AGENT_SKILLS` plus the SwiftUI Expert, Swift Concurrency, and Xcode Build Optimization skills. These are not applicable to the current Rust source.
