# 020 — Agent-era visibility, APFS bulk speed, Mole-grade TUI & 2026+ site

Status: SHIPPED in 0.16.0 (2026-11-22, main `45daa25`). Base `f3818c2`. Target `0.16.0`.

## Sharpened framing (deeper, not bigger)

1. **What a top macOS systems + CLI/TUI engineer would do beyond the prompt:**
   - Measure real syscall/walk cost and real agent/builder disk usage on this Mac *before* designing features or optimizations.
   - Deduplicate Docker `buildx` `docker-container` builders by backing node ID so alias builders (`kiri` and `kiri-build` both backed by `kiri-build0`, 2.05 GB) are never double-counted or double-pruned.
   - Keep `pgrep`/`lsof` for build-process liveness rather than swapping in raw `proc_pidinfo` syscalls, because 15+ planted-violation proofs, `tui.py`, `evals.rs`, and `frames.py` deterministically verify liveness via `PATH` stubs; get the 5–10× scan speedup safely from batched `getattrlistbulk(2)` inside `crates/apfs-size` instead.
   - Keep `.git` strictly forbidden in the deletion sink while surfacing multi-GB `.git/objects/tmp_objdir-*` debris and local AI model stores (Ollama, LM Studio) as read-only inventory.
   - Fix two live landing-page proof defects found by reading `index.html` and `scripts/site/frames.py`: `tui:overview` cropped at 38 cols hid the category sizes column, and `tui:progress` captured the post-stop `0 item(s)` outcome instead of live mid-apply progress.

2. **Rawest material inspected:**
   - Live `docker buildx ls --format '{{json .}}'` & `docker buildx du` on this Mac: 2 `docker-container` builders sharing node `kiri-build0` (`2.05 GB` invisible to `docker system df`).
   - Live agent & model paths on this Mac: `~/.codex/worktrees` (`1.1G`), `~/.t3/worktrees` (present), `~/.ollama/models` (`610M`), `~/.lmstudio/models` (present).
   - Live `src/safety.rs` (`walk_stats`), `crates/apfs-size/src/lib.rs`, `src/ops/docker.rs`, `src/worktrees.rs`, `src/ops/leftovers.rs`, `src/tui.rs`, `index.html`, `styles.css`, `scripts/site/frames.py`, `video/audit-waivers.json` (waiver `GHSA-g3hv-75wg-m249` expires 2026-11-30; `eslint@10.15.0` now pulls `minimatch ^11.0.5`).
   - `modern-web-guidance` guides retrieved live: `tabs`, `responsive-table`, `component-specific-light-dark-theme`, `css-layout`, `anchor-positioning-tab-underline`.

3. **Specific methods and tools:**
   - `getattrlistbulk(2)` in `crates/apfs-size` with parity tests against `walkdir`/`lstat` reference and wall-clock micro-benchmark.
   - Typed `CommandAuthority::DockerBuildxPrune` with strict local-host & builder-name validation and planted-violation coverage.
   - Hardened `git status --porcelain=v1 -z` and `build_process_cwds()` for read-only worktree classification (`in-use` / `dirty` / `active` / `idle`).
   - Real PTY tests (`scripts/tests/tui.py`, `scripts/tests/read-only-views.py`) at 64×18 and 120×40, `NO_COLOR` assertions, `scripts/site/frames.py`, `scripts/site/csp.py`, `video/film/scripts/facts.py`, Chrome desktop/mobile layout checks.
   - `cccc --no-cache --min 0` (no tests written for functions with cognitive complexity ≤ 5 unless they contain regexes), `mneves-prune-tests`, cross-model `autoreview` + `mneves-verify`, `mneves-security` deploy gate, `scripts/verify.sh offline`, and 5 nightly fuzz targets.

4. **External proof that the result is right:**
   - Parity tests + measured scan speedup on a multi-thousand-file APFS fixture.
   - `scripts/verify.sh offline` (50+ checks including MSRV 1.88, `planted-violations.py`, `csp.py`, `film-controls.py`, `cargo audit`, `gitleaks`).
   - `video/` passing `npm ci && python3 -B ../scripts/video-audit.py && npm run lint && npm run format:check && npm run build` with an empty waiver list `[]`.
   - Real Chrome desktop (1440×900) and mobile (390×844) inspection with zero horizontal overflow and ≥ 4.5:1 AA contrast.
   - Fresh cross-model `autoreview`, `mneves-verify`, and `mneves-security` deploy gate.

5. **Rewritten task prompt:**
   > Speed up `devtrim`'s APFS directory measurement with batched `getattrlistbulk(2)` (retiring unused clone fields) while preserving exact byte/error parity; add deduplicated local Docker `buildx` builder cache discovery and typed `CommandAuthority` pruning; enrich read-only `devtrim worktrees` with process-liveness, dirty-tree, and activity state plus `.t3/worktrees` discovery; surface `.git/objects/tmp_objdir-*` debris and Ollama/LM Studio models as read-only leftovers; upgrade the TUI's scannability, progress bars, category-jump ergonomics, and outcome guidance in a Mole-friendly style without weakening any safety or 64×18/`NO_COLOR` contract; retire the expiring `video/` `GHSA-g3hv-75wg-m249` waiver; modernize `index.html`/`styles.css`/`MANUAL.html` per `modern-web-guidance` and fix the broken `frames.py` captures; apply `cccc` complexity gating and `mneves-prune-tests`; then verify independently and ship `v0.16.0`.

## Red team (six months out)

1. **R1 — `getattrlistbulk` silently miscounts hard links, compressed files, symlinks, or unreadable subdirectories.**
   - *Trigger:* `node_modules` with pnpm hard links, `UF_COMPRESSED` macOS files, dangling symlinks, or `0o000` dirs.
   - *Cost:* Understated confirmation sizes or silent partial reads violating `CODING_STANDARDS.md` S8.
   - *Cheapest experiment:* Direct parity test comparing `bulk` walk vs `walkdir`+`lstat`+`getattrlist` reference across regular files, hard links, APFS clones, symlinks, empty dirs, and an unreadable (`0o000`) nested subdir (must return `Err`), plus non-macOS fallback compilation.
2. **R2 — `docker buildx` pruning hits a remote builder, double-counts shared container nodes, or breaks machines without the `buildx` plugin.**
   - *Trigger:* A user with `docker-container` aliases sharing one node (observed live: `kiri` + `kiri-build` → `kiri-build0`), a `remote`/`cloud` driver builder, or a minimal Docker install without `buildx`.
   - *Cost:* Double-counted GB, attempting to prune a remote CI builder, or breaking `clean docker` on non-buildx machines.
   - *Cheapest experiment:* Unit + CLI sandbox tests with fake `docker` scripts covering: alias builders sharing one node ID (1 finding, not 2), remote/cloud driver builder (refused/ignored), malformed builder name (`--flag`, `../x`, rejected at scan & apply), and missing `buildx` subcommand (falls back cleanly to default daemon findings).
3. **R3 — TUI/site polish breaks 64×18 layout, `NO_COLOR` ladder, `csp.py`, `facts.json`, or `tui.py` assertions.**
   - *Trigger:* Adding progress bars, category jump, or new CSS/HTML elements (`csp.py` allows only `ELEMENTS`, no inline `style=""`, `facts.json` anchors must remain intact).
   - *Cost:* CI/release gate failure or clipped confirmation UI.
   - *Cheapest experiment:* Run `tui.py`, `read-only-views.py`, `csp.py --check`, `frames.py`, `film-controls.py`, and Chrome viewport checks immediately after TUI & site changes.

**Simpler route:** Deferring `proc_pidinfo` (which conflicts with `PATH`-stubbed liveness gates) and keeping worktrees/debris/models strictly read-only avoids weakening any deletion boundary while delivering every user-visible win.

**Verdict:** Ship in clean, verified tracks.

## Tasks (`v0.16.0`)

| # | Track | Scope | Owner files | Proof |
|---|---|---|---|---|
| T1 | Perf | Batched `getattrlistbulk(2)` directory walker in `crates/apfs-size` (drop unused `CloneFamily`) + wire into `walk_stats` in `src/safety.rs` | `crates/apfs-size/src/lib.rs`, `src/safety.rs` | Parity tests (clones, hard links, symlinks, `0o000` EACCES fail-closed) + wall-clock benchmark |
| T2 | Docker | Local `docker-container` `buildx` builder cache discovery (deduplicated by node ID), strict name/driver checks, `CommandAuthority::DockerBuildxPrune` | `src/ops/docker.rs`, `src/report.rs` | Fake-`docker` unit + CLI tests + planted-violation cases |
| T3 | Worktrees & Leftovers | Enrich `devtrim worktrees` (read-only `in-use`/`dirty`/`active`/`idle` via hardened `git status` + `build_process_cwds()` + last activity), add `.t3/worktrees` default root, surface `.git/objects/tmp_objdir-*` + Ollama/LM Studio models read-only in `leftovers` | `src/worktrees.rs`, `src/ops/project.rs`, `src/ops/leftovers.rs`, `src/safety.rs`, `src/app.rs` | Eval & unit tests, read-only mutation-flag refusal, `.git` protection preserved |
| T4 | TUI UX | Mole-inspired TUI polish: visual disk + reclaim summary card, proportional inline bars when width fits, live progress bar on scan/apply, `o` direct jump from Scan-everything to actionable category, clearer outcome summary with next-step command | `src/tui.rs` | 64×18 & 120×40 render tests, `NO_COLOR` tests, `scripts/tests/tui.py` |
| T5 | Video waiver | Upgrade `video/` `eslint` to `10.15.0`, clear `GHSA-g3hv-75wg-m249`, empty `video/audit-waivers.json` | `video/package.json`, `video/package-lock.json`, `video/audit-waivers.json` | `npm ci && python3 -B ../scripts/video-audit.py && npm run lint && npm run format:check && npm run build` |
| T6 | 2026+ Site | Fix `frames.py` (`tui:overview` width & `tui:progress` mid-apply capture); modernize `index.html`, `styles.css`, `MANUAL.html`, `DESIGN.md` per `modern-web-guidance` & `frontend-design` | `index.html`, `styles.css`, `MANUAL.html`, `DESIGN.md`, `scripts/site/frames.py`, `video/film/facts.json` | `frames.py`, `csp.py --check`, `film-controls.py`, real Chrome desktop/mobile render check |
| T7 | Quality & Prune | `cccc --no-cache --min 0` complexity check, `mneves-prune-tests`, `scripts/tests/planted-violations.py` | changed tests & `scripts/tests/planted-violations.py` | `cccc` report, pruned test diff, full `verify.sh offline` |
| T8 | Docs & Bump | Bump `0.16.0` across `Cargo.toml`, `Cargo.lock`, `fuzz/Cargo.lock`, `README.md`, `MANUAL.html`, `CHANGELOG.md`, `SECURITY.md`, `AGENTS.md` == `CLAUDE.md`, `MEMORY.md` | docs & manifests | `cmp -s AGENTS.md CLAUDE.md`, `RUSTFLAGS='--cfg fuzzing' cargo check --locked --manifest-path fuzz/Cargo.toml --bins` |
| T9 | Review, Verify & Release | Cross-model `autoreview` (GPT-6.2 Sol), `mneves-verify` (different model), 5 nightly fuzz targets, `mneves-security` deploy gate, PR/merge, `scripts/release.sh 0.16.0-beta1` → staging verification → `0.16.0` prod closeout, `cargo clean` & scratch cleanup | `.scratch/security-gate/<sha>/` | All gates PASS, `verify-live.sh 0.16.0` PASS |
