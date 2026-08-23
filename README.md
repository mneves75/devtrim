# devtrim

Developer-machine disk hygiene for macOS: **measure, classify, trim — safely.**

Born from a real cleanup session that reclaimed 250+ GB: HuggingFace caches,
stale `node_modules`, simulator storage, Xcode support files, Docker bloat,
duplicate Swift toolchains, and agent scratch directories.

**[Website](https://mneves75.github.io/devtrim/)** · **[Manual](https://mneves75.github.io/devtrim/MANUAL.html)** · **[Download v0.2.0](https://github.com/mneves75/devtrim/releases/tag/v0.2.0)**

## Install

Download the Apple silicon archive from the [v0.2.0 release](https://github.com/mneves75/devtrim/releases/tag/v0.2.0), then verify it with the included checksum:

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

- **Preview by default.** Nothing changes without `--apply`.
- **Trash-first.** Filesystem deletions go to macOS Trash (recoverable). `--shred` opts out.
- **Danger scores.** Every finding carries 1–10; gates scale accordingly:
  - ≤2: no prompt
  - 3–5: `-y` skips prompt
  - 6–8: interactive y/N (or `-y`)
  - ≥9: typed numeric confirmation (`--yolo` overrides)
- **Non-TTY safe.** Piped/agent runs refuse to mutate unless `-y`/`--yolo` is explicit.
- **Protected paths.** `/System`, `/usr`, `/Applications`, `~/.ssh`, wholesale `~/Library`, … are refused unconditionally; only known managed subpaths under Library are eligible.
- **Volumes are sacred.** Docker prune never touches volumes (a live DB volume is user data).
- **Agent-friendly.** `--json` on everything.

## Usage

```bash
devtrim scan                     # full read-only report
devtrim scan --json              # machine-readable
devtrim clean caches --apply     # HF/uv/npm/brew caches
devtrim clean node-modules --apply -y   # stale repos only; active skipped
devtrim clean simulators --apply # delete-unavailable (erase-all needs --yolo)
devtrim clean xcode --apply      # DeviceSupport + DerivedData (Archives exempt)
devtrim clean docker --apply     # unused images + build cache
devtrim clean toolchains --apply # old swift.org toolchains (swift-latest preserved)
devtrim clean leftovers --apply  # agent scratch dirs, .supergoal evidence
devtrim icloud                   # upload status of big queued files
devtrim trash-empty --confirm=14 # typed-size acknowledgment required
```

## Config (optional) — `~/.config/devtrim.toml`

```toml
roots = ["~/dev"]        # scan roots
active_days = 30         # repos with commits newer than this are "active"
```

## Safety model

| Layer | Rule |
|---|---|
| Preview | dry-run is the default; `--apply` mutates |
| Trash | default deletion target; recoverable via Finder |
| Danger gate | static score per op + dynamic escalation at 1/10/50 GB |
| Protected paths | hard denylist, no flag bypasses |
| Size guard | >10 GB bumps danger to ≥7, >50 GB to ≥8 |
| Non-TTY | refuses mutation without explicit flags |

## Build and release

```bash
cargo build --release --locked
cargo test
```

Release notes live in [`CHANGELOG.md`](CHANGELOG.md). After committing and pushing a version bump, run `scripts/release.sh <version>` to build the archive, checksum it, tag the commit, and create the GitHub release.
