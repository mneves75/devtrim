#!/usr/bin/env bash
# Local offline gates; prerequisite installation is deliberately separate.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
mode=${1:-focused}
if [[ $# -gt 1 || ( "$mode" != focused && "$mode" != offline ) ]]; then
  echo 'usage: scripts/verify.sh [focused|offline]' >&2
  exit 2
fi
run() {
  local gate=$1
  shift
  local result=0
  "$@" || result=$?
  printf '%s: exit %s\n' "$gate" "$result"
  [[ $result -eq 0 ]] || exit "$result"
}
# Select the installed compiler explicitly: standalone Cargo can shadow rustup.
toolchain=$(sed -n 's/^channel = "\([^"]*\)"$/\1/p' rust-toolchain.toml)
[[ -n "$toolchain" ]] || { echo 'missing Rust toolchain pin' >&2; exit 1; }
compiler=$(rustup which rustc --toolchain "$toolchain")
compiler_bin=$(dirname "$compiler")
export PATH="$compiler_bin:$PATH"
export CARGO_NET_OFFLINE=true
run agent-docs cmp -s AGENTS.md CLAUDE.md
run compiler rustc --version
run format cargo fmt --all -- --check
run structure-tests ast-grep test --skip-snapshot-tests
run structure ast-grep scan --config sgconfig.yml
run clippy cargo clippy --locked --all-targets --all-features -- -D warnings
run tests cargo test --locked --all-targets --all-features
run build cargo build --locked
run tui python3 scripts/tests/tui.py "${CARGO_TARGET_DIR:-target}/debug/devtrim"
run read-only-views python3 scripts/tests/read-only-views.py "${CARGO_TARGET_DIR:-target}/debug/devtrim"
if [[ "$mode" == offline ]]; then
  for script in scripts/verify.sh scripts/release.sh scripts/update-homebrew.sh scripts/tests/release-policy.sh scripts/tests/update-homebrew-formula.sh scripts/perf/ab.sh scripts/perf/corpus.sh; do
    run "bash-syntax:$script" bash -n "$script"
  done
  for script in .githooks/pre-commit scripts/tests/shellcheck-tracked.sh scripts/tests/gitleaks-positive-control.sh; do
    run "sh-syntax:$script" sh -n "$script"
  done
  run shellcheck scripts/tests/shellcheck-tracked.sh
  run workflow actionlint
  run release-policy bash scripts/tests/release-policy.sh
  run msrv rustup run 1.88.0 cargo test --locked --all-targets --all-features
  run audit cargo audit --no-fetch
  run fuzz-audit cargo audit --no-fetch --file fuzz/Cargo.lock
  run gitleaks-control scripts/tests/gitleaks-positive-control.sh "$(command -v gitleaks)"
  run secrets gitleaks git --redact --no-banner .
  # Online advisory refresh, verified secret scans, fuzzing, and video install
  # remain explicit delivery gates; this helper never initiates network access.
fi

if [[ "$mode" == focused ]]; then
  echo "Not run: offline-mode MSRV, shell/workflow/policy checks, cached audits, and Gitleaks."
fi
printf '%s\n' "$mode checks complete. Not run: online advisory refresh, TruffleHog, bounded fuzzing, video install/audit/lint/format/build, release artifact verification, autoreview."
