# Plan 003: Stabilize JSON, config, errors, and exit status

> **Executor instructions**: Treat `--json` as a public API. One invocation must
> emit exactly one JSON value to stdout; diagnostics belong in the value or on
> stderr, never as a second JSON document.
>
> **Drift check**: `git diff --stat 8dbbd6c..HEAD -- src/main.rs src/report.rs src/safety.rs src/cli.rs`

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: `plans/002-immutable-conservative-plans.md`
- **Category**: bug / dx
- **Planned at**: commit `8dbbd6c`, 2026-08-23
- **Execution status**: DONE

## Why this matters

`clean --apply --json` currently emits two adjacent JSON documents, empty clean
results emit no JSON, and partial scan failures disappear. Config examples using
`~/` do not work, while malformed config silently falls back to another root.
Agents and scripts cannot safely automate a destructive tool with those contracts.

## Current state

- `src/main.rs:47-68`: findings print before apply; summary prints later.
- `src/report.rs:51-77`: findings and summary have independent JSON printers.
- `src/ops/mod.rs:48-61`: `scan_all` suppresses category failures in JSON mode.
- `src/safety.rs:83-90`: config parse/read failures collapse to defaults.
- `src/safety.rs:46-49`: config roots become raw `PathBuf`s rather than using
  the existing `shellexpand` helper.
- `src/cli.rs:40-43`: `--root` says "extra paths" while implementation replaces
  configured/default roots.

## Scope

**In scope**: `src/main.rs`, `src/report.rs`, `src/safety.rs`, `src/cli.rs`,
`src/ops/mod.rs`, integration tests.

**Out of scope**: NDJSON, schema version negotiation, logging framework, custom
exit-code taxonomy, config globbing, environment-variable expansion beyond `~/`.

## Steps

1. Introduce one serializable response envelope with the minimum stable fields:
   operation, applied, findings, optional summary, and errors/warnings. Always
   emit it in JSON mode, including empty findings and partial scans.
2. Buffer command results and print once. Keep stdout data-only; progress and
   human diagnostics stay on stderr. A failed or partial apply must return a
   nonzero exit code. A read-only `scan` with category failures must be marked
   incomplete and return nonzero while preserving successful findings.
3. Make human output equally truthful: report `shredded` versus `trashed`, do
   not claim failed bytes reclaimed, and distinguish informational/non-actionable
   rows from reclaimable totals.
4. Expand `~/` in config roots using the existing helper. If the config file
   exists but cannot be read or parsed, return an actionable error rather than
   silently scanning default roots.
5. Resolve `--root` semantics without adding a mode flag: choose the documented
   simplest behavior and make help/docs match it. Preferred: explicit CLI roots
   replace config/default roots, because that is the current behavior and avoids
   surprising broad scans.
6. Correct confirmation help so `-y`, `--yolo`, non-TTY behavior, and danger
   levels match the implemented policy. Preferred minimal policy: non-TTY apply
   always needs `-y` or `--yolo`; `-y` bypasses normal y/N gates but never typed
   critical confirmation; `--yolo` bypasses confirmation only.

## Test plan

Add CLI integration tests using `env!("CARGO_BIN_EXE_devtrim")` or an equivalent
Cargo-supported binary path. For every command in JSON mode, parse stdout with
`serde_json::from_slice` and assert no trailing content. Cover empty results,
dry-run, apply success, apply failure, partial scan, malformed config, and
`~/` config roots. Tests must use temporary HOME/PATH and fake commands.

## Done criteria

- [x] Every `--json` invocation emits exactly one parseable JSON value.
- [x] Empty and partial results are explicit.
- [x] Failed apply actions return nonzero and cannot inflate summaries.
- [x] Existing malformed config fails closed.
- [x] Documented config `roots = ["~/dev"]` works.
- [x] CLI help and safety gates agree.
- [x] Format, Clippy, and tests pass.

## STOP conditions

- Fix requires a second streaming format.
- Existing JSON consumers are discovered and require a compatibility decision;
  stop and report before breaking a documented external contract.
- Tests would touch real HOME, Trash, Docker, npm, brew, Git state, or simulators.

## Maintenance notes

Before future JSON field changes, treat names and types as API. Add fields rather
than renaming/removing them during the 0.x line unless a deliberate minor-version
break is documented.
