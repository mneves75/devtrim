# devtrim

Developer-machine disk hygiene for macOS: **measure, classify, trim — safely.**

Born from a cleanup session that reclaimed 250+ GB across model caches, stale
`node_modules`, simulator storage, Xcode support files, Docker bloat, and old
Swift toolchains.

**[Website](https://mneves75.github.io/devtrim/)** · **[Manual](https://mneves75.github.io/devtrim/MANUAL.html)** · **[Releases](https://github.com/mneves75/devtrim/releases)**

This source tree and its packaged documentation describe devtrim v0.6.2.

## Install

With Homebrew:

```bash
brew install mneves75/devtrim/devtrim
```

Or download the Apple silicon archive for the version you intend to run from
[GitHub Releases](https://github.com/mneves75/devtrim/releases), then verify it
with the included checksum:

```bash
shasum -a 256 -c SHA256SUMS.txt
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
- **Immutable plans.** Apply consumes only paths shown in the preview; it never rescans for new deletion targets.
- **Trash-first.** Filesystem deletions go to macOS Trash. `--shred` explicitly previews permanent deletion and raises danger to critical.
- **Fail closed.** Unknown Git activity, incomplete size measurement, broken toolchain links, unknown or malformed config fields, symlinked ancestors, failed owner commands, and failed liveness probes block mutation.
- **Liveness guards.** `node-modules` and `artifacts` refuse a repo that is the working directory of a running build or package process; `xcode` refuses DerivedData while `xcodebuild` runs. A probe that cannot complete blocks instead of passing.
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
devtrim clean caches --apply -y           # HF/uv/npm/brew/node download caches
devtrim clean node-modules --apply -y     # exact paths in conclusively stale Git repos
devtrim clean artifacts --apply -y        # corroborated build artifacts in stale Git repos
devtrim clean simulators --apply -y       # delete exact previewed unavailable devices
devtrim clean xcode --apply -y            # exact DeviceSupport/DerivedData children
devtrim clean docker --apply -y           # local daemon images + build cache; never volumes
devtrim clean toolchains --apply -y       # only unreferenced swift.org toolchains
devtrim clean leftovers                   # report-only hints; never deletes worktrees
devtrim icloud                            # large iCloud Drive files and local allocation
devtrim trash-empty --confirm=14          # preview permanent Trash purge
devtrim trash-empty --confirm=14 --apply  # perform the verified purge
devtrim history                           # recent journaled applies; --json for one document
devtrim largest --top 20                  # read-only: biggest directories under scan roots
devtrim completions zsh                   # shell completion script (bash | zsh | fish)
devtrim manpage                           # man page in roff format
```

`clean artifacts` deletes a directory only when its name is on a closed list
**and** its ecosystem corroborates it — `target` next to `Cargo.toml`, `.venv`
containing `pyvenv.cfg`, `Pods` next to `Podfile`, `.next` next to
`package.json`, a valid `CACHEDIR.TAG` signature, and so on — inside a Git repo
whose last commit is conclusively stale. Ambiguous names such as `build`,
`dist`, `vendor`, `bin`, and `obj` are deliberately never matched.

`trash-empty` previews each current top-level Trash item as an exact target.
Apply consumes only that set; anything moved to Trash after preview remains.

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
active_days = 30                  # newer commits make a repo active
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

Explicit `--root` flags replace config/default roots. Existing roots are resolved
before preview. An unreadable, malformed, or unknown config field is an error; devtrim never
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

### For agents

devtrim is built to be operated by automation and AI agents without ambiguity:
every invocation emits exactly one JSON document, actions are typed rather than
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
| Terminal output | control and bidirectional-control characters are escaped before rendering |
| Release authority | build code is read-only; a separate publisher receives packaged inputs and holds release/OIDC permissions |

Sizes are estimated logical bytes. APFS clones, sparse files, and container-VM
compaction can make immediately available disk space differ. An estimate may
differ from physical blocks, but devtrim refuses to invent one when traversal
or metadata is incomplete.

## Build and verify

```bash
cargo fmt --all -- --check
ast-grep test --skip-snapshot-tests
ast-grep scan --config sgconfig.yml
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
rustup run 1.88.0 cargo test --locked --all-targets --all-features
cargo audit
cargo audit --file fuzz/Cargo.lock
cargo build --release --locked --target aarch64-apple-darwin
(cd video && npm ci --strict-allow-scripts && npm audit --package-lock-only --audit-level=low && npm run lint && npm run format:check && npm run build)
bash scripts/tests/release-policy.sh
bash -n scripts/release.sh scripts/update-homebrew.sh scripts/tests/release-policy.sh scripts/tests/update-homebrew-formula.sh
shellcheck scripts/release.sh scripts/update-homebrew.sh scripts/tests/release-policy.sh scripts/tests/update-homebrew-formula.sh && actionlint
gitleaks git --redact --no-banner .
trufflehog git "file://$(pwd)" --results=verified,unknown --fail --fail-on-scan-errors --no-update --no-color
```

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
