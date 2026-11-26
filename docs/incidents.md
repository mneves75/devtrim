# Incident ledger

One row per defect a user, a review or a release gate met after it was
committed. `scripts/tests/audit-incidents.py` checks every row in CI:

- **Shipped**: a `CHANGELOG.md` version, or `Unreleased`.
- **Commit**: a commit reachable from `HEAD`. It may be `—` only while
  Shipped is `Unreleased`, or the version `Cargo.toml` declares and no
  `v<version>` tag exists yet, since a squash merge rewrites the branch
  commit.
- **Catcher**: names the check that now fails on the defect. It is either a
  `Case` in `scripts/tests/planted-violations.py`, which mutation-proves it,
  or exactly one test `cargo test --list` names and does not list again
  under `--ignored`, so a test compiled out or ignored does not count. A
  bare name defined more than once must be written as its full path.

A catcher renamed or deleted fails the audit. It must not silently stop
guarding its incident: `33fe4e2`'s two tests were removed in 0.11.0 and
nothing noticed until this ledger.

## Ledger

| ID | Shipped | Class | Commit | Catcher |
|---|---|---|---|---|
| D-01 | 0.7.0 | `clean docker` apply refused the report-only VM disk image, so every machine with one could not prune | 33fe4e2 | eval_clean_docker_prunes_images_and_build_cache_never_volumes |
| D-02 | 0.7.0 | The TUI menu was a hand-written list, so `clean installers` was unreachable | 33fe4e2 | every_home_row_names_its_effect_in_words_and_every_operation_fits |
| D-03 | 0.9.4 | A preview ran a program named in a scanned repository's `.git/config` | 728ec4f | git/signature-program |
| D-04 | 0.10.5 | One unreadable folder (`.fseventsd`) emptied `node-modules` and `artifacts` | b118350 | node_modules/unread-folders-reported |
| D-05 | 0.10.5 | A fresh `git init` repository emptied both project categories | b118350 | project/unborn-branch |
| D-06 | 0.12.6 | A version bump left `fuzz/Cargo.lock` stale, failing the release's clean-checkout gate | 13ccd96 | every_lockfile_pins_the_version_cargo_toml_declares |
| D-07 | 0.15.0 | A stop between the stop check and uv's lock left `<cache>/.lock` with no journal record | cf7962b | journal-stop/model |
| D-08 | 0.17.2 | `clean caches` offered npm's whole cache, including `_npx` installs a running language server loaded, and pnpm's `dlx` (Mole I-11) | — | caches/npm-store-name |
