# Plan 008: Harden the 0.4.0 release boundary

This ExecPlan is a living document. Keep `Progress`, `Surprises & Discoveries`,
`Decision Log`, and `Outcomes & Retrospective` current as implementation and
verification proceed.

## Purpose / Big Picture

devtrim deletes developer data, so its release is acceptable only if the path
authority, preview-to-apply contract, build credentials, and public safety
claims are independently verifiable. This plan closes the concrete gaps found
in a fresh source and security review, finalizes the existing 0.4.0 development
line, ships an immutable `v0.4.0-betaN` prerelease as the staging surface, and
then stops for explicit production confirmation. Only after that confirmation
may the exact beta bytes be promoted to `v0.4.0` and the production site be
updated.

## Progress

- [x] (2026-08-24) Read repository guidance, the Rust rules, security playbook,
  release workflows, every Rust module, tests, and public release surfaces.
- [x] (2026-08-24) Run independent security, test-strategy, and release-surface
  reviews against commit `186856c`.
- [x] (2026-08-24) Prove and fix case-variant aliases at the protected-path boundary.
- [x] (2026-08-24) Add high-value approval and forged-authority regression tests.
- [x] (2026-08-24) Separate unprivileged build work from publication credentials in the
  release workflow and repair its missing Actions read permission.
- [x] (2026-08-24) Narrow the deletion structural-lint exception and prove the positive
  control catches a second sink.
- [x] (2026-08-24) Reconcile concurrent version/document edits and preserve the
  distinction between unreleased 0.4.0 source and the public 0.3.2 download.
- [x] (2026-08-24) Run focused proofs, the full local Rust gate, real
  terminal/browser checks, dependency/secret scans, and a fresh independent
  verifier; repair the verifier's positive-control lint finding.
- [x] (2026-08-24) Rebuild and inspect the arm64 release candidate, then rehearse
  the exact ZIP layout, checksum, packaged license, documentation, architecture,
  and version checks used by the release workflow.
- [x] (2026-08-25) Run two explicitly authorized P3 autoreview passes, verify
  their findings, and fix the accepted boundary defects.
- [x] (2026-08-25) Complete the final P3 closeout review and its one post-fix
  rerun. Fix both confirmed P2 retry defects and pass focused workflow proof;
  stop the model loop after the contracted rerun.
- [x] (2026-08-25) Pass final format, structural positive controls, strict
  Clippy, 61 unit tests, 18 CLI tests, exact Rust 1.88 tests, RustSec, npm,
  shell/workflow, history/worktree secret, arm64 build, PTY, browser, and exact
  package-rehearsal gates.
- [ ] Stage only reviewed files, create one conventional commit, push `master`,
  and prove local HEAD equals `origin/master` with a clean tree and green CI.
- [x] (2026-08-25) Preserve all historical immutable releases and tags as
  provenance instead of deleting or rewriting public history.
- [x] (2026-08-25) Install and verify the local arm64 v0.4.0 candidate while
  preserving the previous binary as a backup.
- [ ] Create and verify the next immutable `v0.4.0-betaN` release as staging,
  then stop for production confirmation.
- [ ] After explicit confirmation, promote the exact beta archive to `v0.4.0`,
  verify GitHub Release and GitHub Pages, then open the next Unreleased changelog.

## Surprises & Discoveries

- Observation: the release workflow executes Cargo build scripts and proc
  macros in the same job that holds repository-write and OIDC attestation
  authority.
  Evidence: `.github/workflows/release.yml` gives write scopes at the job level
  and later executes `cargo build`.
- Observation: the deletion lint exempts all of `src/ops/mod.rs`, so a future
  second sink in the owner module would pass CI.
  Evidence: `rules/no-direct-filesystem-delete.yml` ignores the whole file.
- Observation: protected names are matched case-sensitively even though the
  supported macOS default is commonly case-insensitive.
  Evidence: `is_protected_abs` uses ordinary `Path` equality for exact roots.
- Observation: another Codex process in the same checkout temporarily changed
  version and public-doc files while this plan was being prepared.
  Resolution: wait for that process to settle, preserve its completed TUI work,
  and restore the committed 0.4.0-development/0.3.2-production distinction.
- Observation: autoreview discloses the unpublished diff to the selected review
  engine.
  Resolution: the user explicitly authorized autoreview; two P3 passes ran and
  their accepted findings were fixed. The current combined diff receives one
  final closeout pass before commit.
- Observation: an independent positive-control review proved the deletion lint
  still exempted any same-signature sink in any Rust source file.
  Resolution: split the rule by file ownership, add a second owner-module rule,
  and prove a planted same-signature sink outside `src/ops/mod.rs` fails.
- Observation: the repository has no separate staging web environment.
  Resolution: use the repository's immutable GitHub prerelease as staging;
  GitHub Pages remains the production documentation surface.
- Observation: the split publisher originally reused release-existence state
  captured by `prepare`, so retrying only a failed publisher after creation
  attempted to recreate an immutable release.
  Resolution: refresh and validate remote release state inside `publish`; a
  retry now skips creation and proceeds to exact asset verification.
- Observation: a full workflow rerun retains `github.run_id`, while the pinned
  artifact action refuses an existing immutable artifact name by default.
  Resolution: allow overwrite only for the one-day intermediate handoff; its
  name remains stable so publisher-only retries can still download it.

## Decision Log

- Decision: finish the already-open 0.4.0 line rather than manufacture 0.4.1
  or roll the TUI back into 0.3.2.
  Rationale: Cargo metadata and the committed changelog already define 0.4.0 as
  the unreleased line; 0.3.2 is an immutable published release.
  Date/Author: 2026-08-24, Codex.
- Decision: keep the private `VerifiedTarget` boundary and make the smallest
  fail-closed correction at that boundary.
  Rationale: every filesystem operation already converges there, so a focused
  repair covers all current and future operations without a second deletion
  engine or new dependency.
  Date/Author: 2026-08-24, Codex.
- Decision: split build and publication jobs instead of merely unsetting one
  token around `cargo build`.
  Rationale: job-level OIDC/write permissions remain requestable by any process
  in that job; privilege separation is the actual security boundary.
  Date/Author: 2026-08-24, Codex.
- Decision: do not bypass the autoreview disclosure gate with another external
  engine or commit before it runs.
  Rationale: the user explicitly required autoreview and repository policy
  makes it a pre-commit closeout gate; a local substitute would not satisfy
  either requirement.
  Date/Author: 2026-08-24, Codex.
- Decision: preserve all published immutable tags and releases.
  Rationale: deleting or rewriting signed release history destroys provenance
  and conflicts with the repository's immutable-release security model.
  Date/Author: 2026-08-25, Codex.
- Decision: require a private `CommandAuthority` in addition to each serialized
  command action.
  Rationale: JSON/display state must not be able to forge execution authority;
  the closed capability binds apply to one reviewed fixed-argv variant.
  Date/Author: 2026-08-25, Codex.

## Context and Orientation

`src/safety.rs` converts an ordinary path into the private `VerifiedTarget`
capability after lexical, protected-root, and canonical-parent checks.
`src/ops/mod.rs` is the only production owner that consumes this capability and
invokes Trash or permanent filesystem deletion. Individual cleanup operations
under `src/ops/` produce typed findings and must apply only their exact preview.

`.github/workflows/release.yml` builds immutable beta artifacts and promotes
the exact attested bytes to production. The build must be unprivileged; only a
separate publication job may hold release and attestation permissions.
`rules/no-direct-filesystem-delete.yml` is a structural backstop ensuring no
new deletion call bypasses the shared sink.

The public release surfaces are `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`,
`README.md`, `MANUAL.html`, `index.html`, `SECURITY.md`,
`FOR_YOU_KNOW.md`, and project memory. `AGENTS.md` and `CLAUDE.md` change only
if their actual commands or invariants change; documentation churn is not a
release requirement.

The beta ZIP is also the production ZIP. Its packaged `README.md` and
`MANUAL.html` must therefore describe v0.4.0 without time-dependent “latest” or
“development” claims. `index.html` is not packaged and can continue advertising
the live v0.3.2 production download until explicit promotion.

## Threat Model and Security Acceptance

The protected assets are user data, system paths, release artifacts, and GitHub
publication authority. Attackers considered here are malformed or forged
internal findings, owner commands that return arbitrary paths, a same-user
filesystem race, a compromised Cargo dependency/build script, and a future
developer adding a second raw deletion call. The release is acceptable when:

1. protected path aliases fail closed at the single typed deletion boundary;
2. forged findings and approval-state changes cannot widen an approved action;
3. no job running repository or dependency code has write/OIDC permissions;
4. the structural rule demonstrably rejects an extra deletion sink;
5. dependency, secret, workflow, Rust, terminal, and browser gates pass on the
   exact commit that is pushed.

The known descriptor-free pathname TOCTOU, unsigned/not-notarized binary, and
external command identity drift remain documented limitations. This plan does
not silently claim to solve them.

## Alternatives Considered

1. Keep conventions only: rejected because the earlier protected-path bypass
   proved memory and review are not controls.
2. Rewrite every operation: rejected because the shared boundary already gives
   one auditable owner and a rewrite enlarges the regression surface.
3. Adopt descriptor-relative `cap-std` deletion now: deferred; capability IO is
   attractive, but directory rename/removal identity still needs a precise
   design and the present defect is refusal of an alias, not an internal
   `remove_dir_all` symlink traversal bug.
4. Add a transaction/rollback journal: rejected because normal cleanup already
   uses macOS Trash and a journal cannot make permanent purge reversible.
5. Add a user whitelist: rejected because it delegates a broken authority
   boundary to configuration.
6. Persist plans across invocations: rejected because approval already binds an
   immutable in-memory preview and extra state adds replay/staleness risk.
7. Port Mole's test suite or code: rejected because the GPL-3.0 surface and much
   broader product do not fit this Apache-2.0 CLI.
8. Leave the release job intact and unset `GH_TOKEN` for Cargo: rejected because
   job-level OIDC and token exposure are broader than one environment variable.
9. Add signing/notarization in this pass: deferred as valuable distribution
   work, but it does not repair deletion authority or CI privilege separation.
10. Chosen: repair each shared boundary once, prove it with negative and
    positive controls, stage the exact reviewed bytes as an immutable beta, and
    promote those bytes only after explicit production confirmation.

## Plan of Work

First add a regression in `src/safety.rs` that fails on protected roots,
descendants, and protected user directories spelled with case variants. Run
that one test before changing production code, then implement byte-preserving,
component-wise ASCII case-insensitive comparisons for the fixed protected
names. Preserve exact managed-namespace matching and prove a similarly prefixed
name such as `/systematic` is not misclassified.

Add narrowly targeted tests for TUI approval invalidation, read-only operation
application, owner-reported cache paths, forged cache authority, Xcode Archives,
and node_modules without a verified Git owner. Change production code only if
one of those tests exposes a real defect.

Refactor `.github/workflows/release.yml` into an unprivileged metadata/build job
and a dependent publisher job. The build job checks out with credentials
disabled, validates metadata/CI, builds and packages only a new beta, and
uploads an intermediate artifact. The publisher job downloads that artifact,
does not check out or execute repository/dependency code, and alone receives
contents/attestation/OIDC permissions. It handles beta attestation/publication
and final exact-byte promotion. Grant `actions: read` only where `gh run list`
requires it and scope `GH_TOKEN` to individual GitHub CLI steps.

Replace the whole-file deletion-lint ignore with a narrow structural allowance
for the approved sink implementation and test cleanup helper. Add or extend the
ast-grep positive-control fixture so a planted second deletion owner fails.

Once concurrent edits settle, reconcile rather than discard them: preserve the
0.4.0 release-candidate truth while the public download remains v0.3.2 during
staging, correct permissions/disclaimer details, date the explicit release
section, and record only claims proven by the final gate.

## Concrete Steps

Run from `/Users/mneves/dev/devtrim`:

    cargo test --locked safety::tests::protects_case_variant_aliases
    cargo test --locked --all-targets --all-features
    ast-grep test --skip-snapshot-tests
    ast-grep scan --config sgconfig.yml
    actionlint .github/workflows/release.yml

Then run the release gates documented in `AGENTS.md`, including exact MSRV,
RustSec, arm64 release build, Gitleaks, TruffleHog, release-shell validation,
real PTY cancellation against a disposable home, and desktop/mobile browser
checks. Run autoreview once with `--max-priority P3`; after any accepted fix,
rerun focused proof and autoreview once. A fresh verifier must inspect the final
diff and evidence before commit.

After the exact-commit CI is green, verify repository release immutability and
leave every historical tag and release intact. Select the next unused
`v0.4.0-betaN`, run `scripts/release.sh`, download the published archive,
verify checksum and attestation, and execute the packaged binary. Stop there
until the user explicitly confirms production promotion.

## Validation and Acceptance

The new protected-alias test must fail before the production fix and pass after
it. A positive-control structural fixture must prove a second sink is rejected;
a clean repository scan alone is insufficient. `actionlint` must pass, and a
manual permissions inspection must show no `cargo build` job with write or OIDC
authority and no publication job executing checked-out code.

All Rust tests, exact Rust 1.88 MSRV tests, format, strict Clippy, audit, arm64
build, shell/workflow validation, secret scans, PTY cancellation, and responsive
static-site checks must succeed on the final tree. The pushed commit is accepted
only when `git rev-parse HEAD` equals `git rev-parse origin/master` and
`git status -sb` is clean. Staging is accepted only when the beta release is
immutable, the release/tag dereference to that commit, its exact asset set,
checksum, attestation, architecture, version output, and cancellation behavior
all verify from a fresh download.

## Idempotence and Recovery

Tests, scans, builds, and browser checks are non-destructive and repeatable.
All deletion scenarios use disposable paths under the repository target or
temporary directories; no test may point at the real home Trash or developer
caches. The workflow change is inert until a future explicit tag push. If a
gate fails, keep the uncommitted diff, fix only the proven in-scope cause, and
rerun the affected proof before widening to the full gate.

Git staging lists exact paths. Historical releases and tags are read-only
provenance. Beta creation is the only deployment before the production gate;
no clean production tag, production release, or production site update occurs
without the post-staging confirmation.

## Interfaces and Dependencies

No new runtime dependency is planned. `VerifiedTarget` remains private and
constructible only by `validate_path_for_deletion`; `CommandAuthority` remains
private to findings and constructible only through `Finding::command`.
Ratatui/crossterm interfaces remain unchanged unless a failing regression
proves a defect. The demo-video development graph stays lockfile-pinned and
audited; GitHub Actions remain SHA-pinned.

## Outcomes & Retrospective

Implementation and local verification are complete. Confirmed findings closed
protected-path case aliases, command-action forgery, release privilege
separation, deletion-lint exemptions, publisher retry state, handoff-artifact
retry collisions, CSP, and the demo-video dependency gate. The final local ZIP
contains exactly the arm64 v0.4.0 binary, full Apache-2.0 license, README, and
manual; all match the source inputs and checksum. Commit/CI, immutable beta,
post-beta production confirmation, stable promotion, and the next Unreleased
section remain pending.
