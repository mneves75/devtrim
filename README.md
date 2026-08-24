# devtrim

Developer-machine disk hygiene for macOS: **measure, classify, trim — safely.**

Born from a cleanup session that reclaimed 250+ GB across model caches, stale
`node_modules`, simulator storage, Xcode support files, Docker bloat, and old
Swift toolchains.

**[Website](https://mneves75.github.io/devtrim/)** · **[Manual](https://mneves75.github.io/devtrim/MANUAL.html)** · **[Download v0.3.2](https://github.com/mneves75/devtrim/releases/tag/v0.3.2)**

`master` is the unreleased 0.4.0 development line. The latest published binary
remains v0.3.2 until the 0.4.0 release is staged and promoted.

## Install

Download the Apple silicon archive from the [v0.3.2 release](https://github.com/mneves75/devtrim/releases/tag/v0.3.2), then verify it with the included checksum:

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
- **Fail closed.** Unknown Git activity, incomplete size measurement, broken toolchain links, unknown or malformed config fields, symlinked ancestors, and failed owner commands block mutation.
- **Danger scores.** Actionable findings carry 1–10; aggregate size can raise the plan score:
  - 1–8: y/N prompt (`-y` skips it); non-TTY apply needs `-y`/`--yolo`
  - ≥9: typed numeric confirmation (`--yolo` skips confirmation only)
- **Typed deletion boundary.** Exact `PathBuf` targets must become an internal `VerifiedTarget` immediately before the single deletion sink can consume them. Display strings are never deletion authority.
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
Folders or Full Disk Access in System Settings; grant access manually only when
you understand the request. devtrim never bypasses macOS protections. Apple
documents these controls under [Privacy & Security](https://support.apple.com/guide/mac-help/mchl211c911f/mac).

## Usage

```bash
devtrim                                   # interactive TUI when stdin/stdout are terminals
devtrim tui                               # explicit TUI launch
devtrim scan                              # full read-only report
devtrim scan --json                       # one machine-readable envelope
devtrim clean caches --apply -y           # HF/uv/npm/brew/node download caches
devtrim clean node-modules --apply -y     # exact paths in conclusively stale Git repos
devtrim clean simulators --apply -y       # delete unavailable devices only
devtrim clean xcode --apply -y            # exact DeviceSupport/DerivedData children
devtrim clean docker --apply -y           # unused images + build cache; never volumes
devtrim clean toolchains --apply -y       # only unreferenced swift.org toolchains
devtrim clean leftovers                   # report-only hints; never deletes worktrees
devtrim icloud                            # large queued iCloud uploads
devtrim trash-empty --confirm=14          # preview permanent Trash purge
devtrim trash-empty --confirm=14 --apply  # perform the verified purge
```

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
roots = ["~/dev"]        # scan roots
active_days = 30         # newer commits make a repo active
```

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

## Safety model

| Layer | Rule |
|---|---|
| Preview | `--apply` is mandatory for every mutation |
| Candidate set | apply uses exact previewed findings |
| Trash | recoverable by default; permanent mode is explicit |
| Danger gate | maximum finding score plus aggregate estimated logical bytes |
| TUI consent | approval capability must match the current preview and danger requirement |
| Target identity | exact internal `PathBuf`; display text is never parsed back into authority |
| Deletion sink | only `VerifiedTarget` reaches physical removal; action selects Trash vs. permanent mode |
| Physical path | literal and resolved parent must agree; deny-only resolution |
| Activity | unknown Git/toolchain ownership is ineligible |
| Measurement | incomplete traversal, metadata, or numeric state blocks an actionable plan |
| Automation | one JSON document; partial/failed work returns nonzero |
| Terminal output | control and bidirectional-control characters are escaped before rendering |

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
cargo build --release --locked --target aarch64-apple-darwin
bash -n scripts/release.sh && shellcheck scripts/release.sh && actionlint
gitleaks git --redact --no-banner .
trufflehog git "file://$(pwd)" --results=verified,unknown --fail --fail-on-scan-errors --no-update --no-color
```

See [`SECURITY.md`](SECURITY.md) for the threat model and reporting process.
Release notes live in [`CHANGELOG.md`](CHANGELOG.md). After committing and
pushing a version bump, enable GitHub immutable releases and run
`scripts/release.sh <version>-beta<N>` for staging. Each retry uses a new
counter. The script reruns local gates, requires successful CI for the exact
commit, and pushes an annotated tag. The hosted release workflow builds the
arm64 archive, signs SLSA provenance, publishes an immutable prerelease, and
verifies the downloaded asset. Production uses `scripts/release.sh <version>`;
the workflow promotes the exact highest verified beta artifact from the same
commit without rebuilding it.
