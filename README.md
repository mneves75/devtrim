# devtrim

Developer-machine disk hygiene for macOS: **measure, classify, trim — safely.**

Born from a cleanup session that reclaimed 250+ GB across model caches, stale
`node_modules`, simulator storage, Xcode support files, Docker bloat, and old
Swift toolchains.

**[Website](https://mneves75.github.io/devtrim/)** · **[Manual](https://mneves75.github.io/devtrim/MANUAL.html)** · **[Download v0.3.0](https://github.com/mneves75/devtrim/releases/tag/v0.3.0)**

## Install

Download the Apple silicon archive from the [v0.3.0 release](https://github.com/mneves75/devtrim/releases/tag/v0.3.0), then verify it with the included checksum:

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
- **Fail closed.** Unknown Git activity, broken toolchain links, malformed config, symlinked ancestors, and failed owner commands block mutation.
- **Danger scores.** Actionable findings carry 1–10; aggregate size can raise the plan score:
  - ≤2: no interactive prompt, but non-TTY apply still needs `-y`/`--yolo`
  - 3–8: y/N prompt (`-y` skips it)
  - ≥9: typed numeric confirmation (`--yolo` skips confirmation only)
- **Typed deletion boundary.** Exact `PathBuf` targets must become an internal `VerifiedTarget` immediately before the single deletion sink can consume them. Display strings are never deletion authority.
- **Protected physical paths.** System roots, user secrets, the home root, Trash root, paths reached through symlinked ancestors, and owner-reported cache paths outside npm/Homebrew namespaces are refused.
- **Volumes are sacred.** Docker volumes are never pruned.
- **Archives are sacred.** Xcode Archives are visible but never actionable.
- **Agent-friendly.** Every `--json` invocation emits exactly one JSON document and failures return nonzero.

## Usage

```bash
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

`--yolo` only bypasses confirmation. It never adds simulator erase-all or any
other operation that was absent from the preview.

## Config — `~/.config/devtrim.toml`

```toml
roots = ["~/dev"]        # scan roots
active_days = 30         # newer commits make a repo active
```

Explicit `--root` flags replace config/default roots. Existing roots are resolved
before preview. An unreadable or malformed config is an error; devtrim never
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
| Target identity | exact internal `PathBuf`; display text is never parsed back into authority |
| Deletion sink | only `VerifiedTarget` reaches physical removal; action selects Trash vs. permanent mode |
| Physical path | literal and resolved parent must agree; deny-only resolution |
| Activity | unknown Git/toolchain ownership is ineligible |
| Automation | one JSON document; partial/failed work returns nonzero |

Sizes are estimated logical bytes. APFS clones, sparse files, and container-VM
compaction can make immediately available disk space differ.

## Build and verify

```bash
cargo fmt --all -- --check
ast-grep test --skip-snapshot-tests
ast-grep scan --config sgconfig.yml
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
cargo +1.85.0 test --locked --all-targets --all-features
cargo audit
cargo build --release --locked --target aarch64-apple-darwin
bash -n scripts/release.sh && shellcheck scripts/release.sh && actionlint
```

See [`SECURITY.md`](SECURITY.md) for the threat model and reporting process.
Release notes live in [`CHANGELOG.md`](CHANGELOG.md). After committing and
pushing a version bump, run `scripts/release.sh <version>` to rerun local gates,
require successful CI for that exact commit, build and verify the arm64 archive,
tag the commit, and create the GitHub release.
