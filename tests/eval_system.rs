//! System-category evals: toolchains, installers, agents, simulators, docker,
//! optimize and trash-empty, plus the flag-capability table, each run through
//! the real binary against a planted fixture. Every eval plants the positives
//! its category claims to act on and one near miss per documented exclusion,
//! then proves the preview changed nothing, the plan is exactly the positives,
//! and the apply removed (or ran) exactly that and nothing else.
//!
//! Command categories (simulators, docker, optimize) run stub tools that
//! append their full argv to `$HOME/stub/calls.log`, one argument per line and
//! `--` after each call, so the evals can assert the exact argv sequence and
//! that no forbidden command ever ran.
//!
//! Assertion order matters to the planted-violation gate: every `PV eval/...`
//! assertion that a mutant must trip comes before any untagged one.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions fail by panicking"
)]

mod support;

use serde_json::Value;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use support::{
    Allowed, Interactive, Sandbox, Tree, actionable_targets, assert_journaled, journal_lines, json,
    run, write,
};

const DAY: u64 = 60 * 60 * 24;

/// Set a regular file's modification time to `days` days ago.
fn age(path: &Path, days: u64) {
    let when = SystemTime::now() - Duration::from_secs(days * DAY);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

fn write_aged(path: &Path, contents: &str, days: u64) {
    write(path, contents);
    age(path, days);
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_string()).collect()
}

fn sorted(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.sort();
    paths
}

/// Install a stub that appends its full argv to `$HOME/stub/calls.log` (program
/// name first, then one argument per line, then `--`) and runs `body`. The
/// stub folder exists from the start so a stub that ran is visible as a log.
fn stub(sandbox: &Sandbox, name: &str, body: &str) {
    std::fs::create_dir_all(sandbox.path().join("stub")).unwrap();
    let record = r#"log="$HOME/stub/calls.log"
printf '%s\n' 'NAME' >> "$log"
for argument in "$@"; do printf '%s\n' "$argument" >> "$log"; done
printf -- '--\n' >> "$log"
"#
    .replace("NAME", name);
    sandbox.script(name, &format!("{record}{body}"));
}

/// Every stubbed call so far, each as `[program, args...]`.
fn calls(sandbox: &Sandbox) -> Vec<Vec<String>> {
    let text = std::fs::read_to_string(sandbox.path().join("stub/calls.log")).unwrap_or_default();
    let mut calls = Vec::new();
    let mut current = Vec::new();
    for line in text.lines() {
        if line == "--" {
            calls.push(std::mem::take(&mut current));
        } else {
            current.push(line.to_string());
        }
    }
    calls
}

/// `[program, args...]` of every command finding, in plan order.
fn command_plan(document: &Value) -> Vec<Vec<String>> {
    document["findings"]
        .as_array()
        .unwrap_or_else(|| panic!("no findings array: {document}"))
        .iter()
        .filter(|finding| finding["action"]["type"] == "command")
        .map(|finding| {
            let mut command = vec![finding["action"]["program"].as_str().unwrap().to_string()];
            command.extend(
                finding["action"]["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|argument| argument.as_str().unwrap().to_string()),
            );
            command
        })
        .collect()
}

/// `(label, action type)` of every finding, in plan order.
fn labelled_actions(document: &Value) -> Vec<(String, String)> {
    document["findings"]
        .as_array()
        .unwrap_or_else(|| panic!("no findings array: {document}"))
        .iter()
        .map(|finding| {
            (
                finding["label"].as_str().unwrap().to_string(),
                finding["action"]["type"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn planned_paths(document: &Value) -> Vec<String> {
    document["findings"]
        .as_array()
        .map(|findings| {
            findings
                .iter()
                .filter_map(|finding| finding["path"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
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

/// Every command the journal recorded for `op`: write-ahead (an attempt, then
/// a later result with the same id and status `ok`), in order.
fn assert_journaled_commands(sandbox: &Sandbox, op: &str, expected: &[Vec<String>]) {
    let lines = journal_lines(sandbox);
    let mut seen = Vec::new();
    for (index, attempt) in lines.iter().enumerate() {
        if attempt.phase != "attempt" {
            continue;
        }
        let result = lines[index + 1..]
            .iter()
            .find(|line| line.phase == "result" && line.id == attempt.id)
            .unwrap_or_else(|| panic!("EVAL journal: attempt without a result: {attempt:?}"));
        assert_eq!(attempt.op, op, "EVAL journal: {attempt:?}");
        assert_eq!(attempt.action, "command", "EVAL journal: {attempt:?}");
        assert_eq!(
            result.status.as_deref(),
            Some("ok"),
            "EVAL journal: {result:?}"
        );
        seen.push(attempt.argv.clone().unwrap());
    }
    assert_eq!(
        seen, expected,
        "EVAL journal: journaled commands differ from the plan"
    );
}

fn assert_no_attempts(sandbox: &Sandbox) {
    let attempts: Vec<_> = journal_lines(sandbox)
        .into_iter()
        .filter(|line| line.phase == "attempt")
        .collect();
    assert!(
        attempts.is_empty(),
        "EVAL journal: a refused apply journaled {attempts:?}"
    );
}

/// Run an interactive apply, change the fixture while devtrim waits for the
/// typed confirmation, then give it. Returns exit status and terminal output.
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

// ---------- clean toolchains ----------

/// Two unreferenced swift.org toolchains (the positives), the toolchain
/// `swift-latest` points to, one a second symlink pins, and the near misses.
fn toolchains_fixture(sandbox: &Sandbox) -> Vec<PathBuf> {
    let directory = sandbox.path().join("Library/Developer/Toolchains");
    let stale: Vec<PathBuf> = [
        "swift-5.8-RELEASE.xctoolchain",
        "swift-5.7.3-RELEASE.xctoolchain",
    ]
    .iter()
    .map(|name| directory.join(name))
    .collect();
    for toolchain in &stale {
        write(&toolchain.join("usr/bin/swift"), "unreferenced toolchain");
    }
    // Kept: the toolchain `swift-latest` names, and one another link pins.
    write(
        &directory.join("swift-6.2-RELEASE.xctoolchain/usr/bin/swift"),
        "the latest toolchain",
    );
    symlink(
        "swift-6.2-RELEASE.xctoolchain",
        directory.join("swift-latest.xctoolchain"),
    )
    .unwrap();
    write(
        &directory.join("swift-6.0-RELEASE.xctoolchain/usr/bin/swift"),
        "a pinned toolchain",
    );
    symlink(
        "swift-6.0-RELEASE.xctoolchain",
        directory.join("pinned.xctoolchain"),
    )
    .unwrap();
    // Near misses, each named by the rule that keeps it.
    for relative in [
        "NotAToolchain/usr/bin/swift", // only `.xctoolchain` directories qualify
        "swift-5.9-RELEASE.xctoolchain.bak/usr/bin/swift", // the extension is exactly `xctoolchain`
        "Archive/swift-5.5-RELEASE.xctoolchain/usr/bin/swift", // direct children of Toolchains only
        "../Xcode/DerivedData/keep/Build", // anything outside the Toolchains folder
    ] {
        write(&directory.join(relative), "keep");
    }
    // A regular file with the right name is not a toolchain directory.
    write(&directory.join("stray.xctoolchain"), "keep");
    sorted(stale)
}

#[test]
fn eval_clean_toolchains_removes_only_unreferenced_toolchains() {
    let sandbox = Sandbox::in_target("eval-toolchains");
    let targets = toolchains_fixture(&sandbox);
    let pristine = Tree::snapshot(sandbox.path());

    let preview = run(&sandbox, &["clean", "toolchains", "--shred", "--json"]);
    let document = json(&preview);
    assert_eq!(
        actionable_targets(&document),
        targets,
        "PV eval/toolchains-plan: the preview must offer exactly the unreferenced toolchains"
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "PV eval/toolchains-preview",
    );
    assert!(preview.status.success(), "{document}");

    let apply = run(
        &sandbox,
        &[
            "clean",
            "toolchains",
            "--apply",
            "--shred",
            "--yolo",
            "--json",
        ],
    );
    let document = json(&apply);
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: targets.clone(),
            scratch: vec![sandbox.path().join(".local/state/devtrim")],
        },
        "PV eval/toolchains-apply",
    );
    assert!(apply.status.success(), "{document}");
    assert_eq!(document["summary"]["items_touched"], targets.len());
    assert_journaled(&sandbox, "toolchains", "shred", &targets);
}

/// Without a verifiable `swift-latest` nothing is offered and nothing removed:
/// the category fails closed rather than guessing which toolchain is in use.
#[test]
fn eval_clean_toolchains_fails_closed_without_a_verifiable_latest() {
    type Break = fn(&Path);
    let cases: [(&str, Break); 2] = [
        ("missing swift-latest", |directory| {
            std::fs::remove_file(directory.join("swift-latest.xctoolchain")).unwrap();
        }),
        // A link that leaves the Toolchains folder cannot be trusted to name
        // a toolchain this category owns.
        ("swift-latest escaping the folder", |directory| {
            let outside = directory.parent().unwrap().join("Outside.xctoolchain");
            write(&outside.join("usr/bin/swift"), "outside");
            std::fs::remove_file(directory.join("swift-latest.xctoolchain")).unwrap();
            symlink(&outside, directory.join("swift-latest.xctoolchain")).unwrap();
        }),
    ];
    for (name, break_latest) in cases {
        let sandbox = Sandbox::in_target("eval-toolchains-closed");
        toolchains_fixture(&sandbox);
        break_latest(&sandbox.path().join("Library/Developer/Toolchains"));
        let pristine = Tree::snapshot(sandbox.path());

        let preview = run(&sandbox, &["clean", "toolchains", "--shred", "--json"]);
        let document = json(&preview);
        assert!(
            planned_paths(&document).is_empty(),
            "PV eval/toolchains-closed-plan: {name} still produced a plan: {document}"
        );
        assert!(
            !preview.status.success(),
            "PV eval/toolchains-closed-plan: {name} must be a nonzero error: {document}"
        );
        let apply = run(
            &sandbox,
            &[
                "clean",
                "toolchains",
                "--apply",
                "--shred",
                "--yolo",
                "--json",
            ],
        );
        pristine.assert_only(
            &Tree::snapshot(sandbox.path()),
            &Allowed {
                scratch: vec![sandbox.path().join(".local/state/devtrim")],
                ..Allowed::default()
            },
            "PV eval/toolchains-closed-apply",
        );
        assert!(
            !apply.status.success(),
            "PV eval/toolchains-closed-apply: {name} must fail the apply"
        );
    }
}

/// Apply reasserts what the scan promised: a toolchain a link started naming
/// after the preview is refused, not removed.
#[test]
fn eval_toolchains_apply_refuses_a_toolchain_referenced_after_preview() {
    let sandbox = Sandbox::in_target("eval-toolchains-drift");
    let directory = sandbox.path().join("Library/Developer/Toolchains");
    let candidate = directory.join("swift-5.8-RELEASE.xctoolchain");
    write(&candidate.join("usr/bin/swift"), "previewed");
    write(
        &directory.join("swift-6.2-RELEASE.xctoolchain/usr/bin/swift"),
        "latest",
    );
    symlink(
        "swift-6.2-RELEASE.xctoolchain",
        directory.join("swift-latest.xctoolchain"),
    )
    .unwrap();
    let pristine = Tree::snapshot(sandbox.path());

    let (status, output) = drift_session(
        &sandbox,
        &["clean", "toolchains", "--apply", "--shred"],
        || {
            // After the preview, before consent: something pins the candidate.
            symlink(
                "swift-5.8-RELEASE.xctoolchain",
                directory.join("pinned.xctoolchain"),
            )
            .unwrap();
        },
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            scratch: vec![
                directory.join("pinned.xctoolchain"),
                sandbox.path().join(".local/state/devtrim"),
            ],
            ..Allowed::default()
        },
        "PV eval/toolchains-drift",
    );
    assert!(
        !status.success(),
        "the refusal must fail the run:\n{output}"
    );
    assert!(
        output.contains("became referenced after preview"),
        "{output}"
    );
}

// ---------- clean installers ----------

/// Installers the preview must offer, and the near misses kept. The config
/// gives a one-day project window and a ten-day retention window, so a
/// five-day-old archive proves retention (not the project window) decides.
fn installers_fixture(sandbox: &Sandbox) -> Vec<PathBuf> {
    let home = sandbox.path();
    write(
        &home.join(".config/devtrim.toml"),
        "active_days = 1\nretain_days = 10\n",
    );
    let downloads = home.join("Downloads");
    let desktop = home.join("Desktop");
    let positives = [
        (downloads.join("Tool.dmg"), 400),
        (downloads.join("Other.PKG"), 20), // matched ASCII-case-insensitively
        (downloads.join("old.mpkg"), 11),  // just past the ten-day retention
        (downloads.join("disk.iso"), 400),
        (downloads.join("Xcode.xip"), 400),
        (desktop.join("Setup.dmg"), 400),
    ];
    for (path, days) in &positives {
        write_aged(path, &"x".repeat(2048), *days);
    }
    // Near misses, each named by the rule that keeps it.
    for (relative, days) in [
        ("Downloads/Fresh.dmg", 0),     // presumed mid-install: inside retention
        ("Downloads/Recent.dmg", 5),    // older than active_days, inside retain_days
        ("Downloads/Nine.pkg", 9),      // one day short of retain_days
        ("Downloads/sources.zip", 400), // not on the closed extension list
        ("Downloads/backup.tar.gz", 400),
        ("Downloads/readme.txt", 400),
        ("Downloads/notes.dmg.txt", 400), // the extension is `txt`
        ("Downloads/dmg", 400),           // a name, not an extension
        ("Downloads/.dmg", 400),          // a dotfile has no extension
        ("Downloads/project/nested.dmg", 400), // direct children only
        ("Desktop/sub/deep.pkg", 400),
        ("Documents/old.dmg", 400), // only Downloads and Desktop
        ("Downloads/Folder.dmg/inner.bin", 400), // a directory is not an archive file
        ("outside/real.dmg", 400),  // the target of the symlink below
    ] {
        write_aged(&home.join(relative), &"x".repeat(2048), days);
    }
    // A symlink named like an installer is refused: following it would delete
    // outside the authorized folder.
    symlink("../outside/real.dmg", downloads.join("link.dmg")).unwrap();
    // A stale zero-byte archive frees nothing and is not offered.
    write_aged(&downloads.join("empty.dmg"), "", 400);
    sorted(positives.into_iter().map(|(path, _)| path).collect())
}

#[test]
fn eval_clean_installers_removes_only_stale_installer_files() {
    let sandbox = Sandbox::in_target("eval-installers");
    let targets = installers_fixture(&sandbox);
    let pristine = Tree::snapshot(sandbox.path());

    let preview = run(&sandbox, &["clean", "installers", "--shred", "--json"]);
    let document = json(&preview);
    assert_eq!(
        actionable_targets(&document),
        targets,
        "PV eval/installers-plan: the preview must offer exactly the stale installer files"
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "PV eval/installers-preview",
    );
    let tool = document["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["label"] == "installer archive: Tool.dmg")
        .expect("Tool.dmg is previewed");
    assert!(
        tool["note"]
            .as_str()
            .unwrap()
            .contains("untouched for 400 days"),
        "the preview must show the age: {tool}"
    );
    assert!(preview.status.success(), "{document}");

    let apply = run(
        &sandbox,
        &[
            "clean",
            "installers",
            "--apply",
            "--shred",
            "--yolo",
            "--json",
        ],
    );
    let document = json(&apply);
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: targets.clone(),
            scratch: vec![sandbox.path().join(".local/state/devtrim")],
        },
        "PV eval/installers-apply",
    );
    assert!(apply.status.success(), "{document}");
    assert_eq!(document["summary"]["items_touched"], targets.len());
    assert_journaled(&sandbox, "installers", "shred", &targets);
}

/// Apply re-reads the age: an archive modified in place after the preview keeps
/// its inode, so only the age recheck can see it is no longer stale.
#[test]
fn eval_installers_apply_refuses_an_archive_modified_after_preview() {
    let sandbox = Sandbox::in_target("eval-installers-drift");
    let installer = sandbox.path().join("Downloads/Drift.dmg");
    write_aged(&installer, &"x".repeat(2048), 400);

    let (status, output) = drift_session(
        &sandbox,
        &["clean", "installers", "--apply", "--shred"],
        || age(&installer, 0),
    );
    assert_eq!(
        std::fs::read_to_string(&installer).ok().as_deref(),
        Some("x".repeat(2048).as_str()),
        "PV eval/installers-drift: an archive touched after the preview was removed:\n{output}"
    );
    assert!(
        !status.success(),
        "the refusal must fail the run:\n{output}"
    );
    assert!(
        output.contains("outside its authorized namespace"),
        "{output}"
    );
}

// ---------- clean agents ----------

/// One Codex standalone release in the layout the installer writes.
fn codex_release(releases: &Path, version: &str) -> PathBuf {
    let path = releases.join(format!("{version}-aarch64-apple-darwin"));
    for directory in ["bin", "codex-resources", "codex-path"] {
        std::fs::create_dir_all(path.join(directory)).unwrap();
    }
    for binary in ["bin/codex", "bin/codex-code-mode-host", "codex-path/rg"] {
        let file = path.join(binary);
        std::fs::write(&file, "binary").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    symlink("bin/codex", path.join("codex")).unwrap();
    let voice = path.join("codex-resources/voice");
    std::fs::create_dir_all(&voice).unwrap();
    std::fs::write(voice.join("runtime.json"), "{}").unwrap();
    std::fs::write(
        voice.join("manifest.json"),
        serde_json::json!({
            "schemaVersion": 1,
            "appVersion": version,
            "appTarget": "aarch64-apple-darwin",
            "voiceTarget": "aarch64-apple-darwin",
            "sha256": {
                "bin/codex": "9a3a45d01531a20e89ac6ae10b0b0beb0492acd7216a368aa062d1a5fecaf9cd",
                "codex-resources/voice/runtime.json": "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
            }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        path.join("codex-package.json"),
        format!(
            r#"{{"layoutVersion":1,"version":"{version}","target":"aarch64-apple-darwin","variant":"codex","entrypoint":"bin/codex","resourcesDir":"codex-resources","pathDir":"codex-path"}}"#
        ),
    )
    .unwrap();
    path
}

/// Regenerable caches, stale history at exactly its configured depth, and one
/// older Codex release are the positives. The config sets a one-day project
/// window: agent history follows `retain_days` (30 unset), never that window.
fn agents_fixture(sandbox: &Sandbox) -> Vec<PathBuf> {
    let home = sandbox.path();
    write(&home.join(".config/devtrim.toml"), "active_days = 1\n");
    let mut targets = Vec::new();

    for relative in [
        ".claude/cache",
        ".codex/cache",
        ".pi/web-search-cache",
        ".cache/opencode",
    ] {
        write(&home.join(relative).join("payload.json"), "regenerable");
        targets.push(home.join(relative));
    }
    for (relative, days) in [
        (".claude/shell-snapshots/stale.sh", 400),
        (".codex/shell_snapshots/stale.sh", 400),
        (".codex/archived_sessions/rollout-stale.jsonl", 400),
        (".codex/archived_sessions/rollout-31-days.jsonl", 31),
    ] {
        write_aged(&home.join(relative), "history", days);
        targets.push(home.join(relative));
    }
    // Codex nests sessions as <year>/<month>/<day>; the day is the finding.
    write_aged(
        &home.join(".codex/sessions/2024/05/17/rollout-a.jsonl"),
        "history",
        400,
    );
    targets.push(home.join(".codex/sessions/2024/05/17"));

    // Near misses, each named by the rule that keeps it.
    write(&home.join(".claude/shell-snapshots/fresh.sh"), "now"); // inside retention
    write_aged(&home.join(".claude/shell-snapshots/day29.sh"), "x", 29); // one day short of 30
    // Five days is past the one-day project window but inside retain_days.
    write_aged(&home.join(".codex/shell_snapshots/five-days.sh"), "x", 5);
    // The newest file in a subtree decides its age.
    write_aged(&home.join(".codex/sessions/2024/05/18/old.jsonl"), "x", 400);
    write(&home.join(".codex/sessions/2024/05/18/fresh.jsonl"), "now");
    // Only children at exactly the configured depth are candidates.
    write_aged(&home.join(".codex/sessions/loose.jsonl"), "x", 400);
    write_aged(&home.join(".codex/sessions/2024/loose.jsonl"), "x", 400);
    // A link inside a history root never widens it to a foreign tree.
    write_aged(&home.join("outside-history/rollout.jsonl"), "foreign", 400);
    symlink(
        home.join("outside-history"),
        home.join(".claude/shell-snapshots/linked-stale"),
    )
    .unwrap();
    for (relative, days) in [
        (".claude/projects/repo/memory/MEMORY.md", 400), // auto memory is never a history root
        (".claude/jobs/state.json", 400),                // the supervisor's live state
        (".claude/file-history/abc/v1", 400),            // not listed
        (".codex/.tmp/scratch/x", 400),                  // not listed
        (".claude/cache-backup/keep.json", 400),         // a sibling never inherits authority
        (".cache/opencode-data/keep.json", 400),
        (".codex/auth.json", 400), // credentials
        (".claude/.credentials.json", 400),
        (".claude/settings.json", 400), // configuration
        (".codex/config.toml", 400),
        (".codex/memories/m.md", 400),
        (".claude/skills/s/SKILL.md", 400),
        (".claude.json", 400),
        (".claude.json.backup", 400),
        (".codex/plugins/cache/keep.txt", 400),
        (".codex/thread_history_1.sqlite", 400),
        (".local/share/opencode/auth.json", 400), // OpenCode's persistent state
    ] {
        write_aged(&home.join(relative), "keep", days);
    }

    // Codex releases: only a verified package strictly older than `current`,
    // that no process executes, is offered.
    let standalone = home.join(".codex/packages/standalone");
    let releases = standalone.join("releases");
    let old = codex_release(&releases, "0.155.1");
    let running = codex_release(&releases, "0.154.0"); // a process executes it
    let current = codex_release(&releases, "0.156.1");
    let _newer = codex_release(&releases, "0.157.0"); // not older than current
    let personal = codex_release(&releases, "0.152.0");
    // Unknown contents mean the package is not the installer's own shape.
    write(&personal.join("codex-resources/personal.txt"), "keep");
    write(&standalone.join("install.lock"), "");
    symlink(&current, standalone.join("current")).unwrap();
    sandbox.script(
        "lsof",
        &format!(
            "printf 'p1\\nftxt\\nn/usr/lib/dyld\\np77\\nftxt\\nn{}\\n'",
            running.canonicalize().unwrap().join("bin/codex").display()
        ),
    );
    targets.push(old);
    sorted(targets)
}

#[test]
fn eval_clean_agents_removes_only_regenerable_caches_and_stale_history() {
    let sandbox = Sandbox::in_target("eval-agents");
    let targets = agents_fixture(&sandbox);
    let pristine = Tree::snapshot(sandbox.path());

    let preview = run(&sandbox, &["clean", "agents", "--shred", "--json"]);
    let document = json(&preview);
    assert_eq!(
        actionable_targets(&document),
        targets,
        "PV eval/agents-plan: the preview must offer exactly the caches, stale history and the old release"
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "PV eval/agents-preview",
    );
    assert!(preview.status.success(), "{document}");

    let apply = run(
        &sandbox,
        &["clean", "agents", "--apply", "--shred", "--yolo", "--json"],
    );
    let document = json(&apply);
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: targets.clone(),
            scratch: vec![sandbox.path().join(".local/state/devtrim")],
        },
        "PV eval/agents-apply",
    );
    // `.local` is scratch above, so OpenCode's persistent state is checked here.
    assert_eq!(
        std::fs::read_to_string(sandbox.path().join(".local/share/opencode/auth.json"))
            .ok()
            .as_deref(),
        Some("keep"),
        "PV eval/agents-apply: OpenCode's persistent state was removed"
    );
    assert!(apply.status.success(), "{document}");
    assert_eq!(document["summary"]["items_touched"], targets.len());
    assert_journaled(&sandbox, "agents", "shred", &targets);
}

// ---------- clean simulators ----------

const GONE_ONE: &str = "11111111-1111-1111-1111-111111111111";
const GONE_TWO: &str = "22222222-2222-2222-2222-222222222222";
const WORKING: &str = "33333333-3333-3333-3333-333333333333";

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

/// A `xcrun` answering `simctl list` with `first` once and `later` after, and
/// deleting only the device folder it is told to, like the real `simctl delete`.
fn simulator_stub(sandbox: &Sandbox, first: &str, later: &str) {
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
        .replace("FIRST", first)
        .replace("LATER", later);
    stub(sandbox, "xcrun", &body);
}

fn simulator_devices(sandbox: &Sandbox) -> PathBuf {
    let devices = sandbox
        .path()
        .join("Library/Developer/CoreSimulator/Devices");
    for (udid, bytes) in [(GONE_ONE, 8192), (GONE_TWO, 16384), (WORKING, 32768)] {
        write(&devices.join(udid).join("data/payload"), &"x".repeat(bytes));
    }
    // A folder simctl does not list is not a device devtrim may delete.
    write(&devices.join("not-listed-by-simctl/data/payload"), "keep");
    devices
}

#[test]
fn eval_clean_simulators_deletes_only_unavailable_devices_by_exact_udid() {
    let sandbox = Sandbox::in_target("eval-simulators");
    let devices = simulator_devices(&sandbox);
    let listing = simctl_list(&[(GONE_TWO, false), (WORKING, true), (GONE_ONE, false)]);
    simulator_stub(&sandbox, &listing, &listing);
    let pristine = Tree::snapshot(sandbox.path());
    let list = argv(&["xcrun", "simctl", "list", "devices", "--json"]);

    let preview = run(&sandbox, &["clean", "simulators", "--json"]);
    let document = json(&preview);
    assert_eq!(
        command_plan(&document),
        vec![
            argv(&["xcrun", "simctl", "delete", GONE_ONE]),
            argv(&["xcrun", "simctl", "delete", GONE_TWO]),
        ],
        "PV eval/simulators-plan: the preview must offer a delete for exactly the unavailable devices"
    );
    // The working device appears only as report-only disclosure.
    assert_eq!(
        labelled_actions(&document)
            .into_iter()
            .filter(|(_, action)| action != "command")
            .collect::<Vec<_>>(),
        vec![(
            "Simulator device data (1 available device)".to_string(),
            "none".to_string()
        )],
        "PV eval/simulators-plan: working simulators must stay report-only"
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            scratch: vec![sandbox.path().join("stub")],
            ..Allowed::default()
        },
        "PV eval/simulators-preview",
    );
    assert_eq!(
        calls(&sandbox),
        vec![argv(&["xcrun", "--version"]), list.clone()],
        "PV eval/simulators-preview: a preview may only probe, never delete"
    );
    assert!(preview.status.success(), "{document}");
    let before = calls(&sandbox).len();

    let apply = run(
        &sandbox,
        &["clean", "simulators", "--apply", "--yolo", "--json"],
    );
    let document = json(&apply);
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: vec![devices.join(GONE_ONE), devices.join(GONE_TWO)],
            scratch: vec![
                sandbox.path().join("stub"),
                sandbox.path().join(".local/state/devtrim"),
            ],
        },
        "PV eval/simulators-apply",
    );
    // Scan once, then a fresh recheck before each previewed delete.
    assert_eq!(
        calls(&sandbox)[before..],
        [
            argv(&["xcrun", "--version"]),
            list.clone(),
            list.clone(),
            argv(&["xcrun", "simctl", "delete", GONE_ONE]),
            list,
            argv(&["xcrun", "simctl", "delete", GONE_TWO]),
        ],
        "PV eval/simulators-apply: the exact xcrun sequence"
    );
    assert!(apply.status.success(), "{document}");
    assert_eq!(document["summary"]["items_touched"], 2);
    assert_journaled_commands(
        &sandbox,
        "simulators",
        &[
            argv(&["xcrun", "simctl", "delete", GONE_ONE]),
            argv(&["xcrun", "simctl", "delete", GONE_TWO]),
        ],
    );
    assert!(
        document["summary"]["bytes_freed_estimate"]
            .as_u64()
            .unwrap()
            > 0,
        "the freed estimate must reflect the deleted device folders: {document}"
    );
}

/// Apply asks `simctl` again before each delete: a device that became
/// available, or vanished, after the preview is refused and never deleted.
#[test]
fn eval_simulators_apply_refuses_a_device_that_changed_after_preview() {
    let first = simctl_list(&[(GONE_ONE, false)]);
    let cases = [
        (
            "became available",
            simctl_list(&[(GONE_ONE, true)]),
            "became available after preview",
        ),
        ("vanished", simctl_list(&[]), "vanished after preview"),
    ];
    for (name, later, message) in cases {
        let sandbox = Sandbox::in_target("eval-simulators-recheck");
        let devices = simulator_devices(&sandbox);
        simulator_stub(&sandbox, &first, &later);
        let pristine = Tree::snapshot(sandbox.path());

        let apply = run(
            &sandbox,
            &["clean", "simulators", "--apply", "--yolo", "--json"],
        );
        let document = json(&apply);
        let list = argv(&["xcrun", "simctl", "list", "devices", "--json"]);
        assert_eq!(
            calls(&sandbox),
            vec![argv(&["xcrun", "--version"]), list.clone(), list],
            "PV eval/simulators-recheck: a device that {name} must never reach `simctl delete`"
        );
        pristine.assert_only(
            &Tree::snapshot(sandbox.path()),
            &Allowed {
                scratch: vec![
                    sandbox.path().join("stub"),
                    sandbox.path().join(".local/state/devtrim"),
                ],
                ..Allowed::default()
            },
            "PV eval/simulators-recheck",
        );
        assert!(
            devices.join(GONE_ONE).exists(),
            "PV eval/simulators-recheck: the device folder of a device that {name} was removed"
        );
        assert!(
            !apply.status.success(),
            "a refused delete must fail the run"
        );
        assert!(error_text(&document).contains(message), "{document}");
        assert_eq!(document["summary"]["items_touched"], 0);
        assert_no_attempts(&sandbox);
    }
}

// ---------- clean docker ----------

const DOCKER_HOST: &str = "unix:///var/run/docker.sock";

/// A daemon with reclaimable images and build cache, and volumes that must
/// never be pruned. Every `system df` row carries four tab-separated fields.
fn docker_stub(sandbox: &Sandbox) {
    let body = r#"case "$*" in
  'context inspect') printf '%s\n' '[{"Endpoints":{"docker":{"Host":"unix:///var/run/docker.sock"}}}]' ;;
  '--host unix:///var/run/docker.sock version') printf 'Docker version 28.0.0\n' ;;
  '--host unix:///var/run/docker.sock system df'*)
    printf 'Images\t2GB\t1GB (50%%)\t0\n'
    printf 'Containers\t1MB\t0B (0%%)\t0\n'
    printf 'Local Volumes\t5GB\t4GB (80%%)\t0\n'
    printf 'Build Cache\t3GB\t1GB\t0\n' ;;
  '--host unix:///var/run/docker.sock image prune -a -f') printf 'Total reclaimed space: 1GB\n' ;;
  '--host unix:///var/run/docker.sock builder prune -a -f') printf 'Total:\t3GB\n' ;;
  *) exit 1 ;;
esac"#;
    stub(sandbox, "docker", body);
}

/// Host-side VM disk images of the two runtimes, with real written bytes so
/// their allocated size is nonzero.
fn docker_vm_images(sandbox: &Sandbox) -> Vec<PathBuf> {
    let images = [
        "Library/Group Containers/HUAQ24HBR6.dev.orbstack/data/data.img.raw",
        "Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw",
    ]
    .map(|relative| sandbox.path().join(relative));
    for image in &images {
        write(image, &"v".repeat(64 * 1024));
    }
    write(&sandbox.path().join(".docker/config.json"), "{}");
    images.to_vec()
}

#[test]
fn eval_clean_docker_prunes_images_and_build_cache_never_volumes() {
    let sandbox = Sandbox::in_target("eval-docker");
    docker_vm_images(&sandbox);
    docker_stub(&sandbox);
    let pristine = Tree::snapshot(sandbox.path());
    let probes = vec![
        argv(&["docker", "context", "inspect"]),
        argv(&["docker", "--host", DOCKER_HOST, "version"]),
        argv(&[
            "docker",
            "--host",
            DOCKER_HOST,
            "system",
            "df",
            "--format",
            "{{.Type}}\t{{.Size}}\t{{.Reclaimable}}\t{{.Active}}",
        ]),
    ];
    let image_prune = argv(&[
        "docker",
        "--host",
        DOCKER_HOST,
        "image",
        "prune",
        "-a",
        "-f",
    ]);
    let builder_prune = argv(&[
        "docker",
        "--host",
        DOCKER_HOST,
        "builder",
        "prune",
        "-a",
        "-f",
    ]);

    let preview = run(&sandbox, &["clean", "docker", "--json"]);
    let document = json(&preview);
    assert_eq!(
        command_plan(&document),
        vec![image_prune.clone(), builder_prune.clone()],
        "PV eval/docker-plan: exactly an image prune and a builder prune, never a volume or system prune"
    );
    assert_eq!(
        labelled_actions(&document),
        vec![
            ("OrbStack VM disk image".to_string(), "none".to_string()),
            (
                "Docker Desktop VM disk image".to_string(),
                "none".to_string()
            ),
            (
                "Docker Images reclaimable".to_string(),
                "command".to_string()
            ),
            (
                "Docker Build Cache reclaimable".to_string(),
                "command".to_string()
            ),
        ],
        "PV eval/docker-plan: the VM disk images are report-only and the volumes row produces nothing"
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            scratch: vec![sandbox.path().join("stub")],
            ..Allowed::default()
        },
        "PV eval/docker-preview",
    );
    assert_eq!(
        calls(&sandbox),
        probes,
        "PV eval/docker-preview: a preview may only probe the daemon"
    );
    assert!(preview.status.success(), "{document}");
    let before = calls(&sandbox).len();

    let apply = run(
        &sandbox,
        &["clean", "docker", "--apply", "--yolo", "--json"],
    );
    let document = json(&apply);
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            scratch: vec![
                sandbox.path().join("stub"),
                sandbox.path().join(".local/state/devtrim"),
            ],
            ..Allowed::default()
        },
        "PV eval/docker-apply",
    );
    let mut expected = probes;
    expected.push(image_prune.clone());
    expected.push(builder_prune.clone());
    assert_eq!(
        calls(&sandbox)[before..],
        expected,
        "PV eval/docker-apply: probes, then exactly the two previewed prunes"
    );
    assert!(
        calls(&sandbox)
            .iter()
            .filter(|call| call.contains(&"prune".to_string()))
            .all(|call| *call == image_prune || *call == builder_prune)
            && !calls(&sandbox)
                .iter()
                .any(|call| call.contains(&"volume".to_string())),
        "PV eval/docker-apply: a volume or system prune ran"
    );
    assert!(apply.status.success(), "{document}");
    assert_eq!(document["summary"]["items_touched"], 2);
    assert_journaled_commands(&sandbox, "docker", &[image_prune, builder_prune]);
}

/// A stopped daemon is an error that still tells the operator what the host
/// pays for the VM disk image, and no prune is attempted.
#[test]
fn eval_docker_daemon_down_names_the_vm_image_and_runs_no_prune() {
    let sandbox = Sandbox::in_target("eval-docker-down");
    docker_vm_images(&sandbox);
    stub(
        &sandbox,
        "docker",
        r#"case "$*" in
  'context inspect') printf '%s\n' '[{"Endpoints":{"docker":{"Host":"unix:///var/run/docker.sock"}}}]' ;;
  *) printf 'Cannot connect to the Docker daemon\n' >&2; exit 1 ;;
esac"#,
    );
    let pristine = Tree::snapshot(sandbox.path());

    for args in [
        vec!["clean", "docker", "--json"],
        vec!["clean", "docker", "--apply", "--yolo", "--json"],
    ] {
        let output = run(&sandbox, &args);
        let document = json(&output);
        let errors = error_text(&document);
        assert!(
            errors.contains("Docker daemon unreachable")
                && errors.contains("OrbStack VM disk image is ")
                && errors.contains(" on this host"),
            "PV eval/docker-down: the error must name the VM image and its size: {errors}"
        );
        pristine.assert_only(
            &Tree::snapshot(sandbox.path()),
            &Allowed {
                scratch: vec![
                    sandbox.path().join("stub"),
                    sandbox.path().join(".local/state/devtrim"),
                ],
                ..Allowed::default()
            },
            "PV eval/docker-down",
        );
        assert!(
            !calls(&sandbox)
                .iter()
                .any(|call| call.contains(&"prune".to_string())),
            "PV eval/docker-down: a prune ran against an unreachable daemon"
        );
        assert!(!output.status.success(), "{document}");
    }
}

// ---------- optimize ----------

const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";

#[test]
fn eval_optimize_runs_exactly_the_selected_fixed_argv() {
    let sandbox = Sandbox::in_target("eval-optimize");
    stub(&sandbox, "qlmanage", "exit 0");
    stub(&sandbox, "atsutil", "exit 0");
    let pristine = Tree::snapshot(sandbox.path());
    let quicklook = argv(&["qlmanage", "-r", "cache"]);
    let fonts = argv(&["atsutil", "databases", "-removeUser"]);

    // Previewing every task is allowed and runs nothing.
    let preview = run(&sandbox, &["optimize", "--json"]);
    let document = json(&preview);
    assert_eq!(
        command_plan(&document),
        vec![
            quicklook.clone(),
            fonts.clone(),
            argv(&[
                LSREGISTER, "-kill", "-r", "-domain", "local", "-domain", "user"
            ]),
        ],
        "PV eval/optimize-plan: the catalog's fixed argv, with no root-requiring task"
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "PV eval/optimize-preview",
    );
    assert!(preview.status.success(), "{document}");

    // Applying without a named task is refused before any consent is asked.
    // No --yolo here on purpose: should the refusal ever vanish, this run
    // must still stop at the consent gate rather than reach any command.
    let bare = run(&sandbox, &["optimize", "--apply", "--json"]);
    let document = json(&bare);
    assert!(
        error_text(&document).contains("explicit --task"),
        "PV eval/optimize-refusal: --apply without --task must be refused for that reason: {document}"
    );
    let unknown = run(&sandbox, &["optimize", "--task", "nonsense", "--json"]);
    let document = json(&unknown);
    assert!(
        error_text(&document).contains("unknown task `nonsense`"),
        "PV eval/optimize-refusal: an unknown task must be refused: {document}"
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            scratch: vec![sandbox.path().join(".local/state/devtrim")],
            ..Allowed::default()
        },
        "PV eval/optimize-refusal",
    );
    assert!(
        calls(&sandbox).is_empty(),
        "PV eval/optimize-refusal: a refused run executed {:?}",
        calls(&sandbox)
    );
    assert!(!bare.status.success() && !unknown.status.success());

    // Naming tasks is consent to each of them, once each. Launch Services is
    // not selected: its absolute-path binary could not be stubbed.
    let apply = run(
        &sandbox,
        &[
            "optimize",
            "--task",
            "quicklook",
            "--task",
            "quicklook",
            "--task",
            "fonts",
            "--apply",
            "--yolo",
            "--json",
        ],
    );
    let document = json(&apply);
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            scratch: vec![
                sandbox.path().join("stub"),
                sandbox.path().join(".local/state/devtrim"),
            ],
            ..Allowed::default()
        },
        "PV eval/optimize-apply",
    );
    assert_eq!(
        calls(&sandbox),
        vec![quicklook.clone(), fonts.clone()],
        "PV eval/optimize-apply: exactly the two selected commands, each once, in order"
    );
    assert!(apply.status.success(), "{document}");
    assert_eq!(document["summary"]["items_touched"], 2);
    assert_journaled_commands(&sandbox, "optimize", &[quicklook, fonts]);
}

// ---------- trash-empty ----------

/// Direct Trash children (the positives) beside a `.git`-named child and
/// neighbours outside the Trash folder.
fn trash_fixture(sandbox: &Sandbox) -> Vec<PathBuf> {
    let home = sandbox.path();
    let trash = home.join(".Trash");
    write(&trash.join("old-report.txt"), "report");
    write(&trash.join("Old Project/src/main.c"), "int main(void);");
    write(&trash.join("cache 12.09.42/blob"), "blob");
    // Near misses, each named by the rule that keeps it.
    write(&trash.join(".git/HEAD"), "ref: refs/heads/main\n"); // a Git-named child is never purged
    for relative in [
        ".TrashBackup/keep.txt", // a neighbour sharing the prefix
        "Documents/keep.txt",
        "Library/Application Support/Tool/keep",
    ] {
        write(&home.join(relative), "keep");
    }
    sorted(
        ["old-report.txt", "Old Project", "cache 12.09.42"]
            .iter()
            .map(|name| trash.join(name))
            .collect(),
    )
}

#[test]
fn eval_trash_empty_purges_exactly_the_previewed_direct_children() {
    let sandbox = Sandbox::in_target("eval-trash");
    let targets = trash_fixture(&sandbox);
    let pristine = Tree::snapshot(sandbox.path());

    let preview = run(&sandbox, &["trash-empty", "--json"]);
    let document = json(&preview);
    assert_eq!(
        actionable_targets(&document),
        targets,
        "PV eval/trash-empty-plan: the preview must offer exactly the direct Trash children but the Git-named one"
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "PV eval/trash-empty-preview",
    );
    assert!(preview.status.success(), "{document}");

    // The size acknowledgment is mandatory and must be near the real size
    // (zero GB here, so 3 is outside the two-GB tolerance).
    for (name, args) in [
        (
            "no --confirm",
            vec!["trash-empty", "--apply", "--yolo", "--json"],
        ),
        (
            "a --confirm outside the tolerance",
            vec!["trash-empty", "--apply", "--yolo", "--confirm=3", "--json"],
        ),
        (
            "--confirm without consent",
            vec!["trash-empty", "--apply", "--confirm=0", "--json"],
        ),
    ] {
        let refused = run(&sandbox, &args);
        pristine.assert_only(
            &Tree::snapshot(sandbox.path()),
            &Allowed {
                scratch: vec![sandbox.path().join(".local/state/devtrim")],
                ..Allowed::default()
            },
            "PV eval/trash-empty-refusals",
        );
        assert!(
            !refused.status.success(),
            "PV eval/trash-empty-refusals: {name} must be refused"
        );
    }

    let apply = run(
        &sandbox,
        &["trash-empty", "--apply", "--yolo", "--confirm=0", "--json"],
    );
    let document = json(&apply);
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: targets.clone(),
            scratch: vec![sandbox.path().join(".local/state/devtrim")],
        },
        "PV eval/trash-empty-apply",
    );
    assert!(
        String::from_utf8_lossy(&apply.stderr).contains("skipping Git metadata Trash item"),
        "the kept Git-named child must be reported: {}",
        String::from_utf8_lossy(&apply.stderr)
    );
    assert!(apply.status.success(), "{document}");
    assert_eq!(document["summary"]["items_touched"], targets.len());
    assert_journaled(&sandbox, "trash-empty", "shred", &targets);
}

/// A Trash item the sink refuses (a trashed project still holding its
/// repository) is left whole, reported, and does not stop the items after it.
#[test]
fn eval_trash_empty_continues_past_an_item_holding_a_repository() {
    let sandbox = Sandbox::in_target("eval-trash-repo");
    let trash = sandbox.path().join(".Trash");
    // Sorted first, so a purge that stopped at the refusal would lose the rest.
    write(&trash.join("0-project/.git/HEAD"), "ref: refs/heads/main\n");
    write(&trash.join("0-project/src/lib.rs"), "fn main() {}");
    let targets = sorted(vec![trash.join("b-notes.txt"), trash.join("c-old build")]);
    write(&targets[0], "notes");
    write(&targets[1].join("out"), "out");
    let pristine = Tree::snapshot(sandbox.path());

    let apply = run(
        &sandbox,
        &["trash-empty", "--apply", "--yolo", "--confirm=0", "--json"],
    );
    let document = json(&apply);
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: targets.clone(),
            scratch: vec![sandbox.path().join(".local/state/devtrim")],
        },
        "PV eval/trash-empty-continue",
    );
    assert!(
        !apply.status.success(),
        "PV eval/trash-empty-continue: a refused item must make the run nonzero: {document}"
    );
    assert_eq!(document["summary"]["items_touched"], targets.len());
}

/// `--only-devtrim` offers only items whose identity the journal recorded for
/// a successful move; names prove nothing, and an unfinished or failed move
/// proves nothing either. The journal is written by hand because a real Trash
/// move drives Finder.
#[test]
fn eval_trash_empty_only_devtrim_selects_only_journaled_identities() {
    use std::os::macos::fs::MetadataExt;

    let sandbox = Sandbox::in_target("eval-trash-only");
    let home = sandbox.path();
    let trash = home.join(".Trash");
    let ours_dir = trash.join("cache 12.09.42"); // Finder renamed it
    let ours_file = trash.join("report 14.30.txt");
    let foreign_same_name = trash.join("cache"); // same name as the journaled original
    let failed_move = trash.join("failed-move");
    let interrupted = trash.join("interrupted-move");
    write(&ours_dir.join("file"), "ours");
    write(&ours_file, "ours");
    write(&foreign_same_name.join("file"), "not ours");
    write(&failed_move.join("file"), "recorded as failed");
    write(&interrupted.join("file"), "never finished");
    let identity = |path: &Path| {
        let metadata = std::fs::symlink_metadata(path).unwrap();
        serde_json::json!({
            "dev": metadata.st_dev(),
            "ino": metadata.st_ino(),
            "birth_secs": metadata.st_birthtime(),
            "birth_nanos": metadata.st_birthtime_nsec()
        })
    };
    let record = |id: &str, phase: &str, target: &str, path: &Path, status: Option<&str>| {
        let mut value = serde_json::json!({
            "id": id, "ts": 1, "phase": phase, "op": "caches", "action": "trash",
            "target": target, "size_bytes": 10, "identity": identity(path)
        });
        match status {
            Some("ok") => value["status"] = "ok".into(),
            Some(_) => {
                value["status"] = "error".into();
                value["error"] = "boom".into();
            }
            None => {}
        }
        value.to_string()
    };
    let lines = [
        record(
            "a1",
            "attempt",
            "/Users/x/Library/Caches/cache",
            &ours_dir,
            None,
        ),
        record(
            "a1",
            "result",
            "/Users/x/Library/Caches/cache",
            &ours_dir,
            Some("ok"),
        ),
        record(
            "a2",
            "attempt",
            "/Users/x/Desktop/report.txt",
            &ours_file,
            None,
        ),
        record(
            "a2",
            "result",
            "/Users/x/Desktop/report.txt",
            &ours_file,
            Some("ok"),
        ),
        record("a3", "attempt", "/Users/x/failed-move", &failed_move, None),
        record(
            "a3",
            "result",
            "/Users/x/failed-move",
            &failed_move,
            Some("error"),
        ),
        record(
            "a4",
            "attempt",
            "/Users/x/interrupted-move",
            &interrupted,
            None,
        ),
    ];
    write(&sandbox.journal(), &(lines.join("\n") + "\n"));
    let targets = sorted(vec![ours_dir, ours_file]);
    let pristine = Tree::snapshot(home);

    let preview = run(&sandbox, &["trash-empty", "--only-devtrim", "--json"]);
    let document = json(&preview);
    assert_eq!(
        actionable_targets(&document),
        targets,
        "PV eval/trash-only-plan: only items with a journaled successful move may be offered"
    );
    pristine.assert_only(
        &Tree::snapshot(home),
        &Allowed::default(),
        "PV eval/trash-only-preview",
    );
    assert!(preview.status.success(), "{document}");

    let apply = run(
        &sandbox,
        &[
            "trash-empty",
            "--only-devtrim",
            "--apply",
            "--yolo",
            "--confirm=0",
            "--json",
        ],
    );
    let document = json(&apply);
    pristine.assert_only(
        &Tree::snapshot(home),
        &Allowed {
            removed: targets.clone(),
            scratch: vec![home.join(".local")],
        },
        "PV eval/trash-only-apply",
    );
    assert!(apply.status.success(), "{document}");
    assert_eq!(document["summary"]["items_touched"], targets.len());
    let purged: Vec<String> = journal_lines(&sandbox)
        .into_iter()
        .filter(|line| line.op == "trash-empty" && line.phase == "attempt")
        .filter_map(|line| line.target)
        .collect();
    assert_eq!(
        sorted(purged.iter().map(PathBuf::from).collect()),
        targets,
        "EVAL journal: the purge must be journaled write-ahead"
    );
}

// ---------- flag capabilities ----------

/// Every command the documentation says rejects a flag exits 2 with one JSON
/// error document naming the flag, before it does anything. Each invocation
/// carries all the consent it would need to mutate, so an accepted flag shows
/// as a changed tree or a stub that ran.
#[test]
fn eval_commands_reject_flags_they_cannot_honor_and_change_nothing() {
    let sandbox = Sandbox::in_target("eval-flags");
    write(&sandbox.path().join(".Trash/precious.txt"), "keep");
    write(
        &sandbox
            .path()
            .join("Library/Developer/CoreSimulator/Devices/keep/data"),
        "keep",
    );
    for name in ["docker", "xcrun", "qlmanage"] {
        stub(&sandbox, name, "exit 0");
    }
    let pristine = Tree::snapshot(sandbox.path());

    let cases: [(&[&str], &str, &str); 17] = [
        (
            &[
                "trash-empty",
                "--shred",
                "--apply",
                "--yolo",
                "--confirm=0",
                "--json",
            ],
            "trash-empty",
            "--shred",
        ),
        (
            &[
                "optimize",
                "--shred",
                "--task",
                "quicklook",
                "--apply",
                "--yolo",
                "--json",
            ],
            "optimize",
            "--shred",
        ),
        (
            &["clean", "docker", "--shred", "--apply", "--yolo", "--json"],
            "docker",
            "--shred",
        ),
        (
            &[
                "clean",
                "simulators",
                "--shred",
                "--apply",
                "--yolo",
                "--json",
            ],
            "simulators",
            "--shred",
        ),
        (&["status", "--apply", "--json"], "status", "--apply"),
        (&["status", "--yolo", "--json"], "status", "--yolo"),
        (
            &["uninstall", "Nothing", "--apply", "--json"],
            "uninstall",
            "--apply",
        ),
        (
            &["analyze", "--apply", "--yolo", "--json"],
            "analyze",
            "--apply",
        ),
        (&["largest", "--apply", "--json"], "largest", "--apply"),
        (&["history", "--apply", "--json"], "history", "--apply"),
        (
            &["clean", "leftovers", "--apply", "--yolo", "--json"],
            "leftovers",
            "--apply",
        ),
        (&["scan", "--apply", "--yolo", "--json"], "scan", "--apply"),
        (&["tui", "--apply", "--json"], "tui", "--apply"),
        (&["tui", "-y", "--json"], "tui", "-y/--yes"),
        (&["tui", "--yolo", "--json"], "tui", "--yolo"),
        (&["tui", "--shred", "--json"], "tui", "--shred"),
        (&["tui", "--json"], "tui", "--json"),
    ];
    for (args, operation, flag) in cases {
        let output = run(&sandbox, args);
        let document = json(&output);
        assert_eq!(
            output.status.code(),
            Some(2),
            "PV eval/flags-reject: {args:?} must exit 2: {document}"
        );
        assert_eq!(
            document["operation"], operation,
            "PV eval/flags-reject: {args:?}"
        );
        assert_eq!(document["applied"], false, "PV eval/flags-reject: {args:?}");
        let errors = document["errors"].as_array().unwrap();
        assert!(
            errors.len() == 1
                && errors[0].as_str().unwrap().contains("does not accept flag")
                && errors[0].as_str().unwrap().contains(flag),
            "PV eval/flags-reject: {args:?} must name {flag}: {document}"
        );
        assert!(
            output.stderr.is_empty(),
            "PV eval/flags-reject: {args:?} wrote to stderr"
        );
    }
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "PV eval/flags-nothing-changed",
    );
}
