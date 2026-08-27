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
        exit 1
      elif [[ "${1:-}" == "run" && "${2:-}" == "list" ]]; then
        printf 'success\n'
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
require_fixed "$release_workflow" 'bash -n scripts/release.sh scripts/tests/release-policy.sh'
require_fixed "$release_workflow" 'shellcheck scripts/release.sh scripts/tests/release-policy.sh'
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

immutable_checks=$(grep -Fc 'immutable-releases' "$release_workflow")
[[ "$immutable_checks" -ge 2 ]] || fail "immutable-releases must be checked before preparation and publication"

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
printf '%s\n' '[package]' 'name = "fixture"' 'version = "1.2.3"' > "$seed/Cargo.toml"
printf '%s\n' '# Changelog' '## [1.2.3] - 2099-01-01' '- fixture' > "$seed/CHANGELOG.md"
printf '%s\n' '# fixture v1.2.3' > "$seed/README.md"
printf '%s\n' '<p>fixture v1.2.3</p>' > "$seed/MANUAL.html"
git -C "$seed" add Cargo.toml CHANGELOG.md README.md MANUAL.html scripts/release.sh
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

echo "release-policy: all checks passed"
