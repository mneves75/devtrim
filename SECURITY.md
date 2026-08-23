# Security Policy and Design

## Scope

devtrim is a local macOS cleanup CLI. Its primary security risk is unintended
local data loss, not remote attack. It does not run a server, accept network
requests, use credentials, or send telemetry. Runtime network traffic can still
be initiated by owner tools such as Docker when the user explicitly applies a
command action.

Supported security fixes target the latest release and `master`.

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
- Apply uses only exact previewed findings.
- Filesystem targets go to Trash unless permanent deletion is explicitly shown.
- Literal and physically resolved parents must agree; symlinked ancestors fail closed.
- System roots, the user home root, Trash root, `.ssh`, `.gnupg`, and wholesale
  `~/Library` are protected. Only named managed Library subpaths are eligible.
- Unknown Git activity or toolchain ownership is not deletion authority.
- Docker volumes and Xcode Archives are never pruned.
- Confirmation bypasses never add operations.
- Failed owner commands return nonzero and are not counted as reclaimed work.

## Defense layers

1. **Preview/apply split** — default invocations are read-only.
2. **Typed actions** — argv is stored separately from display text; no shell
   command strings are evaluated.
3. **Immutable candidates** — existing scan roots are canonicalized before
   preview, and apply does not rediscover filesystem targets.
4. **Physical path validation** — deletion validates literal policy and the
   canonical existing parent immediately before mutation. Resolution is
   deny-only and cannot turn a refused spelling into permission.
5. **Trash-first recovery** — normal filesystem removal uses macOS Trash.
6. **Danger and non-TTY gates** — aggregate size can increase confirmation;
   unattended mutation requires explicit consent.
7. **Truthful automation** — JSON is one document; partial/failed operations
   return nonzero with errors.
8. **Regression gates** — macOS CI runs format, strict Clippy, tests, MSRV tests,
   dependency audit, and an explicit arm64 release build.

## Supply chain

- `Cargo.lock` is committed and release builds use `--locked`.
- Rust is pinned in `rust-toolchain.toml`; `rust-version` records the MSRV.
- GitHub Actions are pinned to immutable commit SHAs.
- Dependabot checks Cargo and Actions weekly.
- Release archives include SHA-256 checksums and the full Apache-2.0 license.
- `cargo audit`, Gitleaks, and TruffleHog were clean before v0.2.1.

## Known limitations

- Sizes are estimated logical bytes, not guaranteed immediately reclaimable
  APFS blocks. Clones, sparse files, Trash, and container VM compaction differ.
- Path validation closes symlinked-ancestor redirection but does not claim a
  transaction across arbitrary concurrent hostile filesystem mutation. devtrim
  is a single-user local tool; when identity cannot be proven it refuses.
- Targets skipped because their state could not be proven are reported on
  stderr, not inside the JSON envelope. A JSON consumer therefore sees a
  smaller plan rather than an explicit skip list.
- The `trash` crate and Finder behavior depend on macOS permissions and volume
  support. Trash purge is permanent once explicitly applied.
- External commands can hang or change behavior across installed tool versions;
  broad timeout/process frameworks are deferred until a measured need exists.
- `leftovers` is intentionally report-only because worktree or mission
  staleness cannot be proven from names or branch state.
- Release binaries are checksummed but not currently code-signed or notarized.
