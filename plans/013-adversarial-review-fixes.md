# Plan 013: Fix five adversarial-review findings and release 0.10.9

## What the review changed

Fix the five independently reproduced issues; retain only tested claims. Fix
the release page-measurement tool's reproduced delayed-paint and launch errors,
preserving its timing budgets and broken-page controls. Exclude
firmlink/provider hypotheses and foreign-mount explorer behavior until their
real triggers are established. Preserve intentional startup journal rotation.
Strengthen observable tests instead of adding a generic cleanup framework.

## Owner release decisions (2026-10-09)

The owner explicitly waived the 50 ms website/manual release condition and
instructed us to ignore the blocked external autoreview export and continue.
No external review export is authorized or attempted by that instruction;
the helper is waived, not passed. The already completed independent platform
reviews, black-box acceptance and final-diff inspection remain the review
evidence. Timing failures stay recorded, with the sampler and its controls
unchanged. CI, the full hosted mutation gate, immutable beta/stable provenance,
same-artifact promotion and Homebrew/Mac installation verification are still
required. The release script's exact-commit acknowledgment will record this
explicit owner waiver and the completed manual inspection; it must not be
reported as a successful external autoreview.

## Baseline, authority and ownership

Planned at `64ce5fccb76af4d0dc57643811b06b7c40d2cd6a`, 2026-10-08, clean
`feat/feature-evals` in `/Users/mneves/dev/devtrim`. P1 security/correctness,
medium scope, high safety sensitivity. User authorizes all five fixes, tests,
docs, version 0.10.9, commit/push/main merge, and beta/stable release. Stop before
data deletion, force-push or outside-repository changes without exact authority.
Inspect production Homebrew closeout before a script that invokes it. No new
dependency, async runtime, category, deletion entry or authority expansion.

Main owns safety.rs, shared planted-violations runner, docs/versions/plan,
integration and delivery. Journal worker owns journal.rs, the narrow complete
history caller in ops/mod.rs, and new tests/eval_safety.rs tests. Terminal worker
owns tui.rs/status.rs, tests/eval_reports.rs and scripts/tests/tui.py. Category
worker owns artifacts.rs/node_modules.rs and tests/evals.rs. Workers preserve
concurrent edits, do not spawn agents, commit, change docs/versions or inspect
cross-session memory. Main runs Cargo gates to avoid simultaneous heavy builds.
Fixtures use disposable HOME/PATH in target or ignored scratch; never real HOME.

## Frozen acceptance

1. Unreadable build CWD is an error, never a relative path proving inactivity.
   Both project categories refuse the failed shared probe. Valid active CWD
   still protects its repository; valid inactive control stays eligible and
   escaped absolute real paths still work.
2. `trash-empty --only-devtrim` judges every record still retained across all
   generations. Owned items remain recognizable beyond 1,000 newer results.
   Malformed/unreadable/over-limit complete history refuses everything. Ordinary
   limited `history` keeps newest-tail semantics, pairing, locking/no-follow
   reads and read-only behavior. Complete means either fully read or refused.
3. Every TUI results route detects Finding.scan_error and exits nonzero after
   a partial scan. Success exits zero; errors remain visible/non-actionable.
   Approval, selection, theme and terminal restoration stay intact.
4. A present modern Data-volume metric that fails or is malformed becomes
   unavailable with its reason and nonzero status. Root fallback is allowed
   only after positively establishing absence of the Data layout. Unknown
   layout is unavailable. Keep fixed argv and no new dependency.
5. Explicit/configured roots below node_modules or Git metadata namespaces
   offer no forbidden nested-install/artifact targets. ASCII case variants
   stay excluded, legitimate roots remain eligible, and apply guards remain
   independent. Root spelling never widens category authority.
6. Each new regression fails on baseline and passes with its fix. Safety
   refusals have positive controls plus named planted-violation proof where
   repeatable. No weakened gates or unrelated changes. Independent acceptance
   receives these criteria before the artifact.
7. Docs describe resulting behavior; AGENTS/CLAUDE remain byte-identical.
   Cargo/root lock/fuzz lock and packaged README/manual identify 0.10.9.
   Production landing page remains live stable until production proof.
8. Relevant gates and independent reviews pass before clean commit/push/main
   merge; exact-main CI precedes release. Next unused v0.10.9-betaN stages;
   v0.10.9 promotes the exact highest verified beta archive without rebuilding.
   Check remote checksum/provenance and visible binary behavior separately.

## Baseline findings and chosen implementation

`safety.rs:1739` rejects only empty lsof n fields, then creates a PathBuf.
Darwin reports CWD diagnostics in NAME; lexical build matching treats them as
inactive. Write real-binary active/inactive/error controls, observe red, then
require decoded names to be absolute paths without weakening escape checks.

`ops/mod.rs:1027` asks read_history for usize::MAX; journal.rs clamps to1,000.
Write owned-record and malformed-old-record controls beyond1,000 across rotated
generations, observe red, then add an explicit complete-history API sharing
hardened snapshot and legacy pairing machinery. Use a bounded retained-snapshot
byte budget, explicit refusal on overflow/incomplete input and bounded per-line
reads. Never merely raise the arbitrary count threshold. Preserve display limits.

`tui.rs::finish_results` checks only its errors vector; localized scan failures
are findings. Add a PTY error-exit regression with successful control, observe
red, then preserve failure state for scan_error findings on every results route.

`status.rs::collect` falls back on every Data capture/parse error. Add successful
Data/failing Data/malformed Data/legacy-layout controls, observe red, then choose
the disk tool based on positively known filesystem layout, reporting inspection
or metric failures. Keep the fixed SystemTool abstraction.

Artifact/node_modules walkers exclude encountered entry names but miss excluded
ancestors above their root. Add real-Git explicit/configured/case-variant roots
plus eligible controls, observe red, then exclude that namespace before walking.
Keep independent apply checks. Do not change ownership/activity or candidate lists.

## Research, alternatives and future maintenance

Apple's pinned [lsof Darwin provider](https://github.com/apple-oss-distributions/lsof/blob/7a8a1b2a3c0f35c30a5fcd0927f31d441c3e5255/lsof/dialects/darwin/libproc/dproc.c#L473-L496)
emits non-ESRCH CWD errors in NAME. Apple documents
[APFS volume groups](https://support.apple.com/guide/disk-utility/add-delete-or-erase-apfs-volumes-dskua9e6a110/mac).

Choose direct owner fixes with explicit error/completeness contracts. Reject a
generic history/scanner/metric framework: wider review surface without a better
guarantee. Reject raising history's arbitrary cap: it only moves the failure.
Premise-challenging alternative: remove only-devtrim and legacy fallback; simpler
but unnecessarily discards supported features. Five years later, behavior tests,
positive controls, explicit complete-vs-display semantics and independent apply
rechecks are durable; tests must avoid pinning incidental structure or wording.
A fresh gpt-6-astra advisor challenged these decisions before implementation.

That advisor completed its read-only review before the production edits. It
added apply-time lsof drift, all retained journal generations with legacy
pairing, cumulative TUI failure after a later success, typed `NotFound` rather
than `Path::exists`, and the exact-install-root positive control. The chosen
implementation includes those refinements. Rust's [Path documentation](https://doc.rust-lang.org/std/path/struct.Path.html#method.exists)
confirms that existence checks can hide permission errors. Applying
[OWASP's deny-by-default guidance](https://cheatsheetseries.owasp.org/cheatsheets/Authorization_Cheat_Sheet.html)
to cleanup authority supports explicit refusal when required evidence is
unknown; it does not justify adding new cleanup targets.

## Ordered verification and delivery

1. Tests first; main saves red/green logs in `.scratch/review-013/`. Run exact
   new tests through `rustup run 1.98.1 cargo test --locked`; then the affected
   integration crates and python3 scripts/tests/tui.py target/debug/devtrim.
2. Add named safety mutants to scripts/tests/planted-violations.py. Each must
   compile and fail its tagged assertion. Run new selectors, then full set.
3. Execute bash scripts/verify.sh focused then offline. Read output and preserve
   exit codes. Sandbox restrictions are BLOCKED until the affected gate succeeds
   in its required environment. MSRV Rust 1.88 must actually execute.
4. Remaining release gates from AGENTS: fresh root/fuzz cargo audit, Gitleaks
   runtime positive control plus history scan, TruffleHog, five 60-second fuzz
   targets, video ci/audit/lint/format/build, arm64 release build, shell/policy and
   actionlint. Never use build or fixture proof as native deletion evidence.
5. Update Cargo.toml and both locks, dated CHANGELOG, README/SECURITY/manual,
   AGENTS/CLAUDE, plan and current journal. Keep live stable index until promotion.
   Run format/clippy and all affected gates after final changes.
6. Apply improve deep to the finished scoped artifact and fix checked findings.
   User's explicit execute scope overrides its advisory-only main restrictions.
   Matt Pocock two-axis code review checks standards and this frozen spec against
   baseline-to-final diff. Run local autoreview and fresh different-model
   mneves-verify with real-binary positive controls; correct confirmed findings.
7. Inspect final diff/status, stage explicit task-owned paths (requested add all),
   Conventional Commit, normal push/PR/CI/main merge, restore visible main.
8. Check unused remote tags and immutable releases, stage the next 0.10.9 beta after
   exact-main CI and local review acknowledgment, download checksum/attestation/
   PTY proof. Promote same commit/archive only after beta gates and any required
   outside-repo closeout approval. No production success inferred from a tag.
9. After production proof update stable landing links and next Unreleased patch,
   verify served endpoint. Delete only explicitly authorized merged branch/task
   artifacts; ask before data or unrelated scratch cleanup. Retain cited evidence.

Final branch cleanup is explicitly authorized by the owner's latest instruction.
The current inventory has one checkout, on `feat/feature-evals`, and no auxiliary
worktree. After required gates and delivery pass, restore this checkout to
`main`, verify the task commit is included, and remove the merged task branch
locally and remotely using normal deletion. The local
`archive/codex-ef2c-01f5ef05` currently points to the same commit as `main`
(`01f5ef0`); remove that redundant label at closeout only after rechecking that
it still has no unique commits and no worktree uses it. Never force-delete an
unmerged branch or remove unrelated worktrees or scratch data.

## Stop conditions and completion

Unknown required credentials/private access; a required outside-repo write;
overlapping unrelated edits; an exact gate that cannot honestly pass; unavailable
mandatory native proof; destructive cleanup needing approval. Finish unaffected
work first and report exact blocked gate. No force-push or speculative refactor.

## Checked issues and progress

All five bug regressions failed at their intended behavioral assertion before
the respective production change. Evidence is in `.scratch/review-013/`:
`red-liveness.log`, `red-liveness-drift.log`, `red-journal.log`,
`red-roots.log`, and `red-status.log`. The terminal worker independently
observed the corrected PTY regression failing at exit status 0 versus 1;
the earlier parent PTY timeout is not counted as red proof.

Reproduction and fixed-behavior checks use disposable fixtures:

| Issue | Reproducible check | Expected fixed result |
| --- | --- | --- |
| Unreadable build CWD | `cargo test --locked --test eval_system eval_unreadable_build_cwd_refuses_project_cleanup` and `eval_build_cwd_that_becomes_unreadable_after_preview_refuses_apply` | All three project commands refuse unknown liveness; active/inactive controls distinguish real eligibility; drift changes nothing |
| Truncated Trash ownership | `cargo test --locked --test eval_safety eval_trash_` | Owned identity in oldest retained generation is selected beyond 1,000 newer results; malformed/bounded-history failures select nothing |
| TUI hides scan failure status | `python3 -B scripts/tests/tui.py target/debug/devtrim` | Visible localized error exits 1 and restores terminal; healthy control exits 0 |
| Data failure hidden by root metric | `cargo test --locked --test eval_reports eval_status_data_failure_never_uses_sealed_root` | Disk unavailable and nonzero on failed/malformed Data metric; valid Data control works |
| Root bypasses namespace pruning | `cargo test --locked --test evals eval_project_roots_` and `eval_root_at_top_level_node_modules_still_offers_the_install` | Both root modes/case variants exclude nested targets; legitimate roots and exact install stay eligible |

Run Cargo through the pinned toolchain as `scripts/verify.sh` does. Main
integrated the disjoint worker diffs. The process-visible all-target/all-feature
run passed 508 tests (two intentionally ignored), Clippy, structural checks,
PTY and read-only terminal checks. New root tests also pass after fixture
setup was made cheaper without reducing the matrix. Git automatic maintenance
is disabled while fixture repositories are constructed so a background lock
cannot race the byte-for-byte snapshot.

The opt-in measurement tool also had checked defects: a page revealing its
first content 600 ms after load returned no LCP; an early small paragraph hid
a larger candidate revealed at 200 ms; a missing/early-exiting Chrome left its
profile behind and returned an uncontrolled error. RED/GREEN controls now
prove all three behaviors. The chosen bounded quiet-window observation also
waits for eager images and fonts, keeps foreground visibility mandatory, and
discloses that later dynamic content is outside the measurement. A longer
fixed sleep was rejected because it still guesses when paint occurs.
`pageload-final-green.log` contains the control evidence. The full control
suite still exits 1 because the healthy desktop page exceeded its unchanged
200 ms budget (p95 load 478.5 ms, LCP 528 ms). This is a performance FAIL,
not proof of a remaining missing-paint error. Host load was 762.97; that
observation alone does not prove the cause. The unchanged 30-run/50-ms
site/manual measurement is opt-in as a command, but Plan 012 requires it for
this release.
The final corrected 30-run measurement completed all 120 samples and exited 1:
load medians were 208.4–242.4 ms and LCP medians 228–284 ms; every viewport
exceeded the unchanged 50 ms budget. Raw evidence is
`.scratch/review-013/pageload-settled-final.json`. This release-gate performance
failure is retained explicitly rather than folded into functional PASS claims.
Plan 012 originally required that budget before a beta tag. The owner explicitly
waived it on 2026-10-09; the failed measurement remains separate evidence.

A final control review reproduced another measurement-proof defect: synthetic
reports naming an initial candidate passed both delayed-paint controls when
their timestamps alone exceeded the delays. The complete-report positive
control passed, while both stale-candidate regressions failed at
`PV pageload/control-candidate` in `red-paint-control-candidate.log`.
Samples now retain `LargestContentfulPaint.id`, and the delayed fixtures require
their intended ID as well as the existing timing thresholds. The
[LCP working draft](https://www.w3.org/TR/2026/WD-largest-contentful-paint-20260826/#dom-largestcontentfulpaint-id)
defines that candidate field. Checking a timestamp relative to load was
rejected because scheduler delay can still make an initial candidate look late;
observing the intended element addresses that ambiguity directly. Both tests
pass in `green-paint-control-candidate.log`. This browser-free validator now
runs in local checks, ordinary CI and read-only release gates. Real Chrome
caught a missing serializer field during integration; it now preserves the
observed ID in each reported sample. The final run observed the intended
candidate in both delayed fixtures and passed all eight negative/paint
controls. The suite still exited 1 because the healthy mobile page exceeded
the unchanged 200 ms budget (p95 load 344.5 ms, LCP 396 ms); desktop was within
budget. Evidence is `paint-candidate-browser-controls-final.log`. This is a
timing FAIL, not a remaining candidate-observation failure. Workflow lint,
shell syntax/ShellCheck, release policy and agent-doc equality passed after
the new gate was added.
After the mutation workers finished, the completed tool ran one final unchanged
30-run/50-ms check. All 120 samples were valid; every page/viewport still failed.
Load medians were 150.8–202 ms and LCP medians 184–224 ms. Evidence is
`.scratch/review-013/pageload-candidate-final.json`. The owned measurement server,
isolated browser and test runners stopped. The later owner waiver is recorded
above; it does not turn these measurements into passing results.

Fresh deep/standards, independent spec-axis, and dedicated Astra security
reviews found zero confirmed additional source defects. Fresh root/fuzz audits,
Gitleaks positive control/history scan, TruffleHog, and video install/audit/lint/
format/build and all five bounded fuzz targets have passed. All thirteen new
`review013/` mutations were caught at their tagged assertions in the full run;
the first full catalogue run caught 89 assertions, then timed out rebuilding
within its remaining budget. It is partial proof, not a full gate PASS. The
retry disables only debug-symbol generation in disposable dev/test builds
(`CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`); all 149 cases,
debug assertions, optimization level and time limits remain unchanged.
[Cargo's profile reference](https://doc.rust-lang.org/cargo/reference/profiles.html#debug)
distinguishes debug information from the separate debug-assertions setting.
That retry stopped at an unmutated excluded-root fixture's 120-second limit,
before mutation proof. The fixture now shares one healthy control across case
variants and uses distinct parents for the excluded variants on APFS. Each
test still exercises all 12 excluded combinations (two spellings, explicit and
configured roots, and three commands), plus six healthy combinations. No gate
deadline or assertion was relaxed. Both optimized namespace tests passed on
Rust 1.98.1; those two and the exact-install positive control also passed on
Rust 1.88. That full run caught 105 assertions, then correctly rejected a
stale existing `trash/incomplete-history` binding: removing the caller's
errors-list check could not bypass the complete reader's earlier refusal.
The control now models swallowing a complete-read error as empty successful
history. Its existing positive and malformed-history assertions remain
unchanged, and the focused control passed at its exact tagged assertion.
The normal-concurrency full retry passed every selected baseline but timed out
rebuilding at the one-hour catalogue deadline after 52 assertions. Normal
Cargo concurrency did not resolve the resource limit. Across the attempts,
105 distinct cases had tagged proof. All nine supported prefix batches then
passed, catching 69 controls and closing the remaining 44-case gap. The
inventory in `.scratch/review-013/mutation-coverage-013.json` accounts for every
one of the 149 cases and 150 assertions, with no missing proof and current
source hashes. No case, assertion or deadline was relaxed. This complete split
proof remains separate from the failed single-run gate; exact-candidate hosted
verification must pass the unchanged full gate before merge and release.
The separate full Rust 1.88 run
passed all 508 tests (two intentionally ignored). Its first invocation omitted
`/usr/sbin` from the isolated PATH, so two native `lsof` controls failed to
launch; restoring that path resolved the environment error without a source
change. `msrv-process-tools.log` is the successful run.
Fresh-context Astra black-box acceptance passed all five criteria through the
real binary and a real PTY, including positive controls and unchanged-tree
assertions, without reading source, diffs or implementation tests. Its evidence
is in `.scratch/verify-013/blackbox-k_hs1qbw/`. Native Trash, simulator and
maintenance execution were outside that acceptance. An initial mutation
baseline timed out under host load;
that run earns no mutation proof. The arm64 release build passed with a local
toolchain warning that `rust-objcopy` could not load its LLVM library; hosted
artifact verification remains mandatory.

The owner's later instruction explicitly authorizes Homebrew publication and
installation on this Mac, including the existing closeout's separate tap and
`/opt/homebrew` writes. No renewed approval is needed for that exact delivery.
Those delivery checks subsequently passed; see the completed delivery below.

The local autoreview dry run passed isolation/input preparation, but both real
invocations were rejected by automatic approval before execution. The second
selected Codex Astra and removed inherited endpoint overrides; the stated
blocker was explicit authorization to export uncommitted content to OpenAI.
On 2026-10-09 the owner instructed us to ignore that blocked export and continue.
The external helper is waived; no provider review or clean verdict is inferred
from the dry run. The independent on-platform reviews and acceptance above
remain separate evidence.

## Completed delivery (2026-10-09)

- Fix commit `93e2cf4b82002b8fa347e15d9e8063eccda54939` passed PR CI
  [37882176303](https://github.com/mneves75/devtrim/actions/runs/37882176303).
  [PR #30](https://github.com/mneves75/devtrim/pull/30) was squash-merged after
  normal merge was refused; the merged file tree is identical to the reviewed
  commit. Main `adbadf498bcc6fad7bd0d514221bf87ef956df77` passed exact-main CI
  [37883840310](https://github.com/mneves75/devtrim/actions/runs/37883840310).
- The unchanged full hosted mutation gate passed in PR, main, beta and
  production: all 150 tagged boundary assertions. This supersedes the pending
  hosted gate above, not the recorded local timeout. Rust 1.98.1 and 1.88 each
  passed 508 tests (two intentionally ignored). Fresh audits, secret scans,
  video, shell/policy, terminal and all five fuzz gates passed.
- Immutable [v0.10.9-beta1](https://github.com/mneves75/devtrim/releases/tag/v0.10.9-beta1)
  and [v0.10.9](https://github.com/mneves75/devtrim/releases/tag/v0.10.9) point
  at the merged main commit. Beta run `37885234145` and production run
  `37890552663` passed. Production skipped building and packaging; independently
  downloaded archives compare byte for byte. ZIP SHA-256:
  `b273e1527f4ca872ee58799e1edec8b554bfc8025f33c5ea10778ddce4cc7bcf`.
  Immutable manifests and repository/workflow/source attestations verified;
  the wrong-source control failed specifically for its digest mismatch.
- Homebrew tap `9c7bdea4fa8a6add5d2fc84e3e2aa24a0da597a9` publishes the exact
  stable URL/checksum. Strict online audit, upgrade and formula tests passed.
  `/opt/homebrew/bin/devtrim` is the sole visible binary, reports `0.10.9`,
  and compares byte for byte with the beta executable. Both downloaded and
  installed binaries passed real PTY TUI and read-only-view suites against
  disposable homes. Logs and release/signature JSON remain in
  `.scratch/review-013/`; independent black-box evidence remains in
  `.scratch/verify-013/`.
- Final docs advance the stable landing page only after release/install proof
  and open `0.10.10` as Unreleased. Cargo and both lockfiles stay at 0.10.9.
  Remove merged working branches and task-created uncited caches after final
  documentation delivery; retain cited proof and unrelated pre-existing data.

The two owner waivers remain explicit, not PASS results. The 120 valid timing
samples still failed 50 ms. Native Finder Trash, simulator and maintenance
execution remain unproven end to end. The implementation deliberately keeps
focused, independently checked fixes rather than adding a broader cleanup
framework or expanding deletion authority.
