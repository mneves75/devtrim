//! Reviewer evals written from stated requirements: an empty Trash still
//! guarded, categories continuing past a refusal, and a `.git` file read as
//! Git reads it. Every command runs through the real binary against a planted
//! fixture in a disposable HOME.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions fail by panicking"
)]

mod support;

use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::time::{Duration, SystemTime};
use support::{Allowed, Interactive, ModeGuard, Sandbox, Tree, git, json, real_git, run, write};

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn error_text(document: &Value) -> String {
    document["errors"]
        .as_array()
        .map(|errors| {
            errors
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

fn age(path: &Path, days: u64) {
    let when = SystemTime::now() - Duration::from_secs(days * 60 * 60 * 24);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

/// A journal devtrim can read as history but cannot append to, so only the
/// write check before an apply can refuse it.
fn break_journal(sandbox: &Sandbox) {
    write(&sandbox.journal(), "");
    std::fs::set_permissions(
        sandbox.journal().parent().unwrap(),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::set_permissions(sandbox.journal(), std::fs::Permissions::from_mode(0o400)).unwrap();
}

/// Run an interactive apply, change the fixture at the typed confirmation,
/// then give it.
fn drift_session(
    sandbox: &Sandbox,
    args: &[&str],
    drift: impl FnOnce(),
) -> (std::process::ExitStatus, String) {
    let mut session = Interactive::start(sandbox, args);
    session.wait_for("Type the number to continue:");
    drift();
    session.send("0");
    session.finish()
}

// ---------- trash-empty with nothing to purge ----------

/// Human output: an empty Trash with an unwritable journal is refused, says
/// why, never claims success, and changes nothing. The writable journal beside
/// it is the control that the same command otherwise succeeds.
#[test]
fn review_trash_empty_nothing_to_purge_human_refuses_an_unwritable_journal() {
    let control = Sandbox::in_target("review-trash-human-control");
    std::fs::create_dir_all(control.path().join(".Trash")).unwrap();
    let ok = run(&control, &["trash-empty", "--apply"]);
    assert!(ok.status.success(), "control: {}", text(&ok));
    assert!(text(&ok).contains("nothing to purge"), "{}", text(&ok));

    let sandbox = Sandbox::in_target("review-trash-human-journal");
    std::fs::create_dir_all(sandbox.path().join(".Trash")).unwrap();
    break_journal(&sandbox);
    let pristine = Tree::snapshot(sandbox.path());
    let refused = run(&sandbox, &["trash-empty", "--apply"]);
    let output = text(&refused);
    assert!(
        !refused.status.success(),
        "review/trash-human-journal: an empty purge with an unwritable journal exited 0: {output}"
    );
    assert!(
        !output.contains("nothing to purge"),
        "review/trash-human-journal: a refused apply claimed success: {output}"
    );
    assert!(
        output.contains("journal"),
        "the error names the journal: {output}"
    );
    // devtrim's own lock may appear beside the broken journal; nothing else.
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            scratch: vec![sandbox.path().join(".local/state/devtrim")],
            ..Allowed::default()
        },
        "review/trash-human-journal",
    );
}

/// `--only-devtrim` takes the same empty-plan path and must keep the same
/// journal check. The journal is readable, so the history it selects from is
/// read in full and only the write check can refuse; the same fixture with a
/// writable journal is the control.
#[test]
fn review_trash_empty_only_devtrim_nothing_to_purge_refuses_an_unwritable_journal() {
    let args = ["trash-empty", "--only-devtrim", "--apply", "--json"];
    let fixture = |name| {
        let sandbox = Sandbox::in_target(name);
        write(
            &sandbox.path().join(".Trash/not-ours.txt"),
            "not moved by devtrim",
        );
        write(&sandbox.journal(), "");
        sandbox
    };
    let control = fixture("review-trash-only-control");
    let ok = run(&control, &args);
    let document = json(&ok);
    assert!(ok.status.success(), "control: {document}");
    assert!(control.path().join(".Trash/not-ours.txt").exists());

    let sandbox = fixture("review-trash-only-journal");
    break_journal(&sandbox);
    let refused = run(&sandbox, &args);
    let document = json(&refused);
    assert!(!refused.status.success(), "{document}");
    assert_ne!(document["applied"], true, "{document}");
    assert!(
        error_text(&document).contains("journal"),
        "review/trash-only-journal: refused for another reason: {document}"
    );
    assert!(sandbox.path().join(".Trash/not-ours.txt").exists());
}

/// A regular file where the Trash folder should be is an error in preview and
/// apply, JSON and human alike; it is never "nothing to purge".
#[test]
fn review_trash_empty_refuses_a_non_directory_trash_root() {
    let sandbox = Sandbox::in_target("review-trash-file-root");
    let trash = sandbox.path().join(".Trash");
    write(&trash, "a file, not a folder");
    let pristine = Tree::snapshot(sandbox.path());
    for args in [
        &["trash-empty", "--json"][..],
        &["trash-empty", "--apply", "--json"][..],
        &["trash-empty", "--only-devtrim", "--apply", "--json"][..],
    ] {
        let output = run(&sandbox, args);
        let document = json(&output);
        assert!(
            !output.status.success(),
            "review/trash-file-root {args:?}: exited 0: {document}"
        );
        assert_ne!(document["applied"], true, "{args:?}: {document}");
        assert!(
            error_text(&document).contains(trash.to_str().unwrap()),
            "review/trash-file-root {args:?}: the error must name the Trash: {document}"
        );
    }
    for args in [&["trash-empty"][..], &["trash-empty", "--apply"][..]] {
        let output = run(&sandbox, args);
        let shown = text(&output);
        assert!(
            !output.status.success() && !shown.contains("nothing to purge"),
            "review/trash-file-root {args:?}: {shown}"
        );
        assert!(shown.contains(".Trash"), "{args:?}: {shown}");
    }
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            scratch: vec![sandbox.path().join(".local/state/devtrim")],
            ..Allowed::default()
        },
        "review/trash-file-root",
    );
}

/// A Trash holding only an item that cannot be measured offers nothing, but
/// the run failed: human output must exit nonzero and name the item.
#[test]
fn review_trash_empty_unmeasurable_item_human_preview_fails() {
    let sandbox = Sandbox::in_target("review-trash-unmeasurable");
    let locked = sandbox.path().join(".Trash/locked");
    write(&locked.join("inner/secret"), "secret");
    let sealed = ModeGuard::unreadable(locked.join("inner"));
    let preview = run(&sandbox, &["trash-empty"]);
    let apply = run(&sandbox, &["trash-empty", "--apply"]);
    drop(sealed);
    for (name, output) in [("preview", preview), ("apply", apply)] {
        let shown = text(&output);
        assert!(!output.status.success(), "{name}: {shown}");
        assert!(shown.contains("locked"), "{name}: {shown}");
        assert!(!shown.contains("--confirm"), "{name}: {shown}");
        // A failure never reads as an empty Trash.
        assert!(!shown.contains("nothing to purge"), "{name}: {shown}");
        assert!(shown.contains("could not be measured"), "{name}: {shown}");
    }
    assert!(locked.join("inner/secret").exists());
}

// ---------- continuing past a refusal ----------

/// The refused archive is the first in the plan, so "continues" is not
/// vacuous; the journal holds exactly the later removal, completed.
#[test]
fn review_installers_refused_first_archive_does_not_stop_the_second() {
    let sandbox = Sandbox::in_target("review-installers-order");
    let first = sandbox.path().join("Downloads/A-first.dmg");
    let second = sandbox.path().join("Downloads/B-second.pkg");
    for path in [&first, &second] {
        write(path, &"x".repeat(4096));
        age(path, 400);
    }
    let preview = run(&sandbox, &["clean", "installers", "--shred", "--json"]);
    let document = json(&preview);
    let order: Vec<String> = document["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|finding| finding["path"].as_str().map(str::to_owned))
        .collect();
    let refused_index = order
        .iter()
        .position(|path| path == first.to_str().unwrap());
    let moved_index = order
        .iter()
        .position(|path| path == second.to_str().unwrap());
    assert!(
        matches!((refused_index, moved_index), (Some(a), Some(b)) if a < b),
        "the refused archive must come first in the plan: {document}"
    );

    let (status, output) = drift_session(
        &sandbox,
        &["clean", "installers", "--apply", "--shred"],
        || age(&first, 0),
    );
    assert!(first.exists(), "the touched archive was removed:\n{output}");
    assert!(
        !second.exists(),
        "review/installers-order: the archive after the refusal was kept:\n{output}"
    );
    assert!(
        !status.success(),
        "the refusal must fail the run:\n{output}"
    );
    support::assert_journaled(&sandbox, "installers", "shred", &[second]);
}

/// Two unreferenced toolchains; the first in the plan gains a Git repository
/// after the preview, so the sink refuses it. The second is still removed and
/// the run exits nonzero.
#[test]
fn review_toolchains_continue_past_a_refused_toolchain() {
    let sandbox = Sandbox::in_target("review-toolchains-continue");
    let directory = sandbox.path().join("Library/Developer/Toolchains");
    for name in [
        "swift-5.7.3-RELEASE.xctoolchain",
        "swift-5.8-RELEASE.xctoolchain",
    ] {
        write(&directory.join(name).join("usr/bin/swift"), "toolchain");
    }
    // The toolchain `swift-latest` names is kept; the category needs that link.
    write(
        &directory.join("swift-6.2-RELEASE.xctoolchain/usr/bin/swift"),
        "the latest toolchain",
    );
    std::os::unix::fs::symlink(
        "swift-6.2-RELEASE.xctoolchain",
        directory.join("swift-latest.xctoolchain"),
    )
    .unwrap();
    let preview = run(&sandbox, &["clean", "toolchains", "--shred", "--json"]);
    let document = json(&preview);
    let plan: Vec<PathBuf> = document["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["action"]["type"] == "shred")
        .filter_map(|finding| finding["path"].as_str().map(PathBuf::from))
        .collect();
    assert_eq!(plan.len(), 2, "{document}");
    let (refused, removed) = (plan[0].clone(), plan[1].clone());

    let (status, output) = drift_session(
        &sandbox,
        &["clean", "toolchains", "--apply", "--shred"],
        || write(&refused.join(".git/HEAD"), "ref: refs/heads/main\n"),
    );
    assert!(
        refused.join(".git/HEAD").exists(),
        "the toolchain holding a repository was removed:\n{output}"
    );
    assert!(
        !removed.exists(),
        "review/toolchains-continue: the toolchain after the refusal was kept:\n{output}"
    );
    assert!(
        !status.success(),
        "the refusal must fail the run:\n{output}"
    );
}

const GONE_ONE: &str = "11111111-1111-1111-1111-111111111111";
const GONE_TWO: &str = "22222222-2222-2222-2222-222222222222";

fn simctl_list(devices: &[(&str, bool)]) -> String {
    let entries: Vec<Value> = devices
        .iter()
        .map(|(udid, available)| {
            serde_json::json!({
                "dataPath": "/tmp/device",
                "dataPathSize": if *available { 32768 } else { 0 },
                "logPath": "/tmp/log",
                "udid": udid,
                "isAvailable": available,
                "deviceTypeIdentifier": "com.apple.CoreSimulator.SimDeviceType.iPhone-16",
                "state": "Shutdown",
                "name": "iPhone 16",
                "lastUsedAt": "2026-09-01T10:00:00Z"
            })
        })
        .collect();
    serde_json::json!({
        "devices": { "com.apple.CoreSimulator.SimRuntime.iOS-17-0": entries }
    })
    .to_string()
}

/// The first unavailable device in the plan becomes available before its
/// recheck, so apply refuses it; the second is still deleted, and the run
/// exits nonzero.
#[test]
fn review_simulators_continue_past_a_refused_device() {
    let sandbox = Sandbox::in_target("review-simulators-continue");
    let devices = sandbox
        .path()
        .join("Library/Developer/CoreSimulator/Devices");
    for udid in [GONE_ONE, GONE_TWO] {
        write(&devices.join(udid).join("data/payload"), &"x".repeat(8192));
    }
    std::fs::create_dir_all(sandbox.path().join("stub")).unwrap();
    let first = simctl_list(&[(GONE_ONE, false), (GONE_TWO, false)]);
    let later = simctl_list(&[(GONE_ONE, true), (GONE_TWO, false)]);
    let body = r#"count="$HOME/stub/list-count"
case "$*" in
  '--version') printf 'xcrun version 70\n' ;;
  'simctl list devices --json')
    n=0
    if [ -f "$count" ]; then read n < "$count"; fi
    n=$((n + 1))
    printf '%s\n' "$n" > "$count"
    if [ "$n" -le 1 ]; then printf '%s\n' 'FIRST'; else printf '%s\n' 'LATER'; fi ;;
  'simctl delete '*) /bin/rm -rf "$HOME/Library/Developer/CoreSimulator/Devices/$3" ;;
  *) exit 1 ;;
esac"#
        .replace("FIRST", &first)
        .replace("LATER", &later);
    sandbox.script("xcrun", body.as_str());

    let apply = run(
        &sandbox,
        &["clean", "simulators", "--apply", "--yolo", "--json"],
    );
    let document = json(&apply);
    assert!(
        devices.join(GONE_ONE).exists(),
        "the device that became available was deleted: {document}"
    );
    assert!(
        !devices.join(GONE_TWO).exists(),
        "review/simulators-continue: the device after the refusal was kept: {document}"
    );
    assert!(!apply.status.success(), "{document}");
    assert_eq!(document["summary"]["items_touched"], 1, "{document}");
    assert!(error_text(&document).contains(GONE_ONE), "{document}");
}

// ---------- worktrees: partial read, human output ----------

const FIXTURE_DATE: &str = "2020-01-01T00:00:00Z";

/// One worktree read only in part keeps its row, the readable sibling is
/// still measured in full, and human output warns and exits 1.
#[test]
fn review_worktrees_partial_read_human_keeps_rows_and_exits_one() {
    let sandbox = Sandbox::in_target("review-worktrees-human");
    sandbox.script("git", &format!("exec '{}' \"$@\"", real_git().display()));
    let dev = sandbox.path().join("dev");
    let app = dev.join("app");
    write(&app.join("README"), "readme");
    git(&sandbox, &app, FIXTURE_DATE, &["init", "-q"]);
    git(&sandbox, &app, FIXTURE_DATE, &["add", "README"]);
    git(
        &sandbox,
        &app,
        FIXTURE_DATE,
        &["commit", "-q", "-m", "fixture"],
    );
    git(
        &sandbox,
        &app,
        FIXTURE_DATE,
        &["worktree", "add", "-q", "-b", "locked", "../app-locked"],
    );
    git(
        &sandbox,
        &app,
        FIXTURE_DATE,
        &["worktree", "add", "-q", "-b", "open", "../app-open"],
    );
    let partial = dev.join("app-locked");
    let readable = dev.join("app-open");
    write(&partial.join("sealed/secret.bin"), &"x".repeat(4096));
    write(&readable.join("payload.bin"), &"x".repeat(4096));
    let root = dev.to_str().unwrap();

    let sealed = ModeGuard::unreadable(partial.join("sealed"));
    let human = run(&sandbox, &["worktrees", "--root", root]);
    let document = run(&sandbox, &["worktrees", "--root", root, "--json"]);
    drop(sealed);

    let shown = text(&human);
    assert_eq!(
        human.status.code(),
        Some(1),
        "review/worktrees-human: a partial read must exit 1: {shown}"
    );
    assert!(
        shown.contains("app-locked"),
        "the partial row is listed: {shown}"
    );
    assert!(
        shown.contains("app-open"),
        "the readable row is listed: {shown}"
    );
    assert!(shown.contains("lower bound"), "{shown}");

    assert_eq!(document.status.code(), Some(1));
    let document = json(&document);
    let row = |path: &Path| {
        document["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|finding| finding["path"].as_str() == Some(path.to_str().unwrap()))
            .unwrap_or_else(|| panic!("no row for {}: {document}", path.display()))
            .clone()
    };
    let open = row(&readable);
    assert!(
        !open["note"].as_str().unwrap().contains("lower bound")
            && open["size_bytes"].as_u64().unwrap() >= 4096,
        "the readable sibling is measured whole: {open}"
    );
    let errors = error_text(&document);
    assert!(
        errors.contains(partial.to_str().unwrap()) && !errors.contains(readable.to_str().unwrap()),
        "only the partial worktree is an error: {errors}"
    );
}

// ---------- a `.git` file read as Git reads it ----------

/// A project with a `node_modules` install whose `.git` file holds `marker`.
/// `git` always fails, so a project that is not an orphan is a Git error
/// (nonzero), while an orphan is a skip note (exit 0): the two outcomes say
/// how devtrim parsed the file.
fn gitfile_case(name: &str, marker: impl FnOnce(&Path) -> Vec<u8>) -> Output {
    let sandbox = Sandbox::in_target(name);
    sandbox.script("git", "exit 128");
    let dev = sandbox.path().join("dev");
    let project = dev.join("wt");
    write(&project.join("package.json"), "{}");
    write(&project.join("node_modules/pkg/index.js"), "dependency");
    let bytes = marker(sandbox.path());
    std::fs::write(project.join(".git"), bytes).unwrap();
    run(
        &sandbox,
        &["clean", "node-modules", "--root", dev.to_str().unwrap()],
    )
}

fn assert_orphan(case: &str, output: &Output) {
    let shown = text(output);
    assert!(
        output.status.success() && shown.contains("repository is gone"),
        "{case}: expected an orphan skip note: {shown}"
    );
}

fn assert_not_orphan(case: &str, output: &Output) {
    let shown = text(output);
    assert!(
        !output.status.success() && !shown.contains("repository is gone"),
        "{case}: expected Git to be asked (and fail), not an orphan skip: {shown}"
    );
}

/// Trailing CR and LF are stripped: CRLF naming a gitdir that exists is not
/// an orphan. The same CRLF naming a missing gitdir is the control.
#[test]
fn review_gitfile_crlf_is_stripped() {
    let output = gitfile_case("review-gitfile-crlf", |home| {
        let gitdir = home.join("gitdirs/wt");
        std::fs::create_dir_all(&gitdir).unwrap();
        format!("gitdir: {}\r\n", gitdir.display()).into_bytes()
    });
    assert_not_orphan("review/gitfile-crlf", &output);

    let output = gitfile_case("review-gitfile-crlf-control", |home| {
        format!("gitdir: {}\r\n", home.join("gitdirs/missing").display()).into_bytes()
    });
    assert_orphan("review/gitfile-crlf control", &output);
}

/// Trailing spaces are part of the path, as Git reads them: a gitdir whose
/// real name ends in a space is found, and the same text naming a folder
/// that exists only without the space is an orphan.
#[test]
fn review_gitfile_trailing_spaces_are_kept() {
    let output = gitfile_case("review-gitfile-space", |home| {
        let gitdir = home.join("gitdirs/wt ");
        std::fs::create_dir_all(&gitdir).unwrap();
        format!("gitdir: {}\n", gitdir.display()).into_bytes()
    });
    assert_not_orphan("review/gitfile-space", &output);

    let output = gitfile_case("review-gitfile-space-control", |home| {
        std::fs::create_dir_all(home.join("gitdirs/wt")).unwrap();
        format!("gitdir: {} \n", home.join("gitdirs/wt").display()).into_bytes()
    });
    assert_orphan("review/gitfile-space control", &output);
}

/// A relative gitdir is relative to the folder holding the `.git` file, not
/// to devtrim's working directory: `../gitdirs/wt` exists only from there.
#[test]
fn review_gitfile_relative_path_resolves_against_its_folder() {
    let output = gitfile_case("review-gitfile-relative", |home| {
        std::fs::create_dir_all(home.join("dev/gitdirs/wt")).unwrap();
        b"gitdir: ../gitdirs/wt\n".to_vec()
    });
    assert!(
        !Path::new("../gitdirs/wt").exists(),
        "the test's working directory must not hold the relative target"
    );
    assert_not_orphan("review/gitfile-relative", &output);

    let output = gitfile_case("review-gitfile-relative-control", |_| {
        b"gitdir: ../gitdirs/missing\n".to_vec()
    });
    assert_orphan("review/gitfile-relative control", &output);
}
