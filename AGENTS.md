# AGENTS.md — devtrim

Rust CLI (edition 2024, MSRV 1.88, pinned release toolchain 1.98.1). Sync code,
no async runtime, minimal dependencies. Ratatui 0.30 uses Crossterm 0.29 with
default features disabled; do not enable its optional layout cache without a new audit.

## Start and ownership

Read `MEMORY.md` and the current `memory/` journal, then `README.md`.
`src/app.rs` owns CLI dispatch; `src/ops/` owns category scan/apply;
`src/safety.rs` owns deletion validation; `src/journal.rs` owns audit records.
`src/tui.rs`, `src/analyze.rs`, and `src/status.rs` own the terminal surfaces.
Read `SECURITY.md` before changing an authority boundary and
`CODING_STANDARDS.md` before reviewing. The user's task is the specification
unless they name another source; pin the starting commit for change reviews.

Record existing edits before writing. Assign independent agents disjoint files;
the primary agent integrates and verifies. Stay in this checkout unless an
isolated worktree is authorized. Give an authorized worktree its own
`CARGO_TARGET_DIR`, test HOME, benchmark corpus, and evidence directory.
Never run cleanup against the developer's real HOME to test a change.

## Agent execution

For Fable 5.1 and GPT-6 Astra, specify the outcome, preserved boundaries,
verification, and stopping condition. Batch independent reads, edit targeted
hunks, and keep useful local work moving during independent reviews. Run the
smallest meaningful check first; widen for changed safety boundaries and final
delivery. A failed gate needs a diagnosis, not repeated unchanged runs.
Preserve the baseline, ownership, decisions, evidence paths, and remaining work
across compaction. Model/effort selection belongs to the harness, not this repo;
verify availability before claiming a model ran.

These procedures follow the [Fable 5.1 guide](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-fable-5-1)
and [Astra guide](https://developers.openai.com/api/docs/guides/latest-model?model=gpt-6-astra),
checked 2026-09-04. User instructions take precedence over skill procedures.

## Commands
- Rust-focused local checks: `bash scripts/verify.sh focused`; broader cached/offline checks: `bash scripts/verify.sh offline`. Neither replaces fresh audits, TruffleHog, fuzz, video, or review gates.
- Real terminal smoke: `python3 scripts/tests/tui.py target/debug/devtrim` (isolated HOME/PATH, explicit terminal dimensions, visible-content assertions).
- Read-only views: `python3 scripts/tests/read-only-views.py target/debug/devtrim` (analyze navigation/progress/help/resize; status sampling/resize/blocked-probe quit; terminal restoration).
- Format: `rustup run 1.98.1 cargo fmt --all -- --check`
- Lint: `rustup run 1.98.1 cargo clippy --locked --all-targets --all-features -- -D warnings`
- Structural lint: `ast-grep test --skip-snapshot-tests` then `ast-grep scan --config sgconfig.yml`
- Test: `rustup run 1.98.1 cargo test --locked --all-targets --all-features`
- MSRV: `rustup run 1.88.0 cargo test --locked --all-targets --all-features`
- Audit: `cargo audit`
- Fuzz (local release gate, nightly): with the nightly toolchain's bin dir first on PATH (`PATH="$(dirname "$(rustup which cargo --toolchain nightly)"):$HOME/.cargo/bin:$PATH"`), run `cargo fuzz run <target> -- -max_total_time=60` for each target in `fuzz/fuzz_targets/`. A standalone stable cargo earlier on PATH breaks cargo-fuzz's inner build.
- Video: `cd video && npm ci && npm audit --package-lock-only --audit-level=low && npm run lint && npm run format:check && npm run build`
- Build: `rustup run 1.98.1 cargo build --release --locked --target aarch64-apple-darwin`
- Site/manual: `python3 -m http.server 4173`, then open `/index.html` or `/MANUAL.html`

## Conventions
- Every cleanup category = one file in `src/ops/`, implementing the `Op` trait.
- Findings use typed actions. Confirmation flags may bypass a gate but MUST NOT add or widen actions.
- Apply consumes only exact previewed findings; never rescan for deletion targets after confirmation.
- `trash-empty` omits direct Trash children named as an ASCII-case variant of `.git`, warns, and leaves them in place so one protected item cannot block other exact previewed children.
- **Deletion-list evidence.** Every added or widened deletion-list entry carries evidence for its exact scope: the owner, the observed contents, and why removal is appropriate. Cite specific vendor documentation or a versioned owner source. Where neither exists, say so explicitly, name what was observed, and state the limit — an empty directory or a cache-like name is not evidence. A parent entry needs evidence for the whole subtree it authorizes. `safety::DeletionEntry` and `HistoryRoot` make the field required, so omission will not compile and a blank one will not pass; whether the cited source actually supports deletion stays a hard review requirement no gate can settle.
- `scripts/tests/planted-violations.py` proves the named safety assertions can still fail: it breaks each guarded branch on a throwaway copy and requires the tagged assertion to fail, rejecting a mutant that does not compile, selects no test, or fails somewhere else. It runs in `verify.sh offline`, ordinary CI, and the read-only release job, never in the credential-bearing release script; `release-policy.sh` enforces all four facts. It covers only the cases it names — currently the two `agents` deletion boundaries, the Codex current-release executable check, the refusal of a Codex release a process still executes, the refusal of an executable mapping `lsof` cannot name, the five per-list evidence loops, the two Git activity-probe hardenings, reflog activity and HEAD's own commit date, the no-replace quarantine rename, the `lsof` name decoding, the refusal of an unreported `lsof` process that is still running, the refusal of an incompletely read successor process, and the Xcode apply refusal of a non-directory target — and review owns the rest.
- `caches` apply continues past a refused finding, recording every failure so the run still reports nonzero. The list spans unrelated tools and a single entry can be permanently unremovable — a `uv` source distribution checked out with its own `.git` trips the repository-root refusal on every run — so stopping at the first failure would block every cache listed after it.
- `agents` apply continues past a refused finding, like `caches`, and reports an age-gate refusal as history that became active rather than as a target outside its namespace. The gate is re-read at apply, so a session resumed between preview and apply is an expected refusal that must drop only its own finding.
- Codex standalone releases are a third closed `agents` authority, `CODEX_RELEASES`, a `DeletionEntry` whose evidence is required. A release is offered only when the installer lock and `current` symlink prove an older direct-child layout-v1 package with canonical version numbers, and no process is executing any file inside it: one system-wide `lsof -d txt` taken under the lock, whose failure — including any mapping it cannot name with an absolute path — refuses release cleanup. Apply rechecks all of it, probing liveness again at each release rather than once per plan, and preserves current, newer, same-core, staging, running, and unknown layouts. Vendor files are opened no-follow, non-blocking and close-on-exec. A busy or unverifiable installer refuses only its release findings during apply.
- `~/.claude/jobs` is never a history root: it is the background-session supervisor's state (`state.json`, `timeline.jsonl`, `tmp`), a pinned session stays alive while idle and a shed one is woken from it, and the `pins.json` beside it is a liveness file a closed category must not need to consult.
- A `HistoryRoot` in `agents` earns its place only when the space it offers is worth the preview lines it costs. Measured against the development machine, Codex lanes gave 558 findings for 0.25 GB, its `.tmp` tree 324 for 0.07 GB, and the Claude Code file-edit history 123 for 0.10 GB; all three are excluded, and what remains offers 4.02 GB in 57 findings.
- Every human apply prints the data-loss notice; every interactive mutation confirms regardless of danger, `trash-empty` included. `-y` skips y/N only; `--yolo` skips all interactive prompts. Operation-specific acknowledgments such as `trash-empty --confirm=<gb>` still apply, and that acknowledgment is measured over the exact previewed findings, never the whole Trash; JSON remains machine-only.
- Global flags are capability-scoped: commands reject mutation flags they cannot honor. `scan --shred` may change previewed actions; Docker and simulator cleanup reject `--shred` because they execute typed commands rather than filesystem deletion actions; `trash-empty` accepts apply/confirmation flags but rejects `--shred`; report-only commands accept no mutation flags.
- Unknown activity, ownership, symlink resolution, owner-command status, configuration fields, or size measurement fails closed.
- `scan_all` owns shared liveness/Git observations for one preview, including failures. Standalone scans start fresh; apply must never reuse preview observations.
- `scan_all` runs the ten categories concurrently on scoped threads and joins them in registry order, so findings and errors stay byte-identical to a serial scan; a panicked scan thread becomes that category's error. Apply is never parallel, because the journal serializes each attempt/result pair. Cross-category diagnostics arrive in completion order, not registry order.
- `Op` has exactly one scan entry point, taking `&ScanObservations`. Observations live for exactly one scan: each repository's Git date and the build-process probe are computed once, the mutex is released before the subprocess runs, and a failed probe is memoized and re-yielded to every category so it blocks rather than silently passing.
- Hugging Face cleanup targets only `~/.cache/huggingface/hub`; its parent, tokens, and settings are never built-in cache authority.
- Filesystem findings retain an exact internal `PathBuf`; serialized display text is never parsed back into deletion authority.
- Only `safety::validate_path_for_deletion` creates `VerifiedTarget`; only the private sink in `src/ops/mod.rs` consumes it. Raw filesystem deletion anywhere else is a blocking ast-grep violation.
- Apply derives Trash versus permanent mode from the previewed typed `Action`, never from a runtime flag.
- Permanent deletion must be explicit in preview and danger scoring.
- Never add shell-string execution. Process execution uses a fixed program; a dynamic argument is allowed only when a closed typed authority validates and carries it.
- `simulators` deletes only unavailable devices. Working simulators are one report-only `Action::None` finding sized from simctl's `dataPathSize`, never a tree walk, parsed apart from the device authority and omitted with a diagnostic when a size is missing or malformed; apply skips it on actionability and returns before measuring when nothing in the plan is actionable, because the CLI applies such plans.
- Only `Finding::command` creates command authority from the closed `CommandAuthority` enum; apply verifies that capability, its validated arguments, and its serialized `Action` before execution. Docker authority binds an absolute local Unix-socket endpoint; simulator authority binds one exact UDID.
- Docker volumes are never pruned. Xcode Archives are never pruned.
- Xcode and Swift toolchain apply must reassert the scanner's exact direct-child target shape before calling the shared deletion sink. Xcode offers and applies only real directory children of `iOS DeviceSupport` and `DerivedData`; a file (Finder's `.DS_Store`) or a symlink beside them is never a symbol or build tree.
- `node_modules` apply must reassert the scanner's exact category shape: a real `node_modules` directory leaf inside its owning repo, never a symlink or through a symlinked category ancestor, nor below an ASCII-case variant of `.git`, another `node_modules`, or a non-normal path component.
- Every directory deletion preflights device boundaries and nested Git repository/worktree markers before either Trash or permanent mutation. Git metadata names are denied ASCII-case-insensitively by scanners, ownership and category checks, target validation, and open-handle preflight.
- `artifacts` matches only its closed corroborated-name list plus valid `CACHEDIR.TAG` signatures; ambiguous names (`build`, `dist`, `out`, `vendor`, `bin`, `obj`, `coverage`) are never added. Its scanner never traverses an ASCII-case variant of `node_modules`, apply independently refuses those ancestors, and corroboration is re-verified at apply.
- `installers` matches only direct children of `Downloads` and `Desktop` whose extension is on its closed list (`dmg`, `pkg`, `mpkg`, `iso`, `xip`), matched ASCII-case-insensitively, and only after the configured active window. Scanning never recurses, formats that can carry source or user data are never added, and apply reasserts the full shape, refusing symlinks and any target outside those two directories.
- `agents` keeps two closed tiers, never one, beside the separate Codex release authority above. A *regenerable* entry is an exact `$HOME`-relative cache its owner rebuilds on demand and is offered unconditionally; a *history* entry (session transcripts, shell snapshots, and archived sessions) is not regenerable and is offered only once the newest regular file anywhere in its subtree is older than the configured active window. Only regular files vote on that age, because a directory mtime moves on creation and on removal. History findings are children at exactly the configured depth below their root — Codex sessions nest `<year>/<month>/<day>` — so neither the root nor a deeper descendant can borrow its authority. A shell snapshot is history, not cache: it is written once per session, sourced by every later shell call in that session, and never rewritten if it disappears. Authentication material, configuration, memories, skills, agent definitions, installed plugins and `.claude.json` backups are on neither list. Apply reasserts tier membership, exact depth, symlink refusal, and the age gate re-read from disk.
- `~/.claude/projects` is never a history root. It holds Claude Code auto memory (`<project>/memory/`) keyed by repository root while transcripts beside it are keyed by working directory, and since v2.1.248 Claude Code retains a Desktop- or Cowork-originated transcript at any age with nothing in the filename to distinguish one. File age is therefore not evidence that anything there is finished with, and identifying the exception would mean reading transcript contents.
- `~/Library` stays protected wholesale. `safety::MANAGED_LIBRARY_CACHES` is the closed carve-out of exact `Library/Caches` subdirectories and is the single source of truth for both the protection boundary and the `caches` category, so a path can never be previewed but refused, or protected but unlisted. A name qualifies only when one developer tool owns that directory and rebuilds it on demand; a shared or ambiguous name is never added, and the pnpm entry is the metadata cache rather than the content-addressable store that installed dependency trees hard-link into. `Caches/JetBrains` is excluded because on macOS it is the IDE system directory whose product subdirectories hold the non-regenerable `LocalHistory` store, and `Caches/deno` because it is `DENO_DIR`, holding default-path Deno KV databases and `localStorage`. Matching is exact or `<entry>/`-prefixed, so a neighbour sharing a prefix stays protected.
- `docker` reports the host-side VM disk image as a non-actionable finding measured in allocated blocks, not logical length, because the image is sparse. It is emitted when `docker` is not installed. An installed `docker` whose daemon is unreachable fails the category, and that error names the image and its size; a refused non-local endpoint and a malformed `docker` response remain hard errors. devtrim never compacts or deletes a runtime disk image, and its notes never promise when the runtime returns freed blocks. The build-cache estimate is `docker system df` SIZE, labelled "up to", when ACTIVE is 0, because the daemon omits records shared with the image store from RECLAIMABLE and `builder prune -a` removes them; with records in use it is RECLAIMABLE, labelled "at least". Only the Build Cache row parses ACTIVE, and a non-integer there fails the category.
- Every deletion and typed command writes a write-ahead journal record (attempt before, result after) via `src/journal.rs`, synced per record; an unwritable journal blocks apply. Rotation is shift-and-rename at journal-open time only, never truncation, never mid-apply. `history` is read-only, reads rotated files, and emits its own single JSON document. An append first terminates a tail left without a newline by a short write, so no record fuses with a fragment.
- Findings capture preview-time device/inode identity; the sink refuses actionable filesystem findings whose identity is missing or drifted and verifies it through a cap-std parent-directory handle. Permanent deletion continues through that handle. Trash remains path-based after the identity check (no fd-anchored macOS Trash API) — keep that disclosure accurate.
- `analyze` is a read-only explorer and must never create deletion authority. Deletion in devtrim is always bound to a closed, corroborated category; an explorer that deleted the highlighted path would replace structural evidence with the operator's aim. It measures on a worker thread with a cancellation flag so the interface never blocks, never follows a symlink, never enters a foreign device, and discloses unreadable subtrees as partial lower bounds.
- `status` is read-only and adds no dependency: fixed-argv tools and fail-closed parsers supply every value. Unreadable metrics are unavailable with a reason, never zero; health lists missing inputs. Memory used is `active + wired + compressed`, excluding reclaimable inactive pages. Index variable-width `netstat -ib` rows from the end and match exact keys, never substrings.
- `uninstall` is a conservative report, never a complete inventory or deletion authority. Match the exact bundle identifier from the bundle's `Info.plist`, not display names. Product-named data is invisible to this match; disclose that limit. Omit group containers because entitlement names cannot establish ownership by suffix. Deletion would widen the shared `/Applications` and `~/Library` protection boundary and requires a separate decision.
- `optimize` uses fixed program/argv with no caller data. Keep root-requiring or hours-long tasks (`mdutil -E`, `periodic`, `purge`) and incomplete DNS flushing out, with regression coverage. `--apply` requires an explicit `--task`; `optimize` stays outside `ops::all()` so scan never reports maintenance commands as reclaimable space.
- `largest` is report-only visibility: `Action::Info` findings, lenient traversal with disclosed skip counts, never deletion authority, no TUI entry.
- Demo video: edit `video/src/DevtrimDemo.tsx`, render with `npx remotion render DevtrimDemo ../media/demo.mp4 --overwrite --muted` from `video/`, and verify a menu frame plus a single video stream before shipping.
- Config `protect` entries are deny-only, enforced inside `validate_path_for_deletion`, and fail closed on malformed input. No flag may bypass them.
- Liveness probes (`build_process_cwds`, `xcode_build_running`) use fixed argv `pgrep`/`lsof`; probe failure blocks the affected findings and surfaces as an error, never a silent pass. DerivedData liveness covers Xcode itself and the `SWBBuildService`/`XCBBuildService` processes IDE builds run through, not only `xcodebuild`. `lsof` escapes names for display; its unambiguous escapes are decoded and an ambiguous `^X` refuses, because a working directory whose displayed spelling differs from its path matches no repository. `lsof -p` exits 1 when any listed process is gone *or* unreadable, so its output is parsed per process (`p` then `n` lines) and exit 1 passes only when every unreported PID is absent from a fresh `pgrep` of the same pattern; a still-running unreported PID, a failed recheck, or malformed output refuses. A PID first seen in that recheck gets exactly one more `lsof`, which must report it completely. Both liveness `pgrep` calls pass `-a` (`PGREP_MATCH_ARGS`), because pgrep omits the caller's ancestors by default.
- The Git activity probe treats a scanned repository as hostile: fixed `-c` overrides for hooks, fsmonitor and signature display, plus `--no-show-signature`, `--no-lazy-fetch` and `--no-pager`, so preview never runs a program `.git/config` names. A `git` that cannot honor a flag fails the probe, which refuses. Activity is the newer of HEAD's commit date, read on its own, and HEAD's newest reflog entry; with reflogs disabled it is the commit date.
- `completions` and `manpage` write plain stdout and refuse `--json` with the standard error envelope.
- AGENTS.md and CLAUDE.md must stay byte-identical (release gate compares them).
- Whole worktrees are never deleted; `leftovers` is report-only.
- `--json` emits exactly one document; failed/partial operations return nonzero. A run-time error names the operation it failed in. `analyze` reports each lower-bound entry as an error, and `status --json` is its own documented vitals document.
- Gitleaks installation gates must pass the runtime synthetic-token positive control before the tool directory reaches `PATH` or any clean secret scan is trusted.
- User-facing strings are English; size values are estimated logical bytes.
- The TUI discards input queued while a scan or apply blocked its loop. Keys typed before a plan is displayed must never toggle permanent mode or approve it; `scripts/tests/tui.py` proves this in a real PTY with a positive control.
- Permanent-deletion quarantine and its restore rename with `RENAME_EXCL` (`renameat_with` + `NOREPLACE`), never a check-then-rename that could overwrite a file recreated at the name.
- Typed commands run through `ops::run_command_authority`, which journals them and reports a failure with the command, its exit status and stderr.
- `src/tui.rs` is a presentation adapter over existing `Op` scan/apply owners. It must not duplicate scanners, deletion logic, or danger policy. TUI apply requires a matching typed approval; CLI bypass flags never pre-authorize it.
- `src/theme.rs` is the only place that decides how a span looks. Render code names a semantic `Token`; no `Color::` literal may appear in `src/tui.rs`. Colors stay named ANSI rather than RGB so they resolve through the user's terminal theme, and `NO_COLOR` degrades every token to a modifier that keeps the same distinction, with the danger ladder ordered. A render-level test asserts no screen paints a cell under `NO_COLOR`, paired with a positive control proving the colored theme does paint.
- The footer shows only the keys valid on the current screen; `?` opens the complete reference. That overlay is refused on the confirmation screen, where it would obscure the exact plan being approved, and while it is open no key reaches the screen beneath it.
- Scanner diagnostics go through `Ctx`: explicit CLI commands may render stderr, while the TUI captures, escapes, and retains them in its own state.
- Complete human-facing actions, findings, errors, and notes are terminal-escaped at their final rendering sink, including clap parse errors that quote argv; JSON data remains unmodified.
- Bare `devtrim` opens the TUI only with interactive stdin and stdout. Non-TTY automation uses explicit subcommands; `--json` remains exactly one document.
- Keep CSP metadata intact in shipped HTML. Landing page is `index.html` + `styles.css`; demo media lives in `media/`.
- Code review reads `CODING_STANDARDS.md`. Every bullet in this section is a hard standard, citable as `CLAUDE.md § Conventions`.

## Apple platforms

devtrim itself contains no Swift — it is a Rust CLI, and nothing in this
repository triggers the rule below. It is recorded here because the workspace
owner asked every repository to carry it; the workspace copy in
`/Users/mneves/dev/AGENTS.md` remains authoritative if the two ever disagree.

For Swift, SwiftUI, iOS, iPadOS, or macOS work, read the documentation bundled
with the Xcode that is actually selected rather than older training data. Resolve
it, never hardcode it:

```bash
DOCS="$(xcode-select -p)/../PlugIns/IDEIntelligenceChat.framework/Versions/A/Resources/AdditionalDocumentation"
xcodebuild -version   # confirm which SDK that Xcode ships
```

`/Applications/Xcode.app` is not a synonym for the previous major version and
`/Applications/Xcode-beta.app` frequently does not exist — on this machine the
released `Xcode.app` is already 27.0 and there is no beta bundle, so a hardcoded
`Xcode-beta.app` path for the newest SDK would point at nothing. Select by what
`xcode-select -p` reports and what `xcodebuild -version` confirms, then read the
`AdditionalDocumentation` note matching the API you are touching (Liquid Glass,
App Intents, Swift Concurrency, SwiftData, FoundationModels, and so on).

Load the matching skill in `/Users/mneves/dev/Skills/XCODE_AGENT_SKILLS` before
changing Swift code — SwiftUI, Swift Concurrency, and Xcode Build Optimization
in particular — and review the changed Swift surface against those sources and
the project's own deployment target.

## Release
1. Bump `Cargo.toml` and every version reference packaged with the artifact. This includes regenerating `fuzz/Cargo.lock` (`cd fuzz && cargo update -p devtrim --precise <version>`): the fuzz crate depends on `devtrim` by path, so its tracked lockfile still pins the old version and the hosted "fuzz gates leave the checkout clean" step fails on a dirty `fuzz/Cargo.lock` after every bump.
2. Add the dated `CHANGELOG.md` section; update README, manual, security, and agent docs. Keep the production landing page on the live stable version during beta staging.
3. Run every command above; MSRV must execute and may never be skipped. Also run a real PTY TUI cancel flow against a disposable home, shell syntax and ShellCheck for every script under the release policy, `actionlint`, Gitleaks, and TruffleHog.
4. Run the local autoreview helper in local mode and inspect the final diff.
5. Commit and push a clean tree.
6. GitHub immutable releases must be enabled. Stage with `scripts/release.sh <version>-beta<N>`; every retry uses a new `N`. The credential-bearing script performs provenance-only preflight and pushes an annotated tag; read-only hosted jobs rerun every gate before the no-checkout publisher receives release-write/OIDC authority.
7. The hosted release workflow builds/verifies arm64, packages the full Apache-2.0 license, signs artifact provenance, publishes an immutable prerelease, and verifies the remote asset.
8. After staging verification, promote the same commit with `scripts/release.sh <version>`. Production reuses the exact highest verified beta artifact and checksum without rebuilding, then automatically runs the idempotent Homebrew closeout. That helper re-verifies release provenance, changes only the tap formula, pushes normally, locks validation to the published tap commit, upgrades/tests the existing `/opt/homebrew` installation, and requires it to be the sole visible `devtrim`. Beta never invokes it; a failed closeout resumes with `scripts/update-homebrew.sh <version>` without moving the tag or release.
9. After verification, update the production landing page and open the next patch `Unreleased` section.
