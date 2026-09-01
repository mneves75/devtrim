#!/bin/sh
# Lint every tracked *.sh plus the pre-commit hook without interpreting filenames as shell text.
set -eu

if ! repo_root=$(git rev-parse --show-toplevel 2>/dev/null); then
  echo "cannot locate the Git worktree for shell lint" >&2
  exit 1
fi
cd "$repo_root"

if ! command -v shellcheck >/dev/null 2>&1; then
  echo "shellcheck is required but was not found in PATH; install ShellCheck or run the CI tool installer" >&2
  exit 1
fi

shellcheck_paths=$(mktemp "${TMPDIR:-/tmp}/devtrim-shellcheck.XXXXXX")
cleanup_shellcheck_paths() {
  rm -f "$shellcheck_paths"
}
trap cleanup_shellcheck_paths EXIT

if ! git ls-files -z -- '*.sh' > "$shellcheck_paths"; then
  echo "cannot enumerate tracked shell scripts" >&2
  exit 1
fi

xargs -0 shellcheck -- .githooks/pre-commit < "$shellcheck_paths"
