//! Feature evals: each command run through the real binary against a planted
//! fixture, proving it does what it says and nothing else. A preview must
//! leave the fixture as it was; an apply must remove exactly the previewed
//! targets, journal each one write-ahead, and leave every near miss — chosen
//! from the feature's own documented exclusions — byte for byte in place.
//!
//! The oracle (`support::Tree`) is itself proven by the controls below: it
//! must reject each kind of unauthorized change and accept a legitimate one.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions fail by panicking"
)]

mod support;

use std::path::{Path, PathBuf};
use support::{
    Allowed, Interactive, Sandbox, Tree, actionable_targets, assert_journaled, json, run,
};

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

// ---------- oracle controls ----------

/// A small tree with a file, a nested file, and a link, for the controls.
fn control_fixture(name: &str) -> (Sandbox, PathBuf) {
    let sandbox = Sandbox::new(name);
    let root = sandbox.path().join("tree");
    write(&root.join("keep.txt"), "keep");
    write(&root.join("dir/nested.txt"), "nested");
    write(&root.join("doomed/inner.txt"), "doomed");
    std::os::unix::fs::symlink("keep.txt", root.join("link")).unwrap();
    (sandbox, root)
}

#[test]
fn oracle_accepts_exactly_the_allowed_removal() {
    let (_sandbox, root) = control_fixture("oracle-ok");
    let before = Tree::snapshot(&root);
    std::fs::remove_dir_all(root.join("doomed")).unwrap();
    let allowed = Allowed {
        removed: vec![root.join("doomed")],
        ..Allowed::default()
    };
    // The parent's modification time changed; that alone is not a violation.
    before.diff(&Tree::snapshot(&root), &allowed).unwrap();
}

#[test]
fn oracle_rejects_every_unauthorized_change() {
    type Mutation = fn(&Path);
    // Each case names the violation the oracle must report, and where.
    let cases: [(&str, &str, &str, Mutation); 8] = [
        ("modified content", "changed: ", "keep.txt", |root| {
            std::fs::write(root.join("keep.txt"), "KEEP").unwrap()
        }),
        ("created file", "created: ", "new.txt", |root| {
            std::fs::write(root.join("new.txt"), "x").unwrap()
        }),
        (
            "extra deletion",
            "removed without authority: ",
            "dir/nested.txt",
            |root| std::fs::remove_file(root.join("dir/nested.txt")).unwrap(),
        ),
        ("retargeted link", "changed: ", "link", |root| {
            std::fs::remove_file(root.join("link")).unwrap();
            std::os::unix::fs::symlink("dir/nested.txt", root.join("link")).unwrap();
        }),
        ("permission change", "changed: ", "keep.txt", |root| {
            use std::os::unix::fs::PermissionsExt;
            let path = root.join("keep.txt");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }),
        ("modification time only", "changed: ", "keep.txt", |root| {
            let file = std::fs::File::options()
                .write(true)
                .open(root.join("keep.txt"))
                .unwrap();
            file.set_modified(std::time::SystemTime::UNIX_EPOCH)
                .unwrap();
        }),
        (
            "same-content replacement",
            "changed: ",
            "keep.txt",
            |root| {
                // Same bytes, same mtime: only the identity differs.
                let path = root.join("keep.txt");
                let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
                std::fs::rename(&path, root.join("old")).unwrap();
                std::fs::write(&path, "keep").unwrap();
                std::fs::remove_file(root.join("old")).unwrap();
                let file = std::fs::File::options().write(true).open(&path).unwrap();
                file.set_modified(modified).unwrap();
            },
        ),
        (
            "planned removal skipped",
            "expected removal survived: ",
            "doomed",
            |_| {},
        ),
    ];
    for (name, kind, leaf, mutate) in cases {
        let (_sandbox, root) = control_fixture("oracle-violation");
        let before = Tree::snapshot(&root);
        if name != "planned removal skipped" {
            std::fs::remove_dir_all(root.join("doomed")).unwrap();
        }
        mutate(&root);
        let allowed = Allowed {
            removed: vec![root.join("doomed")],
            ..Allowed::default()
        };
        let problems = before
            .diff(&Tree::snapshot(&root), &allowed)
            .expect_err(&format!("the oracle accepted a {name}"));
        let expected = format!("{kind}{}", root.join(leaf).display());
        assert!(
            problems.lines().any(|line| line == expected),
            "the oracle missed the {name}: wanted `{expected}`, got:\n{problems}"
        );
    }
}

// ---------- clean caches ----------

/// The `clean caches` fixture: every listed cache, planted, and the near
/// misses its documentation excludes. Returns the targets it must remove.
fn caches_fixture(sandbox: &Sandbox) -> Vec<PathBuf> {
    let home = sandbox.path();
    sandbox.script("npm", "printf '%s\\n' \"$HOME/.npm\"");
    sandbox.script("brew", "printf '%s\\n' \"$HOME/Library/Caches/Homebrew\"");
    let targets: Vec<PathBuf> = [
        ".cache/huggingface/hub",
        ".cache/uv",
        ".cache/node",
        ".bun/install/cache",
        ".cache/gh",
        ".cargo/registry/cache",
        ".cargo/registry/src",
        "Library/Caches/pip",
        "Library/Caches/go-build",
        "Library/Caches/pnpm",
        "Library/Caches/org.swift.swiftpm",
        ".npm",
        // Homebrew's cache holds a Git clone, so it is offered as its other children.
        "Library/Caches/Homebrew/downloads",
        "Library/Caches/Homebrew/bootsnap",
    ]
    .iter()
    .map(|relative| home.join(relative))
    .collect();
    for target in &targets {
        write(&target.join("payload.bin"), "regenerable");
    }
    // Near misses, each named by the rule that keeps it.
    for relative in [
        ".cache/huggingface/token", // HF tokens and settings are never authority
        ".cache/huggingface/settings",
        "Library/pnpm/store/v10/index.json", // the pnpm store, unlike its metadata cache
        "Library/Caches/JetBrains/IDEA/LocalHistory/changes", // IDE system dir with LocalHistory
        "Library/Caches/deno/location_data/kv", // DENO_DIR holds KV databases
        "Library/Caches/ms-playwright-extra/x", // a neighbour sharing a listed prefix
        "Library/Caches/com.apple.Safari/x", // an unlisted Library cache
        ".cargo/registry/index/x",           // cargo's index is not listed
        ".cargo/bin/cargo",
        "Library/Caches/Homebrew/omlx--git/.git/HEAD", // a Homebrew Git clone stays
        "Library/Caches/Homebrew/omlx--git/README",
    ] {
        write(&home.join(relative), "keep");
    }
    let mut targets = targets;
    targets.sort();
    targets
}

#[test]
fn eval_clean_caches_removes_every_listed_cache_and_nothing_else() {
    let sandbox = Sandbox::in_target("eval-caches");
    let targets = caches_fixture(&sandbox);
    let pristine = Tree::snapshot(sandbox.path());

    let preview = run(&sandbox, &["clean", "caches", "--shred", "--json"]);
    let document = json(&preview);
    assert!(preview.status.success(), "{document}");
    assert_eq!(
        actionable_targets(&document),
        targets,
        "PV eval/caches-plan: the preview must offer exactly the listed caches"
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "PV eval/caches-preview",
    );

    let apply = run(
        &sandbox,
        &["clean", "caches", "--apply", "--shred", "--yolo", "--json"],
    );
    let document = json(&apply);
    assert!(apply.status.success(), "{document}");
    assert_eq!(document["summary"]["items_touched"], targets.len());
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: targets.clone(),
            scratch: vec![sandbox.path().join(".local")],
        },
        "PV eval/caches-apply",
    );
    assert_journaled(&sandbox, "caches", "shred", &targets);
}

/// Write-ahead: when the attempt cannot be journaled, nothing is removed.
/// Counting attempt/result pairs after a successful run cannot show this;
/// both could have been written after the deletion.
#[test]
fn eval_an_unwritable_journal_prevents_every_removal() {
    use std::os::unix::fs::PermissionsExt;
    let sandbox = Sandbox::in_target("eval-write-ahead");
    caches_fixture(&sandbox);
    let journal = sandbox.journal();
    write(&journal, "");
    std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o400)).unwrap();
    let pristine = Tree::snapshot(sandbox.path());

    let apply = run(
        &sandbox,
        &["clean", "caches", "--apply", "--shred", "--yolo", "--json"],
    );
    let document = json(&apply);
    assert!(
        !apply.status.success(),
        "PV eval/write-ahead: an unjournaled apply must fail: {document}"
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            // devtrim's state folder (it tightens it to 0700 and keeps its
            // apply lock there; the journal itself is checked below), and
            // uv's own lock file: apply takes uv's lock, creating it as uv
            // does, before the sink journals the cache. Nothing else.
            scratch: vec![
                journal.parent().unwrap().to_path_buf(),
                sandbox.path().join(".cache/uv/.lock"),
            ],
            ..Allowed::default()
        },
        "PV eval/write-ahead",
    );
    assert_eq!(
        std::fs::read_to_string(&journal).unwrap(),
        "",
        "PV eval/write-ahead: journal"
    );
}

// ---------- the plan is immutable once previewed ----------

/// Apply consumes exactly the previewed findings: a target that appears
/// after the preview is never touched, and an approved target replaced after
/// the preview is refused rather than removed. Drives a real terminal so the
/// change happens inside one invocation, between consent and apply.
#[test]
fn eval_apply_ignores_new_targets_and_refuses_swapped_ones_after_preview() {
    let sandbox = Sandbox::in_target("eval-drift");
    let home = sandbox.path();
    for relative in [
        ".cache/node",
        ".cache/huggingface/hub",
        "Library/Caches/pip",
    ] {
        write(&home.join(relative).join("payload.bin"), "previewed");
    }
    let mut session = Interactive::start(&sandbox, &["clean", "caches", "--apply", "--shred"]);
    session.wait_for("Type the number to continue:");

    // After consent was asked for, before it is given.
    write(
        &home.join(".cache/gh/appeared.bin"),
        "appeared after preview",
    );
    std::fs::remove_dir_all(home.join(".cache/node")).unwrap();
    write(
        &home.join(".cache/node/payload.bin"),
        "replaced after preview",
    );

    session.send("0");
    let (status, output) = session.finish();
    assert_eq!(
        std::fs::read_to_string(home.join(".cache/gh/appeared.bin"))
            .ok()
            .as_deref(),
        Some("appeared after preview"),
        "PV eval/drift-new-target: a target found after the preview was removed:\n{output}"
    );
    assert_eq!(
        std::fs::read_to_string(home.join(".cache/node/payload.bin"))
            .ok()
            .as_deref(),
        Some("replaced after preview"),
        "PV eval/drift-swapped-target: a target replaced after the preview was removed:\n{output}"
    );
    assert!(
        !status.success(),
        "the swapped target must make the run fail:\n{output}"
    );
    assert!(
        output.contains("identity changed after preview"),
        "{output}"
    );
    assert!(!home.join(".cache/huggingface/hub").exists(), "{output}");
    assert!(!home.join("Library/Caches/pip").exists(), "{output}");
}
