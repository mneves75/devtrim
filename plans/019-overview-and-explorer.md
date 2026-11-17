# 019 — Answer "where did my disk go" before asking what to clean

Status: TODO (plan only). Base `9ce4edc` (0.14.0 production). Target 0.15.0.

Origin: review of `govijr/fix-storage` f354e20 (TypeScript, 2026-11-16) and
`morajabi/poo` 1893c39 (Rust + SwiftUI, 2026-11-16), requested 2026-11-17 as
"improvements like the UI/UX, inspired, not identical". Plan 018 already
compared poo at the same commit for cleanup *behavior* and rejected least-regret
ordering, auto-clean, schedules, hooks, self-update, user rules and worktree
removal; those rejections stand and are not reopened here. This plan covers the
*interface* ideas 018 did not weigh. Both sources were read as untrusted data
in a scratch clone; nothing was built or run from them.

## The task, played back

Study both tools' interfaces and the product ideas around them, decide which
make devtrim better for its users, and write a plan to build them in devtrim's
own form: no copied layout, wording, palette or mechanics, and nothing that
loosens a rule in `CLAUDE.md § Conventions`.

## The deeper version

Both tools open on the user's real question — *why is my disk full, and what
can I get back without regret?* — and answer it with the volume's own numbers
first. devtrim opens on a list of fourteen operations: you must already know its
taxonomy before it tells you anything about your disk, and the one place it
shows totals (Scan everything) gives a blank screen with a clock for the whole
scan, measured at 110–261 s on this Mac. fix-storage's best idea is the *culprit*: under a big
folder it names where the space really is (`mostly in app/node_modules · 77%`).
poo's best ideas are a progress line during a clean with "stop after this item",
and a detail pane that says what removal costs (`comes back with: cargo build`).

A professional building this (the people behind ncdu, DaisyDisk, Finder's
Storage pane) starts from the rawest numbers the system gives — `statvfs`, the
APFS snapshot list, the apply journal, the findings devtrim already corroborated —
never from a name heuristic, and proves the result by reconciliation and by
timing, not by taste. For devtrim the restated task is:

1. Show the volume's free and used space the instant the TUI opens, and turn
   the operations menu into an overview once Scan everything has run: each row
   carries the size it can give back.
2. Make the 2–4 minute scan legible: list each category as it finishes.
3. During an apply, show how far it has gone and let the user stop *between*
   targets, never inside one.
4. In each finding's detail, say how the target comes back, from evidence the
   scanner already holds.
5. Make `analyze` say where a folder's space really is, and — on request — what
   devtrim itself would do with each entry, by the same findings `explain` uses.
   It still never deletes.

Evidence it is right: time to first number < 100 ms in a PTY test; category
rows equal to `scan --json` per-category actionable sums; a planted test that a
stop request leaves every journal attempt paired and every later target
untouched; the culprit chain equal to a brute-force reference on a fixture tree;
the 64×18 render tests and `NO_COLOR` tests passing for every new line.

## Evidence gathered (2026-11-17, this Mac)

- `target/debug/devtrim scan --json`: real 110.1 s, user 15.4 s, sys 97.5 s
  (I/O-bound; the build profile barely matters). Release binary 0.14.0,
  two warm runs: real 180.7 s and 261.5 s (sys 131 s and 189 s). A scan takes 2–4 minutes.
- Apply journal (`~/.local/state/devtrim/journal.jsonl`, 7,830 records, 52
  runs split at 120 s gaps): median 36.5 s, p90 338 s, longest 834 s (a 338-item
  `trash-empty`, 72.2 GB). Four runs over 7 minutes. The TUI's apply screen
  shows only a clock for all of it.
- `scan.err` from the debug scan: no unreadable-folder lines at all, only
  "skipping protected path" and the DerivedData note. A Full Disk Access notice
  has no evidence here.
- `com.apple.lastuseddate#PS` is absent on `/Applications/Safari.app`,
  `/System/Applications/Notes.app` and `/Applications/Xcode.app`, while
  `mdls kMDItemLastUsedDate` returns dates for two of them: last-used dates live
  in the Spotlight index only. Reading them means an `mdls` subprocess with
  dynamic path arguments or CoreServices FFI. Falsified as a cheap win.
- `diskutil info -plist /` gives `APFSContainerFree` but no purgeable figure;
  poo computes purgeable through `NSURLVolumeAvailableCapacityForImportantUsageKey`,
  which needs CoreFoundation FFI (`unsafe`).
- The TUI outcome screen already prints the measured free-space line
  (`report::home_volume_line`, `src/tui.rs:2178`) since 0.14.0.
- R1 experiment (one release scan, 511 findings): actionable 80.05 GB by allocated
  blocks, at least 54.94 GB by APFS private size. Stale `node_modules`: 21.77 GB
  shown, at least 1.88 GB (ratio 0.09, pnpm clones); `uv` 1.76 vs 0.25 GB;
  DeviceSupport, DerivedData, Docker, Gradle equal.
- devtrim has no issues and two stars; a contribution template has no audience yet.

## Decisions

Adopted, each in devtrim's own form:

| Idea (source) | devtrim's version | Why it fits |
|---|---|---|
| Disk meter on open (both) | One text line plus an eighths-block bar in the menu's right-hand pane, from `volume.rs` `statvfs`, and the local snapshot count from the existing `diskutil apfs listSnapshots` query | Instant, no new probe, no new dependency, no pixel graphics |
| Category sizes on the home screen (both) | After Scan everything, each menu row shows its actionable size and finding count from the scan's own sections; rows of a category applied since go back to "rescan" | Reuses `scan_all` sections; menu numbering and the "b, then 2 (caches)" routes stay stable |
| Scan progress (fix-storage "calculating…", poo first-scan screen) | The loading screen lists categories as each completes (size, count, failed), the rest as waiting | Registry-order results stay byte-identical; only a completion channel is added |
| Clean progress + stop (poo "esc cancel after this item") | `n of N · X of Y · <category> <short path>` and a stop key honored only between findings | Journal evidence: p90 apply 5.6 min |
| Rebuild line (poo `rebuild` field) | "Comes back with: `cargo build`" in the TUI detail pane and the human `scan` note, derived from the corroboration the scanner already recorded (sibling `Cargo.toml`, the lockfile kind, `REBUILT_BY_COMMAND`); absent when there is no evidence | Display only; no new rule table |
| Culprit (fix-storage `└ mostly in …`, `g`) | `analyze` shows "mostly in a/b/node_modules · 5.1 GB · 77%" under the highlighted folder; one key descends straight to it | Computed in the walk `analyze` already makes, no second pass |
| Labels (fix-storage junk/unused/system) | `analyze` verdicts from real findings: OFFERED by `clean <category>` (TRASH or PERMANENT), HOLDS n offers, PROTECTED (reason), or no mark; a filter shows only entries holding offers; loaded on a key, never automatically | Name heuristics are forbidden as authority; `explain`'s matching already exists |

Rejected for this plan (each with its reason):

- Pixel or dot-matrix meters, kitty graphics, particles (fix-storage): a new
  dependency and terminal-specific rendering for decoration; eighths blocks
  already show proportion honestly.
- Name-based labels (`build`, `dist`, `.venv` as "unused"): `CLAUDE.md` forbids
  ambiguous names as evidence.
- Two-press delete with a 4 s timer (fix-storage) and single-key confirm (poo):
  weaker than the typed, plan-bound confirmation.
- Delete from the explorer (fix-storage `⌫`): `analyze` never creates deletion
  authority.
- Last-used column (fix-storage Spotlight dates): falsified above; Git activity
  is the better evidence for projects.
- Purgeable space (poo): needs CoreFoundation FFI; no measured need.
- Full Disk Access banner (both): no unreadable folders on this Mac's scan; the
  README already tells users when to grant it. Revisit with a real report.
- `/` filter and `s` sort in the TUI results (poo): the plan is "displayed
  findings minus those left out"; hidden rows would make that ambiguous, and `s`
  is the permanent-mode key. The filter goes to `analyze`, which applies nothing.
- `growth` history and fill forecast (poo): stores a new file on the user's Mac.
  That needs your decision, not mine; it is listed below.
- `--demo` (fix-storage): previews are already read-only; `frames.py` already
  renders the site from an isolated HOME.
- Mouse support: no request, and a click near the confirmation is a risk the
  keyboard model avoids.
- Menu bar app, MCP server, agent hooks, schedules, `curl | sh` installer,
  rules as TOML mods, FSEvents index (poo): outside `PRODUCT.md`, or rejected
  in 018.
- Separate `unexplained` command (poo `suggest`): the `analyze` verdict filter
  plus `largest` answer the same question.
- Issue template for deletion-list proposals: no contributors yet.

Owner decision (not blocking): a free-space history and forecast would need a
new state file beside the journal (one sample per scan, compacted). Say yes and
it becomes plan 020.

## Red team

Assume 0.15.0 shipped and failed six months later; the three likeliest reasons:

**R1 — The menu shows "Caches 40 GB" and the user gets back 2 GB.** Assumption
taken as true: an actionable size is what comes back. It is allocated blocks;
APFS clones overstate it (018) and Trash moves free nothing until
`trash-empty`. Trigger: pnpm-heavy trees, Trash-first applies. Cost: the
overview becomes the place users learn to distrust devtrim. Falsify early: from
one `scan --json`, compare per-category `size_bytes` against
`frees_at_least_bytes`. Result: confirmed — 21.77 GB shown for
`node_modules` against 1.88 GB guaranteed. Rule adopted: a row whose lower bound is smaller shows the range
`1.9–21.8 GB` and the detail says what the lower bound means; a Trash row never says "freed".

**R2 — Stop leaves the journal or a target half done.** Failure case nobody
considered: categories that preflight every finding first (`artifacts`,
`node_modules`), typed commands that run up to 900 s (Docker, simulators), and
`trash-empty`'s own loop. Trigger: stop pressed mid-preflight or mid-command.
Cost: an attempt with no result, which `history` reads as interrupted, or a
category that ignores the key for 15 minutes while the screen says "stopping".
Falsify early: a planted test that sets the stop flag from the sink after the
first target and asserts one result, every attempt paired, the rest untouched
and reported "not attempted: stopped"; plus a stub command that sleeps, proving
the screen says "stops after this command". If either cannot hold, stop ships
for filesystem categories only.

**R3 — The verdict misleads where analyze looks.** Optimism: "no mark" reads
as "devtrim has no opinion", but `analyze` can browse anywhere while
categories look only under their roots. Trigger: browsing `~/Documents/x` where
no root reaches. Cost: a user concludes a folder is safe or unsafe from silence.
Falsify early: `devtrim explain` on a path outside every root and read its
wording. The verdict must show three states: offered, protected, and
"outside what devtrim scans", never a bare blank for the last.

Simpler route: tasks T1, T3 and T4a alone (instant volume line, sizes on the
menu, apply progress without stop) give most of the visible gain with no change
to an authority path. The plan is layered so that version ships first.

Verdict: **run R1's experiment, then ship in layers.** The experiment is
read-only and runs here; nothing in it needs your input.

## Tasks

Layer 1 needs no change to any apply path. Layer 2 touches apply. Layer 3 is
read-only exploration.

| # | Layer | Task | Files | Acceptance check |
|---|---|---|---|---|
| T0 | — | R1 experiment and release-binary timing recorded here | this file | numbers filled in |
| T1 | 1 | Volume line in the menu's right pane: free of total, percent used, eighths bar, snapshot count; snapshot query on a worker with "checking…"; failure shows "unavailable" with its reason | `src/tui.rs`, `src/volume.rs` | render test at 64×18 and 120×40; `NO_COLOR` test; PTY test that the line appears before any key; unreadable volume stub shows reason, exit status unchanged |
| T2 | 1 | Streaming scan progress: `scan_all` takes an optional completion sender used from each scoped thread; loading screen lists done/failed/waiting categories with size and count | `src/ops/mod.rs`, `src/tui.rs` | unit test: `scan_all` with and without sender returns byte-identical `ScanResult`; PTY test with one stubbed probe that never answers shows nine done and one waiting; planted violation: result order follows completion → fails |
| T3 | 1 | Menu rows carry the last Scan-everything size and count; a `low–high` range when the APFS lower bound is smaller; a category applied since then says "rescan"; scan failure says "failed" | `src/tui.rs`, `src/report.rs` | eval: row sums equal `scan --json` per-category actionable sums on a planted fixture; render tests at 64×18 keep every key line whole |
| T4a | 2 | Apply progress line: findings done of total, bytes done of total, current category and shortened path, from a progress callback the shared sink and `run_command_authority` call after each result is journaled | `src/ops/mod.rs`, `src/tui.rs` | PTY test with a slow stub shows advancing counts; journal unchanged byte for byte against a run without the callback |
| T4b | 2 | Stop between findings: a key on the apply screen sets a flag read before each attempt is journaled; remaining findings become "not attempted: stopped" in the summary; the screen says "stops after this item/command"; SIGINT/SIGTERM behavior unchanged | `src/ops/*.rs` apply loops, `src/tui.rs`, `CLAUDE.md` | planted-violations case (stop ignored → test fails); R2 tests; `tui.py` proves `q` and Ctrl-C still refused during apply, and stop returns to the outcome screen with nothing after the current target touched |
| T5 | 1 | "Comes back with" line from existing corroboration; none without evidence | `src/ops/artifacts.rs`, `node_modules.rs`, `caches.rs`, `report.rs` | unit tests per evidence kind; eval: `scan` human output shows `cargo build` beside a planted `target`, nothing beside an uncorroborated one |
| T6 | 3 | Culprit chain in `analyze`: post-order walk keeps, per open directory, its running total and largest child chain; chain continues while a child holds ≥ 50 %, stops at `node_modules`, a bundle, or a single file; jump key descends straight to the end, `b` climbs back one level at a time | `src/analyze.rs` | property test against a brute-force reference on generated trees; hard-link and partial-walk cases mark the chain as a lower bound; help overlay fits 64×18; `read-only-views.py` extended |
| T7 | 3 | Verdicts in `analyze` on a key: one `scan_all` on a worker (progress as T2), entries tagged OFFERED/HOLDS/PROTECTED/OUTSIDE via `explain`'s matching; filter key shows only entries holding offers; detail names the command | `src/analyze.rs`, `src/explain.rs` | evals on a planted fixture for each state; test that no code path from `analyze` reaches the sink (ast-grep rule or module dependency test); `NO_COLOR` keeps the four states distinct in text |
| T8 | — | Docs: README, MANUAL, `CLAUDE.md`/`AGENTS.md` (identical), DESIGN.md terminal section, CHANGELOG 0.15.0, site frames via `frames.py` | docs, `index.html`, `MANUAL.html` | `verify.sh offline`, `csp.py --check`, AGENTS/CLAUDE comparison |
| T9 | — | Gates: full test suite, MSRV, Clippy, `tui.py`, `read-only-views.py`, planted violations, reviewer agent, mneves-verify, mneves-security deploy gate, release per `CLAUDE.md § Release` | — | records under `.scratch/security-gate/` |

Delegation: T1+T3 (one worker, `src/tui.rs` menu), T2+T4a+T4b (one worker,
apply/scan plumbing), T5 (one worker, ops notes), T6+T7 (one worker,
`analyze.rs`). Disjoint files except `src/tui.rs`, so T1/T3 land before the
T2/T4 worker touches the loading and apply screens. The main session keeps T0,
T8, T9 and every review.

## Out of scope, noted

- poo reads process working directories with `proc_pidinfo` instead of
  `pgrep`/`lsof`. That could retire CORE-PARSE-1's `lsof` name ambiguity, but
  it needs `unsafe` FFI in a crate like `apfs-size`. Worth its own plan.
