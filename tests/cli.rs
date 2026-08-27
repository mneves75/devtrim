use serde_json::Value;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Sandbox(PathBuf);

impl Sandbox {
    fn new(name: &str) -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("devtrim-cli-{name}-{}-{id}", std::process::id()));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        let sandbox = Self(path);
        sandbox.script("pgrep", "exit 1");
        sandbox
    }

    fn in_target(name: &str) -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-cli-{name}-{}-{id}", std::process::id()));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        let sandbox = Self(path);
        sandbox.script("pgrep", "exit 1");
        sandbox
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn bin(&self) -> PathBuf {
        let path = self.0.join("bin");
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.bin().join(name);
        std::fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn run(sandbox: &Sandbox, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_devtrim"))
        .args(args)
        .env("HOME", sandbox.path())
        .env("PATH", sandbox.bin())
        .env_remove("XDG_STATE_HOME")
        .output()
        .unwrap()
}

fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON: {error}; stdout={}; stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
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
fn explicit_tui_requires_an_interactive_terminal() {
    let sandbox = Sandbox::new("tui-non-terminal");
    let output = run(&sandbox, &["tui"]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("requires an interactive stdin and stdout terminal")
    );
}

#[test]
fn tui_rejects_cli_confirmation_bypasses() {
    let sandbox = Sandbox::new("tui-flags");
    let output = run(&sandbox, &["tui", "--apply", "--yolo"]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("the TUI owns preview and confirmation")
    );
}

#[test]
fn no_command_json_error_remains_one_document() {
    let sandbox = Sandbox::new("no-command-json");
    let output = run(&sandbox, &["--json"]);
    let value = json(&output);

    assert!(!output.status.success());
    assert_eq!(value["operation"], "unknown");
    assert_eq!(value["errors"].as_array().unwrap().len(), 1);
}

#[test]
fn empty_json_scan_is_one_document() {
    let sandbox = Sandbox::new("empty-json");
    let output = run(&sandbox, &["scan", "--json"]);
    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(value["operation"], "scan");
    assert_eq!(value["findings"].as_array().unwrap().len(), 0);
}

#[test]
fn largest_json_is_one_read_only_document() {
    let sandbox = Sandbox::new("largest-json");
    let root = sandbox.path().join("largest-root");
    std::fs::create_dir_all(root.join("project/cache")).unwrap();
    std::fs::write(root.join("project/cache/payload"), vec![0; 12]).unwrap();

    let output = run(
        &sandbox,
        &[
            "largest",
            "--root",
            root.to_str().unwrap(),
            "--apply",
            "--shred",
            "--json",
        ],
    );

    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(value["operation"], "largest");
    assert_eq!(value["applied"], false);
    assert_eq!(value["findings"][0]["label"], "large directory");
    assert_eq!(value["findings"][0]["action"]["type"], "info");
    assert!(value["errors"].as_array().unwrap().is_empty());
}

#[test]
fn largest_unreadable_entry_is_disclosed_and_nonzero() {
    let sandbox = Sandbox::new("largest-unreadable");
    let root = sandbox.path().join("largest-root");
    let unreadable = root.join("project/private");
    std::fs::create_dir_all(&unreadable).unwrap();
    std::fs::write(unreadable.join("hidden"), "not measured").unwrap();
    let original = std::fs::metadata(&unreadable).unwrap().permissions();
    let mut denied = original.clone();
    denied.set_mode(0o000);
    std::fs::set_permissions(&unreadable, denied).unwrap();

    let output = run(
        &sandbox,
        &["largest", "--root", root.to_str().unwrap(), "--json"],
    );
    std::fs::set_permissions(&unreadable, original).unwrap();

    // Partial visibility matches the scan contract: errors AND nonzero status.
    assert!(!output.status.success());
    let value = json(&output);
    assert_eq!(value["operation"], "largest");
    assert_eq!(value["errors"].as_array().unwrap().len(), 1);
    assert!(
        value["errors"][0]
            .as_str()
            .unwrap()
            .contains("totals are lower bounds")
    );
}

#[test]
fn oversized_journal_rotates_once_when_context_opens() {
    let sandbox = Sandbox::new("journal-open-rotation");
    let root = sandbox.path().join("largest-root");
    std::fs::create_dir_all(&root).unwrap();
    let journal = sandbox.path().join(".local/state/devtrim/journal.jsonl");
    std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
    let file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&journal)
        .unwrap();
    file.set_len(10 * 1024 * 1024 + 1).unwrap();
    drop(file);
    let args = ["largest", "--root", root.to_str().unwrap(), "--json"];

    let first = run(&sandbox, &args);
    let second = run(&sandbox, &args);

    assert!(first.status.success());
    assert!(second.status.success());
    assert_eq!(
        std::fs::metadata(journal.with_extension("jsonl.1"))
            .unwrap()
            .len(),
        10 * 1024 * 1024 + 1
    );
    assert!(!journal.with_extension("jsonl.2").exists());
    assert!(!journal.exists());
}

#[test]
fn malformed_config_fails_closed_with_json() {
    let sandbox = Sandbox::new("bad-config");
    std::fs::create_dir_all(sandbox.path().join(".config")).unwrap();
    std::fs::write(sandbox.path().join(".config/devtrim.toml"), "roots = [").unwrap();
    let output = run(&sandbox, &["scan", "--json"]);
    assert!(!output.status.success());
    assert!(!json(&output)["errors"].as_array().unwrap().is_empty());
}

#[test]
fn unknown_config_field_fails_closed_with_json() {
    let sandbox = Sandbox::new("unknown-config-field");
    std::fs::create_dir_all(sandbox.path().join(".config")).unwrap();
    std::fs::write(
        sandbox.path().join(".config/devtrim.toml"),
        "rootz = [\"~/only-this-root\"]\n",
    )
    .unwrap();

    let output = run(&sandbox, &["scan", "--json"]);

    assert!(!output.status.success());
    assert!(
        json(&output)["errors"][0]
            .as_str()
            .unwrap()
            .contains("unknown field `rootz`")
    );
}

#[test]
fn relative_protect_entry_fails_closed_with_json() {
    let sandbox = Sandbox::new("relative-protect");
    std::fs::create_dir_all(sandbox.path().join(".config")).unwrap();
    std::fs::write(
        sandbox.path().join(".config/devtrim.toml"),
        "protect = [\"dev/keep\"]\n",
    )
    .unwrap();

    let output = run(&sandbox, &["scan", "--json"]);

    assert!(!output.status.success());
    let value = json(&output);
    assert!(value["errors"][0].as_str().unwrap().contains("dev/keep"));
    assert!(value["errors"][0].as_str().unwrap().contains("absolute"));
}

#[test]
fn config_tilde_root_is_expanded() {
    let sandbox = Sandbox::new("tilde-root");
    let project = sandbox.path().join("dev/project");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::create_dir_all(project.join("node_modules")).unwrap();
    std::fs::write(project.join("node_modules/file"), "x").unwrap();
    std::fs::create_dir_all(sandbox.path().join(".config")).unwrap();
    std::fs::write(
        sandbox.path().join(".config/devtrim.toml"),
        "roots = [\"~/dev\"]\nactive_days = 30\n",
    )
    .unwrap();
    sandbox.script("git", "printf '2020-01-01\\n'");
    let output = run(&sandbox, &["clean", "node-modules", "--json"]);
    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(value["findings"].as_array().unwrap().len(), 1);
    let home = sandbox.path().canonicalize().unwrap();
    assert!(
        value["findings"][0]["path"]
            .as_str()
            .unwrap()
            .starts_with(home.to_str().unwrap())
    );
}

#[test]
fn config_tilde_protect_filters_preview_with_diagnostic() {
    let sandbox = Sandbox::new("tilde-protect");
    let project = sandbox.path().join("dev/project");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::create_dir_all(project.join("node_modules")).unwrap();
    std::fs::write(project.join("node_modules/file"), "x").unwrap();
    std::fs::create_dir_all(sandbox.path().join(".config")).unwrap();
    std::fs::write(
        sandbox.path().join(".config/devtrim.toml"),
        "roots = [\"~/dev\"]\nprotect = [\"~/dev/project/node_modules\"]\nactive_days = 30\n",
    )
    .unwrap();
    sandbox.script("git", "printf '2020-01-01\\n'");

    let output = run(&sandbox, &["clean", "node-modules", "--json"]);

    assert!(output.status.success());
    assert!(json(&output)["findings"].as_array().unwrap().is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("skipping protected path"));
}

#[test]
fn artifacts_target_scans_corroborated_stale_repo() {
    let sandbox = Sandbox::new("artifacts-target");
    let project = sandbox.path().join("dev/project");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::create_dir_all(project.join("target")).unwrap();
    std::fs::write(project.join("Cargo.toml"), "[package]\nname = \"test\"\n").unwrap();
    std::fs::write(project.join("target/output"), "x").unwrap();
    sandbox.script("git", "printf '2020-01-01\\n'");

    let output = run(
        &sandbox,
        &[
            "clean",
            "artifacts",
            "--root",
            sandbox.path().join("dev").to_str().unwrap(),
            "--json",
        ],
    );

    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(value["operation"], "artifacts");
    assert_eq!(value["findings"].as_array().unwrap().len(), 1);
    assert_eq!(value["findings"][0]["label"], "stale target artifacts");
}

#[test]
fn project_cleanup_probe_failures_are_nonzero_json_errors() {
    let sandbox = Sandbox::new("project-probe-failure");
    sandbox.script("pgrep", "exit 2");

    for target in ["node-modules", "artifacts"] {
        let output = run(&sandbox, &["clean", target, "--json"]);

        assert!(!output.status.success());
        let value = json(&output);
        assert_eq!(value["operation"], target);
        assert!(
            value["errors"][0]
                .as_str()
                .unwrap()
                .contains("cannot verify build-process liveness")
        );
    }
}

#[test]
fn trash_empty_requires_apply() {
    let sandbox = Sandbox::new("trash-preview");
    std::fs::create_dir_all(sandbox.path().join(".Trash")).unwrap();
    let sentinel = sandbox.path().join(".Trash/keep");
    std::fs::write(&sentinel, "keep").unwrap();
    let output = run(&sandbox, &["trash-empty", "--confirm=0", "--json"]);
    assert!(output.status.success());
    assert!(sentinel.exists());
    assert_eq!(json(&output)["applied"], false);
}

#[test]
fn trash_empty_yolo_still_requires_size_acknowledgment() {
    let sandbox = Sandbox::new("trash-yolo-ack");
    std::fs::create_dir_all(sandbox.path().join(".Trash")).unwrap();
    let sentinel = sandbox.path().join(".Trash/keep");
    std::fs::write(&sentinel, "keep").unwrap();

    let output = run(&sandbox, &["trash-empty", "--apply", "--yolo", "--json"]);

    assert!(!output.status.success());
    assert!(sentinel.exists());
    assert!(
        json(&output)["errors"][0]
            .as_str()
            .unwrap()
            .contains("requires --confirm=<gb>")
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
fn unavailable_simulator_json_is_detected_without_erase_all() {
    let sandbox = Sandbox::new("simulator-json");
    let log = sandbox.path().join("xcrun.log");
    sandbox.script(
        "xcrun",
        "printf '%s\\n' \"$*\" >> \"$DEVTRIM_TEST_LOG\"\ncase \"$*\" in\n  '--version') exit 0 ;;\n  'simctl list devices --json') printf '%s\\n' '{\"devices\":{\"com.apple.CoreSimulator.SimRuntime.iOS-18-0\":[{\"dataPath\":\"/tmp/device\",\"dataPathSize\":0,\"logPath\":\"/tmp/log\",\"udid\":\"00000000-0000-0000-0000-000000000000\",\"isAvailable\":false,\"deviceTypeIdentifier\":\"com.apple.CoreSimulator.SimDeviceType.iPhone-16\",\"state\":\"Shutdown\",\"name\":\"iPhone 16\"}]}}' ;;\n  'simctl delete unavailable') exit 0 ;;\n  *) exit 1 ;;\nesac",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_devtrim"))
        .args(["clean", "simulators", "--apply", "--yolo", "--json"])
        .env("HOME", sandbox.path())
        .env("PATH", sandbox.bin())
        .env_remove("XDG_STATE_HOME")
        .env("DEVTRIM_TEST_LOG", &log)
        .output()
        .unwrap();
    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(
        value["findings"][0]["label"],
        "unavailable Apple simulator devices"
    );
    let calls = std::fs::read_to_string(log).unwrap();
    assert!(calls.contains("simctl list devices --json"));
    assert!(calls.contains("simctl delete unavailable"));
    assert!(!calls.contains("erase all"));
    let records =
        std::fs::read_to_string(sandbox.path().join(".local/state/devtrim/journal.jsonl")).unwrap();
    let records = records
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(records[0]["action"], "command");
    assert_eq!(
        records[0]["argv"],
        serde_json::json!(["xcrun", "simctl", "delete", "unavailable"])
    );
    assert_eq!(records[1]["status"], "ok");
}

#[test]
fn node_modules_apply_refuses_repo_that_became_active() {
    let sandbox = Sandbox::new("node-became-active");
    let project = sandbox.path().join("dev/project");
    let target = project.join("node_modules");
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("sentinel"), "keep").unwrap();
    let state = sandbox.path().join("git-state");
    sandbox.script(
        "git",
        &format!(
            "if [ -e '{}' ]; then printf '2999-01-01\\n'; else : > '{}'; printf '2020-01-01\\n'; fi",
            state.display(),
            state.display()
        ),
    );

    let output = run(
        &sandbox,
        &["clean", "node-modules", "--apply", "-y", "--json"],
    );
    assert!(!output.status.success());
    assert!(target.join("sentinel").exists());
    let value = json(&output);
    assert_eq!(value["operation"], "node-modules");
    assert!(
        value["errors"][0]
            .as_str()
            .unwrap()
            .contains("repo became active after preview")
    );
}

#[test]
fn partial_apply_serializes_first_success_then_stops() {
    let sandbox = Sandbox::in_target("partial-apply");
    let first = sandbox.path().join("dev/a/node_modules");
    let second = sandbox.path().join("dev/b/node_modules");
    for target in [&first, &second] {
        std::fs::create_dir_all(target.parent().unwrap().join(".git")).unwrap();
        std::fs::create_dir_all(target).unwrap();
        std::fs::write(target.join("sentinel"), "keep").unwrap();
    }
    let counter = sandbox.path().join("git-count");
    sandbox.script(
        "git",
        "count=0\nif [ -f \"$DEVTRIM_TEST_COUNT\" ]; then read count < \"$DEVTRIM_TEST_COUNT\"; fi\ncount=$((count + 1))\nprintf '%s\\n' \"$count\" > \"$DEVTRIM_TEST_COUNT\"\ncase \"$count\" in\n  1|2|3) printf '2020-01-01\\n' ;;\n  *) exit 9 ;;\nesac",
    );

    let output = Command::new(env!("CARGO_BIN_EXE_devtrim"))
        .args([
            "clean",
            "node-modules",
            "--apply",
            "--shred",
            "--yolo",
            "--json",
        ])
        .env("HOME", sandbox.path())
        .env("PATH", sandbox.bin())
        .env_remove("XDG_STATE_HOME")
        .env("DEVTRIM_TEST_COUNT", &counter)
        .output()
        .unwrap();

    let value = json(&output);
    assert!(!output.status.success());
    assert!(
        !first.exists(),
        "first target remained; response={value}; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(second.join("sentinel").exists());
    assert_eq!(value["operation"], "node-modules");
    assert_eq!(value["applied"], true);
    assert_eq!(value["summary"]["items_touched"], 1);
    assert_eq!(value["errors"].as_array().unwrap().len(), 1);
    assert!(
        value["errors"][0]
            .as_str()
            .unwrap()
            .contains("Git activity check failed")
    );
}

#[test]
fn failed_docker_prune_is_nonzero_with_truthful_zero_summary() {
    let sandbox = Sandbox::new("docker-failure");
    // The format argument contains a real tab, so match on a prefix: an exact pattern
    // would silently fall through to the catch-all and make this a scan-failure test.
    sandbox.script(
        "docker",
        "case \"$*\" in\n  'version') exit 0 ;;\n  'system df'*) printf 'Images\\t2GB\\t1GB (50%%)\\n' ;;\n  'image prune -a -f') exit 9 ;;\n  *) exit 1 ;;\nesac",
    );
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
        serde_json::json!(["docker", "image", "prune", "-a", "-f"])
    );
    assert_eq!(records[1]["status"], "error");
}

#[test]
fn human_apply_prints_data_loss_warning_before_action() {
    let sandbox = Sandbox::new("risk-warning");
    sandbox.script(
        "docker",
        "case \"$*\" in\n  'version') exit 0 ;;\n  'system df'*) printf 'Images\\t2GB\\t1GB (50%%)\\n' ;;\n  'image prune -a -f') exit 0 ;;\n  *) exit 1 ;;\nesac",
    );

    let output = run(&sandbox, &["clean", "docker", "--apply", "-y"]);

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("DATA-LOSS WARNING"));
}

#[test]
fn help_states_that_apply_flags_accept_data_loss_risk() {
    let sandbox = Sandbox::new("risk-help");
    let output = run(&sandbox, &["--help"]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success());
    assert!(stdout.contains("cleanup can delete data"));
    assert!(stdout.matches("Accept data-loss risk").count() >= 2);
}

#[test]
fn history_json_is_one_document_and_skips_malformed_lines() {
    let sandbox = Sandbox::new("history-json");
    let journal = sandbox.path().join(".local/state/devtrim/journal.jsonl");
    std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
    std::fs::write(
        &journal,
        concat!(
            "{\"ts\":2,\"phase\":\"result\",\"op\":\"caches\",\"action\":\"trash\",\"target\":\"/tmp/cache\",\"size_bytes\":4,\"status\":\"ok\"}\n",
            "not-json\n"
        ),
    )
    .unwrap();

    let output = run(&sandbox, &["history", "--json"]);

    // A partial audit is a partial operation: entries survive, errors are
    // reported, and the status is nonzero.
    assert!(!output.status.success());
    let value = json(&output);
    assert_eq!(value["operation"], "history");
    assert_eq!(value["entries"].as_array().unwrap().len(), 1);
    assert_eq!(value["entries"][0]["status"], "ok");
    assert_eq!(value["errors"].as_array().unwrap().len(), 1);
}

#[test]
fn missing_history_is_empty_and_successful() {
    let sandbox = Sandbox::new("history-missing");

    let output = run(&sandbox, &["history", "--json"]);

    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(value["operation"], "history");
    assert!(value["entries"].as_array().unwrap().is_empty());
    assert!(value["errors"].as_array().unwrap().is_empty());
}

#[test]
fn history_human_marks_orphan_attempt_interrupted() {
    let sandbox = Sandbox::new("history-interrupted");
    let journal = sandbox.path().join(".local/state/devtrim/journal.jsonl");
    std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
    std::fs::write(
        &journal,
        "{\"ts\":1,\"phase\":\"attempt\",\"op\":\"xcode\",\"action\":\"shred\",\"target\":\"/tmp/build\",\"size_bytes\":8}\n",
    )
    .unwrap();

    let output = run(&sandbox, &["history"]);

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("interrupted"));
}

#[test]
fn unreadable_history_is_one_error_document_and_nonzero() {
    let sandbox = Sandbox::new("history-unreadable");
    let journal = sandbox.path().join(".local/state/devtrim/journal.jsonl");
    std::fs::create_dir_all(&journal).unwrap();

    let output = run(&sandbox, &["history", "--json"]);

    assert!(!output.status.success());
    let value = json(&output);
    assert_eq!(value["operation"], "history");
    assert!(value["entries"].as_array().unwrap().is_empty());
    assert_eq!(value["errors"].as_array().unwrap().len(), 1);
}

#[test]
fn history_uses_only_an_absolute_xdg_state_home() {
    let record = "{\"ts\":2,\"phase\":\"result\",\"op\":\"caches\",\"action\":\"trash\",\"target\":\"/tmp/cache\",\"size_bytes\":4,\"status\":\"ok\"}\n";

    let absolute = Sandbox::new("history-xdg-absolute");
    let state_home = absolute.path().join("state");
    let journal = state_home.join("devtrim/journal.jsonl");
    std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
    std::fs::write(&journal, record).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_devtrim"))
        .args(["history", "--json"])
        .env("HOME", absolute.path())
        .env("PATH", absolute.bin())
        .env("XDG_STATE_HOME", &state_home)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(json(&output)["entries"].as_array().unwrap().len(), 1);

    let relative = Sandbox::new("history-xdg-relative");
    let fallback = relative.path().join(".local/state/devtrim/journal.jsonl");
    std::fs::create_dir_all(fallback.parent().unwrap()).unwrap();
    std::fs::write(&fallback, record).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_devtrim"))
        .args(["history", "--json"])
        .env("HOME", relative.path())
        .env("PATH", relative.bin())
        .env("XDG_STATE_HOME", "relative-state")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(json(&output)["entries"].as_array().unwrap().len(), 1);
}

#[test]
fn zsh_completions_are_printed_to_piped_stdout() {
    let sandbox = Sandbox::new("completions");

    let output = run(&sandbox, &["completions", "zsh"]);

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("_devtrim"));
}

#[test]
fn completions_reject_unsupported_shells() {
    let sandbox = Sandbox::new("unsupported-completions");

    let output = run(&sandbox, &["completions", "powershell"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("bash, zsh, fish"));
}

#[test]
fn manpage_is_printed_to_piped_stdout() {
    let sandbox = Sandbox::new("manpage");

    let output = run(&sandbox, &["manpage"]);

    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains(".TH devtrim"));
}

#[test]
fn generated_docs_reject_json_with_one_error_document() {
    let sandbox = Sandbox::new("generated-docs-json");
    for command in ["completions", "manpage"] {
        let args = if command == "completions" {
            vec![command, "zsh", "--json"]
        } else {
            vec![command, "--json"]
        };
        let output = run(&sandbox, &args);
        assert!(!output.status.success());
        let value = json(&output);
        assert_eq!(value["operation"], command);
        assert_eq!(value["errors"].as_array().unwrap().len(), 1);
    }
}
