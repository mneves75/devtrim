# Project Memory

## Current state (0.8.2)

Scanning is concurrent: nine categories on scoped threads joined in registry
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
