#!/bin/sh
# Prove the installed detector rejects a non-allowlisted synthetic GitHub PAT.
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: $0 /absolute/path/to/gitleaks" >&2
  exit 1
fi

gitleaks_bin="$1"
if [ ! -x "$gitleaks_bin" ]; then
  echo "gitleaks positive control requires an executable: $gitleaks_bin" >&2
  exit 1
fi

set +e
printf '%s%s%s%s\n' 'token = "ghp_' 'dc831f20456cd20fa6' '112d38ca4eb7fdb8f2' '"' |
  "$gitleaks_bin" stdin --no-banner --redact --no-color
control_status=$?
set -e

if [ "$control_status" -ne 1 ]; then
  echo "gitleaks positive control expected leak exit 1, got $control_status" >&2
  exit 1
fi
