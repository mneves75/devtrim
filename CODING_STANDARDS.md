# CODING_STANDARDS.md — devtrim

Rules a reviewer checks in a diff. Read at review time, not implementation time.

Cite a finding as `CODING_STANDARDS.md S<n>`. Precedence when sources disagree:
this file, then `SECURITY.md`, then `CLAUDE.md`, then generic code smells.

**Hard** rules block. **Judgement** rules are raised as a suggestion and never
block on their own.

## Already enforced — do not report

These run in `.githooks/pre-commit` and CI. A diff that violates one cannot
merge, so reporting it wastes the review.

| Gate | Covers |
| --- | --- |
| `cargo fmt --all -- --check` | all formatting |
| `cargo clippy --all-targets --all-features -- -D warnings` | every default clippy lint, plus `unsafe_code = "forbid"`, `unwrap_used`, `expect_used`, `panic`, `unreachable`, `todo`, `unimplemented`, `dbg_macro`, and `allow_attributes_without_reason` |
| `ast-grep scan --config sgconfig.yml` | `no-direct-filesystem-delete` and `no-unowned-filesystem-delete-in-owner-module` (which pin the sink signature `fn remove_path(target: VerifiedTarget, permanent: bool, expected: FileIdentity)` and sanction `crate::ops::remove_test_path` as the only test escape hatch); `no-shell-invocation`; `no-abbreviated-bindings` |
| `cargo test --all-targets --all-features` | 170 tests |
| `rustup run 1.88.0 cargo test` | MSRV |
| `cargo audit` (root and `fuzz/`) | advisories |
| five `cargo fuzz` targets | path validation, size and probe parsers, config parsing |
| `cmp -s AGENTS.md CLAUDE.md` | the two agent docs are byte-identical |

Both new rules have a stated blind spot, and the blind spot is where review
still has work to do. `no-abbreviated-bindings` checks binding positions — `let`
patterns, closure parameters, function parameter patterns, tuple, struct, and
tuple-struct patterns — against a fixed denylist, so an abbreviation outside
that list is S6's job. `no-shell-invocation` matches the program name only where
it appears as a literal at the call site; a shell reached through a variable is
invisible to it, which is S12's job.

## Inherited invariants — cite, do not restate

The architecture invariants are documented once, elsewhere. Check the diff
against them and cite the source; do not copy them here.

| If the diff touches | Cite |
| --- | --- |
| the deletion path, `VerifiedTarget`, `src/safety.rs` | `SECURITY.md` § Defense layers 4, 5, 5b |
| a new or changed `Action`, `CommandAuthority`, or confirmation flag | `CLAUDE.md` § Conventions — flags gate, never widen |
| probes, parsers, size measurement | `SECURITY.md` § Defense layers 10, 12 |
| `src/tui.rs` | `CLAUDE.md` § Conventions (presentation adapter only) and `DESIGN.md` § Terminal interface |
| journal records | `SECURITY.md` § Defense layers 13 |
| `src/ops/artifacts.rs` name matching | `CLAUDE.md` § Conventions — closed corroborated-name list |

## Standards

### S1 — Tautological and vacuous tests

**Hard.** An assertion the setup already guarantees, or a negative assertion
with nothing proving the mechanism fired. A test that cannot fail is worse than
no test: it reports coverage it does not have.

Fix: assert the observable outcome, and pair every negative assertion with a
positive control. Precedent, by searchable phrase: a surviving sentinel in
`src/ops/mod.rs` (`"keep"`), proof the stub actually ran in `tests/cli.rs`
(`the fake npm owner command was not exercised`), and a real existing parent so
the fuzz oracle is not vacuously false in `fuzz/fuzz_targets/validate_path.rs`
(`prevents the filesystem-dependent validator`).

### S2 — Change-detector tests

**Hard.** A test that mirrors *how* the code works instead of *what* it does:
asserting on `Finding` internals, re-deriving the danger arithmetic in the test,
pinning an exact `note` string, or checking that a helper was called. It breaks
on a refactor that preserved behaviour, and stays green when the behaviour
actually breaks.

Fix: assert the observable contract — exit code, the single JSON document, what
survives on disk — and use a real fake process (`sandbox.script(…)`) rather than
a mirror of the call graph.

Deliberate exception: `rules/no-unowned-filesystem-delete-in-owner-module.yml`
pins `remove_path`'s exact signature on purpose, because there the shape *is*
the contract. Say so in the test when you mean it.

### S3 — Comments carry only what the code cannot

**Hard.** A comment restating the statement below it.

Fix: delete it, or replace it with the constraint, threat, or platform reality
that forced the code. Precedent: the residual Trash rename window
(`src/ops/mod.rs`, `a residual rename window`), case-insensitive filesystems
forcing denylist ordering (`src/ops/artifacts.rs`, `case-insensitive and
case-preserving`), Darwin's transient `ENOENT` under a create race
(`src/journal.rs`, `transiently report ENOENT`).

### S4 — Every module opens with a `//!` contract line

**Hard.** A new file under `src/` without one. All 20 existing modules have one,
and several state a prohibition — `src/ops/xcode.rs`: "Archives are deliberately
exempt release artifacts."

Fix: one or two lines naming what the module owns and what it must never do.

### S5 — A deliberate panic carries its proof

**Hard.** Clippy now requires an `#[allow(…, reason = …)]` to panic at all, so
the remaining risk is a reason that restates the macro instead of proving the
branch unreachable.

Fix: the reason must say *why* control cannot arrive here, as the
`context-free commands return earlier in run()` arm in `src/app.rs` does.

### S6 — Names are spelled out

**Judgement.** An abbreviation outside the gated denylist: `op`, `f`, `n`, `s`,
`cfg`, `buf`, `ctx` in a new position.

Fix: `operation`, `file`, `count`, `source`, `configuration`, `buffer`. Only
serialized JSON keys keep short forms (`Summary.op`), and `ctx` is established
for the existing `Ctx` parameter.

### S7 — Errors are anyhow, refusal reason first

**Hard.** A new error path that invents an error type, drops context, or
reports a failure without saying what was refused.

Fix: `bail!("refusing …: {}", path.display())` for a policy refusal,
`.with_context(|| format!("cannot …: {}", path.display()))` for I/O, and
surface with `{error:#}` so the chain survives into JSON.

### S8 — Failure is refusal, not omission

**Hard.** A probe, parse, metadata read, or arithmetic overflow that yields a
smaller plan and exit 0. A silently shorter plan is indistinguishable from a
clean machine.

Fix: block the affected findings and report the error. Only
`ErrorKind::NotFound` may become an empty success (`src/safety.rs`). Measured
bytes use `checked_add` and error on overflow (`dir_size` in `src/safety.rs`);
saturating arithmetic is for display aggregation only (`actionable_bytes` in
`src/report.rs`).

### S9 — The crate surface stays closed

**Hard.** A new `pub` item escaping the crate. `src/lib.rs` exposes only
`pub mod app` and the `#[cfg(fuzzing)] fuzz_api`.

Fix: `pub(crate)`. A fuzz target gets a thin bool-returning re-export in
`fuzz_api`, not a widened module.

### S10 — Tests run the real binary in a disposable home

**Hard.** An integration test touching the developer's real `HOME`, real
`PATH`, or the network.

Fix: use the `Sandbox` harness (`tests/cli.rs`): a unique temp directory,
`env!("CARGO_BIN_EXE_devtrim")`, `HOME` and `PATH` pinned to the sandbox,
`XDG_STATE_HOME` removed, and every external binary stubbed with
`sandbox.script(…)`. A new `proptest!` block pins `rng_seed` and sets
`failure_persistence: None`, as the block in `src/safety.rs` does.

### S11 — A change lands with its docs and its justification

**Hard.** A new or changed flag, subcommand, JSON field, or exit-code path
without `README.md`, `MANUAL.html`, and `CHANGELOG.md` in the same commit. Or a
new `Cargo.toml` dependency with no note on why the standard library or an
existing dependency cannot do it — the bar is `next_quarantine_name` in
`src/ops/mod.rs`, which uses `RandomState`'s SipHash keys "without adding a
dependency."

Fix: update all three documents; justify the dependency in the diff and record
it in `SECURITY.md` § Supply chain. `CLAUDE.md` and `AGENTS.md` change together
or not at all.

### S12 — Process execution names a fixed program

**Hard.** `Command::new` reached through a variable whose value is not a
`&'static str` from a closed enum. `no-shell-invocation` only sees a literal at
the call site, so `let program = "sh"; Command::new(program)` passes the lint.

Fix: name the program as a literal at the call site, or take it from
`CommandAuthority::parts()`. The existing dynamic call sites in
`src/ops/docker.rs`, `src/ops/simulators.rs`, and `src/ops/caches.rs` are
approved because their program comes from a closed enum or an owner namespace,
and `repo_last_commit_with` takes `"git"` from its only production caller.

## Adding a rule

Add one rule per review miss you actually observed — not per rule you can
imagine. If a gate can check it, write the gate instead: a clippy lint in
`Cargo.toml` or an ast-grep rule in `rules/` with a positive control in
`rule-tests/`. Delete a rule from this file the moment a gate absorbs it.
