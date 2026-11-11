# 016 — UI/UX redesign and whole-project audit (0.13.0)

Status: DONE in 0.13.0 (2026-11-11). Branch `feat/ux-0.13`, from `main` `af269a3`. Track C skipped one item: the shared group-kill helper, because two planted cases anchor separate kill lines.

## Request

Improve UI/UX, visuals and design; read the whole project and fix what is
certainly broken, inconsistent, inefficient or dead; list the rest; move toward
fewer, deeper modules; then verify, bump, document, merge, tag and release.

## Red team (before planning)

| Likeliest failure | Trigger | Experiment that falsifies it early | Result |
|---|---|---|---|
| A "deeper modules" refactor weakens a deletion guard | A structural move in `safety.rs` or the sink turns a planted mutant into one that no longer applies | `planted-violations.py` before and after each structural change | Decision: no structural change to the deletion boundary in this release; boundary items are listed for the owner |
| One release carries three unrelated risks | Any red gate | Baseline on untouched `main`; read hosted CI | `verify.sh focused` green; hosted CI red from one fake key in PR #83's history, fixed by an exact-fingerprint `.gitleaksignore` (17fd335) |
| The redesign follows trends instead of the product | "2026 trends" read as gradients, glows, bento | Render the page; find concrete defects first | Frames are monochrome because `frames.py` drops `38;5;N`; hero frame small with a dead right third; commands are three text walls; no install line above the fold |

Verdict: run the experiments (done), then ship in four tracks with separate commits.

## Evidence base

Seven read-only surveys (five code partitions, scripts/docs, outside design
research), each finding re-opened at its `file:line`. Reproduced by hand against
the built binary: `trash-empty` on an empty Trash asks for `--confirm=0` and then
for `-y` to remove nothing; `dashboard` errors read `dashboard: dashboard failed`.

## Track S — site (index.html, MANUAL.html, styles.css, scripts/site/frames.py, DESIGN.md v4)

1. `frames.py` maps `38;5;0-15` to the page's colour classes, so frames show the TUI's real colours.
2. First viewport: headline, one-paragraph pitch, the Homebrew install line with a copy control, manual link; then the hero frame at full container width.
3. A stepped walkthrough of real frames (preview, leave one out, confirm, outcome); without script every step is visible in order.
4. Commands as one compact table grouped READ-ONLY / TRASH / PERMANENT, one line each, linking to the manual; add the missing `agents` and `installers`.
5. "Never touches" beside the real refusals; verification commands stay.
6. Box-drawing characters join; frames keep the 11 px floor and scroll inside their own box.
7. Manual shares the landing page's bar, tokens and type scale.
8. Constraints: CSP by hash only, no `style` attributes, elements limited to `csp.py`'s list (extend it deliberately), every sentence the film's `copy.json` cites stays verbatim, version strings stay at the live stable version until release.

## Track T — terminal UI (src/tui.rs, theme.rs, report.rs, analyze.rs, status.rs)

1. Results title names the mode the screen will actually apply: READ-ONLY, TRASH-FIRST or PERMANENT (today Empty Trash reads TRASH-FIRST).
2. Confirmation states the count, size and effect; is sized to its content; clears the rows it covers; y/N plans show `y apply · n/Esc cancel` and no input line.
3. Menu badges fit at 64 columns; PREVIEW becomes TRASH, matching the site and DESIGN.md.
4. Header is one row and the footer two, unbordered; taglines go. Three more rows for content at 64×18.
5. Status line carries its own token (neutral, refusal, failure) instead of always warning yellow.
6. Clipped diagnostic rows end with an ellipsis; `info:` is not styled as a warning.
7. Path column shows `<project>/<relative path>` so rows stay distinguishable when narrow.
8. One footer grammar across the TUI, analyze and status; `Enter details` always listed.
9. Startup diagnostics are kept, not discarded by the first scan.
10. `Scanning everything…`, plain error and outcome copy, an empty result shows one message.
11. analyze: reselect the folder left on ascend, paging keys, trailing slash for folders, error count, too-small screen. status --watch: unavailable reasons above processes, band colour on the headline, "updated Ns ago".
12. theme.rs gains the test its comments cite: the NO_COLOR danger ladder is strictly ordered.

## Track R — certain fixes in Rust (each with a test that fails first)

- `trash-empty`: no hint, confirmation or prompt for a plan with nothing actionable.
- Trash listing, DeviceSupport and Codex release read errors name the path.
- `--yes`, `--apply` and `purge` help text match behaviour.
- `dashboard` errors name the command once.
- `simulators` apply no longer walks the whole Devices tree twice or fails on an unrelated unreadable device.
- `installers` reports an age-gate refusal as such; `installers`, `toolchains` and `simulators` continue past a refused finding.
- `icloud` contains a per-file metadata failure; `largest` stays on one device.
- `xcode_activity` probes the current user's processes only, as the build probe does.
- `agents` apply runs the system-wide `lsof` probe once per release, not twice.
- Partial reads in `worktrees`, `leftovers` and `uninstall` exit nonzero (SECURITY.md layer 9).

## Track C — shape and dead code (no behaviour change, after R and T)

Journal lock observer and the unreachable `errors` branch; `measure_lenient` moves
to `worktrees.rs` (the docs already cite it there); one group-kill; one `pgrep_pids`;
one category-name table with a test tying `Target` to `ops::all()`; one report tail in
`app.rs`; one error-finding constructor; one `.git`-file parser; one test `Ctx`
constructor (about 300 lines of repeated literals); stale comments.

## Track D — documents

`CLAUDE.md`/`AGENTS.md` (lines 138, 149, the pinned nightly, the Codex voice
manifest), `CODING_STANDARDS.md` S12 and the hooks line, `SECURITY.md`
(`rust-analyzer`), `PRODUCT.md`, `plans/README.md`, `video/README.md`, changelog,
README and manual for every changed behaviour.

## Not done here — for the owner

Recorded in the final report with evidence: boundary items (`Ctx::protect`
sentinel type, the redundant second protection check, APFS `ſ` folding in the
protected-name check), the `artifacts`/`node_modules` shared skeleton, Unicode
whitespace in the evidence check, `--root` accepted by commands that ignore it,
`--json` skip-note asymmetry, Codex releases failing wider than one release,
docker/optimize stopping at the first refusal, CI scope and duplicated shell-syntax
lists, unreferenced tracked media (about 16 MB), the npm waiver expiring 2026-11-30,
the "macOS 13+" claim with no deployment target, PR #83 and six Dependabot PRs.

## Verification

`verify.sh offline`, MSRV, the five fuzz targets, the video gate, the PTY flows,
`planted-violations.py`, a real browser at 1440 and 390 px, the page-load budget,
a cross-model review (Sol reviews Opus-built changes), `reviewer`, and an
independent `mneves-verify` verdict before the tag.
