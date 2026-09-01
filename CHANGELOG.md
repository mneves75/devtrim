# Changelog

All notable changes to devtrim. Format follows Keep a Changelog; versioning is semver.

## [Unreleased]

### Added
- `devtrim status` reports read-only machine vitals — uptime, load, memory, disk, battery, thermals, cumulative network, busiest processes — and a health score that names every input it could not read instead of scoring over the gap. Each value comes from a fixed-argv system tool through a parser that fails closed on malformed input; memory used is stated as `active + wired + compressed`, because counting reclaimable inactive pages reports a healthy machine at 96%
- `devtrim analyze [path]` is an interactive, read-only disk explorer: it measures each child on a worker thread and streams results in as they land, so a directory that takes minutes to size never freezes the interface, and leaving a directory cancels its in-flight walk. Symbolic links are reported at their own size rather than followed, a different device is never entered, and unreadable entries are disclosed as `(partial)` lower bounds. `--json` emits one document; every mutation flag is rejected
- The terminal interface honors `NO_COLOR`, degrading every style to a modifier that preserves the same distinction — the danger ladder stays ordered as dim, plain, bold, bold+reversed — so the interface remains usable with color stripped entirely
- `?` opens a full keybinding reference over any screen, deliberately except the confirmation prompt, where a second overlay would obscure the plan being approved; the footer keeps only the few keys that apply to the current screen
- `clean installers` reclaims downloaded installer archives (`dmg`, `pkg`, `mpkg`, `iso`, `xip`) left as direct children of `Downloads` and `Desktop` after the configured active window, refusing symlinks, nested copies inside extracted project trees, and any target outside those two directories at apply time

### Changed
- Terminal styling moved from 30 inline color literals to semantic tokens in `src/theme.rs`, so call sites name what a span means and one module decides how it looks; colors remain named ANSI rather than RGB so they keep resolving through the user's own terminal theme
- Every tracked `*.sh` plus the pre-commit hook now pass `shellcheck` before local commits and in CI through one fail-closed, NUL-safe helper; CI installs the official ShellCheck 0.11.0 arm64 asset only after checksum verification
- CI and non-Intel release jobs move from the deprecated `macos-14` image to the supported `macos-15` arm64 image with exact runner-policy checks; the deterministic x86 release gate remains on `macos-15-intel`
- Ordinary PR/main CI now installs checksum-verified arm64 Gitleaks 8.30.1 and TruffleHog 3.97.1, proves Gitleaks detects a non-allowlisted synthetic PAT, then runs the same full-history secret scans that release gates already run

### Fixed
- `clean docker` under-reported reclaimable space by roughly 7x because `docker system df` measures only inside the guest: the host-side OrbStack/Docker Desktop VM disk image is now disclosed as a report-only finding measured in allocated blocks, with a note stating that pruning frees guest space but never shrinks that sparse file until the runtime compacts it
- The Docker VM disk image is now reported even when the daemon is not running, which is the one state where it is invisible to `docker` and still occupying the host; a refused remote endpoint and a malformed `docker` response remain hard errors
- Artifact scanning and apply now both refuse targets below every ASCII-case variant of `node_modules`, closing the sibling dependency-namespace deletion path with an end-to-end surviving-sentinel regression
- `trash-empty` now warns and leaves a direct `.git` case variant in place without letting that protected item block other exact previewed Trash children
- Permanent and Trash preflight reuse each directory listing for Git-marker checks instead of enumerating every directory twice, while retaining the final mutation-time recheck
- Git-backed unit fixtures now disable ambient commit signing and hooks, so maintainer Git configuration cannot make the Rust suite fail
- Release policy now proves the Gitleaks positive control runs before the scanner directory reaches `PATH`, and both workflows syntax-check that control script explicitly
- `node-modules` apply now reasserts the scanner's exact target shape before deletion, refusing non-directory and symlink targets, symlinked category ancestors, forged non-`node_modules` leaves, plus ASCII-case-insensitive `.git` and outer `node_modules` ancestors and non-normal paths
- The landing page and packaged manual now declare a compact project favicon instead of generating a browser-level `/favicon.ico` 404 on every fresh visit
- The landing-page hero caption now keeps readable contrast over every part of its image instead of combining muted text with a translucent overlay
- The ShellCheck helper now fails before linting with an actionable error when `shellcheck` is unavailable, and release policy proves that path does not invoke ShellCheck
- `CODING_STANDARDS.md` no longer tells review to skip gates that run only at release (`cmp -s AGENTS.md CLAUDE.md`, the fuzz targets, `actionlint`), and now lists the `video/`, shell, and secret-scanning merge gates it had omitted, so a reviewer no longer spends the budget on checks that already block
- `CODING_STANDARDS.md` corrects an S1 precedent that no search could find, S12's incomplete list of approved dynamic call sites, S6's unstated denylist, and the ast-grep escape hatches sanctioned by the deletion-sink rule
- The binary entry point now carries the `//!` module contract that S4 requires of every file under `src/`

### Security
- Git metadata is now denied ASCII-case-insensitively by project scanners, ownership and category checks, target validation, and open-handle Trash/permanent preflight, closing actionable `.GIT` findings on case-insensitive macOS filesystems
- Common local environment, private-key, and signing-material files are ignored, while checksum-pinned full-history Gitleaks and TruffleHog scans now block ordinary PR/main CI as well as releases
- CI and release refuse a Gitleaks binary that cannot trip a runtime positive control, so a version string and clean scan cannot mask a no-op detector

## [0.6.3] - 2026-08-28

### Changed
- Global mutation flags are now capability-scoped: report-only commands reject flags they cannot honor, while `scan --shred` and Trash purge retain only their meaningful controls
- Docker and simulator cleanup now reject `--shred` instead of accepting a flag that cannot affect their exact typed command actions
- Release version validation now checks the authoritative changelog heading and exact, unique README and manual version declarations instead of accepting substring matches

### Fixed
- The TUI now filters configured protected Trash items before it calculates danger or asks for approval
- Bare `devtrim --json` now rejects the implicit TUI before terminal launch, matching explicit `devtrim tui --json` and preserving automation-only JSON behavior
- A present but missing, broken, escaping, or otherwise invalid `swift-latest.xctoolchain` reference now blocks toolchain cleanup instead of producing an empty successful scan
- The production landing page stayed on the actual stable v0.6.2 archive and release while the v0.6.3 candidate was in beta staging

### Security
- Human command previews escape the complete action string, closing terminal control-character injection through dynamic but validated command arguments without altering JSON data
- Xcode and Swift toolchain apply now reassert each scanner's exact direct-child target shape before the shared deletion sink, so a forged nested finding cannot borrow the category's authority
- Release verification passed current and MSRV suites, strict Clippy, structural positive controls, root and fuzz dependency audits, all five 60-second fuzz targets, the arm64 build, PTY cancellation, workflow/shell/secret gates, P3 autoreview, Matt Pocock standards/spec review, video build/render/container checks, desktop/mobile browser checks, and a fresh independent verifier

[0.6.3]: https://github.com/mneves75/devtrim/compare/v0.6.2...v0.6.3

## [0.6.2] - 2026-08-28

### Changed
- Production release closeout now re-verifies the immutable artifact, updates and audits the Homebrew tap, upgrades the maintainer installation, and proves the sole `/opt/homebrew/bin/devtrim` reports the released version
- The crate now forbids `unsafe` and denies `unwrap`/`expect`/`panic`/`unreachable`/`todo`/`unimplemented`/`dbg!` and unreasoned lint suppression outside tests, and structural lints with positive-control tests now cover shell invocation and binding names alongside the deletion sink
- `CODING_STANDARDS.md` documents the review-time rules that tooling cannot check, as citable `S<n>` entries

### Fixed
- Filesystem size and artifact/toolchain evidence checks now treat metadata errors as blocking failures instead of silently reporting an absent or empty path
- Build-process liveness checks now reject every nonzero `lsof` result after `pgrep` finds candidate processes instead of treating an uncertain probe as no activity

### Security
- Docker cleanup rejects remote contexts, previews the exact absolute local Unix-socket endpoint, and pins that endpoint into the typed command authority used by apply
- Simulator cleanup previews and authorizes one validated UDID per finding, then rechecks that exact device is still unavailable before deletion
- Trash-mode directory cleanup now rejects foreign filesystem devices and nested Git repository/worktree markers before the path-based Trash call, matching the permanent-deletion preflight
- Release verification passed the full and MSRV test suites, strict Clippy, structural controls and positive controls, dependency audits, five 60-second fuzz targets, the arm64 build, PTY TUI cancellation, workflow and shell policy checks, secret scans, P3 autoreview, video build and render, and desktop/mobile layout and accessibility checks

[0.6.2]: https://github.com/mneves75/devtrim/compare/v0.6.1...v0.6.2

## [0.6.1] - 2026-08-27

### Changed
- `devtrim icloud` now reports a recursive inventory of large iCloud Drive files with logical and locally allocated sizes; it no longer infers upload progress from filesystem allocation
- Scanner and apply preflights now treat unreadable roots, traversal gaps, Git ownership/activity failures, liveness-probe failures, and nonzero owner-tool exits as blocking errors instead of silently producing partial authority

### Fixed
- Every `--json` invocation, including Clap help/version/parse failures, emits exactly one JSON document with a truthful operation, error list, and nonzero exit; empty applies report a zero summary
- Failed human and TUI applies no longer render as successful, simulator cleanup measures the previewed device directory, and Docker/simulator discovery distinguishes an absent tool from a failed one
- Journal history reverse-scans a bounded newest tail with per-line and total-byte caps, pairs legacy records across rotations, waits for active guarded applies, serializes complete synced records, refuses symlinked path components, and creates no lock file

### Security
- Permanent recursive deletion now rejects device crossings and Git repository/worktree markers at every depth, rechecks macOS file generation as part of identity, and revalidates configured `protect` aliases immediately before mutation
- The release chain runs project and dependency code only in read-only jobs, executes all five bounded fuzz targets, audits and monitors the separate fuzz lockfile, pins Actions and downloaded tools, requires the current default-branch head plus exact-commit CI/autoreview, and gives only the no-checkout publisher release-write and OIDC authority
- Production promotion verifies immutable beta provenance, signer workflow, exact asset names, and checksums before reusing the same archive without rebuilding

[0.6.1]: https://github.com/mneves75/devtrim/compare/v0.6.0...v0.6.1

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
