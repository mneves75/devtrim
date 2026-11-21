# Progress — 0.16.0 (plans/020)

In progress (2026-11-21) on branch `plan/020-agent-era-perf-ux` (from `main` `f3818c2`):
- **T1 (APFS `getattrlistbulk` & `.t3/worktrees`)**: Added `read_dir_bulk` in `crates/apfs-size` (removing unused `CloneFamily` fields), wired into `safety::walk_stats` with fallback and parity/unreadable tests, and added `.t3/worktrees` to `DEFAULT_PROJECT_ROOTS`. Hot A/B micro-benchmark on 8,000 files (80 dirs): 35.5 ms median vs 37.3 ms (`0.15.0`).
- **T2 (Docker `buildx` local builders)**: Added `CommandAuthority::DockerBuildxPrune`, `docker context ls` local-only verification, alias deduplication by node ID, apply re-verification, end-to-end eval (`eval_clean_docker_buildx_prunes_deduplicated_local_builder_and_skips_remote`), and planted-violation proof (`eval/docker-buildx`).
- **T3 (Enriched `worktrees` & `leftovers`)**: Added hardened `worktree_dirty_count` + read-only active-process probe (`in-use` / `dirty` / `clean` + UTC activity date, fail-closed on probe failure) to `worktrees`, and read-only `.git/objects/tmp_objdir-*` + `~/.ollama/models` / `~/.lmstudio/models` reporting to `leftovers`.
- **T4 (Mole-inspired TUI UX)**: Added menu overview summary card, 6-cell inline share bars (`width >= 96`), live Working progress bar, `o` category jump from `Scan everything`, and post-apply `trash-empty --only-devtrim` hint. Verified via `tui.py` and `read-only-views.py`.
- **T5 (Video waiver retirement)**: Upgraded `eslint` to `10.15.0`, pinned `minimatch` `11.0.8`, cleared `video/audit-waivers.json` to `[]`. Verified clean on Node 24 and Node 26 (`video-audit.py` + `video-audit-controls.py`).
- **T6 (2026+ Site Modernization & Frames)**: Applied `modern-web-guidance` (scroll-driven tab fades, forced-colors-safe border underline + CSS anchor positioning, `hidden="until-found"`, container-queried table wrapper, `subgrid` pillars, `color-scheme: only dark` on `.term`), fixed `tui:overview` (54 cols) and `tui:progress` (live mid-apply capture) in `frames.py`, regenerated `facts.json` & updated `copy.json` IDs, verified 0 horizontal overflow via Chrome CDP at `390×844` and `1440×900`, `csp-controls.py`, and `film-controls.py`.
- **T7 (`cccc` rule & test pruning)**: Audited every new function with `cccc 1.8.0` and removed 3 unit tests targeting `<= 5` non-regex helpers (`is_valid_builder_name`, `parse_buildx_du`, `format_worktree_state`), keeping behavioral/end-to-end and `> 5` tests.
- **T8 (Version bump & docs)**: Bumped to `0.16.0` across `Cargo.toml`, lockfiles, `README.md`, `MANUAL.html`, `CHANGELOG.md`, `SECURITY.md`, `CODING_STANDARDS.md`, `AGENTS.md`/`CLAUDE.md`.
- **T9 (Gates, cross-model reviews, security gate & release)**: Next.

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
