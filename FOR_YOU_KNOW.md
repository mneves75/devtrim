# FOR_YOU_KNOW

devtrim is a cautious bouncer for developer-machine disk space. Scanners can
nominate candidates, but they do not hold the keys to the exit. Every category
builds typed findings; confirmation shows exactly those findings; apply may
consume only their stored targets.

The most important seam is `src/ops/mod.rs`. Think of
`safety::VerifiedTarget` as a wristband issued at the last checkpoint:
`PathBuf` values cannot enter the deletion sink until literal protection,
existing-parent resolution, and symlink-ancestor checks pass. The wristband's
constructor is private, and ast-grep rejects new side doors. Human/JSON path
strings are labels on the wristband, never the identity used for removal.

Each cleanup category lives in `src/ops/` and implements `Op`. Scans stay
read-only. Filesystem operations default to Trash; `Action::Shred` is the
only typed instruction for permanent removal. Docker and simulator operations
use fixed argv allowlists, never shell strings. A serialized `Action::Command`
is only a label: `Finding::command` must also issue a private, closed
`CommandAuthority`, and apply rejects the finding unless both forms describe
the same allowlisted operation. Docker volumes, Xcode Archives, and whole
worktrees are deliberately outside the deletion product.

The easy mistake is to confuse “previewed” with “authorized forever.” Apply
rechecks facts that can change: Git activity, toolchain references, cache
namespace, and physical path safety. If the second target fails after the first
succeeds, `ApplyOutcome` reports the first success, includes the error, stops,
and exits nonzero. Trash is the recovery mechanism; the apply journal is an
audit trail of what was attempted and what completed, not a rollback engine.

The 0.2.x lesson was sharp: six tests existed, but none touched the protected
deletion sink, and the MSRV note described a gate that had not executed. In
0.3.0, property tests attack the path invariant and the structural rule proves
it can catch a planted direct delete before trusting a clean scan. A gate that
cannot run fails the release.

The 0.3.1 lesson is that authority is broader than the final delete call. A
directory size influences danger and the decision to act, so an unreadable
entry cannot be silently discarded. A config key influences policy, so a typo
cannot be silently ignored. A Docker size influences the same plan, so its unit
and numeric range must be proven. These parsers and measurements now return
errors at their shared boundaries instead of manufacturing plausible data.

Release candidates follow the same rule. A beta is built on GitHub's hosted
arm64 runner, checksummed, bound to its source commit with signed provenance,
and published as an immutable release. Production does not rebuild it; it
promotes the exact verified bytes. Think of staging as inspecting a sealed
shipping crate, not inspecting one crate and sending a newly packed lookalike.

Pathname TOCTOU was the honest limitation for three releases, and 0.6.0
finally anchored it: the sink now holds an open parent-directory handle
through identity verification and deletion, and permanent deletes quarantine
the verified entry before removing it. What is still path-based — parent
resolution and the recoverable move to Trash — is written down in SECURITY.md
rather than papered over. The threat model still assumes a single-user local
tool; the difference is how little now depends on that assumption.

Protected roots need special care on macOS's commonly case-insensitive
filesystems. The shared boundary compares fixed ASCII path components without
case, so `/system`, `/system/tmp`, and `~/.SSH` are refused before canonicalizing
the existing parent. Component-wise matching avoids treating a
different name such as `/systematic` as `/System`. The test belongs at this sink
because every current and future operation inherits the same refusal.

Human consent is a separate boundary from path authority. Every human apply
states the data-loss and AS-IS terms before mutation, and every interactive
danger level confirms. `-y` accepts risk and skips normal y/N prompts, while
only `--yolo` skips critical interactive prompts. Neither flag removes an
operation-specific acknowledgment such as `trash-empty --confirm=<gb>`.
JSON receives no prose because its one-document contract is an automation
boundary, not an interactive disclaimer surface.

`trash-empty` used to preview one aggregate `~/.Trash` finding and enumerate
its children only after confirmation. That broke the immutable-plan promise:
a newly trashed file could join the purge without appearing in the preview.
The scanner now records each exact top-level child, and apply consumes only
those findings. Presentation also escapes terminal controls and bidi controls;
the exact internal `PathBuf` remains untouched and is never reconstructed from
the safe display string.

The 0.4.0 TUI is another adapter, not another cleanup engine. `src/tui.rs`
asks the existing `Op` implementations to scan and apply, renders their stored
findings with Ratatui, and obtains a typed approval that must match the current
danger requirement. It cannot accept `-y`, `--yolo`, `--apply`, or `--shred`
from the command line as prepaid consent. Switching to permanent mode changes
only already-previewed `Trash` actions, then the same owner checks and private
`VerifiedTarget` sink run.

The alternate screen owns its diagnostics as well as its pixels. Scanners send
warnings through `Ctx`: explicit CLI commands render them to stderr, while the
TUI retains and escapes them in its results. Long outcomes scroll to the final
error, and a viewport below 64×18 accepts only quit input so an invisible
confirmation cannot authorize deletion.

Release authority follows the same separation. The job that checks out source
and runs Cargo has only read access and cannot mint OIDC credentials. A second
job receives the packaged archive, checksum, and notes; it never checks out or
compiles repository code and alone can attest or publish. That boundary limits
what a compromised build script can steal even though reviewed build output is
still what ultimately ships.

The compact keyboard menu is behaviorally inspired by
[Mole](https://github.com/tw93/mole), not ported from it. Mole is GPL-3.0;
devtrim remains Apache-2.0 and uses original Rust code over Ratatui. No Mole
source, tests, or visual assets are copied into this repository.

0.5.0 exists because of a comparison audit against Mole's public incident
record. Mole's 2026 data-loss bugs shared one root: safety rails wired
per-script, where one entry point can forget the whitelist. devtrim answered
with features that all route through the existing choke points. `artifacts` is
`node_modules` generalized — a closed name list that additionally demands
ecosystem corroboration (`target` beside `Cargo.toml`, `.venv` containing
`pyvenv.cfg`, an exact `CACHEDIR.TAG` signature) so an unlucky name alone is
never authority, and ambiguous names like `build` and `dist` are refused on
principle. The `protect` config list is enforced inside
`validate_path_for_deletion` itself, so no future op can forget it. The
write-ahead journal brackets every deletion with an attempt record and a
result record; if the journal cannot be written, the apply refuses to run,
because a safety tool that cannot remember what it did should not act.
Liveness guards ask `pgrep`/`lsof` whether a build owns the repo before
touching it, and a probe that fails blocks rather than passes.

0.6.0 attacked the limitation every honest release note had carried: pathname
TOCTOU. The research trail leads straight through Rust's own CVE-2022-21658 —
check a path, then delete by path, and an attacker who swaps the path between
the two steps deletes something else. The fix is the same shape Rust std and
GNU rm adopted: stop trusting names at deletion time. Every finding now
remembers its target's device and inode from the moment it was previewed, and
the sink opens the parent directory as a handle (cap-std), re-reads the
identity through that handle, and deletes through that same handle. Renaming
the target after preview no longer redirects the deletion — it refuses it.
Permanent deletes go one step further: the verified entry is first renamed to
a private quarantine name nobody else knows, verified again, and only then
deleted — so even a swap in the microseconds between check and delete hits
the quarantine wall instead of your data.
Two things stay path-based, and the docs say so instead of hiding it: parent
resolution, and the Trash call, because macOS simply has no fd-anchored Trash
API. The journal also grew up: rotation is shift-and-rename at startup only —
never truncation, the bash-history mistake — so an in-flight apply's records
can never be clipped, and each record is synced to disk before devtrim claims
success. Shipping binaries got an explicit ad-hoc signature in the release
workflow, and notarization is written down as a credential runbook rather
than pretended.

0.6.1 is the release where the surrounding promises caught up with that sink.
The adversarial review found that “reject a worktree root” was weaker than
“never delete a whole worktree”: a valid cache could contain a nested checkout.
Permanent preflight and removal now inspect every directory for a `.git`
marker, and a positive-control test proves the target is restored untouched.
The same review attacked the journal as a filesystem boundary, not merely a
logging feature. It now opens state directories component by component without
following symlinks, serializes each complete synced JSONL append, keeps an
attempt/result pair out of rotation, reverse-scans only the bounded newest
history tail needed for the requested output, reconciles legacy pairs across
rotations, waits for live guarded applies, and makes `history` genuinely
read-only.

The release chain applies the same authority separation. Before tagging, the
local script does only provenance checks; it never runs Cargo, npm, or the
product in that privileged preflight. Read-only hosted jobs rerun deterministic gates, both
dependency audits, all five fuzzers, PTY/UI, video, and secret scans. Only after
they pass does a job with no source checkout receive release-write and OIDC
authority. Production still promotes the exact immutable beta archive, so
reviewing one sealed crate cannot accidentally ship a newly packed lookalike.
Only then does the local closeout update Homebrew: it verifies the sealed crate
again, changes the tap's formula URL and checksum without rewriting its body,
pushes normally, and refuses to run the local upgrade unless the tap still
points at that exact commit. A failed closeout is resumed with the idempotent
Homebrew helper; neither the production tag nor release is moved.

The first implementation used Ratatui 0.29 to preserve Rust 1.85, but its
mandatory `lru 0.12.5` dependency later failed the 2026 security review with two
RustSec soundness advisories. Ratatui 0.30 makes its layout cache optional, so
devtrim moved to Ratatui 0.30.2/Crossterm 0.29, resolves patched `lru 0.18.2`,
disabled default features, and raised the proven compiler floor only to Rust
1.88. The safer graph won over compatibility; vendoring unsafe cache code
would have been the wrong trade.
