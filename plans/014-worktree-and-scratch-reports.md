# Plan 014: Report worktrees and agent scratch; delete neither

## What the review changed

Two independent plans were written for the owner's request to bring the general
parts of his personal `mac-disk-hygiene` setup into devtrim: one by the main
session, one by GPT-6.1 Sol. The main session's plan removed clean, idle
worktrees through the Trash. Three checks against the code ended that:

- **No inert cleanliness proof.** `git status` refreshes the index, and that
  refresh can run a `filter.<driver>.clean` program the repository configures.
  devtrim's Git queries are `ls-files`, `log` and `for-each-ref` precisely so
  that previewing an untrusted repository runs nothing.
- **A trashed worktree could never be purged.** `trash-empty` refuses a Trash
  item that still holds a Git marker; only uv's marker is granted.
- **"HEAD on a branch" excludes the main population.** Codex creates its
  worktrees detached.

The merged plan is therefore report-only. It takes Sol's two surfaces and its
refusal to call anything safe, and drops machinery no test yet demands.

A fresh-context adversarial review of the merged plan then ran its Git queries
against a hostile fixture (Git 2.56) and changed it again: "ignored" is now an
actual `check-ignore` test; a listed worktree path is treated as repository
data and sized only after its `.git` resolves to the listing repository; an
orphaned worktree is a skip note instead of exit 1; the reflog date and the
total are gone; and two tests that could not fail were replaced.

## Owner decision (2026-10-11)

Option A: two read-only reports, no new deletion authority. Whole-worktree
deletion, duplicate-copy detection and archiving to another volume each need
their own plan. Scheduling, admission gates, shell paths and third-party tool
configuration stay in `mac-disk-hygiene`. A public release needs a separate yes.

## Baseline and ownership

Planned at `b2bf8a28f67ae0f18fb404fca6922d5479557c62`, branch
`feat/worktree-reports` in `/Users/mneves/dev/devtrim`. No new dependency,
category in `ops::all()`, deletion entry, command authority or sink change.
Codex implements; the main session owns the plan, docs wording, review,
integration and acceptance.

## Behavior

### 1. `devtrim worktrees` (new read-only command)

A standalone report like `largest` and `status`, outside `ops::all()`, so
`scan` does not pay for it.

- Repositories are found under `normalized_roots` the way the project
  categories find them. Each is queried once with
  `git worktree list --porcelain -z` through the existing hardened invocation
  and the bounded command runner. The first record is always the main
  worktree, even when the query runs from a linked one; its path is the key
  that prevents a second query.
- A directory for which `orphaned_worktree` is true (a linked worktree whose
  repository is gone, routine under `~/.codex/worktrees`) is named in a skip
  note, as the scanners already do. It is not an error.
- Per linked worktree, one `Action::Info` row: path, estimated logical size,
  and a `note` in fixed wording holding the branch or `detached`,
  `locked: <reason>` and `prunable: <reason>` as Git reported them.
- **A listed path is data from the repository, not a fact.** It is sized only
  when it is a real directory inside a scan root, judged by canonical path or
  device and inode, whose `.git` file resolves to this repository's common
  directory. Anything else is listed unsized. No Git command ever runs with
  `-C` into a listed worktree.
- The main worktree is listed unsized: it is the repository, not a copy.
- Sizing uses a walker that follows no symlink, stays on one device, and
  counts unreadable entries instead of failing; a size with unread entries is
  shown as a lower bound. A linked worktree nested inside another listed one
  is subtracted from its parent. No total is printed.
- The report never says safe, stale, inactive or removable, and shows no
  activity date: a date reads as staleness, and uncommitted work leaves none.
- What was not assessed (uncommitted or untracked work, ignored files, open
  files, running processes) is stated once: as a human `info` diagnostic, and
  in JSON as a final Info finding with no path. It never goes in `errors`. It
  names `git worktree remove <path>` and says that command runs the
  repository's configured programs, so it belongs only in repositories the
  reader trusts and after their own review.
- Mutation flags (`--apply`, `-y`, `--yolo`, `--shred`) are rejected through
  the same tables as the other report-only commands, and error JSON names the
  operation. No journal is created. `--json` emits one document in the
  existing findings-plus-errors envelope.
- A listing that fails, times out or cannot be parsed is one error and exit 1;
  the other repositories are still reported. Unknown porcelain attributes are
  ignored; a record without a `worktree` line is a parse error.
- Paths are parsed from the NUL-delimited output and displayed escaped. A
  bare repository has no `.git` marker and is not discovered; the manual says
  so.

### 2. `.scratch` entries in `clean leftovers`

`leftovers` already hints at agent leftovers by name (`kimi-*`,
`codex-worktree.*`, `.supergoal`). Add: for each direct child of a normalized
root that is a repository and has a real `.scratch` directory at its root,
the direct real-directory children of that `.scratch`, when Git ignores it.

- Ignored means both: a hardened `git check-ignore -q -- .scratch/` exits 0,
  and the existing `ls-files` query shows no tracked path below it. Git runs
  only when `<repo>/.scratch` exists.
- One `Action::Info` row per child with its estimated size and the note
  "review manually; ignored files are still your data".
- Not reported: a `.scratch` that is not ignored, one that holds tracked
  files, one that is a symlink, one nested below the repository root,
  symlinked children, plain files.
- A child that is itself a linked worktree (its `.git` is a gitfile) is
  labelled as one.
- `leftovers` switches to `normalized_roots`, so overlapping roots stop
  duplicating rows. The existing hints otherwise keep their behavior and tests.

### 3. Manual section: keeping worktrees small

Documentation only, in `MANUAL.html` and linked from `README.md`: leave a large
tracked directory out with a sparse checkout; install dependencies per
worktree from the package manager's store instead of copying them; copy data a
task changes with an APFS clone (`cp -cR`, same volume, and note that links are
copied as links); reach read-only data through one shared copy. State that
devtrim reports logical bytes, so cloned trees look larger than the space they
hold.

## Tests (written first, seen failing)

Through the real binary with the disposable-HOME harness
(`tests/eval_reports.rs`, `tests/cli.rs`, the command tables in
`tests/eval_system.rs`), plus unit tests for the parser and the walker:

- Parser: the documented porcelain examples; `-z` records with a newline in
  the path; a non-UTF-8 path (asserting the displayed replacement form);
  `locked` with and without a reason; `prunable`; `bare`; an unknown
  attribute; a truncated record.
- A repository with linked worktrees on a branch, detached and locked yields
  exactly those rows, with fixture-derived sizes and the fixed note wording.
- Forged registration: a `gitdir` entry naming a plain file, a path outside
  the roots, and a directory whose `.git` points at another repository are
  each listed unsized.
- Outside the roots, a `chmod 000` subdirectory in a listed worktree does not
  fail the run; the control is the same fixture inside the roots, where the
  size becomes a lower bound.
- An orphaned linked worktree produces the skip note and exit 0.
- Hostile repository config (hooks, fsmonitor, a clean filter, a signature
  program): no sentinel after `worktrees` or after `clean leftovers`. The
  positive control is the unhardened `ls-files` firing the `core.fsmonitor`
  sentinel; the other keys have no query that fires them and carry no
  control. No planted violation is added for `worktree list`, which runs no
  configured program even unhardened (measured on Git 2.56); the shared
  builder's existing planted violation covers the hardening.
- `.scratch`: the positive case, and one near miss per exclusion above,
  including an untracked `.scratch` that is not ignored.
- Planted violations: the `check-ignore` condition, the tracked-file
  exclusion, and the common-directory check before sizing.
- Mutation flags are rejected; the fixture, its index, refs and worktree
  registrations are byte-identical after both reports; no journal exists.
- A failing listing in one repository gives exit 1 and still reports the other.

## Documentation and version

`README.md` (command list, the paragraph that says leftovers "never deletes
worktrees", scan-root text), `MANUAL.html`, `SECURITY.md` (the new Git query
and that no authority was added), `AGENTS.md` and `CLAUDE.md` kept
byte-identical, the man page and completions if generated from the CLI,
`CHANGELOG.md` under Unreleased, `plans/README.md` row for 014, `MEMORY.md`.
Version 0.11.0 (new command) set where the release procedure sets it.

## Gates

`bash scripts/verify.sh focused` during work; before acceptance
`bash scripts/verify.sh offline` and the release build. Then the code-review
pass with an adversarial challenge and independent verification against this
file's Behavior and Tests sections.

## Rejected alternatives

- **Remove worktrees through the Trash** (the main session's first plan):
  blocked by the three findings above. It needs its own plan: an inert
  cleanliness proof, a Trash grant that `trash-empty` can honor, and a rule
  for detached HEADs.
- **Put sized worktrees in `leftovers`**: one command fewer, but every `scan`
  would traverse whole worktrees.
- **Leave everything in `mac-disk-hygiene`** (the premise challenge): cheapest,
  but the visibility stays tied to one machine's configuration.

## Five years on

Reports gather evidence; cleanup categories justify exact actions. A name or
an age must never quietly become permission to delete.
