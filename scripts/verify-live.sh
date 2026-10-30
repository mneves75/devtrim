#!/usr/bin/env bash
# devtrim live proof, run after each deploy and before the next tag: the site
# GitHub Pages serves (HTTP 200 over TLS, CSP meta present, bytes equal to the
# reviewed commit) and, given a version, its published release (immutable,
# checksum, provenance attestation).
# usage: scripts/verify-live.sh [<version>]   (e.g. 0.12.2-beta1 or 0.12.2)
#   DEVTRIM_SITE_URL  site to check, ending in / (default https://mneves75.github.io/devtrim/)
#   DEVTRIM_SITE_REF  commit the served files must equal (default origin/main, fetched first)
set -euo pipefail
cd "$(dirname "$0")/.."

[[ $# -le 1 ]] || { echo "ERROR: usage: scripts/verify-live.sh [<version>]"; exit 2; }
site_url="${DEVTRIM_SITE_URL:-https://mneves75.github.io/devtrim/}"
site_ref="${DEVTRIM_SITE_REF:-origin/main}"
# Production is HTTPS only; plain HTTP is accepted for a loopback control server.
[[ "$site_url" =~ ^https://[^/]+/ || "$site_url" =~ ^http://127\.0\.0\.1:[0-9]+/ ]] || {
  echo "ERROR: DEVTRIM_SITE_URL must be https:// (or http://127.0.0.1:<port>/ for a control)"
  exit 2
}
[[ "$site_url" == */ ]] || { echo "ERROR: DEVTRIM_SITE_URL must end in /"; exit 2; }

work=$(mktemp -d "/tmp/devtrim-verify-live.XXXXXX")
trap 'rm -r "$work"' EXIT
failures=0
pass() { printf 'PASS %s\n' "$*"; }
failed() { printf 'FAIL %s\n' "$*"; failures=$((failures + 1)); }

if [[ -z "${DEVTRIM_SITE_REF:-}" ]]; then
  git -c submodule.recurse=false fetch --quiet --no-tags origin \
    "refs/heads/main:refs/remotes/origin/main" || { echo "ERROR: cannot refresh origin/main"; exit 1; }
fi
site_commit=$(git rev-parse --verify "${site_ref}^{commit}") || { echo "ERROR: unknown ref $site_ref"; exit 1; }
echo "==> site ${site_url} against ${site_ref} (${site_commit})"

# check_page <published path> <file in the commit> <requires CSP meta: yes|no>
check_page() {
  local url="${site_url}$1" served="$work/served-$2" expected="$work/expected-$2" headers="$work/headers-$2" status
  if ! status=$(curl --silent --show-error --proto '=https,http' --max-time 30 \
    --dump-header "$headers" --output "$served" --write-out '%{http_code}' "$url"); then
    failed "$url could not be fetched"
    return
  fi
  [[ "$status" == 200 ]] || { failed "$url answered HTTP $status"; return; }
  if [[ "$3" == yes ]]; then
    if grep -Fq '<meta http-equiv="Content-Security-Policy"' "$served"; then
      pass "$url carries its CSP meta"
    else
      failed "$url serves no CSP meta"
    fi
  fi
  git show "${site_commit}:$2" > "$expected"
  if cmp -s "$served" "$expected"; then
    pass "$url equals $2 at ${site_commit:0:12}"
  else
    # Pages caches for minutes: the headers tell a stale cache from real drift.
    failed "$url differs from $2 at ${site_commit:0:12}: served $(shasum -a 256 "$served" | cut -c1-16), expected $(shasum -a 256 "$expected" | cut -c1-16); $(grep -i -E '^(age|last-modified):' "$headers" | tr -d '\r' | tr '\n' ' ')"
  fi
}

check_page "" index.html yes
check_page MANUAL.html MANUAL.html yes
check_page styles.css styles.css no

if [[ $# -eq 1 ]]; then
  release="$1"
  [[ "$release" =~ ^([0-9]+\.[0-9]+\.[0-9]+)(-beta[1-9][0-9]*)?$ ]] || {
    echo "ERROR: version must be X.Y.Z or X.Y.Z-betaN"
    exit 2
  }
  ver="${BASH_REMATCH[1]}"
  prerelease=false
  [[ -z "${BASH_REMATCH[2]:-}" ]] || prerelease=true
  tag="v${release}"
  out="devtrim-${ver}-macos-arm64"
  repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
  echo "==> release ${tag} in ${repo}"
  release_commit=$(git ls-remote origin "refs/tags/${tag}^{}" | cut -f1)
  [[ "$release_commit" =~ ^[0-9a-f]{40}$ ]] || { failed "tag ${tag} is not published as an annotated tag"; release_commit=""; }
  if [[ -n "$release_commit" ]]; then
    state=$(gh release view "$tag" --json tagName,isImmutable,isPrerelease \
      --jq '[.tagName, .isImmutable, .isPrerelease] | @tsv' || true)
    if [[ "$state" == "$tag"$'\ttrue\t'"$prerelease" ]]; then
      pass "${tag} is immutable with prerelease=${prerelease}"
    else
      failed "${tag} release state is '${state}', expected immutable with prerelease=${prerelease}"
    fi
    if gh release verify "$tag" --format json >/dev/null; then
      pass "${tag} release attestation verifies"
    else
      failed "${tag} release attestation does not verify"
    fi
    if gh release download "$tag" --dir "$work/release" --pattern "${out}.zip" --pattern SHA256SUMS.txt &&
      (cd "$work/release" && shasum -a 256 -c SHA256SUMS.txt >/dev/null); then
      pass "${out}.zip matches SHA256SUMS.txt"
      attestation_args=(
        --repo "$repo"
        --signer-workflow "$repo/.github/workflows/release.yml"
        --source-digest "$release_commit"
        --deny-self-hosted-runners
      )
      [[ "$prerelease" == false ]] || attestation_args+=(--source-ref "refs/tags/${tag}")
      if gh attestation verify "$work/release/${out}.zip" "${attestation_args[@]}" >/dev/null; then
        pass "${out}.zip provenance names ${release_commit:0:12} and the hosted release workflow"
      else
        failed "${out}.zip provenance does not verify against ${release_commit:0:12}"
      fi
    else
      failed "${out}.zip could not be downloaded or does not match SHA256SUMS.txt"
    fi
  fi
fi

if [[ "$failures" -eq 0 ]]; then
  echo "verify-live: all checks passed"
else
  echo "verify-live: ${failures} check(s) failed"
  exit 1
fi
