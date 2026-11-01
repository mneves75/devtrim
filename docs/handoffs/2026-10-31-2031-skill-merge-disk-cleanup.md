# Handoff: motion-film skill merged, disk freed, finish-cleanup (2026-10-31)

## Goal

Marcus's latest instructions:
- "take note; when all finished, merge to main and delete branch/worktree / .scratch folders not used / necessary anymore; if rust project, execute cargo clean to free disk space;"
- "continue, then run and free my macos disk space".

This continues the devtrim demo-film work. The film is already live; see
`docs/handoffs/2026-10-31-1440-demo-film-site.md`.

## Status

**Done**
- The owner rule was noted:
  - OptMem #3315, plus local memory `finish-cleanup-rule.md` in the devtrim auto-memory folder;
  - outcome note OptMem #3332.
- **devtrim**:
  - `cargo clean` removed 1787 files (546.2 MiB);
  - `.scratch` (security-gate, verify-0.12.1/2/3) is all cited release evidence, so it was kept;
  - repo on `main` 6894e8c, clean except the untracked `docs/handoffs/`.
- **mneves-skills PR #37** (`mneves-motion-film`) was merged as `c3dc2bb529da530e07572af6046d4e7312863711`. Its tree `5ca06ffc2d23d2537b7a5c87bff9d4b68ceb38ff` equals the gated candidate `5336080e6919f6f44f8abb90eb1bf9455ab1cc37`.
  - Main CI run 41248161254: success. Pages deploy of c3dc2bb: success.
  - Served `site/index.html` (d9a95ca4…) and `site/howto.html` (0351e401…) match the merge byte for byte.
  - Branch `feat/mneves-motion-film` deleted locally (`-d`) and on the remote. Worktree `.scratch/wt-motion-gate` removed and pruned. Local main was fast-forwarded to c3dc2bb.
  - Only cited entries remain in `.scratch`: readiness, security-gate, skills-ref.
- **Fixes landed in PR #37 before merge.** Each came with planted tests that fail before the fix and pass after, on Python 3.15.2 and 3.10.22.
  - **624562e**, from Sol autoreview P2s:
    - `build.py` runs `npx eslint <src>` before render;
    - `check_sync.py` requires an absolute onset floor of 0.1, so silence no longer passes a frame-0 kick;
    - `check_facts.py` accepts `line('id')`;
    - `contact.py` uses `fps=…:round=up`, because the default picked frame 5 for the "0 s" tile at 60 fps (measured with framemd5).
  - **eb59afb**: `facts.py --check` prints page text with `!r` (gate finding C-M1, Medium, SEC-11).
  - **5336080**:
    - `facts.py` JSON output escapes U+007F–U+009F as `\uXXXX`;
    - `capture.py` prints Chrome's stderr with `!r`.
- **Security gate (mneves-skills)**:
  - 624562e: BLOCK (C-M1);
  - eb59afb: BLOCK (SEC-11 not tested across all output paths: C-M1-C1, DEL/C1 in JSON);
  - 5336080: **WAIVED, independent acceptance PASS** (GPT-6.1 Sol high), relying on owner waivers SEC-15, SEC-17, SEC-20, SEC-21, SEC-23 and CMP-06, all to 2027-01-25;
  - the readiness record is ready with 0 open blockers;
  - records were copied to the merge commit with `reused_from`.
- **Disk.** It went from 48 GiB free to about 84 GiB (it fluctuates, because other agents build on this Mac).
  - The owner chose 4 groups in AskUserQuestion, and devtrim moved them to the Trash:
    - `clean caches` twice: 17.8 GB, then 5.6 GB including the uv cache, which was locked by a running uv the first time;
    - `purge`: 232 items, 35.9 GB (171 node_modules, Pods, CACHEDIR target, `.build`, etc.);
    - `clean xcode`: 81 items, 13.6 GB (DeviceSupport iPhone19,3 and iPad13,4 27.2, plus idle DerivedData; Archives excluded).
  - Then `devtrim trash-empty --only-devtrim --apply --yolo --confirm=67`, and later `--confirm=5`, purged only devtrim's own moves. The Trash went from 73 GB back to the owner's own 2.5 GB.

**Not done (owner's call)**
- The local branch `feat/demo-film` in devtrim still exists. Its tree c9d0202 equals main. dcg blocks `git branch -D` (rule `core.git:branch-force-delete`, allow-once id 216060).

## Decisions

- **No version bump.** PR #37 merged as a feature merge with VERSION 1.44.0 and a CHANGELOG `[Unreleased]` entry, following the PR #30 merge f7f3958.
- **Gate scope.** A delta audit from d98853b (= be1a548 tree 87efb4e). Finders A and B on Opus, the verifier on Sol. Later candidates were verified on their delta from 624562e.
- **Declined autoreview finding:** "count copy usage only where it feeds line()". It would break literal lists mapped through `line()`, so the docstring now states the limit instead.
- **`check_sync` floor is absolute (0.1).** A relative floor of max/10 failed real kicks (onset ~0.55) because the final chord dwarfs them. Re-checked on the real devtrim film: 45 events, worst 7.0 ms.
- **Trash purge.** Only `--only-devtrim`, never the owner's whole Trash. `--yolo` is required for the danger-9 non-TTY plan, and `--confirm=<gb>` still applies.
- **Not cleaned** (not in the approved groups):
  - OrbStack VM 69 GB;
  - simulator data 16.8 GB;
  - Swift toolchain 4.8 GB;
  - other repos' `.scratch` (minuta 5.4 GB, macos-dynamic-island 4.8 GB, …);
  - Chrome cache 6 GB;
  - pnpm store (the contract forbids prune without authorization).

## Dead ends

- **autoreview without CODEX_HOME** fails with `codex engine failed (1)`. Resolve the home with `codex-auto home` (the personal account was rate-limited, so it used `/Users/mneves/.codex-berlin`) and prefix the run with `CODEX_HOME=…`.
- **dcg blocks:**
  - redirects to variable paths (`> $G/...`): use literal paths or the Write tool;
  - `git branch -D`;
  - `sleep N; cmd` chains: use background runs with an `until` loop.
- **`devtrim history`** shows only a short tail. Verify a purge by the Trash size (`du -sh ~/.Trash`) and `df`.
- **First run of the gate runner:** `bun test` failed one timing test (`tools/lib/checks/shared.test.ts:71`, "bounds grep: fast") under load average >100. It passed on rerun; `tools/` was unchanged.
- **Runner typo:** a missing space before the python3.10 path gave exit 127. It is fixed in the stored `run-gates.sh`.

## Files

- mneves-skills gate records, kept as release evidence and never `scratch clean`ed:
  - `/Users/mneves/dev/MEUS_SITES/mneves-skills/.scratch/security-gate/{624562e…,eb59afb…,5336080…,c3dc2bb…}/`, with `verify/verdict.md`, `production.{md,json}`, and the Sol autoreview JSON in `5336080…/autoreview/`;
  - `…/.scratch/readiness/{624562e…,eb59afb…,5336080…,c3dc2bb…}/`, with `production.json`, `production-assessment.md`, `run-gates.sh` and `gates.{log,summary.txt}`.
- `skills/mneves-motion-film/` in mneves-skills: the shipped skill. Its scripts are in `scripts/`; `tests/test_pipeline.py` has 29 planted checks.
- `video/film/scripts/` in devtrim: the film pipeline the skill was distilled from. It still has the pre-fix behaviours.

## Verification

- `python3 -B skills/mneves-motion-film/tests/test_pipeline.py` (mneves-skills): `0 failure(s)`, also under `/Users/mneves/.local/share/uv/python/cpython-3.10-macos-aarch64-none/bin/python3.10`.
- `gh run view 41248161254 --repo mneves75/skills`: success. Pages c3dc2bb: success. curl SHA-256 of the live pages equals `git show c3dc2bb:site/…`.
- `df -h /System/Volumes/Data`: ~84 GiB free. `du -sh ~/.Trash`: 2.5G.
- devtrim `git status`: only `docs/handoffs/` untracked.

## Next steps

1. The owner deletes the leftover devtrim branch: `git branch -D feat/demo-film` (or `dcg allow-once 216060`).
2. Optional follow-up PR in devtrim: port the skill's fixes into `video/film/scripts/`, then run the devtrim gate (the site deploys):
   - `contact.py` `round=up`;
   - the `check_sync.py` absolute onset floor;
   - `check_facts.py` single quotes;
   - `facts.py` repr diagnostics and DEL/C1 escaping;
   - ESLint in `build.py`.
3. Next mneves-skills release PR:
   - fold the d98853b/5336080 runs into `docs/readiness/ledger.md`;
   - fix the docs notes D-B10..D-B15 ("Twenty-two" skills should read 23, AGENTS/SECURITY scope and the Python floor for the new skill, egress lines, "until ship", "as served");
   - hardening H-M2 (`npx --no`), H-M3 (Chrome `--`, http(s) only), H-M4 (MP4 box-size bound), H-M5 (closed cue set), H-M6 (`persist-credentials: false`).
4. Before 2026-12-01, remove devtrim's GHSA-g3hv-75wg-m249 waiver by upgrading minimatch once eslint allows it; otherwise the video gate fails.

## Open questions

- Clean the remaining large items? OrbStack VM (69 GB), simulator data (16.8 GB), other repos' `.scratch`, Chrome cache.
- Port the skill fixes back into devtrim's own film pipeline now, or leave it?

## Standing instructions

- Reply in pt-BR, starting with "Marcus,".
- Branch first; never commit to main. devtrim squash-merges; mneves-skills uses merge commits.
- Commits end with "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"; PR bodies end with the Claude Code line.
- Run the mneves-security gate on the exact tree before any merge that deploys. mneves-skills also needs a readiness record.
- Opus builds; GPT-6.1 Sol reviews and verifies via `codex-auto` (`-m gpt-6.1-sol -c model_reasoning_effort=high`).
- When finished: merge, delete the branch and worktree, remove unneeded `.scratch` (keep gate and readiness records), and `cargo clean` Rust repos.
- Never run cleanup against the real HOME to test devtrim. The disk cleanup here was a real-HOME run of the installed Homebrew devtrim, explicitly approved by the owner.
- Delete only what you created; list everything else for the owner.
