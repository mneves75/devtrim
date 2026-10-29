//! Dashboard behavior through the real CLI and generated offline document.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions fail by panicking"
)]

mod support;

use serde_json::Value;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::process::Command;
use support::{Allowed, Sandbox, Tree, git_activity, json, run, write};

/// Under `target`: a home in the temporary folder resolves inside the
/// protected `/private/var`, where no cleanup preview offers anything.
fn fixture(name: &str) -> Sandbox {
    let sandbox = Sandbox::in_target(name);
    sandbox.script("xcrun", "if [ \"$1\" = --version ]; then printf 'xcrun version 70\\n'; else printf '{\"devices\":{}}\\n'; fi");
    sandbox
}

fn embedded(document: &str) -> Value {
    let marker = "<script id=\"snapshot\" type=\"application/json\">";
    let data = document
        .split_once(marker)
        .unwrap()
        .1
        .split_once("</script>")
        .unwrap()
        .0;
    serde_json::from_str(data).unwrap()
}

#[test]
fn copied_project_preview_preserves_explicit_roots_and_inert_shell_data() {
    let sandbox = Sandbox::in_target("dashboard-root-preview");
    sandbox.script("xcrun", "if [ \"$1\" = --version ]; then printf 'xcrun version 70\\n'; else printf '{\"devices\":{}}\\n'; fi");
    sandbox.script("git", &git_activity("2000-01-01", "2000-01-01"));
    sandbox.script(
        "devtrim",
        "printf '%s\\0' \"$@\" > \"$HOME/copied-argv\"\nexec \"$DEVTRIM_BIN\" \"$@\"",
    );
    let root = sandbox
        .path()
        .join("odd ' \\ path; & | $(printf shell-ran > $HOME/shell-ran)\n\t\r\u{7f}\u{85}");
    let target = root.join("project/target");
    let ordinary_root = sandbox
        .path()
        .join("literal ' \\ path $(printf shell-ran > $HOME/shell-ran)");
    std::fs::create_dir_all(&ordinary_root).unwrap();
    write(&root.join("project/.git/HEAD"), "ref: refs/heads/main\n");
    write(
        &root.join("project/Cargo.toml"),
        "[package]\nname = 'fixture'\n",
    );
    write(&target.join("build.bin"), "build output");
    write(
        &sandbox.path().join(".cache/huggingface/hub/weights"),
        "cache",
    );
    std::fs::create_dir(sandbox.path().join("configured")).unwrap();
    write(
        &sandbox.path().join(".config/devtrim.toml"),
        "roots = [\"~/configured\"]\n",
    );
    let before = Tree::snapshot(sandbox.path());

    let ordinary = run(&sandbox, &["clean", "artifacts", "--json"]);
    assert!(ordinary.status.success());
    assert!(json(&ordinary)["findings"].as_array().unwrap().is_empty());
    let eligible = run(
        &sandbox,
        &[
            "clean",
            "artifacts",
            "--root",
            root.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(
        eligible.status.success(),
        "{}",
        String::from_utf8_lossy(&eligible.stdout)
    );
    assert_eq!(
        json(&eligible)["findings"][0]["path"],
        target.to_str().unwrap()
    );

    let report = sandbox.path().join("report.html");
    let snapshot_output = Command::new(env!("CARGO_BIN_EXE_devtrim"))
        .args([
            "dashboard",
            "--root",
            root.to_str().unwrap(),
            "--root",
            ordinary_root.to_str().unwrap(),
            "--root",
            "missing projects",
            "--output",
            report.to_str().unwrap(),
            "--no-open",
            "--json",
        ])
        .current_dir(sandbox.path())
        .env_clear()
        .env("HOME", sandbox.path())
        .env("PATH", sandbox.bin())
        .output()
        .unwrap();
    assert!(
        snapshot_output.status.success(),
        "{}",
        String::from_utf8_lossy(&snapshot_output.stdout)
    );
    let document = json(&snapshot_output);
    let categories = document["recommendations"]["categories"]
        .as_array()
        .unwrap();
    let category = categories
        .iter()
        .find(|category| category["category"] == "artifacts")
        .unwrap();
    assert_eq!(category["findings"][0]["path"], target.to_str().unwrap());
    let command = category["preview_command"].as_str().unwrap();
    assert!(
        !command.chars().any(char::is_control),
        "a single-line command field must preserve controls through escapes"
    );
    let later_cwd = sandbox.path().join("later-terminal");
    std::fs::create_dir(&later_cwd).unwrap();
    let copied = Command::new("/bin/zsh")
        .args(["-f", "-c", &format!("{command} --json")])
        .current_dir(&later_cwd)
        .env_clear()
        .env("HOME", sandbox.path())
        .env("PATH", sandbox.bin())
        .env("DEVTRIM_BIN", env!("CARGO_BIN_EXE_devtrim"))
        .output()
        .unwrap();
    assert!(
        !sandbox.path().join("shell-ran").exists(),
        "PV dashboard-preview/inert-roots"
    );
    assert!(
        copied.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&copied.stdout),
        String::from_utf8_lossy(&copied.stderr)
    );
    assert_eq!(
        json(&copied)["findings"],
        json(&eligible)["findings"],
        "copied preview must retain the displayed project's scope"
    );
    let argv = std::fs::read(sandbox.path().join("copied-argv")).unwrap();
    let arguments = argv.split(|byte| *byte == 0).collect::<Vec<_>>();
    assert!(arguments.contains(&root.as_os_str().as_encoded_bytes()));
    assert!(arguments.contains(&ordinary_root.as_os_str().as_encoded_bytes()));
    assert!(
        arguments.contains(
            &sandbox
                .path()
                .join("missing projects")
                .as_os_str()
                .as_encoded_bytes()
        )
    );

    let caches = categories
        .iter()
        .find(|category| category["category"] == "caches")
        .unwrap();
    let cache_command = caches["preview_command"].as_str().unwrap();
    let copied_cache = Command::new("/bin/zsh")
        .args(["-f", "-c", &format!("{cache_command} --json")])
        .current_dir(&later_cwd)
        .env_clear()
        .env("HOME", sandbox.path())
        .env("PATH", sandbox.bin())
        .env("DEVTRIM_BIN", env!("CARGO_BIN_EXE_devtrim"))
        .output()
        .unwrap();
    assert!(copied_cache.status.success());
    let ordinary_cache = run(&sandbox, &["clean", "caches", "--json"]);
    assert!(ordinary_cache.status.success());
    assert_eq!(
        json(&copied_cache)["findings"],
        json(&ordinary_cache)["findings"]
    );
    assert_eq!(json(&copied_cache)["findings"].as_array().unwrap().len(), 1);
    assert_eq!(
        std::fs::read(sandbox.path().join("copied-argv")).unwrap(),
        b"clean\0caches\0--json\0"
    );
    before
        .diff(
            &Tree::snapshot(sandbox.path()),
            &Allowed {
                scratch: vec![report, sandbox.path().join("copied-argv"), later_cwd],
                ..Allowed::default()
            },
        )
        .unwrap();
}

#[test]
fn offline_snapshot_maps_real_bytes_without_mutating_the_inventory() {
    let sandbox = fixture("dashboard-map");
    let mapped = sandbox.path().join("mapped");
    write(&mapped.join("project/output.bin"), "12345");
    write(&mapped.join("notes.txt"), "abc");
    std::fs::create_dir(mapped.join("empty")).unwrap();
    let before = Tree::snapshot(&mapped);
    let output = sandbox.path().join("report.html");
    let result = run(
        &sandbox,
        &[
            "dashboard",
            mapped.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--no-open",
            "--json",
        ],
    );
    let metadata = json(&result);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(metadata["operation"], "dashboard");
    assert_eq!(
        metadata["root"],
        mapped.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(metadata["inventory"]["bytes"], 8);
    assert!(
        metadata["inventory"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["name"] == "project" && entry["bytes"] == 5)
    );
    let document = std::fs::read_to_string(&output).unwrap();
    assert_eq!(embedded(&document)["inventory"], metadata["inventory"]);
    assert!(document.contains("Content-Security-Policy"));
    before
        .diff(&Tree::snapshot(&mapped), &Allowed::default())
        .unwrap();
}

#[test]
fn recommendations_use_category_owners_and_stay_inside_the_map() {
    let sandbox = fixture("dashboard-previews");
    let mapped = sandbox.path().join(".cache/huggingface");
    write(&mapped.join("hub/weights.bin"), "weights");
    write(&mapped.join("token"), "secret");
    write(&sandbox.path().join(".bun/install/cache/pkg"), "outside");
    let before = Tree::snapshot(&mapped);
    let output = sandbox.path().join("report.html");
    let result = run(
        &sandbox,
        &[
            "dashboard",
            mapped.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--no-open",
            "--json",
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    let metadata = json(&result);
    assert_eq!(metadata["inventory"]["bytes"], 13);
    let categories = metadata["recommendations"]["categories"]
        .as_array()
        .unwrap();
    let caches = categories
        .iter()
        .find(|category| category["category"] == "caches")
        .unwrap();
    assert_eq!(caches["preview_command"], "devtrim clean caches");
    let findings = caches["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(
        findings[0]["path"],
        mapped.join("hub").canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(findings[0]["bytes"], 7);
    assert_eq!(findings[0]["kind"], "preview");
    assert!(!metadata["recommendations"].to_string().contains("--apply"));
    before
        .diff(&Tree::snapshot(&mapped), &Allowed::default())
        .unwrap();

    sandbox.script("xcrun", "printf 'probe refused\\n' >&2; exit 4");
    let failed_output = sandbox.path().join("partial.html");
    let partial = run(
        &sandbox,
        &[
            "dashboard",
            mapped.to_str().unwrap(),
            "--output",
            failed_output.to_str().unwrap(),
            "--no-open",
            "--json",
        ],
    );
    assert!(!partial.status.success());
    let data = json(&partial);
    assert!(
        data["recommendations"]["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|error| error.as_str().unwrap().contains("simulators"))
    );
    assert_eq!(data["inventory"]["bytes"], 13);
    assert_eq!(data["coverage"]["partial"], false);
    assert_eq!(
        embedded(&std::fs::read_to_string(failed_output).unwrap())["errors"],
        data["errors"]
    );
}

#[test]
fn invalid_roots_and_parse_errors_do_not_create_reports() {
    let sandbox = fixture("dashboard-invalid");
    let mapped = sandbox.path().join("mapped");
    write(&mapped.join("keep"), "do not touch");
    let before = Tree::snapshot(sandbox.path());
    let unknown = run(&sandbox, &["dashboard", "--unknown", "--no-open", "--json"]);
    assert!(!unknown.status.success());
    assert_eq!(json(&unknown)["operation"], "dashboard");
    for root in [mapped.join("missing"), mapped.join("keep")] {
        let result = run(
            &sandbox,
            &["dashboard", root.to_str().unwrap(), "--no-open", "--json"],
        );
        assert!(!result.status.success());
        assert_eq!(json(&result)["operation"], "dashboard");
    }
    before
        .diff(&Tree::snapshot(sandbox.path()), &Allowed::default())
        .unwrap();
}

fn output_preservation_control(linked: bool, marker: &str) {
    let sandbox = fixture("dashboard-output-guard");
    let mapped = sandbox.path().join("mapped");
    write(&mapped.join("keep"), "known");
    let control_path = sandbox.path().join("eligible.html");
    let control = run(
        &sandbox,
        &[
            "dashboard",
            mapped.to_str().unwrap(),
            "--output",
            control_path.to_str().unwrap(),
            "--no-open",
            "--json",
        ],
    );
    assert!(control.status.success());
    assert_eq!(
        embedded(&std::fs::read_to_string(&control_path).unwrap())["inventory"]["bytes"],
        5
    );
    let victim = sandbox.path().join("original.html");
    write(&victim, "original bytes must survive");
    let output = if linked {
        let link = sandbox.path().join("linked.html");
        symlink(&victim, &link).unwrap();
        link
    } else {
        victim.clone()
    };
    sandbox.script("xcrun", "printf called > \"$HOME/probe-called\"; exit 4");
    let before = Tree::snapshot(sandbox.path());
    let result = run(
        &sandbox,
        &[
            "dashboard",
            mapped.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--no-open",
            "--json",
        ],
    );
    assert_eq!(
        std::fs::read_to_string(&victim).unwrap(),
        "original bytes must survive",
        "{marker}"
    );
    assert!(!sandbox.path().join("probe-called").exists(), "{marker}");
    assert!(!result.status.success(), "{marker}");
    assert_eq!(json(&result)["operation"], "dashboard");
    before
        .diff(&Tree::snapshot(sandbox.path()), &Allowed::default())
        .unwrap();
}

#[test]
fn existing_output_is_preserved_without_scanning() {
    output_preservation_control(false, "PV dashboard/existing-output");
}

#[test]
fn symlink_output_is_preserved_without_scanning() {
    output_preservation_control(true, "PV dashboard/symlink-output");
}

#[test]
fn dashboard_mutation_flags_refuse_before_any_work() {
    let sandbox = fixture("dashboard-flags");
    let mapped = sandbox.path().join("mapped");
    write(&mapped.join("keep"), "known");
    let control_path = sandbox.path().join("eligible.html");
    let control = run(
        &sandbox,
        &[
            "dashboard",
            mapped.to_str().unwrap(),
            "--output",
            control_path.to_str().unwrap(),
            "--no-open",
            "--json",
        ],
    );
    assert!(control.status.success());
    assert_eq!(
        embedded(&std::fs::read_to_string(&control_path).unwrap())["inventory"]["bytes"],
        5
    );
    let before = Tree::snapshot(sandbox.path());
    for (index, flag) in ["--apply", "--yes", "-y", "--yolo", "--shred"]
        .iter()
        .enumerate()
    {
        let forbidden_output = sandbox.path().join(format!("forbidden-{index}.html"));
        let result = run(
            &sandbox,
            &[
                "dashboard",
                mapped.to_str().unwrap(),
                flag,
                "--output",
                forbidden_output.to_str().unwrap(),
                "--no-open",
                "--json",
            ],
        );
        assert!(
            !forbidden_output.exists(),
            "PV dashboard/mutation-flags: {flag} must refuse before report creation"
        );
        assert!(
            !result.status.success(),
            "PV dashboard/mutation-flags: {flag}"
        );
        assert_eq!(json(&result)["operation"], "dashboard");
    }
    before
        .diff(&Tree::snapshot(sandbox.path()), &Allowed::default())
        .unwrap();
}

#[test]
fn partial_inventory_keeps_readable_bytes_and_does_not_follow_symlinks() {
    assert!(
        !rustix::process::geteuid().is_root(),
        "run this coverage test as an ordinary user"
    );
    let sandbox = fixture("dashboard-partial");
    let mapped = sandbox.path().join("mapped");
    write(&mapped.join("visible"), "1234");
    write(&mapped.join("locked/hidden"), "not measurable");
    write(&sandbox.path().join("outside/large"), &"x".repeat(8192));
    symlink("../outside", mapped.join("linked")).unwrap();
    let before = Tree::snapshot(&mapped);
    std::fs::set_permissions(
        mapped.join("locked"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    let output = sandbox.path().join("partial.html");
    let result = run(
        &sandbox,
        &[
            "dashboard",
            mapped.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--no-open",
            "--json",
        ],
    );
    // Restore before assertions and the filesystem oracle, including on failure.
    std::fs::set_permissions(
        mapped.join("locked"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    assert!(!result.status.success());
    let data = json(&result);
    assert_eq!(data["coverage"]["partial"], true);
    assert_eq!(data["inventory"]["bytes"], 14);
    assert!(data["coverage"]["errors"].to_string().contains("locked"));
    let children = data["inventory"]["children"].as_array().unwrap();
    assert!(
        children
            .iter()
            .any(|entry| entry["name"] == "locked" && entry["partial"] == true)
    );
    assert!(children.iter().any(|entry| entry["name"] == "linked"
        && entry["kind"] == "symlink"
        && entry["bytes"] == 10));
    assert_eq!(
        embedded(&std::fs::read_to_string(output).unwrap())["coverage"]["partial"],
        true
    );
    before
        .diff(&Tree::snapshot(&mapped), &Allowed::default())
        .unwrap();
}

#[test]
fn opening_is_explicit_bounded_and_failure_preserves_the_private_report() {
    let sandbox = fixture("dashboard-open");
    let mapped = sandbox.path().join("mapped");
    write(&mapped.join("a"), "a");
    sandbox.script("open", "printf '%s\\n' \"$#\" \"$1\" > \"$HOME/open-argv\"");
    let no_open = sandbox.path().join("closed.html");
    let result = run(
        &sandbox,
        &[
            "dashboard",
            mapped.to_str().unwrap(),
            "--output",
            no_open.to_str().unwrap(),
            "--no-open",
            "--json",
        ],
    );
    assert!(result.status.success());
    assert!(!sandbox.path().join("open-argv").exists());
    let opened = sandbox.path().join("opened.html");
    let result = run(
        &sandbox,
        &[
            "dashboard",
            mapped.to_str().unwrap(),
            "--output",
            opened.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    assert_eq!(
        std::fs::read_to_string(sandbox.path().join("open-argv")).unwrap(),
        format!("1\n{}\n", opened.canonicalize().unwrap().display())
    );
    sandbox.script("open", "printf 'browser unavailable\\n' >&2; exit 9");
    let failed = sandbox.path().join("failed.html");
    let result = run(
        &sandbox,
        &[
            "dashboard",
            mapped.to_str().unwrap(),
            "--output",
            failed.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(!result.status.success());
    let data = json(&result);
    assert_eq!(
        data["output_file"],
        failed.canonicalize().unwrap().to_str().unwrap()
    );
    assert!(data["errors"].to_string().contains("browser unavailable"));
    assert!(
        std::fs::read_to_string(&failed)
            .unwrap()
            .contains("Disk explorer")
    );
    assert_eq!(
        std::fs::metadata(&failed).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let temporary = sandbox.path().join("temporary");
    std::fs::create_dir(&temporary).unwrap();
    let snapshot_root = sandbox.path().canonicalize().unwrap();
    let before = Tree::snapshot(&snapshot_root);
    let result = Command::new(env!("CARGO_BIN_EXE_devtrim"))
        .args(["dashboard", "--no-open", "--json"])
        .env_clear()
        .env("HOME", sandbox.path())
        .env("PATH", sandbox.bin())
        .env("TMPDIR", &temporary)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    let data = json(&result);
    let default_file = std::path::PathBuf::from(data["output_file"].as_str().unwrap());
    assert!(default_file.starts_with(temporary.canonicalize().unwrap()));
    assert_eq!(
        data["root"],
        sandbox.path().canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(
        std::fs::metadata(&default_file)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    before
        .diff(
            &Tree::snapshot(&snapshot_root),
            &Allowed {
                scratch: vec![default_file],
                ..Allowed::default()
            },
        )
        .unwrap();
}

#[test]
fn hostile_paths_remain_inert_data_and_human_errors_are_escaped() {
    let sandbox = fixture("dashboard-hostile");
    let mapped = sandbox.path().join("mapped");
    let hostile = mapped
        .join("<")
        .join("script><script>globalThis.injected=1<")
        .join("script>&\"\u{2028}\u{2029}\u{202e}");
    write(&hostile, "payload");
    let output = sandbox.path().join("report.html");
    let result = run(
        &sandbox,
        &[
            "dashboard",
            mapped.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--no-open",
            "--json",
        ],
    );
    assert!(result.status.success());
    let document = std::fs::read_to_string(output).unwrap();
    assert!(!document.contains("<script>globalThis.injected=1</script>"));
    assert!(document.contains("\\u003c/script\\u003e"));
    assert!(document.contains("\\u2028") && document.contains("\\u2029"));
    let data = embedded(&document);
    let leaf = &data["inventory"]["children"][0]["children"][0]["children"][0];
    assert_eq!(
        leaf["path"],
        hostile.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(leaf["bytes"], 7);
    let bad_output = sandbox
        .path()
        .join("missing\u{1b}\u{202e}")
        .join("report.html");
    let result = run(
        &sandbox,
        &[
            "dashboard",
            mapped.to_str().unwrap(),
            "--output",
            bad_output.to_str().unwrap(),
            "--no-open",
        ],
    );
    assert!(!result.status.success());
    let error = String::from_utf8(result.stderr).unwrap();
    assert!(!error.contains('\u{1b}') && !error.contains('\u{202e}'));
    assert!(error.contains("dashboard") && error.contains("missing"));
}

#[test]
fn depth_and_node_budgets_summarize_detail_without_losing_bytes() {
    let sandbox = fixture("dashboard-bounds");
    let mapped = sandbox.path().join("mapped");
    write(&mapped.join("a/b/c/d/e/f/g/h/i/deep"), "1234567");
    write(&mapped.join("shallow"), "abc");
    let output = sandbox.path().join("depth.html");
    let result = run(
        &sandbox,
        &[
            "dashboard",
            mapped.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--no-open",
            "--json",
        ],
    );
    assert!(result.status.success());
    let data = json(&result);
    assert_eq!(data["inventory"]["bytes"], 10);
    assert_eq!(data["coverage"]["partial"], false);
    assert_eq!(data["coverage"]["summarized_entries"], 4);
    let mut directory = &data["inventory"];
    for _ in 0..6 {
        directory = &directory["children"][0];
    }
    assert_eq!(directory["summarized_bytes"], 7);
    assert_eq!(directory["children"].as_array().unwrap().len(), 0);

    let wide = sandbox.path().join("wide");
    std::fs::create_dir(&wide).unwrap();
    for index in 0..10_010 {
        std::fs::write(wide.join(format!("entry-{index:05}")), "").unwrap();
    }
    write(&wide.join("z-last"), &"x".repeat(131_072));
    let output = sandbox.path().join("wide.html");
    let result = run(
        &sandbox,
        &[
            "dashboard",
            wide.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--no-open",
            "--json",
        ],
    );
    assert!(result.status.success());
    let data = json(&result);
    assert_eq!(data["inventory"]["bytes"], 131_072);
    assert!(data["coverage"]["retained_nodes"].as_u64().unwrap() <= 10_000);
    assert!(data["coverage"]["summarized_entries"].as_u64().unwrap() > 0);
    assert_eq!(data["coverage"]["partial"], false);

    let crowded = wide.join("a-crowded");
    std::fs::create_dir(&crowded).unwrap();
    for index in 0..10_010 {
        let name = format!("entry-{index:05}");
        std::fs::rename(wide.join(&name), crowded.join(&name)).unwrap();
    }
    std::fs::create_dir(wide.join("z-important")).unwrap();
    std::fs::rename(wide.join("z-last"), wide.join("z-important/last")).unwrap();
    let output = sandbox.path().join("siblings.html");
    let result = run(
        &sandbox,
        &[
            "dashboard",
            wide.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--no-open",
            "--json",
        ],
    );
    assert!(result.status.success());
    let data = json(&result);
    assert_eq!(data["inventory"]["bytes"], 131_072);
    assert!(data["coverage"]["retained_nodes"].as_u64().unwrap() <= 10_000);
    assert!(
        data["inventory"]["children"]
            .as_array()
            .unwrap()
            .iter()
            .any(|node| node["name"] == "z-important" && node["bytes"] == 131_072),
        "a crowded earlier folder must not consume the top-level overview"
    );
}

#[test]
fn creating_a_dashboard_never_rotates_an_existing_apply_journal() {
    let sandbox = fixture("dashboard-journal");
    let mapped = sandbox.path().join("mapped");
    write(&mapped.join("a"), "a");
    write(&sandbox.journal(), "audit evidence");
    std::fs::OpenOptions::new()
        .write(true)
        .open(sandbox.journal())
        .unwrap()
        .set_len(10 * 1024 * 1024 + 1)
        .unwrap();
    let journal_parent = sandbox.journal().parent().unwrap().to_path_buf();
    let before = Tree::snapshot(&journal_parent);
    let output = sandbox.path().join("report.html");
    let result = run(
        &sandbox,
        &[
            "dashboard",
            mapped.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--no-open",
            "--json",
        ],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
    assert_eq!(json(&result)["inventory"]["bytes"], 1);
    before
        .diff(&Tree::snapshot(&journal_parent), &Allowed::default())
        .unwrap();
}

#[test]
fn skipped_default_root_warning_is_kept_in_the_offline_report() {
    let sandbox = fixture("dashboard-root-warning");
    let mapped = sandbox.path().join("mapped");
    write(&mapped.join("data"), "known");
    symlink(sandbox.path(), sandbox.path().join("dev")).unwrap();
    let output = sandbox.path().join("report.html");
    let result = run(
        &sandbox,
        &[
            "dashboard",
            mapped.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--no-open",
            "--json",
        ],
    );
    assert!(result.status.success());
    let data = json(&result);
    assert_eq!(data["inventory"]["bytes"], 5);
    let warnings = data["recommendations"]["diagnostics"].as_array().unwrap();
    assert!(
        warnings.iter().any(|warning| warning
            .as_str()
            .unwrap()
            .contains("default project folder skipped")),
        "construction warnings must remain visible in the saved report"
    );
    assert_eq!(
        embedded(&std::fs::read_to_string(output).unwrap())["recommendations"]["diagnostics"],
        data["recommendations"]["diagnostics"]
    );
    assert!(result.stderr.is_empty());
}
