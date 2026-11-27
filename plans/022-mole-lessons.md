# 022 — Lessons from Mole, made structural

Base `3b6bde9` (0.17.1), branch `feat/mole-lessons`, target 0.18.0. Mole read at
`ff7aac5a` (V1.74.0 + 68), 1183 commits since V1.56.0, `docs/incidents.md` (113 rows).

## A. Expert diagnosis

1. **What a top practitioner would do unasked.** Treat Mole's incident ledger as an eval set
   and check devtrim against it, row by row, instead of porting features. Most Mole fixes since
   V1.56.0 are about one thing: something devtrim removes is still in use by a program the user
   is running. Example: `_npx` installs that an MCP server is loading as it runs. The first job
   is to find where devtrim does that today, prove it on this machine, and close it with owner
   evidence. More features are the second job.
2. **Rawest material.**
   - Mole's `docs/incidents.md` and the fix commits it names (`f954e2fd`, `c32c7274`, `6f2c0ce5`).
   - npm 12.2.3's own source.
   - devtrim's git log. Its 47 PRs have no review comments, so the rationale lives in commit
     bodies, `memory/` and `CHANGELOG.md`.
   - The live machine. On 2026-11-25, process 26224 (`typescript-language-server`) ran from
     `~/.npm/_npx/94207acffe67148d/`. `_npx` holds 746 MB of the 781 MB npm cache, and devtrim
     offers the whole folder.
3. **Methods and tools.**
   - Vendor-source evidence for every narrowed deletion entry.
   - Test-first planted fixtures through the real binary (`support::Tree`).
   - Mutation proof in `planted-violations.py`.
   - An exhaustive bounded interleaving test that drives the real `admit`/`begin`/stop code.
   - A ledger audit that runs in `verify.sh` and CI.
   - `cccc` for the test-pruning boundary.
4. **What else shows the result is right.**
   - The vendor's own code: npm `cache clean --force` removes `<cache>/_cacache` and nothing
     else (`@npmcli/config` definitions.js:443-444).
   - The pre-fix source: each new check must fail on `git show <fix>^`.
   - The hosted CI run.
   - A real preview on a disposable HOME showing `_npx` and `dlx` no longer offered.

## B. The task, rewritten (deeper, not bigger)

> Find where devtrim can still remove something a running program depends on, using Mole's
> incident ledger as the eval set, and close each confirmed case with owner-source evidence and
> a check that fails on the pre-fix code. Make devtrim's own repeated mistakes traceable by
> giving every user-met defect a ledger row whose catcher a gate proves exists. Replay the one
> real ordering bug in its history (cf7962b) as an exhaustive model that fails on the old
> ordering and runs in CI. Ship each step as its own verified commit. Defer everything else with
> a reason instead of half-building it.

## C. Verification and the five-year critique

- **Proof standard.**
  - Every new check fails on the actual pre-fix code (`git show <sha>^:<file>`, or the 0.17.1
    binary), not only on a synthetic plant. Each guard gets a negative control.
  - Security and legal tests are never pruned.
  - `cccc` ≤5 code gets no new unit test unless it holds a regex, a path or safety boundary,
    deletion or concurrency.
- **Five years out.**
  - Feature ports age. The ledger and the model are what last: a defect without a row, or an
    ordering change the model rejects, fails a gate a future agent cannot miss.
  - The narrowed npm and pnpm entries hold up because they rest on the owner's own clean
    command rather than a folder name.
  - What would look wrong in five years is ten hand-written apply loops and about 25 hand-rolled
    folding sites. They are recorded below as the next structural work, not started here,
    because 236 planted cases anchor on them.

## Red-team (six months on, it failed because…)

| Likely cause | Trigger | Cost | Cheapest early experiment |
|---|---|---|---|
| pnpm renames its metadata folders | pnpm 13 uses `v13/meta*` | pnpm offers nothing (fails safe) | The near-miss fixture pins the shape; a release note names it |
| The model checks a model, not the code | A driver drifts from `caches.rs` | False confidence | The model calls the real `admit`/`begin` on a real journal; planted `journal-stop/*` cases still bind `caches.rs` |
| Ledger rows become busywork | Catchers named loosely | Gate passes vacuously | The gate resolves each catcher to a planted `Case.name` or a defined Rust `fn`, and fails on the plant |

A simpler route was considered: a process guard (`pgrep npx`) instead of narrowing. It was
rejected because npx children run as `node`, so the name match is a heuristic. Narrowing to what
npm itself cleans needs no probe. **Verdict: ship.**

## Steps (one verified commit each)

1. **npm and pnpm: offer only what the owner cleans** (Mole I-11, `f954e2fd`, `c32c7274`).
   - **npm.** The npm finding becomes exactly `<npm cache>/_cacache`. A new
     `TargetAuthority::NpmCacheStore` makes apply reassert the exact child of the npm-reported
     root. `_npx`, `_logs` and `_prebuilds` stay.
   - **pnpm.** The pnpm carve-out stays the protection boundary. The `caches` finding becomes
     each `Library/Caches/pnpm/v<digits>/metadata*` folder (here 899 MB of 916 MB), and apply
     reasserts that shape. `dlx`, `lockfile-verified.jsonl` and transient folders stay.
   - **Tests and proof.** Evals plant `_npx`, `dlx` and an unknown `v12/pkg` as near misses
     beside the positives. A planted case breaks each apply reassertion. The eval fails on
     `3b6bde9`.
2. **Incident ledger and audit gate.**
   - `docs/incidents.md` holds one row per user-met defect: ID, date, class, fix SHA, catcher.
   - `scripts/tests/audit-incidents.py` checks that each SHA exists and each catcher resolves to
     a planted case or a defined test `fn`. It also rejects duplicate IDs and malformed cells.
   - `scripts/tests/audit-incidents-controls.py` plants each failure. Both run in
     `verify.sh focused` and CI.
   - Rows are seeded from the mistakes report (cf7962b, 728ec4f, 33fe4e2, b118350, 13ccd96, …)
     plus this plan's ports.
3. **Formal model of the apply stop protocol** (cf7962b).
   - An exhaustive bounded interleaving test in `journal.rs`. It covers every sequence of up to
     three attempts from {plain, uv, uv failing before begin}, with the stop placed before every
     step. It drives the real `admit`/`begin`/`finish` on a temp journal.
   - **Invariants.**
     - No write before begin without a journal record.
     - No attempt after the stop without an admission.
     - An admission lets at most one begin through, and one dropped before begin lets none
       through.
   - **Negative control.** The pre-cf7962b driver order (`refuse_if_stopped` → lock → `begin`)
     must fail with a printed counterexample trace.
   - Runs in `cargo test`, so CI already gates it.
   - A project-local `.claude/skills/verify-apply-stop/SKILL.md` documents the one command and
     its scope.
4. **Comment pass (`/no-comments`).** Over the files this plan touches only. Delete comments that
   restate code, and turn any that hold a constraint into a type or test where cheap.
5. **Closeout.**
   - `cccc` evidence, then `mneves-prune-tests`, then `mneves-verify` until it passes.
   - Run the reviewer agent and autoreview.
   - Bump to 0.18.0 and update the docs.
   - Run the security gate, then beta, then production and Homebrew.

## Deferred, with reasons

- **Open-file probe for `artifacts`/`node_modules` (I-52).** It is another `lsof` heuristic, the
  class devtrim has patched one review round at a time. It needs a single probe module with a
  complete-or-unjudgeable result first, and a measured cost (a system-wide lsof took 16 s under
  load).
- **Process guards for bun/bunx, cargo `registry/src`, gh, the Go module cache with gopls, and
  VS Code.** Each needs owner evidence per tool; the bun guard is the most urgent.
- **Docker `image prune -a`.** Mole prunes dangling images only (I-38, `4075102a`). A tagged local
  image can be unrecoverable, but narrowing changes what devtrim reclaims. This is the owner's
  product decision.
- **Codex history and `.codex/cache/codex_app_directory`.** Mole calls it a conversation index;
  devtrim's evidence calls it a catalog. Settle it on a live Codex install before changing it.
- **Dataless pruning in `analyze`/`icloud` (I-86/I-91).** `SF_DATALESS` = `0x40000000`, verified
  in the SDK's `sys/stat.h:359`. It needs a real dataless fixture to prove, and none can be
  created without iCloud.
- **Analyze hard links across siblings (I-77) and the `walk_stats` device check (I-69).** They
  only inflate previews, and the sink refuses foreign devices.
- **A shared apply driver, one volume-identity type, and a clippy ban on `Path::exists`.** These
  are the right next structure. Each invalidates many planted-case anchors, so each needs its own
  plan.
- **`simctl` first-launch gate (`a0cf037c`) and the `node_modules` local-edit check
  (`52c5a7fa`).** Both are real Mole fixes but outside this plan's in-use theme.
