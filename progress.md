# Progress — 0.17.0 (plans/021)

Current phase: 1, home and scan landing. Branch `feat/tui-clarity` from `3310ef3`, worktree
`../devtrim-wt-tui`. Baseline captures and prototypes in `.scratch/tui-clarity/` (untracked).

- Decision: overview home (prototype A) over a Mole-style intent menu (C); evidence and
  scenarios in the plan. Keys and launch behaviour unchanged.
- Phase boundary: the film quotes result rows, details and the confirmation, not the home;
  phase 2 (row format) therefore re-renders the film. A draft render of the unchanged film is
  the probe for that cost.
- Next action: failing tests for AC1-AC3, then the home in `src/tui.rs`.

# Progress — 0.16.0 (plans/020)

Shipped 2026-11-22: PR #101 squash-merged to `main` `45daa25cd62f23e2b05e470ddd1eaec22165686b` (tree `e11ec50c8462698c32490ed187ffdc8cd4c3a417`), immutable `v0.16.0-beta1` (run `44968377996`) and `v0.16.0` (run `44970344995`, ZIP SHA-256 `912cc97a446d208de15a52323cf5c4fc5e031ab3c7c1f586a05c93d756c61c9b`), Homebrew tap `7656d47`, `/opt/homebrew/bin/devtrim` `0.16.0` (SHA-256 `b5906877ea47963f8eb235e59e05d6aabde24ec7dc1334da03e9cea5903a8791`). Security gate: **PASS** for staging, GitHub Pages, and production with **zero waivers** (`GHSA-g3hv-75wg-m249` retired); records in `.scratch/security-gate/45daa25cd62f23e2b05e470ddd1eaec22165686b/`. Post-promotion closeout updated `index.html` to `v0.16.0`, regenerated `video/film/facts.json`, and added `llms.txt` + README AI-agent guide.
- **T1 (APFS `getattrlistbulk` & `.t3/worktrees`)**: Added `read_dir_bulk` in `crates/apfs-size` (removing unused `CloneFamily` fields), wired into `safety::walk_stats` with fallback and parity/unreadable tests, and added `.t3/worktrees` to `DEFAULT_PROJECT_ROOTS`. Hot A/B micro-benchmark on 8,000 files (80 dirs): 35.5 ms median vs 37.3 ms (`0.15.0`).
- **T2 (Docker `buildx` local builders)**: Added `CommandAuthority::DockerBuildxPrune`, `docker context ls` local-only verification, alias deduplication by `(resolved socket, worker IDs)`, apply re-verification + per-attempt exclusive `0700` `DOCKER_CONFIG`/`BUILDX_CONFIG` isolation, end-to-end eval, and planted-violation proof (`eval/docker-buildx`).
- **T3 (Enriched `worktrees` & `leftovers`)**: Added hardened `worktree_dirty_count` (`--attr-source=<empty-tree>`, `GIT_ATTR_NOSYSTEM=1`, `--git-dir`/`--work-tree`, raw-byte `GIT_CONFIG_COUNT` overrides) + read-only active-process probe (`in-use` / `dirty` / `clean` + UTC activity date, fail-closed on probe failure) to `worktrees`, and read-only `.git/objects/tmp_objdir-*` (with `O_NOFOLLOW|O_NONBLOCK` 4 KiB pointer reads, `openat` `commondir`, and fail-closed containment) + `~/.ollama/models` / `~/.lmstudio/models` reporting to `leftovers`.
- **T4 (Mole-inspired TUI UX)**: Added menu overview summary card, 6-cell inline share bars (`width >= 96`), live Working progress bar, `o` category jump from `Scan everything`, and post-apply `trash-empty --only-devtrim` hint. Verified via `tui.py` and `read-only-views.py`.
- **T5 (Video waiver retirement)**: Upgraded `eslint` to `10.15.0`, pinned `minimatch` `11.0.8`, cleared `video/audit-waivers.json` to `[]`. Verified clean on Node 24 and Node 26 (`video-audit.py` + `video-audit-controls.py`).
- **T6 (2026+ Site Modernization & Frames)**: Applied `modern-web-guidance` (scroll-driven tab fades, forced-colors-safe border underline + CSS anchor positioning, `hidden="until-found"`, container-queried table wrapper, `subgrid` pillars, `color-scheme: only dark` on `.term`), fixed `tui:overview` (54 cols) and `tui:progress` (live mid-apply capture) in `frames.py`, regenerated `facts.json` & updated `copy.json` IDs, verified 0 horizontal overflow via Chrome CDP at `390×844` and `1440×900`, `csp-controls.py`, and `film-controls.py`.
- **T7 (`cccc` rule & test pruning)**: Audited every new function with `cccc 1.8.0` and removed 3 unit tests targeting `<= 5` non-regex helpers (`is_valid_builder_name`, `parse_buildx_du`, `format_worktree_state`), keeping behavioral/end-to-end and `> 5` tests.
- **T8 (Version bump & docs)**: Bumped to `0.16.0` across `Cargo.toml`, lockfiles, `README.md`, `MANUAL.html`, `CHANGELOG.md`, `SECURITY.md`, `CODING_STANDARDS.md`, `DESIGN.md`, `AGENTS.md`/`CLAUDE.md`, plus `llms.txt` for AI agents.
- **T9 (Gates, cross-model reviews, security gate & release)**: All offline/online gates, 5/5 nightly fuzz targets, 227/227 planted violations, MSRV 1.88.0, Sol cross-model audits (0 Critical/High/Medium/Low repository findings), security gate PASS (0 waivers), staging `v0.16.0-beta1`, production `v0.16.0`, Homebrew tap `7656d47`, and live Pages verification complete.

# Progress — 0.15.0 (plans/019)

Shipped 2026-11-17: main cf7962b (tree fc54bef), immutable v0.15.0-beta1 and v0.15.0
(zip sha256 d19faa60…), Homebrew tap 549e79c, `/opt/homebrew/bin/devtrim` 0.15.0 sole.
Security gate: staging, Pages and production WAIVED (GHSA-g3hv-75wg-m249, expires
2026-11-30); records in `.scratch/security-gate/cf7962b…/`. Plan 019 merged as 92b384f
(PR #97); the release autoreview found one P1 (a stop between caches' pre-lock check and
`journal::begin` left uv's `.lock` unjournaled), fixed by `journal::admit` in PR #98.
Inspired by `govijr/fix-storage` f354e20 and `morajabi/poo` 1893c39, nothing copied.

Open owner decision: free-space history (new state file) as plan 020, yes or no.

Decisions: `init.sh` not added, since `scripts/verify.sh` already sets up and runs every
check. The overview frame is cropped to the operations column so the demo machine's disk
numbers never reach the site.

# Progress — 0.14.0 (plans/018)

Shipped 2026-11-16: main 26ed547 (tree 9205907), immutable v0.14.0-beta1 and
v0.14.0 (zip sha256 e0f01326…), Homebrew tap 20fc13a, `/opt/homebrew/bin/devtrim`
0.14.0 sole. Security gate: staging and production WAIVED (GHSA-g3hv-75wg-m249,
expires 2026-11-30); records in `.scratch/security-gate/26ed547…/`.

## Decisions

- 5b least-regret ordering dropped: no rebuild-cost data; largest-first is a documented convention.
- Consent, danger and totals use allocated blocks; the APFS private size is a displayed lower
  bound only (`frees_at_least_bytes`), trusted only on APFS volumes with no snapshot.
- Clone-family credit dropped: an edited clone outside the tree can still hold the shared blocks.

## Follow-ups

- Remove the unused clone fields from `crates/apfs-size` (touches the only `unsafe` call).
- CORE-PARSE-1 (lsof omitted-name hypothesis) stays needs-validation, pre-existing.
- GHSA-g3hv-75wg-m249 waiver expires 2026-11-30.
