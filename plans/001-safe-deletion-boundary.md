# Plan 001: Make every destructive path physically safe and explicitly applied

> **Executor instructions**: Follow this plan step by step. Run each verification
> command before continuing. Stop on any STOP condition; do not improvise.
>
> **Drift check**: `git diff --stat 8dbbd6c..HEAD -- src/safety.rs src/ops/mod.rs src/main.rs src/cli.rs`

## Status

- **Priority**: P0
- **Effort**: M
- **Risk**: HIGH
- **Depends on**: none
- **Category**: security / bug
- **Planned at**: commit `8dbbd6c`, 2026-08-23
- **Execution status**: DONE

## Why this matters

`is_protected` validates only the path spelling. A symlinked ancestor can route
an apparently allowed path into protected user data; an independent advisor
reproduced permanent deletion under `~/Library` through such a root.
`trash-empty` also permanently mutates without `--apply` and deletes directly,
contradicting the repository's headline safety contract.

## Current state

- `src/safety.rs:93-158`: `is_protected` performs lexical component cleanup but
  never validates a resolved physical parent.
- `src/ops/mod.rs:72-84`: `remove_path` checks `is_protected`, then trashes or
  permanently deletes the supplied path.
- `src/main.rs:74-76`: `trash-empty` immediately calls `purge_trash` without
  checking `cli.apply`.
- `src/safety.rs:247-260`: Trash purge calls `remove_dir_all` / `remove_file`
  directly.
- Convention: all filesystem deletion must funnel through a single audited
  safety owner; no shell-string execution; Trash-first unless an explicit
  permanent operation says otherwise.

## Commands

| Purpose | Command | Expected |
|---|---|---|
| Format | `cargo fmt --all -- --check` | exit 0 |
| Lint | `cargo clippy --locked --all-targets --all-features -- -D warnings` | exit 0 |
| Tests | `cargo test --locked --all-targets --all-features` | all pass |

## Scope

**In scope**: `src/safety.rs`, `src/ops/mod.rs`, `src/main.rs`, `src/cli.rs`, and
focused tests under `tests/` or module test blocks.

**Out of scope**: sudo, SIP handling, arbitrary user-path protection, rollback
journals, new dependencies, Mole code, network-volume guarantees.

## Steps

1. Add one shared physical-path validation helper used immediately before every
   filesystem mutation. Validate both the literal path and the canonicalized
   existing parent plus leaf name. Resolution may deny an operation but must
   never grant permission that the literal path lacked. Refuse unreadable or
   symlinked destructive roots when safety cannot be proven.
   - **Verify**: tests prove a managed path through a symlinked ancestor is
     refused while a normal managed path remains eligible.
2. Protect the user home root and `~/.Trash` root explicitly. `purge_trash`
   must refuse a symlinked Trash root and must only remove direct children of
   the verified Trash directory.
   - **Verify**: a temp HOME with `.Trash` symlinked elsewhere leaves a sentinel
     intact and returns an error.
3. Require `--apply` for `trash-empty`. Without it, report the measured Trash
   size and the exact command needed to apply. Rename the Clap field so the
   documented `--confirm=<gb>` spelling is accepted.
   - **Verify**: invoking `trash-empty --confirm=0` without `--apply` never
     removes a sentinel; adding `--apply` reaches the gate.
4. Make permanent deletion explicit. `--shred` must change the previewed action
   and confirmation severity; it must not merely reinterpret `"trash"` at the
   sink. Keep low-risk Trash operations recoverable.
   - **Verify**: preview output and JSON distinguish `trash` from `shred`.

## Test plan

Add deterministic tests using temporary directories created under
`std::env::temp_dir()`; no external test crate is required. Cover literal
protected paths, symlinked ancestors, normal managed paths, home root, symlinked
Trash, and missing `--apply`. Tests must never touch the real home directory or
real Trash.

## Done criteria

- [x] A symlinked ancestor cannot bypass protected-path policy.
- [x] `trash-empty` cannot mutate without `--apply`.
- [x] Trash purge refuses a symlinked/unverifiable Trash root.
- [x] Preview states whether deletion is recoverable or permanent.
- [x] Format, Clippy, and tests pass.
- [x] No new dependency is added.

## STOP conditions

- Physical validation requires following a symlink in order to grant access.
- A safe implementation appears to require `unsafe`, raw `openat`, or a new
  filesystem crate; report instead.
- Tests cannot isolate all deletion under a temporary HOME.

## Maintenance notes

Keep policy in one deletion owner. Future ops must not add direct
`remove_file`, `remove_dir_all`, or `trash::delete` calls outside that owner.
