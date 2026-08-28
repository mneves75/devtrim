#!/usr/bin/env bash
# devtrim release: preflight → tag → hosted gates/build/promotion → attest → release
# usage: scripts/release.sh <version>   (e.g. 0.4.0-beta1 or 0.4.0)
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
version_pattern="${ver//./\\.}"
first_release_heading=$(grep -m1 '^## \[' CHANGELOG.md || true)
[[ "$first_release_heading" =~ ^##\ \[$version_pattern\]\ -\ [0-9]{4}-[0-9]{2}-[0-9]{2}$ ]] || {
  echo "ERROR: first CHANGELOG.md release heading is not ${ver}"
  exit 1
}
grep -Fqx "This source tree and its packaged documentation describe devtrim v${ver}." README.md || {
  echo "ERROR: README.md source-tree version != v${ver}"
  exit 1
}
[[ "$(grep -Ec "^This source tree and its packaged documentation describe devtrim v[0-9]+\\.[0-9]+\\.[0-9]+\\.$" README.md)" -eq 1 ]] || {
  echo "ERROR: README.md must contain exactly one source-tree version declaration"
  exit 1
}
grep -Fqx "    <span class=\"chip g\">v${ver}</span>" MANUAL.html || {
  echo "ERROR: MANUAL.html version chip != v${ver}"
  exit 1
}
[[ "$(grep -Ec "^    <span class=\\\"chip g\\\">v[0-9]+\\.[0-9]+\\.[0-9]+</span>$" MANUAL.html)" -eq 1 ]] || {
  echo "ERROR: MANUAL.html must contain exactly one version chip"
  exit 1
}
grep -Fqx "  <span>devtrim <b>v${ver}</b></span>" MANUAL.html || {
  echo "ERROR: MANUAL.html footer version != v${ver}"
  exit 1
}
[[ "$(grep -Ec "^  <span>devtrim <b>v[0-9]+\\.[0-9]+\\.[0-9]+</b></span>$" MANUAL.html)" -eq 1 ]] || {
  echo "ERROR: MANUAL.html must contain exactly one footer version"
  exit 1
}
[[ -z "$(git -c core.fsmonitor=false -c submodule.recurse=false status --porcelain=v1 --untracked-files=all --ignore-submodules=all)" ]] || {
  echo "ERROR: commit all changes before releasing"
  exit 1
}

repo_info=$(gh repo view --json nameWithOwner,defaultBranchRef --jq '[.nameWithOwner, .defaultBranchRef.name] | @tsv') || {
  echo "ERROR: cannot resolve the origin repository and its default branch"
  exit 1
}
IFS=$'\t' read -r repo default_branch <<< "$repo_info"
[[ -n "$repo" && -n "$default_branch" ]] || {
  echo "ERROR: origin repository metadata is incomplete"
  exit 1
}
current_branch=$(git symbolic-ref --quiet --short HEAD) || {
  echo "ERROR: releases must run from the checked-out default branch, not detached HEAD"
  exit 1
}
[[ "$current_branch" == "$default_branch" ]] || {
  echo "ERROR: release branch $current_branch is not the origin default branch $default_branch"
  exit 1
}
upstream=$(git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}' 2>/dev/null) || { echo "ERROR: current branch has no upstream"; exit 1; }
[[ "$upstream" == "origin/$default_branch" ]] || {
  echo "ERROR: release branch must track origin/$default_branch, not $upstream"
  exit 1
}
git -c submodule.recurse=false fetch --quiet --no-tags origin \
  "refs/heads/${default_branch}:refs/remotes/origin/${default_branch}" || {
    echo "ERROR: cannot reach origin to verify the default-branch head"
    exit 1
  }
release_commit=$(git rev-parse 'HEAD^{commit}')
origin_commit=$(git rev-parse "refs/remotes/origin/${default_branch}^{commit}")
api_commit=$(gh api "repos/${repo}/commits/${default_branch}" --jq .sha) || {
  echo "ERROR: cannot query the current GitHub default-branch head"
  exit 1
}
[[ "$origin_commit" == "$api_commit" ]] || {
  echo "ERROR: fetched origin/$default_branch does not match GitHub's current default-branch head"
  exit 1
}
[[ "$release_commit" == "$origin_commit" ]] || {
  echo "ERROR: HEAD is not the current origin/$default_branch head; historical or superseded commits cannot be tagged"
  exit 1
}
git rev-parse "$tag" >/dev/null 2>&1 && { echo "ERROR: local tag $tag already exists"; exit 1; }
remote_tag=$(git ls-remote --tags origin "refs/tags/${tag}") || { echo "ERROR: cannot query remote tags"; exit 1; }
[[ -z "$remote_tag" ]] || { echo "ERROR: remote tag $tag already exists"; exit 1; }
gh release view "$tag" --json tagName >/dev/null 2>&1 && { echo "ERROR: GitHub release $tag already exists"; exit 1; }
immutable_enabled=$(gh api "repos/${repo}/immutable-releases" --jq .enabled)
[[ "$immutable_enabled" == "true" ]] || {
  echo "ERROR: GitHub immutable releases must be enabled before tagging"
  exit 1
}

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

[[ "${DEVTRIM_AUTOREVIEW_COMMIT:-}" == "$release_commit" ]] || {
  echo "ERROR: manual local autoreview and final-diff inspection are required for commit $release_commit"
  echo "ACTION: run autoreview without release credentials, inspect its output, then acknowledge that exact commit with:"
  echo "ACTION: DEVTRIM_AUTOREVIEW_COMMIT=$release_commit scripts/release.sh $release"
  exit 1
}

echo "==> rechecking current default-branch head before tagging"
git -c submodule.recurse=false fetch --quiet --no-tags origin \
  "refs/heads/${default_branch}:refs/remotes/origin/${default_branch}" || {
    echo "ERROR: cannot refresh origin/$default_branch before tagging"
    exit 1
  }
origin_commit=$(git rev-parse "refs/remotes/origin/${default_branch}^{commit}")
api_commit=$(gh api "repos/${repo}/commits/${default_branch}" --jq .sha) || {
  echo "ERROR: cannot refresh the current GitHub default-branch head"
  exit 1
}
[[ "$release_commit" == "$origin_commit" && "$origin_commit" == "$api_commit" ]] || {
  echo "ERROR: origin/$default_branch advanced during preflight; do not tag a superseded commit"
  exit 1
}
[[ -z "$(git -c core.fsmonitor=false -c submodule.recurse=false status --porcelain=v1 --untracked-files=all --ignore-submodules=all)" ]] || {
  echo "ERROR: the working tree changed during release preflight"
  exit 1
}

echo "==> tag + hosted release workflow"
gh auth status
git tag --no-sign -a "$tag" -m "$tag"
if ! git push --no-verify origin "refs/tags/${tag}:refs/tags/${tag}"; then
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
attestation_args=(
  --repo "$repo"
  --signer-workflow "$repo/.github/workflows/release.yml"
  --source-digest "$release_commit"
)
if [[ "$expected_prerelease" == "true" ]]; then
  attestation_args+=(--source-ref "refs/tags/${tag}")
fi
gh attestation verify "$verify_dir/${out}.zip" "${attestation_args[@]}" >/dev/null

if [[ "$expected_prerelease" == "false" ]]; then
  echo "==> updating Homebrew distribution"
  if ! scripts/update-homebrew.sh "$ver"; then
    echo "ERROR: ${tag} is released, but Homebrew publication or local verification failed"
    echo "ACTION: fix the reported issue, then resume idempotently with scripts/update-homebrew.sh ${ver}"
    exit 1
  fi
fi
echo "==> released and verified ${tag}"
