#!/usr/bin/env bash
# devtrim release: verify → tag → hosted build/promotion → attest → GitHub release
# usage: scripts/release.sh <version>   (e.g. 0.3.0-beta1 or 0.3.0)
set -euo pipefail

release="${1:?usage: scripts/release.sh <version>}"
[[ $# -eq 1 ]] || { echo "ERROR: usage: scripts/release.sh <version>"; exit 1; }
[[ "$release" =~ ^([0-9]+\.[0-9]+\.[0-9]+)(-beta[1-9][0-9]*)?$ ]] || {
  echo "ERROR: version must be X.Y.Z or X.Y.Z-betaN (N starts at 1)"
  exit 1
}
ver="${BASH_REMATCH[1]}"
prerelease_suffix="${BASH_REMATCH[2]:-}"
tag="v${release}"
out="devtrim-${ver}-macos-arm64"
cd "$(dirname "$0")/.."

echo "==> verifying release state"
grep -Fqx "version = \"${ver}\"" Cargo.toml || { echo "ERROR: Cargo.toml version != ${ver}"; exit 1; }
grep -Fq "## [${ver}]" CHANGELOG.md || { echo "ERROR: no CHANGELOG.md section for ${ver}"; exit 1; }
grep -Fq "v${ver}" README.md || { echo "ERROR: README.md lacks v${ver}"; exit 1; }
grep -Fq "v${ver}" MANUAL.html || { echo "ERROR: MANUAL.html lacks v${ver}"; exit 1; }
grep -Fq "v${ver}" index.html || { echo "ERROR: index.html lacks v${ver}"; exit 1; }
[[ -z "$(git status --porcelain=v1 --untracked-files=all)" ]] || { echo "ERROR: commit all changes before releasing"; exit 1; }
upstream=$(git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null) || { echo "ERROR: current branch has no upstream"; exit 1; }
git fetch --quiet origin || { echo "ERROR: cannot reach origin to verify release state"; exit 1; }
[[ "$(git rev-parse HEAD)" == "$(git rev-parse "$upstream")" ]] || { echo "ERROR: push the release commit and sync with $upstream first"; exit 1; }
git rev-parse "$tag" >/dev/null 2>&1 && { echo "ERROR: local tag $tag already exists"; exit 1; }
remote_tag=$(git ls-remote --tags origin "refs/tags/${tag}") || { echo "ERROR: cannot query remote tags"; exit 1; }
[[ -z "$remote_tag" ]] || { echo "ERROR: remote tag $tag already exists"; exit 1; }
gh release view "$tag" --json tagName >/dev/null 2>&1 && { echo "ERROR: GitHub release $tag already exists"; exit 1; }
repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
immutable_enabled=$(gh api "repos/${repo}/immutable-releases" --jq .enabled)
[[ "$immutable_enabled" == "true" ]] || {
  echo "ERROR: GitHub immutable releases must be enabled before tagging"
  exit 1
}

echo "==> quality gates"
cargo fmt --all -- --check
command -v ast-grep >/dev/null 2>&1 || { echo "ERROR: ast-grep is required for release validation"; exit 1; }
ast-grep test --skip-snapshot-tests
ast-grep scan --config sgconfig.yml
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
command -v rustup >/dev/null 2>&1 || { echo "ERROR: rustup is required to execute the MSRV gate"; exit 1; }
rustup toolchain install 1.85.0 --profile minimal
rustup run 1.85.0 cargo test --locked --all-targets --all-features
release_commit=$(git rev-parse HEAD)
if ! ci_conclusion=$(gh run list --workflow ci.yml --event push --commit "$release_commit" --limit 1 --json conclusion,status --jq '.[0] | select(.status == "completed") | .conclusion'); then
  echo "ERROR: could not query CI for release commit ${release_commit}"
  echo "ACTION: verify gh authentication and run: gh run list --workflow ci.yml --commit ${release_commit}"
  exit 1
fi
if [[ "$ci_conclusion" != "success" ]]; then
  echo "ERROR: release commit ${release_commit} has no successful completed CI run"
  echo "ACTION: wait for or rerun CI on that exact pushed commit, then retry this release"
  exit 1
fi
cargo audit
bash -n scripts/release.sh
shellcheck scripts/release.sh
actionlint
cmp -s AGENTS.md CLAUDE.md || { echo "ERROR: AGENTS.md and CLAUDE.md differ"; exit 1; }

echo "==> tag + hosted release workflow"
gh auth status
git tag -a "$tag" -m "$tag"
if ! git push origin "$tag"; then
  echo "ERROR: local tag $tag was created but the push failed"
  echo "RECOVER: inspect git ls-remote --tags origin refs/tags/$tag before acting"
  echo "RECOVER: if absent, fix the push issue, delete the local tag with git tag -d $tag, and rerun; if present, create the GitHub release manually"
  exit 1
fi

run_id=""
for _ in {1..30}; do
  run_id=$(gh run list --workflow release.yml --event push --commit "$release_commit" --limit 20 \
    --json databaseId,headBranch \
    --jq ".[] | select(.headBranch == \"${tag}\") | .databaseId" | head -n 1)
  [[ -z "$run_id" ]] || break
  sleep 2
done
if [[ -z "$run_id" ]]; then
  echo "ERROR: release workflow did not start for $tag"
  echo "RECOVER: inspect Actions for the pushed tag; never move or reuse it"
  exit 1
fi
gh run watch "$run_id" --exit-status || {
  echo "ERROR: hosted release workflow failed for $tag"
  echo "RECOVER: inspect run ${run_id}; rerun it only after the failure is understood"
  exit 1
}

expected_prerelease=false
[[ -z "$prerelease_suffix" ]] || expected_prerelease=true
release_state=$(gh release view "$tag" --json tagName,isImmutable,isPrerelease \
  --jq '[.tagName, .isImmutable, .isPrerelease] | @tsv')
[[ "$release_state" == "$tag"$'\ttrue\t'"$expected_prerelease" ]] || {
  echo "ERROR: published release state does not match $tag"
  exit 1
}
gh release verify "$tag" --format json >/dev/null

verify_dir=$(mktemp -d "/tmp/devtrim-release-verify.XXXXXX")
trap 'rm -r "$verify_dir"' EXIT
gh release download "$tag" --dir "$verify_dir" --pattern "${out}.zip" --pattern SHA256SUMS.txt
( cd "$verify_dir" && shasum -a 256 -c SHA256SUMS.txt )
attestation_args=(--repo "$repo" --source-digest "$release_commit")
if [[ "$expected_prerelease" == "true" ]]; then
  attestation_args+=(--source-ref "refs/tags/${tag}")
fi
gh attestation verify "$verify_dir/${out}.zip" "${attestation_args[@]}" >/dev/null
echo "==> released and verified ${tag}"
