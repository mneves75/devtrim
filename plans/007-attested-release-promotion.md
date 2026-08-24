# Plan 007 — Attested beta-to-production promotion

Status: DONE

## Problem

The first `v0.3.0-beta1` prerelease proved the checksum and architecture, but
it was mutable, had no attestation, and was built locally. The production path
would rebuild the binary, so staging did not prove the exact bytes later sent
to users.

## Decision

Build beta artifacts once on a pinned GitHub-hosted arm64 workflow, generate
signed SLSA provenance, publish an immutable prerelease, then promote the exact
verified ZIP and checksum to the final immutable release from the same
dereferenced tag commit. The local script owns validation and annotated-tag
creation; the hosted workflow alone owns build, attestation, and publication.

## Ten alternatives considered

1. Keep local beta and production rebuilds — rejected because equal source does not prove equal artifacts.
2. Keep local builds and add SHA-256 only — rejected because a checksum detects change but does not prove builder or source.
3. Put `-betaN` in `Cargo.toml`, then commit the final version — rejected because staging and production would use different commits and artifacts.
4. Rebuild production from the beta commit in Actions — rejected because it still tests one artifact and distributes another.
5. Require byte-reproducible independent beta/final builds — deferred; valuable, but promotion avoids making reproducibility a release prerequisite.
6. Sign locally with a long-lived GPG key — rejected because it moves trust to a developer workstation and persistent secret.
7. Add Apple signing/notarization without changing the pipeline — deferred; distribution trust improves, but provenance and promotion remain unsolved.
8. Adopt `cargo-dist` immediately — deferred; strong future option, but migration is larger than the single-target release surface needs today.
9. Publish only a source tag and require users to build — rejected because it removes the supported arm64 binary deliverable.
10. Hosted beta build + signed provenance + immutable exact-byte promotion — chosen as the smallest design that closes builder, tampering, and rebuild drift together.

## Evidence and invariants

- A prerelease tag uses SemVer's lower-precedence prerelease form, while the
  promoted archive keeps the final base-version filename so its bytes do not
  change during promotion.
- Annotated tags must dereference to the workflow commit; `targetCommitish`
  is not accepted as identity.
- Final selection sorts `betaN` numerically and accepts only immutable,
  non-draft prereleases whose tag commit, asset set, checksum, release
  attestation, and artifact provenance all verify.
- Final publication never runs `cargo build`.
- A failed tag is never moved or reused. A failed beta attempt consumes its
  counter; a final workflow is retried only after its failure is understood.

Primary references:

- [Semantic Versioning 2.0.0](https://semver.org/)
- [GitHub immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases)
- [GitHub release creation](https://cli.github.com/manual/gh_release_create)
- [GitHub release verification](https://cli.github.com/manual/gh_release_verify)
- [GitHub artifact verification](https://cli.github.com/manual/gh_attestation_verify)
- [actions/attest](https://github.com/actions/attest)
- [SLSA provenance](https://slsa.dev/spec/v1.2/provenance)
- [SLSA artifact verification](https://slsa.dev/spec/v1.2/verifying-artifacts)
- [Cargo package versions](https://doc.rust-lang.org/cargo/reference/manifest.html#the-version-field)

## Five-year view

The next mature step is not another shell layer. Move the release graph to a
well-maintained generator such as `cargo-dist` once devtrim ships more targets,
then add Apple signing/notarization, an SBOM attestation, and an independent
reproducibility check. Keep exact-byte promotion as the invariant even when the
orchestrator changes.

## Done when

- Immutable releases are enabled before the next beta tag.
- `.github/workflows/release.yml` passes `actionlint` and positive/negative release tests.
- `scripts/release.sh` creates only annotated tags and waits for hosted verification.
- A new beta is immutable; `gh release verify` and `gh attestation verify` pass.
- Production promotion downloads the same beta ZIP digest and does not rebuild.

`v0.3.0-beta2` satisfied the hosted-build, immutable-release, checksum,
attestation, and exact-commit criteria. Production promotion remains gated by
an explicit release decision rather than plan completion.
