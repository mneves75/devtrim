#!/usr/bin/env bash
# Successful, byte-identical scans over an isolated corpus, measured in both orders.
set -euo pipefail

usage() {
  echo "usage: $0 <baseline-binary> <candidate-binary> <corpus-home> [runs=15]" >&2
  exit 2
}
baseline=${1:-}
candidate=${2:-}
corpus=${3:-}
runs=${4:-15}
[ "$#" -le 4 ] && [ -f "$baseline" ] && [ -x "$baseline" ] && [ -f "$candidate" ] && [ -x "$candidate" ] || usage
[ -d "$corpus/dev" ] && [ -d "$corpus/bin" ] || usage
[[ "$runs" =~ ^[1-9][0-9]{0,3}$ ]] || usage
hyperfine_bin=$(command -v hyperfine) || { echo "hyperfine is required" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
baseline=$(cd -- "$(dirname -- "$baseline")" && pwd -P)/$(basename -- "$baseline")
candidate=$(cd -- "$(dirname -- "$candidate")" && pwd -P)/$(basename -- "$candidate")
corpus=$(cd -- "$corpus" && pwd -P)
out_dir=${PERF_OUT:-"$corpus-results-$(date +%Y%m%d-%H%M%S)-$$"}
# Never overwrite earlier evidence.
mkdir -- "$out_dir"
out_dir=$(cd -- "$out_dir" && pwd -P)

check_load() {
  local evidence=$1 ncpu loadavg load1
  ncpu=$(sysctl -n hw.ncpu)
  loadavg=$(sysctl -n vm.loadavg)
  printf 'loadavg=%s ncpu=%s\n' "$loadavg" "$ncpu" >"$evidence"
  load1=$(printf '%s\n' "$loadavg" | awk '{ print $2 }')
  if awk -v load="$load1" -v cpus="$ncpu" 'BEGIN { exit !(load > cpus / 2) }'; then
    if [ "${PERF_FORCE:-0}" != 1 ]; then
      echo "load $load1 exceeds half of $ncpu CPUs; refusing timing (PERF_FORCE=1 overrides)" >&2
      exit 2
    fi
    echo "WARNING: overloaded host; timings are indicative, not a verified speedup" | tee -a "$out_dir/warnings.txt" >&2
  fi
}
scan() {
  env -i HOME="$corpus" PATH="$corpus/bin" NO_COLOR=1 "$1" scan --json
}
scan "$baseline" >"$out_dir/baseline.json"
scan "$candidate" >"$out_dir/candidate.json"
python3 - "$out_dir/baseline.json" "$out_dir/candidate.json" <<'PY'
import json
import sys
for name in sys.argv[1:]:
    with open(name, encoding="utf-8") as source:
        result = json.load(source)
    if not isinstance(result, dict) or result.get("operation") != "scan" or result.get("errors") != [] or not isinstance(result.get("findings"), list):
        raise SystemExit(f"refusing invalid or failed scan document: {name}")
PY
if ! cmp -s "$out_dir/baseline.json" "$out_dir/candidate.json"; then
  echo "refusing timing: scan JSON differs" >&2
  exit 1
fi
{
  printf 'baseline: %s\n' "$baseline"
  "$baseline" --version
  shasum -a 256 "$baseline"
  printf 'candidate: %s\n' "$candidate"
  "$candidate" --version
  shasum -a 256 "$candidate"
  printf 'baseline build info (caller supplied): %s\n' "${PERF_BASELINE_BUILD_INFO:-not supplied; compiler unknown}"
  printf 'candidate build info (caller supplied): %s\n' "${PERF_CANDIDATE_BUILD_INFO:-not supplied; compiler unknown}"
  printf 'corpus: %s\nruns: %s\n' "$corpus" "$runs"
  "$hyperfine_bin" --version
} >"$out_dir/metadata.txt"
# Hyperfine's shell-free parser still needs quoted executable paths.
baseline_command=$(python3 -c 'import shlex,sys; print(shlex.quote(sys.argv[1]) + " scan --json")' "$baseline")
candidate_command=$(python3 -c 'import shlex,sys; print(shlex.quote(sys.argv[1]) + " scan --json")' "$candidate")
bench() {
  local label=$1 first=$2 second=$3
  check_load "$out_dir/$label-load-before.txt"
  env -i HOME="$corpus" PATH="$corpus/bin" NO_COLOR=1 "$hyperfine_bin" \
    --shell=none --warmup 3 --runs "$runs" \
    --export-json "$out_dir/$label.json" "$first" "$second"
  check_load "$out_dir/$label-load-after.txt"
}
bench baseline-first "$baseline_command" "$candidate_command"
bench candidate-first "$candidate_command" "$baseline_command"
# Timing must not leave the fixture or observations changed.
scan "$baseline" >"$out_dir/baseline-after.json"
scan "$candidate" >"$out_dir/candidate-after.json"
cmp "$out_dir/baseline.json" "$out_dir/baseline-after.json"
cmp "$out_dir/baseline.json" "$out_dir/candidate-after.json"
printf 'successful identical scans; results: %s\n' "$out_dir"
