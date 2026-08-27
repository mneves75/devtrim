# Security Policy and Design

## Scope

devtrim is a local macOS cleanup CLI. Its primary security risk is unintended
local data loss, not remote attack. It does not run a server, accept network
requests, use credentials, or send telemetry. Runtime network traffic can still
be initiated by owner tools such as Docker when the user explicitly applies a
command action.

Supported security fixes target the latest release and `main`.

## Reporting

Report suspected vulnerabilities privately through GitHub Security Advisories
for `mneves75/devtrim`, or email the repository owner if private reporting is
unavailable. Do not publish an unpatched deletion-boundary bypass in a public
issue.

Useful reports include the devtrim/macOS version, exact command, preview output,
whether `--apply`/`--shred` was used, and whether symlinks, custom roots, Git,
Docker, Xcode, or Trash were involved.

## Threat model

We assume the invoking user intentionally runs devtrim but can make mistakes,
have stale config, or have paths change between inspection and action. We do
not defend against a user who modifies the binary/source to remove safeguards,
or a fully compromised host.

Non-negotiable boundaries:

- Every mutation requires `--apply`.
- Apply uses only exact previewed findings and preserves their exact non-lossy path identity.
- Filesystem targets go to Trash unless permanent deletion is explicitly shown; apply derives the mode from that typed preview action.
- Literal and physically resolved parents must agree; symlinked ancestors fail closed.
- System roots and descendants (including ASCII case variants), the user home
  root, Trash root, `.ssh`, `.gnupg`, and wholesale `~/Library` are protected.
  Only named managed Library subpaths are eligible.
- Unknown Git activity or toolchain ownership is not deletion authority.
- Permanent recursive deletion refuses foreign filesystem devices and a Git
  repository/worktree marker at any depth; a cache cannot carry a nested
  worktree across the deletion boundary.
- A repo owning the working directory of a running build/package process, and
  DerivedData while `xcodebuild` runs, are refused. Liveness probes use fixed
  argv `pgrep`/`lsof`; a probe that cannot complete blocks instead of passing.
- User-configured `protect` paths are refused at the deletion sink (literal and
  resolved, ASCII-case-insensitive and Unicode-normalization-insensitive, so
  NFC config text still protects NFD on-disk names) and filtered from previews;
  malformed entries are a configuration error and unresolved entries warn.
- Every deletion and fixed-argv command is journaled write-ahead (attempt
  before, result after) to a mode-0600 file in a mode-0700 state directory,
  with every path component opened without following symlinks. Each complete
  JSONL record is exclusively serialized through `sync_data`; an unwritable
  journal blocks the apply. `history` creates nothing, takes an exclusive
  read-only snapshot lock so guarded work cannot appear interrupted, pairs
  legacy records across generations, reverse-scans only a bounded newest tail,
  caps each line and total scanned bytes, and reports genuinely unmatched
  attempts as interrupted.
  Rotation is writer-owned, shift-and-rename under an advisory flock that dies
  with its process, re-checks size while holding the lock, and happens only at
  journal-open time — never mid-apply, never by truncation. An apply holds
  rotation coordination from attempt through result, so the pair stays in one
  generation.
- `artifacts` requires both a closed directory-name list with ecosystem
  corroboration (or an exact `CACHEDIR.TAG` signature) and a conclusively stale
  owning Git repo; corroboration, ownership, staleness, and liveness are all
  re-verified at apply time.
- Incomplete directory traversal, metadata, or numeric parsing is not size authority for an actionable plan.
- Unknown configuration fields are rejected so a misspelled safety setting cannot appear active.
- Docker volumes and Xcode Archives are never pruned.
- A serialized command action is not execution authority. Only the closed internal `CommandAuthority` capability can authorize one of the fixed Docker or simulator argv variants, and apply must match both representations exactly.
- Confirmation bypasses never add operations.
- Every human apply displays a data-loss warning. Interactive mutation confirms at every danger level; `-y` skips normal y/N only, `--yolo` skips interactive prompts but not operation-specific acknowledgments, and JSON stays machine-only.
- The TUI accepts no CLI confirmation bypass. Its internal approval must match the current preview and danger requirement; permanent actions use typed size confirmation, Trash purge uses `PURGE <gb>`, and undersized terminals cannot submit hidden confirmations.
- Owner-reported cache roots are limited to the reporting program's exact namespace and revalidated at apply time.
- Terminal-facing findings, errors, and outcome notes escape control and bidirectional-control characters before rendering; internal paths remain typed `PathBuf` values.
- Failed or partial work returns nonzero; successful earlier actions remain visible in the summary.

## Defense layers

1. **Preview/apply split** — default invocations are read-only.
2. **Typed actions and command authority** — argv is stored separately from display text; no shell command strings are evaluated, and a private closed capability must match each executable action before fixed-argument dispatch.
3. **Immutable candidates** — existing scan roots are canonicalized before
   preview, and apply does not rediscover filesystem targets.
4. **Typed deletion capability** — display paths are presentation only. The exact internal `PathBuf` must pass validation to become a private `VerifiedTarget`, which alone can reach physical removal.
5. **Physical path validation** — deletion validates literal policy and the canonical existing parent immediately before mutation. Resolution is deny-only and cannot turn a refused spelling into permission.
5b. **Anchored identity verification** — the sink re-reads the target's
   preview-time `(device, inode, generation)` on macOS through an open
   parent-directory handle and deletes through that same handle; identity
   drift refuses the deletion.
   Permanent deletes quarantine the verified leaf under a private
   unpredictable name, re-verify, refuse device crossings and Git markers at
   every depth, and drive recursive deletion through open handles.
6. **Trash-first recovery** — normal filesystem removal uses macOS Trash.
7. **Risk, danger, and non-TTY gates** — human apply displays the AS-IS/data-loss notice, every interactive mutation confirms, aggregate size can require typed input, and unattended mutation requires explicit consent.
8. **TUI authorization** — Ratatui renders the existing findings; a separate typed approval capability must still match that exact plan before the existing `Op::apply` owner runs.
9. **Truthful automation** — JSON is one document; partial/failed operations
   return nonzero with errors.
10. **Truthful measurement** — traversal, metadata, numeric parsing, and overflow
   errors block actionable plans rather than producing partial estimates.
11. **Terminal-safe presentation** — control characters are escaped before human rendering and never parsed back into deletion authority.
12. **Liveness guards** — running build processes (by working directory) and a
   running `xcodebuild` block the affected repo or DerivedData targets, failing
   closed when the probe itself fails.
13. **Write-ahead journal** — attempt/result records surround every deletion and
   fixed-argv command; symlink-safe parent handles, serialized appends, and
   bounded read-only history preserve a coherent local audit trail.
14. **Regression gates** — macOS CI runs format, strict Clippy, tests, MSRV tests,
   root and fuzz-lock dependency audits, a positive-control structural
   deletion-sink lint, and an explicit arm64 release build. Read-only hosted
   release jobs additionally run all five bounded fuzz targets, PTY/UI, video,
   workflow-policy, and secret-scanning gates before publication authority is
   available.

## Supply chain

- `Cargo.lock` is committed and release builds use `--locked`.
- Rust is pinned in `rust-toolchain.toml`; `rust-version` records the MSRV.
- GitHub Actions are pinned to immutable commit SHAs.
- Dependabot checks the root and fuzz Cargo graphs, the demo video's npm graph,
  and Actions weekly.
- Ratatui 0.30.2 and Crossterm 0.29 require Rust 1.88. Default Ratatui features stay disabled, including the optional layout cache; the graph resolves patched `lru 0.18.2` instead of the `0.12.5` affected by RUSTSEC-2026-0002 and RUSTSEC-2026-0253.
- Hosted release builds produce SHA-256 checksums, the full Apache-2.0 license, and signed artifact provenance.
- Hosted repository and dependency code runs only in read-only validation,
  fuzz, and release-preparation jobs. A separate publisher downloads packaged inputs,
  never checks out or compiles the repository, and alone holds release-write
  and OIDC permissions.
- GitHub releases and their tags/assets are immutable. Production promotes the exact verified beta archive from the same commit instead of rebuilding it.
- Production closeout independently re-verifies that immutable archive,
  checksum manifest, GitHub asset digest, and attestation before changing the
  Homebrew tap. The helper permits one formula-file commit through a normal
  push, pins local validation to that exact tap commit, scrubs GitHub credential
  variables from Homebrew execution, and requires strict audit, upgrade, test,
  and sole-path/version proof.
- Release preparation runs both Cargo lockfile audits, all five bounded fuzz
  targets, a strict clean npm install plus low-severity audit/lint/format/build
  gates for the demo video, Gitleaks, and TruffleHog; results are recorded in
  the matching changelog section only after they execute.
- Shipped HTML uses a deny-by-default CSP; inline scripts are admitted only by exact SHA-256 hashes.

## Known limitations

- Sizes are estimated logical bytes, not guaranteed immediately reclaimable
  APFS blocks. Clones, sparse files, Trash, and container VM compaction differ.
  The estimate is nevertheless complete for the traversed logical tree: an
  unreadable entry or overflow is an error, not a partial result.
- The typed target prevents unvalidated and lossy-display paths from reaching
  removal. Since 0.6.0 every finding records its target's `(device, inode)`
  identity at preview (including file generation on macOS), and the sink
  re-reads that identity through an open
  parent-directory handle immediately before deleting through the same handle
  (cap-std's dirfd-anchored implementation — the shape Rust std adopted after
  CVE-2022-21658). A target renamed or swapped after preview is refused, not
  followed. What remains path-based, stated plainly: resolving the parent
  directory itself, and the Trash call (macOS offers no fd-anchored Trash
  API) — identity is re-verified immediately before it, but removal is not
  atomic against a concurrent rename in that final window. devtrim is a
  single-user local tool; when identity cannot be proven it refuses. Permanent
  non-directory targets are finally unlinked by their private unpredictable
  quarantine name because macOS has no general remove-by-open-file API.
  Recursive deletion rechecks each entry, device, Git marker, and open
  directory identity, but a concurrent post-preflight change can still stop a
  partially completed tree; there is no rollback after deletion begins.
- The `trash` crate and Finder behavior depend on macOS permissions and volume
  support. Files & Folders, App Management, Automation, or Full Disk Access
  authorization is a manual user decision in System Settings; devtrim does not
  bypass it. Trash purge is permanent once explicitly applied.
- External commands can hang or change behavior across installed tool versions;
  broad timeout/process frameworks are deferred until a measured need exists.
- Liveness probes are point-in-time snapshots. A process can start after the
  final check; apply therefore still relies on immutable targets, identity
  checks, and conservative refusal rather than treating liveness as a lock.
- Journal files are bounded local audit data, not tamper-evident logs. A user or
  fully compromised host with write access can alter past records.
- `leftovers` is intentionally report-only because worktree or mission
  staleness cannot be proven from names or branch state.
- Release binaries are checksummed and carry an explicit ad-hoc code
  signature, verified in the release workflow (Apple silicon refuses wholly
  unsigned binaries; linker-only signatures have field reports of rejection).
  They are not Developer-ID-signed or notarized. In practice the primary
  install paths never quarantine — Homebrew formulae and `curl` do not set
  the quarantine attribute; only browser-downloaded archives trigger
  Gatekeeper, where right-click-Open or `xattr -d com.apple.quarantine`
  applies. Enabling notarization is a maintainer credential decision with a
  documented path: export the local "Developer ID Application" certificate,
  create `notarytool` credentials, sign with
  `codesign --options runtime --timestamp` and notarize the archive in a
  workflow job that never executes repository code (the signing secret must
  not be exposed to the build job), accepting that a bare Mach-O cannot be
  stapled, so first launch of a quarantined copy performs an online ticket
  check. Until those credentials exist in CI, the machinery is deliberately
  not implemented: an untestable signing path in the release chain is risk,
  not safety.
