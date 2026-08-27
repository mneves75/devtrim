#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
updater="$script_dir/../update-homebrew-formula.rb"
test_root=$(mktemp -d "${TMPDIR:-/tmp}/devtrim-homebrew-formula.XXXXXX")
trap 'rm -r "$test_root"' EXIT
source_formula="$test_root/source.rb"
updated_formula="$test_root/updated.rb"
expected_formula="$test_root/expected.rb"
new_url="https://github.com/mneves75/devtrim/releases/download/v1.2.3/devtrim-1.2.3-macos-arm64.zip"
new_checksum="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"

cat > "$source_formula" <<'EOF'
# retained header
class Devtrim < Formula
  desc "retained body"
  url "https://github.com/mneves75/devtrim/releases/download/v0.9.0/devtrim-0.9.0-macos-arm64.zip"
  version "0.9.0"
  sha256 "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"

  test do
    assert_match "retained test", "retained test"
  end
end
EOF
cat > "$expected_formula" <<EOF
# retained header
class Devtrim < Formula
  desc "retained body"
  url "${new_url}"
  sha256 "${new_checksum}"

  test do
    assert_match "retained test", "retained test"
  end
end
EOF
ruby "$updater" "$source_formula" "$updated_formula" "$new_url" "$new_checksum"
cmp -s "$updated_formula" "$expected_formula" || {
  echo "update-homebrew-formula: valid transformation did not preserve the formula body" >&2
  exit 1
}

expect_rejection() {
  local label="$1"
  local fixture="$2"
  if ruby "$updater" "$fixture" "$test_root/rejected.rb" "$new_url" "$new_checksum" >/dev/null 2>&1; then
    echo "update-homebrew-formula: accepted ${label}" >&2
    exit 1
  fi
}

cp "$source_formula" "$test_root/duplicate-url.rb"
printf '%s\n' '  url "https://github.com/mneves75/devtrim/releases/download/v0.9.0/devtrim-0.9.0-macos-arm64.zip"' >> "$test_root/duplicate-url.rb"
expect_rejection "duplicate URL" "$test_root/duplicate-url.rb"

cp "$source_formula" "$test_root/duplicate-sha.rb"
printf '%s\n' '  sha256 "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"' >> "$test_root/duplicate-sha.rb"
expect_rejection "duplicate checksum" "$test_root/duplicate-sha.rb"

cp "$source_formula" "$test_root/malformed-version.rb"
sed 's/version "0.9.0"/version "beta"/' "$source_formula" > "$test_root/malformed-version.rb"
expect_rejection "malformed explicit version" "$test_root/malformed-version.rb"

if ruby "$updater" "$source_formula" "$updated_formula" "https://example.invalid/devtrim.zip" "$new_checksum" >/dev/null 2>&1; then
  echo "update-homebrew-formula: accepted an untrusted destination URL" >&2
  exit 1
fi
if ruby "$updater" "$source_formula" "$updated_formula" "$new_url" "not-a-checksum" >/dev/null 2>&1; then
  echo "update-homebrew-formula: accepted an invalid checksum" >&2
  exit 1
fi

echo "update-homebrew-formula: all checks passed"
