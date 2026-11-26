# Progress — 0.18.0 (plans/022)

Branch `feat/mole-lessons` from `3b6bde9`. Mole clone (untrusted data) in the session scratchpad.

- Baseline `verify.sh focused` passes on 3b6bde9; offline running.
- Plan 022 written: npm/pnpm narrowing, incident ledger gate, apply-stop formal model.
- Live evidence: pid 26224 typescript-language-server runs from ~/.npm/_npx (746 MB of 781 MB).
- Step 1 done: npm `_cacache` only, pnpm metadata folders only, apply reasserts both; 3 planted cases caught.
- Host degraded since ~11:40: `pgrep` fails (sysmond missing), `hdiutil create` fails; 7 host-dependent tests fail for that reason only.
- Step 2 done: docs/incidents.md (8 rows), audit + 13 controls in verify.sh and CI. Found: 33fe4e2's two catcher tests were deleted in 0.11.0 unnoticed; rows now name the evals that guard them.
- Host recovered: the 7 host-dependent tests pass.
- Step 3 done: bounded stop model (345 runs) in journal.rs; cf7962b's order replays D-07; 2 planted mutants caught; D-07 now caught by the model.
- Step 4 done: one restating comment deleted; the others carry constraints or sources.
- cccc recorded; prune-tests: one stop test rewritten to the behavior the model cannot reach, 8 journal-stop mutants proven.
- Offline gate passed (cae04c1). Reviewer + autoreview: npm apply now asks npm again (`is_npm_store`), pnpm legacy versions need digits per part, audit skips `#[ignore]`d/commented tests; all proven by planted cases/controls.
- mneves-verify round 1 FAIL: Go/Gradle could claim npm/pnpm installs (fixed 981663b), audit credited ignored catchers (fixed 60db621), D-08 dash (in-flight, filled at closeout).
- mneves-verify round 2 (Sonnet, host) FAIL on 48a05ff: linked install cache, compiled-out/orphan catchers, tagged in-flight version, pnpm wording; all fixed with controls (owner asked to fix until 100% satisfied).
- Round-3 reverify FAIL on 2f40898 (verifier ran as Opus, so not independent by model): npm `_npx`/pnpm `dlx` still reachable through every other caches authority, npm's reported root and pnpm's XDG cacheDir. Same bug survived three fixes: stopped for the owner. Audit catcher collisions fixed with controls.
- Owner (2026-11-26): "yes to all" — one choke point, Sonnet reverify that self-checks its model, ship 0.18.0.
- Choke point done: `InstallCaches::reached_by` at scan and apply for every caches authority; 7-row table eval with controls, apply unit test, 9 planted mutants caught. Docs updated.
- Next action: full gate, commit, merge PR #112, rebase, Sonnet reverify, release.
- Sonnet 5.5 reverify of a6e933d PASS: no caches authority offers `_npx`, `dlx` or an install cache whole (14+ fixture probes, 9/9 planted).
- PR #112 (prune-tests) CI was cancelled by the 30-minute verify limit; raised to 50 (9f85531), rerunning.
- Owner tagged v0.17.2-beta1/beta2 on main 6ba0b44 from another checkout; beta1 hit the runner's `hdiutil create` "Resource busy" flake; left untouched.
- Security audit (standard, budget 40): recon 4/4, 18 units, 16 hunters; wave 1 hunters h01–h04 running.
- `verify.sh offline` caught `caches/pnpm-whole` surviving: the install-cache rule masks the carve-out at apply; the tagged assertion now checks `is_builtin_cache_root` (0814a20). The manual edit left `video/film/facts.json` stale; regenerated (7765c43), film-controls 0 failures.
- PR #112 green (verify 24 min under the 50-minute limit) and squash-merged as eb3d175; feat/mole-lessons rebases onto it after the audit hunters stop reading the tree.
- Audit wave 1: h01–h08 ingested (ledger PASS). Candidates: Git discovery escaping an invalid `.git` marker (h01), Trash re-encoding a non-UTF-8 path (h02), configured `protect` ignored by `clean simulators` (h08). h09–h12 running.
