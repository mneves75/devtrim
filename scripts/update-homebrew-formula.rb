#!/usr/bin/env ruby
# frozen_string_literal: true

abort "usage: update-homebrew-formula.rb <source> <destination> <url> <sha256>" unless ARGV.length == 4

source_path, destination_path, asset_url, checksum = ARGV
url_match = %r{\Ahttps://github\.com/mneves75/devtrim/releases/download/v([0-9]+\.[0-9]+\.[0-9]+)/devtrim-([0-9]+\.[0-9]+\.[0-9]+)-macos-arm64\.zip\z}.match(asset_url)
abort "invalid production archive URL" unless url_match && url_match[1] == url_match[2]
abort "invalid SHA-256" unless checksum.match?(/\A[0-9a-f]{64}\z/)

source = File.binread(source_path)
url_pattern = %r{^  url "https://github\.com/mneves75/devtrim/releases/download/v([0-9]+\.[0-9]+\.[0-9]+)/devtrim-([0-9]+\.[0-9]+\.[0-9]+)-macos-arm64\.zip"$}
version_pattern = /^  version "[0-9]+\.[0-9]+\.[0-9]+"$\n?/
sha_pattern = /^  sha256 "[0-9a-f]{64}"$/

url_lines = source.lines.grep(/\A  url /)
version_lines = source.lines.grep(/\A  version /)
sha_lines = source.lines.grep(/\A  sha256 /)
abort "expected exactly one trusted devtrim release URL" unless url_lines.length == 1
current_url = url_pattern.match(url_lines.first.chomp)
abort "expected exactly one trusted devtrim release URL" unless current_url && current_url[1] == current_url[2]
abort "expected at most one simple explicit version" unless version_lines.length <= 1
abort "expected at most one simple explicit version" if version_lines.one? && !version_pattern.match?(version_lines.first)
abort "expected exactly one SHA-256" unless sha_lines.length == 1 && sha_pattern.match?(sha_lines.first.chomp)

updated = source.sub(url_pattern, %(  url "#{asset_url}"))
updated = updated.sub(version_pattern, "")
updated = updated.sub(sha_pattern, %(  sha256 "#{checksum}"))
File.binwrite(destination_path, updated)
