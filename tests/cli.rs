#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions fail by panicking"
)]

mod support;

use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use support::{Sandbox, git_activity, json, run};

fn docker_script(sandbox: &Sandbox, image_prune_exit: i32) {
    let body = format!(
        "{}{image_prune_exit}{}",
        "case \"$*\" in\n  'context inspect') printf '[{\"Endpoints\":{\"docker\":{\"Host\":\"unix:///var/run/docker.sock\"}}}]\\n' ;;\n  '--host unix:///var/run/docker.sock version') printf 'Docker version 28.0.0\\n' ;;\n  '--host unix:///var/run/docker.sock system df'*) printf 'Images\\t2GB\\t1GB (50%%)\\t0\\n' ;;\n  '--host unix:///var/run/docker.sock image prune -a -f') printf 'prune refused by daemon\\n' >&2; exit ",
        " ;;\n  *) exit 1 ;;\nesac"
    );
    sandbox.script("docker", &body);
}

#[test]
fn no_arguments_in_a_non_terminal_prints_help_and_does_not_start_tui() {
    let sandbox = Sandbox::new("no-args");
    let output = run(&sandbox, &[]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert_eq!(output.status.code(), Some(2));
    assert!(stdout.contains("Usage: devtrim"));
    assert!(!stdout.contains("\u{1b}[?1049h"));
}

#[test]
fn parse_errors_escape_terminal_controls_from_argv() {
    let sandbox = Sandbox::new("clap-escape");
    // Clap strips ANSI sequences itself but passes other controls through,
    // such as the bidirectional override that reorders the rest of the line.
    let hostile = "\u{1b}]0;title\u{7}\u{202e}evil\u{85}";

    for args in [vec!["clean", hostile], vec!["help", hostile], vec![hostile]] {
        let output = run(&sandbox, &args);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(output.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(
            stderr.contains("evil"),
            "positive control: {args:?} echoes the argument"
        );
        assert!(
            !stderr.contains(['\u{1b}', '\u{7}', '\u{202e}', '\u{85}']),
            "{args:?} rendered raw terminal controls: {stderr:?}"
        );
    }
}

#[test]
fn run_time_json_errors_name_the_operation_that_failed() {
    let sandbox = Sandbox::new("run-error-operation");
    std::fs::create_dir_all(sandbox.path().join(".config")).unwrap();
    std::fs::write(
        sandbox.path().join(".config/devtrim.toml"),
        "unknown_field = 1\n",
    )
    .unwrap();

    for (args, operation) in [
        (vec!["clean", "caches", "--json"], "caches"),
        (vec!["scan", "--json"], "scan"),
        (vec!["largest", "--json"], "largest"),
    ] {
        let output = run(&sandbox, &args);
        let value = json(&output);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(
            value["errors"][0]
                .as_str()
                .unwrap()
                .contains("invalid config"),
            "{args:?} failed for another reason: {value}"
        );
        assert_eq!(value["operation"], operation, "{args:?}");
    }
}

#[test]
fn clap_parse_errors_with_json_are_one_document_with_the_derived_operation() {
    let sandbox = Sandbox::new("clap-json-error");

    for args in [
        vec!["--json", "clean", "node-modules", "--unknown"],
        vec!["clean", "node-modules", "--unknown", "--json"],
    ] {
        let output = run(&sandbox, &args);

        assert_eq!(output.status.code(), Some(2));
        assert!(output.stderr.is_empty());
        let value = json(&output);
        assert_eq!(value["operation"], "node-modules");
        assert_eq!(value["applied"], false);
        assert!(value["findings"].as_array().unwrap().is_empty());
        assert_eq!(value["errors"].as_array().unwrap().len(), 1);
        assert!(
            value["errors"][0]
                .as_str()
                .unwrap()
                .contains("unexpected argument")
        );
    }

    let output = run(
        &sandbox,
        &[
            "--root",
            "node-modules",
            "clean",
            "artifacts",
            "--unknown",
            "--json",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(json(&output)["operation"], "artifacts");
}

#[test]
fn json_help_and_version_are_one_nonzero_error_document() {
    let sandbox = Sandbox::new("clap-json-display");
    for (args, operation, message) in [
        (vec!["--help", "--json"], "unknown", "help has no JSON form"),
        (vec!["-h", "--json"], "unknown", "help has no JSON form"),
        (
            vec!["clean", "node-modules", "--help", "--json"],
            "node-modules",
            "help has no JSON form",
        ),
        (
            vec!["--version", "--json"],
            "unknown",
            "version has no JSON form",
        ),
        (vec!["-V", "--json"], "unknown", "version has no JSON form"),
    ] {
        let output = run(&sandbox, &args);

        assert_eq!(output.status.code(), Some(1));
        assert!(output.stderr.is_empty());
        let value = json(&output);
        assert_eq!(value["operation"], operation);
        assert_eq!(value["errors"], serde_json::json!([message]));
    }
}

/// Two stale repositories: `small` with a `node_modules`, `large` with a Rust
/// `target` and a `node_modules`. Returns the canonical dev root and both repos.
fn purge_fixture(sandbox: &Sandbox) -> (PathBuf, PathBuf, PathBuf) {
    let dev = sandbox.path().join("dev");
    let small = dev.join("alpha");
    let large = dev.join("beta");
    for project in [&small, &large] {
        std::fs::create_dir_all(project.join(".git")).unwrap();
        std::fs::create_dir_all(project.join("node_modules/pkg")).unwrap();
        std::fs::write(project.join("package.json"), "{}").unwrap();
    }
    std::fs::write(small.join("node_modules/pkg/index.js"), vec![b'x'; 100]).unwrap();
    std::fs::write(large.join("Cargo.toml"), "[package]\nname = \"beta\"\n").unwrap();
    std::fs::create_dir_all(large.join("target/debug")).unwrap();
    std::fs::write(large.join("target/debug/out"), vec![b'x'; 5000]).unwrap();
    std::fs::write(large.join("node_modules/pkg/index.js"), vec![b'x'; 1000]).unwrap();
    sandbox.script("git", &git_activity("2020-01-01", "2020-01-01"));
    let dev = dev.canonicalize().unwrap();
    (dev.clone(), dev.join("alpha"), dev.join("beta"))
}

/// Restores a fixture folder's mode when dropped, so the sandbox can be removed
/// even when an assertion fails first.
struct Unreadable(PathBuf);

impl Unreadable {
    fn new(path: PathBuf) -> Self {
        std::fs::create_dir_all(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert!(
            std::fs::read_dir(&path).is_err(),
            "fixture is still readable; run without privileges that ignore modes"
        );
        Self(path)
    }
}

impl Drop for Unreadable {
    fn drop(&mut self) {
        std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755)).ok();
    }
}

/// One cache the scan cannot measure must not take the rest of the category
/// with it. The unreadable cache is named in one error finding, so the run
/// still exits nonzero, and is never offered; the readable cache listed before
/// it and the one listed after it are. Once it is readable again the same run
/// offers all three.
#[test]
fn one_unreadable_cache_does_not_fail_the_caches_category() {
    let sandbox = Sandbox::in_target("caches-unreadable");
    let home = sandbox.path().canonicalize().unwrap();
    let before = home.join(".cache/huggingface/hub");
    let broken = home.join(".cache/node");
    let after = home.join(".cache/gh");
    for cache in [&before, &broken, &after] {
        support::write(&cache.join("entry"), "cached");
    }
    let locked = Unreadable::new(broken.join("corepack"));

    let output = run(&sandbox, &["clean", "caches", "--json"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let value = json(&output);
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    let mut offered = value["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|finding| finding["path"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();
    offered.sort();
    let mut expected = vec![before.display().to_string(), after.display().to_string()];
    expected.sort();
    assert_eq!(offered, expected, "PV caches/unreadable-cache: {value}");
    let errors = value["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    let error = errors[0].as_str().unwrap();
    assert!(
        error.contains("could not be read") && error.contains(&broken.display().to_string()),
        "{error}"
    );

    drop(locked);
    let healed = run(&sandbox, &["clean", "caches", "--json"]);
    assert!(
        healed.status.success(),
        "{}",
        String::from_utf8_lossy(&healed.stderr)
    );
    assert_eq!(finding_paths(&json(&healed)).len(), 3);
}

/// A tracked-file query that fails is no evidence either way, so the category
/// refuses rather than offer what it could not check, and says why in Git's
/// own words.
#[test]
fn a_failing_tracked_file_check_refuses_rather_than_trusts() {
    let sandbox = Sandbox::new("tracked-check-fails");
    let (dev, _small, _large) = purge_fixture(&sandbox);
    sandbox.script(
        "git",
        "case \"$*\" in\n  *ls-files*) echo 'fatal: index file corrupt' >&2; exit 128 ;;\n  *' -g '*) printf 'HEAD@{2020-01-01}\\n' ;;\n  *) printf '2020-01-01\\n' ;;\nesac",
    );

    let output = run(
        &sandbox,
        &["purge", "--root", dev.to_str().unwrap(), "--json"],
    );

    assert!(!output.status.success());
    let value = json(&output);
    assert!(
        value["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|finding| finding["path"].is_null()),
        "{value}"
    );
    let errors = value["errors"].to_string();
    assert!(
        errors.contains("Git tracked-file check failed") && errors.contains("index file corrupt"),
        "{value}"
    );
}

fn finding_paths(value: &Value) -> Vec<String> {
    value["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| finding["path"].as_str().unwrap().to_string())
        .collect()
}

/// The tracked-file check against real Git. A pathspec setting in the caller's
/// environment makes Git refuse `--literal-pathspecs` outright, so the check
/// must not inherit one. CocoaPods recommends committing `Pods`; the untracked
/// `target` beside it is the positive control.
#[test]
fn the_tracked_file_check_uses_real_git_and_ignores_ambient_pathspec_settings() {
    let sandbox = Sandbox::new("tracked-real-git");
    let resolved = Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(resolved.status.success(), "git is required for this test");
    let git = String::from_utf8(resolved.stdout).unwrap();
    sandbox.script("git", &format!("exec '{}' \"$@\"", git.trim()));
    let project = sandbox.path().join("dev/app");
    std::fs::create_dir_all(project.join("Pods/Alamofire")).unwrap();
    std::fs::write(project.join("Podfile"), "platform :ios, '17.0'\n").unwrap();
    std::fs::write(
        project.join("Pods/Alamofire/Session.swift"),
        "// vendored\n",
    )
    .unwrap();
    std::fs::write(project.join("Cargo.toml"), "[package]\nname = \"app\"\n").unwrap();
    std::fs::create_dir_all(project.join("target/debug")).unwrap();
    std::fs::write(project.join("target/debug/out"), vec![b'x'; 4096]).unwrap();
    for args in [
        vec!["init", "-q"],
        vec!["add", "-f", "--", "Podfile", "Pods", "Cargo.toml"],
        vec![
            "-c",
            "user.name=devtrim-test",
            "-c",
            "user.email=devtrim@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "vendored pods",
        ],
    ] {
        let status = Command::new(git.trim())
            .args(&args)
            .current_dir(&project)
            .env("HOME", sandbox.path())
            .env_remove("XDG_CONFIG_HOME")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }
    let project = project.canonicalize().unwrap();

    // One makes Git refuse the query outright; the other would silently turn
    // its pathspec magic into a literal name that matches nothing.
    for setting in ["GIT_ICASE_PATHSPECS", "GIT_LITERAL_PATHSPECS"] {
        let output = Command::new(env!("CARGO_BIN_EXE_devtrim"))
            .args(["purge", "--root", project.to_str().unwrap(), "--json"])
            .env("HOME", sandbox.path())
            .env("PATH", sandbox.bin())
            .env_remove("XDG_STATE_HOME")
            .env(setting, "1")
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "{setting}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let paths = finding_paths(&json(&output));
        assert!(
            !paths.contains(&project.join("Pods").display().to_string()),
            "{setting}: a committed Pods directory was offered: {paths:?}"
        );
        assert!(
            paths.contains(&project.join("target").display().to_string()),
            "{setting}: positive control: the untracked target was not offered: {paths:?}"
        );
    }
}

/// A failed liveness probe is no evidence that nothing is building, so the
/// stale output beside it is refused, not offered: in each category alone and
/// in both halves of one `scan`, which share a single memoized probe. The
/// same fixture is offered once the probe answers.
#[test]
fn project_cleanup_probe_failures_are_nonzero_json_errors() {
    let sandbox = Sandbox::new("project-probe-failure");
    let project = sandbox.path().join("dev/project");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::create_dir_all(project.join("node_modules/pkg")).unwrap();
    std::fs::write(project.join("package.json"), "{}").unwrap();
    std::fs::write(project.join("node_modules/pkg/index.js"), "x").unwrap();
    std::fs::create_dir_all(project.join("target")).unwrap();
    std::fs::write(project.join("Cargo.toml"), "[package]\nname = \"test\"\n").unwrap();
    std::fs::write(project.join("target/output"), "x").unwrap();
    sandbox.script("git", &git_activity("2020-01-01", "2020-01-01"));
    let offered = |value: &Value| {
        value["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|finding| finding["path"].as_str().map(str::to_owned))
            .count()
    };

    sandbox.script("pgrep", "exit 1");
    let control = run(&sandbox, &["scan", "--json"]);
    assert_eq!(
        offered(&json(&control)),
        2,
        "positive control: with no build running both outputs are offered"
    );

    sandbox.script("pgrep", "exit 2");
    for target in ["node-modules", "artifacts"] {
        let output = run(&sandbox, &["clean", target, "--json"]);

        assert!(!output.status.success());
        let value = json(&output);
        assert_eq!(value["operation"], target);
        assert_eq!(offered(&value), 0, "{value}");
        assert!(
            value["errors"][0]
                .as_str()
                .unwrap()
                .contains("cannot verify build-process liveness")
        );
    }
    let scan = run(&sandbox, &["scan", "--json"]);
    assert!(!scan.status.success());
    let value = json(&scan);
    assert_eq!(offered(&value), 0, "{value}");
    for operation in ["node-modules", "artifacts"] {
        assert!(
            value["errors"].as_array().unwrap().iter().any(|error| {
                let error = error.as_str().unwrap();
                error.contains(operation) && error.contains("cannot verify build-process liveness")
            }),
            "{operation}: {value}"
        );
    }
}

#[test]
fn empty_json_apply_includes_a_zero_summary() {
    let sandbox = Sandbox::new("empty-apply-summary");
    let output = run(&sandbox, &["clean", "caches", "--apply", "-y", "--json"]);

    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(value["applied"], true);
    assert_eq!(value["summary"]["op"], "caches");
    assert_eq!(value["summary"]["items_touched"], 0);
    assert_eq!(value["summary"]["bytes_freed_estimate"], 0);
    assert!(value["summary"]["notes"].as_array().unwrap().is_empty());
}

#[test]
fn status_reads_system_tools_in_the_c_locale() {
    let sandbox = Sandbox::new("status-locale");
    // As on a pt_BR machine: decimal commas unless the probe pins LC_ALL=C.
    sandbox.script(
        "sysctl",
        "case \"$*\" in\n  '-n vm.loadavg') if [ \"${LC_ALL:-}\" = C ]; then printf '{ 1.50 2.25 3.00 }\\n'; else printf '{ 1,50 2,25 3,00 }\\n'; fi ;;\n  *) exit 1 ;;\nesac",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_devtrim"))
        .args(["status", "--json"])
        .env("HOME", sandbox.path())
        .env("PATH", sandbox.bin())
        .env("LC_ALL", "pt_BR.UTF-8")
        .output()
        .unwrap();

    let value = json(&output);
    assert_eq!(
        value["load_average"],
        serde_json::json!([1.5, 2.25, 3.0]),
        "{value}"
    );
}

#[test]
fn shred_is_explicit_in_preview() {
    let sandbox = Sandbox::new("shred-preview");
    std::fs::create_dir_all(sandbox.path().join(".cache/uv")).unwrap();
    std::fs::write(sandbox.path().join(".cache/uv/file"), "x").unwrap();
    let output = run(&sandbox, &["clean", "caches", "--shred", "--json"]);
    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(value["findings"][0]["action"]["type"], "shred");
    assert_eq!(value["findings"][0]["danger"], 9);
}

#[test]
fn owner_reported_cache_outside_namespace_is_skipped() {
    let sandbox = Sandbox::new("owner-cache-outside");
    let documents = sandbox.path().canonicalize().unwrap().join("Documents");
    std::fs::create_dir_all(&documents).unwrap();
    std::fs::write(documents.join("sentinel"), "keep").unwrap();
    let called = sandbox.path().join("npm-called");
    sandbox.script(
        "npm",
        &format!(
            "printf '%s\\n' '{}'; : > '{}'",
            documents.display(),
            called.display()
        ),
    );

    let output = run(&sandbox, &["clean", "caches", "--json"]);
    let value = json(&output);

    assert!(output.status.success());
    assert!(
        called.exists(),
        "the fake npm owner command was not exercised"
    );
    assert!(documents.join("sentinel").exists());
    assert!(
        value["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|finding| finding["path"].as_str() != documents.to_str())
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("outside a home cache location; skipping")
    );
}

#[test]
fn failed_docker_prune_is_nonzero_with_truthful_zero_summary() {
    let sandbox = Sandbox::new("docker-failure");
    // The format argument contains a real tab, so match on a prefix: an exact pattern
    // would silently fall through to the catch-all and make this a scan-failure test.
    docker_script(&sandbox, 9);
    let output = run(
        &sandbox,
        &["clean", "docker", "--apply", "--yolo", "--json"],
    );
    assert!(!output.status.success());
    let value = json(&output);
    assert_eq!(value["summary"]["items_touched"], 0);
    let errors = value["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    // Proves the failure came from the prune itself, not from a mismatched scan mock.
    assert!(
        errors[0].as_str().unwrap().contains("image prune -a -f"),
        "expected prune failure, got {errors:?}"
    );
    // A failed typed command is diagnosable: its exit status and stderr survive.
    assert!(
        errors[0].as_str().unwrap().contains("exit status: 9")
            && errors[0]
                .as_str()
                .unwrap()
                .contains("prune refused by daemon"),
        "expected status and stderr, got {errors:?}"
    );
    assert_eq!(value["findings"].as_array().unwrap().len(), 1);
    assert!(!String::from_utf8_lossy(&output.stderr).contains("DATA-LOSS WARNING"));
    let records =
        std::fs::read_to_string(sandbox.path().join(".local/state/devtrim/journal.jsonl")).unwrap();
    let records = records
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        records[0]["argv"],
        serde_json::json!([
            "docker",
            "--host",
            "unix:///var/run/docker.sock",
            "image",
            "prune",
            "-a",
            "-f"
        ])
    );
    assert_eq!(records[1]["status"], "error");
}

#[test]
fn human_apply_prints_data_loss_warning_before_action() {
    let sandbox = Sandbox::new("risk-warning");
    docker_script(&sandbox, 0);

    let output = run(&sandbox, &["clean", "docker", "--apply", "-y"]);

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("DATA-LOSS WARNING"));
}
