//! Remaining observable refusal and continuation boundaries, through the real
//! binary in disposable homes. Native process listings are controlled inputs;
//! uv's filesystem lock is real and is held across the child invocation.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions fail by panicking"
)]

mod support;

use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::PathBuf;
use support::{Allowed, Sandbox, Tree, actionable_targets, assert_journaled, json, run, write};

fn cache(sandbox: &Sandbox, relative: &str) -> PathBuf {
    let path = sandbox.path().join(relative);
    write(&path.join("payload"), "regenerable");
    path.canonicalize().unwrap()
}

fn apply(sandbox: &Sandbox, category: &str) -> std::process::Output {
    run(
        sandbox,
        &["clean", category, "--apply", "--shred", "--yolo", "--json"],
    )
}

#[cfg(target_os = "macos")]
fn retained_trash_record(path: &std::path::Path) -> String {
    use std::os::macos::fs::MetadataExt;
    let metadata = std::fs::symlink_metadata(path).unwrap();
    serde_json::json!({
        "ts": 1, "phase": "result", "op": "caches", "action": "trash",
        "target": "/former/cache", "size_bytes": 4, "status": "ok",
        "identity": {
            "dev": metadata.st_dev(), "ino": metadata.st_ino(),
            "birth_secs": metadata.st_birthtime(),
            "birth_nanos": metadata.st_birthtime_nsec()
        }
    })
    .to_string()
        + "\n"
}

#[cfg(target_os = "macos")]
fn newer_history_records(count: usize) -> String {
    (0..count)
        .map(|index| {
            serde_json::json!({
                "id": format!("{:x}", index + 1), "ts": index + 2,
                "phase": "result", "op": "caches", "action": "shred",
                "target": "/former/unrelated-cache", "size_bytes": 1, "status": "ok"
            })
            .to_string()
                + "\n"
        })
        .collect()
}

#[cfg(target_os = "macos")]
#[test]
fn eval_trash_owned_identity_beyond_display_limit_across_retained_generations() {
    let sandbox = Sandbox::in_target("eval-trash-retained");
    let owned = cache(&sandbox, ".Trash/renamed-cache");
    let foreign = cache(&sandbox, ".Trash/foreign-cache");
    write(
        &sandbox.journal().with_extension("jsonl.3"),
        &retained_trash_record(&owned),
    );
    for generation in 0..3 {
        let journal = if generation == 0 {
            sandbox.journal()
        } else {
            sandbox
                .journal()
                .with_extension(format!("jsonl.{generation}"))
        };
        write(&journal, &newer_history_records(400));
    }
    let pristine = Tree::snapshot(sandbox.path());
    let preview = run(&sandbox, &["trash-empty", "--only-devtrim", "--json"]);
    assert!(preview.status.success(), "{}", json(&preview));
    assert_eq!(
        actionable_targets(&json(&preview)),
        [owned],
        "PV eval/trash-retained-owned: every retained generation must identify owned Trash beyond 1000 newer results"
    );
    assert!(foreign.exists());
    for (arguments, expected) in [
        (vec!["history", "--json"], 20),
        (vec!["history", "--limit", "1000", "--json"], 1000),
    ] {
        let output = run(&sandbox, &arguments);
        assert!(output.status.success(), "{}", json(&output));
        assert_eq!(json(&output)["entries"].as_array().unwrap().len(), expected);
    }
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "PV eval/trash-retained-preview",
    );
}

#[cfg(target_os = "macos")]
#[test]
fn eval_trash_malformed_retained_record_beyond_display_limit_refuses_everything() {
    let sandbox = Sandbox::in_target("eval-trash-retained-malformed");
    let owned = cache(&sandbox, ".Trash/recent-owned-cache");
    write(&sandbox.journal(), &retained_trash_record(&owned));
    write(
        &sandbox.journal().with_extension("jsonl.1"),
        &newer_history_records(1100),
    );
    let oldest = sandbox.journal().with_extension("jsonl.3");
    write(&oldest, "malformed retained ownership record\n");
    let pristine = Tree::snapshot(sandbox.path());
    let preview = run(&sandbox, &["trash-empty", "--only-devtrim", "--json"]);
    assert!(
        !preview.status.success(),
        "PV eval/trash-retained-malformed: malformed older retained history must refuse the whole ownership selection"
    );
    assert!(actionable_targets(&json(&preview)).is_empty());
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "PV eval/trash-retained-malformed-preview",
    );
    // The ordinary display intentionally reads only its newest tail.
    let display = run(&sandbox, &["history", "--limit", "1", "--json"]);
    assert!(display.status.success(), "{}", json(&display));
    write(&oldest, &newer_history_records(1));
    let control = run(&sandbox, &["trash-empty", "--only-devtrim", "--json"]);
    assert!(control.status.success(), "{}", json(&control));
    assert_eq!(actionable_targets(&json(&control)), [owned]);
}

#[cfg(target_os = "macos")]
#[test]
fn eval_trash_retained_snapshot_over_resource_budget_refuses_everything() {
    let sandbox = Sandbox::in_target("eval-trash-retained-budget");
    let owned = cache(&sandbox, ".Trash/recent-owned-cache");
    write(&sandbox.journal(), &retained_trash_record(&owned));
    write(
        &sandbox.journal().with_extension("jsonl.1"),
        &newer_history_records(1100),
    );
    let oldest = sandbox.journal().with_extension("jsonl.3");
    write(&oldest, "");
    // Sparse: prove resource refusal without allocating a large test corpus.
    std::fs::OpenOptions::new()
        .write(true)
        .open(&oldest)
        .unwrap()
        .set_len(50 * 1024 * 1024)
        .unwrap();
    let output = run(&sandbox, &["trash-empty", "--only-devtrim", "--json"]);
    assert!(
        !output.status.success(),
        "PV eval/trash-retained-budget: an oversized retained snapshot must refuse all ownership selection"
    );
    assert!(
        json(&output)["errors"].to_string().contains("bounded"),
        "PV eval/trash-retained-budget: the whole snapshot must be refused by its resource bound before line parsing"
    );
    assert!(actionable_targets(&json(&output)).is_empty());
    write(&oldest, &newer_history_records(1));
    let control = run(&sandbox, &["trash-empty", "--only-devtrim", "--json"]);
    assert!(control.status.success(), "{}", json(&control));
    assert_eq!(actionable_targets(&json(&control)), [owned]);
}

#[test]
fn eval_busy_uv_cache_is_kept_while_later_caches_are_removed() {
    let sandbox = Sandbox::in_target("eval-uv-lock");
    let uv = cache(&sandbox, ".cache/uv");
    let node = cache(&sandbox, ".cache/node/corepack");
    let lock = std::fs::File::create(uv.join(".lock")).unwrap();
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::LockShared).unwrap();
    let pristine = Tree::snapshot(sandbox.path());
    let preview = run(&sandbox, &["clean", "caches", "--shred", "--json"]);
    assert!(preview.status.success(), "{}", json(&preview));
    assert_eq!(
        actionable_targets(&json(&preview)),
        [node.clone(), uv.clone()]
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "uv preview",
    );

    let result = apply(&sandbox, "caches");
    assert!(
        uv.join("payload").exists(),
        "PV eval/uv-busy: a held uv cache lock was ignored"
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: vec![node.clone()],
            scratch: vec![sandbox.journal().parent().unwrap().to_path_buf()],
        },
        "PV eval/caches-continue",
    );
    let document = json(&result);
    assert!(!result.status.success(), "{document}");
    assert_eq!(
        document["errors"].as_array().unwrap().len(),
        1,
        "{document}"
    );
    assert_journaled(&sandbox, "caches", "shred", &[node]);

    // The exact cache that was refused becomes removable after releasing the
    // real lock; this also proves the fixture reached uv's locking boundary.
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::Unlock).unwrap();
    drop(lock);
    let before = Tree::snapshot(sandbox.path());
    let result = apply(&sandbox, "caches");
    assert!(result.status.success(), "{}", json(&result));
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: vec![uv],
            scratch: vec![sandbox.journal().parent().unwrap().to_path_buf()],
        },
        "uv unlocked control",
    );
}

/// A `pgrep` that reports a Gradle daemon to the Gradle probe and nothing to
/// the build-process probe.
const GRADLE_RUNNING: &str = "case \"$*\" in *GradleDaemon*) printf '4242\\n' ;; *) exit 1 ;; esac";

/// Gradle locks its caches only among its own processes, so they are kept
/// while any Gradle process runs: left out of a preview taken then, and refused
/// at apply when Gradle starts after the preview. The other caches still go.
#[test]
fn eval_gradle_caches_are_kept_while_gradle_runs() {
    let sandbox = Sandbox::in_target("eval-gradle-running");
    let gradle = cache(&sandbox, ".gradle/caches");
    let other = cache(&sandbox, ".cache/gh");
    sandbox.script("pgrep", GRADLE_RUNNING);
    let preview = run(&sandbox, &["clean", "caches", "--shred", "--json"]);
    assert_eq!(
        actionable_targets(&json(&preview)),
        std::slice::from_ref(&other),
        "PV eval/gradle-running-scan: Gradle's caches were offered while Gradle runs"
    );
    assert!(preview.status.success(), "{}", json(&preview));

    sandbox.script("pgrep", "exit 1");
    let pristine = Tree::snapshot(sandbox.path());
    let mut session =
        support::Interactive::start(&sandbox, &["clean", "caches", "--apply", "--shred"]);
    session.wait_for("Type the number to continue:");
    // Gradle starts after the preview, before consent.
    sandbox.script("pgrep", GRADLE_RUNNING);
    session.send("0");
    let (status, output) = session.finish();
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: vec![other],
            scratch: vec![
                sandbox.bin(),
                sandbox.journal().parent().unwrap().to_path_buf(),
            ],
        },
        "PV eval/gradle-running-apply",
    );
    assert!(gradle.join("payload").exists());
    assert!(
        !status.success(),
        "the refusal must fail the run:\n{output}"
    );
    assert!(output.contains("while Gradle is running"), "{output}");
}

/// Makes one folder read-only, as Go makes its module cache, and on drop
/// makes every folder under `root` writable again, wherever a failed removal
/// left it, so the sandbox can still be removed.
struct ReadOnly {
    root: PathBuf,
}

impl ReadOnly {
    fn new(folder: &std::path::Path, root: PathBuf) -> Self {
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o555)).unwrap();
        Self { root }
    }
}

impl Drop for ReadOnly {
    fn drop(&mut self) {
        fn writable(folder: &std::path::Path) {
            std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o755)).ok();
            for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    writable(&entry.path());
                }
            }
        }
        writable(&self.root);
    }
}

/// Go keeps its module cache read-only. A permanent removal would stop inside
/// the first read-only folder and leave a half-removed cache, so it is refused
/// before anything changes; the caches beside it are still removed.
#[test]
fn eval_a_read_only_go_module_cache_is_refused_whole_for_permanent_removal() {
    let sandbox = Sandbox::in_target("eval-go-read-only");
    sandbox.script("go", "printf '%s\\n' \"$HOME/go/pkg/mod\"");
    let modules = sandbox.path().join("go/pkg/mod");
    let module = modules.join("example.com/m@v1.0.0");
    write(&module.join("go.mod"), "module example.com/m\n");
    write(
        &modules.join("cache/download/example.com/m/@v/v1.0.0.zip"),
        "zip",
    );
    let modules = modules.canonicalize().unwrap();
    let other = cache(&sandbox, ".cache/gh");
    let _read_only = ReadOnly::new(&module, sandbox.path().join("go"));
    let preview = run(&sandbox, &["clean", "caches", "--shred", "--json"]);
    assert_eq!(
        actionable_targets(&json(&preview)),
        [other.clone(), modules.clone()]
    );
    let pristine = Tree::snapshot(sandbox.path());

    let result = apply(&sandbox, "caches");
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: vec![other],
            scratch: vec![sandbox.journal().parent().unwrap().to_path_buf()],
        },
        "PV eval/go-read-only",
    );
    let document = json(&result);
    assert!(!result.status.success(), "{document}");
    assert!(document.to_string().contains("not writable"), "{document}");
}

/// `go env GOMODCACHE` is trusted only for a folder inside the home holding
/// Go's own download cache, so a `go` that answers with a project folder offers
/// nothing from it.
#[test]
fn eval_a_folder_without_go_layout_is_never_a_module_cache() {
    let sandbox = Sandbox::in_target("eval-go-namespace");
    sandbox.script("go", "printf '%s\\n' \"$HOME/dev/project\"");
    write(
        &sandbox.path().join("dev/project/main.go"),
        "package main\n",
    );
    let other = cache(&sandbox, ".cache/gh");
    let preview = run(&sandbox, &["clean", "caches", "--shred", "--json"]);
    assert_eq!(
        actionable_targets(&json(&preview)),
        [other],
        "PV eval/go-namespace: a folder without Go's layout was offered"
    );
    assert!(preview.status.success(), "{}", json(&preview));
}

#[test]
fn eval_agents_continues_after_a_cache_holding_a_repository_is_refused() {
    let sandbox = Sandbox::in_target("eval-agents-continue");
    let refused = cache(&sandbox, ".claude/cache");
    write(&refused.join("nested/.git/HEAD"), "ref: refs/heads/main\n");
    let eligible = cache(&sandbox, ".codex/cache");
    let pristine = Tree::snapshot(sandbox.path());
    let preview = run(&sandbox, &["clean", "agents", "--shred", "--json"]);
    assert!(preview.status.success(), "{}", json(&preview));
    assert_eq!(
        actionable_targets(&json(&preview)),
        [refused.clone(), eligible.clone()]
    );
    let result = apply(&sandbox, "agents");
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: vec![eligible],
            scratch: vec![sandbox.journal().parent().unwrap().to_path_buf()],
        },
        "PV eval/agents-continue",
    );
    let document = json(&result);
    assert!(!result.status.success(), "{document}");
    assert_eq!(
        document["errors"].as_array().unwrap().len(),
        1,
        "{document}"
    );
    let records = support::journal_lines(&sandbox);
    assert_eq!(
        records.len(),
        4,
        "each attempt must have its result: {records:?}"
    );
    assert_eq!(records[0].target.as_deref(), refused.to_str());
    assert_eq!(records[0].phase, "attempt");
    assert_eq!(records[1].id, records[0].id);
    assert_eq!(records[1].status.as_deref(), Some("error"));
    assert_eq!(records[2].phase, "attempt");
    assert_eq!(records[3].id, records[2].id);
    assert_eq!(records[3].status.as_deref(), Some("ok"));
}

#[test]
fn eval_a_symlinked_journal_refuses_apply_without_touching_its_destination() {
    for link_parent in [false, true] {
        let sandbox = Sandbox::in_target("eval-journal-link");
        let target = cache(&sandbox, ".cache/node/corepack");
        let outside = sandbox.path().join("outside");
        write(&outside.join("journal.jsonl"), "keep\n");
        let journal = sandbox.journal();
        if link_parent {
            std::fs::create_dir_all(journal.parent().unwrap().parent().unwrap()).unwrap();
            symlink(&outside, journal.parent().unwrap()).unwrap();
        } else {
            std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
            std::fs::set_permissions(
                journal.parent().unwrap(),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            symlink(outside.join("journal.jsonl"), &journal).unwrap();
        }
        let pristine = Tree::snapshot(sandbox.path());
        let preview = run(&sandbox, &["clean", "caches", "--shred", "--json"]);
        assert!(preview.status.success(), "{}", json(&preview));
        assert_eq!(actionable_targets(&json(&preview)), [target]);
        let result = apply(&sandbox, "caches");
        pristine.assert_only(
            &Tree::snapshot(sandbox.path()),
            &Allowed {
                scratch: vec![journal.with_file_name("journal.lock")],
                ..Allowed::default()
            },
            "PV eval/journal-symlink",
        );
        assert!(!result.status.success(), "{}", json(&result));
        assert!(!json(&result)["errors"].as_array().unwrap().is_empty());
    }
    let sandbox = Sandbox::in_target("eval-journal-link-control");
    let target = cache(&sandbox, ".cache/node/corepack");
    let result = apply(&sandbox, "caches");
    assert!(result.status.success(), "{}", json(&result));
    assert!(!target.exists());
    assert_journaled(&sandbox, "caches", "shred", &[target]);
}

#[test]
fn eval_noninteractive_apply_requires_explicit_consent() {
    let sandbox = Sandbox::in_target("eval-unattended");
    sandbox.script("docker", r#"case "$*" in
      'context inspect') printf '%s\n' '[{"Endpoints":{"docker":{"Host":"unix:///var/run/docker.sock"}}}]' ;;
      '--host unix:///var/run/docker.sock version') printf 'Docker 28\n' ;;
      '--host unix:///var/run/docker.sock system df'*)
        printf 'Images\t2GB\t1GB\t0\nBuild Cache\t3GB\t1GB\t0\n' ;;
      '--host unix:///var/run/docker.sock image prune -a -f' | '--host unix:///var/run/docker.sock builder prune -a -f')
        printf '%s\n' "$*" >> "$HOME/pruned.log" ;;
      *) exit 1 ;;
    esac"#);
    let pristine = Tree::snapshot(sandbox.path());
    let preview = run(&sandbox, &["clean", "docker", "--json"]);
    assert!(preview.status.success(), "{}", json(&preview));
    assert_eq!(
        json(&preview)["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|finding| finding["action"]["type"] == "command")
            .count(),
        2
    );
    let result = run(&sandbox, &["clean", "docker", "--apply", "--json"]);
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "PV eval/noninteractive-consent",
    );
    assert!(!result.status.success(), "{}", json(&result));
    let result = run(&sandbox, &["clean", "docker", "--apply", "-y", "--json"]);
    assert!(result.status.success(), "{}", json(&result));
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            scratch: vec![
                sandbox.journal().parent().unwrap().to_path_buf(),
                sandbox.path().join("pruned.log"),
            ],
            ..Allowed::default()
        },
        "explicit consent control",
    );
    assert_eq!(
        std::fs::read_to_string(sandbox.path().join("pruned.log")).unwrap(),
        "--host unix:///var/run/docker.sock image prune -a -f\n--host unix:///var/run/docker.sock builder prune -a -f\n"
    );
    let records = support::journal_lines(&sandbox);
    assert_eq!(records.len(), 4);
    for pair in records.as_chunks::<2>().0 {
        assert_eq!(pair[0].phase, "attempt");
        assert_eq!(pair[1].id, pair[0].id);
        assert_eq!(pair[1].status.as_deref(), Some("ok"));
    }
}

#[test]
fn eval_build_process_liveness_keeps_only_the_busy_repository() {
    for category in ["node-modules", "artifacts"] {
        let sandbox = Sandbox::in_target("eval-build-liveness");
        sandbox.script("git", &support::git_activity("2020-01-01", "2020-01-01"));
        let dev = sandbox.path().join("dev");
        for name in ["busy", "idle"] {
            let project = dev.join(name);
            write(&project.join(".git/HEAD"), "ref: refs/heads/main\n");
            write(&project.join("package.json"), "{}");
            write(
                &project.join("Cargo.toml"),
                "[package]\nname = \"fixture\"\n",
            );
            write(&project.join("node_modules/pkg/file"), "dependency");
            write(&project.join("target/debug/file"), "build output");
        }
        let dev = dev.canonicalize().unwrap();
        sandbox.script("pgrep", "printf '42\\n'");
        sandbox.script(
            "lsof",
            &format!("printf 'p42\\nn{}\\n'", dev.join("busy").display()),
        );
        let leaf = if category == "node-modules" {
            "node_modules"
        } else {
            "target"
        };
        let eligible = dev.join("idle").join(leaf);
        let pristine = Tree::snapshot(sandbox.path());
        let root = dev.to_str().unwrap();
        let preview = run(
            &sandbox,
            &["clean", category, "--root", root, "--shred", "--json"],
        );
        assert!(preview.status.success(), "{}", json(&preview));
        assert_eq!(
            actionable_targets(&json(&preview)).as_slice(),
            std::slice::from_ref(&eligible),
            "PV eval/build-busy: {category} offered active build output"
        );
        pristine.assert_only(
            &Tree::snapshot(sandbox.path()),
            &Allowed::default(),
            "build liveness preview",
        );
        let result = run(
            &sandbox,
            &[
                "clean", category, "--root", root, "--apply", "--shred", "--yolo", "--json",
            ],
        );
        assert!(result.status.success(), "{}", json(&result));
        pristine.assert_only(
            &Tree::snapshot(sandbox.path()),
            &Allowed {
                removed: vec![eligible.clone()],
                scratch: vec![sandbox.journal().parent().unwrap().to_path_buf()],
            },
            "build liveness apply",
        );
        assert_journaled(&sandbox, category, "shred", &[eligible]);
    }
}
