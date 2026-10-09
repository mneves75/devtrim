# Changelog

All notable changes to devtrim. Format follows Keep a Changelog; versioning is semver.

## [0.10.10] - Unreleased

### Fixed
- External commands run under a wall-clock limit, so one stuck program no longer hangs a scan. Git, `simctl`, `docker`, `npm` and `brew` queries get two minutes, the `pgrep` and `lsof` liveness probes three (a system-wide `lsof` took 16 s at load average 300), and the typed Docker and simulator commands an apply runs fifteen. Past its limit the child is killed and reaped and the repository, category or safety check that asked fails with an error naming the command and the limit; a timed-out liveness probe refuses like any failed one and never reads as "nothing running". Other categories still complete. The `no-unbounded-subprocess` ast-grep rule rejects an unbounded `output`, `status` or `spawn` in `src/`

## [0.10.9] - 2026-10-09

This release also includes the unpublished 0.10.8 feature evals and safeguards:
home-repository ownership, per-target activity rechecks, mount-point Trash
refusal and UUID-only simulator cleanup, alongside the fixes below.

### Fixed
- Build-process liveness refuses decoded `lsof` working-directory names that are not absolute, including Darwin error diagnostics, at preview and apply
- `trash-empty --only-devtrim` reads all retained journal generations independently of the history display limit; malformed, unreadable, oversized records or snapshots above the explicit 40.25 MiB resource ceiling refuse the entire narrowed purge
- Localized `scan_error` findings preserve the TUI's failure exit status through every results route and later successful scans
- `status` reports a failed or malformed Data-volume metric as unavailable; root-filesystem fallback requires a positively absent Data layout, and an unreadable or non-directory layout refuses
- Explicit and configured project scan roots at or below ASCII-case variants of `.git`, or below `node_modules`, cannot offer nested dependency installs or build artifacts; a root exactly at a top-level install remains eligible
- The opt-in page-load measurement waits for observed, settled paint after image/font readiness instead of sampling too early; failed Chrome starts return a measurement error and clean their disposable profiles. Timing budgets remain unchanged
- Delayed-paint controls require the intended observed LCP element ID, so a slow initial candidate cannot satisfy their timing checks; browser-free stale-report regressions run in local checks, CI and read-only release verification

### Added
- Real-binary regressions and positive controls for liveness drift, retained Trash ownership beyond 1,000 entries, malformed old records, journal resource limits, Data-volume failures and excluded root namespaces; PTY controls cover localized scan failures and successful exit
- Thirteen named `review013/` planted mutations cover the changed liveness, complete-history, TUI, disk-layout and root-namespace branches; each requires a compiling mutant and its tagged assertion to fail

## [0.10.8] - 2026-10-08

Not published separately; included in 0.10.9.

Feature evals run the real binary against planted fixtures. Named mutation
controls prove specific assertions can fail; they do not prove every assertion
or native integration. The evals and a fresh-context security audit found four
gaps addressed here.

### Added
- Report content evals cover ranked sizes and limits, exact app identifiers, iCloud allocation, rotated history, analyze partial results, status memory and unavailable metrics, scan summaries, completions, manpage and maintenance previews. Safety evals cover a real held uv lock, caches and agents continuing past a refusal, symlinked journals, build-process liveness, and explicit consent for unattended command apply
- Feature evals for every command (`tests/evals.rs`, `tests/eval_system.rs`, `tests/eval_reports.rs`, `tests/eval_safety.rs`). Each plants what the feature claims to act on beside one near miss per documented exclusion. A preview must leave the fixture byte for byte unchanged; an apply must remove exactly the previewed targets, journal each one write-ahead, and keep every near miss. Further evals cover a target that appears or is swapped between consent and apply (driven inside one terminal session), an unwritable journal, a hostile `.git/config`, Xcode running, the two retention windows, and the flags each command rejects. Named planted mutants prove the assertions they select can fail; `planted-violations.py` now rebuilds the binary and selects integration tests, and accepts a name prefix while a case is developed
- `scripts/tests/native-docker.sh` proves `clean docker` against a real, disposable Docker engine run by Apple's `container`: every unused image and all build cache go, and a volume and its data stay. A binary that skips the apply fails it
- `scripts/perf/pageload.mjs` measures cold loads of the website and the manual in headless Chrome and fails closed on a broken image, a CSP violation or a page that paints nothing; `scripts/tests/pageload-controls.py` proves it

### Fixed
- The demo build locks `source-map-js` to patched 1.2.2, addressing GHSA-68fv-2mgg-jv7q without changing the pinned Remotion dependencies
- Test subprocesses clear inherited configuration and Git authority; the native Docker check binds its disposable configuration, clears ambient endpoint/context selectors, and verifies that the selected context names its own socket
- Eval fixtures stay inside their compiled checkout, so timed-out mutation runs remove their fixtures with the disposable source copy
- Mutation checks refresh bindings for the apply rechecks and validate every binding before compiling. They allow 120 seconds per fixture, including unit fixtures that run real Git, within a one-hour total budget; a timeout or compile failure still fails the gate
- A home folder kept as a Git repository (dotfiles at `~`) no longer decides for the projects under it that have no repository of their own. Its last commit made a project edited that morning look stale, so `purge`, `clean node-modules` and `clean artifacts` offered its `node_modules`, `target` and `.venv`. Such a project is now not judged and the human preview names the skip
- `clean node-modules` and `clean artifacts` judge each finding again just before removing it, not only before the first removal, so a repository that becomes active while earlier findings are removed keeps its output
- Moving a directory to the Trash now refuses one on another device than its parent (a mount point), as permanent deletion already did
- A simulator is deleted only by a UUID; `simctl`'s set words (`all`, `unavailable`, `booted`) are refused
- The manual said installers wait for the project window; they follow `retain_days`

### Changed
- Native Docker checks use a unique container and socket per run, preserve the container owner's HOME, discover buildx before starting the VM, require a positive build-cache control, and remove their disposable HOME on exit
- The website's hero image is a 34 KB WebP instead of a 1.3 MB PNG; the landing page now transfers 55 KB

## [0.10.7] - 2026-10-04

Using the TUI showed two puzzles: `A` seemed to do nothing, and the menu put a
word such as `PREVIEW` beside each operation without saying what it meant.

### Changed
- The TUI menu explains the highlighted operation's badge: `READ-ONLY` reports and never changes anything, `PREVIEW` changes nothing until you select items, press `a` and approve, and `PERMANENT` deletes for good after a typed size acknowledgment
- The results footer names what the next `A` does: "A none" while nothing is left out, "A all" after, and each press says what it did. `A` still toggles every finding between selected and left out; everything starts selected, so its first press leaves everything out. Space is labelled "Space pick" so the footer fits whole at the minimum 64×18, and the opening status line drops its final period for the same reason. Three older status lines that the minimum width cut off are shorter now: permanent mode ("Permanent mode: every action is SHRED; danger is critical."), an apply with nothing selected, which no longer offers to add a row Space cannot add, and an apply that finished with errors

### Fixed
- In "Scan everything", which only reports, Space, `A` and `s` did nothing and said nothing, and `a` said "This result has no actionable findings" beside a total of actionable space. Each now names the category view that can act on the highlighted finding — "Read-only scan. To act on it: b, then 2 (caches)." — or says the item is a report with nothing to apply. In every other view, a key that cannot act on the highlighted row or on the plan (Space on a report row, `s` with no Trash item selected, any action key in iCloud or Agent leftovers) says why instead of doing nothing
- The `agents` tests for Codex releases no longer depend on every process the host runs. Ten tests read the system-wide `lsof -d txt` probe, and a busy host occasionally failed several of them at once (once in about 175 local runs). They now receive the probe's answer: no mapping, a mapping inside the release, or a failed probe, which is newly tested to refuse both preview and apply. The planted-violation gate runs only these deterministic tests. One test still runs a real program from a fixture release through the real probe and the production entry points; it retries only the probe's own refusal, at most three times in fresh fixtures, and fails with those reasons if every attempt refuses. Production behavior is unchanged

## [0.10.6] - 2026-10-03

The open items from 0.10.5, all approved by the owner, plus what using 0.10.5
on this Mac showed: the Homebrew cache was stranded whole by one Git clone, the
Trash could only be emptied whole although other sessions keep items there, and
an idle Xcode window blocked every DerivedData folder.

### Added
- `trash-empty --only-devtrim` purges only what devtrim itself moved to the Trash. Each move to the Trash now journals the item's device, inode and birth time, which survive the move while Finder renames the item, and only a Trash item matching a successful move is offered; everyone else's items stay and are counted in a note. The journal field is additive: older records parse unchanged and never match. A history that cannot be read whole refuses the purge and offers nothing
- `artifacts` (and so `purge`) recognizes three more build outputs Mole purges, each only beside its owner's file: Android's native build folder `.cxx` beside `build.gradle` or `build.gradle.kts` (the Android Gradle Plugin's default `buildStagingDirectory`), `.terragrunt-cache` beside `terragrunt.hcl` (Terragrunt: "you can safely delete this folder any time") unless it holds Terraform state, and Nuxt's `.output` beside a `nuxt.config.*` (re-created by every `nuxt build`)

- Any build-output tree holding Terraform state (`*.tfstate`, `*.tfstate.backup`, in any ASCII case) is never offered, and apply refuses it: Terraform's local backend writes `terraform.tfstate` into the working directory Terragrunt runs it in, inside `.terragrunt-cache`, and that file is the only record of the infrastructure it manages

### Changed
- While Xcode, `xcodebuild` or its build services run, DerivedData is judged folder by folder instead of skipped whole: a folder whose newest file changed within `active_days`, or that holds a file an Xcode process has open, stays, and every other folder is offered; apply judges each again. A folder holding no file yet is judged by its own modification time. Xcode's open files come from `lsof -F tDn`, which must exit 0 and give every file that is not a socket, pipe or queue a path and its device — Apple's `lsof` prints `no more information` for a file it could not resolve — or DerivedData is refused. Only files on the folder's own device are judged against it; among those, lsof's fallback for a file whose path the kernel could not give — the mount followed by that volume's mount source, such as `/System/Volumes/Data (/dev/disk3s5)` — leaves the folder unjudged. An open file is matched to its folder by spelling, by the folder's real path, and by device and inode, since the kernel may name it through a link or the `/System/Volumes/Data` firmlink. A failed probe now blocks DerivedData alone, with an error entry, instead of the whole `xcode` category; a folder whose age cannot be read blocks only itself
- Homebrew's standard cache (`~/Library/Caches/Homebrew`) holding a Git clone Homebrew keeps for a Git-sourced formula (`<name>--git`) is offered as its other direct children — `downloads`, `api`, `Cask` and the like — instead of whole, which the deletion sink always refused (1.1 GB stranded on this Mac); the clones stay, and apply reasserts that shape
- Human totals count error entries apart from findings: "17.9 GB actionable across 31 finding(s) and 2 error(s)", and per category "4 finding(s), 1 error(s)". JSON is unchanged
- Dependencies: `rustix` 1.1.5, `trash` 5.2.9 (macOS code unchanged), `clap` 4.6.7, `clap_complete` 4.6.11 and `toml` 1.1.6 in both lockfiles; the demo-video tooling moves to Remotion 4.0.529 for all its packages, React 19.2.8, `@types/react` 19.2.18 and Prettier 3.9.6, within the 7-day release-age cooldown

### Fixed
- A cache root that `brew --cache` or `npm config get cache` reported through a linked home folder was skipped as "outside a home cache location": devtrim resolves `$HOME` to its real path while the owner answers in the spelling it was given. The reported root is now resolved the same way before it is judged

## [0.10.5] - 2026-10-03

Found by running `devtrim scan` on the owner's Mac with 27 GiB free: `purge`
offered nothing at all. One root-owned `.fseventsd` that a mounted disk image
had left in a project's scratch folder emptied `node-modules` and `artifacts`.
Routed around it, a repository created with `git init` the day before emptied
them again, and the preview that finally appeared offered `node_modules`
folders inside archived iOS app bundles.

### Changed
- A failure now blocks only what it touches. A repository whose Git query, tracked-file check, tree search or size measurement fails offers nothing — not even a finding judged before the failure — and appears as an error finding naming it, so the run still exits nonzero; every other repository in `node-modules`, `artifacts` and `purge` is judged as usual. Until now one such repository emptied the whole category. A failed build-process liveness probe still blocks its whole category
- `node-modules` offers a `node_modules` only when a `package.json` that is a regular file sits beside it, the manifest it was installed from and the only thing that lets an install recreate it, and apply refuses one without it. React Native copies package images into `<App>.app/assets/node_modules`, so every archived (`.xcarchive`) or unpacked (`Payload/`) iOS build holds one, where removing it breaks the bundle's signature; corepack's pnpm ships its dependencies in `dist/node_modules`, which no install restores

### Fixed
- A folder the project walk cannot read — such as a root-owned `.fseventsd`, where even probing for `CACHEDIR.TAG` is refused — no longer fails `node-modules`, `artifacts` and `purge` whole. It is reported as an error naming the folder, so the run still exits nonzero, nothing in it is offered, and everything else is judged as usual; a scan root itself that cannot be read still fails
- A new repository (`git init` before the first commit) made the activity query fail and emptied the category. It is now left out and named in a skip note, like a worktree whose repository is gone. Only that state qualifies: no commit object anywhere in the repository (staged files are only blobs), listed by Git without a complaint, with HEAD naming a branch, `git refs verify` finding the reference storage sound, and every reference resolving. A HEAD naming a missing commit, damaged reference storage, a dangling symbolic branch, and an orphan checkout beside other branches keep their error

### Security
- The demo-video tooling's lockfile moves `fast-uri` from 3.1.6 to 3.1.8 (GHSA-58mr-gqgx-xq4g, GHSA-qw65-cvwx-89v3, GHSA-hrr3-gc8f-f4qj, all high) and `brace-expansion` from 5.0.9 to 5.0.12, so the `npm audit --audit-level=low` gate in CI and the release workflow passes again. Neither ships in the devtrim binary

## [0.10.4] - 2026-09-28

Asked after 0.10.3: "why 10 days? can be less?" Measured on the owner's `~/dev`,
10 or 7 days offered 1.2 GB, 3 days 4.5 GB and 1 day 36 GB, and answering the
question showed that one setting was deciding two different things.

### Added
- `retain_days` in `~/.config/devtrim.toml` decides when files nothing can regenerate — agent session history and installer archives — are offered. Until now `active_days` decided that too, so lowering it to free build output sooner also offered history and installers that much sooner. Unset, `retain_days` is `active_days` or 30, whichever is longer, so a short project window never shortens how long those are kept; a config that set only `active_days` below 30 now keeps history and installers longer than before, never shorter
- `~/.codex/worktrees`, where Codex creates its managed worktrees, is one of the default project folders, so the dependencies an agent installed in a worktree it left behind are found; the worktree itself is a repository devtrim never removes. `~/.claude/worktrees`, which Mole also lists, is not: Claude Code creates its worktrees inside the repository, under `.claude/worktrees/`, which the repository's own root already covers

### Changed
- The "repos active in the last N days" skip notes name where that window is set, `active_days` in `~/.config/devtrim.toml`, and the README and manual explain what a shorter window costs: a reinstall or rebuild when you return to a project, and it cannot see edits you have not committed

### Fixed
- A linked worktree whose repository was deleted made every Git query fail, and one such worktree emptied the whole `node-modules`, `artifacts` and `purge` preview. It is now left out and named in a skip note; nothing in it is offered, because its activity cannot be read

## [0.10.3] - 2026-09-27

Found by running `purge --apply` for real. It trashed 43 items, all under
`~/dev`, 40 of them one-byte fixtures devtrim's own tests had left behind, and
nothing said that `~/dev` was the only place it had looked.

### Changed
- Without `--root` or configured `roots`, the project commands (`purge`, `clean node-modules`, `clean artifacts`, `clean leftovers`, `largest`, and the project half of `scan`) now look in every conventional project folder that exists — `~/dev`, `~/Developer`, `~/Development`, `~/Projects`, `~/Code`, `~/GitHub`, `~/Repos`, `~/Workspace`, and `~/www` — instead of `~/dev` alone. These are Mole V1.56.0's `mo purge` defaults plus the folder Finder marks as `Developer`. They still never walk the whole disk or home folder: Desktop, Documents and Downloads sit behind macOS privacy prompts, cloud storage holds synced and dataless files, and outside a project folder there is no Git owner to judge staleness by. A root only says where to look, so no finding gains authority. `--root` and `roots` replace the defaults as before
- Human previews of those commands open with `info scan roots:`, naming the roots, whether they are the defaults, the config, or `--root`, and how to change them. When no default folder exists, it says so and lists the folders it looked for. JSON output is unchanged
- Skip notes say what holds findings back: "in repos active in the last 30 days" instead of "in active repos", and the repositories where a build process is running, three by name and a count of the rest

### Fixed
- A stale Next.js project built with `output: 'standalone'` was offered twice in one `purge` plan: its `.next` and the `node_modules` Next.js copies into `.next/standalone`, so the plan counted those bytes twice. A `node_modules` inside a repository's own build output is now part of that output, never a finding of its own, and apply refuses one. The `node_modules` walk no longer enters build output below a scan root, as the `artifacts` walk already did, which also keeps a stale repository left inside a Cargo `target` out of the plan and skips the largest trees on the disk
- A default project folder that cannot be read, or that links to the home folder or above it, is skipped with a warning — a link like `~/www -> ~`, or to the same folder through the data-volume path `/System/Volumes/Data/Users/<name>`, would otherwise have made the whole home a default root; the folder is recognized by the volume's identity, not only by spelling — and one that links into another default folder is scanned once through it
- `CACHEDIR.TAG` is read in one open that follows no link and never blocks, as uv's tag already was, rather than checked and then reopened; every directory the project walks visit is probed, so a FIFO swapped in between the two could stall a scan
- A devtrim test that failed left its fixture repository under `target/`, where a later `purge` of the checkout found it as a stale project from 2000. Fixtures are now removed while a failing test unwinds

## [0.10.2] - 2026-09-26

A second review of 0.10.0 and 0.10.1, checked again against Mole V1.56.0's
source (`239c90d`). Each fix below has a test that fails without it.

### Added
- `clean xcode` cleans a DerivedData folder holding a project's resolved Swift packages. Each package there is a Git clone, and the nested-repository refusal blocked the whole folder, so a project using any Swift package could never have its DerivedData cleaned. Such a folder is now offered directory by directory — `Build`, `Index.noindex` and the like — while `SourcePackages`, which holds the clones, stays; apply refuses `SourcePackages` in any ASCII case. A folder that is itself a Git repository is never split. devtrim still never deletes a Git repository
- `artifacts` and `node-modules` never offer a directory holding any file its repository tracks, and `artifacts` never offers a tree holding a Git repository anywhere below its root or an entry named `*-keypair.json`, ported from Mole V1.56.0 (`lib/clean/project.sh`). A SwiftPM `.build` keeps its dependencies as Git clones in `checkouts`; such a tree was offered, and its apply was refused. A tracked file makes the directory part of the repository — CocoaPods recommends committing `Pods` — and `cargo build-sbf`, which `anchor build` runs, writes a Solana program's keypair into `target/deploy` only when none exists (cargo-build-sbf v4.4.0 `src/post_processing.rs:167-171,188,201`), so a rebuild after removal mints a different program address. Tracked files are recognized by the directory the volume resolves each tracked path to, not by spelling, so a directory renamed only in case, in Unicode case, or to an alias the volume folds to (`STRASSE` for `Straße`) is still recognized. These checks run again at apply. Human output counts the skipped directories
- In the interactive view, `Enter` shows the highlighted finding in full in a view of its own that scrolls, for when the detail pane cannot hold it. Nothing typed there changes the plan, and `Esc` returns to the results unchanged. The pane's overflow line now says `Enter` shows the rest instead of asking for a larger terminal

### Changed
- A failed Git query now names Git's own reason, for example a repository owned by another user, instead of only saying the check failed

### Fixed
- The tolerance for uv's empty `.git` applied to any deletion root carrying a `CACHEDIR.TAG`, but only `clean caches` takes uv's lock. A uv cache kept inside a repository (a local `UV_CACHE_DIR`), reached through `artifacts`, could be removed under a running uv. The exception now needs uv's lock on that exact root, which only `clean caches` holds, or the cache must already be in the Trash, where `trash-empty` purges it and no uv uses it; everywhere else the marker still refuses
- `artifacts` and `node-modules` apply stopped at the first finding the deletion sink refused, leaving every later finding untouched. Each refusal now costs only its own finding and is still reported, as in `caches`, `agents`, `xcode` and `trash-empty`
- A permanent deletion refused after its quarantine rename named devtrim's internal `.devtrim-quarantine-…` entry. The error now names the finding's own path, as the journal does
- At the minimum 64×18 the key reference (`?`) cut off its last lines, the quit binding among them, with no sign anything was missing. It now fits whole at that size
- A results row that keeps the end of a long path could split a character the terminal draws as one — a flag, or an accent macOS stores as a separate mark — and show half of it after the ellipsis. Rows now keep whole characters

## [0.10.1] - 2026-09-26

Found by using 0.10.0 to free space on the machine it was released from.

### Fixed
- `xcode` apply continues past a refused finding, as `caches` and `agents` already did. A DerivedData folder holding SwiftPM checkouts carries nested Git markers that the deletion sink always refuses, and until now that one refusal stopped every build tree after it in the plan. Each refusal is still recorded, so the run reports nonzero
- The Playwright browser cache no longer claims it is "regenerated automatically on next use": its browsers come back only through `npx playwright install`, and the finding now names that command. Every other cache keeps the automatic note
- In the TUI detail pane, a space that would land in a row's last cell now opens the next row, where its indent shows. Before, it read as padding, so two names in a path looked joined
- After an apply that ends with errors but changed something, the TUI no longer says it "stopped after an error". `caches`, `agents`, `trash-empty` and now `xcode` continue past a refused item, so that was false. It now says the apply finished with errors, and the summary lists what changed and what failed

## [0.10.0] - 2026-09-26

Found by using devtrim to free space on a Mac at 97% full. Its scan listed 830
findings, the uv cache failed on every run, and a 25 GB apply reported space
"reclaimed" while free space did not move. The purge and selection work
follows a study of Mole V1.56.0 (`239c90d`): its ergonomics are ported, its
matching is not.

### Added
- `devtrim purge` puts stale `node_modules` and corroborated build artifacts in one plan, ordered by project with the largest first and a header per project. It adds no authority: every finding comes from the `node-modules` or `artifacts` scanner, with that category's staleness and build-liveness gates, and is applied by the same category, which reasserts its exact target shape, so a finding routed to the wrong one is refused. Ambiguous names such as `build`, `dist` or `coverage` stay unmatched; Mole V1.56.0's `mo purge` matches those names without requiring a manifest or Git (`lib/clean/purge_shared.sh`, `lib/clean/project.sh`) and deletes permanently, and that is the part deliberately not ported
- The interactive view can leave items out of a plan. Space leaves out the highlighted item or adds it back, `A` selects every item or none, and the confirmation is recomputed for the selected items, so leaving out a critical item can lower a typed-size confirmation to y/N. The confirmation states how many findings it covers and how many were left out, and the outcome repeats the count. Selection only narrows: the approved plan is always a subset of the displayed preview, and changing the selection invalidates an earlier approval. The interactive view also gains a Project purge entry (`p`)
- Findings from `node-modules` and `artifacts` carry the repository that owns them, as a `project` field in JSON and a header per project in human output

### Changed
- `scan` leads with one line per category giving its size and the command that acts on it, largest first, then lists the five largest findings of any category with more than eight. `scan --all` lists every finding; `--json` is unchanged and always complete. On the machine that prompted this, the report went from about 1,660 lines to 53
- The interactive results screen shows one line per finding, with its selection mark, danger, size and action, and a detail pane that grows to show the highlighted finding whole: label, action, whether it is left out, path, note and project. It is the one place a long path appears in full, so a terminal too short to hold it says how many lines it hides instead of cutting them
- An apply that moves items to Trash no longer calls those bytes reclaimed. The summary says how much moved to Trash and that it is freed once the Trash is emptied (`devtrim trash-empty`); JSON summaries gain `bytes_trashed_estimate`, the part of `bytes_freed_estimate` that is still on disk
- Activity dates are compared in UTC, the clock the cutoff already used, and shown as `repo last active <date> UTC`

### Fixed
- The uv cache could never be removed. uv writes an empty `.git` into its `sdists-v<N>` bucket every time it initialises a cache (uv 0.9.24 `crates/uv-cache/src/lib.rs:439-449`), so builds there never read Git metadata from an enclosing repository, and the nested-repository refusal blocked the whole cache on every run, in `clean caches` and again in `trash-empty`. Git rejects an empty gitfile as an invalid format, so that file marks no repository. The sink now tolerates it in exactly that shape: an empty regular file spelled `.git`, in a bucket named `sdists-v<digits>` directly under a deletion root carrying a valid `CACHEDIR.TAG`. A worktree pointer, a case variant, a directory, a symlink, an untagged root, a marker at another depth and one in another bucket all still refuse
- Removing the uv cache now honours uv's own lock. Every running uv holds a shared `flock` on `<cache>/.lock` and `uv cache clean` takes it exclusively; devtrim takes it the same way without waiting, so the cache is refused while any uv process uses it instead of being moved out from under a running `uv sync`. A cache reached through a symlink or symlinked ancestor is refused before the lock file is created
- `trash-empty` stopped at the first item it refused, so one trashed project that still held its repository kept every item after it. It now continues and records each refusal, reporting a nonzero status
- Git activity dates were read in each entry's recorded time zone while the cutoff counted UTC days, so west of Greenwich an evening's activity landed a day early, and three activity-probe tests failed every evening in UTC−3

## [0.9.8] - 2026-09-23

### Fixed
- Codex release cleanup skips a package when the `lsof` snapshot reports a process executing a file inside it. Apply probes again for each release, including after earlier findings have moved. An unavailable process probe or an executable mapping without an absolute name refuses release cleanup. This reduces interruption risk for an older agent, though the snapshot cannot see every other user's process or one started after the probe; `0.9.7` did not have this check
- The process probe explicitly requests both descriptor and name fields, as required by `lsof` versions after 4.93.2; without the descriptor field, its strict parser refused every release scan
- Vendor package checks now open files non-blocking and close-on-exec, while continuing to refuse symlinks. A FIFO at a required file name previously hung scanning; files that are not regular are refused
- Release version components reject leading zeros, so a name such as `0.0156.0` cannot pass as `0.156.0`
- The installer lock's file identity is rechecked after locking, refusing a lock file replaced during acquisition

### Changed
- The Codex release deletion authority now carries required owner evidence through `DeletionEntry`, including the limits of removing a package that might still be in use

## [0.9.7] - 2026-09-23

A review of 0.9.5 and 0.9.6 on two axes (Standards and Spec), plus a security
pass over the same diff. Each fix below has a test that fails without it.

### Added
- `clean agents` offers older Codex standalone packages after checking the installer lock, current package, required voice manifest, executables, audited resource names, and manifest SHA-256 digests. Current, newer, same-version, staged, and unknown packages remain; an unverifiable lock or current package produces a read-only refusal and nonzero CLI status while cache/history cleanup stays available; invalid older candidates are omitted and preserved. Apply rechecks eligibility. Trash remains the default and does not free physical bytes until purged

### Fixed
- The working-simulator disclosure summed simctl's sizes with saturating arithmetic, so a total that overflowed was shown capped at 18.4 EB instead of being refused. It now uses checked addition and drops only the disclosure with a diagnostic, as it already did for a missing size
- No test covered a simulator plan that mixed a device to delete with the report-only disclosure, and apply stops at its first error, so removing the skip that keeps the disclosure out of the command path passed every test. A new test gives the forgery its own refusal message and fails when the skip is removed

### Changed
- The test proving `pgrep -a` matches devtrim's own ancestors no longer runs a shell script; `/usr/bin/time` under a unique name is the ancestor. `CODING_STANDARDS.md` S12 lists it with the other test-only variable program
- The real `lsof` race test calls the production probe instead of rebuilding its arguments, so the two cannot drift apart
- `scripts/tests/planted-violations.py` proves the Xcode apply refusal of a non-directory target
- SECURITY.md states the limit of the `lsof` recheck: it compares PIDs, so a reported build whose PID a new build reuses within the probe keeps the first directory. That window is no wider than a build starting just after a clean probe

## [0.9.6] - 2026-09-15

### Fixed
- Build-process liveness refused whole `artifacts` and `node-modules` runs whenever a build tool exited between its two probes. `lsof -p` exits 1 when any listed process is gone, and devtrim treated every nonzero exit as an unverifiable probe; on a busy machine that was 2 of 5 consecutive probes, and a real `clean artifacts --apply` refused for exactly this reason before a retry succeeded. The probe now records which processes `lsof` reported and accepts exit 1 only when every process it did not report is also absent from a fresh `pgrep`. A process still running whose working directory `lsof` could not read — another user's, for instance — refuses as before, and so does PID reuse by a build process, a failed recheck, any other exit status, and output that names a directory before its process, a process without a directory, or a process twice. A build process that appears in the recheck — a build moving to its next step — has its directory looked up once, and any gap in that answer refuses
- Liveness `pgrep` left out devtrim's own ancestors, which it does by default, so a `make` or `npm run` that invoked devtrim inside a build never counted as that repository's build process. Both probes now pass `-a`

## [0.9.5] - 2026-09-15

Found by using devtrim on a Mac that had run down to 1.7 GB free, and comparing
what it offered with what actually held the space.

### Fixed
- `clean xcode` offered Finder's `iOS DeviceSupport/.DS_Store` as a symbol cache: every direct child was listed whatever it was, so a file or a symlink beside the symbol directories borrowed the category's authority. The scan now lists only real directories, and apply refuses any other target shape on its own
- The Docker build-cache estimate used `docker system df`'s RECLAIMABLE column while apply runs `builder prune -a`. The daemon leaves records shared with the image store out of RECLAIMABLE, and `-a` prunes them anyway: one OrbStack machine previewed 4.902 GB and the prune reported 17.37 GB. With no record in use the estimate is now SIZE, labelled "up to" because bytes an image still references stay on disk; with records in use it stays RECLAIMABLE, labelled "at least". Only the Build Cache row reads the new ACTIVE column, so an odd value on a row devtrim ignores cannot fail the category. The build-cache note also stopped describing image pruning
- Notes about the Docker VM disk image said pruning does not shrink it until the VM stops, and told you to restart OrbStack to trigger TRIM. OrbStack documents that its image shrinks automatically as data is deleted and Docker Desktop returns space within seconds, though neither is guaranteed (orbstack/orbstack#2030). The notes now say the file shrinks once the runtime returns the blocks, and to scan again to measure it
- The DeviceSupport note promised the symbols are "rebuilt on next device connect". Xcode copies them only from a connected device running that exact OS build, so a superseded build cannot come back and matters only for symbolicating its crash logs. The note now says so

### Added
- `clean simulators` discloses the data held by simulators that still work — on the machine above, 101 GB across 26 devices, while the category reported nothing because none was unavailable. It is one report-only finding sized from simctl's own `dataPathSize`, naming the three largest devices with their last use (last boot on Xcode versions that report only that) and `xcrun simctl delete <UDID>` as the manual step. Deleting a working simulator destroys its apps and data, so devtrim still deletes only devices whose runtime is gone; when a size is missing or its field changes shape the disclosure is omitted with a diagnostic, without touching unavailable-device cleanup, and applying a plan that holds only the disclosure measures nothing

## [0.9.4] - 2026-09-15

A whole-codebase review — two security audits, a Standards and a Spec axis, a
correctness hunt, and an independent Codex review of every tracked source file —
found two ways a *preview* or an unread screen could act, and a set of places
where the code was weaker than its own documents.

### Security
- Previewing a directory could run a program chosen by a repository inside it. The Git activity probe disabled hooks and fsmonitor but not the two other ways `git log` spawns a configured program: `log.showSignature` with `gpg.program` on a signed HEAD, and a lazy fetch through a promisor remote's `uploadpack`. Both were reproduced end to end from a dry-run `clean node-modules --json` against a repository copied in with its `.git/config`. The probe now passes `-c log.showSignature=false`, `--no-show-signature`, `--no-lazy-fetch` and `--no-pager`; a `git` too old for `--no-lazy-fetch` refuses the repository. Each path has a fixture that arms exactly it, a positive control proving the fixture fires without the hardening, and its own planted case
- The TUI delivered keys typed during a scan to the screen that appeared afterwards. Pressing `2sa0⏎` in one burst selected caches, switched to permanent mode, opened the critical confirmation and answered it — permanently deleting a cache whose results were never displayed. Input queued while a scan or apply blocks the loop is now discarded. `scripts/tests/tui.py` proves it in a real PTY, with the same keys typed after the results render as the positive control
- CLI `trash-empty --apply` never reached the shared confirmation gate, so a piped `--confirm=0 --apply` purged permanently without `-y`, `--yolo` or a prompt, and a terminal run asked nothing — contradicting "every interactive mutation confirms". It now shows the set it will purge and requires typed confirmation on a terminal or `--yolo` unattended; `-y` and `--yolo` are no longer silent no-ops there
- The size acknowledgment for a Trash purge measured the whole Trash while the plan excluded protected items, so in the TUI one large excluded item made the required `PURGE <gb>` unsatisfiable. Both the CLI and TUI now measure the exact findings being purged
- Permanent deletion restored a refused quarantine with a check-then-rename, and `Dir::rename` replaces its destination, so a file recreated at the original name in that gap would have been overwritten by something no preview showed. Quarantine and restore now rename with `RENAME_EXCL`
- `clap` quotes the offending argument in a parse error and strips ANSI sequences but not other controls; a bidirectional override in argv reached the terminal raw. Parse errors now go through a line-preserving terminal-safe renderer
- Build-process liveness compared `lsof`'s *display* spelling of each working directory with repository paths. `lsof` renders a newline as `\n`, a backslash as `\\`, and a non-ASCII byte as `\xHH`, so a build running in such a directory matched no repository and its dependencies stayed deletable. Unambiguous escapes are decoded; `^X`, which `lsof` also uses for a literal caret, refuses
- DerivedData liveness only looked for `xcodebuild`. Builds started from Xcode.app run through `SWBBuildService` (observed here with `Xcode` as its parent), so an IDE build or index could lose DerivedData underneath it. The probe now covers `Xcode`, `xcodebuild`, `SWBBuildService` and `XCBBuildService`
- The release publisher attested whatever artifact arrived under the expected *name*. A dependency running in another release job that obtained the runtime token could have substituted it after upload. `prepare` now publishes a digest of the exact inputs and the publisher verifies it before attesting, and never executes a downloaded input
- `release-policy.sh` passed with a workflow-level `write-all`, a job with no `permissions` block, extra write scopes on non-publisher jobs, any write scope in CI, a `- uses:` tag pin, or a publisher step running a downloaded script — each proven with a planted change. Permissions are now checked on the parsed workflow YAML, so `contents: "write"`, a trailing comment, and a flow mapping are the same grant they are to GitHub; every new rule was shown to fail on its violation, including those three spellings
- Every `gh attestation verify` in the release workflow, release script and Homebrew closeout now passes `--deny-self-hosted-runners`, and README and the landing page document attestation verification for users rather than only a checksum downloaded beside the archive
- Dependabot waits seven days before proposing a new release in every ecosystem; `actions/attest` moves to v4.2.2

### Fixed
- A repository was judged by HEAD's commit date alone, so an old project cloned today — whose `node_modules` had just been installed — was offered for cleanup immediately, as was a checkout of an old tag. Activity is now the newer of HEAD's commit date and HEAD's newest reflog entry, which clone, checkout and pull all write; with reflogs disabled it is the commit date, as before. HEAD is read on its own, not through the reflog walk, whose newest entry need not name HEAD
- `status` read its tools in the user's locale. Under `pt_BR`, `sysctl` prints a load average as `65,41` and `ps` a CPU share as `136,6`, so both metrics became unavailable and the health score *rose* because the missing load input could no longer lower it. Tools now run with `LC_ALL=C`
- `status` skipped a `netstat` link row of undocumented width, reporting a smaller network total with exit 0 even though the module's own comment says a sum refuses rather than skips. `vm_stat` sums saturated instead of refusing overflow
- `analyze` refused to *measure* a directory on another device but entered one when you pressed Enter on it, rooting the next walk on that device and traversing a network share. It now refuses, and a lower-bound entry is reported in `--json` `errors` with a nonzero exit, as quitting the explorer does
- A typed command that failed to start or exited nonzero reported neither its exit status nor its stderr. Docker, simulator and maintenance commands now share one runner that journals them and names both
- Errors raised after parsing reported `"operation": "unknown"` in JSON even for a known command such as `clean caches`
- A journal append after a short write (ENOSPC is likely on the disk this tool is cleaning) fused its record onto the unterminated fragment, and `history` discarded both. The tail is terminated first
- An explicit `--root` that does not exist scanned nothing and reported a clean machine; it is now warned about
- npm's and Homebrew's cache probes ran in whatever directory devtrim was started from, where a project `.npmrc` could redirect the reported cache root within the npm namespace. They run from `$HOME`, and report stderr when they fail

### Changed
- `trash` 5.2.8 (its only macOS change is a format string), `toml` 1.1.5 and `clap_mangen` 0.3.3; the generated man page is byte-identical. `toml` 1.1.6 was held back under the new seven-day cooldown
- `planted-violations.py` proves twelve boundaries instead of six: the two Git probe hardenings, reflog activity, HEAD's own commit date, the no-replace rename and `lsof` name decoding join the existing cases

### Documentation
- `uninstall`'s output, its module contract, and README described a "four-entry allowlist" under `~/Library`; MANUAL listed only four paths. The carve-out also includes every managed `Caches` entry, and the text no longer states a count that drifts
- README said the Docker VM disk image is shown even when the daemon is not running. It is shown when `docker` is not installed; with an installed but stopped daemon the category fails and its error names the image and size — the trade the code comment already recorded
- `status --json` is a vitals document, not the response envelope, and exits nonzero when any metric is unavailable; README and MANUAL now say so. The caches descriptions list Node/Corepack and the GitHub CLI cache, `active_days = 0` is documented as 1, and `installers` states that age is modification time, so a copy preserving it counts as old
- `CODING_STANDARDS.md S12` listed approved variable-program sites that no longer matched the tree; it names the four that exist. Stale comments about a removed resolver-flush task, a retired session-shape rule, and a misplaced `MANAGED_LIBRARY_CACHES` doc comment were corrected

## [0.9.3] - 2026-09-12

A retroactive review of the shipped 0.9.2 commit — run because the release
attestation had been given without it — found that the release which made
evidence mandatory shipped an entry with false evidence.

### Fixed
- `~/Library/Caches/gh` leaves the carve-out. Its evidence called it the GitHub CLI's API-response cache, but go-gh resolves that cache to `$XDG_CACHE_HOME/gh` and then `~/.cache/gh`, never to `~/Library/Caches` on macOS. Verified here: `XDG_CACHE_HOME` is unset, `~/.cache/gh` exists with content, and `~/Library/Caches/gh` does not exist at all. The entry was therefore authorized on the strength of its *name* — the exact thing 0.9.2's own convention calls insufficient — and it had been carrying that authority since 0.9.1
- gh's real cache is now covered: `~/.cache/gh` joins the built-in list with evidence that states the resolution order, the verification, and that cached private API response bodies can live there even though credentials cannot
- The three `carries_evidence` tests could not fail for the cases their documentation described. The const assertion rejects empty and ASCII-whitespace evidence *while compiling*, so such a crate never builds and no test runs; the 0.9.2 changelog's claim that "a test additionally names the offending path" was unobservable, and the commit's claim to have proven it was mistaken — that proof had shown the const assertion firing. Each test is now scoped to the one gap the const check cannot see — a non-ASCII blank such as U+00A0 that `str::trim` strips — and each is proven by its own planted case in `planted-violations.py`, which writes that blank into a real entry and requires that list's tagged assertion to fail. Four lists, four separate guards, four proofs — `REGENERABLE` and `HISTORY` sit in one test but are two loops, so they carry distinct markers and distinct plants. An in-test assertion about the helper would have proven nothing about any of them, and covering one list would not cover the rest
- A doc comment claiming "the case proven below" was applied to all three evidence tests when only one had gained assertions — and those assertions checked the const helper and `str::trim`, not the loop they sat in. The gate now carries the proof and the comments say where it lives; a leftover paragraph describing a carve-out acceptance check was also removed from the wrong test
- `planted-violations.py` removed its scratch tree only on success. Every `fail()` and an uncaught build timeout exited first, leaving a full source copy and an `--all-features` debug build — about 558 MB each — under `target/`. The likeliest failures are the early ones, in a gate developers run locally. Cleanup now runs in a `finally`, verified on both the success and failure paths

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
