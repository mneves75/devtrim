#!/usr/bin/env bash
# devtrim release: verify → build arm64 → package → checksum → tag → GitHub release
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
target="aarch64-apple-darwin"
out="devtrim-${release}-macos-arm64"
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

echo "==> locked arm64 release build"
if command -v rustup >/dev/null 2>&1; then
  rustup target add "$target"
fi
cargo build --release --locked --target "$target"
binary="target/${target}/release/devtrim"
file "$binary" | grep -q 'arm64' || { echo "ERROR: release binary is not arm64"; exit 1; }
"$binary" --version | grep -Fqx "devtrim ${ver}" || { echo "ERROR: binary version mismatch"; exit 1; }

echo "==> clean packaging"
# SAFE: both paths are ignored, version-derived release outputs under repository dist/.
[[ ! -e "dist/${out}" ]] || rm -r "dist/${out}"
rm -f "dist/${out}.zip"
rm -f "dist/SHA256SUMS.txt"
mkdir -p "dist/${out}"
cp "$binary" "dist/${out}/devtrim"
cp MANUAL.html README.md LICENSE "dist/${out}/"
( cd dist && zip -qr "${out}.zip" "${out}" )
( cd dist && shasum -a 256 "${out}.zip" > SHA256SUMS.txt )
unzip -l "dist/${out}.zip"
( cd dist && shasum -a 256 -c SHA256SUMS.txt )

echo "==> tag + GitHub release"
notes=$(awk -v hdr="## [${ver}]" 'index($0,hdr)==1{found=1;next} /^## /{if(found)exit} found' CHANGELOG.md)
[[ -n "$notes" ]] || { echo "ERROR: empty release notes for ${ver}"; exit 1; }
gh auth status
git tag -a "$tag" -m "$tag"
if ! git push origin "$tag"; then
  echo "ERROR: local tag $tag was created but the push failed"
  echo "RECOVER: inspect git ls-remote --tags origin refs/tags/$tag before acting"
  echo "RECOVER: if absent, fix the push issue, delete the local tag with git tag -d $tag, and rerun; if present, create the GitHub release manually"
  exit 1
fi
release_args=(--verify-tag)
if [[ -n "$prerelease_suffix" ]]; then
  release_args+=(--prerelease --latest=false)
fi
if ! gh release create "$tag" "dist/${out}.zip" "dist/SHA256SUMS.txt" --title "devtrim ${release}" --notes "$notes" "${release_args[@]}"; then
  echo "ERROR: tag $tag was pushed but GitHub release creation failed"
  echo "RECOVER: inspect the pushed tag/assets, then rerun gh release create for ${tag} with flags: ${release_args[*]}"
  exit 1
fi
echo "==> released ${tag}"
