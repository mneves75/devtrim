//! App-storage reports measure application data without creating cleanup authority.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions fail by panicking"
)]

mod support;

use serde_json::Value;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::Path;
use support::{Allowed, Sandbox, Tree, json, on_disk, run, write};

#[test]
fn reports_whatsapp_media_database_and_other_data_without_writing() {
    let sandbox = Sandbox::new("app-storage-whatsapp");
    let container = sandbox
        .path()
        .join("Library/Group Containers/group.net.whatsapp.WhatsApp.shared");
    let media = container.join("Message/Media/photo.bin");
    let database = container.join("ChatStorage.sqlite");
    let other = container.join("stickers/sticker.bin");
    write(&media, "media fixture");
    write(
        &database,
        "not a database: this command must only measure metadata",
    );
    write(&other, "sticker fixture");
    let before = Tree::snapshot(sandbox.path());

    let output = run(&sandbox, &["app-storage", "--json"]);
    let document = json(&output);
    assert!(output.status.success(), "{document}");
    assert_eq!(document["operation"], "app-storage");
    assert_eq!(document["applied"], false);
    assert_eq!(document["errors"], serde_json::json!([]));
    let findings = document["findings"].as_array().unwrap();
    assert_eq!(
        findings
            .iter()
            .map(|finding| finding["size_bytes"].as_u64().unwrap())
            .sum::<u64>(),
        on_disk(&[&media, &database, &other])
    );
    for (path, label) in [
        (container.join("Message/Media"), "media"),
        (database, "database"),
    ] {
        assert!(
            findings.iter().any(|finding| {
                finding["path"] == path.canonicalize().unwrap().to_string_lossy().as_ref()
                    && finding["label"].as_str().unwrap().contains(label)
            }),
            "missing {label}: {document}"
        );
    }
    assert!(
        findings
            .iter()
            .all(|finding| finding["action"]["type"] == "info")
    );
    assert!(findings.iter().any(|finding| {
        finding["note"]
            .as_str()
            .unwrap()
            .contains("not deletion authority")
    }));
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "app-storage/read-only",
    );
}

fn reported_bytes(document: &Value) -> u64 {
    document["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["size_bytes"].as_u64().unwrap())
        .sum()
}

#[test]
fn screen_studio_search_is_explicit_and_prunes_bundles_and_dependency_trees() {
    let sandbox = Sandbox::new("app-storage-studio");
    let roots = sandbox.path().join("Movies");
    let recording = roots.join("edit.screenstudio/recording.bin");
    write(&recording, "original recording");
    write(
        &roots.join("node_modules/hidden.screenstudio/data"),
        "dependency",
    );
    write(&roots.join(".GIT/hidden.screenstudio/data"), "git metadata");
    write(
        &roots.join("Packaged.app/hidden.screenstudio/data"),
        "application",
    );
    let canonical = roots.canonicalize().unwrap();
    let before = Tree::snapshot(sandbox.path());

    let implicit = run(&sandbox, &["app-storage", "--json"]);
    assert!(implicit.status.success());
    assert_eq!(reported_bytes(&json(&implicit)), 0);
    let output = run(
        &sandbox,
        &[
            "app-storage",
            "--root",
            canonical.to_str().unwrap(),
            "--json",
        ],
    );
    let document = json(&output);
    assert!(output.status.success(), "{document}");
    assert_eq!(reported_bytes(&document), on_disk(&[&recording]));
    assert!(
        document["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| {
                finding["path"]
                    == canonical
                        .join("edit.screenstudio")
                        .to_string_lossy()
                        .as_ref()
            })
    );
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "app-storage/explicit-roots",
    );
}

#[test]
fn all_mutation_flags_are_rejected_in_the_json_envelope() {
    let sandbox = Sandbox::new("app-storage-flags");
    let before = Tree::snapshot(sandbox.path());
    for flag in ["--apply", "--yes", "--yolo", "--shred"] {
        let output = run(&sandbox, &["app-storage", flag, "--json"]);
        let document = json(&output);
        assert_eq!(output.status.code(), Some(2), "{flag}: {document}");
        assert_eq!(document["operation"], "app-storage");
        assert_eq!(document["applied"], false);
        assert!(!document["errors"].as_array().unwrap().is_empty());
    }
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "app-storage/mutation-flags",
    );
}

#[test]
fn sparse_files_and_hard_links_are_counted_in_allocated_bytes_once() {
    let sandbox = Sandbox::new("app-storage-allocation");
    let media = sandbox
        .path()
        .join("Library/Group Containers/group.net.whatsapp.WhatsApp.shared/Message/Media");
    let original = media.join("original");
    write(&original, "one allocation");
    std::fs::hard_link(&original, media.join("duplicate-link")).unwrap();
    let sparse = media.join("sparse");
    let file = std::fs::File::create(&sparse).unwrap();
    file.set_len(1024 * 1024 * 1024).unwrap();
    drop(file);
    let before = Tree::snapshot(sandbox.path());
    let output = run(&sandbox, &["app-storage", "--json"]);
    let document = json(&output);
    assert!(output.status.success(), "{document}");
    assert_eq!(reported_bytes(&document), on_disk(&[&original, &sparse]));
    assert!(reported_bytes(&document) < 1024 * 1024 * 1024);
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "app-storage/allocated-bytes",
    );
}

#[test]
fn symlinked_container_and_search_roots_are_refused_without_reading_their_targets() {
    let sandbox = Sandbox::new("app-storage-links");
    let outside = sandbox.path().join("outside");
    write(&outside.join("secret.screenstudio/private"), "not in scope");
    let groups = sandbox.path().join("Library/Group Containers");
    std::fs::create_dir_all(&groups).unwrap();
    symlink(&outside, groups.join("group.net.whatsapp.WhatsApp.shared")).unwrap();
    let linked = sandbox.path().join("linked");
    symlink(&outside, &linked).unwrap();
    let before = Tree::snapshot(sandbox.path());
    for args in [
        vec!["app-storage", "--json"],
        vec!["app-storage", "--root", linked.to_str().unwrap(), "--json"],
    ] {
        let output = run(&sandbox, &args);
        let document = json(&output);
        assert!(!output.status.success(), "{document}");
        assert_eq!(reported_bytes(&document), 0);
        assert!(!document["errors"].as_array().unwrap().is_empty());
    }
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "app-storage/no-follow",
    );
}

#[test]
fn invalid_search_roots_fail_with_an_operation_named_error() {
    let sandbox = Sandbox::new("app-storage-invalid");
    let regular = sandbox.path().join("regular-file");
    write(&regular, "not a search directory");
    let long = "x".repeat(4096);
    for root in [
        "",
        "/definitely-missing-devtrim-app-storage",
        regular.to_str().unwrap(),
        &long,
    ] {
        let output = run(&sandbox, &["app-storage", "--root", root, "--json"]);
        let document = json(&output);
        assert!(!output.status.success(), "root {root:?}: {document}");
        assert_eq!(document["operation"], "app-storage");
        assert!(!document["errors"].as_array().unwrap().is_empty());
    }
}

struct RestorePermissions<'a>(&'a Path);

impl Drop for RestorePermissions<'_> {
    fn drop(&mut self) {
        std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn an_explicit_child_root_is_measured_when_its_parent_cannot_be_listed() {
    let sandbox = Sandbox::new("app-storage-child-root");
    let parent = sandbox.path().join("Movies");
    let child = parent.join("known.screenstudio");
    let recording = child.join("raw");
    write(
        &recording,
        "a reachable project under an unreadable listing",
    );
    let canonical_parent = parent.canonicalize().unwrap();
    let canonical_child = child.canonicalize().unwrap();
    let before = Tree::snapshot(sandbox.path());
    let output = {
        let _restore = RestorePermissions(&parent);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o111)).unwrap();
        let positive = run(
            &sandbox,
            &[
                "app-storage",
                "--root",
                canonical_child.to_str().unwrap(),
                "--json",
            ],
        );
        assert!(positive.status.success(), "{positive:?}");
        assert_eq!(reported_bytes(&json(&positive)), on_disk(&[&recording]));
        run(
            &sandbox,
            &[
                "app-storage",
                "--root",
                canonical_parent.to_str().unwrap(),
                "--root",
                canonical_child.to_str().unwrap(),
                "--json",
            ],
        )
    };
    let document = json(&output);
    assert_eq!(output.status.code(), Some(1), "{document}");
    assert!(!document["errors"].as_array().unwrap().is_empty());
    assert_eq!(reported_bytes(&document), on_disk(&[&recording]));
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "app-storage/explicit-child",
    );
}

#[test]
fn unreadable_data_is_partial_and_does_not_hide_other_application_data() {
    let sandbox = Sandbox::new("app-storage-partial");
    let container = sandbox
        .path()
        .join("Library/Group Containers/group.net.whatsapp.WhatsApp.shared");
    let blocked = container.join("Message/Media/blocked");
    write(&blocked.join("private"), "unreadable");
    let database = container.join("ChatStorage.sqlite");
    write(&database, "visible metadata");
    let before = Tree::snapshot(sandbox.path());
    let output = {
        let _restore = RestorePermissions(&blocked);
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
        run(&sandbox, &["app-storage", "--json"])
    };
    let document = json(&output);
    assert_eq!(output.status.code(), Some(1), "{document}");
    assert!(!document["errors"].as_array().unwrap().is_empty());
    assert_eq!(reported_bytes(&document), on_disk(&[&database]));
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "app-storage/partial",
    );
}

#[test]
fn bounded_bundle_details_preserve_all_measured_bytes_and_overlapping_roots_do_not_duplicate() {
    let sandbox = Sandbox::new("app-storage-bounded");
    let roots = sandbox.path().join("Movies");
    let files = (0..121)
        .map(|index| roots.join(format!("edit-{index:03}.screenstudio/raw")))
        .collect::<Vec<_>>();
    for file in &files {
        write(file, "recording source");
    }
    let canonical = roots.canonicalize().unwrap();
    let nested = canonical.join("edit-000.screenstudio");
    let output = run(
        &sandbox,
        &[
            "app-storage",
            "--root",
            canonical.to_str().unwrap(),
            "--root",
            nested.to_str().unwrap(),
            "--json",
        ],
    );
    let document = json(&output);
    assert!(output.status.success(), "{document}");
    assert_eq!(
        reported_bytes(&document),
        on_disk(&files.iter().map(|file| file.as_path()).collect::<Vec<_>>())
    );
    let findings = document["findings"].as_array().unwrap();
    assert!(
        findings.len() <= 106,
        "detail limit was not honored: {}",
        findings.len()
    );
    assert!(findings.iter().any(|finding| finding["path"].is_null() && finding["size_bytes"].as_u64().unwrap() > 0));
}

#[test]
fn configured_project_roots_do_not_start_an_application_data_search() {
    let sandbox = Sandbox::new("app-storage-config");
    let root = sandbox.path().join("Movies");
    write(&root.join("edit.screenstudio/raw"), "private recording");
    write(
        &sandbox.path().join(".config/devtrim.toml"),
        &format!("roots = [{:?}]\n", root.to_string_lossy()),
    );
    let output = run(&sandbox, &["app-storage", "--json"]);
    let document = json(&output);
    assert!(output.status.success(), "{document}");
    assert_eq!(reported_bytes(&document), 0);
}

#[test]
fn hostile_bundle_names_remain_inert_in_terminal_output() {
    let sandbox = Sandbox::new("app-storage-hostile");
    let root = sandbox.path().join("Movies");
    write(
        &root.join("clip\x1b[31m\n.screenstudio/raw"),
        "recording source",
    );
    let canonical = root.canonicalize().unwrap();
    let output = run(
        &sandbox,
        &["app-storage", "--root", canonical.to_str().unwrap()],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rendered = String::from_utf8(output.stdout).unwrap();
    assert!(
        !rendered.contains('\x1b'),
        "a filename emitted terminal controls"
    );
    assert!(rendered.contains("not deletion authority"));
    assert!(
        rendered.contains(r"clip\u{1b}[31m\n.screenstudio"),
        "the hostile bundle name must remain visible as inert text: {rendered}"
    );
}

#[test]
fn app_storage_preserves_an_oversized_apply_journal_and_its_rotations() {
    let sandbox = Sandbox::new("app-storage-journal");
    let media = sandbox
        .path()
        .join("Library/Group Containers/group.net.whatsapp.WhatsApp.shared/Message/Media/photo");
    write(&media, "ordinary media remains visible");
    write(&sandbox.journal(), "audit evidence");
    std::fs::OpenOptions::new()
        .write(true)
        .open(sandbox.journal())
        .unwrap()
        .set_len(10 * 1024 * 1024 + 1)
        .unwrap();
    let journal_parent = sandbox.journal().parent().unwrap().to_path_buf();
    for generation in 1..=5 {
        write(
            &journal_parent.join(format!("journal.jsonl.{generation}")),
            &format!("retained generation {generation}"),
        );
    }
    let before = Tree::snapshot(sandbox.path());
    for args in [vec!["app-storage"], vec!["app-storage", "--json"]] {
        let output = run(&sandbox, &args);
        assert!(output.status.success(), "{output:?}");
        if args.contains(&"--json") {
            assert_eq!(reported_bytes(&json(&output)), on_disk(&[&media]));
        } else {
            assert!(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .contains("WhatsApp media")
            );
        }
        before.assert_only(
            &Tree::snapshot(sandbox.path()),
            &Allowed::default(),
            "app-storage must preserve the journal and every rotation",
        );
    }
}

#[test]
fn an_explicit_root_inside_an_excluded_tree_cannot_bypass_discovery_pruning() {
    let sandbox = Sandbox::new("app-storage-excluded-root");
    let root = sandbox.path().join("node_modules/package");
    write(&root.join("private.screenstudio/raw"), "packaged content");
    let canonical = root.canonicalize().unwrap();
    let output = run(
        &sandbox,
        &[
            "app-storage",
            "--root",
            canonical.to_str().unwrap(),
            "--json",
        ],
    );
    let document = json(&output);
    assert_eq!(output.status.code(), Some(1), "{document}");
    assert_eq!(reported_bytes(&document), 0);
    assert!(!document["errors"].as_array().unwrap().is_empty());
}

#[test]
fn database_sidecars_and_each_known_container_are_measured_without_parsing() {
    let sandbox = Sandbox::new("app-storage-containers");
    let groups = sandbox.path().join("Library/Group Containers");
    let mut files = Vec::new();
    for name in [
        "group.net.whatsapp.WhatsApp.shared",
        "group.net.whatsapp.WhatsApp.private",
        "group.net.whatsapp.WhatsAppSMB.shared",
        "group.net.whatsapp.family",
    ] {
        for suffix in ["", "-wal", "-shm"] {
            let path = groups
                .join(name)
                .join(format!("ChatStorage.sqlite{suffix}"));
            write(&path, "intentionally invalid SQLite contents");
            files.push(path);
        }
    }
    write(
        &groups.join("unrecognized-whatsapp-layout/ChatStorage.sqlite"),
        "outside known scope",
    );
    let before = Tree::snapshot(sandbox.path());
    let output = run(&sandbox, &["app-storage", "--json"]);
    let document = json(&output);
    assert!(output.status.success(), "{document}");
    assert_eq!(
        reported_bytes(&document),
        on_disk(&files.iter().map(|file| file.as_path()).collect::<Vec<_>>())
    );
    assert_eq!(
        document["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|finding| finding["label"] == "WhatsApp database")
            .count(),
        12
    );
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "app-storage/container-scope",
    );
}

#[test]
fn excessive_errors_are_counted_without_growing_the_report_unboundedly() {
    let sandbox = Sandbox::new("app-storage-error-limit");
    let roots = (0..70)
        .map(|index| format!("/missing-devtrim-app-storage-{index}"))
        .collect::<Vec<_>>();
    let mut args = vec!["app-storage", "--json"];
    for root in &roots {
        args.extend(["--root", root]);
    }
    let output = run(&sandbox, &args);
    let document = json(&output);
    assert_eq!(output.status.code(), Some(1), "{document}");
    let errors = document["errors"].as_array().unwrap();
    assert!(errors.len() <= 33, "unbounded retained error detail");
    assert!(
        errors
            .last()
            .unwrap()
            .as_str()
            .unwrap()
            .contains("additional errors omitted")
    );
}

#[test]
fn filesystem_aliases_of_a_search_root_do_not_count_the_same_blocks_twice() {
    let sandbox = Sandbox::new("app-storage-root-alias");
    let root = sandbox.path().join("Movies");
    let recording = root.join("edit.screenstudio/raw");
    write(&recording, "one project's blocks");
    let canonical = root.canonicalize().unwrap();
    let aliases = [
        canonical.parent().unwrap().join("mOvIeS"),
        Path::new("/System/Volumes/Data").join(canonical.strip_prefix("/").unwrap()),
    ];
    for alias in aliases {
        if !alias.exists() {
            // These spellings are aliases only on the corresponding macOS volume.
            continue;
        }
        let output = run(
            &sandbox,
            &[
                "app-storage",
                "--root",
                canonical.to_str().unwrap(),
                "--root",
                alias.to_str().unwrap(),
                "--json",
            ],
        );
        let document = json(&output);
        assert!(output.status.success(), "{document}");
        assert_eq!(
            reported_bytes(&document),
            on_disk(&[&recording]),
            "an APFS alias duplicated a project's allocation: {document}"
        );
    }
}

#[test]
fn an_explicit_studio_search_does_not_recount_files_already_in_whatsapp_storage() {
    let sandbox = Sandbox::new("app-storage-overlapping-apps");
    let media = sandbox
        .path()
        .join("Library/Group Containers/group.net.whatsapp.WhatsApp.shared/Message/Media");
    let recording = media.join("saved.screenstudio/raw");
    write(&recording, "one physical allocation across both views");
    let root = media.canonicalize().unwrap();
    let output = run(
        &sandbox,
        &["app-storage", "--root", root.to_str().unwrap(), "--json"],
    );
    let document = json(&output);
    assert!(output.status.success(), "{document}");
    assert_eq!(reported_bytes(&document), on_disk(&[&recording]));
    assert!(
        document["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["label"] == "Screen Studio saved project")
    );
}
