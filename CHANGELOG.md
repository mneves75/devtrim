# Changelog

All notable changes to devtrim. Format follows Keep a Changelog; versioning is semver.

## [0.9.2] - 2026-09-12

Every closed-list entry in 0.9.1 was justified by a doc comment covering several
entries at once, and nothing failed if one arrived with no justification at all.
Six entries had already reached a release on the strength of a directory *name*.
This release makes that impossible to repeat quietly.

### Added
- Evidence is now a required field, not a comment. `safety::DeletionEntry` carries `label`, `relative` and `evidence`, and `HistoryRoot` gains the same field, so a new deletion-list entry without evidence **does not compile** (`E0063`) and one whose evidence is empty or whitespace fails a const assertion while compiling. Each of the four lists — the two agent tiers, the built-in caches, and the `~/Library/Caches` carve-out — now states per entry what the directory holds and the source that establishes it, including `.codex/cache` recorded honestly as inspection-only because no vendor documentation describes it
- `scripts/tests/planted-violations.py`: a gate that proves named safety assertions can still fail. It breaks each guarded branch on a throwaway copy of the source and requires the *tagged* assertion to fail, rejecting a mutant that does not compile, selects no test, or fails at a different check — devtrim had already shipped a symlink-refusal assertion that could not fail, and nothing noticed for several commits. Two fixed cases, not a mutation framework: `cargo-mutants` counts a mutant as caught when *any* test fails, which is exactly the confusion this removes. Runs in `verify.sh offline`, CI, and the read-only release job in about 14 seconds; `release-policy.sh` requires all three invocations and forbids a fourth in the credential-bearing script
- `CODING_STANDARDS.md S1` gains a citable **planted-violation proof** rule with a worked example, rather than a competing new rule

### Changed
- The regenerable agent tier no longer under-warns. Its finding note said "rebuilt on demand by the agent", which describes the content coming back but not the cost of removing it from under a running agent; it now reads "rebuilt on demand; cleanup may interrupt an active session, so close agents first". No vendor documents these directories as removable mid-session, and Trash-first is *recovery* rather than safety — `--shred` removes even that. A CLI test asserts the warning reaches both the JSON envelope and the human preview, since a warning only machines can see is not a warning

## [0.9.3] - 2026-09-12

A retroactive review of the shipped 0.9.2 commit — run because the release
attestation had been given without it — found that the release which made
evidence mandatory shipped an entry with false evidence.

### Fixed
- `~/Library/Caches/gh` leaves the carve-out. Its evidence called it the GitHub CLI's API-response cache, but go-gh resolves that cache to `$XDG_CACHE_HOME/gh` and then `~/.cache/gh`, never to `~/Library/Caches` on macOS. Verified here: `XDG_CACHE_HOME` is unset, `~/.cache/gh` exists with content, and `~/Library/Caches/gh` does not exist at all. The entry was therefore authorized on the strength of its *name* — the exact thing 0.9.2's own convention calls insufficient — and it had been carrying that authority since 0.9.1
- gh's real cache is now covered: `~/.cache/gh` joins the built-in list with evidence that states the resolution order, the verification, and that cached private API response bodies can live there even though credentials cannot
- The three `carries_evidence` tests could not fail for the cases their documentation described. The const assertion rejects empty and ASCII-whitespace evidence *while compiling*, so such a crate never builds and no test runs; the 0.9.2 changelog's claim that "a test additionally names the offending path" was unobservable, and the commit's claim to have proven it was mistaken — that proof had shown the const assertion firing. Each test is now scoped to the one gap the const check cannot see, a non-ASCII blank such as U+00A0 that `str::trim` strips, and proven on it
- `planted-violations.py` removed its scratch tree only on success. Every `fail()` and an uncaught build timeout exited first, leaving a full source copy and an `--all-features` debug build — about 558 MB each — under `target/`. The likeliest failures are the early ones, in a gate developers run locally. Cleanup now runs in a `finally`, verified on both the success and failure paths

## [0.9.4] - Unreleased

## [0.9.1] - 2026-09-11

Everything 0.9.0 claimed was checked against the vendors' own documentation
rather than against the directory names. Nothing was contradicted — no shipped
rule called a directory safe to delete when it is not — but two entries turned
out to rest on no source at all, and they are gone.

### Removed
- `~/.claude/downloads` is no longer a regenerable-cache entry. No Anthropic documentation acknowledges the directory, so its contents cannot be characterised; a deletion rule for a directory nobody documents is the weakest kind of entry, and in this category the closed list is the only thing guarding the path. Every remaining entry now cites a primary source for what it holds
- `~/Library/Caches/claude-cli-nodejs` leaves the `~/Library` carve-out. Despite its location it holds per-project `mcp-logs-<server>/` diagnostic logs, which nothing regenerates — removing them loses MCP debugging history rather than costing a re-fetch, so the "regenerated automatically on next use" note was wrong about it

### Fixed
- `devtrim clean agents --badflag --json` reported `"operation": "clean"` instead of `"agents"`, the only cleanup target missing from the error-envelope map. Every sibling target was already listed
- The `agents` symlink refusal had no test coverage. The existing test planted its symlink under `~/.claude/projects`, a root retired during 0.9.0 review, so the scan never reached the symlink check and the assertion could not fail. The fixture now sits in a live root with a real stale transcript beside it as the positive control, and both the leaf refusal and the traversal skip are exercised
- `SECURITY.md` still documented the session-shape corroboration rule that 0.9.0 removed along with the root it guarded, telling anyone auditing the boundary about a check that cannot fire
- A stale test fixture and comment referred to `~/.claude/jobs` and the removed `include_files` field as if both still existed
- The `SECURITY.md` boundary saying `~/.claude/projects` and `~/.claude/jobs` are never cleanup roots is now executable rather than prose. Both were roots during development and both were retired after review found live or unjudgeable data inside them, so a test asserts it structurally at the lists and behaviourally at the scan, with a `.codex` fixture as the control proving the scan could have returned something
- The error-envelope map is now checked as a table over clap's own variant list. `agents` shipped with the wrong operation name precisely because nothing enumerated the subcommands against that map, so an eleventh category cannot regress the same way
- The retired-root boundary is checked in both directions. A target *beneath* one of those trees deletes part of it, but a target that is an *ancestor* — a `.claude` entry, say — deletes the whole thing while never starting with the retired path, and the first version of the assertion would have passed that. Planting an ancestor entry now fails the test with the path it would have reached
- The symlink fixture pointed at a directory, where the wrong-file-type check would have refused it even with the symlink check deleted. It points at a regular file now, so symlink-ness is the only thing that can refuse it, and the apply assertion checks the refusal names the symlink rather than counting errors

### Changed
- `dir_stats` now reports an unreadable modification time as `None` rather than failing the whole measurement. 0.9.0 made every category fail-closed on a timestamp when only the `agents` age gate reads one, so a tree that measures perfectly well could stop being measurable; the staleness caller still treats a missing timestamp as a refusal
- `MANAGED_LIBRARY_CACHES` entries are asserted to be exactly one normal path component. The list is read by two places that treat it differently — the cache category joins it raw, the protection boundary sees only cleaned paths — so an entry containing `..` would be previewed under one spelling and matched under another. Nothing but this assertion prevented that
- Documented what the sources actually say: removing `com.microsoft.VSCode.ShipIt` mid-update interrupts that update, the Go build cache holds a fuzz corpus that only fuzzing regenerates, and Corepack's downloads live inside the `.cache/node` entry that already covers them
- The regenerable tier no longer claims a running agent will not notice. No vendor documents these directories as removable mid-session; Trash-first is what makes the tier safe, and the claim is now only that the content comes back
- `clean leftovers` is named in the README as where agent scratch worktrees are reported, closing a gap between what was asked for and what the docs said was delivered

### Added
- An Apple-platforms section in `AGENTS.md`/`CLAUDE.md`, recorded at the workspace owner's request. devtrim contains no Swift, so nothing here triggers it; it resolves the Xcode documentation path from `xcode-select -p` rather than hardcoding `Xcode-beta.app`, which does not exist on the machine this was written on — where the released `Xcode.app` is already 27.0

## [0.9.0] - 2026-09-10

### Added
- `devtrim clean agents`: a tenth cleanup category for coding-agent storage, in two tiers because it is not one kind of data. The *regenerable* tier is a closed list of exact `$HOME`-relative caches an agent rebuilds on demand — Claude Code downloads and metadata cache, the Codex catalog cache, the Pi web-search cache, the OpenCode cache — offered unconditionally at a low danger score. The *history* tier — Claude Code shell snapshots; Codex shell snapshots, session and archived-session trees — is **not regenerable**, so a child is offered only once the newest regular file anywhere in its subtree is older than the configured active window, its note says so, and its danger score reflects it. Codex nests sessions as `<year>/<month>/<day>`, so the day directory is the unit: waiting for a whole year to go stale would never offer the current one
- Authentication material (`auth.json`, `.credentials.json`), configuration, memories, skills, agent definitions, installed plugins, and `.claude.json` backup copies are on neither list and are never candidates. Apply reasserts tier membership, exact depth below the configured root, symlink refusal, and the age gate re-read from disk, so a session resumed between preview and apply falls out of the plan rather than being deleted
- `~/.claude/projects` is not a cleanup root at all. It holds Claude Code auto memory at `<project>/memory/`, keyed by *repository root* while transcripts beside it are keyed by *working directory* — so a project directory can hold memory and no live transcript. Narrowing the unit to session-shaped entries answered that, but not the second problem: since v2.1.248 Claude Code retains a transcript originating in Claude Desktop or Cowork at any age, and nothing in the filename distinguishes one. File age is therefore not evidence that anything under this root is finished with, and identifying the exception would mean reading transcript contents — the same "consult a liveness file" shape that disqualified `~/.claude/jobs`. The root returned 0.00 GB on the development machine, so the honest trade was to drop it
- A shell snapshot is treated as history, not cache. Claude Code writes one per session and sources that exact file on every later shell call, and nothing rewrites it when it disappears, so calling it regenerable and deleting it under a live session was wrong on both counts. Both agents' snapshot directories now sit in the age-gated tier, where an untouched file is evidence that no live session still needs it
- Only regular files vote on staleness. A directory's own mtime moves when the tree is created and whenever an entry is removed, so a store restored from backup would otherwise look permanently active; an addition is still caught, because the added file carries its own fresh timestamp
- `devtrim clean caches` now also covers the bun package cache and the cargo registry, both its download cache and its extracted sources
- `devtrim clean caches` reaches a closed list of exact `~/Library/Caches` subdirectories: Playwright browsers, the VS Code HTTP cache and its Squirrel update staging, SwiftPM, the Claude Code CLI cache, pip, the pnpm metadata cache, GitHub CLI, Go, and TypeScript. `~/Library/Caches/JetBrains` is deliberately absent: on macOS that is the IDE *system directory*, and each `<Product><Version>` subdirectory holds `LocalHistory`, the per-file change history the IDE keeps for files Git never saw — JetBrains stopped clearing it on "Invalidate Caches" for exactly that reason. Carving out only its `caches` and `index` subdirectories would need a per-product depth rule this exact-name list cannot express. `~/Library/Caches/deno` is absent for the same reason: on macOS it is `DENO_DIR`, and `location_data/<hash>/kv.sqlite3` is where every `Deno.openKv()` opened without an explicit path stores its database, with the sibling `local_storage` file backing `localStorage` — both documented as persistent across runs, and neither rebuilt by anything. The pnpm entry is deliberately the metadata cache and never the content-addressable store at `~/Library/pnpm/store`, which every installed `node_modules` tree hard-links into. Editor logs and application data stay out of scope: VS Code keeps its logs, `CachedData` and webview caches under `~/Library/Application Support/Code`, and JetBrains keeps logs under `~/Library/Logs` — both outside `~/Library/Caches`, both mixed in with real user state, and neither reachable without a second carve-out this release does not make
- A finding's note now says a cache is "regenerated" rather than "re-downloads": an editor index or a compiler cache is rebuilt locally, and the old wording misdescribed the cost of removing one
- `~/.claude/jobs` is deliberately not a history root. Despite the name it is the background-session supervisor's state — `state.json`, `timeline.jsonl`, `tmp` — rather than job output: a pinned session is kept alive while idle and a shed one is woken from that state, so an idle stretch past the active window would offer a directory a live process still owns. The `pins.json` beside it records exactly that, and a category that has to consult a liveness file to stay safe is one directory too far

### Fixed
- `devtrim clean agents --apply` no longer abandons the rest of the previewed plan when one finding is refused, and an age-gate refusal now says the history became active rather than claiming the target was outside its authorized namespace. The age gate is re-read at apply, so a session resumed between preview and apply is an ordinary, expected refusal — and the documented promise is that such a session falls out of the plan, not that it takes every later finding with it
- `devtrim clean caches --apply` no longer abandons the rest of the previewed plan when one cache is refused. Found by running it: a `uv` source distribution checked out with its own `.git` trips the repository-root refusal on every run, and because it sorts first it blocked all eight remaining caches — 4.56 GB that reported as zero. Every failure is still recorded, so the run continues to exit nonzero

### Changed
- `~/Library` remains protected wholesale. `safety::MANAGED_LIBRARY_CACHES` is the closed carve-out for its `Caches` subtree and is the single source of truth for both the protection boundary and the cache category, so a path can never be previewed but refused by the sink, or protected but silently unlisted. Matching is exact or `<entry>/`-prefixed, so a neighbour sharing a name prefix stays protected. A name qualifies only when one developer tool owns that directory and rebuilds it on demand; a shared or ambiguous name is not added
- `devtrim scan` and the terminal interface now cover ten categories; the new one is reachable in the TUI with `a`

## [0.8.2] - 2026-09-05

### Changed
- `devtrim scan` now runs its nine category scanners concurrently instead of one after another, so the independent external programs each category waits on (`npm`, `brew`, `xcrun`, `docker`, `pgrep`, `lsof`, `git`) overlap rather than queue. Results are joined in registry order, so the JSON document is byte-identical to the serial version: over a 25-repository corpus the parallel binary and the previous serial binary produce the same SHA-256, and twelve consecutive parallel runs produce that one digest. Cross-category diagnostic lines now appear in arrival order rather than registry order; findings, errors, and the single-document guarantee are unaffected
- `Op` now has exactly one scan entry point, taking the per-scan observations explicitly. The previous optional second method could be left unimplemented by a new category, which would silently reprobe instead of sharing the scan's observations

### Fixed
- Removed duplicated and vacuous test code: two `node_modules` apply tests whose shapes the authority table already covers, a Git fixture duplicated byte-for-byte across two modules, five forged-authority tests collapsed into two table-driven ones, a docker stub pasted three times, and four assertions that restated their own setup or re-derived the value under test. The unit suite runs in roughly half the wall time it did
- The `node_modules` authority table now asserts the refusal *reason* for non-normal path spellings rather than only that some refusal occurred

### Security
- Added an adversarial concurrency regression test: a mutator thread races the deletion sink, repeatedly swapping the target with a symbolic link to a bystander file while the sink runs. Across sixty loaded runs the sink never destroyed the bystander. The test carries a deterministic control deletion first, so an all-refusals outcome cannot be mistaken for success
- Restored explicit assertions that the home directory root and the Trash root are protected. Both share one branch of the protected-path check that no remaining test covered after the test cleanup; a deliberately broken branch now fails the suite
- Restored coverage for a forged actionable finding carrying no path at all, in both the Docker and simulator authority tables

## [0.8.1] - 2026-09-04

### Security
- Limit Hugging Face cleanup to `~/.cache/huggingface/hub`, preserving authentication tokens and other parent state; scanner and apply reject the parent as cleanup authority
- Pin Rust 1.98.1 to avoid the 1.98.0 vtable-generation miscompilation; MSRV remains 1.88.0

### Changed
- Share successful and failed build-process and Git observations across one scan, keeping fresh checks at apply; reuse installer eligibility metadata within a scan
- Redraw analyze and status dashboards only when visible state changes, and format only the visible analyze rows
- Consolidate subprocess parsing, category dispatch, removal notes, root normalization, and popup geometry; remove redundant forwarding code and unused video scaffolding/dependencies
- Add isolated PTY verification, an explicit local verification helper, and a controlled A/B benchmark harness that rejects failed or unequal scans and overloaded-host timing
- Refresh shared agent instructions, verification and worktree guidance, and compiler/shell/terminal checks in CI and release workflows

### Fixed
- Preserve the `installers` operation name in JSON command-line parsing errors
- Refresh the demo version, installer menu entry, Hugging Face target, and size-escalated danger scores

## [0.8.0] - 2026-09-04

### Security
- The demo-video dependency graph moves `fast-uri` from 3.1.5 to 3.1.6, clearing four high-severity advisories (host confusion via skipped IDN canonicalization and via percent-encoded scheme normalization, plus SSRF via malformed IPv6 normalization and via repeated hostname percent-decoding). It reaches the tree four levels down, through `@remotion/cli` to `webpack` to `schema-utils` to `ajv`, and does not enter the shipped binary — but the video graph is a release gate, so the advisory blocked the release until fixed

### Added
- `devtrim optimize` runs macOS maintenance tasks as typed commands with fixed argv and no caller-supplied data: QuickLook thumbnail cache, user font caches, and the Launch Services database. `--apply` requires an explicit `--task`, because `plan_danger` takes the maximum and one confirmation must not authorize unrelated work. A task that cannot do what its name says is not offered: root-requiring or hours-long ones, and DNS, because `dscacheutil -flushcache` does not clear the `mDNSResponder` resolver cache it would advertise
- `devtrim status --watch` is a live dashboard: sampling runs on a worker thread and the interface redraws when a report lands, so a slow probe delays the numbers rather than the keyboard. Every metric keeps a fixed row slot and an unreadable one renders as `unavailable`, so a number never moves because a probe failed once. Quitting does not join the sampler: the stop flag is only observed between samples, so joining would make `q` wait out an in-flight probe and a hung system command would block the exit entirely. It has no JSON form and says so instead of ignoring the flag
- `devtrim uninstall <app>` lists the paths macOS keys by an application's exact bundle identifier, read from the bundle's own `Info.plist`: support directories, caches, containers, preferences, saved state, HTTP storages, WebKit data, and launch agents. Matching is exact — `com.example.thing` never selects `com.example.thingy`, and a display name never selects by word — which is why it works at all: Amazon Kindle is `com.amazon.Lassen`. It is a conservative report rather than an inventory, and says so: an app storing data under a product name is invisible to identifier matching, and group containers are omitted because their names come from an arbitrary entitlement. Report-only, because `safety::is_protected` refuses `/Applications` and everything under `~/Library` outside a four-entry allowlist, and widening that would weaken every command rather than only this one

## [0.7.0] - 2026-09-01

### Added
- `devtrim status` reports read-only machine vitals — uptime, load, memory, disk, battery, thermals, cumulative network, busiest processes — and a health score that names every input it could not read instead of scoring over the gap. Each value comes from a fixed-argv system tool through a parser that fails closed on malformed input. Disk is measured on the writable Data volume rather than the sealed root, because `df /` reports a nearly full machine as 17% used; memory used is stated as `active + wired + compressed`, because counting reclaimable inactive pages reports a healthy machine at 96%
- `devtrim analyze [path]` is an interactive, read-only disk explorer: it measures each child on a worker thread and streams results in as they land, so a directory that takes minutes to size never freezes the interface, and leaving a directory cancels its in-flight walk. Symbolic links are reported at their own size rather than followed, a different device is never entered, and unreadable entries are disclosed as `(partial)` lower bounds. `--json` emits one document; every mutation flag is rejected
- The terminal interface honors `NO_COLOR`, degrading every style to a modifier that preserves the same distinction — the danger ladder stays ordered as dim, plain, bold, bold+reversed — so the interface remains usable with color stripped entirely
- `?` opens a full keybinding reference over any screen, deliberately except the confirmation prompt, where a second overlay would obscure the plan being approved; the footer keeps only the few keys that apply to the current screen
- `clean installers` reclaims downloaded installer archives (`dmg`, `pkg`, `mpkg`, `iso`, `xip`) left as direct children of `Downloads` and `Desktop` after the configured active window, refusing symlinks, nested copies inside extracted project trees, and any target outside those two directories at apply time

### Changed
- Terminal styling moved from 30 inline color literals to semantic tokens in `src/theme.rs`, so call sites name what a span means and one module decides how it looks; colors remain named ANSI rather than RGB so they keep resolving through the user's own terminal theme
- Every tracked `*.sh` plus the pre-commit hook now pass `shellcheck` before local commits and in CI through one fail-closed, NUL-safe helper; CI installs the official ShellCheck 0.11.0 arm64 asset only after checksum verification
- CI and non-Intel release jobs move from the deprecated `macos-14` image to the supported `macos-15` arm64 image with exact runner-policy checks; the deterministic x86 release gate remains on `macos-15-intel`
- Ordinary PR/main CI now installs checksum-verified arm64 Gitleaks 8.30.1 and TruffleHog 3.97.1, proves Gitleaks detects a non-allowlisted synthetic PAT, then runs the same full-history secret scans that release gates already run

### Fixed
- `clean docker` under-reported reclaimable space by roughly 7x because `docker system df` measures only inside the guest: the host-side OrbStack/Docker Desktop VM disk image is now disclosed as a report-only finding measured in allocated blocks, with a note stating that pruning frees guest space but never shrinks that sparse file until the runtime compacts it
- The Docker VM disk image is now reported even when the daemon is not running, which is the one state where it is invisible to `docker` and still occupying the host; a refused remote endpoint and a malformed `docker` response remain hard errors
- Artifact scanning and apply now both refuse targets below every ASCII-case variant of `node_modules`, closing the sibling dependency-namespace deletion path with an end-to-end surviving-sentinel regression
- `trash-empty` now warns and leaves a direct `.git` case variant in place without letting that protected item block other exact previewed Trash children
- Permanent and Trash preflight reuse each directory listing for Git-marker checks instead of enumerating every directory twice, while retaining the final mutation-time recheck
- Git-backed unit fixtures now disable ambient commit signing and hooks, so maintainer Git configuration cannot make the Rust suite fail
- Release policy now proves the Gitleaks positive control runs before the scanner directory reaches `PATH`, and both workflows syntax-check that control script explicitly
- `node-modules` apply now reasserts the scanner's exact target shape before deletion, refusing non-directory and symlink targets, symlinked category ancestors, forged non-`node_modules` leaves, plus ASCII-case-insensitive `.git` and outer `node_modules` ancestors and non-normal paths
- The landing page and packaged manual now declare a compact project favicon instead of generating a browser-level `/favicon.ico` 404 on every fresh visit
- The landing-page hero caption now keeps readable contrast over every part of its image instead of combining muted text with a translucent overlay
- The ShellCheck helper now fails before linting with an actionable error when `shellcheck` is unavailable, and release policy proves that path does not invoke ShellCheck
- `CODING_STANDARDS.md` no longer tells review to skip gates that run only at release (`cmp -s AGENTS.md CLAUDE.md`, the fuzz targets, `actionlint`), and now lists the `video/`, shell, and secret-scanning merge gates it had omitted, so a reviewer no longer spends the budget on checks that already block
- `CODING_STANDARDS.md` corrects an S1 precedent that no search could find, S12's incomplete list of approved dynamic call sites, S6's unstated denylist, and the ast-grep escape hatches sanctioned by the deletion-sink rule
- The binary entry point now carries the `//!` module contract that S4 requires of every file under `src/`

### Security
- Git metadata is now denied ASCII-case-insensitively by project scanners, ownership and category checks, target validation, and open-handle Trash/permanent preflight, closing actionable `.GIT` findings on case-insensitive macOS filesystems
- Common local environment, private-key, and signing-material files are ignored, while checksum-pinned full-history Gitleaks and TruffleHog scans now block ordinary PR/main CI as well as releases
- CI and release refuse a Gitleaks binary that cannot trip a runtime positive control, so a version string and clean scan cannot mask a no-op detector

## [0.6.3] - 2026-08-28

### Changed
- Global mutation flags are now capability-scoped: report-only commands reject flags they cannot honor, while `scan --shred` and Trash purge retain only their meaningful controls
- Docker and simulator cleanup now reject `--shred` instead of accepting a flag that cannot affect their exact typed command actions
- Release version validation now checks the authoritative changelog heading and exact, unique README and manual version declarations instead of accepting substring matches

### Fixed
- The TUI now filters configured protected Trash items before it calculates danger or asks for approval
- Bare `devtrim --json` now rejects the implicit TUI before terminal launch, matching explicit `devtrim tui --json` and preserving automation-only JSON behavior
- A present but missing, broken, escaping, or otherwise invalid `swift-latest.xctoolchain` reference now blocks toolchain cleanup instead of producing an empty successful scan
- The production landing page stayed on the actual stable v0.6.2 archive and release while the v0.6.3 candidate was in beta staging

### Security
- Human command previews escape the complete action string, closing terminal control-character injection through dynamic but validated command arguments without altering JSON data
- Xcode and Swift toolchain apply now reassert each scanner's exact direct-child target shape before the shared deletion sink, so a forged nested finding cannot borrow the category's authority
- Release verification passed current and MSRV suites, strict Clippy, structural positive controls, root and fuzz dependency audits, all five 60-second fuzz targets, the arm64 build, PTY cancellation, workflow/shell/secret gates, P3 autoreview, Matt Pocock standards/spec review, video build/render/container checks, desktop/mobile browser checks, and a fresh independent verifier

[0.6.3]: https://github.com/mneves75/devtrim/compare/v0.6.2...v0.6.3

## [0.6.2] - 2026-08-28

### Changed
- Production release closeout now re-verifies the immutable artifact, updates and audits the Homebrew tap, upgrades the maintainer installation, and proves the sole `/opt/homebrew/bin/devtrim` reports the released version
- The crate now forbids `unsafe` and denies `unwrap`/`expect`/`panic`/`unreachable`/`todo`/`unimplemented`/`dbg!` and unreasoned lint suppression outside tests, and structural lints with positive-control tests now cover shell invocation and binding names alongside the deletion sink
- `CODING_STANDARDS.md` documents the review-time rules that tooling cannot check, as citable `S<n>` entries

### Fixed
- Filesystem size and artifact/toolchain evidence checks now treat metadata errors as blocking failures instead of silently reporting an absent or empty path
- Build-process liveness checks now reject every nonzero `lsof` result after `pgrep` finds candidate processes instead of treating an uncertain probe as no activity

### Security
- Docker cleanup rejects remote contexts, previews the exact absolute local Unix-socket endpoint, and pins that endpoint into the typed command authority used by apply
- Simulator cleanup previews and authorizes one validated UDID per finding, then rechecks that exact device is still unavailable before deletion
- Trash-mode directory cleanup now rejects foreign filesystem devices and nested Git repository/worktree markers before the path-based Trash call, matching the permanent-deletion preflight
- Release verification passed the full and MSRV test suites, strict Clippy, structural controls and positive controls, dependency audits, five 60-second fuzz targets, the arm64 build, PTY TUI cancellation, workflow and shell policy checks, secret scans, P3 autoreview, video build and render, and desktop/mobile layout and accessibility checks

[0.6.2]: https://github.com/mneves75/devtrim/compare/v0.6.1...v0.6.2

## [0.6.1] - 2026-08-27

### Changed
- `devtrim icloud` now reports a recursive inventory of large iCloud Drive files with logical and locally allocated sizes; it no longer infers upload progress from filesystem allocation
- Scanner and apply preflights now treat unreadable roots, traversal gaps, Git ownership/activity failures, liveness-probe failures, and nonzero owner-tool exits as blocking errors instead of silently producing partial authority

### Fixed
- Every `--json` invocation, including Clap help/version/parse failures, emits exactly one JSON document with a truthful operation, error list, and nonzero exit; empty applies report a zero summary
- Failed human and TUI applies no longer render as successful, simulator cleanup measures the previewed device directory, and Docker/simulator discovery distinguishes an absent tool from a failed one
- Journal history reverse-scans a bounded newest tail with per-line and total-byte caps, pairs legacy records across rotations, waits for active guarded applies, serializes complete synced records, refuses symlinked path components, and creates no lock file

### Security
- Permanent recursive deletion now rejects device crossings and Git repository/worktree markers at every depth, rechecks macOS file generation as part of identity, and revalidates configured `protect` aliases immediately before mutation
- The release chain runs project and dependency code only in read-only jobs, executes all five bounded fuzz targets, audits and monitors the separate fuzz lockfile, pins Actions and downloaded tools, requires the current default-branch head plus exact-commit CI/autoreview, and gives only the no-checkout publisher release-write and OIDC authority
- Production promotion verifies immutable beta provenance, signer workflow, exact asset names, and checksums before reusing the same archive without rebuilding

[0.6.1]: https://github.com/mneves75/devtrim/compare/v0.6.0...v0.6.1

## [0.6.0] - 2026-08-27

### Added
- Identity-verified, parent-anchored deletion: every finding records its target's device/inode at preview, and the sink re-verifies that identity through an open parent-directory handle (cap-std) and deletes through the same handle — a target renamed or swapped after preview is refused; Trash calls re-verify identity immediately before the path-based call, with the residual window documented
- `devtrim largest [--top N]`: read-only ranking of the biggest directories under the scan roots, with skipped-entry disclosure and the standard one-document JSON envelope
- Journal rotation: writer-owned shift-and-rename at startup only (10 MiB, keep 3), never truncation, never mid-apply; history reads rotated files so attempt/result pairs cannot split, and records are synced to disk before an apply reports success
- The release workflow explicitly ad-hoc signs and verifies the built binary before packaging; Developer ID signing and notarization are documented as a runbook pending CI credentials
- The structural deletion lint now also blocks method-call deletion primitives (`.remove_file`, `.remove_dir_all`, …) outside the sink, with positive controls

### Changed
- The landing demo video shows the current interface (Build artifacts entry, `i` for iCloud status) with a version-neutral caption

[0.6.0]: https://github.com/mneves75/devtrim/compare/v0.5.0...v0.6.0

## [0.5.0] - 2026-08-26

### Added
- `devtrim clean artifacts`: multi-ecosystem build artifacts (`target`, `.venv`, `__pycache__`, tool caches, `Pods`, `.gradle`, `.next`-family, `.build`, `.dart_tool`, `.zig-cache`, and valid `CACHEDIR.TAG` directories) in conclusively stale Git repos, each requiring ecosystem corroboration before it can even be previewed; ambiguous names such as `build`, `dist`, `vendor`, `bin`, and `obj` are deliberately never matched
- Write-ahead apply journal at `~/.local/state/devtrim/journal.jsonl` (`$XDG_STATE_HOME` honored): an attempt record before every deletion or fixed-argv command and a result record after; an unwritable journal blocks the apply, and `devtrim history [--limit N] [--json]` renders records, flagging attempts without results as interrupted
- `protect` config list: user-declared paths that the deletion sink refuses and previews filter out; entries expand `~`, must be absolute, and malformed entries fail closed
- Liveness guards: `node-modules` and `artifacts` skip and refuse repos owning the working directory of a running build or package process, and `xcode` refuses DerivedData while `xcodebuild` runs; a probe that cannot complete blocks instead of passing
- `devtrim completions <bash|zsh|fish>` and `devtrim manpage`; both refuse `--json` with the standard error envelope
- Fuzz targets for the deletion-path validator, path normalizer, Docker size parser, and config parser join the documented local release gates
- Homebrew tap `mneves75/devtrim` installs the attested release binary with generated completions and man page

### Security
- Independent pre-release reviews found and fixed a set of `protect` weaknesses before any release shipped: matching is now Unicode-normalization-insensitive (NFC config entries protect NFD on-disk names, the common macOS state), deleting an ancestor of a protected entry is refused, symlinked entries also match their resolved location, unresolved entries warn instead of failing quietly, and Trash purge previews filter protected items
- The stale-repository gate clears ambient `GIT_DIR`/`GIT_WORK_TREE`-style variables so an inherited environment cannot make an active repo read as stale, and the ambiguous artifact-name denylist matches ASCII case variants (`Build`, `DIST`) before any positive evidence is considered
- A journal write that fails after a successful deletion keeps the summary truthful while surfacing the failure in `errors` with a nonzero exit, and `history` exits nonzero when journal lines were skipped so a partial audit is never silent

### Changed
- The TUI menu adds Build artifacts and opens entries by their listed key; iCloud status moved to `i`
- Stdout writes tolerate a closed pipe, so `devtrim … | head` ends quietly instead of aborting
- The landing-page demo video shows the v0.4.0 Ratatui interface with a matching transcript and caption; the file stays video-only with no silent audio stream

[0.5.0]: https://github.com/mneves75/devtrim/compare/v0.4.0...v0.5.0

## [0.4.0] - 2026-08-25

### Added
- Original Ratatui terminal interface for interactive scan, preview, Trash-first apply, explicit permanent mode, iCloud status, and Trash purge; bare `devtrim` opens it only when stdin and stdout are terminals
- Deterministic TestBackend coverage plus manual PTY verification for navigation, non-color risk labels, small terminals, warnings, non-TTY behavior, and confirmation state

### Changed
- CLI and TUI now derive confirmation requirements from one safety policy; automation subcommands and the one-document JSON contract remain unchanged
- Ratatui 0.30.2 and Crossterm 0.29 raise the MSRV from Rust 1.85 to 1.88; Ratatui's optional layout cache stays disabled
- Release validation now installs the demo-video lockfile exactly, audits its npm graph, and runs lint, formatting, and production-build gates

### Fixed
- Removed the demo video's entirely silent audio stream, added an explicit silent-video caption linked to its transcript, and made scrollable install commands keyboard-focusable
- The TUI now blocks hidden confirmation input below 64×18, retains scanner diagnostics inside the alternate screen, and lets users scroll long outcomes to partial-apply errors
- The landing-page “Read the manual” button now opens `MANUAL.html` instead of the in-page command section
- Protected system roots, their descendants, and protected user roots reject ASCII case variants at the shared deletion boundary

### Security
- TUI apply requires an internal approval capability carrying the exact current preview and calculated danger; CLI bypass flags are rejected by the TUI and cannot pre-authorize an action
- Permanent plans require typed size confirmation, while Trash purge requires the exact `PURGE <gb>` acknowledgment before the existing Trash and deletion boundaries execute
- Replaced transitive `lru 0.12.5` after RustSec reported two soundness advisories, including potential use-after-free; Ratatui 0.30 resolves the patched `lru 0.18.2`
- `trash-empty` now previews each exact child and applies only that immutable set, so items arriving in Trash after preview are preserved
- Terminal-facing findings, errors, and outcome notes escape control and bidirectional-control characters while internal `PathBuf` deletion identity stays unchanged
- Release builds run with read-only repository and Actions permissions; a separate publisher receives only packaged inputs and alone holds release and OIDC authority
- Release retries safely replace the intermediate handoff artifact and refresh immutable state inside the publisher, so a post-publication verification retry verifies the existing release instead of attempting to recreate it
- The structural deletion rule exempts only the typed sink and test cleanup scopes, with a positive control proving a second sink is rejected
- External command findings require a private closed command authority that must match the exact serialized preview before fixed-argument execution
- Upgraded the demo-video ESLint toolchain past GHSA-xffm-g5w8-qvg7, added weekly npm Dependabot coverage, and replaced permissive inline-script CSP directives with exact SHA-256 hashes

[0.4.0]: https://github.com/mneves75/devtrim/compare/v0.3.2...v0.4.0

## [0.3.2] - 2026-08-24

### Added
- Human apply displays an AS-IS data-loss warning, and CLI help plus public documentation explain risk, backups, and manual macOS permission decisions

### Changed
- Every interactive mutation now confirms: `-y` skips normal y/N prompts and `--yolo` skips interactive prompts, while operation-specific acknowledgments such as `trash-empty --confirm=<gb>` remain mandatory

### Fixed
- Manual layout keeps the table of contents and document content in their intended desktop columns, with keyboard access to scrollable examples
- Production promotion selects beta tags without generating invalid jq regex escapes
- Actionable-size and apply-summary aggregation saturate instead of wrapping, so extreme totals cannot lower danger or misstate results
- iCloud allocated-size inspection now fails on metadata or arithmetic errors instead of silently presenting a partial value
- The shared protected-system boundary explicitly rejects `/bin`, `/sbin`, and `/var` aliases

[0.3.2]: https://github.com/mneves75/devtrim/releases/tag/v0.3.2

## [0.3.1] - 2026-08-23

### Fixed
- Directory sizing now fails closed on traversal, metadata, or overflow errors and never follows a symlink supplied as the scan root
- Configuration files reject unknown fields instead of silently accepting misspelled safety settings
- Docker disk-usage parsing recognizes documented petabyte and exabyte units while rejecting missing, unsupported, negative, non-finite, and out-of-range values

### Security
- Release preparation now actually executes Gitleaks and TruffleHog before tagging, matching the documented release contract
- The privileged local preflight queries GitHub's immutable-release setting directly, while hosted jobs verify the published release is actually immutable
- Actionable scans refuse to build a cleanup plan when their logical-byte measurement cannot be completed truthfully

[0.3.1]: https://github.com/mneves75/devtrim/releases/tag/v0.3.1

## [0.3.0] - 2026-08-23

### Added
- A private `VerifiedTarget` capability at the shared filesystem deletion sink; unvalidated paths cannot reach physical removal
- Deterministic property tests for protected roots, managed `~/Library` exceptions, cleaned parent aliases, and non-UTF-8 target identity
- A positive-control ast-grep rule, pre-commit hook, CI step, and release gate that reject direct filesystem deletion outside the shared sink

### Changed
- Findings retain exact internal `PathBuf` identity while serialized paths remain presentation-only
- Apply derives Trash versus permanent deletion from each previewed typed action and reports successful work before a later failure
- npm and Homebrew owner-reported cache paths are constrained to exact program namespaces and revalidated immediately before apply
- Release validation now fails when the Rust 1.85 MSRV gate cannot execute instead of silently skipping it
- Release automation now builds immutable `-betaN` prereleases on hosted CI with signed provenance, then promotes the exact verified beta artifact to production without rebuilding

### Security
- Fixed an owner-cache protected-path bypass that accepted arbitrary hidden directories and parent-component escapes under the user home
- Added forged-action tests proving Docker volumes and simulator erase-all cannot cross command allowlists
- Release requires `cargo audit`, Gitleaks, TruffleHog, full tests, strict Clippy, MSRV tests, an arm64 build, and independent autoreview on the exact commit before publication
- The remaining pathname TOCTOU limitation is explicit: validation does not hold a directory descriptor across deletion and assumes no hostile concurrent local mutation

[0.3.0]: https://github.com/mneves75/devtrim/releases/tag/v0.3.0

## [0.2.1] - 2026-08-23

### Added
- macOS CI for formatting, strict Clippy, tests, Rust 1.85 MSRV coverage, dependency audit, and explicit arm64 release builds
- Regression coverage for physical-path validation, symlinked Trash, fail-closed Git/toolchain checks, immutable `node_modules` plans, JSON output, and owner-command failures
- `SECURITY.md` threat model, safety design, reporting guidance, and known limitations
- Exact Rust 1.98.0 release-toolchain pin and weekly Cargo/GitHub Actions Dependabot checks

### Changed
- `--json` now emits one response envelope per invocation, including empty, partial, and failed results
- Unprovable state degrades to a skipped target with a warning instead of failing an entire scan: repos whose Git activity cannot be read, and hosts where `simctl` is unavailable
- `node-modules` applies exact previewed paths, prunes dependency/`.git` traversal, and requires conclusive Git activity
- `leftovers` is report-only because worktree or mission staleness cannot be proven safely
- Simulator cleanup reads `simctl` JSON and only deletes unavailable devices; `--yolo` bypasses confirmation but never adds erase-all
- Swift toolchains are eligible only when `swift-latest` and every other symlink reference resolve safely
- Release packaging now requires successful exact-commit CI/MSRV evidence, runs local gates, builds/verifies arm64 explicitly, starts from clean artifacts, and includes the full Apache-2.0 license

### Fixed
- Protected paths could be permanently deleted through a symlinked ancestor
- `trash-empty` mutated without `--apply`, accepted the wrong documented flag spelling, and bypassed the shared deletion owner
- Config `~/` roots were not expanded or physically resolved, while malformed config silently fell back to another root
- Failed Docker/cache/simulator actions could return success or inflate reclaimed-byte summaries
- Human apply prompts appeared before the reviewed findings, and JSON TTY prompts polluted stdout
- `--shred` previews and notes incorrectly described permanent deletion as recoverable Trash
- Aggregate actionable size did not consistently drive danger escalation
- `active_days = 0` in config silently disabled the active-repo guard; it now clamps to 1
- The Docker finding described `image prune -a` as "unused images" without stating that untagged local builds are removed too
- `scripts/release.sh` compared against a possibly stale remote ref and ignored a failed remote-tag query

### Security
- Deletion now validates both literal policy and the canonical existing parent immediately before mutation; resolution is deny-only
- Unknown ownership/activity and failed external commands fail closed
- Cache roots reported by `npm` and `brew` are user-controlled configuration and are now refused unless they resolve inside a home cache location
- Git activity checks neutralize repository-controlled configuration (`core.fsmonitor`, `core.hooksPath`) when inspecting untrusted clones
- `cargo audit`, Gitleaks, and TruffleHog found no dependency vulnerabilities or committed secrets before release

[0.2.1]: https://github.com/mneves75/devtrim/releases/tag/v0.2.1

## [0.2.0] - 2026-08-22

### Added
- Landing page + GitHub Pages site (mneves75.github.io/devtrim) with lazy-loaded demo video and visual transcript
- 12s product demo video rendered with Remotion (`media/demo.mp4`)
- `scripts/release.sh` — reproducible release automation: clean-tree and existing-tag guards, locked release build, zip + SHA256SUMS, tag, GitHub release from changelog
- PRODUCT.md / DESIGN.md design-system docs; AGENTS.md + CLAUDE.md agent guidance

### Fixed
- Release-notes extraction used an awk character class, truncating published release bodies (v0.1.0 repaired retroactively)
- Landing stylesheet blocked by its own CSP (`style-src` now allows self)
- Reveal-on-scroll left content invisible without JavaScript (`noscript` fallback)

### Security
- CSP (`default-src 'none'`, `style-src 'self' 'unsafe-inline'`) + `referrer: no-referrer` on all shipped HTML

[0.2.0]: https://github.com/mneves75/devtrim/releases/tag/v0.2.0

## [0.1.0] - 2026-08-21

First public release.

### Added
- `scan` — read-only report across all reclaimable categories, `--json` output
- `clean caches` — HuggingFace / npm / Homebrew / uv / node caches (Trash-first)
- `clean node-modules` — stale-repo sweep with active-repo guard (commit recency)
- `clean simulators` — delete unavailable devices; erase-all behind `--yolo`
- `clean xcode` — DeviceSupport + DerivedData (Archives exempt by design)
- `clean docker` — unused images + build cache; volumes never touched
- `clean toolchains` — old swift.org toolchains, `swift-latest` preserved
- `clean leftovers` — agent scratch dirs and `.supergoal` artifacts
- `icloud` — upload status for large queued iCloud Drive files
- `trash-empty --confirm=<gb>` — typed-size acknowledgment gate
- Safety core: danger scoring 1–10 with size escalation, protected-path denylist,
  Trash-first deletion, non-TTY refusal, preview-by-default (`--apply` to mutate)
- 6 unit tests covering scanner helper logic
- MANUAL.html: single-file interactive manual (dark/light, CSP hardened)
- Landing page (GitHub Pages) + release automation script

### Security
- CSP (`default-src 'none'`) and `referrer: no-referrer` on shipped HTML
- No network access at runtime except package-manager subprocesses
[0.1.0]: https://github.com/mneves75/devtrim/releases/tag/v0.1.0
