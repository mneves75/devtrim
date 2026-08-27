# Changelog

All notable changes to devtrim. Format follows Keep a Changelog; versioning is semver.

## [0.6.1] - Unreleased

[0.6.1]: https://github.com/mneves75/devtrim/compare/v0.6.0...HEAD

## [0.6.0] - 2026-08-27

### Added
- Identity-verified, parent-anchored deletion: every finding records its target's device/inode at preview, and the sink re-verifies that identity through an open parent-directory handle (cap-std) and deletes through the same handle — a target renamed or swapped after preview is refused; Trash calls re-verify identity immediately before the path-based call, with the residual window documented
- `devtrim largest [--top N]`: read-only ranking of the biggest directories under the scan roots, with skipped-entry disclosure and the standard one-document JSON envelope
- Journal rotation: writer-owned shift-and-rename at startup only (10 MiB, keep 3), never truncation, never mid-apply; history reads rotated files so attempt/result pairs cannot split, and records are synced to disk before an apply reports success
- The release workflow explicitly ad-hoc signs and verifies the built binary before packaging; Developer ID signing and notarization are documented as a runbook pending CI credentials
- The structural deletion lint now also blocks method-call deletion primitives (`.remove_file`, `.remove_dir_all`, …) outside the sink, with positive controls

### Changed
- The landing demo video shows the current interface (Build artifacts entry, `i` for iCloud status) with a version-neutral caption

[0.6.0]: https://github.com/mneves75/devtrim/compare/v0.5.0...v0.6.0

## [0.5.0] - 2026-08-26

### Added
- `devtrim clean artifacts`: multi-ecosystem build artifacts (`target`, `.venv`, `__pycache__`, tool caches, `Pods`, `.gradle`, `.next`-family, `.build`, `.dart_tool`, `.zig-cache`, and valid `CACHEDIR.TAG` directories) in conclusively stale Git repos, each requiring ecosystem corroboration before it can even be previewed; ambiguous names such as `build`, `dist`, `vendor`, `bin`, and `obj` are deliberately never matched
- Write-ahead apply journal at `~/.local/state/devtrim/journal.jsonl` (`$XDG_STATE_HOME` honored): an attempt record before every deletion or fixed-argv command and a result record after; an unwritable journal blocks the apply, and `devtrim history [--limit N] [--json]` renders records, flagging attempts without results as interrupted
- `protect` config list: user-declared paths that the deletion sink refuses and previews filter out; entries expand `~`, must be absolute, and malformed entries fail closed
- Liveness guards: `node-modules` and `artifacts` skip and refuse repos owning the working directory of a running build or package process, and `xcode` refuses DerivedData while `xcodebuild` runs; a probe that cannot complete blocks instead of passing
- `devtrim completions <bash|zsh|fish>` and `devtrim manpage`; both refuse `--json` with the standard error envelope
- Fuzz targets for the deletion-path validator, path normalizer, Docker size parser, and config parser join the documented local release gates
- Homebrew tap `mneves75/devtrim` installs the attested release binary with generated completions and man page

### Security
- Independent pre-release reviews found and fixed a set of `protect` weaknesses before any release shipped: matching is now Unicode-normalization-insensitive (NFC config entries protect NFD on-disk names, the common macOS state), deleting an ancestor of a protected entry is refused, symlinked entries also match their resolved location, unresolved entries warn instead of failing quietly, and Trash purge previews filter protected items
- The stale-repository gate clears ambient `GIT_DIR`/`GIT_WORK_TREE`-style variables so an inherited environment cannot make an active repo read as stale, and the ambiguous artifact-name denylist matches ASCII case variants (`Build`, `DIST`) before any positive evidence is considered
- A journal write that fails after a successful deletion keeps the summary truthful while surfacing the failure in `errors` with a nonzero exit, and `history` exits nonzero when journal lines were skipped so a partial audit is never silent

### Changed
- The TUI menu adds Build artifacts and opens entries by their listed key; iCloud status moved to `i`
- Stdout writes tolerate a closed pipe, so `devtrim … | head` ends quietly instead of aborting
- The landing-page demo video shows the v0.4.0 Ratatui interface with a matching transcript and caption; the file stays video-only with no silent audio stream

[0.5.0]: https://github.com/mneves75/devtrim/compare/v0.4.0...v0.5.0

## [0.4.0] - 2026-08-25

### Added
- Original Ratatui terminal interface for interactive scan, preview, Trash-first apply, explicit permanent mode, iCloud status, and Trash purge; bare `devtrim` opens it only when stdin and stdout are terminals
- Deterministic TestBackend coverage plus manual PTY verification for navigation, non-color risk labels, small terminals, warnings, non-TTY behavior, and confirmation state

### Changed
- CLI and TUI now derive confirmation requirements from one safety policy; automation subcommands and the one-document JSON contract remain unchanged
- Ratatui 0.30.2 and Crossterm 0.29 raise the MSRV from Rust 1.85 to 1.88; Ratatui's optional layout cache stays disabled
- Release validation now installs the demo-video lockfile exactly, audits its npm graph, and runs lint, formatting, and production-build gates

### Fixed
- Removed the demo video's entirely silent audio stream, added an explicit silent-video caption linked to its transcript, and made scrollable install commands keyboard-focusable
- The TUI now blocks hidden confirmation input below 64×18, retains scanner diagnostics inside the alternate screen, and lets users scroll long outcomes to partial-apply errors
- The landing-page “Read the manual” button now opens `MANUAL.html` instead of the in-page command section
- Protected system roots, their descendants, and protected user roots reject ASCII case variants at the shared deletion boundary

### Security
- TUI apply requires an internal approval capability carrying the exact current preview and calculated danger; CLI bypass flags are rejected by the TUI and cannot pre-authorize an action
- Permanent plans require typed size confirmation, while Trash purge requires the exact `PURGE <gb>` acknowledgment before the existing Trash and deletion boundaries execute
- Replaced transitive `lru 0.12.5` after RustSec reported two soundness advisories, including potential use-after-free; Ratatui 0.30 resolves the patched `lru 0.18.2`
- `trash-empty` now previews each exact child and applies only that immutable set, so items arriving in Trash after preview are preserved
- Terminal-facing findings, errors, and outcome notes escape control and bidirectional-control characters while internal `PathBuf` deletion identity stays unchanged
- Release builds run with read-only repository and Actions permissions; a separate publisher receives only packaged inputs and alone holds release and OIDC authority
- Release retries safely replace the intermediate handoff artifact and refresh immutable state inside the publisher, so a post-publication verification retry verifies the existing release instead of attempting to recreate it
- The structural deletion rule exempts only the typed sink and test cleanup scopes, with a positive control proving a second sink is rejected
- External command findings require a private closed command authority that must match the exact serialized preview before fixed-argument execution
- Upgraded the demo-video ESLint toolchain past GHSA-xffm-g5w8-qvg7, added weekly npm Dependabot coverage, and replaced permissive inline-script CSP directives with exact SHA-256 hashes

[0.4.0]: https://github.com/mneves75/devtrim/compare/v0.3.2...v0.4.0

## [0.3.2] - 2026-08-24

### Added
- Human apply displays an AS-IS data-loss warning, and CLI help plus public documentation explain risk, backups, and manual macOS permission decisions

### Changed
- Every interactive mutation now confirms: `-y` skips normal y/N prompts and `--yolo` skips interactive prompts, while operation-specific acknowledgments such as `trash-empty --confirm=<gb>` remain mandatory

### Fixed
- Manual layout keeps the table of contents and document content in their intended desktop columns, with keyboard access to scrollable examples
- Production promotion selects beta tags without generating invalid jq regex escapes
- Actionable-size and apply-summary aggregation saturate instead of wrapping, so extreme totals cannot lower danger or misstate results
- iCloud allocated-size inspection now fails on metadata or arithmetic errors instead of silently presenting a partial value
- The shared protected-system boundary explicitly rejects `/bin`, `/sbin`, and `/var` aliases

[0.3.2]: https://github.com/mneves75/devtrim/releases/tag/v0.3.2

## [0.3.1] - 2026-08-23

### Fixed
- Directory sizing now fails closed on traversal, metadata, or overflow errors and never follows a symlink supplied as the scan root
- Configuration files reject unknown fields instead of silently accepting misspelled safety settings
- Docker disk-usage parsing recognizes documented petabyte and exabyte units while rejecting missing, unsupported, negative, non-finite, and out-of-range values

### Security
- Release preparation now actually executes Gitleaks and TruffleHog before tagging, matching the documented release contract
- The privileged local preflight queries GitHub's immutable-release setting directly, while hosted jobs verify the published release is actually immutable
- Actionable scans refuse to build a cleanup plan when their logical-byte measurement cannot be completed truthfully

[0.3.1]: https://github.com/mneves75/devtrim/releases/tag/v0.3.1

## [0.3.0] - 2026-08-23

### Added
- A private `VerifiedTarget` capability at the shared filesystem deletion sink; unvalidated paths cannot reach physical removal
- Deterministic property tests for protected roots, managed `~/Library` exceptions, cleaned parent aliases, and non-UTF-8 target identity
- A positive-control ast-grep rule, pre-commit hook, CI step, and release gate that reject direct filesystem deletion outside the shared sink

### Changed
- Findings retain exact internal `PathBuf` identity while serialized paths remain presentation-only
- Apply derives Trash versus permanent deletion from each previewed typed action and reports successful work before a later failure
- npm and Homebrew owner-reported cache paths are constrained to exact program namespaces and revalidated immediately before apply
- Release validation now fails when the Rust 1.85 MSRV gate cannot execute instead of silently skipping it
- Release automation now builds immutable `-betaN` prereleases on hosted CI with signed provenance, then promotes the exact verified beta artifact to production without rebuilding

### Security
- Fixed an owner-cache protected-path bypass that accepted arbitrary hidden directories and parent-component escapes under the user home
- Added forged-action tests proving Docker volumes and simulator erase-all cannot cross command allowlists
- Release requires `cargo audit`, Gitleaks, TruffleHog, full tests, strict Clippy, MSRV tests, an arm64 build, and independent autoreview on the exact commit before publication
- The remaining pathname TOCTOU limitation is explicit: validation does not hold a directory descriptor across deletion and assumes no hostile concurrent local mutation

[0.3.0]: https://github.com/mneves75/devtrim/releases/tag/v0.3.0

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
