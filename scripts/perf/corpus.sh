#!/usr/bin/env bash
# Build a deterministic synthetic HOME for measuring devtrim scan performance.
# Every path is created under the directory given; a real home is refused.
set -euo pipefail

usage() {
  echo "usage: $0 <new-home-dir> [stale_repos=20] [recent_repos=5] [files_per_tree=100] [noise_dirs=20000]" >&2
  exit 2
}

corpus=${1:-}
[ "$#" -le 5 ] && [ -n "$corpus" ] || usage
stale_repos=${2:-20}
recent_repos=${3:-5}
files_per_tree=${4:-100}
noise_dirs=${5:-20000}

for count in "$stale_repos" "$recent_repos" "$files_per_tree" "$noise_dirs"; do
  [[ "$count" =~ ^(0|[1-9][0-9]{0,5})$ ]] || usage
done
if (( (stale_repos + recent_repos) * (4 * files_per_tree + 30) + noise_dirs > 1000000 )); then
  echo "refusing a corpus larger than 1000000 estimated entries" >&2
  exit 2
fi
parent=$(cd -- "$(dirname -- "$corpus")" && pwd -P) || usage
leaf=$(basename -- "$corpus")
corpus="$parent/$leaf"

case "$corpus" in
  / | "$HOME" | "$HOME"/)
    echo "refusing to build a corpus in a real home: $corpus" >&2
    exit 2
    ;;
esac
if [ -e "$corpus" ] || [ -L "$corpus" ]; then
  echo "refusing to overwrite an existing path: $corpus" >&2
  exit 2
fi
if ! command -v git >/dev/null 2>&1; then
  echo "git is required to build the repository fixtures" >&2
  exit 1
fi

mkdir -- "$corpus"
corpus=$(cd -- "$corpus" && pwd -P)
mkdir -p "$corpus/dev"

fill() {
  local dir=$1 count=$2 index
  mkdir -p "$dir"
  for ((index = 0; index < count; index++)); do
    printf 'payload %d\n' "$index" >"$dir/f$index"
  done
}

make_repo() {
  local dir=$1 commit_date=$2
  mkdir -p "$dir/src"
  printf 'fn main() {}\n' >"$dir/src/main.rs"
  printf '[package]\nname = "fixture"\nversion = "0.1.0"\n' >"$dir/Cargo.toml"
  printf '{ "name": "fixture", "private": true }\n' >"$dir/package.json"
  mkdir -p "$dir/.venv"
  printf 'home = /usr/bin\n' >"$dir/.venv/pyvenv.cfg"
  fill "$dir/node_modules/pkg-a" "$files_per_tree"
  fill "$dir/node_modules/pkg-b" "$files_per_tree"
  fill "$dir/target/debug" "$files_per_tree"
  fill "$dir/.venv/lib" "$files_per_tree"
  fill "$dir/__pycache__" 10
  fill "$dir/cache" 10
  printf 'Signature: 8a477f597d28d172789f06886806bc55\n' >"$dir/cache/CACHEDIR.TAG"
  env -i HOME="$corpus" PATH="$PATH" GIT_CONFIG_NOSYSTEM=1 git -c init.templateDir= -C "$dir" init -q
  env -i HOME="$corpus" PATH="$PATH" GIT_CONFIG_NOSYSTEM=1 \
    GIT_AUTHOR_DATE="$commit_date" GIT_COMMITTER_DATE="$commit_date" \
    git -C "$dir" -c user.name=perf -c user.email=perf@example.invalid \
    -c commit.gpgsign=false -c core.hooksPath=/dev/null \
    commit -q --allow-empty -m fixture
}

for ((index = 0; index < stale_repos; index++)); do
  make_repo "$corpus/dev/stale-$index" "2024-01-01T00:00:00Z"
done
now=$(date -u +%Y-%m-%dT%H:%M:%SZ)
for ((index = 0; index < recent_repos; index++)); do
  make_repo "$corpus/dev/recent-$index" "$now"
done

# Plain directories so directory-walk cost is visible; bucketed so no single
# directory holds more than 1000 entries.
if [ "$noise_dirs" -gt 0 ]; then
  for ((index = 1; index <= noise_dirs; index++)); do
    printf '%s\0' "$corpus/dev/noise/b$((index / 1000))/d$index"
  done | xargs -0 mkdir -p
fi

for index in 0 1 2; do
  fill "$corpus/Library/Developer/Xcode/DerivedData/App-$index/Build" "$files_per_tree"
done
fill "$corpus/.npm/_cacache" "$files_per_tree"
fill "$corpus/Library/Caches/Homebrew" "$files_per_tree"
fill "$corpus/.cache/uv" "$files_per_tree"
fill "$corpus/.cache/huggingface/hub" "$files_per_tree"
printf 'synthetic credential sentinel; never cleanup data\n' >"$corpus/.cache/huggingface/token"
mkdir -p "$corpus/Downloads" "$corpus/Desktop" "$corpus/.Trash"
for name in old.dmg old.pkg old.iso; do
  printf 'installer\n' >"$corpus/Downloads/$name"
  touch -t 202401010000 "$corpus/Downloads/$name"
done
printf 'installer\n' >"$corpus/Downloads/new.dmg"

# Stubs shaped like tests/cli.rs `Sandbox::script`; git is the real binary.
mkdir -p "$corpus/bin"
printf '#!/bin/sh\nexit 1\n' >"$corpus/bin/pgrep"
cat >"$corpus/bin/npm" <<'STUB'
#!/bin/sh
printf '%s\n' "$HOME/.npm"
STUB
cat >"$corpus/bin/brew" <<'STUB'
#!/bin/sh
printf '%s\n' "$HOME/Library/Caches/Homebrew"
STUB
chmod 755 "$corpus/bin/pgrep" "$corpus/bin/npm" "$corpus/bin/brew"
ln -s "$(command -v git)" "$corpus/bin/git"

printf 'corpus home: %s\n' "$corpus"
printf 'repos: %d stale, %d recent; noise directories: %d\n' "$stale_repos" "$recent_repos" "$noise_dirs"
printf 'entries: %s\n' "$(find "$corpus" | wc -l | tr -d ' ')"
printf 'run: HOME=%s PATH=%s/bin devtrim scan --json\n' "$corpus" "$corpus"
