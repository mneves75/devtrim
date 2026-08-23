#!/usr/bin/env bash
# devtrim release: build → package → checksums → tag → GitHub release
# usage: scripts/release.sh <version>   (e.g. 0.1.0)
set -euo pipefail

ver="${1:?usage: release.sh <version>}"
tag="v${ver}"
cd "$(dirname "$0")/.."

echo "==> verifying version in Cargo.toml matches ${ver}"
grep -q "^version = \"${ver}\"$" Cargo.toml || { echo "ERROR: Cargo.toml version != ${ver}. Bump first."; exit 1; }
grep -q "## [${ver}]" CHANGELOG.md || echo "WARN: no CHANGELOG.md section for ${ver}"

echo "==> clean release build"
cargo build --release --locked 2>/dev/null || cargo build --release

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
git tag -f "${tag}"
git push origin "${tag}"
notes=$(awk -v hdr="## [${ver}]" 'index($0,hdr)==1{f=1;next} /^## /{if(f)exit} f' CHANGELOG.md)
gh release create "${tag}" "dist/${out}.zip" "dist/SHA256SUMS.txt" --title "devtrim ${ver}" --notes "$notes"
echo "==> released ${tag}"
