#!/bin/sh
# Install reviewed local Git hooks into the repository's common hooks directory
# outside the working tree, so checking out another branch cannot replace or run
# hooks from `.githooks/`. The hooks are read from the local `main` branch's
# commit, never from the checked-out files, so running this on another branch
# still installs what `main` reviewed. Run once per clone and after `.githooks/`
# changes on `main`.
set -eu

hooks=$(git rev-parse --path-format=absolute --git-common-dir)/hooks
mkdir -p "$hooks"
for name in pre-commit pre-push; do
  staged="$hooks/.$name.devtrim-new"
  git show "refs/heads/main:.githooks/$name" >"$staged"
  chmod 0755 "$staged"
  mv -f "$staged" "$hooks/$name"
done
git config --local --replace-all core.hooksPath "$hooks"
