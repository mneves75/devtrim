#!/usr/bin/env bash
# Publish a verified production release to Homebrew and verify the local install.
# usage: scripts/update-homebrew.sh <version>   (e.g. 0.6.1)
set -euo pipefail

version="${1:?usage: scripts/update-homebrew.sh <version>}"
[[ $# -eq 1 ]] || { echo "ERROR: usage: scripts/update-homebrew.sh <version>"; exit 1; }
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo "ERROR: Homebrew publication requires a production X.Y.Z version"
  exit 1
}

source_repo="mneves75/devtrim"
tap_repo="mneves75/homebrew-devtrim"
tap_name="mneves75/devtrim"
formula_name="${tap_name}/devtrim"
tag="v${version}"
asset="devtrim-${version}-macos-arm64.zip"
asset_url="https://github.com/${source_repo}/releases/download/${tag}/${asset}"
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
formula_updater="$script_dir/update-homebrew-formula.rb"
[[ -f "$formula_updater" && ! -L "$formula_updater" ]] || {
  echo "ERROR: trusted formula updater is unavailable"
  exit 1
}

for tool in gh git ruby shasum awk grep; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "ERROR: required tool is unavailable: $tool"
    exit 1
  }
done
brew_bin="/opt/homebrew/bin/brew"
[[ -x "$brew_bin" ]] || {
  echo "ERROR: Homebrew is unavailable at /opt/homebrew/bin/brew"
  exit 1
}
[[ "$(uname -m)" == "arm64" ]] || {
  echo "ERROR: the published formula contains an Apple silicon binary"
  exit 1
}
[[ "$("$brew_bin" --prefix)" == "/opt/homebrew" ]] || {
  echo "ERROR: expected the sole Homebrew installation at /opt/homebrew"
  exit 1
}

work_root=$(mktemp -d "${TMPDIR:-/tmp}/devtrim-homebrew.XXXXXX")
cleanup() {
  chmod -R u+w "$work_root" 2>/dev/null || true
  rm -r "$work_root"
}
trap cleanup EXIT
verify_dir="$work_root/release"
tap_dir="$work_root/tap"
mkdir -p "$verify_dir"

echo "==> verifying immutable production release ${tag}"
release_state=$(gh release view "$tag" --repo "$source_repo" \
  --json tagName,isDraft,isPrerelease,isImmutable,assets \
  --jq "[.tagName, .isDraft, .isPrerelease, .isImmutable, (.assets | length), ([.assets[] | select(.name == \"${asset}\")] | length), ([.assets[] | select(.name == \"${asset}\")][0].digest // \"\"), ([.assets[] | select(.name == \"SHA256SUMS.txt\")] | length)] | @tsv") || {
  echo "ERROR: cannot read GitHub release ${tag}"
  exit 1
}
IFS=$'\t' read -r release_tag is_draft is_prerelease is_immutable asset_total asset_count asset_digest sums_count <<< "$release_state"
[[ "$release_tag" == "$tag" && "$is_draft" == "false" && "$is_prerelease" == "false" && "$is_immutable" == "true" ]] || {
  echo "ERROR: ${tag} is not a verified immutable production release"
  exit 1
}
[[ "$asset_total" == "2" && "$asset_count" == "1" && "$sums_count" == "1" ]] || {
  echo "ERROR: ${tag} does not contain exactly the expected archive and checksum manifest"
  exit 1
}
[[ "$asset_digest" =~ ^sha256:[0-9a-f]{64}$ ]] || {
  echo "ERROR: ${asset} lacks a valid GitHub SHA-256 digest"
  exit 1
}

gh release verify "$tag" --repo "$source_repo" --format json >/dev/null
gh release download "$tag" --repo "$source_repo" --dir "$verify_dir" \
  --pattern "$asset" --pattern SHA256SUMS.txt
[[ "$(wc -l < "$verify_dir/SHA256SUMS.txt" | tr -d '[:space:]')" == "1" ]] || {
  echo "ERROR: SHA256SUMS.txt must contain exactly one entry"
  exit 1
}
checksum=$(shasum -a 256 "$verify_dir/$asset" | awk '{print $1}')
[[ "$(< "$verify_dir/SHA256SUMS.txt")" == "${checksum}  ${asset}" ]] || {
  echo "ERROR: SHA256SUMS.txt does not name the exact production archive"
  exit 1
}
( cd "$verify_dir" && shasum -a 256 -c SHA256SUMS.txt )
[[ "$asset_digest" == "sha256:${checksum}" ]] || {
  echo "ERROR: downloaded checksum does not match the GitHub asset digest"
  exit 1
}
release_commit=$(gh api "repos/${source_repo}/commits/${tag}" --jq .sha) || {
  echo "ERROR: cannot resolve the release commit for ${tag}"
  exit 1
}
[[ "$release_commit" =~ ^[0-9a-f]{40}$ ]] || {
  echo "ERROR: GitHub returned an invalid release commit for ${tag}"
  exit 1
}
gh attestation verify "$verify_dir/$asset" \
  --repo "$source_repo" \
  --signer-workflow "$source_repo/.github/workflows/release.yml" \
  --source-digest "$release_commit" >/dev/null

echo "==> updating ${tap_repo}"
gh repo clone "$tap_repo" "$tap_dir" -- --quiet --filter=blob:none --single-branch
[[ "$(git -C "$tap_dir" symbolic-ref --quiet --short HEAD)" == "main" ]] || {
  echo "ERROR: ${tap_repo} default branch is not main"
  exit 1
}
tap_origin=$(git -C "$tap_dir" remote get-url origin)
case "$tap_origin" in
  "https://github.com/${tap_repo}.git"|"git@github.com:${tap_repo}.git") ;;
  *) echo "ERROR: cloned tap origin is unexpected: ${tap_origin}"; exit 1 ;;
esac
[[ -z "$(git -C "$tap_dir" status --porcelain=v1 --untracked-files=all)" ]] || {
  echo "ERROR: freshly cloned tap is not clean"
  exit 1
}

formula="$tap_dir/Formula/devtrim.rb"
candidate="$work_root/devtrim.rb"
[[ -f "$formula" && ! -L "$formula" ]] || {
  echo "ERROR: expected a regular Formula/devtrim.rb in ${tap_repo}"
  exit 1
}
env -u GITHUB_TOKEN -u GH_TOKEN -u HOMEBREW_GITHUB_API_TOKEN -u SSH_AUTH_SOCK \
  ruby "$formula_updater" "$formula" "$candidate" "$asset_url" "$checksum"
env -u GITHUB_TOKEN -u GH_TOKEN -u HOMEBREW_GITHUB_API_TOKEN -u SSH_AUTH_SOCK \
  ruby -c "$candidate"
grep -Fqx "  url \"${asset_url}\"" "$candidate" || {
  echo "ERROR: generated formula lacks the exact production URL"
  exit 1
}
grep -Fqx "  sha256 \"${checksum}\"" "$candidate" || {
  echo "ERROR: generated formula lacks the exact production checksum"
  exit 1
}
if grep -Eq '^  version ' "$candidate"; then
  echo "ERROR: generated formula contains a redundant explicit version"
  exit 1
fi

if ! cmp -s "$candidate" "$formula"; then
  cp "$candidate" "$formula"
  [[ "$(git -C "$tap_dir" status --porcelain=v1 --untracked-files=all)" == " M Formula/devtrim.rb" ]] || {
    echo "ERROR: Homebrew update changed files outside Formula/devtrim.rb"
    exit 1
  }
  git -C "$tap_dir" diff --check
  env -u GITHUB_TOKEN -u GH_TOKEN -u HOMEBREW_GITHUB_API_TOKEN -u SSH_AUTH_SOCK \
    "$brew_bin" style "$formula"
  git -C "$tap_dir" add -- Formula/devtrim.rb
  [[ "$(git -C "$tap_dir" diff --cached --name-only)" == "Formula/devtrim.rb" ]] || {
    echo "ERROR: staged Homebrew update contains an unexpected path"
    exit 1
  }
  git -C "$tap_dir" commit -m "chore: update devtrim to ${version}" -- Formula/devtrim.rb
  git -C "$tap_dir" push origin HEAD:refs/heads/main
else
  echo "==> ${tap_repo} already publishes ${version}"
fi
published_tap_commit=$(git -C "$tap_dir" rev-parse HEAD)

echo "==> auditing and installing ${formula_name}"
if ! "$brew_bin" tap | grep -Fqx "$tap_name"; then
  env -u GITHUB_TOKEN -u GH_TOKEN -u HOMEBREW_GITHUB_API_TOKEN -u SSH_AUTH_SOCK \
    "$brew_bin" tap "$tap_name"
fi
env -u GITHUB_TOKEN -u GH_TOKEN -u HOMEBREW_GITHUB_API_TOKEN -u SSH_AUTH_SOCK \
  "$brew_bin" update
local_tap_dir=$("$brew_bin" --repo "$tap_name")
remote_tap_commit=$(env -u GITHUB_TOKEN -u GH_TOKEN -u HOMEBREW_GITHUB_API_TOKEN -u SSH_AUTH_SOCK \
  git ls-remote "https://github.com/${tap_repo}.git" refs/heads/main | awk '{print $1}')
local_tap_commit=$(git -C "$local_tap_dir" rev-parse HEAD)
[[ -n "$remote_tap_commit" && "$published_tap_commit" == "$remote_tap_commit" && "$local_tap_commit" == "$remote_tap_commit" ]] || {
  echo "ERROR: local Homebrew tap does not match ${tap_repo}@main"
  exit 1
}
local_formula="$local_tap_dir/Formula/devtrim.rb"
cmp -s "$candidate" "$local_formula" || {
  echo "ERROR: local Homebrew formula differs from the formula just published"
  exit 1
}
grep -Fqx "  url \"${asset_url}\"" "$local_formula" || {
  echo "ERROR: local Homebrew tap has a stale production URL"
  exit 1
}
grep -Fqx "  sha256 \"${checksum}\"" "$local_formula" || {
  echo "ERROR: local Homebrew tap has a stale production checksum"
  exit 1
}
if grep -Eq '^  version ' "$local_formula"; then
  echo "ERROR: local Homebrew formula contains a redundant explicit version"
  exit 1
fi

env -u GITHUB_TOKEN -u GH_TOKEN -u HOMEBREW_GITHUB_API_TOKEN -u SSH_AUTH_SOCK \
  HOMEBREW_NO_AUTO_UPDATE=1 "$brew_bin" info --json=v2 "$formula_name" | \
  env -u GITHUB_TOKEN -u GH_TOKEN -u HOMEBREW_GITHUB_API_TOKEN -u SSH_AUTH_SOCK \
    ruby -rjson -e 'formula = JSON.parse(STDIN.read).fetch("formulae").fetch(0); abort "wrong inferred version" unless formula.dig("versions", "stable") == ARGV[0]; stable = formula.dig("urls", "stable"); abort "wrong formula URL" unless stable["url"] == ARGV[1]; abort "wrong formula checksum" unless stable["checksum"] == ARGV[2]' \
    "$version" "$asset_url" "$checksum"
env -u GITHUB_TOKEN -u GH_TOKEN -u HOMEBREW_GITHUB_API_TOKEN -u SSH_AUTH_SOCK \
  HOMEBREW_NO_AUTO_UPDATE=1 "$brew_bin" audit --strict --online "$formula_name"
"$brew_bin" list --versions devtrim >/dev/null 2>&1 || {
  echo "ERROR: devtrim must already be installed before production release closeout"
  exit 1
}
env -u GITHUB_TOKEN -u GH_TOKEN -u HOMEBREW_GITHUB_API_TOKEN -u SSH_AUTH_SOCK \
  HOMEBREW_NO_AUTO_UPDATE=1 "$brew_bin" upgrade "$formula_name"
env -u GITHUB_TOKEN -u GH_TOKEN -u HOMEBREW_GITHUB_API_TOKEN -u SSH_AUTH_SOCK \
  HOMEBREW_NO_AUTO_UPDATE=1 "$brew_bin" test "$formula_name"

installed_versions=$("$brew_bin" list --versions devtrim)
[[ "$installed_versions" == "devtrim ${version}" ]] || {
  echo "ERROR: expected only devtrim ${version}; found: ${installed_versions:-none}"
  exit 1
}
resolved_path=$(command -v devtrim || true)
[[ "$resolved_path" == "/opt/homebrew/bin/devtrim" ]] || {
  echo "ERROR: devtrim resolves to ${resolved_path:-nothing}, not /opt/homebrew/bin/devtrim"
  exit 1
}
all_paths=$(type -a -p devtrim | awk '!seen[$0]++')
[[ "$all_paths" == "/opt/homebrew/bin/devtrim" ]] || {
  echo "ERROR: multiple devtrim executables are visible on PATH:"
  printf '%s\n' "$all_paths"
  exit 1
}
actual_version=$(env -u GITHUB_TOKEN -u GH_TOKEN -u HOMEBREW_GITHUB_API_TOKEN -u SSH_AUTH_SOCK \
  /opt/homebrew/bin/devtrim --version)
[[ "$actual_version" == "devtrim ${version}" ]] || {
  echo "ERROR: installed devtrim version does not match ${version}"
  exit 1
}
echo "==> Homebrew publishes and runs devtrim ${version}"
