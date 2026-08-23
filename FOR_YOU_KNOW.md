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
use fixed argv allowlists, never shell strings. Docker volumes, Xcode Archives,
and whole worktrees are deliberately outside the deletion product.

The easy mistake is to confuse “previewed” with “authorized forever.” Apply
rechecks facts that can change: Git activity, toolchain references, cache
namespace, and physical path safety. If the second target fails after the first
succeeds, `ApplyOutcome` reports the first success, includes the error, stops,
and exits nonzero. Trash is the recovery mechanism; there is no rollback journal.

The 0.2.x lesson was sharp: six tests existed, but none touched the protected
deletion sink, and the MSRV note described a gate that had not executed. In
0.3.0, property tests attack the path invariant and the structural rule proves
it can catch a planted direct delete before trusting a clean scan. A gate that
cannot run fails the release.

The remaining honest limitation is pathname TOCTOU. `VerifiedTarget` proves
the checks ran; it does not hold an open directory descriptor through deletion.
The threat model assumes a single-user local tool without hostile concurrent
filesystem mutation. A future capability-filesystem design should close that
window instead of pretending this type already does.
