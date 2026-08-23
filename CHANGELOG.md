# Changelog

All notable changes to devtrim. Format follows Keep a Changelog; versioning is semver.

## [0.2.1] - 2026-08-23

### Added
- macOS CI for formatting, strict Clippy, tests, Rust 1.85 MSRV coverage, dependency audit, and explicit arm64 release builds
- Regression coverage for physical-path validation, symlinked Trash, fail-closed Git/toolchain checks, immutable `node_modules` plans, JSON output, and owner-command failures
- `SECURITY.md` threat model, safety design, reporting guidance, and known limitations
- Exact Rust 1.98.0 release-toolchain pin and weekly Cargo/GitHub Actions Dependabot checks

### Changed
- `--json` now emits one response envelope per invocation, including empty, partial, and failed results
- Unprovable state degrades to a skipped target with a warning instead of failing an entire scan: repos whose Git activity cannot be read, and hosts where `simctl` is unavailable
- `node-modules` applies exact previewed paths, prunes dependency/`.git` traversal, and requires conclusive Git activity
- `leftovers` is report-only because worktree or mission staleness cannot be proven safely
- Simulator cleanup reads `simctl` JSON and only deletes unavailable devices; `--yolo` bypasses confirmation but never adds erase-all
- Swift toolchains are eligible only when `swift-latest` and every other symlink reference resolve safely
- Release packaging now requires successful exact-commit CI/MSRV evidence, runs local gates, builds/verifies arm64 explicitly, starts from clean artifacts, and includes the full Apache-2.0 license

### Fixed
- Protected paths could be permanently deleted through a symlinked ancestor
- `trash-empty` mutated without `--apply`, accepted the wrong documented flag spelling, and bypassed the shared deletion owner
- Config `~/` roots were not expanded or physically resolved, while malformed config silently fell back to another root
- Failed Docker/cache/simulator actions could return success or inflate reclaimed-byte summaries
- Human apply prompts appeared before the reviewed findings, and JSON TTY prompts polluted stdout
- `--shred` previews and notes incorrectly described permanent deletion as recoverable Trash
- Aggregate actionable size did not consistently drive danger escalation
- `active_days = 0` in config silently disabled the active-repo guard; it now clamps to 1
- The Docker finding described `image prune -a` as "unused images" without stating that untagged local builds are removed too
- `scripts/release.sh` compared against a possibly stale remote ref and ignored a failed remote-tag query

### Security
- Deletion now validates both literal policy and the canonical existing parent immediately before mutation; resolution is deny-only
- Unknown ownership/activity and failed external commands fail closed
- Cache roots reported by `npm` and `brew` are user-controlled configuration and are now refused unless they resolve inside a home cache location
- Git activity checks neutralize repository-controlled configuration (`core.fsmonitor`, `core.hooksPath`) when inspecting untrusted clones
- `cargo audit`, Gitleaks, and TruffleHog found no dependency vulnerabilities or committed secrets before release

[0.2.1]: https://github.com/mneves75/devtrim/releases/tag/v0.2.1

## [0.2.0] - 2026-08-22

### Added
- Landing page + GitHub Pages site (mneves75.github.io/devtrim) with lazy-loaded demo video and visual transcript
- 12s product demo video rendered with Remotion (`media/demo.mp4`)
- `scripts/release.sh` — reproducible release automation: clean-tree and existing-tag guards, locked release build, zip + SHA256SUMS, tag, GitHub release from changelog
- PRODUCT.md / DESIGN.md design-system docs; AGENTS.md + CLAUDE.md agent guidance

### Fixed
- Release-notes extraction used an awk character class, truncating published release bodies (v0.1.0 repaired retroactively)
- Landing stylesheet blocked by its own CSP (`style-src` now allows self)
- Reveal-on-scroll left content invisible without JavaScript (`noscript` fallback)

### Security
- CSP (`default-src 'none'`, `style-src 'self' 'unsafe-inline'`) + `referrer: no-referrer` on all shipped HTML

[0.2.0]: https://github.com/mneves75/devtrim/releases/tag/v0.2.0

## [0.1.0] - 2026-08-21

First public release.

### Added
- `scan` — read-only report across all reclaimable categories, `--json` output
- `clean caches` — HuggingFace / npm / Homebrew / uv / node caches (Trash-first)
- `clean node-modules` — stale-repo sweep with active-repo guard (commit recency)
- `clean simulators` — delete unavailable devices; erase-all behind `--yolo`
- `clean xcode` — DeviceSupport + DerivedData (Archives exempt by design)
- `clean docker` — unused images + build cache; volumes never touched
- `clean toolchains` — old swift.org toolchains, `swift-latest` preserved
- `clean leftovers` — agent scratch dirs and `.supergoal` artifacts
- `icloud` — upload status for large queued iCloud Drive files
- `trash-empty --confirm=<gb>` — typed-size acknowledgment gate
- Safety core: danger scoring 1–10 with size escalation, protected-path denylist,
  Trash-first deletion, non-TTY refusal, preview-by-default (`--apply` to mutate)
- 6 unit tests covering scanner helper logic
- MANUAL.html: single-file interactive manual (dark/light, CSP hardened)
- Landing page (GitHub Pages) + release automation script

### Security
- CSP (`default-src 'none'`) and `referrer: no-referrer` on shipped HTML
- No network access at runtime except package-manager subprocesses
[0.1.0]: https://github.com/mneves75/devtrim/releases/tag/v0.1.0
