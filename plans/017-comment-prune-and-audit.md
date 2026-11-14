# 017 — Comment prune and whole-project audit (0.13.1)

Status: DONE in 0.13.1 (2026-11-14). Branch `chore/comment-prune-audit`, from `main` `79d50e4`.

## Request

Find the code comments worth pruning and keep the ones that must exist; plan
which comment-excused workarounds are best solved by refactoring and which can
simply be deleted; read the whole project and fix what is broken,
inconsistent, inefficient or no longer needed; verify and release.

## How the comments were judged

The rule already exists: `CODING_STANDARDS.md` S3 — a comment carries only what
the code cannot (a constraint, a threat, a platform or vendor fact). Five
read-only audits covered disjoint parts of the tree (safety core, categories,
terminal surfaces, tests and fuzz, scripts/CI/docs), after three mechanical
passes over every tracked source file:

| Pass | Result |
| --- | --- |
| `TODO` / `FIXME` / `HACK` / `XXX` | 0 |
| Commented-out code (Rust, Python, shell) | 0 |
| Backticked paths and identifiers in comments that no longer resolve | 0 (17 hits, all vendor files outside the repo: uv, Mole, swiftly) |

So the comments are, as a body, the kind that should exist. Most carry vendor
sources required by the deletion-list evidence rule, measured incidents, or
macOS behaviour (`df /` on the sealed system volume, `lsof` name escapes,
Darwin's transient `ENOENT`, `netstat` column counting). The prune is a list
of specific lines, not a sweep.

Load-bearing text that a prune must not move: the 215 `before=` anchors in
`scripts/tests/planted-violations.py` (checked after every batch), `PV` tags,
text `release-policy.sh` pins, CSP-hashed inline code, page lines quoted by
`video/film/facts.json`, and `AGENTS.md` == `CLAUDE.md`.

## Deleted or corrected (done in this change)

Restating, duplicated or wrong comments, by area:

- Safety core: the Go cache comment that repeated its `evidence` string; a
  deferred "ponytail" note and a restated convention in `scan_all`; a duplicated
  TZ note; owner-machine trivia in a test; the stale `dir_stats` doc ("every
  caller skips a zero-byte target" — `xcode` judges age); "macOS has no
  fd-relative unlink" (wrong: what is missing is unlink by object); "for the
  TUI" where `analyze` and `status --watch` install the same handler; the
  journal ceiling's "one record of overshoot".
- Categories, terminal surfaces, tests, scripts: see each fix commit; every
  item from the audits' "delete" lists was applied or is listed below with why
  it stayed.

## Workarounds a comment excuses — refactor or delete

| Where | Workaround | Verdict | Why |
| --- | --- | --- | --- |
| `src/safety.rs` `Ctx::protect` marker sentinel | an in-band marker distinguishes config-loaded entries from raw test values | REFACTOR (later) | a typed `ProtectEntry { literal, resolved }` makes the unmarked shape unrepresentable; production always marks, only tests and `fuzz_api` build the other shape. Touches the deny boundary, so it needs its own planted cases. |
| `src/ops/xcode.rs` "a folder holding no regular file yet" | `UNIX_EPOCH` sentinel from `dir_stats` | REFACTOR (later) | `dir_stats` should return "no regular file" as a value; the sentinel line is a planted anchor, so the refactor must move the case with it. |
| `src/ops/installers.rs` "why apply refuses…" | `judge()` runs twice, an arm covers the race between runs | REFACTOR (later) | judge once and match; the call is a planted anchor. |
| `src/ops/docker.rs` VM-image disclosure inside the error text | the disclosure rides in an error string | REFACTOR (owner decision) | returning image findings plus one error finding changes documented behaviour. |
| `src/ops/caches.rs` "only the standard cache is split" | a relocated `HOMEBREW_CACHE` with a clone is refused whole | KEEP | re-asking `brew --cache` at apply is possible, but nothing reported the relocated case. |
| `src/ops/caches.rs` / `agents.rs` ASCII-only evidence check | `evidence_is_meaningful` treats only ASCII whitespace as blank | REFACTOR (later) | reject Unicode whitespace; `GRADLE` has no runtime check, so U+00A0-only evidence would pass. Low risk: evidence is written by maintainers in review. |
| `src/analyze.rs` "not reachable while `same_file_system` holds" | a device check after walkdir enforced one device | DELETE | done in this change if walkdir guarantees it (see the UI commit). |
| `src/tui.rs` iCloud skips `filter_protected_findings` | a special case for a report-only category | DELETE | the filter only drops actionable findings; iCloud emits none. |
| `src/dashboard.rs` diagnostics pushed to `ctx` and taken back | round-trip through captured diagnostics | REFACTOR (done) | print the errors directly; the comment's claim about write failures was also wrong. |
| `src/ops/mod.rs` "ponytail: diagnostics use arrival order" | a deferred-work note | DELETE (done) | the order is a documented convention. |
| `rules/no-unbounded-subprocess.yml` status/uninstall exemption | report-only commands run unbounded subprocesses | REFACTOR (later) | route `status` and `uninstall` through `BoundedCommand`, then drop the exemption. |
| `.github/workflows/release.yml` x86_64 runner for pinned tools | release tools pinned to Intel assets | REFACTOR (later) | pin arm64 assets as CI does; pinned by `release-policy.sh`, unverified urgency. |
| `scripts/release.sh` "Slurped…" | `jq -s` beside a Python check that already refuses several documents | DELETE (done) | the comment; the check stays as defence in depth. |

"Later" items change a deletion boundary, a planted anchor or a pinned
workflow. Each needs its own planted case or policy update, and nothing has
reported a failure from any of them, so they are recorded rather than mixed
into a comment change.

## Fixed (behaviour), each with a test that failed first

- Journal rotation ran while building every command's context, so read-only
  commands renamed journal files and a TUI session never rotated; it now runs
  only in `journal::check_writable` (CLI and TUI apply start).
- `leftovers`: an unreadable `.supergoal` failed the category; `installers`:
  an unreadable folder or archive failed the category; both now withhold only
  what they could not read.
- `toolchains` read errors name the folder (S7).
- `node_modules` lockfile note: searched only the repository root; now the
  install's folder up to the root, and `unknown` instead of withholding the
  repository when a folder cannot be inspected.
- `scan` summary repeated a project's header; `analyze` exit status covered
  only the last folder shown; `status --watch` had no too-small screen.
- `trash-empty --confirm` messages named the whole Trash.
- `release.sh` exited without recovery guidance on a transient `gh` failure
  after the tag push.
- Gates: a `PV` tag extending a selected marker could count as another case's
  proof (now refused by the harness); 33 unselected tags lost their `PV`
  prefix; `eval/read-only` retargeted after rotation moved; film, CSP and
  planted-violation controls prove each plant fails for its own reason;
  fixture Git isolated from the caller's environment; tests restore saved
  modes; the `validate_path` fuzz oracle has a real home and reads the managed
  Library lists from the crate; `lsof` open-file parsers are fuzzed.

## Verification

`verify.sh focused` (baseline and after), `verify.sh offline` (MSRV, policy,
ShellCheck, planted violations 218/218, controls, cached audits, Gitleaks),
TruffleHog, actionlint, the video gate, fuzz (`validate_path`,
`probe_parsers`), Codex `gpt-6.1-sol` autoreview at P1 (scoped-clean), the
`reviewer` agent (PASS; each fix mutated in a scratch copy fails its test),
and `mneves-verify` before the tag.

## Known limit kept

`MAX_COMPLETE_HISTORY_BYTES` assumes about one record of overshoot per
generation, but a generation can grow by one apply's records before the next
rotation. Very heavy history can make `trash-empty --only-devtrim` refuse until
the next rotation. It refuses rather than guesses, predates this change, and
nothing has reported it.

## Rejected, with reason

- Folding the size measurement into the `artifacts` keypair walk and the
  `xcode` DerivedData age walk (each walks a surviving tree twice): real, but
  both touch planted anchors and must match `dir_size`'s hard-link and device
  rules exactly; machine load (~250) made `scripts/perf/ab.sh` timings
  untrustworthy, so it waits for a measurement.
- Dropping a missing `--root` from the scan-root list: `dashboard` preview
  commands deliberately carry every named root verbatim, so a folder created
  later is still scanned (`tests/dashboard.rs`
  `copied_project_preview_preserves_explicit_roots_and_inert_shell_data`).
- Removing the redundant re-checks inside the Codex-release and simulator
  apply paths: defence in depth at the deletion boundary is not dead code.
