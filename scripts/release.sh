#!/usr/bin/env bash
# devtrim release: build → package → checksums → tag → GitHub release
# usage: scripts/release.sh <version>   (e.g. 0.2.0)
set -euo pipefail

ver="${1:?usage: release.sh <version>}"
tag="v${ver}"
cd "$(dirname "$0")/.."

echo "==> verifying release state"
grep -q "^version = \"${ver}\"$" Cargo.toml || { echo "ERROR: Cargo.toml version != ${ver}. Bump first."; exit 1; }
grep -Fq "## [${ver}]" CHANGELOG.md || { echo "ERROR: no CHANGELOG.md section for ${ver}"; exit 1; }
git diff --quiet && git diff --cached --quiet || { echo "ERROR: commit changes before releasing"; exit 1; }
git rev-parse "${tag}" >/dev/null 2>&1 && { echo "ERROR: tag ${tag} already exists"; exit 1; }

echo "==> clean release build"
cargo build --release --locked

echo "==> smoke test"
./target/release/devtrim --version

echo "==> packaging"
out="devtrim-${ver}-macos-arm64"
mkdir -p "dist/${out}"
cp target/release/devtrim "dist/${out}/"
cp MANUAL.html README.md LICENSE "dist/${out}/" 2>/dev/null || cp MANUAL.html README.md "dist/${out}/"
( cd dist && zip -qr "${out}.zip" "${out}" )
( cd dist && shasum -a 256 "${out}.zip" > SHA256SUMS.txt )
cat "dist/SHA256SUMS.txt"

echo "==> tag + release"
notes=$(awk -v hdr="## [${ver}]" 'index($0,hdr)==1{f=1;next} /^## /{if(f)exit} f' CHANGELOG.md)
[ -n "${notes}" ] || { echo "ERROR: empty release notes for ${ver}"; exit 1; }
git tag "${tag}"
git push origin "${tag}"
gh release create "${tag}" "dist/${out}.zip" "dist/SHA256SUMS.txt" --title "devtrim ${ver}" --notes "$notes"
echo "==> released ${tag}"
