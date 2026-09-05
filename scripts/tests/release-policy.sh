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
precommit_hook="$repo_root/.githooks/pre-commit"
shellcheck_script="$repo_root/scripts/tests/shellcheck-tracked.sh"
gitleaks_control_script="$repo_root/scripts/tests/gitleaks-positive-control.sh"
landing_page="$repo_root/index.html"
manual_page="$repo_root/MANUAL.html"
favicon="$repo_root/favicon.svg"
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

require_line() {
  local file="$1"
  local text="$2"
  grep -Fxq -- "$text" "$file" || fail "$file lacks required line: $text"
}

require_before() {
  local file="$1"
  local before="$2"
  local after="$3"
  local before_line
  local after_line
  before_line=$(grep -Fn -- "$before" "$file" | awk -F: 'NR == 1 { print $1 }')
  after_line=$(grep -Fn -- "$after" "$file" | awk -F: 'END { print $1 }')
  [[ -n "$before_line" && -n "$after_line" && "$before_line" -lt "$after_line" ]] ||
    fail "$file must place '$before' before '$after'"
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
require_fixed "$release_workflow" 'scripts/tests/shellcheck-tracked.sh'
require_fixed "$release_workflow" 'actionlint'
require_fixed "$release_workflow" 'gitleaks git --redact --no-banner .'
require_fixed "$release_workflow" 'trufflehog git "file://$(pwd)"'
for workflow in "$release_workflow" "$ci_workflow"; do
  require_fixed "$workflow" 'for script in scripts/verify.sh scripts/release.sh scripts/update-homebrew.sh scripts/tests/release-policy.sh scripts/tests/update-homebrew-formula.sh scripts/perf/ab.sh scripts/perf/corpus.sh; do'
  require_fixed "$workflow" 'bash -n "$script"'
  require_fixed "$workflow" 'for script in .githooks/pre-commit scripts/tests/shellcheck-tracked.sh scripts/tests/gitleaks-positive-control.sh; do'
  require_fixed "$workflow" 'sh -n "$script"'
  require_fixed "$workflow" 'python3 scripts/tests/tui.py target/debug/devtrim'
  require_fixed "$workflow" 'python3 scripts/tests/read-only-views.py target/debug/devtrim'
  require_fixed "$workflow" 'rustup toolchain install 1.98.1 --profile minimal'
  require_fixed "$workflow" 'rustup default 1.98.1'
done
require_fixed "$repo_root/rust-toolchain.toml" 'channel = "1.98.1"'
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
[[ "$(grep -c '^    runs-on: macos-15$' "$release_workflow")" -eq 4 ]] ||
  fail "all non-Intel release jobs must use the supported macos-15 arm64 runner"
[[ "$(grep -c '^    runs-on: macos-15$' "$ci_workflow")" -eq 1 ]] ||
  fail "CI must use exactly one supported macos-15 arm64 runner"
[[ "$(grep -c '^          fetch-depth: 0$' "$ci_workflow")" -eq 1 ]] ||
  fail "CI checkout must fetch full history exactly once for secret scans"
if grep -Eq '^    runs-on: macos-15-intel$' "$ci_workflow"; then
  fail "CI must not use the Intel runner required only by release-tool assets"
fi
if grep -Fq 'runs-on: macos-14' "$release_workflow" "$ci_workflow"; then
  fail "deprecated macos-14 runners must not remain in CI or release workflows"
fi

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
require_fixed "$ci_workflow" 'fetch-depth: 0'
require_fixed "$ci_workflow" 'cargo audit --file fuzz/Cargo.lock'
require_fixed "$ci_workflow" 'npm ci --strict-allow-scripts'
require_fixed "$ci_workflow" 'runs-on: macos-15'
require_fixed "$ci_workflow" 'shellcheck-v0.11.0.darwin.aarch64.tar.gz'
require_fixed "$ci_workflow" '339b930feb1ea764467013cc1f72d09cd6b869ebf1013296ba9055ab2ffbd26f'
require_fixed "$ci_workflow" 'pinned ShellCheck asset requires the macos-15 arm64 runner'
require_fixed "$ci_workflow" 'scripts/tests/shellcheck-tracked.sh'
require_fixed "$ci_workflow" 'name: Install checksum-verified secret scanners'
require_fixed "$ci_workflow" 'pinned secret-scanner assets require the macos-15 arm64 runner'
require_fixed "$ci_workflow" 'gh release download v8.30.1 -R gitleaks/gitleaks'
require_fixed "$ci_workflow" 'gitleaks_8.30.1_darwin_arm64.tar.gz'
require_fixed "$ci_workflow" 'b40ab0ae55c505963e365f271a8d3846efbc170aa17f2607f13df610a9aeb6a5'
require_fixed "$ci_workflow" 'gh release download v3.97.1 -R trufflesecurity/trufflehog'
require_fixed "$ci_workflow" 'trufflehog_3.97.1_darwin_arm64.tar.gz'
require_fixed "$ci_workflow" '1af86cf30c1cc5c1735ec6af9292b399ec9bed3ff1b30be13fcbfd4a30ab449a'
require_fixed "$ci_workflow" '"$tool_bin/gitleaks" version | grep -Fqx '\''8.30.1'\'''
require_fixed "$ci_workflow" '"$tool_bin/trufflehog" --version | grep -Fq '\''trufflehog 3.97.1'\'''
require_fixed "$ci_workflow" 'scripts/tests/gitleaks-positive-control.sh "$tool_bin/gitleaks"'
require_fixed "$ci_workflow" 'name: Full-history secret scans'
require_fixed "$ci_workflow" 'gitleaks git --redact --no-banner .'
require_fixed "$ci_workflow" 'trufflehog git "file://$(pwd)" --results=verified,unknown --fail'
require_fixed "$ci_workflow" '--fail-on-scan-errors --no-update --no-color'
require_fixed "$release_workflow" 'shellcheck-v0.11.0.darwin.x86_64.tar.gz'
require_fixed "$release_workflow" 'c2c15e08df0e8fbc374c335b230a7ee958c313fa5714817a59aa59f1aa594f51'
require_fixed "$release_workflow" 'scripts/tests/gitleaks-positive-control.sh "$tool_bin/gitleaks"'
require_before "$ci_workflow" \
  'scripts/tests/gitleaks-positive-control.sh "$tool_bin/gitleaks"' \
  'echo "$tool_bin" >> "$GITHUB_PATH"'
require_before "$release_workflow" \
  'scripts/tests/gitleaks-positive-control.sh "$tool_bin/gitleaks"' \
  'echo "$tool_bin" >> "$GITHUB_PATH"'
require_fixed "$release_workflow" 'cp MANUAL.html README.md LICENSE favicon.svg "dist/$out/"'
require_fixed "$precommit_hook" 'scripts/tests/shellcheck-tracked.sh'
[[ -x "$shellcheck_script" ]] || fail "shellcheck helper must be executable"
require_fixed "$shellcheck_script" 'if ! repo_root=$(git rev-parse --show-toplevel 2>/dev/null); then'
require_fixed "$shellcheck_script" 'if ! command -v shellcheck >/dev/null 2>&1; then'
require_fixed "$shellcheck_script" 'shellcheck_paths=$(mktemp "${TMPDIR:-/tmp}/devtrim-shellcheck.XXXXXX")'
require_fixed "$shellcheck_script" 'rm -f "$shellcheck_paths"'
require_fixed "$shellcheck_script" 'trap cleanup_shellcheck_paths EXIT'
require_fixed "$shellcheck_script" "if ! git ls-files -z -- '*.sh' > \"\$shellcheck_paths\"; then"
require_fixed "$shellcheck_script" 'xargs -0 shellcheck -- .githooks/pre-commit < "$shellcheck_paths"'
[[ -x "$gitleaks_control_script" ]] || fail "Gitleaks positive-control helper must be executable"
require_fixed "$gitleaks_control_script" "'token = \"ghp_'"
require_fixed "$gitleaks_control_script" "'dc831f20456cd20fa6'"
require_fixed "$gitleaks_control_script" "'112d38ca4eb7fdb8f2'"
require_fixed "$gitleaks_control_script" '"$gitleaks_bin" stdin --no-banner --redact --no-color'
[[ -s "$favicon" ]] || fail "favicon.svg must exist and be non-empty"
require_fixed "$landing_page" '<link rel="icon" href="favicon.svg" type="image/svg+xml">'
require_fixed "$manual_page" '<link rel="icon" href="favicon.svg" type="image/svg+xml">'
require_fixed "$dependabot" 'directory: /fuzz'
require_line "$repo_root/.gitignore" '.env*'
require_line "$repo_root/.gitignore" '!.env.example'
require_line "$repo_root/.gitignore" '!.env.sample'
require_line "$repo_root/.gitignore" '*.key'
require_line "$repo_root/.gitignore" '*.pem'
require_line "$repo_root/.gitignore" '*.p12'
require_line "$repo_root/.gitignore" '*.pfx'
[[ "$(tr -d '[:space:]' < "$npmrc")" == 'strict-allow-scripts=true' ]] ||
  fail "video/.npmrc must enable strict lifecycle-script allowlisting"

test_root=$(mktemp -d "${TMPDIR:-/tmp}/devtrim-release-policy.XXXXXX")
cleanup() {
  chmod -R u+w "$test_root" 2>/dev/null || true
  rm -r "$test_root"
}
trap cleanup EXIT

gitleaks_mock="$test_root/gitleaks-positive"
cat > "$gitleaks_mock" <<'EOF'
#!/bin/sh
set -eu
input=$(cat)
expected=$(printf '%s%s%s%s' 'token = "ghp_' 'dc831f20456cd20fa6' '112d38ca4eb7fdb8f2' '"')
[ "$input" = "$expected" ] || exit 91
[ "$*" = 'stdin --no-banner --redact --no-color' ] || exit 92
exit 1
EOF
chmod +x "$gitleaks_mock"
"$gitleaks_control_script" "$gitleaks_mock"

gitleaks_noop="$test_root/gitleaks-noop"
cat > "$gitleaks_noop" <<'EOF'
#!/bin/sh
cat >/dev/null
exit 0
EOF
chmod +x "$gitleaks_noop"
set +e
gitleaks_noop_output=$("$gitleaks_control_script" "$gitleaks_noop" 2>&1)
gitleaks_noop_status=$?
set -e
[[ "$gitleaks_noop_status" -ne 0 ]] || fail "Gitleaks positive control accepted a no-op detector"
grep -Fq 'expected leak exit 1, got 0' <<<"$gitleaks_noop_output" ||
  fail "Gitleaks positive control did not explain a no-op detector"

shellcheck_fixture="$test_root/shellcheck-fixture"
shellcheck_bin="$test_root/shellcheck-bin"
shellcheck_log="$test_root/shellcheck-args"
mkdir -p "$shellcheck_fixture/.githooks" "$shellcheck_fixture/scripts/tests" "$shellcheck_bin"
cp "$shellcheck_script" "$shellcheck_fixture/scripts/tests/shellcheck-tracked.sh"
chmod +x "$shellcheck_fixture/scripts/tests/shellcheck-tracked.sh"
printf '%s\n' '#!/bin/sh' 'exit 0' > "$shellcheck_fixture/.githooks/pre-commit"
printf '%s\n' '#!/bin/sh' 'exit 0' > "$shellcheck_fixture/space name.sh"
printf '%s\n' '#!/bin/sh' 'exit 0' > "$shellcheck_fixture/-leading.sh"
cat > "$shellcheck_bin/shellcheck" <<'EOF'
#!/bin/sh
set -eu
: "${DEVTRIM_SHELLCHECK_LOG:?}"
for argument in "$@"; do
  printf '%s\n' "$argument"
done > "$DEVTRIM_SHELLCHECK_LOG"
EOF
chmod +x "$shellcheck_bin/shellcheck"
git -C "$shellcheck_fixture" init -b main >/dev/null
git -C "$shellcheck_fixture" add -- \
  .githooks/pre-commit scripts/tests/shellcheck-tracked.sh 'space name.sh' '-leading.sh'
(
  cd "$shellcheck_fixture"
  DEVTRIM_SHELLCHECK_LOG="$shellcheck_log" PATH="$shellcheck_bin:$PATH" \
    scripts/tests/shellcheck-tracked.sh
)
for expected_argument in -- .githooks/pre-commit scripts/tests/shellcheck-tracked.sh 'space name.sh' '-leading.sh'; do
  [[ "$(grep -Fxc -- "$expected_argument" "$shellcheck_log")" -eq 1 ]] ||
    fail "ShellCheck helper did not preserve argument: $expected_argument"
done

shellcheck_git_fail_bin="$test_root/shellcheck-git-fail-bin"
mkdir -p "$shellcheck_git_fail_bin"
cat > "$shellcheck_git_fail_bin/git" <<'EOF'
#!/bin/sh
case "${1:-}" in
  rev-parse)
    pwd
    ;;
  ls-files)
    exit 93
    ;;
  *)
    exit 94
    ;;
esac
EOF
chmod +x "$shellcheck_git_fail_bin/git"
rm -f "$shellcheck_log"
set +e
enumeration_output=$(
  cd "$shellcheck_fixture"
  DEVTRIM_SHELLCHECK_LOG="$shellcheck_log" \
    PATH="$shellcheck_git_fail_bin:$shellcheck_bin:$PATH" \
    scripts/tests/shellcheck-tracked.sh 2>&1
)
enumeration_status=$?
set -e
[[ "$enumeration_status" -ne 0 ]] || fail "ShellCheck helper ignored Git enumeration failure"
[[ ! -e "$shellcheck_log" ]] || fail "ShellCheck ran after Git enumeration failed"
grep -Fq 'cannot enumerate tracked shell scripts' <<<"$enumeration_output" ||
  fail "ShellCheck helper did not explain Git enumeration failure"

shellcheck_missing_bin="$test_root/shellcheck-missing-bin"
mkdir -p "$shellcheck_missing_bin"
cat > "$shellcheck_missing_bin/git" <<EOF
#!/bin/sh
case "\${1:-}" in
  rev-parse)
    printf '%s\n' '$shellcheck_fixture'
    ;;
  *)
    exit 94
    ;;
esac
EOF
chmod +x "$shellcheck_missing_bin/git"
rm -f "$shellcheck_log"
set +e
missing_shellcheck_output=$(
  cd "$shellcheck_fixture"
  DEVTRIM_SHELLCHECK_LOG="$shellcheck_log" PATH="$shellcheck_missing_bin" \
    scripts/tests/shellcheck-tracked.sh 2>&1
)
missing_shellcheck_status=$?
set -e
[[ "$missing_shellcheck_status" -ne 0 ]] || fail "ShellCheck helper passed without shellcheck on PATH"
[[ ! -e "$shellcheck_log" ]] || fail "ShellCheck ran after shellcheck availability check failed"
grep -Fq 'shellcheck is required but was not found in PATH' <<<"$missing_shellcheck_output" ||
  fail "ShellCheck helper did not explain missing shellcheck"

shellcheck_outside="$test_root/shellcheck-outside"
mkdir -p "$shellcheck_outside"
cp "$shellcheck_script" "$shellcheck_outside/shellcheck-tracked.sh"
chmod +x "$shellcheck_outside/shellcheck-tracked.sh"
rm -f "$shellcheck_log"
set +e
outside_output=$(
  cd "$shellcheck_outside"
  DEVTRIM_SHELLCHECK_LOG="$shellcheck_log" PATH="$shellcheck_bin:$PATH" \
    ./shellcheck-tracked.sh 2>&1
)
outside_status=$?
set -e
[[ "$outside_status" -ne 0 ]] || fail "ShellCheck helper passed outside a Git worktree"
[[ ! -e "$shellcheck_log" ]] || fail "ShellCheck ran after Git worktree discovery failed"
grep -Fq 'cannot locate the Git worktree for shell lint' <<<"$outside_output" ||
  fail "ShellCheck helper did not explain Git worktree discovery failure"

tui_noop="$test_root/tui-noop"
printf '%s\n' '#!/bin/sh' 'exit 0' > "$tui_noop"
chmod +x "$tui_noop"
set +e
tui_noop_output=$(python3 "$script_dir/tui.py" "$tui_noop" 2>&1)
tui_noop_status=$?
set -e
[[ "$tui_noop_status" -ne 0 ]] || fail "PTY harness accepted a binary that never rendered"
grep -Fq 'TUI exited before rendering' <<<"$tui_noop_output" ||
  fail "PTY harness did not explain missing menu rendering"

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
printf '%s\n' 'This source tree and its packaged documentation describe devtrim v1.2.3.' > "$seed/README.md"
printf '%s\n' '    <span class="chip g">v1.2.3</span>' '  <span>devtrim <b>v1.2.3</b></span>' > "$seed/MANUAL.html"
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

for stale_surface in changelog readme readme-duplicate manual-chip manual-chip-duplicate manual-footer manual-footer-duplicate; do
  fixture="$test_root/version-$stale_surface"
  mkdir -p "$fixture/scripts"
  cp "$release_script" "$fixture/scripts/release.sh"
  printf '%s\n' '[package]' 'name = "fixture"' 'version = "1.2.3"' > "$fixture/Cargo.toml"
  printf '%s\n' '# Changelog' '## [1.2.3] - 2099-01-01' '- fixture' > "$fixture/CHANGELOG.md"
  printf '%s\n' 'This source tree and its packaged documentation describe devtrim v1.2.3.' > "$fixture/README.md"
  printf '%s\n' '    <span class="chip g">v1.2.3</span>' '  <span>devtrim <b>v1.2.3</b></span>' > "$fixture/MANUAL.html"
  case "$stale_surface" in
    changelog)
      printf '%s\n' '# Changelog' '## [1.2.2] - 2099-01-01' 'mentions v1.2.3 later' '## [1.2.3] - 2099-01-02' > "$fixture/CHANGELOG.md"
      expected_error='first CHANGELOG.md release heading is not 1.2.3'
      ;;
    readme)
      printf '%s\n' 'This source tree and its packaged documentation describe devtrim v1.2.2.' 'incidental v1.2.3 mention' > "$fixture/README.md"
      expected_error='README.md source-tree version != v1.2.3'
      ;;
    readme-duplicate)
      printf '%s\n' 'This source tree and its packaged documentation describe devtrim v1.2.3.' 'This source tree and its packaged documentation describe devtrim v1.2.2.' > "$fixture/README.md"
      expected_error='README.md must contain exactly one source-tree version declaration'
      ;;
    manual-chip)
      printf '%s\n' '    <span class="chip g">v1.2.2</span>' '  <span>devtrim <b>v1.2.3</b></span>' '<p>incidental v1.2.3 mention</p>' > "$fixture/MANUAL.html"
      expected_error='MANUAL.html version chip != v1.2.3'
      ;;
    manual-chip-duplicate)
      printf '%s\n' '    <span class="chip g">v1.2.3</span>' '    <span class="chip g">v1.2.2</span>' '  <span>devtrim <b>v1.2.3</b></span>' > "$fixture/MANUAL.html"
      expected_error='MANUAL.html must contain exactly one version chip'
      ;;
    manual-footer)
      printf '%s\n' '    <span class="chip g">v1.2.3</span>' '  <span>devtrim <b>v1.2.2</b></span>' '<p>incidental v1.2.3 mention</p>' > "$fixture/MANUAL.html"
      expected_error='MANUAL.html footer version != v1.2.3'
      ;;
    manual-footer-duplicate)
      printf '%s\n' '    <span class="chip g">v1.2.3</span>' '  <span>devtrim <b>v1.2.3</b></span>' '  <span>devtrim <b>v1.2.2</b></span>' > "$fixture/MANUAL.html"
      expected_error='MANUAL.html must contain exactly one footer version'
      ;;
  esac
  set +e
  stale_output=$(cd "$fixture" && bash scripts/release.sh 1.2.3 2>&1)
  stale_status=$?
  set -e
  [[ "$stale_status" -ne 0 ]] || fail "$stale_surface version drift unexpectedly passed"
  grep -Fq "$expected_error" <<<"$stale_output" ||
    fail "$stale_surface version drift did not report its authoritative surface"
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
