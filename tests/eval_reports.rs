//! Fixture-derived report contracts through the real binary, with a complete
//! before/after filesystem oracle. External metrics use isolated PATH stubs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions fail by panicking"
)]

mod support;

use serde_json::Value;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::PathBuf;
use std::process::Output;
use std::time::{Duration, SystemTime};
use support::{Allowed, Sandbox, Tree, json, run, write};

fn sandbox(name: &str) -> Sandbox {
    Sandbox::in_target(name)
}

fn unchanged(before: &Tree, sandbox: &Sandbox) {
    before
        .diff(&Tree::snapshot(sandbox.path()), &Allowed::default())
        .unwrap();
}

fn successful(output: &Output) {
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn findings(document: &Value) -> Vec<(PathBuf, u64)> {
    document["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| {
            (
                PathBuf::from(finding["path"].as_str().unwrap()),
                finding["size_bytes"].as_u64().unwrap(),
            )
        })
        .collect()
}

#[test]
fn eval_largest_ranks_fixture_totals_and_honors_top() {
    let sandbox = sandbox("report-largest");
    let root = sandbox.path().join("work");
    write(&root.join("large/deep/blob"), &"x".repeat(73));
    write(&root.join("large/direct"), &"x".repeat(29));
    write(&root.join("small/blob"), &"x".repeat(11));
    symlink(root.join("large"), root.join("alias")).unwrap();
    let before = Tree::snapshot(sandbox.path());
    let output = run(
        &sandbox,
        &[
            "largest",
            "--root",
            root.to_str().unwrap(),
            "--top",
            "2",
            "--json",
        ],
    );
    let document = json(&output);
    assert_eq!(
        findings(&document),
        vec![(root.join("large"), 102), (root.join("large/deep"), 73)],
        "PV eval/largest-ranking"
    );
    successful(&output);
    let one = run(
        &sandbox,
        &[
            "largest",
            "--root",
            root.to_str().unwrap(),
            "--top",
            "0",
            "--json",
        ],
    );
    assert_eq!(findings(&json(&one)), vec![(root.join("large"), 102)]);
    successful(&one);
    unchanged(&before, &sandbox);
}

#[test]
fn eval_icloud_reports_large_logical_files_with_local_allocation() {
    let sandbox = sandbox("report-icloud");
    let root = sandbox
        .path()
        .join("Library/Mobile Documents/com~apple~CloudDocs");
    write(&root.join("large.dat"), "allocated");
    let large = root.join("large.dat");
    std::fs::File::options()
        .write(true)
        .open(&large)
        .unwrap()
        .set_len(100 * 1024 * 1024)
        .unwrap();
    write(&root.join("small.dat"), "small");
    symlink(&large, root.join("linked.dat")).unwrap();
    let allocation = std::fs::metadata(&large).unwrap().blocks() * 512;
    let before = Tree::snapshot(sandbox.path());
    let output = run(&sandbox, &["icloud", "--json"]);
    let document = json(&output);
    assert_eq!(
        findings(&document),
        vec![(large, 100 * 1024 * 1024)],
        "PV eval/icloud-threshold"
    );
    let reported_allocation = document["findings"][0]["note"]
        .as_str()
        .unwrap()
        .split_whitespace()
        .find_map(|word| word.parse::<u64>().ok())
        .unwrap();
    assert_eq!(reported_allocation, allocation);
    assert_eq!(document["findings"][0]["action"]["type"], "info");
    successful(&output);
    unchanged(&before, &sandbox);
}

#[test]
fn eval_leftovers_reports_exact_hints_and_sizes() {
    let sandbox = sandbox("report-leftovers");
    let root = sandbox.path().join("work");
    write(&root.join("codex-worktree.aB1234/blob"), &"x".repeat(31));
    write(
        &root.join("project/.supergoal/evidence/blob"),
        &"x".repeat(17),
    );
    write(&root.join("project/.supergoal/perf/blob"), &"x".repeat(7));
    write(&root.join("codex-worktree.short/blob"), "near miss");
    write(&root.join("ordinary.aB1234/blob"), "near miss");
    let before = Tree::snapshot(sandbox.path());
    let output = run(
        &sandbox,
        &[
            "clean",
            "leftovers",
            "--root",
            root.to_str().unwrap(),
            "--json",
        ],
    );
    let mut actual = findings(&json(&output));
    actual.sort();
    let mut expected = vec![
        (root.join("codex-worktree.aB1234"), 31),
        (root.join("project/.supergoal/evidence"), 17),
        (root.join("project/.supergoal/perf"), 7),
    ];
    expected.sort();
    assert_eq!(actual, expected, "PV eval/leftovers-hints");
    successful(&output);
    unchanged(&before, &sandbox);
}

#[test]
fn eval_uninstall_attributes_only_exact_bundle_identifier() {
    let sandbox = sandbox("report-uninstall");
    let app = sandbox.path().join("Applications/EvalUniqueWidget.app");
    write(
        &app.join("Contents/Info.plist"),
        "fixture plist read by isolated plutil",
    );
    sandbox.script("plutil", "printf 'com.devtrim.evalwidget\\n'");
    sandbox.script("ps", "exit 0");
    let support = sandbox
        .path()
        .join("Library/Application Support/com.devtrim.evalwidget");
    let prefs = sandbox
        .path()
        .join("Library/Preferences/com.devtrim.evalwidget.plist");
    write(&support.join("blob"), "owned");
    write(&prefs, "prefs");
    write(
        &sandbox
            .path()
            .join("Library/Application Support/com.devtrim.evalwidget-extra/blob"),
        "neighbor",
    );
    write(
        &sandbox
            .path()
            .join("Library/Group Containers/group.com.devtrim.evalwidget/blob"),
        "shared",
    );
    let before = Tree::snapshot(sandbox.path());
    let output = run(&sandbox, &["uninstall", app.to_str().unwrap(), "--json"]);
    let mut actual = findings(&json(&output));
    actual.sort();
    let mut expected = vec![
        (app, "fixture plist read by isolated plutil".len() as u64),
        (support, 5),
        (prefs, 5),
    ];
    expected.sort();
    assert_eq!(actual, expected, "PV eval/uninstall-identifier");
    successful(&output);
    unchanged(&before, &sandbox);
}

#[test]
fn eval_history_limit_selects_newest_results_across_rotation() {
    let sandbox = sandbox("report-history");
    write(
        &sandbox.path().join(".config/devtrim.toml"),
        "malformed = [\n",
    );
    let config_refusal = run(&sandbox, &["clean", "caches", "--json"]);
    assert!(
        !config_refusal.status.success(),
        "the malformed configuration control must fail"
    );
    let record = |timestamp: u64, target: &str| {
        format!(
            "{{\"ts\":{timestamp},\"phase\":\"result\",\"op\":\"caches\",\"action\":\"trash\",\"target\":\"{target}\",\"size_bytes\":{timestamp},\"status\":\"ok\"}}\n"
        )
    };
    write(
        &sandbox.journal().with_extension("jsonl.1"),
        &record(1, "/fixture/old"),
    );
    write(
        &sandbox.journal(),
        &(record(2, "/fixture/middle") + &record(3, "/fixture/new")),
    );
    let before = Tree::snapshot(sandbox.path());
    let output = run(&sandbox, &["history", "--limit", "2", "--json"]);
    let document = json(&output);
    let targets: Vec<_> = document["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["target"].as_str().unwrap())
        .collect();
    assert_eq!(
        targets,
        vec!["/fixture/new", "/fixture/middle"],
        "PV eval/history-limit"
    );
    successful(&output);
    let all = run(&sandbox, &["history", "--limit", "3", "--json"]);
    assert_eq!(json(&all)["entries"][2]["target"], "/fixture/old");
    successful(&all);
    unchanged(&before, &sandbox);
}

#[test]
fn eval_analyze_measures_children_and_discloses_partial_lower_bounds() {
    let sandbox = sandbox("report-analyze");
    let root = sandbox.path().join("browse");
    write(&root.join("folder/visible"), &"x".repeat(23));
    write(&root.join("folder/locked/hidden"), &"x".repeat(47));
    write(&root.join("direct"), &"x".repeat(13));
    symlink("folder", root.join("alias")).unwrap();
    let before = Tree::snapshot(sandbox.path());
    let full = run(&sandbox, &["analyze", root.to_str().unwrap(), "--json"]);
    assert_eq!(
        findings(&json(&full)),
        vec![
            (root.join("folder"), 70),
            (root.join("direct"), 13),
            (root.join("alias"), 6)
        ]
    );
    successful(&full);
    let locked = root.join("folder/locked");
    let permissions = std::fs::metadata(&locked).unwrap().permissions();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o0)).unwrap();
    let inaccessible = std::fs::read_dir(&locked).is_err();
    let partial = run(&sandbox, &["analyze", root.to_str().unwrap(), "--json"]);
    std::fs::set_permissions(&locked, permissions).unwrap();
    assert!(
        inaccessible,
        "this eval requires an unprivileged user to prove partial measurement"
    );
    let document = json(&partial);
    assert_eq!(
        findings(&document),
        vec![
            (root.join("folder"), 23),
            (root.join("direct"), 13),
            (root.join("alias"), 6)
        ],
        "PV eval/analyze-partial"
    );
    assert!(!partial.status.success(), "PV eval/analyze-partial");
    assert!(
        !document["errors"].as_array().unwrap().is_empty(),
        "PV eval/analyze-partial"
    );
    assert!(
        document["findings"][0]["note"]
            .as_str()
            .unwrap()
            .contains("lower bound")
    );
    unchanged(&before, &sandbox);
}

#[test]
fn eval_status_uses_resident_memory_and_names_unavailable_metrics() {
    let sandbox = sandbox("report-status");
    sandbox.script("sysctl", "case \"$*\" in *hw.memsize*) printf '1048576\\n';; *hw.logicalcpu*) printf '4\\n';; *vm.loadavg*) printf '{ 1.25 0.75 0.5 }\\n';; *kern.boottime*) printf '{ sec = 1700000000, usec = 0 }\\n';; *) exit 1;; esac");
    sandbox.script("vm_stat", "printf 'Mach Virtual Memory Statistics: (page size of 4096 bytes)\\nPages free: 10.\\nPages speculative: 2.\\nPages active: 20.\\nPages inactive: 80.\\nPages wired down: 5.\\nPages occupied by compressor: 3.\\n'");
    sandbox.script("df", "printf 'Filesystem 1024-blocks Used Available Capacity Mounted on\\n/dev/eval 1000 250 750 25%% /\\n'");
    sandbox.script("pmset", "case \"$*\" in *batt*) printf \"Now drawing from 'AC Power'\\n -InternalBattery-0 85%%; charging;\\n\";; *therm*) printf 'CPU_Speed_Limit = 100\\n';; *) exit 1;; esac");
    sandbox.script("netstat", "printf 'Name Mtu Network Address Ipkts Ierrs Ibytes Opkts Oerrs Obytes Coll\\nen0 1500 <Link#1> aa:bb 1 0 1234 2 0 5678 0\\n'");
    sandbox.script(
        "ps",
        "printf 'PID %%CPU RSS COMM\\n321 12.5 128 FixtureWorker\\n'",
    );
    let before = Tree::snapshot(sandbox.path());
    let output = run(&sandbox, &["status", "--json"]);
    let document = json(&output);
    assert_eq!(
        document["memory"]["used_bytes"], 114688,
        "PV eval/status-memory"
    );
    assert_eq!(document["memory"]["inactive_bytes"], 327680);
    assert_eq!(document["memory"]["free_bytes"], 49152);
    assert_eq!(document["disk"]["available_bytes"], 768000);
    assert_eq!(
        document["load_average"],
        serde_json::json!([1.25, 0.75, 0.5])
    );
    assert_eq!(document["network"]["received_bytes"], 1234);
    assert_eq!(document["network"]["sent_bytes"], 5678);
    assert_eq!(document["top_processes"][0]["resident_bytes"], 131072);
    assert_eq!(document["battery"]["percent"], 85);
    successful(&output);
    unchanged(&before, &sandbox);
    sandbox.script("vm_stat", "printf 'fixture unavailable\\n' >&2; exit 7");
    let before = Tree::snapshot(sandbox.path());
    let unavailable = run(&sandbox, &["status", "--json"]);
    let document = json(&unavailable);
    assert!(
        document["memory"].is_null() && !unavailable.status.success(),
        "PV eval/status-unavailable"
    );
    assert!(
        document["unavailable"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason.as_str().unwrap().contains("fixture unavailable")),
        "PV eval/status-unavailable"
    );
    assert!(
        document["health"]["missing_inputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|input| input == "memory")
    );
    assert_eq!(document["network"]["received_bytes"], 1234);
    unchanged(&before, &sandbox);
}

#[test]
fn eval_generated_docs_expose_commands_and_refuse_json() {
    let sandbox = sandbox("report-generated");
    write(
        &sandbox.path().join(".config/devtrim.toml"),
        "malformed = [\n",
    );
    let config_refusal = run(&sandbox, &["clean", "caches", "--json"]);
    assert!(
        !config_refusal.status.success(),
        "the malformed configuration control must fail"
    );
    let before = Tree::snapshot(sandbox.path());
    for args in [
        vec!["completions", "bash"],
        vec!["completions", "fish"],
        vec!["manpage"],
    ] {
        let output = run(&sandbox, &args);
        successful(&output);
        let text = String::from_utf8(output.stdout)
            .unwrap()
            .replace("\\-", "-");
        for command in ["analyze", "trash-empty", "history", "uninstall", "optimize"] {
            assert!(
                text.contains(command),
                "PV eval/generated-commands: {args:?} lacks {command}"
            );
        }
        assert!(text.contains("devtrim"));
        let mut json_args = args.clone();
        json_args.push("--json");
        let refused = run(&sandbox, &json_args);
        assert!(!refused.status.success());
        assert!(!json(&refused)["errors"].as_array().unwrap().is_empty());
    }
    unchanged(&before, &sandbox);
}

#[test]
fn eval_scan_human_summarizes_large_categories_and_all_lists_every_target() {
    let sandbox = sandbox("report-scan");
    let root = sandbox.path().join("work");
    std::fs::create_dir_all(&root).unwrap();
    for index in 0..10 {
        let path = sandbox
            .path()
            .join(format!("Downloads/eval-installer-{index}.dmg"));
        write(&path, &"x".repeat((index + 1) * 17));
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(90 * 86400))
            .unwrap();
    }
    let before = Tree::snapshot(sandbox.path());
    let summarized = run(&sandbox, &["scan", "--root", root.to_str().unwrap()]);
    successful(&summarized);
    let summarized = String::from_utf8(summarized.stdout).unwrap();
    assert!(summarized.contains("devtrim clean installers --apply"));
    for index in 0..10 {
        assert_eq!(
            summarized.contains(&format!("eval-installer-{index}.dmg")),
            index >= 5,
            "PV eval/scan-summary"
        );
    }
    let all = run(
        &sandbox,
        &["scan", "--root", root.to_str().unwrap(), "--all"],
    );
    successful(&all);
    let all = String::from_utf8(all.stdout).unwrap();
    for index in 0..10 {
        assert!(
            all.contains(&format!("eval-installer-{index}.dmg")),
            "PV eval/scan-all"
        );
    }
    unchanged(&before, &sandbox);
}

#[test]
fn eval_optimize_selected_preview_contains_only_named_task() {
    let sandbox = sandbox("report-optimize");
    let before = Tree::snapshot(sandbox.path());
    let output = run(&sandbox, &["optimize", "--task", "quicklook", "--json"]);
    let document = json(&output);
    assert_eq!(document["findings"].as_array().unwrap().len(), 1);
    assert_eq!(
        document["findings"][0]["action"],
        serde_json::json!({"type": "command", "program": "qlmanage", "args": ["-r", "cache"]})
    );
    successful(&output);
    unchanged(&before, &sandbox);
}
