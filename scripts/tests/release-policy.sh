#!/usr/bin/env bash
# Static policy needles intentionally preserve shell expressions literally.
# shellcheck disable=SC2016
set -euo pipefail

if [[ "${DEVTRIM_RELEASE_POLICY_MOCK:-}" == "1" ]]; then
  tool="${0##*/}"
  case "$tool" in
    gh)
      if [[ "${1:-}" == "repo" && "${2:-}" == "view" ]]; then
        printf 'test/repo\tmain\n'
      elif [[ "${1:-}" == "api" && "${2:-}" == "repos/test/repo/commits/main" ]]; then
        printf '%s\n' "${DEVTRIM_RELEASE_POLICY_HEAD:?}"
      elif [[ "${1:-}" == "api" && "${2:-}" == "repos/test/repo/immutable-releases" ]]; then
        printf 'true\n'
      elif [[ "${1:-}" == "release" && "${2:-}" == "view" ]]; then
        if [[ " $* " == *' --json tagName '* ]]; then
          exit 1
        fi
        mocked_tag="${3:?}"
        mocked_prerelease=false
        [[ "$mocked_tag" != *-beta* ]] || mocked_prerelease=true
        printf '%s\ttrue\t%s\n' "$mocked_tag" "$mocked_prerelease"
      elif [[ "${1:-}" == "run" && "${2:-}" == "list" ]]; then
        if [[ " $* " == *' --json conclusion,status '* ]]; then
          printf 'success\n'
        else
          printf '4242\n'
        fi
      elif [[ "${1:-}" == "run" && "${2:-}" == "watch" ]]; then
        :
      elif [[ "${1:-}" == "auth" && "${2:-}" == "status" ]]; then
        :
      elif [[ "${1:-}" == "release" && "${2:-}" == "verify" ]]; then
        :
      elif [[ "${1:-}" == "release" && "${2:-}" == "download" ]]; then
        mocked_tag="${3:?}"
        mocked_version="${mocked_tag#v}"
        mocked_version="${mocked_version%%-beta*}"
        mocked_dir=""
        shift 3
        while [[ $# -gt 0 ]]; do
          if [[ "$1" == "--dir" ]]; then
            mocked_dir="${2:?}"
            break
          fi
          shift
        done
        [[ -n "$mocked_dir" ]] || exit 96
        mocked_asset="devtrim-${mocked_version}-macos-arm64.zip"
        printf 'release-policy fixture\n' > "$mocked_dir/$mocked_asset"
        mocked_checksum=$(shasum -a 256 "$mocked_dir/$mocked_asset" | awk '{print $1}')
        printf '%s  %s\n' "$mocked_checksum" "$mocked_asset" > "$mocked_dir/SHA256SUMS.txt"
      elif [[ "${1:-}" == "attestation" && "${2:-}" == "verify" ]]; then
        :
      else
        echo "unexpected mocked gh invocation: $*" >&2
        exit 98
      fi
      exit 0
      ;;
    cargo|npm|npx|rustup|ast-grep|gitleaks|trufflehog|shellcheck|actionlint)
      echo "release script executed forbidden project/dependency tool: $tool" >&2
      exit 97
      ;;
  esac
fi

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_root=$(cd "$script_dir/../.." && pwd)
release_script="$repo_root/scripts/release.sh"
homebrew_script="$repo_root/scripts/update-homebrew.sh"
formula_updater="$repo_root/scripts/update-homebrew-formula.rb"
release_workflow="$repo_root/.github/workflows/release.yml"
ci_workflow="$repo_root/.github/workflows/ci.yml"
dependabot="$repo_root/.github/dependabot.yml"
npmrc="$repo_root/video/.npmrc"

fail() {
  echo "release-policy: $*" >&2
  exit 1
}

require_fixed() {
  local file="$1"
  local text="$2"
  grep -Fq -- "$text" "$file" || fail "$file lacks required policy: $text"
}

forbidden_local='^[[:space:]]*(cargo|npm|npx|rustup|ast-grep|gitleaks|trufflehog|shellcheck|actionlint)([[:space:]]|$)'
if grep -En "$forbidden_local" "$release_script"; then
  fail "credential-bearing release script must not execute project or dependency tooling"
fi

require_fixed "$release_script" 'refs/remotes/origin/${default_branch}^{commit}'
require_fixed "$release_script" 'historical or superseded commits cannot be tagged'
require_fixed "$release_script" 'DEVTRIM_AUTOREVIEW_COMMIT'
require_fixed "$release_script" 'git push --no-verify origin'
require_fixed "$release_script" '--signer-workflow "$repo/.github/workflows/release.yml"'
require_fixed "$release_script" 'if [[ "$expected_prerelease" == "false" ]]; then'
require_fixed "$release_script" 'scripts/update-homebrew.sh "$ver"'
require_fixed "$release_script" 'resume idempotently with scripts/update-homebrew.sh ${ver}'
if grep -Fq 'env -u GITHUB_TOKEN -u GH_TOKEN' "$release_script"; then
  fail "release dispatch must preserve supported environment-token authentication for Homebrew publication"
fi

require_fixed "$homebrew_script" '[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]'
require_fixed "$homebrew_script" 'isDraft,isPrerelease,isImmutable,assets'
require_fixed "$homebrew_script" '[[ "$asset_total" == "2" && "$asset_count" == "1" && "$sums_count" == "1" ]]'
require_fixed "$homebrew_script" '[[ "$(< "$verify_dir/SHA256SUMS.txt")" == "${checksum}  ${asset}" ]]'
require_fixed "$homebrew_script" '--signer-workflow "$source_repo/.github/workflows/release.yml"'
require_fixed "$formula_updater" 'expected at most one simple explicit version'
require_fixed "$homebrew_script" 'status --porcelain=v1 --untracked-files=all)" == " M Formula/devtrim.rb"'
require_fixed "$homebrew_script" 'git -C "$tap_dir" push origin HEAD:refs/heads/main'
require_fixed "$homebrew_script" '"$brew_bin" audit --strict --online "$formula_name"'
require_fixed "$homebrew_script" '"$published_tap_commit" == "$remote_tap_commit"'
require_fixed "$homebrew_script" '[[ "$all_paths" == "/opt/homebrew/bin/devtrim" ]]'

bash "$script_dir/update-homebrew-formula.sh"

require_fixed "$release_workflow" 'name: Validate tag provenance'
require_fixed "$release_workflow" 'name: Read-only deterministic release gates'
require_fixed "$release_workflow" 'runs-on: macos-15-intel'
require_fixed "$release_workflow" 'pinned release-tool assets require the macos-15-intel x86_64 runner'
require_fixed "$release_workflow" 'name: Read-only bounded fuzz gates'
require_fixed "$release_workflow" 'needs: [validate, gate, fuzz]'
require_fixed "$release_workflow" 'name: Publish without source checkout'
require_fixed "$release_workflow" 'needs: [validate, prepare]'
require_fixed "$release_workflow" 'cargo fmt --all -- --check'
require_fixed "$release_workflow" 'ast-grep test --skip-snapshot-tests'
require_fixed "$release_workflow" 'ast-grep scan --config sgconfig.yml'
require_fixed "$release_workflow" 'cargo clippy --locked --all-targets --all-features -- -D warnings'
require_fixed "$release_workflow" 'cargo test --locked --all-targets --all-features'
require_fixed "$release_workflow" 'rustup run 1.88.0 cargo test --locked --all-targets --all-features'
require_fixed "$release_workflow" 'cargo audit --file fuzz/Cargo.lock'
require_fixed "$release_workflow" 'bash -n scripts/release.sh scripts/update-homebrew.sh scripts/tests/release-policy.sh'
require_fixed "$release_workflow" 'shellcheck scripts/release.sh scripts/update-homebrew.sh scripts/tests/release-policy.sh'
require_fixed "$release_workflow" 'actionlint'
require_fixed "$release_workflow" 'gitleaks git --redact --no-banner .'
require_fixed "$release_workflow" 'trufflehog git "file://$(pwd)"'
require_fixed "$release_workflow" 'spawn env HOME=$env(TUI_TEST_HOME)'
require_fixed "$release_workflow" 'npm ci --strict-allow-scripts'
require_fixed "$release_workflow" 'npm audit --package-lock-only --audit-level=low'
require_fixed "$release_workflow" 'npm run lint'
require_fixed "$release_workflow" 'npm run format:check'
require_fixed "$release_workflow" 'npm run build'
require_fixed "$release_workflow" 'for target in validate_path clean_path docker_size probe_parsers config_parse'
require_fixed "$release_workflow" 'cargo fuzz run "$target" -- -max_total_time=60'
require_fixed "$release_workflow" 'cmp -s AGENTS.md CLAUDE.md'
require_fixed "$release_workflow" '--signer-workflow "${GITHUB_REPOSITORY}/.github/workflows/release.yml"'
require_fixed "$release_workflow" 'ERROR: release gates changed the checkout'
require_fixed "$release_workflow" 'ERROR: fuzz gates changed the checkout'

[[ "$(grep -c '^    runs-on: macos-15-intel$' "$release_workflow")" -eq 1 ]] ||
  fail "exactly the deterministic gate job must use the Intel runner required by pinned assets"

require_fixed "$release_script" 'immutable-releases'
if grep -Fq 'immutable-releases' "$release_workflow"; then
  fail "GITHUB_TOKEN cannot call the admin-read immutable-release settings endpoint"
fi
require_fixed "$release_workflow" '--json isDraft,isImmutable,isPrerelease,tagName'
require_fixed "$release_workflow" '[[ "$(jq -r .isImmutable <<<"$release_state")" == "true" ]]'

checkout_count=$(grep -Fc 'uses: actions/checkout@' "$release_workflow")
credentialless_count=$(grep -Fc 'persist-credentials: false' "$release_workflow")
[[ "$checkout_count" -eq "$credentialless_count" ]] || fail "every source checkout must disable persisted credentials"

[[ "$(grep -c '^      contents: write$' "$release_workflow")" -eq 1 ]] ||
  fail "only the publisher may receive contents: write"
[[ "$(grep -c '^      id-token: write$' "$release_workflow")" -eq 1 ]] ||
  fail "only the publisher may receive id-token: write"

publish_block=$(sed -n '/^  publish:/,$p' "$release_workflow")
if grep -Eq 'actions/checkout@|^[[:space:]]+(cargo|npm|npx|rustup)[[:space:]]' <<<"$publish_block"; then
  fail "publisher must neither check out nor execute project/dependency code"
fi

while IFS= read -r action_ref; do
  [[ "$action_ref" =~ ^[0-9a-f]{40}$ ]] || fail "GitHub Action is not pinned to a full commit SHA: $action_ref"
done < <(sed -n 's/^[[:space:]]*uses: [^@]*@\([0-9A-Za-z._-]*\).*/\1/p' "$release_workflow" "$ci_workflow")

require_fixed "$ci_workflow" 'bash scripts/tests/release-policy.sh'
require_fixed "$ci_workflow" 'cargo audit --file fuzz/Cargo.lock'
require_fixed "$ci_workflow" 'npm ci --strict-allow-scripts'
require_fixed "$dependabot" 'directory: /fuzz'
[[ "$(tr -d '[:space:]' < "$npmrc")" == 'strict-allow-scripts=true' ]] ||
  fail "video/.npmrc must enable strict lifecycle-script allowlisting"

test_root=$(mktemp -d "${TMPDIR:-/tmp}/devtrim-release-policy.XXXXXX")
cleanup() {
  chmod -R u+w "$test_root" 2>/dev/null || true
  rm -r "$test_root"
}
trap cleanup EXIT
origin="$test_root/origin.git"
seed="$test_root/seed"
subject="$test_root/subject"
fake_bin="$test_root/bin"
mkdir -p "$seed/scripts" "$fake_bin"
git init --bare --initial-branch=main "$origin" >/dev/null
git -C "$seed" init -b main >/dev/null
git -C "$seed" config user.name release-policy
git -C "$seed" config user.email release-policy@example.invalid
cp "$release_script" "$seed/scripts/release.sh"
cat > "$seed/scripts/update-homebrew.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "${GH_TOKEN:-}" == "release-policy-token" ]] || {
  echo "release policy stripped environment-token authentication before Homebrew publication" >&2
  exit 95
}
printf '%s\n' "$1" >> "${DEVTRIM_HOMEBREW_INVOCATIONS:?}"
EOF
chmod +x "$seed/scripts/update-homebrew.sh"
printf '%s\n' '[package]' 'name = "fixture"' 'version = "1.2.3"' > "$seed/Cargo.toml"
printf '%s\n' '# Changelog' '## [1.2.3] - 2099-01-01' '- fixture' > "$seed/CHANGELOG.md"
printf '%s\n' '# fixture v1.2.3' > "$seed/README.md"
printf '%s\n' '<p>fixture v1.2.3</p>' > "$seed/MANUAL.html"
git -C "$seed" add Cargo.toml CHANGELOG.md README.md MANUAL.html scripts/release.sh scripts/update-homebrew.sh
git -C "$seed" commit -m 'fixture: release candidate' >/dev/null
git -C "$seed" remote add origin "$origin"
git -C "$seed" push -u origin main >/dev/null
git --git-dir="$origin" symbolic-ref HEAD refs/heads/main
git clone "$origin" "$subject" >/dev/null
git -C "$subject" config user.name release-policy
git -C "$subject" config user.email release-policy@example.invalid

for tool in gh cargo npm npx rustup ast-grep gitleaks trufflehog shellcheck actionlint; do
  ln -s "$script_dir/release-policy.sh" "$fake_bin/$tool"
done

printf '%s\n' superseding > "$seed/superseding-commit"
git -C "$seed" add superseding-commit
git -C "$seed" commit -m 'fixture: supersede candidate' >/dev/null
git -C "$seed" push origin main >/dev/null
current_head=$(git -C "$seed" rev-parse HEAD)

set +e
historical_output=$(
  cd "$subject" &&
    DEVTRIM_RELEASE_POLICY_MOCK=1 \
    DEVTRIM_RELEASE_POLICY_HEAD="$current_head" \
    PATH="$fake_bin:$PATH" \
    bash scripts/release.sh 1.2.3 2>&1
)
historical_status=$?
set -e
[[ "$historical_status" -ne 0 ]] || fail "historical release commit unexpectedly passed"
grep -Fq 'historical or superseded commits cannot be tagged' <<<"$historical_output" ||
  fail "historical release rejection was not explicit"
[[ -z "$(git -C "$subject" tag --list v1.2.3)" ]] || fail "historical release created a tag"

git -C "$subject" pull --ff-only >/dev/null
set +e
current_output=$(
  cd "$subject" &&
    DEVTRIM_RELEASE_POLICY_MOCK=1 \
    DEVTRIM_RELEASE_POLICY_HEAD="$current_head" \
    PATH="$fake_bin:$PATH" \
    bash scripts/release.sh 1.2.3 2>&1
)
current_status=$?
set -e
[[ "$current_status" -ne 0 ]] || fail "release without autoreview acknowledgment unexpectedly passed"
grep -Fq 'manual local autoreview and final-diff inspection are required' <<<"$current_output" ||
  fail "current-head release did not reach the exact-commit autoreview prerequisite"
[[ -z "$(git -C "$subject" tag --list v1.2.3)" ]] || fail "preflight-only release created a tag"

homebrew_invocations="$test_root/homebrew-invocations"
DEVTRIM_RELEASE_POLICY_MOCK=1 \
DEVTRIM_RELEASE_POLICY_HEAD="$current_head" \
DEVTRIM_AUTOREVIEW_COMMIT="$current_head" \
DEVTRIM_HOMEBREW_INVOCATIONS="$homebrew_invocations" \
GH_TOKEN=release-policy-token \
PATH="$fake_bin:$PATH" \
bash "$subject/scripts/release.sh" 1.2.3-beta1 >/dev/null
[[ ! -e "$homebrew_invocations" ]] || fail "beta release unexpectedly invoked Homebrew publication"

DEVTRIM_RELEASE_POLICY_MOCK=1 \
DEVTRIM_RELEASE_POLICY_HEAD="$current_head" \
DEVTRIM_AUTOREVIEW_COMMIT="$current_head" \
DEVTRIM_HOMEBREW_INVOCATIONS="$homebrew_invocations" \
GH_TOKEN=release-policy-token \
PATH="$fake_bin:$PATH" \
bash "$subject/scripts/release.sh" 1.2.3 >/dev/null
[[ "$(< "$homebrew_invocations")" == "1.2.3" ]] ||
  fail "production release did not invoke Homebrew publication exactly once"

echo "release-policy: all checks passed"
