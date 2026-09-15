# Project Memory

## Current state (0.9.4)

Production is verified. `v0.9.4-beta1` and `v0.9.4` both point at `728ec4f`;
production reused the beta archive byte for byte (ZIP SHA-256
`14d759aacbd3725a46a34b095e4a638589c96531acc4f26d57b779ad32a6dce2`), the
release is immutable, and the attestation verifies against the tag with
self-hosted runners denied. The new digest handoff step ran in both hosted
workflows. The downloaded beta passed the PTY TUI flow including type-ahead
discard, the read-only views, and end-to-end checks that a hostile
`.git/config` runs nothing during preview (with a plain-`git` control that did
run it) and that `trash-empty --apply` refuses without consent. Homebrew
installs and tests 0.9.4 as the sole visible `devtrim`.

A whole-codebase review found the two things this tool must never do — act
during a *preview*, and act on a screen nobody read — both reachable. A dry-run
scan ran programs named by a scanned repository's `.git/config`
(`gpg.program` through `log.showSignature`, a promisor remote's `uploadpack`
through a lazy fetch), and TUI keys typed during a scan approved a permanent
plan that was never displayed. Both were reproduced end to end, fixed, and are
now proven by fixtures that arm exactly one path each, with positive controls.

The same pass found `trash-empty --apply` bypassing the shared confirmation, a
Trash acknowledgment measured over a different set than the plan, a
check-then-rename restore that could overwrite a recreated file, clap errors
carrying bidi controls from argv, `lsof` display escapes defeating liveness,
DerivedData liveness blind to Xcode.app builds (`SWBBuildService`, observed with
`Xcode` as parent), and a release publisher attesting an artifact bound only by
name. `release-policy.sh` passed with `write-all` and friends; it now reads
permissions from parsed YAML because line matching kept missing spellings.

Staleness had a false-positive shape nobody had tested: an old project cloned
today read as stale because only HEAD's commit date counted. Activity is now the
newer of HEAD's commit date (read on its own) and the newest HEAD reflog entry.
The first version read the commit date *through* the reflog walk; autoreview
showed the newest entry need not name HEAD.

Process that worked: three Claude reviewers plus a correctness hunt plus a Codex
autoreview of the entire tree (a synthetic commit adding the tree onto an empty
root, in a frozen clone), then autoreview on the fix diff, one rerun,
scoped-clean. Every reviewer found something the others did not; the Codex pass
independently confirmed the git-config execution. `planted-violations.py` went
from 6 to 12 proven boundaries.

Environment lessons: the host ran at load average 160–460 from other sessions,
which made a 5-second test module take 318 s and once made a planted mutant
"fail to compile" without saying why — the gate now prints the compiler tail.
A PTY test home under the system temp dir resolves into protected
`/private/var`, so apply there is refused; use the build directory.

## Previous state (0.9.3)

0.9.2 made deletion evidence a required field — and shipped an entry whose
evidence was false. `~/Library/Caches/gh` was described as the GitHub CLI's
cache; go-gh resolves that to `$XDG_CACHE_HOME/gh` then `~/.cache/gh` and never
to `~/Library/Caches`. Verified here: `XDG_CACHE_HOME` unset, `~/.cache/gh`
present, `~/Library/Caches/gh` absent. The entry had carried that authority
since 0.9.1 on the strength of its name. It is gone; gh's real cache is listed
with evidence that states the resolution order.

That is the lesson worth keeping: **requiring evidence does not make the
evidence true.** The mechanism worked exactly as designed while one of the
strings it forced me to write was simply wrong. Only a review that went and
read the vendor's source caught it.

Three further rounds each found the claim running slightly ahead of the
mechanism: a doc saying "proven below" applied to three tests when one had
assertions; those assertions checking the const helper rather than the loop they
sat in; and `every_agent_entry_carries_evidence` holding two loops where only
one was planted. `planted-violations.py` now carries six cases — two deletion
boundaries and four per-list evidence loops, markers kept mutually
non-prefixing — and the gate leaked 558 MB per early failure until cleanup moved
into a `finally`.

Process note: the 0.9.2 release attestation (`DEVTRIM_AUTOREVIEW_COMMIT`) was
given without running autoreview. Running it afterwards is what found all of the
above. An attestation gate is worth exactly what the attestation is worth.

## Previous state (0.9.2)

Two lessons that had lived only as prose are now structural. Evidence is a
required field: `safety::DeletionEntry` and `HistoryRoot` carry it, so a new
deletion-list entry without justification does not compile, a blank one fails a
const assertion, and a test names the offending path. Six entries had already
reached a release on the strength of a directory *name*; that specific mistake
can no longer be made quietly. What the field cannot do is judge whether the
cited source supports deletion — that stays review's job, and the standard says
so rather than implying the gate is stronger than it is.

`scripts/tests/planted-violations.py` answers the sharper failure: this repo
shipped a symlink-refusal assertion that could not fail, and nothing noticed for
several commits. The gate breaks each guarded branch on a throwaway copy and
requires the *tagged* assertion to fail, rejecting a mutant that does not
compile, selects no test, or fails elsewhere. Deleting the symlink branch is
deliberately not the mutation — the file-type check below also rejects a link —
so it makes a symlink positively eligible instead. Two fixed cases, ~14 seconds,
wired into `verify.sh offline`, CI and the read-only release job, with
`release-policy.sh` requiring all three and forbidding a fourth in the
credential-bearing script.

The plan came from GPT-6 Astra, which usefully argued *down* a broad
`cargo-mutants` gate (its "caught" means any test failed, not the intended one),
an evidence framework, and new gates for the regenerable paths. Every guard was
proven by breaking what it guards: E0063, a const-evaluation panic, the named
path, MISSED, ERROR, and the policy failure.

## Previous state (0.9.1)

0.9.0 justified every closed-list entry by what its directory was named. 0.9.1
checked all fifteen load-bearing claims against the vendors' own documentation
instead. Nothing was contradicted — no rule called a directory safe to delete
when it is not — but two entries rested on no source at all and are gone:
`~/.claude/downloads`, which Anthropic documents nowhere and which was empty
everywhere it could be examined, and `~/Library/Caches/claude-cli-nodejs`, which
holds per-project MCP diagnostic logs that nothing regenerates. `.codex/cache`
stayed, with its comment walked back to say it rests on direct inspection rather
than on a vendor source that does not exist — applying the standard to one
directory and exempting the next is the inconsistency the release existed to
remove.

Three review axes then found what the gates could not, and all of it was in the
tests rather than the binary. A symlink-refusal test had been silently disarmed:
its fixture sat under a root retired during 0.9.0 review, so the scan never
reached the symlink check and deleting that check would have left the suite
green. SECURITY.md still documented the corroboration rule removed with that
same root. And fixing those removed the only assertions guarding the
"`projects` and `jobs` are never roots" boundary, so that sentence is now
executable — asserted in both directions, because an *ancestor* entry like
`.claude` would have passed a downward-only check while making both trees
deletable. Each new assertion carries a planted-violation proof.

0.9.0 had also made every category fail-closed on an unreadable timestamp when
only the age gate reads one; `dir_stats` reports the gap now and lets the
staleness caller be the one to refuse.

The durable lesson, across eleven review rounds: the enforcement was never the
problem. Every single defect was in what the closed lists contained, or in a
test that asserted a count where it should have asserted a reason.

## Previous state (0.9.0)

A tenth category, `agents`, covers coding-agent storage in two tiers. The split
is the whole design: a regenerable cache costs a re-fetch, a session transcript
costs the transcript, and one danger score cannot honestly describe both. The
history tier is age-gated on the newest *regular file* in the subtree, because a
directory's own mtime moves on creation and on removal — a store restored from
backup would otherwise read as permanently active. Codex nests sessions as
`<year>/<month>/<day>`, so `HistoryRoot::depth` makes the day directory the
unit; waiting for a year to go stale would never offer the current one.

Two stores were measured and left out rather than shipped. `.codex/lanes`
produced 558 findings for 0.25 GB against the development machine — a preview
nobody can read is not a preview, and the natural unit is a lane, which the
on-disk naming does not express. The paste cache has the same shape and is worth
2 MB. Dropping both took the category from 592 findings to 34 for 2.51 GB.

`~/Library` is still protected wholesale. `safety::MANAGED_LIBRARY_CACHES` is the
closed carve-out for its `Caches` subtree and is read by both the protection
boundary and the cache category, so the two cannot drift into previewing a path
the sink refuses. The XDG spellings of tools that use the platform location on
macOS were removed rather than listed twice: two findings sharing one label read
as a duplicate, not as two places.

Three independent review axes each found something no gate could. The standards
axis: `metadata.modified()` failing silently would have let a subtree read older
than it is, so an unreadable timestamp now refuses instead of abstaining; `clean
agents` had no CLI-level test; a negative assertion about `include_files` had no
positive control. The spec axis: the docs claimed VS Code and JetBrains *logs*,
which live under `Application Support` and `Logs` — the claim was narrowed
rather than the boundary widened.

The independent model review found the one that mattered. Claude Code stores its
auto memory at `~/.claude/projects/<project>/memory/`, and keys memory by
repository root while keying transcripts by working directory — so a project
directory can hold memory and no live transcript, go stale, and be deleted. The
directories exist on this machine; the category would have destroyed them. The first
fix narrowed the unit to session-shaped entries; a later pass retired the root
outright, for the reason recorded below. The same
review showed a shell snapshot is not a cache — it is written once per session,
sourced by every later shell call, and never rewritten — so both snapshot
directories moved to the age-gated tier; and that `Caches/JetBrains` is the IDE
*system directory* holding the non-regenerable `LocalHistory` store, so it left
the carve-out entirely.

The lesson is the one this repo keeps relearning: the enforcement was sound in
all three cases. What was wrong was what the closed lists contained.

Running it for real against a full disk then found what no test had. `caches`
apply stopped at the first refused finding, and the `uv` cache — permanently
unremovable because a source distribution inside it carries its own `.git` —
sorts first, so it blocked all eight remaining caches and reported zero while
4.56 GB sat there. Apply now continues and records every failure. The same run
showed two more roots failing the ratio rule: `.codex/.tmp` gave 324 findings
for 0.07 GB and `.claude/file-history` 123 for 0.10 GB, so both left the list
alongside lanes and the paste cache.

Two confirming passes then found three more, the same defect class repeating.
`~/.claude/jobs/<id>` is the background-session supervisor's state, not job
output: a pinned session is kept alive while idle, so an idle stretch past the
window would delete a directory a live process owns — and `pins.json` sits right
there recording it, which is the tell that a closed category has gone one
directory too far. Dropped. `Agents::apply` still carried the `break` that the
same commit removed from `Caches::apply`, so one resumed session abandoned every
later finding, contradicting the "falls out of the plan" promise in three
documents. And `Caches/deno` is `DENO_DIR`, and `location_data/<hash>/kv.sqlite3` holds
every default-path `Deno.openKv()` database with `local_storage` beside it.
Dropped. A fifth pass then retired `~/.claude/projects` outright: narrowing the
unit to session-shaped entries protected `memory/`, but Claude Code retains a
Desktop- or Cowork-originated transcript at any age and nothing in the filename
says which — so age was never evidence there, and telling them apart would mean
reading transcript contents, the same shape that disqualified `jobs`. It
returned 0.00 GB here, so the trade was easy. Its `require_session_shape`
machinery went with it.

The pattern across all of them — JetBrains, deno, the project
directories, and the job supervisor state — is that a directory named like a cache is not thereby a cache, and
only reading what a tool actually stores inside it settles the question.

That session reclaimed 36 GiB on the development machine (11 GiB free to 47),
with `active_days` lowered to 10. The largest remaining item is the OrbStack VM
disk image at 45.1 GB, which devtrim reports and deliberately never touches.

## Previous state (0.8.2)

Scanning is concurrent: ten categories on scoped threads joined in registry
order, proven equivalent rather than assumed. Over a fixed 25-repository corpus
the parallel binary and the released 0.8.1 serial binary produce the same
SHA-256, and twelve consecutive parallel runs produce that one digest.
Exactly-once probing survives — still one `git log` per repository per scan —
because each repository's observation is an `Arc<OnceLock<..>>` cloned out from
under the mutex, so no lock is ever held across a subprocess.

The suite lost roughly 250 lines of duplicated and vacuous tests and now runs in
about half the wall time. Two independent review axes then found what no gate
could: the home root had lost its only protection assertion when a test was
deleted, and the pathless forged-command payload disappeared when tests were
collapsed into loops. Both are restored with positive controls that fail when
the production branch is deliberately broken.

The deletion sink now has an adversarial test. A mutator thread swaps the target
with a symbolic link to a bystander while the sink runs; the bystander survives
every interleaving across sixty loaded runs. Its assertions took three attempts:
both "no quarantine leftover" and "the leftover is a directory" are false under
a hostile mutator, because declining to move an entry back over an occupied name
is the safe outcome, and the leftover may be the mutator's own symlink.

## 0.8.1 production release

The user authorized production deployment after source delivery. Immutable
`v0.8.1-beta1` and `v0.8.1` both point to
`6fa2c8d04b55b374d286ba40db4eba7cdea08826`; hosted runs `33939147875` and
`33940354222` passed every release gate. Production reused the beta archive
without rebuilding. Both release and artifact attestations verified; the ZIP
SHA-256 is `5cc59b980bd034aa9e3bc602e558078a7f94a7887a6265c92a45fef952a5cc2d`.
The downloaded binary passed isolated TUI and read-only-view checks. Homebrew
tap commit `70a8b1a` publishes 0.8.1; audit, upgrade, formula tests, and the sole
visible `/opt/homebrew/bin/devtrim` version check passed. The landing page now
targets 0.8.1 and the changelog opens 0.8.2 Unreleased.

## 0.8.1 audit completed

The requested audit plan, decisions, sources and evidence are in
`agent_planning/archive/devtrim-audit.md`. The user approved integrating the existing
Rust/video edits and sending the diff/context to OpenAI for GPT-6 Astra
autoreview. Source version and both Cargo locks are 0.8.1; no release or tag
is part of this source-delivery task.

Hugging Face cleanup now targets only `hub`, preserving authentication state.
One preview shares process/Git observations, including failures; apply probes
again. Installer eligibility uses one metadata read, and read-only dashboards
avoid idle redraws and offscreen formatting. Redundant wrappers, duplicate
parsing/geometry and unused video scaffolding were removed or consolidated.

Rust 1.98.1 and MSRV 1.88.0 each pass 212 unit tests and 56 CLI tests; the one
ignored unit is a helper process covered by its parent test. The full offline
helper, arm64 release build, all five 60-second fuzz targets, fresh dependency
audits, secret scanners, independent Standards/Spec review, desktop/mobile
manual QA and video gates pass. The new terminal driver exposed and fixed a
partial-frame synchronization race without relaxing the quit deadline.

Baseline/candidate JSON is identical on the 32,180-entry scan corpus and
5,000-installer corpus. Host load above 260 on 18 CPUs made latency comparisons
unreliable; no speedup percentage is claimed. The benchmark harness refuses
such timing. P3 autoreview identified one missed post-run load guard; its
red/green regression passes and the confirming full source review is clean.
The generated video is independently verified because autoreview cannot
inspect binary diffs. All accepted findings and source-delivery gates are closed.

## Project Environment

Rust synchronous CLI; no React Native, mobile build, Metro, or async runtime.
Ratatui/Crossterm terminal UI; static HTML manual/landing page; a separate npm
Remotion video project. `scripts/verify.sh` selects the installed pinned compiler.
QA uses disposable HOME/PATH PTYs and CLI fixtures; performance uses isolated
corpora and preserved release binaries. Authorized worktrees need separate
Cargo targets and evidence paths. Commands and boundaries live in AGENTS.md.

## 0.8.0 feature baseline

0.8.0 adds the three parity commands deferred from 0.7.0, two of them narrower
than asked. `uninstall` resolves an app's bundle identifier and lists what macOS
keys to it, but cannot delete: `is_protected` refuses `/Applications` and all of
`~/Library` outside a four-entry allowlist, and widening that would weaken every
command. `optimize` is three fixed-argv tasks with `--apply` requiring an
explicit `--task`. `status --watch` samples on a worker thread.

Ten review findings, all real, mostly the same failure: a claim outrunning the
implementation. `uninstall` promised "every file belonging to an app" while
matching only identifier-named paths; group-container matching was unsound in
both directions and was removed; `optimize --apply` still bundled every task
behind one prompt; `status --watch` joined a worker that could not be
interrupted; the battery row called a failed probe "none".

## Previous state (0.7.0)

0.7.0 adds three surfaces and closes one measurement defect. `analyze` is an
interactive read-only disk explorer that measures on a worker thread and streams
results, so a directory taking minutes never freezes the interface. `status`
reports machine vitals with a health score that names the inputs it could not
read. `clean installers` reclaims stale `dmg`/`pkg`/`mpkg`/`iso`/`xip` archives
from `Downloads` and `Desktop`. Terminal styling moved to semantic tokens with a
`NO_COLOR` baseline and a `?` key reference.

Four independent reviews ran before release — security, standards, spec, and a
P3 autoreview — and every one found something no gate did. The three that
mattered most: `clean docker --apply` was fully broken because the new
report-only VM-image finding was pushed first and the apply loop refused
anything without a command authority; `status` measured the SEALED root volume,
reporting a 94%-full machine as 17% used; and the monochrome danger ladder was
defeated at the render site by `theme.bold()` while the theme's own test kept
passing because it exercises `style()`. Two of my own tests were passing
vacuously (a help-overlay assertion satisfied by the footer, a colour positive
control reading the environment).

## Previous state

devtrim 0.6.3 is the immutable production release. Production reused the exact
verified beta artifact, and the Homebrew tap plus the sole visible local
installation report 0.6.3. That release closed eight evidence-backed gaps:
terminal-safe complete command actions, TUI protected-Trash filtering before
approval, fail-closed present Swift aliases, capability-scoped global flags
(including command-only `--shred` rejection), implicit-TUI JSON rejection, exact
release-version declarations, and category-specific apply authorization for
Xcode/toolchain direct children.

The current Unreleased tree extends category authorization to `node_modules`,
denies Git metadata names ASCII-case-insensitively from scan through
open-handle preflight, and aligns artifact scan/apply around the same
case-insensitive dependency-namespace boundary. Direct `.git` case variants in
Trash are warned about and left without blocking other exact items. Ordinary CI
now runs full-history Gitleaks and TruffleHog gates; Gitleaks must trip a
non-allowlisted runtime positive control before its directory reaches `PATH` or
either CI path trusts a clean result. The source landing page now targets the
immutable v0.6.3 release, but the public GitHub Pages deployment still serves
v0.6.2; no push or deployment was authorized for this worktree.

On a controlled `node_modules` corpus, devtrim and Mole 1.52.0 both found the
same 20 stale trees and excluded all 5 recent controls. Under high machine load,
15 alternating samples averaged 0.481 s for devtrim and 5.800 s for Mole; this
is a narrow scanner-path comparison, not a whole-product performance claim.

## Decisions

- `uninstall` is a CONSERVATIVE REPORT, not an inventory, and must keep saying
  so. Identifier matching cannot see an app that stores data under a product
  name (VS Code's `~/Library/Application Support/Code`), and group containers
  are omitted entirely because their names come from an arbitrary entitlement —
  a suffix rule both misses real ones and misattributes others.
- A maintenance task that cannot do what its name says is omitted, not shipped
  with a warning. DNS is out because `dscacheutil -flushcache` does not clear
  the `mDNSResponder` resolver cache it would advertise.
- `optimize --apply` requires an explicit `--task`: `plan_danger` takes the
  maximum, so an unselected default lets a trivial task ride in on an expensive
  one's prompt.
- A version bump must regenerate `fuzz/Cargo.lock`, or the hosted fuzz job fails
  its clean-checkout step after running all five targets.
- `analyze` never creates deletion authority. Deletion here is always bound to a
  closed, corroborated category; an explorer that deleted the highlighted path
  would swap structural evidence for the operator's aim. Mole's analyze deletes;
  this one does not, and the README says so.
- Every remaining gap is stated in the README with its reason. An undisclosed
  gap is worse than a declared one, and that section is now what keeps
  `uninstall`'s narrowness and `optimize`'s three-task catalog honest rather
  than looking like oversights.
- `status` measures `/System/Volumes/Data`, not `/`. Only `statfs` separates the
  two: `st_dev` is identical across `/`, `/System/Volumes/Data` and `/Users`
  because of the APFS firmlink, so no metadata comparison can find that
  boundary. Memory used is `active + wired + compressed`; counting reclaimable
  inactive pages reports a healthy machine at 96%.
- A device comparison stops foreign mounts (proved on `/Volumes/Recovery`) but
  cannot stop the system/data firmlink — and should not, since that is the
  user's own data.
- Command output whose column count varies must be indexed from the END
  (`netstat -ib` link rows are 10 or 11 fields wide), and fields must be matched
  by exact key, never substring (`usec = ` ends with `sec = `).
- An aggregate is exact or refused; a top-N display list may skip a row.
- Physical removal accepts only a private `VerifiedTarget`; serialized paths
  are presentation-only.
- Git metadata matching is ASCII-case-insensitive at scanner, ownership,
  category, target-validation, and open-handle boundaries; an ordinary
  directory named `git` remains eligible.
- `node_modules` apply independently reasserts a real leaf inside its physical
  owner and rejects raw non-normal spellings, symlinked category ancestors,
  ASCII-case-insensitive Git metadata and outer dependency namespaces before
  the shared sink.
- Artifact scan and apply independently reject ASCII-case variants of the
  `node_modules` namespace; the scanner alone is never deletion authority.
- Direct Git-metadata-named Trash children remain in place with a warning and
  do not block other exact previewed items.
- Owner-reported npm and Homebrew caches are authorized only inside exact
  program namespaces and revalidated at apply.
- The structural deletion rule has positive-control tests and runs in
  pre-commit, CI, and release validation.
- MSRV is a mandatory executed gate; absence of its toolchain is a failure.
- An invariant a machine can check becomes a gate, not prose. `CODING_STANDARDS.md`
  carries only what clippy and ast-grep cannot see, as citable `S<n>` rules, and
  states each gate's blind spot so review knows where its work actually is.
- Pathname TOCTOU remains documented rather than overstated as solved.
- A production release may consume only an immutable, attested beta artifact
  from the same dereferenced commit; production never rebuilds it.
- The production release script is the sole automatic Homebrew entrypoint. Its
  idempotent closeout re-verifies the immutable artifact, updates only the tap
  formula with a normal push, locks local validation to that commit, and proves
  the existing sole `/opt/homebrew/bin/devtrim` installation. Beta skips it.
- Actionable size measurement, Docker size parsing, and config schema parsing
  fail closed; partial or ambiguous inputs never become cleanup authority.
- A failed final-tag workflow never moves the tag. Recovery may publish the
  already verified beta bytes manually, then fixes the workflow on `main`.
- Every human apply states the data-loss risk. `-y` skips normal y/N only;
  `--yolo` skips interactive prompts, but operation-specific acknowledgments
  such as `trash-empty --confirm=<gb>` remain mandatory.
- Aggregated sizes saturate instead of wrapping, and measurement errors fail
  closed before they can lower a danger score or authorize mutation.
- The TUI is a presentation adapter over existing `Op` owners. A matching typed
  approval is required at apply time, and CLI bypass flags are rejected.
- Global mutation flags are capability-scoped and rejected when the selected
  command cannot honor them; command-only Docker and simulator cleanup reject
  filesystem-only `--shred`, so flags never become silent no-ops.
- Xcode and Swift toolchain apply reassert the scanner's exact direct-child
  category shape before passing a target to the shared deletion sink.
- Terminal escaping happens at the final human rendering sink for the complete
  action or message; structured JSON retains the original value.
- Ratatui 0.30.2/Crossterm 0.29 require MSRV 1.88. Default Ratatui features,
  including its optional layout cache, stay off; the graph resolves patched
  `lru 0.18.2` instead of affected `0.12.5`.
- Fixed protected path components are compared ASCII case-insensitively at the
  shared validation boundary, while component-aware matching keeps similarly
  prefixed names such as `/systematic` outside the protected set.
- Release builds run without write or OIDC authority. A separate publisher job
  receives the packaged artifact and alone owns attestation and release writes;
  retrying all jobs replaces only the intermediate handoff, while retrying the
  publisher refreshes remote release state before deciding whether to create.
- Structural deletion enforcement uses separate rules for ordinary Rust source
  and the single owner module; positive controls prove a forged second sink is
  rejected both inside and outside that module.
- `Finding::command` alone issues the closed `CommandAuthority` capability;
  Docker and simulator apply require that capability, its validated endpoint or
  UDID, and its exact serialized action to agree before execution.
- Historical immutable tags and releases are provenance records and stay
  intact. The earlier request to delete or rewrite them was rejected.
- The production landing page remains on the current stable download during a
  beta, then advances only after exact-byte production promotion. Artifact
  validation does not treat `index.html` as a beta package version surface.
- The demo-video dependency graph is a release gate and receives weekly npm
  Dependabot coverage; shipped inline scripts use exact CSP hashes.
- Journal paths are opened component-by-component without following symlinks;
  apply writers serialize each synced record and keep rotation coordination
  across attempt/result, while `history` creates no state, waits for active
  guarded attempts, pairs legacy records across generations, and bounds input.
- Permanent recursive deletion performs a complete same-device/Git-marker
  preflight before the first removal, then repeats the checks while consuming
  the quarantined tree through directory handles.
- Hosted release credentials never share a job with repository or dependency
  execution. The local pre-tag phase is provenance-only; hosted read-only jobs
  produce the handoff consumed by the no-checkout publisher. After immutable
  publication, the local Homebrew closeout uses the authenticated tap boundary
  and scrubs token environment variables from install/test execution.

## Next boundary

0.5.0, 0.6.0, and 0.6.1 each passed independent security review plus structured
P3 autoreview, and the 0.6.2 local candidate passed the same review boundary,
with every verified release-scope finding fixed before publication; full
Rust/MSRV/npm/workflow/security gates, bounded fuzz runs, real PTY TUI passes,
exact-commit CI, staged-binary smoke tests, and exact-byte production promotion
also passed. Notarization
remains blocked on notarytool credentials (Developer ID cert exists locally;
runbook in SECURITY.md). Residual path-based windows (parent resolution,
Trash, single-entry unlink) are documented, not hidden.
