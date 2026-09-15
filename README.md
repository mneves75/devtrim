# devtrim

Developer-machine disk hygiene for macOS: **measure, classify, trim — safely.**

Born from a cleanup session that reclaimed 250+ GB across model caches, stale
`node_modules`, simulator storage, Xcode support files, Docker bloat, and old
Swift toolchains.

**[Website](https://mneves75.github.io/devtrim/)** · **[Manual](https://mneves75.github.io/devtrim/MANUAL.html)** · **[Releases](https://github.com/mneves75/devtrim/releases)**

This source tree and its packaged documentation describe devtrim v0.9.6.

## Install

With Homebrew:

```bash
brew install mneves75/devtrim/devtrim
```

Or download the Apple silicon archive for the version you intend to run from
[GitHub Releases](https://github.com/mneves75/devtrim/releases), then verify it.
The checksum comes from the same release as the archive, so it proves only an
intact download; the build attestation proves which workflow built it from
which commit:

```bash
shasum -a 256 -c SHA256SUMS.txt
gh attestation verify devtrim-<version>-macos-arm64.zip --repo mneves75/devtrim \
  --signer-workflow mneves75/devtrim/.github/workflows/release.yml \
  --deny-self-hosted-runners
```

Or build from source:

```bash
git clone https://github.com/mneves75/devtrim
cd devtrim
cargo build --release --locked
cp target/release/devtrim /usr/local/bin/
```

## Principles

- **Preview by default.** Every mutation, including `trash-empty`, requires `--apply`.
- **Immutable plans.** Apply consumes only paths shown in the preview; it never rescans for new deletion targets. Xcode and Swift toolchain apply reassert exact direct-child authority; `node_modules` apply reasserts a real authorized directory leaf and rejects symlinks plus `.git`, nested dependency-tree, and non-normal ancestors.
- **Trash-first.** Filesystem deletions go to macOS Trash. `--shred` explicitly previews permanent deletion and raises danger to critical.
- **Untrusted repositories stay inert.** The Git activity probe disables every repository-configurable path by which `git log` runs a program — hooks, fsmonitor, signature verification through `gpg.program`, and lazy fetches through a promisor remote's `uploadpack` — so previewing a directory that arrived with a hostile `.git/config` runs nothing. A `git` too old for `--no-lazy-fetch` refuses the repository.
- **Fail closed.** Unknown Git activity, incomplete size measurement, broken toolchain links, unknown or malformed config fields, symlinked ancestors, failed owner commands, and failed liveness probes block mutation.
- **Liveness guards.** `node-modules` and `artifacts` refuse a repo that is the working directory of a running build or package process; `xcode` refuses DerivedData while Xcode, `xcodebuild`, or the build services Xcode.app builds run through are running. A probe that cannot complete blocks instead of passing; a build tool that exited between the process list and the directory lookup is not mistaken for one.
- **Identity-verified deletion.** Every finding records its target's device/inode at preview (plus file generation on macOS); the sink re-checks that identity through an open parent-directory handle. Every directory action rejects foreign devices and Git repository/worktree markers at any depth before mutation. Permanent deletes additionally quarantine the verified leaf and drive recursion through open handles. A target swapped after preview is refused. Trash remains path-based because macOS has no fd-anchored Trash API; that residual window is documented, not denied.
- **Write-ahead journal.** Every apply records an attempt before deletion and a result after it in `~/.local/state/devtrim/journal.jsonl` (`$XDG_STATE_HOME` honored). Symlinked path components are refused, complete records are serialized and synced, and an unwritable journal blocks apply. Rotation (10 MiB, keep 3) cannot split an in-flight pair. `devtrim history` is read-only, waits for guarded applies before snapshotting, pairs legacy records across generations, reverse-scans only the bounded newest tail needed for the requested limit, and reports a genuinely unmatched attempt as interrupted.
- **Danger scores.** Actionable findings carry 1–10; aggregate size can raise the plan score:
  - 1–8: y/N prompt (`-y` skips it); non-TTY apply needs `-y`/`--yolo`
  - ≥9: typed numeric confirmation (`--yolo` skips confirmation only)
- **Typed deletion boundary.** Exact `PathBuf` targets must become an internal `VerifiedTarget` immediately before the single deletion sink can consume them. Display strings are never deletion authority.
- **Typed command boundary.** A displayed command action is not enough to execute; a private closed capability binds the exact operation and its validated arguments. Docker cleanup accepts only the previewed absolute local Unix-socket endpoint; simulator cleanup accepts only the previewed device UDID and rechecks that it is still unavailable.
- **Protected physical paths.** System roots, user secrets, the home root, Trash root, paths reached through symlinked ancestors, and owner-reported cache paths outside npm/Homebrew namespaces are refused.
- **Volumes are sacred.** Docker volumes are never pruned.
- **Archives are sacred.** Xcode Archives are visible but never actionable.
- **Capability-scoped flags.** Mutation flags are rejected when a command cannot use them; they never become silent no-ops. `scan --shred` remains meaningful because it changes the previewed action, while report-only commands reject it.
- **Color is never the only signal.** The terminal interface styles through semantic tokens and honors `NO_COLOR`, degrading each token to a modifier that preserves the same distinction; the danger ladder stays ordered even with color stripped. Colors are named ANSI, so they resolve through your own terminal theme instead of a fixed palette that matches nowhere.
- **Agent-friendly.** Every `--json` invocation emits exactly one JSON document and failures return nonzero.

## Data-loss risk, warranty, and macOS permissions

devtrim is free, open-source software provided **AS IS**, without warranties or
conditions of any kind; the [Apache-2.0 license](LICENSE) is authoritative.
Cleanup can delete files. Safety checks reduce risk but cannot replace a current
backup or your review of the preview. By applying a plan — including with `-y`
or `--yolo` — you accept the risk of data loss for the exact targets shown.

Preview first, prefer Trash, and use `--shred` or `trash-empty` only when you
intend permanent removal. macOS may deny access or ask you to authorize Files &
Folders, App Management, Automation, or Full Disk Access in System Settings;
grant access manually only when you understand the request. devtrim never
bypasses macOS protections. Apple documents these controls under
[Privacy & Security](https://support.apple.com/guide/mac-help/mchl211c911f/mac).

## Usage

```bash
devtrim                                   # interactive TUI when stdin/stdout are terminals
devtrim tui                               # explicit TUI launch
devtrim scan                              # full read-only report
devtrim scan --json                       # one machine-readable envelope
devtrim clean caches --apply -y           # tool download caches (HF, uv, npm, brew, cargo, bun, gh, …)
devtrim clean node-modules --apply -y     # exact paths in Git repos with no recent activity
devtrim clean artifacts --apply -y        # corroborated build artifacts in stale Git repos
devtrim clean simulators --apply -y       # delete exact previewed unavailable devices; working ones are only reported
devtrim clean xcode --apply -y            # exact DeviceSupport/DerivedData child directories
devtrim clean docker --apply -y           # local daemon images + build cache; never volumes
devtrim clean toolchains --apply -y       # only unreferenced swift.org toolchains
devtrim clean installers --apply -y       # stale installer archives in Downloads/Desktop
devtrim clean agents --apply -y           # agent caches + session history past the active window
devtrim clean leftovers                   # report-only hints; never deletes worktrees
devtrim icloud                            # large iCloud Drive files and local allocation
devtrim trash-empty --confirm=14          # preview permanent Trash purge
devtrim trash-empty --confirm=14 --apply  # perform the verified purge
devtrim history                           # recent journaled applies; --json for one document
devtrim analyze                           # interactive read-only disk explorer (never deletes)
devtrim analyze ~/Library --json          # one-shot breakdown of a directory
devtrim status                            # read-only machine vitals and a health score
devtrim status --watch                    # live dashboard; q quits
devtrim uninstall AltTab                  # paths named for an app's bundle id (report-only)
devtrim optimize                          # preview macOS maintenance tasks
devtrim optimize --task quicklook --apply # run one selected task
devtrim largest --top 20                  # read-only: biggest directories under scan roots
devtrim completions zsh                   # shell completion script (bash | zsh | fish)
devtrim manpage                           # man page in roff format
```

Global flags are capability-scoped. Read-only/report-only commands reject
`--apply`, `-y`, `--yolo`, and `--shred` instead of pretending to honor
them. `scan --shred` is the exception because it explicitly previews
permanent actions. Docker and simulator cleanup reject `--shred` because they
execute exact typed commands rather than filesystem deletion actions.
`trash-empty` accepts apply/confirmation flags but rejects `--shred`: its
preview is already permanent and its exact `--confirm=<gb>` acknowledgment
remains mandatory.

`clean artifacts` deletes a directory only when its name is on a closed list
**and** its ecosystem corroborates it — `target` next to `Cargo.toml`, `.venv`
containing `pyvenv.cfg`, `Pods` next to `Podfile`, `.next` next to
`package.json`, a valid `CACHEDIR.TAG` signature, and so on — inside a Git repo
whose last activity is conclusively stale. Ambiguous names such as `build`,
`dist`, `vendor`, `bin`, and `obj` are deliberately never matched, and the
scanner/apply owner refuse artifacts below every ASCII-case variant of
`node_modules`.

`clean installers` considers only direct children of `Downloads` and `Desktop`
whose extension is on a closed list (`dmg`, `pkg`, `mpkg`, `iso`, `xip`) and
which have been untouched for longer than the configured active window. Age is
the file's modification time, so a copy that preserves it — a Finder copy from
another disk, for example — counts as old immediately; the preview shows the
age it judged.
Scanning is deliberately non-recursive, because those directories routinely hold
extracted project trees whose bundled installers are not loose clutter. Formats
that can carry source or user data, such as `zip` and `tar`, are never matched.
Apply re-checks the whole shape and refuses symlinks and any target outside
those two directories.

`clean agents` covers coding-agent storage in two tiers, because it is not one
kind of data. *Regenerable caches* — the Claude Code metadata cache,
the Codex catalog cache, the Pi web-search cache, the OpenCode cache — are exact
paths their owner rebuilds on demand, so they are offered unconditionally at a
low danger score. *Session history* — Claude Code shell snapshots; Codex shell snapshots,
session and archived-session trees — is **not regenerable**, so a
child is offered only once the newest
regular file anywhere in its subtree is older than the configured active window,
its note says the content does not come back, and its danger score reflects
that. Codex nests sessions as `<year>/<month>/<day>`, so the day directory is
the unit; waiting for a whole year to go stale would never offer the current
one. Authentication material (`auth.json`, `.credentials.json`), configuration,
memories, skills, agent definitions, installed plugins and the `.claude.json`
backup copies are on neither list and are never candidates. Apply reasserts the
full shape — tier membership, exact depth below the configured root, no
symlink, and the age gate re-read from disk — so a session resumed after preview
falls out of the plan. Agent *scratch worktrees* are a different question and
stay where they were: `clean leftovers` lists them for review and never deletes
them, because a worktree's staleness cannot be proven from its name.

Two of those rules exist because the obvious design was wrong. A shell snapshot
is *not* a cache: Claude Code writes one per session and sources that exact file
on every later shell call, and nothing rewrites it if it disappears — so it sits
in the age-gated tier. Age is the right signal there because Claude Code sweeps
the same directory itself once an entry passes its own retention period.

And `~/.claude/projects` is not a root at all. It holds Claude Code auto memory
at `<project>/memory/`, keyed by repository root while the transcripts beside it
are keyed by working directory — and since v2.1.248 Claude Code retains a
transcript originating in Claude Desktop or Cowork at any age, with nothing in
the filename to distinguish one. File age is therefore not evidence that
anything under that root is finished with, and identifying the exception would
mean reading transcript contents. It returned 0.00 GB on the machine this was
built against, so the honest trade was to leave it alone.

`clean caches` also reaches a closed list of exact `~/Library/Caches`
subdirectories — Playwright browsers, the VS Code HTTP cache and its Squirrel
update staging, SwiftPM, pip, pnpm, Go and TypeScript. `~/Library` stays protected
wholesale; that list is the carve-out, it is the same constant the protection
boundary reads, and a name is only on it when one developer tool owns the
directory and rebuilds it on demand.

Two boundaries inside that are worth stating, because a shorter list would
over-claim. The pnpm entry is the metadata cache, never the content-addressable
store at `~/Library/pnpm/store` that installed `node_modules` trees hard-link
into. And editor *logs* and application data are not covered at all: VS Code
keeps its logs, `CachedData` and webview caches under `~/Library/Application
Support/Code`, and JetBrains keeps logs under `~/Library/Logs` — both outside
`~/Library/Caches`, both mixed in with real user state, and neither reachable
without a second carve-out this release does not make. `~/Library/Caches/JetBrains`
is excluded for a stronger reason: on macOS that is the IDE *system directory*,
and each product subdirectory holds `LocalHistory`, the per-file change history
the IDE keeps for files Git never saw. Nothing regenerates it.

`analyze` is navigation, never deletion, and that boundary is deliberate. Every
cleanup category binds deletion to structural corroboration — a `target` beside
`Cargo.toml`, a `.venv` containing `pyvenv.cfg` — and an explorer that deleted
whatever the cursor was on would swap that evidence for the operator's aim. It
measures on a worker thread and streams results in, so a directory that takes
minutes to size never freezes the screen; leaving a directory cancels its walk.
Symlinks are reported at their own size rather than followed, a different device
is never measured or entered, and anything unreadable is disclosed as a
`(partial)` lower bound. A lower bound is a partial result, so `analyze --json`
lists each one in `errors` and exits nonzero, as quitting the explorer does.

`status` reads machine vitals through fixed-argv system tools and reports a
health score that **names every input it could not read** rather than scoring
over the gap. Memory used is stated as `active + wired + compressed`: counting
macOS's reclaimable inactive pages as used reports a healthy machine at 96%.

## Where devtrim stops, and why

Other Mac cleaners delete more. These boundaries are choices with reasons:

- **`uninstall` reports; it does not remove.** It resolves an app's bundle
  identifier and lists the paths macOS keys by it — the hard part — but it is a
  conservative report, not an inventory: an app storing data under a product
  name is invisible to it, as Visual Studio Code's `~/Library/Application
  Support/Code` is. It does not delete because `is_protected` refuses
  `/Applications` and everything under `~/Library` outside a closed allowlist
  of developer-managed paths, and widening that would weaken every command,
  not just this one.
- **`optimize` is three tasks, not twenty-two.** Rebuilding a Spotlight index,
  running the periodic scripts and purging memory all need root and cost more
  than they return. A DNS flush is absent too: on modern macOS the resolver
  cache lives in `mDNSResponder`, so `dscacheutil -flushcache` would report
  success without clearing what it names. `--apply` requires an explicit
  `--task`, because one confirmation must not cover unrelated work.
- **`analyze` never deletes.** Navigation and deletion authority stay separate:
  every cleanup category binds deletion to structural corroboration, and an
  explorer that removed the highlighted path would replace that with your aim.
- **No Dock or login-item surgery, and no "speed up your Mac" claims.** devtrim
  measures and trims; it does not tune.

`clean docker` also reports the host-side VM disk image for OrbStack and Docker
Desktop, as a report-only finding that is never actionable. `docker system df`
measures space *inside* the guest, while the host pays for a sparse image that a
prune shrinks only once the runtime returns the freed blocks — OrbStack documents
that as automatic and Docker Desktop as taking seconds, but neither is
guaranteed, so scan again to measure what came back. The build-cache estimate is
"up to" the full cache size when no record is in use, because `builder prune -a`
also removes cache shared with images that Docker does not count as reclaimable,
and otherwise "at least" the reclaimable figure. The VM image finding is measured in allocated blocks rather
than logical length, and it is shown when `docker` is not installed at all.
When `docker` is installed but its daemon is down, the category fails — a
silently shorter plan would read as nothing to reclaim — and its error names
the image and its size, which is the one state where the cost is invisible to
`docker` and still present.

`trash-empty` previews each current top-level Trash item as an exact target.
`--apply` shows that set again and purges it only after the same confirmation
every mutation requires — typed on a terminal, or `--yolo` — so an item moved
to Trash after the earlier preview is shown before it can be approved. The
`--confirm=<gb>` acknowledgment is measured over that exact set, not the whole
Trash, so an excluded item never makes it unsatisfiable. Anything moved to
Trash after confirmation remains.
A direct item named as an ASCII-case variant of `.git` is warned about and left
in Trash instead of blocking the other exact items.

The TUI offers the same scanners and apply owners behind a keyboard interface:
arrow keys or `j`/`k` navigate, `Enter` previews, `a` starts confirmation, `s`
switches an already-previewed Trash action to permanent mode, and `Esc` cancels.
Results and outcomes scroll with arrows or `j`/`k`, including retained scanner
warnings and partial-apply errors. Risk labels are written as text as well as
color. Below 64×18, the interface blocks operation input and asks you to resize;
only quit remains available. The interface requires an
interactive stdin and stdout; bare `devtrim` prints help and exits nonzero when
piped, while automation continues to use explicit subcommands and `--json`.

TUI confirmation is deliberately separate from CLI bypass flags. `devtrim tui`
rejects `--apply`, `-y`, `--yolo`, `--shred`, and `--json`; ordinary actions
require `y`, critical plans require their displayed numeric size, and Trash
purge requires the exact phrase `PURGE <gb>`. The warning is shown after the
exact preview and before authorization.

`-y` acknowledges the data-loss warning and bypasses normal y/N prompts;
critical plans still require typed confirmation. `--yolo` acknowledges the
risk and skips interactive prompts, but it never bypasses operation-specific
acknowledgments such as `trash-empty --confirm=<gb>` or adds an operation that
was absent from the preview.

## Config — `~/.config/devtrim.toml`

```toml
roots = ["~/dev"]                 # scan roots
active_days = 30                  # newer Git activity makes a repo active (0 means 1)
protect = ["~/dev/keep"]          # never delete these paths or their children
```

`protect` entries expand `~`, must be absolute, and are enforced deny-only at
the single deletion sink — a protected target is refused even if a scanner
offers it, and previews filter it out with a diagnostic. Relative or malformed
entries are an error, never silently ignored; an entry that does not resolve to
an existing path warns loudly. Matching is Unicode-normalization-insensitive
(NFC config text protects an NFD on-disk name) and ASCII-case-insensitive,
symlinked entries also protect their resolved location, and deleting an
ancestor of a protected entry is refused too.

A repository is active when its HEAD commit or its newest HEAD reflog entry is
inside the window. The reflog is what a clone, checkout, or pull writes, so an
old project cloned today — whose dependencies were just installed — is not
offered; a repository with reflogs disabled is judged by its commit date.

Explicit `--root` flags replace config/default roots. Existing roots are resolved
before preview; an explicit root that does not exist is warned about instead of
silently scanning nothing. An unreadable, malformed, or unknown config field is an error; devtrim never
silently falls back to another root.

## JSON contract

JSON mode returns one envelope, including empty and failed results:

```json
{
  "operation": "caches",
  "applied": false,
  "findings": [],
  "errors": []
}
```

Applied commands additionally include `summary`. If a later target fails, the summary retains earlier successful work, `errors` explains the stop, and the process exits nonzero. Each action is typed (`trash`, `shred`, `command`, `info`, or `none`) rather than encoded as a shell string.

`devtrim history --json` emits its own single document —
`{"operation":"history","entries":[…],"errors":[…]}` — where each entry is a
journal record with numeric `ts`, `phase`, `op`, `action`, and either `target`
or the exact `argv`. `completions` and `manpage` have no JSON form and return
the standard error envelope when `--json` is passed.

`devtrim status --json` is also its own single document: a vitals report with
`uptime_seconds`, `load_average`, `cpu_count`, `memory`, `disk`, `battery`,
`thermal`, `network`, `top_processes`, `health` (`score` and the
`missing_inputs` it was computed without), and `unavailable` — one reason per
metric that could not be read. A metric is `null` when unread, never zero, and
the process exits nonzero whenever `unavailable` is not empty. `status --watch`
is interactive and rejects `--json`.

### For agents

devtrim is built to be operated by automation and AI agents without ambiguity:
every `--json` invocation emits exactly one JSON document, actions are typed rather than
parsed from display strings, partial failure exits nonzero with earlier work
reported, mutation always requires explicit `--apply` plus explicit consent
flags, and every apply leaves a write-ahead journal an agent can audit with
`devtrim history --json`. Nothing devtrim does depends on parsing human-facing
output.

## Safety model

| Layer | Rule |
|---|---|
| Preview | `--apply` is mandatory for every mutation |
| Candidate set | apply uses exact previewed findings |
| Category authority | Xcode and toolchain apply reassert exact direct-child targets; `node_modules` apply reasserts its scanner's leaf and ancestor rules |
| Trash | recoverable by default; permanent mode is explicit |
| Danger gate | maximum finding score plus aggregate estimated logical bytes |
| TUI consent | approval capability must match the current preview and danger requirement |
| Target identity | exact internal `PathBuf` plus preview-time device/inode; display text is never parsed back into authority |
| Anchored deletion | the sink verifies identity through an open parent-directory handle; permanent deletion continues through that handle and drift refuses |
| Deletion sink | only `VerifiedTarget` reaches physical removal; action selects Trash vs. permanent mode |
| Command execution | serialized action and private closed authority must match the operation and its validated arguments |
| Physical path | literal and resolved parent must agree; deny-only resolution |
| Directory preflight | foreign devices and nested Git repository/worktree markers are refused before Trash or permanent mutation |
| Activity | unknown Git/toolchain ownership is ineligible |
| Liveness | a repo owning a running build process, or DerivedData under a running `xcodebuild`, is refused; probe failure blocks |
| Protect config | user-listed `protect` paths are refused at the deletion sink and filtered from previews |
| Journal | a write-ahead attempt/result record precedes and follows every deletion; an unwritable journal blocks apply |
| Measurement | incomplete traversal, metadata, or numeric state blocks an actionable plan |
| Automation | one JSON document; partial/failed work returns nonzero |
| Terminal output | complete human-facing actions, findings, errors, and notes escape control and bidirectional-control characters before rendering |
| Release authority | build code is read-only; a separate publisher receives packaged inputs and holds release/OIDC permissions |

Sizes are estimated logical bytes. APFS clones, sparse files, and container-VM
compaction can make immediately available disk space differ. An estimate may
differ from physical blocks, but devtrim refuses to invent one when traversal
or metadata is incomplete.

## Build and verify

Use the pinned Rust 1.98.1 toolchain for builds; a standalone Cargo installed
before rustup on PATH can otherwise bypass `rust-toolchain.toml`. MSRV remains
1.88.0. Install the toolchains and gate tools once, then use the local helper:

```bash
bash scripts/verify.sh focused
bash scripts/verify.sh offline
```

`focused` runs Rust checks and the real terminal smoke test. `offline` adds
the local workflow, shell, MSRV, cached dependency, and Gitleaks checks. Both
report failed prerequisites and exit nonzero on failure. Neither installs
tools nor substitutes for fresh advisory scans, TruffleHog, fuzzing, video
checks, and review before delivery. Cached audit success does not prove that
the advisory database is current.

The PTY smoke test can also run against an already built binary:

```bash
python3 scripts/tests/tui.py target/debug/devtrim
python3 scripts/tests/read-only-views.py target/debug/devtrim
```

It uses an isolated HOME and PATH, explicit terminal dimensions, visible
content assertions, and bounded waits. Use `tests/cli.rs` fixtures for JSON,
probe failures, and deletion sentinels. Debug a failing scenario in its
disposable fixture; do not grant broader permissions or run cleanup in your
real home merely to make a test pass.

For authorized parallel worktrees, set a distinct `CARGO_TARGET_DIR`, evidence
directory, benchmark corpus, and local-server port for each checkout. Record
the starting commit and assign one writer to each changed file. A benchmark
compares the same successful workload and compiler on both binaries; keep
intentional security/output changes separate from optimization baselines.

Build a disposable scan corpus with `bash scripts/perf/corpus.sh <new-home>`.
Then run `bash scripts/perf/ab.sh <baseline> <candidate> <corpus-home>` with
Hyperfine installed. The harness refuses unsuccessful or unequal scan output,
keeps both execution orders, and records binary hashes and machine load.
Set `PERF_BASELINE_BUILD_INFO` and `PERF_CANDIDATE_BUILD_INFO` to the compiler,
target, and build profile used for each binary. `python3 scripts/perf/test.py`
checks that the harness accepts equivalent work and rejects invalid evidence.

The individual delivery gates remain available below:

```bash
rustup run 1.98.1 cargo fmt --all -- --check
ast-grep test --skip-snapshot-tests
ast-grep scan --config sgconfig.yml
rustup run 1.98.1 cargo clippy --locked --all-targets --all-features -- -D warnings
rustup run 1.98.1 cargo test --locked --all-targets --all-features
rustup run 1.88.0 cargo test --locked --all-targets --all-features
cargo audit
cargo audit --file fuzz/Cargo.lock
rustup run 1.98.1 cargo build --release --locked --target aarch64-apple-darwin
(cd video && npm ci --strict-allow-scripts && npm audit --package-lock-only --audit-level=low && npm run lint && npm run format:check && npm run build)
bash scripts/tests/release-policy.sh
(for script in scripts/verify.sh scripts/release.sh scripts/update-homebrew.sh scripts/tests/release-policy.sh scripts/tests/update-homebrew-formula.sh scripts/perf/ab.sh scripts/perf/corpus.sh; do
  bash -n "$script" || exit "$?"
done)
(for script in .githooks/pre-commit scripts/tests/shellcheck-tracked.sh scripts/tests/gitleaks-positive-control.sh; do
  sh -n "$script" || exit "$?"
done)
scripts/tests/shellcheck-tracked.sh
actionlint
cmp AGENTS.md CLAUDE.md
scripts/tests/gitleaks-positive-control.sh "$(command -v gitleaks)"
gitleaks git --redact --no-banner .
trufflehog git "file://$(pwd)" --results=verified,unknown --fail --fail-on-scan-errors --no-update --no-color
```

Run each target under `fuzz/fuzz_targets/` for 60 seconds with the configured
nightly compiler before release; the exact PATH setup is in `AGENTS.md`.
Keep output and exit status for each gate. A push request authorizes source
delivery; publishing a release and promoting it to production are separate
actions.

See [`SECURITY.md`](SECURITY.md) for the threat model and reporting process.
Release notes live in [`CHANGELOG.md`](CHANGELOG.md). After committing and
pushing a version bump, enable GitHub immutable releases and run
`scripts/release.sh <version>-beta<N>` for staging. Each retry uses a new
counter. Run the local gates and P3 autoreview before committing. Before
tagging, the script performs only clean-tree, current-default-branch,
exact-CI/autoreview, and immutable-release provenance checks; it does not run
project or dependency code in that privileged preflight. Hosted
read-only jobs rerun the deterministic, fuzz, dependency, UI, video, and secret
gates and build the arm64 archive. A no-checkout publisher alone receives
release-write and OIDC authority, signs provenance, publishes the immutable
prerelease, and verifies the downloaded asset. Production uses
`scripts/release.sh <version>`; the workflow promotes the exact highest
verified beta artifact from the same commit without rebuilding it.
After the production release and attestation verify, the same script invokes
the idempotent `scripts/update-homebrew.sh <version>` closeout. It validates the
exact archive and checksum again, updates only `Formula/devtrim.rb` in
`mneves75/homebrew-devtrim` with a normal push, audits the updated tap, upgrades
the existing local formula, runs its test, and requires the only visible binary
to be `/opt/homebrew/bin/devtrim` at the released version. Beta releases never
touch Homebrew. If this post-release step fails, rerun the helper directly; the
immutable GitHub release and tag are not moved or reused.
