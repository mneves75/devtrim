# Changelog

All notable changes to devtrim. Format follows Keep a Changelog; versioning is semver.

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
- 6 unit tests + integration coverage of scanner logic
- MANUAL.html: single-file interactive manual (dark/light, CSP hardened)
- Landing page (GitHub Pages) + release automation script

### Security
- CSP (`default-src 'none'`) and `referrer: no-referrer` on shipped HTML
- No network access at runtime except package-manager subprocesses
[0.1.0]: https://github.com/mneves75/devtrim/releases/tag/v0.1.0
