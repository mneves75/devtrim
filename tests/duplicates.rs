//! Duplicate-file preview and Trash apply, through the real binary.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions fail by panicking"
)]

mod support;

use support::{Allowed, Sandbox, Tree, json, run, write};

fn plant(sandbox: &Sandbox) -> (std::path::PathBuf, std::path::PathBuf) {
    let root = sandbox.path().join("proj");
    let keeper = root.join("keep.bin");
    let extra = root.join("extra.bin");
    write(&keeper, "same-bytes");
    write(&extra, "same-bytes");
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(86_400);
    std::fs::File::options()
        .write(true)
        .open(&keeper)
        .unwrap()
        .set_modified(old)
        .unwrap();
    (keeper, extra)
}

#[test]
fn preview_lists_the_extra_copy_and_changes_nothing() {
    let sandbox = Sandbox::in_target("duplicates-preview");
    let (keeper, extra) = plant(&sandbox);
    let before = Tree::snapshot(sandbox.path());
    let output = run(
        &sandbox,
        &[
            "duplicates",
            "--min-bytes",
            "1",
            "--root",
            sandbox.path().join("proj").to_str().unwrap(),
            "--json",
        ],
    );
    let document = json(&output);
    assert!(output.status.success(), "{document}");
    assert_eq!(document["operation"], "duplicates");
    assert_eq!(document["applied"], false);
    let findings = document["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{document}");
    assert_eq!(findings[0]["action"]["type"], "trash");
    assert_eq!(findings[0]["path"], extra.to_str().unwrap());
    assert!(
        findings[0]["note"]
            .as_str()
            .unwrap()
            .contains(keeper.to_str().unwrap())
    );
    assert!(
        !document.to_string().contains("\"shred\""),
        "duplicates must not preview permanent deletion: {document}"
    );
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "duplicates/preview-unchanged",
    );
}

#[test]
fn apply_moves_only_the_extra_copy_to_trash() {
    let sandbox = Sandbox::in_target("duplicates-apply");
    let (keeper, extra) = plant(&sandbox);
    let before = Tree::snapshot(sandbox.path());
    let output = run(
        &sandbox,
        &[
            "duplicates",
            "--min-bytes",
            "1",
            "--root",
            sandbox.path().join("proj").to_str().unwrap(),
            "--apply",
            "-y",
            "--json",
        ],
    );
    let document = json(&output);
    assert!(output.status.success(), "{document}");
    assert_eq!(document["applied"], true);
    assert!(keeper.exists(), "kept copy must stay");
    assert!(!extra.exists(), "extra copy must leave the tree");
    assert_eq!(std::fs::read(&keeper).unwrap(), b"same-bytes");
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: vec![extra],
            scratch: vec![sandbox.path().join(".local")],
        },
        "duplicates/apply-only-extra",
    );
}

#[test]
fn apply_without_consent_changes_nothing() {
    let sandbox = Sandbox::in_target("duplicates-consent");
    let (keeper, extra) = plant(&sandbox);
    let before = Tree::snapshot(sandbox.path());
    let output = run(
        &sandbox,
        &[
            "duplicates",
            "--min-bytes",
            "1",
            "--root",
            extra.parent().unwrap().to_str().unwrap(),
            "--apply",
            "--json",
        ],
    );
    assert!(!output.status.success());
    assert_eq!(json(&output)["applied"], false);
    assert!(keeper.exists() && extra.exists());
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "duplicates/consent",
    );
}

#[test]
fn protected_copies_are_not_offered() {
    let sandbox = Sandbox::in_target("duplicates-protected");
    let root = sandbox.path().join(".ssh");
    write(&root.join("a.bin"), "same-bytes");
    write(&root.join("b.bin"), "same-bytes");
    let before = Tree::snapshot(sandbox.path());
    let output = run(
        &sandbox,
        &[
            "duplicates",
            "--min-bytes",
            "1",
            "--root",
            root.to_str().unwrap(),
            "--json",
        ],
    );
    let document = json(&output);
    assert!(output.status.success(), "{document}");
    let findings = document["findings"].as_array().unwrap();
    assert!(
        findings
            .iter()
            .all(|finding| finding["action"]["type"] != "trash"),
        "a protected directory must not gain a deletion plan: {document}"
    );
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "duplicates/protected",
    );
}

#[test]
fn scan_does_not_hash_duplicate_files() {
    let sandbox = Sandbox::in_target("duplicates-not-in-scan");
    plant(&sandbox);
    let output = run(
        &sandbox,
        &[
            "scan",
            "--root",
            sandbox.path().join("proj").to_str().unwrap(),
            "--json",
        ],
    );
    let document = json(&output);
    let rendered = document.to_string();
    assert!(
        !rendered.contains("extra copy"),
        "scan must not grow a duplicate plan: {document}"
    );
}
