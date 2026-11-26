#!/bin/sh
# Install reviewed local Git hooks into the repository's common hooks directory
# outside the working tree, so checking out another branch cannot replace or run
# hooks from `.githooks/`. Run from reviewed `main` once per clone and after
# `.githooks/` changes on `main`.
set -eu

repo_root=$(git rev-parse --show-toplevel)
hooks=$(git rev-parse --path-format=absolute --git-common-dir)/hooks
mkdir -p "$hooks"
git config --local --replace-all core.hooksPath "$hooks"
for name in pre-commit pre-push; do
  install -m 0755 "$repo_root/.githooks/$name" "$hooks/$name"
done
