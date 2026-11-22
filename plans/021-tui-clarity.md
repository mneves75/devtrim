# 021 — TUI clarity: overview home, decision-first rows, effect-first confirmation

Status: IN PROGRESS. Base `3310ef3` (0.16.0 + site closeout). Branch `feat/tui-clarity`.
Target `0.17.0` (minor: visible TUI changes and help ordering; no command, flag, exit code
or JSON change).

## Assumptions (no interview was possible; the request was a pasted brief)

- The brief's claim that the interface "overwhelms users" is tested against captures, not
  taken as given. Evidence below supports it for the home, the scan landing, result rows and
  the confirmation; it does not support changing the command vocabulary, which already follows
  Mole's shape (`scan`, `clean <category>`, `purge`, `analyze`, `status`, `optimize`).
- Keys stay as they are (`1`…`9`, `0`, `p`, `d`, `a`, `i`): documented, tested, used by the
  site frames, and learned by existing users.
- Launch behaviour stays: bare `devtrim` runs nothing until a key is pressed.
- No real-user study is available; improvements are reported as engineering measurements
  (keys per scenario, words and labels per screen), never as proven usability.

## Evidence (baseline, PTY, planted home: 2 caches, 2 stale `node_modules`, 1 old `.dmg`)

Captures in `.scratch/tui-clarity/baseline/` (untracked), made by
`.scratch/tui-clarity/capture.py` from the 0.16.0 binary.

- Home: 14 operations at equal weight; a 10-column uppercase safety badge on every row
  (15 on screen, the most saturated colour there); a side pane that repeats label, badge
  meaning and description and narrows to 23 columns after a scan at 80×24, where the shipped
  "Reclaimable" line does not fit (it shows only at 120×40).
- Scan everything lands on a flat list of findings in registry order (3 MB, 880 KB, 2 MB,
  1 MB), diagnostics mixed in; acting means finding the right row and pressing `o`, or `b`
  plus the category key. The home keeps the cursor on Scan everything and marks no category.
- Result rows: `› [x]   1. danger-3     3 MB  TRASH    uv package cache  ~/.cache/uv` — a row
  number and a 1–10 score lead, the name and the path trail.
- The confirmation for moving 3 MB to the Trash opens with `DATA-LOSS WARNING`, the same
  heading a permanent delete gets.

The three most consequential failures: (1) nothing answers "where should I look"; (2) the
row hierarchy is inverted; (3) the decision point warns a reversible move like an
irreversible one, which trains people to skip warnings.

## Design decision

Two prototypes rendered from the same data at 64×18, 80×24, 120×40
(`.scratch/tui-clarity/proto.py`, throwaway):

- **A, overview home (chosen).** One screen: header, a disk line, after a scan a headline
  (total reclaimable, largest category), the 14 operations in their current order with keys,
  a size column and one effect word per row (`trash` quiet, `PERMANENT` loud, `read-only`
  quiet), and the highlighted operation's description in the status row. Scan everything
  returns here with the cursor on the largest category that moves to the Trash.
- **C, Mole-style intent menu.** Five entries (scan, clean a category, purge, empty Trash,
  reports) with the categories one level down. Cleaner first screen, but every direct
  category action gains a level and a key (caches: 3 keys → 5), and Empty Trash moves from
  `0` to a submenu.

Scenarios, keys from launch: clean the largest category — baseline `1`, ↓×k, `o`, `a`, `y`
(4 + k); A `1`, `Enter`, `a`, `y` (4); C `1`, `Enter`, `a`, `y` (4). Clean a known category —
baseline 3, A 3, C 5. Empty Trash — baseline 1 + typed, A same, C 2 + typed. A failed category —
baseline: find its error row in the flat list; A and C: the row says `failed`, `Enter` opens
the category, which reports the reason and `r` retries.

Rejected: auto-scan at launch (runs probes unasked, 16 s and more under load, changes the
launch contract), auto-suggesting a PERMANENT category because it is largest, and deleting
the flat scan report (it carries the scan-roots note and every unjudged finding): it stays
one key away (`v`).

## Phases

1. **Home and scan landing** (`src/tui.rs` only): single column, header + disk line +
   headline; effect word per row; status row carries the description; Scan everything ends
   on the home with the cursor on the largest Trash category; `v` opens the last scan's full
   report. (`r` on the home was dropped: `1` already rescans, and the landing status says so.)
2. **Decision-first rows and effect-first confirmation**: rows lead with size and name, the
   row number goes, danger and action move to the end; the confirmation says the effect and
   whether it can be undone before the data-loss notice, which stays. Film copy, site frames
   and facts are regenerated from the new binary and the film is re-rendered.
3. **CLI help**: subcommands listed in task order (preview and clean first, reports next,
   utilities last) and an examples block; no command, flag or output contract changes.
4. **Structural fix for a recurring release mistake**: a test that fails when
   `Cargo.lock` or `fuzz/Cargo.lock` pins a `devtrim` version other than `Cargo.toml`'s
   (0.12.6-beta1 failed late on exactly this).
5. Docs, gates, independent review, security gate, release.

## Frozen acceptance criteria

- **AC1** After Scan everything, the home's cursor is on the category that moves to the Trash
  with the largest actionable size; no PERMANENT or read-only operation is ever placed there;
  with nothing offered the cursor stays on Scan everything. A headline names the total and the
  largest category. Proof: unit test and a PTY flow.
- **AC2** Scan everything ends on the home. Each failed category reads `failed` on its row and
  the status says how to see why and retry; `v` opens the full read-only report of that scan,
  which still lists unjudged findings, diagnostics and failures. Proof: PTY flow with a stubbed
  failing category.
- **AC3** The home is one column at every supported size; every operation's effect word is on
  its row and readable with `NO_COLOR`; all 14 operations, the headline and the footer fit at
  64×18 without scrolling. Proof: render tests at 64×18, 80×24, 120×40 and the NO_COLOR
  render test.
- **AC4** Result rows show size, then name, then path, then danger and action; no row number;
  the NO_COLOR danger ladder stays ordered; long paths keep whole characters. Proof: render
  tests; `tui.py` flows.
- **AC5** The confirmation shows the effect sentence first and says whether the plan can be
  undone (Trash) or not (permanent, typed command); the data-loss notice stays on screen; keys,
  typed answers, the 32-character cap, type-ahead, paste and modified-key refusals are
  unchanged. Proof: existing PTY flows pass; render test of the order.
- **AC6** CLI commands, flags, exit codes and JSON documents are unchanged (`tests/cli.rs`,
  feature evals); `--help` lists the cleanup commands first and shows examples. Proof: tests.
- **AC7** No safety boundary moves: `src/ops/`, `src/safety.rs`, `src/journal.rs`,
  `src/process.rs` unchanged (diff stat); every planted violation passes, with
  `tui/row-number-width` removed only together with the row numbers it guards.
- **AC8** fmt, Clippy `-D warnings`, tests on 1.98.1 and MSRV 1.88, ast-grep, `tui.py`,
  `read-only-views.py`, feature evals, `verify.sh offline` pass.
- **AC9** README, MANUAL, CHANGELOG, AGENTS.md = CLAUDE.md describe the shipped behaviour;
  site frames come from the built binary; `facts.json` regenerated; the film's copy matches
  its facts and the MP4 is re-rendered and reviewed; `csp.py --check` passes.
- **AC10** Reported improvements come only from captures (keys per scenario, words and
  labels per screen, before and after).
- **AC11** The lockfile test fails on a stale `fuzz/Cargo.lock` and passes on the bumped tree.

## Red team (assume it failed six months out)

1. **People think Scan everything did nothing**, because they expected the list. Trigger:
   0.16 habit. Cost: perceived regression. Cheapest experiment: the PTY capture after a scan
   must show the headline, sizes on rows and `v report` in the footer; the changelog says
   where the list went.
2. **64×18 overflows** once the headline joins 14 rows. Trigger: the header, disk line,
   headline and footer leave 13 rows. Cost: clipped safety words. Cheapest experiment: merge
   the disk line into the headline below 19 rows, and a render test asserting every row's
   effect word at 64×18.
3. **The film drifts from the product.** Trigger: row strings change, the MP4 is not
   re-rendered. Cost: the landing page demonstrates a UI that no longer exists. Cheapest
   experiment: a draft render of the current film before phase 2 (started; result in
   `progress.md`).

Simpler route considered: only reorder the existing side pane so the shipped headline fits
and place the cursor. It answers failure 1 partly and leaves 2 and 3. Verdict: ship in phases.
