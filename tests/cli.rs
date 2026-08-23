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
        Self(path)
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
fn empty_json_scan_is_one_document() {
    let sandbox = Sandbox::new("empty-json");
    let output = run(&sandbox, &["scan", "--json"]);
    assert!(output.status.success());
    let value = json(&output);
    assert_eq!(value["operation"], "scan");
    assert_eq!(value["findings"].as_array().unwrap().len(), 0);
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
fn failed_docker_prune_is_nonzero_and_not_summarized() {
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
    assert!(value.get("summary").is_none());
    let errors = value["errors"].as_array().unwrap();
    assert_eq!(errors.len(), 1);
    // Proves the failure came from the prune itself, not from a mismatched scan mock.
    assert!(
        errors[0].as_str().unwrap().contains("image prune -a -f"),
        "expected prune failure, got {errors:?}"
    );
    assert_eq!(value["findings"].as_array().unwrap().len(), 1);
}
