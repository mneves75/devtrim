//! A version bump must reach both lockfiles. The fuzz crate depends on
//! `devtrim` by path, so `fuzz/Cargo.lock` keeps the old version until it is
//! updated by hand; 0.12.6-beta1 reached the hosted release gate with it
//! stale, which failed there after every local gate had passed.

#![allow(clippy::panic, reason = "test assertions fail by panicking")]

use std::path::Path;

/// The version a lockfile records for the package `name`.
fn locked_version(lock: &str, name: &str) -> Option<String> {
    let wanted = format!("name = \"{name}\"");
    lock.split("[[package]]").find_map(|entry| {
        let mut lines = entry.lines().map(str::trim);
        lines.any(|line| line == wanted).then(|| {
            entry.lines().map(str::trim).find_map(|line| {
                line.strip_prefix("version = \"")
                    .and_then(|rest| rest.strip_suffix('"'))
                    .map(str::to_owned)
            })
        })?
    })
}

#[test]
fn every_lockfile_pins_the_version_cargo_toml_declares() {
    let version = env!("CARGO_PKG_VERSION");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for lock in ["Cargo.lock", "fuzz/Cargo.lock"] {
        let text = std::fs::read_to_string(root.join(lock))
            .unwrap_or_else(|error| panic!("{lock}: {error}"));
        assert_eq!(
            locked_version(&text, "devtrim").as_deref(),
            Some(version),
            "{lock} pins another devtrim than Cargo.toml's {version}; \
             run `cargo update -p devtrim --precise {version}` beside it"
        );
    }
}
