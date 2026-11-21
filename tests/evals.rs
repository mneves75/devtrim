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
    Allowed, Interactive, ModeGuard, Sandbox, Tree, actionable_targets, assert_journaled, git,
    json, on_disk, real_git, run, write,
};

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

#[test]
fn oracle_allows_only_directories_on_the_way_to_scratch() {
    let (_sandbox, root) = control_fixture("oracle-scratch");
    let scratch = root.join("state/app");
    let allowed = Allowed {
        scratch: vec![scratch.clone()],
        ..Allowed::default()
    };
    let before = Tree::snapshot(&root);
    write(&scratch.join("journal"), "record");
    before.diff(&Tree::snapshot(&root), &allowed).unwrap();

    let (_sandbox, root) = control_fixture("oracle-scratch-file");
    let scratch = root.join("state/app");
    let allowed = Allowed {
        scratch: vec![scratch],
        ..Allowed::default()
    };
    let before = Tree::snapshot(&root);
    write(
        &root.join("state"),
        "a file where a folder would lead to scratch",
    );
    let problems = before.diff(&Tree::snapshot(&root), &allowed).unwrap_err();
    assert!(problems.contains("created: "), "{problems}");
}

// ---------- clean caches ----------

/// The `clean caches` fixture: every listed cache, planted, and the near
/// misses its documentation excludes. Returns the targets it must remove.
fn caches_fixture(sandbox: &Sandbox) -> Vec<PathBuf> {
    let home = sandbox.path();
    // `npm` is asked not to write a log or its update-check timestamp.
    sandbox.script(
        "npm",
        "[ \"${npm_config_logs_max:-}\" = 0 ] && [ \"${npm_config_update_notifier:-}\" = false ] || exit 3\nprintf '%s\\n' \"$HOME/.npm\"",
    );
    sandbox.script("brew", "printf '%s\\n' \"$HOME/Library/Caches/Homebrew\"");
    // `go` must be asked with `GOTOOLCHAIN=local`, or a `go.mod` could make it
    // download a toolchain.
    sandbox.script(
        "go",
        "[ \"$*\" = 'env GOMODCACHE' ] && [ \"${GOTOOLCHAIN:-}\" = local ] || exit 3\nprintf '%s\\n' \"$HOME/go/pkg/mod\"",
    );
    let targets: Vec<PathBuf> = [
        ".cache/huggingface/hub",
        ".cache/uv",
        ".cache/node/corepack",
        ".bun/install/cache",
        ".cache/gh",
        ".cargo/registry/cache",
        ".cargo/registry/src",
        "Library/Caches/pip",
        "Library/Caches/go-build",
        "Library/Caches/pnpm",
        "Library/Caches/org.swift.swiftpm",
        "Library/Caches/CocoaPods",
        ".npm",
        // Homebrew's cache holds a Git clone, so it is offered as its other children.
        "Library/Caches/Homebrew/downloads",
        "Library/Caches/Homebrew/bootsnap",
        ".gradle/caches",
        ".gradle/wrapper/dists",
        "go/pkg/mod",
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
        ".cache/node/compile-cache/entry", // only Corepack's child of `.cache/node` is listed
        "Library/pnpm/store/v10/index.json", // the pnpm store, unlike its metadata cache
        "Library/Caches/JetBrains/IDEA/LocalHistory/changes", // IDE system dir with LocalHistory
        "Library/Caches/deno/location_data/kv", // DENO_DIR holds KV databases
        "Library/Caches/ms-playwright-extra/x", // a neighbour sharing a listed prefix
        "Library/Caches/com.apple.Safari/x", // an unlisted Library cache
        ".cargo/registry/index/x",         // cargo's index is not listed
        ".cargo/bin/cargo",
        "Library/Caches/Homebrew/omlx--git/.git/HEAD", // a Homebrew Git clone stays
        "Library/Caches/Homebrew/omlx--git/README",
        ".gradle/gradle.properties", // Gradle's configuration beside its caches
        ".gradle/init.d/init.gradle",
        ".gradle/daemon/9.8.1/registry.bin",
        "go/bin/tool", // installed commands beside the module cache
    ] {
        write(&home.join(relative), "keep");
    }
    write(
        &home.join("go/pkg/mod/cache/download/example.com/m/@v/v1.0.0.zip"),
        "module",
    );
    write(
        &home.join(".gradle/caches/modules-2/files-2.1/x.jar"),
        "module",
    );
    write(
        &home.join(".gradle/wrapper/dists/gradle-9.8.1-bin/a1b2/gradle-9.8.1-bin.zip"),
        "distribution",
    );
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
        "eval/caches-preview",
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
            scratch: vec![sandbox.path().join(".local/state/devtrim")],
        },
        "PV eval/caches-apply",
    );
    assert_journaled(&sandbox, "caches", "shred", &targets);
}

/// The apply document says what became of every actionable finding, so an
/// agent never has to read the journal to learn which items went: `ok` and
/// `error` mirror the journaled result, and `not_attempted` names a finding
/// its category refused before the sink.
#[test]
fn eval_apply_json_reports_each_items_result() {
    let sandbox = Sandbox::in_target("eval-apply-results");
    let home = sandbox.path();
    let targets = caches_fixture(&sandbox);
    // A Git marker inside a cache: the sink refuses it.
    write(&home.join("Library/Caches/pip/nested/.git/HEAD"), "ref");
    // uv busy with its cache: refused before the sink, never journaled.
    let uv_lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.join(".cache/uv/.lock"))
        .unwrap();
    rustix::fs::flock(&uv_lock, rustix::fs::FlockOperation::NonBlockingLockShared).unwrap();

    let apply = run(
        &sandbox,
        &["clean", "caches", "--apply", "--shred", "--yolo", "--json"],
    );
    let document = json(&apply);
    assert!(!apply.status.success(), "{document}");
    let findings = document["findings"].as_array().unwrap();
    let results = document["results"].as_array().unwrap_or_else(|| {
        panic!("eval/apply-results: the apply document lists no results: {document}")
    });
    let actionable: Vec<usize> = findings
        .iter()
        .enumerate()
        .filter(|(_, finding)| {
            matches!(finding["action"]["type"].as_str(), Some("trash" | "shred"))
        })
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        results
            .iter()
            .map(|result| result["finding"].as_u64().unwrap() as usize)
            .collect::<Vec<_>>(),
        actionable,
        "every actionable finding has exactly one result, in plan order: {document}"
    );
    let status_of = |relative: &str| {
        let path = home.join(relative);
        let result = results
            .iter()
            .find(|result| result["path"].as_str() == path.to_str())
            .unwrap_or_else(|| panic!("no result for {relative}: {document}"));
        (
            result["status"].as_str().unwrap().to_owned(),
            result.get("error").cloned(),
        )
    };
    let (pip, pip_error) = status_of("Library/Caches/pip");
    assert_eq!(pip, "error", "{document}");
    assert!(pip_error.is_some_and(|error| error.is_string()));
    let (uv, uv_error) = status_of(".cache/uv");
    assert_eq!(uv, "not_attempted", "{document}");
    assert!(uv_error.is_none());
    for target in &targets {
        let relative = target.strip_prefix(home).unwrap().to_str().unwrap();
        if relative == "Library/Caches/pip" || relative == ".cache/uv" {
            continue;
        }
        assert_eq!(status_of(relative), ("ok".to_owned(), None), "{document}");
        assert!(!target.exists());
    }
    assert!(home.join("Library/Caches/pip").exists());
    assert!(home.join(".cache/uv").exists());

    // A preview carries no results.
    let preview = run(&sandbox, &["clean", "caches", "--shred", "--json"]);
    assert!(json(&preview).get("results").is_none());
    drop(uv_lock);
}

/// Sizes are bytes on disk: a cache whose only file is a hole frees nothing
/// and is not offered, though its length is 1 GiB; one written byte brings it
/// back at the size `du` gives it.
#[test]
fn eval_a_cache_holding_no_blocks_is_not_offered() {
    let sandbox = Sandbox::in_target("eval-sparse-cache");
    let hub = sandbox.path().join(".cache/huggingface/hub");
    write(&hub.join("model.bin"), "");
    std::fs::File::options()
        .write(true)
        .open(hub.join("model.bin"))
        .unwrap()
        .set_len(1024 * 1024 * 1024)
        .unwrap();
    let preview = run(&sandbox, &["clean", "caches", "--json"]);
    assert!(preview.status.success(), "{}", json(&preview));
    assert!(
        !actionable_targets(&json(&preview)).contains(&hub),
        "a cache holding only a hole was offered: {}",
        json(&preview)
    );

    std::fs::OpenOptions::new()
        .write(true)
        .open(hub.join("model.bin"))
        .and_then(|file| std::os::unix::fs::FileExt::write_all_at(&file, b"x", 0))
        .unwrap();
    let preview = json(&run(&sandbox, &["clean", "caches", "--json"]));
    let finding = preview["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["path"].as_str() == hub.to_str())
        .unwrap_or_else(|| panic!("positive control: the written cache is offered: {preview}"));
    assert_eq!(finding["size_bytes"], on_disk(&[&hub.join("model.bin")]));
    assert!(finding["size_bytes"].as_u64().unwrap() < 1024 * 1024);
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
        ".cache/node/corepack",
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
    std::fs::remove_dir_all(home.join(".cache/node/corepack")).unwrap();
    write(
        &home.join(".cache/node/corepack/payload.bin"),
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
        std::fs::read_to_string(home.join(".cache/node/corepack/payload.bin"))
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

// ---------- project build output: node-modules, artifacts, purge ----------

/// A planted `dev/` folder of real Git repositories, so activity dates,
/// tracked files and hostile configuration are judged by Git itself.
struct Projects {
    dev: PathBuf,
    node_modules: Vec<PathBuf>,
    artifacts: Vec<PathBuf>,
    /// Created only if Git ever runs a program a repository's config names.
    hostile_marker: PathBuf,
}

const STALE: &str = "2020-01-01T00:00:00Z";

#[test]
fn fixture_home_cannot_follow_the_callers_working_directory() {
    const CHILD: &str = "DEVTRIM_TARGET_ISOLATION_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let fixture = Sandbox::in_target("target-isolation-child");
        let owned = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
        assert!(
            fixture.path().starts_with(owned),
            "the fixture escaped its compiled checkout into the caller's directory"
        );
        return;
    }
    let foreign = Sandbox::new("target-isolation-owner");
    let pristine = Tree::snapshot(foreign.path());
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "--nocapture",
            "fixture_home_cannot_follow_the_callers_working_directory",
        ])
        .env_clear()
        .env(CHILD, "1")
        .current_dir(foreign.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    // `--exact` with a misspelled name runs nothing and still exits 0.
    assert!(
        output.status.success() && stdout.contains("test result: ok. 1 passed;"),
        "{stdout}"
    );
    pristine.assert_only(
        &Tree::snapshot(foreign.path()),
        &Allowed::default(),
        "the fixture must not change the caller's directory",
    );
}

#[test]
fn fixture_git_cannot_write_into_an_ambient_repository() {
    const CHILD: &str = "DEVTRIM_GIT_ISOLATION_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let sandbox = Sandbox::in_target("git-isolation-child");
        let project = sandbox.path().join("project");
        write(&project.join("package.json"), "{}");
        repo(&sandbox, &project, STALE, &["package.json"]);
        assert!(
            project.join(".git").is_dir(),
            "fixture Git wrote into the ambient repository"
        );
        return;
    }
    let owner = Sandbox::in_target("git-isolation-owner");
    let foreign = owner.path().join("foreign.git");
    write(&foreign.join("sentinel"), "keep");
    let pristine = Tree::snapshot(owner.path());
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "--nocapture",
            "fixture_git_cannot_write_into_an_ambient_repository",
        ])
        .env(CHILD, "1")
        .env("GIT_DIR", &foreign)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("test result: ok. 1 passed;"),
        "fixture isolation failed: {stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    pristine.assert_only(
        &Tree::snapshot(owner.path()),
        &Allowed::default(),
        "fixture Git environment isolation",
    );
}

/// A repository whose last activity is `date`, tracking `tracked`.
fn repo(sandbox: &Sandbox, path: &Path, date: &str, tracked: &[&str]) {
    std::fs::create_dir_all(path).unwrap();
    git(sandbox, path, date, &["init", "-q"]);
    if !tracked.is_empty() {
        let mut add = vec!["add", "-f", "--"];
        add.extend_from_slice(tracked);
        git(sandbox, path, date, &add);
        git(sandbox, path, date, &["commit", "-q", "-m", "fixture"]);
    }
}

fn projects_fixture(sandbox: &Sandbox) -> Projects {
    let git_binary = real_git();
    sandbox.script("git", &format!("exec '{}' \"$@\"", git_binary.display()));
    let dev = sandbox.path().join("dev");
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let now = format!("@{epoch} +0000");
    let now = now.as_str();
    let file = |path: PathBuf| write(&path, "build output");

    // Positives.
    let web = dev.join("web");
    write(&web.join("package.json"), "{}");
    file(web.join("node_modules/pkg/index.js"));
    file(web.join(".next/cache/chunk.js"));
    repo(sandbox, &web, STALE, &["package.json"]);

    let rust = dev.join("rust");
    write(&rust.join("Cargo.toml"), "[package]\nname = \"rust\"\n");
    file(rust.join("target/debug/out"));
    repo(sandbox, &rust, STALE, &["Cargo.toml"]);

    // Next.js standalone output copies a node_modules into .next: it belongs
    // to that output, so only .next is offered and its bytes count once.
    let standalone = dev.join("standalone");
    write(&standalone.join("package.json"), "{}");
    write(&standalone.join(".next/standalone/package.json"), "{}");
    file(standalone.join(".next/standalone/node_modules/pkg/index.js"));
    repo(sandbox, &standalone, STALE, &["package.json"]);

    // A stale repository whose config names programs Git could run. Its
    // dependencies are still offered; the programs must never run.
    let hostile = dev.join("hostile");
    write(&hostile.join("package.json"), "{}");
    file(hostile.join("node_modules/pkg/index.js"));
    repo(sandbox, &hostile, STALE, &["package.json"]);
    let marker = sandbox.path().join("HOSTILE-PROGRAM-RAN");
    let payload = sandbox.script("payload", &format!("echo ran >> '{}'", marker.display()));
    let hooks = hostile.join(".git/evil-hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    for hook in [
        "pre-commit",
        "post-checkout",
        "post-index-change",
        "reference-transaction",
    ] {
        std::os::unix::fs::symlink(&payload, hooks.join(hook)).unwrap();
    }
    for (key, value) in [
        ("core.fsmonitor", payload.display().to_string()),
        ("core.hooksPath", hooks.display().to_string()),
        ("core.pager", payload.display().to_string()),
        ("diff.external", payload.display().to_string()),
    ] {
        git(sandbox, &hostile, STALE, &["config", key, &value]);
    }

    // Near misses, each kept by one documented rule.
    let active = dev.join("active"); // recent Git activity
    write(&active.join("package.json"), "{}");
    file(active.join("node_modules/pkg/index.js"));
    write(&active.join("Cargo.toml"), "[package]\nname = \"active\"\n");
    file(active.join("target/debug/out"));
    repo(sandbox, &active, now, &["package.json", "Cargo.toml"]);

    let pods = dev.join("pods"); // a build directory holding tracked files
    write(&pods.join("Podfile"), "platform :ios, '17.0'\n");
    write(&pods.join("Pods/Lib/Lib.swift"), "// vendored\n");
    repo(sandbox, &pods, STALE, &["Podfile", "Pods"]);

    let solana = dev.join("solana"); // a program keypair nothing regenerates
    write(&solana.join("Cargo.toml"), "[package]\nname = \"solana\"\n");
    write(
        &solana.join("target/deploy/program-keypair.json"),
        "[1,2,3]",
    );
    repo(sandbox, &solana, STALE, &["Cargo.toml"]);

    let terraform = dev.join("terraform"); // Terraform state nothing regenerates
    write(&terraform.join("terragrunt.hcl"), "");
    write(
        &terraform.join(".terragrunt-cache/abc/terraform.tfstate"),
        "{}",
    );
    repo(sandbox, &terraform, STALE, &["terragrunt.hcl"]);

    let nested = dev.join("nested"); // a repository inside build output
    write(&nested.join("Cargo.toml"), "[package]\nname = \"nested\"\n");
    write(
        &nested.join("target/checkout/.git/HEAD"),
        "ref: refs/heads/main\n",
    );
    repo(sandbox, &nested, STALE, &["Cargo.toml"]);

    let manifestless = dev.join("manifestless"); // node_modules with no package.json
    file(manifestless.join("node_modules/pkg/index.js"));
    write(&manifestless.join("README"), "");
    repo(sandbox, &manifestless, STALE, &["README"]);

    let ambiguous = dev.join("ambiguous"); // names too ambiguous to own
    write(&ambiguous.join("package.json"), "{}");
    for name in ["build", "dist", "out", "coverage", "vendor"] {
        file(ambiguous.join(name).join("x"));
    }
    repo(sandbox, &ambiguous, STALE, &["package.json"]);

    let fresh = dev.join("fresh"); // a new repository with no commit to judge
    write(&fresh.join("package.json"), "{}");
    file(fresh.join("node_modules/pkg/index.js"));
    repo(sandbox, &fresh, STALE, &[]);

    let linked = dev.join("linked"); // a node_modules that is a link
    write(&linked.join("package.json"), "{}");
    repo(sandbox, &linked, STALE, &["package.json"]);
    std::os::unix::fs::symlink("../web/node_modules", linked.join("node_modules")).unwrap();

    let dev = dev.canonicalize().unwrap();
    let mut node_modules = vec![
        dev.join("web/node_modules"),
        dev.join("hostile/node_modules"),
    ];
    node_modules.sort();
    let mut artifacts = vec![
        dev.join("web/.next"),
        dev.join("rust/target"),
        dev.join("standalone/.next"),
    ];
    artifacts.sort();
    Projects {
        dev,
        node_modules,
        artifacts,
        hostile_marker: marker,
    }
}

/// One project category, previewed then applied against the fixture.
fn eval_project_category(
    name: &str,
    args: &[&str],
    op: Option<&str>,
    pick: fn(&Projects) -> Vec<PathBuf>,
) {
    let sandbox = Sandbox::in_target(name);
    let projects = projects_fixture(&sandbox);
    let expected = pick(&projects);
    let root = projects.dev.to_str().unwrap().to_owned();
    let pristine = Tree::snapshot(sandbox.path());

    let mut preview_args = args.to_vec();
    preview_args.extend(["--root", &root, "--shred", "--json"]);
    let preview = run(&sandbox, &preview_args);
    let document = json(&preview);
    assert_eq!(
        actionable_targets(&document),
        expected,
        "PV eval/projects-plan: {name} must offer exactly the stale, owned build output"
    );
    assert!(
        !projects.hostile_marker.exists(),
        "PV eval/hostile-repo: previewing ran a program the repository's config names"
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "eval/projects-preview",
    );
    assert!(preview.status.success(), "{document}");

    let mut apply_args = args.to_vec();
    apply_args.extend(["--root", &root, "--apply", "--shred", "--yolo", "--json"]);
    let apply = run(&sandbox, &apply_args);
    let document = json(&apply);
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: expected.clone(),
            scratch: vec![sandbox.path().join(".local/state/devtrim")],
        },
        "PV eval/projects-apply",
    );
    assert!(apply.status.success(), "{document}");
    if let Some(op) = op {
        assert_journaled(&sandbox, op, "shred", &expected);
    }
}

#[test]
fn eval_clean_node_modules_removes_only_stale_installs() {
    eval_project_category(
        "eval-node-modules",
        &["clean", "node-modules"],
        Some("node-modules"),
        |p| p.node_modules.clone(),
    );
}

#[test]
fn eval_clean_artifacts_removes_only_corroborated_stale_output() {
    eval_project_category(
        "eval-artifacts",
        &["clean", "artifacts"],
        Some("artifacts"),
        |p| p.artifacts.clone(),
    );
}

/// `scan` names what brings a target back only from the evidence that let it
/// be offered: a `Cargo.toml` beside `target`, a lockfile beside
/// `node_modules`. A `CACHEDIR.TAG` cache and an install with no lockfile
/// are offered with no such claim.
#[test]
fn eval_scan_says_what_brings_a_target_back_only_with_evidence() {
    let sandbox = Sandbox::in_target("eval-comes-back");
    let projects = projects_fixture(&sandbox);
    let tagged = projects.dev.join("tagged");
    write(&tagged.join("README"), "");
    write(
        &tagged.join("cache/CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    );
    write(&tagged.join("cache/blob"), "cached");
    repo(&sandbox, &tagged, STALE, &["README"]);
    let locked = projects.dev.join("locked");
    write(&locked.join("package.json"), "{}");
    write(&locked.join("pnpm-lock.yaml"), "lockfileVersion: '9.0'\n");
    write(&locked.join("node_modules/pkg/index.js"), "dependency");
    repo(
        &sandbox,
        &locked,
        STALE,
        &["package.json", "pnpm-lock.yaml"],
    );
    let root = projects.dev.to_str().unwrap().to_owned();
    let pristine = Tree::snapshot(sandbox.path());

    let scan = run(&sandbox, &["scan", "--all", "--root", &root]);
    let output = String::from_utf8_lossy(&scan.stdout);
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "eval/comes-back-preview",
    );
    assert!(scan.status.success(), "{output}");
    // Each finding prints its path, then its note on the next line.
    let note = |target: &Path| -> String {
        let path = target.display().to_string();
        let mut lines = output.lines();
        lines
            .by_ref()
            .find(|line| line.ends_with(&path))
            .unwrap_or_else(|| panic!("no finding for {path}: {output}"));
        lines.next().unwrap_or_default().to_owned()
    };
    let rust = note(&projects.dev.join("rust/target"));
    assert!(
        rust.contains("corroboration: sibling Cargo.toml; comes back with: cargo build"),
        "{rust}"
    );
    let pnpm = note(&projects.dev.join("locked/node_modules"));
    assert!(
        pnpm.contains("lockfile: pnpm-lock.yaml; comes back with: pnpm install"),
        "{pnpm}"
    );
    let cache = note(&projects.dev.join("tagged/cache"));
    assert!(
        cache.contains("corroboration: CACHEDIR.TAG signature"),
        "{cache}"
    );
    assert!(!cache.contains("comes back with"), "{cache}");
    let unlocked = note(&projects.dev.join("web/node_modules"));
    assert!(unlocked.contains("lockfile: none"), "{unlocked}");
    assert!(!unlocked.contains("comes back with"), "{unlocked}");
}

#[test]
fn eval_purge_is_exactly_node_modules_plus_artifacts() {
    eval_project_category("eval-purge", &["purge"], None, |p| {
        let mut both = [p.node_modules.clone(), p.artifacts.clone()].concat();
        both.sort();
        both
    });
}

fn eval_excluded_roots(names: &[&str], tag: &str) {
    let commands: [(&[&str], &[&str]); 3] = [
        (&["clean", "node-modules"], &["node_modules"]),
        (&["clean", "artifacts"], &[".next"]),
        (&["purge"], &["node_modules", ".next"]),
    ];
    let sandbox = Sandbox::in_target("eval-excluded-roots");
    sandbox.script("git", &format!("exec '{}' \"$@\"", real_git().display()));
    let mut scenarios = vec![("healthy", sandbox.path().join("dev/healthy"), true)];
    for (index, name) in names.iter().enumerate() {
        // Distinct parents keep case variants distinct on APFS too.
        scenarios.push((
            *name,
            sandbox
                .path()
                .join(format!("dev/case-{index}"))
                .join(name)
                .join("nested"),
            false,
        ));
    }
    for (name, project, eligible) in scenarios {
        write(&project.join("package.json"), "{}");
        write(&project.join("node_modules/pkg/index.js"), "dependency");
        write(&project.join(".next/cache/chunk.js"), "build output");
        repo(&sandbox, &project, STALE, &["package.json"]);
        for configured in [false, true] {
            for (command, leaves) in commands {
                let root = project.canonicalize().unwrap();
                let spelling = root.to_str().unwrap();
                let mut args = command.to_vec();
                if configured {
                    write(
                        &sandbox.path().join(".config/devtrim.toml"),
                        &format!("roots = [{}]\n", serde_json::to_string(spelling).unwrap()),
                    );
                } else {
                    args.extend(["--root", spelling]);
                }
                args.push("--json");
                let pristine = Tree::snapshot(sandbox.path());
                let output = run(&sandbox, &args);
                let document = json(&output);
                assert!(output.status.success(), "{args:?}: {document}");
                let mut expected = if eligible {
                    leaves.iter().map(|leaf| root.join(leaf)).collect()
                } else {
                    Vec::new()
                };
                expected.sort();
                assert_eq!(
                    actionable_targets(&document),
                    expected,
                    "PV eval/excluded-roots-{tag}: {args:?}, namespace {name}, configured {configured}, eligible {eligible}"
                );
                pristine.assert_only(
                    &Tree::snapshot(sandbox.path()),
                    &Allowed::default(),
                    "eval/excluded-roots-preview",
                );
            }
        }
    }
}

#[test]
fn eval_project_roots_below_dependency_namespaces_offer_nothing() {
    eval_excluded_roots(&["node_modules", "NODE_MODULES"], "dependencies");
}

#[test]
fn eval_project_roots_below_git_metadata_offer_nothing() {
    eval_excluded_roots(&[".git", ".GIT"], "git");
}

#[test]
fn eval_root_at_top_level_node_modules_still_offers_the_install() {
    let sandbox = Sandbox::in_target("eval-install-root");
    sandbox.script("git", &format!("exec '{}' \"$@\"", real_git().display()));
    let project = sandbox.path().join("dev/project");
    write(&project.join("package.json"), "{}");
    write(&project.join("node_modules/pkg/index.js"), "dependency");
    repo(&sandbox, &project, STALE, &["package.json"]);
    let root = project.join("node_modules").canonicalize().unwrap();
    let pristine = Tree::snapshot(sandbox.path());
    let output = run(
        &sandbox,
        &[
            "clean",
            "node-modules",
            "--root",
            root.to_str().unwrap(),
            "--json",
        ],
    );
    let document = json(&output);
    assert!(output.status.success(), "{document}");
    assert_eq!(actionable_targets(&document), vec![root]);
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "eval/excluded-roots-install-control",
    );
}

// ---------- explain ----------

/// `explain` answers from one fresh scan and the protection rules: the
/// finding that offers a path, or holds it, names its category; a protected
/// path names its rule; anything else says nothing offers it, and whether it
/// lies outside the scan roots. It changes nothing and refuses mutation flags.
#[test]
fn eval_explain_names_the_finding_or_rule_behind_a_path() {
    let sandbox = Sandbox::in_target("eval-explain");
    sandbox.script("git", &format!("exec '{}' \"$@\"", real_git().display()));
    let web = sandbox.path().join("dev/web");
    write(&web.join("package.json"), "{}");
    write(&web.join("node_modules/pkg/index.js"), "dependency");
    repo(&sandbox, &web, STALE, &["package.json"]);
    write(
        &sandbox.path().join("Library/Keychains/login.keychain-db"),
        "keys",
    );
    write(&sandbox.path().join("elsewhere/file"), "unrelated");
    std::os::unix::fs::symlink("dev", sandbox.path().join("alias")).unwrap();
    let home = sandbox.path().canonicalize().unwrap();
    let dev = home.join("dev");
    let installed = dev.join("web/node_modules");
    let config = sandbox.path().join(".config/devtrim.toml");
    write(&config, "");

    let explain = |path: &Path, extra: &[&str]| {
        let mut args = vec![
            "explain",
            path.to_str().unwrap(),
            "--root",
            dev.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        let pristine = Tree::snapshot(sandbox.path());
        let output = run(&sandbox, &args);
        pristine.assert_only(
            &Tree::snapshot(sandbox.path()),
            &Allowed::default(),
            "eval/explain-read-only",
        );
        output
    };
    let document = |path: &Path| {
        let output = explain(path, &["--json"]);
        let document = json(&output);
        assert_eq!(document["operation"], "explain", "{document}");
        assert_eq!(
            output.status.success(),
            document["errors"].as_array().unwrap().is_empty(),
            "{document}"
        );
        document
    };
    let paths = |document: &serde_json::Value| -> Vec<PathBuf> {
        document["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|finding| PathBuf::from(finding["path"].as_str().unwrap()))
            .collect()
    };
    let human = |path: &Path| String::from_utf8(explain(path, &[]).stdout).unwrap();

    // The stale install itself, a file inside it, and the same install named
    // through a linked folder all name the one node-modules finding.
    let exact = document(&installed);
    assert_eq!(paths(&exact), std::slice::from_ref(&installed), "{exact}");
    assert_eq!(exact["path"], installed.to_str().unwrap());
    assert_eq!(exact["protected"]["builtin"], false);
    assert!(exact["protected"]["config"].is_null());
    assert!(exact["errors"].as_array().unwrap().is_empty(), "{exact}");
    let text = human(&installed);
    assert!(
        text.contains("offered by node-modules: ") && text.contains("(this path)"),
        "{text}"
    );
    let inside = installed.join("pkg/index.js");
    assert_eq!(paths(&document(&inside)), std::slice::from_ref(&installed));
    assert!(human(&inside).contains("(holds this path)"));
    let aliased = sandbox.path().join("alias/web/node_modules");
    assert_eq!(paths(&document(&aliased)), std::slice::from_ref(&installed));
    // A project folder holds the install: it is reported as inside the path.
    assert!(human(&dev.join("web")).contains("(inside this path)"));
    // On a case-insensitive volume another spelling names the same folders,
    // so it relates to the install as the spelling the scan found does.
    if std::fs::symlink_metadata(dev.join("WEB/NODE_MODULES")).is_ok() {
        let variant = dev.join("web/NODE_MODULES");
        assert_eq!(
            paths(&document(&variant)),
            std::slice::from_ref(&installed),
            "eval/explain-case-variant: {variant:?}"
        );
        assert!(human(&variant).contains("(this path)"));
        let inside = dev.join("WEB/node_modules/PKG/index.js");
        assert_eq!(paths(&document(&inside)), std::slice::from_ref(&installed));
        assert!(human(&inside).contains("(holds this path)"));
        assert!(human(&dev.join("WEB")).contains("(inside this path)"));
    }

    // A built-in protected path names its rule; the install above is the
    // control that the rule is not reported for everything.
    let keychains = home.join("Library/Keychains");
    let protected = document(&keychains);
    assert_eq!(protected["protected"]["builtin"], true, "{protected}");
    assert!(paths(&protected).is_empty());
    let text = human(&keychains);
    assert!(
        text.contains("built-in protection: refuses it (~/Library"),
        "{text}"
    );

    // A configured entry is named; without it (above) the same install was
    // offered, and with it the scan withholds it.
    write(&config, "protect = [\"~/dev/web\"]\n");
    let configured = document(&installed);
    assert_eq!(
        configured["protected"]["config"],
        dev.join("web").to_str().unwrap(),
        "{configured}"
    );
    assert!(paths(&configured).is_empty(), "{configured}");
    let text = human(&installed);
    assert!(
        text.contains(&format!(
            "configured protect: refuses it (entry {})",
            dev.join("web").display()
        )),
        "{text}"
    );
    write(&config, "");

    // Nothing offers an unrelated path outside the roots; a path inside a root
    // that nothing offers is the control for the outside note.
    let elsewhere = home.join("elsewhere");
    assert!(paths(&document(&elsewhere)).is_empty());
    let text = human(&elsewhere);
    assert!(
        text.contains(&format!("no category offers {}", elsewhere.display()))
            && text.contains("is outside every scan root; scan roots: ")
            && text.contains("(from --root)"),
        "{text}"
    );
    let manifest = dev.join("web/package.json");
    let text = human(&manifest);
    assert!(
        text.contains("no category offers") && !text.contains("outside every scan root"),
        "{text}"
    );

    // Report-only: every mutation flag is refused before anything runs.
    for flag in ["--apply", "--yes", "--yolo", "--shred"] {
        let refused = explain(&installed, &[flag, "--json"]);
        assert_eq!(refused.status.code(), Some(2), "{flag}");
        let refusal = json(&refused);
        assert_eq!(refusal["operation"], "explain");
        assert!(
            refusal["errors"][0]
                .as_str()
                .unwrap()
                .contains("explain does not accept flag"),
            "{refusal}"
        );
    }
}

/// A relative path is read against the current directory, and a scan error
/// is reported and makes the run exit nonzero.
#[test]
fn eval_explain_reads_relative_paths_and_reports_scan_errors() {
    let sandbox = Sandbox::in_target("eval-explain-relative");
    sandbox.script("git", &format!("exec '{}' \"$@\"", real_git().display()));
    let web = sandbox.path().join("dev/web");
    write(&web.join("package.json"), "{}");
    write(&web.join("node_modules/pkg/index.js"), "dependency");
    repo(&sandbox, &web, STALE, &["package.json"]);
    let installed = web.join("node_modules").canonicalize().unwrap();
    let dev = sandbox.path().join("dev").canonicalize().unwrap();
    let explain = || {
        std::process::Command::new(env!("CARGO_BIN_EXE_devtrim"))
            .args([
                "explain",
                "./node_modules/../node_modules",
                "--root",
                dev.to_str().unwrap(),
                "--json",
            ])
            .current_dir(&web)
            .env_clear()
            .env("HOME", sandbox.path())
            .env("PATH", sandbox.bin())
            .output()
            .unwrap()
    };
    let output = explain();
    let document = json(&output);
    assert!(output.status.success(), "{document}");
    assert_eq!(document["path"], installed.to_str().unwrap());
    assert_eq!(
        document["findings"][0]["path"],
        installed.to_str().unwrap(),
        "{document}"
    );

    let locked = sandbox
        .path()
        .join("Library/Developer/Xcode/DerivedData/App-locked");
    write(&locked.join("Build/file"), "unreadable");
    let _locked = ModeGuard::unreadable(locked.join("Build"));
    let output = explain();
    let document = json(&output);
    assert!(
        !output.status.success() && !document["errors"].as_array().unwrap().is_empty(),
        "a scan error must be reported with a nonzero status: {document}"
    );
    assert_eq!(document["findings"][0]["path"], installed.to_str().unwrap());
}

// ---------- read-only commands and previews change nothing ----------

/// Every report-only command and every preview, run against a fixture holding
/// targets for every category, leaves it byte for byte as it was — first
/// with no devtrim state at all, then with an existing journal.
#[test]
fn eval_read_only_commands_and_previews_change_nothing() {
    let sandbox = Sandbox::in_target("eval-read-only");
    caches_fixture(&sandbox);
    let dev = sandbox.path().join("dev");
    let project = dev.join("project");
    write(&project.join("package.json"), "{}");
    write(
        &project.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\n",
    );
    write(&project.join("node_modules/pkg/index.js"), "dependency");
    write(&project.join("target/debug/out"), "build output");
    repo(&sandbox, &project, STALE, &["package.json", "Cargo.toml"]);
    write(&sandbox.path().join("Downloads/old.dmg"), "installer");
    let root = dev.canonicalize().unwrap().to_str().unwrap().to_owned();
    let dev = root.as_str();
    let commands: Vec<Vec<&str>> = vec![
        vec!["scan", "--json"],
        vec!["scan"],
        vec!["scan", "--all", "--shred"],
        vec!["purge", "--root", dev, "--json"],
        vec!["purge", "--root", dev, "--shred"],
        vec!["clean", "caches", "--json"],
        vec!["clean", "node-modules", "--root", dev, "--json"],
        vec!["clean", "artifacts", "--root", dev, "--json"],
        vec!["clean", "simulators", "--json"],
        vec!["clean", "xcode", "--json"],
        vec!["clean", "docker", "--json"],
        vec!["clean", "toolchains", "--json"],
        vec!["clean", "installers", "--json"],
        vec!["clean", "agents", "--json"],
        vec!["clean", "leftovers", "--root", dev, "--json"],
        vec!["largest", "--root", dev, "--json"],
        vec!["explain", dev, "--root", dev, "--json"],
        vec!["explain", dev, "--root", dev],
        vec!["icloud", "--json"],
        vec!["history", "--json"],
        vec!["analyze", dev, "--json"],
        vec!["status", "--json"],
        vec!["disk", "--json"],
        vec!["disk"],
        vec!["uninstall", "AltTab", "--json"],
        vec!["optimize", "--json"],
        vec!["trash-empty", "--json"],
        vec!["completions", "zsh"],
        vec!["manpage"],
        vec!["--help"],
    ];
    for journal in [false, true] {
        if journal {
            write(
                &sandbox.journal(),
                "{\"id\":\"eval\",\"ts\":1,\"phase\":\"attempt\",\"op\":\"caches\",\"action\":\"shred\",\"target\":\"/gone\",\"size_bytes\":1}\n",
            );
        }
        let pristine = Tree::snapshot(sandbox.path());
        for args in &commands {
            let output = run(&sandbox, args);
            // Positive control: the command reached its own result. A usage
            // error or a silent failure would leave the disk unchanged too.
            assert_ne!(
                output.status.code(),
                Some(2),
                "`devtrim {}` was rejected",
                args.join(" ")
            );
            assert!(
                !output.stdout.is_empty(),
                "`devtrim {}` printed nothing",
                args.join(" ")
            );
            if args.contains(&"--json") {
                json(&output);
            }
            if let Err(problems) =
                pristine.diff(&Tree::snapshot(sandbox.path()), &Allowed::default())
            {
                panic!(
                    "PV eval/read-only: `devtrim {}` (journal present: {journal}) changed the disk:\n{problems}",
                    args.join(" ")
                );
            }
        }
    }
}

// ---------- clean xcode while Xcode runs ----------

/// Set every regular file under `root` to `days` days old.
fn age(root: &Path, days: u64) {
    let when = std::time::SystemTime::now() - std::time::Duration::from_secs(days * 86_400);
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        let kind = std::fs::symlink_metadata(&path).unwrap().file_type();
        if kind.is_dir() {
            age(&path, days);
        } else if kind.is_file() {
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(when).unwrap();
        }
    }
}

#[test]
fn eval_clean_xcode_removes_only_idle_build_trees_while_xcode_runs() {
    use std::os::unix::fs::MetadataExt;
    let sandbox = Sandbox::in_target("eval-xcode");
    let developer = sandbox.path().join("Library/Developer/Xcode");
    let derived = developer.join("DerivedData");
    let build = |folder: &str| {
        write(
            &derived.join(folder).join("Build/Products/Debug/out"),
            "built",
        )
    };
    build("App-old");
    build("App-recent"); // touched within the activity window while Xcode runs
    build("App-held"); // old, but Xcode holds a file in it open
    build("App-pkg");
    write(&derived.join("App-pkg/Index.noindex/DataStore/v5"), "index");
    write(
        &derived.join("App-pkg/SourcePackages/checkouts/Example/.git/HEAD"),
        "ref: refs/heads/main\n",
    );
    write(&derived.join(".DS_Store"), "finder"); // a file beside the build trees
    std::os::unix::fs::symlink(&developer, derived.join("link")).unwrap(); // a link is never a build tree
    write(
        &developer.join("Archives/2026-01-01/App.xcarchive/Info.plist"),
        "archive",
    ); // Archives are sacred
    write(
        &developer.join("iOS DeviceSupport/17.0 (21A329)/Symbols/dyld"),
        "symbols",
    );
    let platforms = [
        "watchOS DeviceSupport/Watch7,1 27.0 (24R5)",
        "tvOS DeviceSupport/AppleTV14,1 27.0 (24J5)",
        "visionOS DeviceSupport/RealityDevice14,1 27.0 (24N5)",
    ];
    for folder in platforms {
        write(&developer.join(folder).join("Symbols/dyld"), "symbols");
    }
    for folder in ["App-old", "App-held", "App-pkg"] {
        age(&derived.join(folder), 60);
    }
    age(&developer.join("iOS DeviceSupport"), 60);
    age(&developer.join("Archives"), 60);

    let held = derived
        .join("App-held/Build/Products/Debug/out")
        .canonicalize()
        .unwrap();
    let device = std::fs::metadata(&held).unwrap().dev();
    sandbox.script(
        "pgrep",
        "case \"$*\" in\n  *Xcode*) echo 4242 ;;\n  *) exit 1 ;;\nesac",
    );
    sandbox.script(
        "lsof",
        &format!(
            "printf 'p4242\\ntREG\\nD0x{device:x}\\nn{}\\n'",
            held.display()
        ),
    );
    let derived = derived.canonicalize().unwrap();
    let developer = developer.canonicalize().unwrap();
    let mut expected = vec![
        derived.join("App-old"),
        derived.join("App-pkg/Build"),
        derived.join("App-pkg/Index.noindex"),
        developer.join("iOS DeviceSupport/17.0 (21A329)"),
    ];
    expected.extend(platforms.iter().map(|folder| developer.join(folder)));
    expected.sort();
    let pristine = Tree::snapshot(sandbox.path());

    let preview = run(&sandbox, &["clean", "xcode", "--shred", "--json"]);
    let document = json(&preview);
    assert_eq!(
        actionable_targets(&document),
        expected,
        "PV eval/xcode-plan: only idle, unheld build trees and symbols may be offered: {document}"
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "eval/xcode-preview",
    );
    assert!(preview.status.success(), "{document}");

    let apply = run(
        &sandbox,
        &["clean", "xcode", "--apply", "--shred", "--yolo", "--json"],
    );
    let document = json(&apply);
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: expected.clone(),
            scratch: vec![sandbox.path().join(".local/state/devtrim")],
        },
        "eval/xcode-apply",
    );
    assert!(apply.status.success(), "{document}");
    assert_journaled(&sandbox, "xcode", "shred", &expected);
}

/// One DeviceSupport or DerivedData folder that cannot be read or measured
/// offers nothing and is reported, so the run exits nonzero; every folder
/// beside it is still offered. Xcode is idle (the default `pgrep` stub finds
/// nothing), the state in which DerivedData folders are measured directly.
#[test]
fn eval_an_unreadable_xcode_folder_blocks_only_itself() {
    let sandbox = Sandbox::in_target("eval-xcode-unreadable");
    let developer = sandbox.path().join("Library/Developer/Xcode");
    let offered = vec![
        developer.join("DerivedData/App-readable"),
        developer.join("iOS DeviceSupport/iPhone18,2 27.0 (24A437)"),
    ];
    for folder in &offered {
        write(&folder.join("Symbols/file"), "regenerable");
    }
    let locked_child = developer.join("iOS DeviceSupport/iPhone17,1 26.0 (23A341)/locked");
    write(&locked_child.join("file"), "unreadable");
    let locked_folder = developer.join("DerivedData/App-locked");
    write(&locked_folder.join("Build/file"), "unreadable");
    let _child = ModeGuard::unreadable(locked_child);
    let _folder = ModeGuard::unreadable(locked_folder);

    let preview = run(&sandbox, &["clean", "xcode", "--json"]);
    let document = json(&preview);
    assert_eq!(
        actionable_targets(&document),
        offered,
        "PV eval/xcode-unreadable: an unreadable folder blocked the folders beside it"
    );
    assert!(
        !preview.status.success(),
        "an unreadable folder must fail the run: {document}"
    );
}

/// The workspace a DerivedData folder names in its `info.plist` is evidence
/// for the reader: a note, a diagnostic when it is gone, and nothing else —
/// both folders are offered and removed exactly as they would be without it.
/// A control character in the recorded path reaches the terminal escaped.
#[test]
fn eval_derived_data_notes_name_their_workspace() {
    let sandbox = Sandbox::in_target("eval-xcode-workspace");
    let derived = sandbox.path().join("Library/Developer/Xcode/DerivedData");
    let existing = sandbox.path().join("work/App.xcworkspace");
    std::fs::create_dir_all(&existing).unwrap();
    let existing = existing.canonicalize().unwrap();
    let gone = format!(
        "{}/gone/App\u{1b}[31m.xcworkspace",
        sandbox.path().display()
    );
    let plist = |path: &str| {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\">\n<dict>\n\t<key>WorkspacePath</key>\n\t<string>{path}</string>\n</dict>\n</plist>\n"
        )
    };
    write(&derived.join("Gone-abc/Build/out"), "built");
    write(&derived.join("Gone-abc/info.plist"), &plist(&gone));
    write(&derived.join("Here-abc/Build/out"), "built");
    write(
        &derived.join("Here-abc/info.plist"),
        &plist(&existing.display().to_string()),
    );
    let derived = derived.canonicalize().unwrap();
    let offered = vec![derived.join("Gone-abc"), derived.join("Here-abc")];
    let pristine = Tree::snapshot(sandbox.path());

    let preview = run(&sandbox, &["clean", "xcode", "--json"]);
    let document = json(&preview);
    assert!(preview.status.success(), "{document}");
    assert_eq!(actionable_targets(&document), offered, "{document}");
    let note = |folder: &str| {
        document["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|finding| finding["path"].as_str().unwrap().ends_with(folder))
            .map(|finding| finding["note"].as_str().unwrap().to_owned())
            .unwrap()
    };
    assert!(
        note("Gone-abc").ends_with(&format!("workspace no longer exists: {gone}")),
        "{document}"
    );
    assert!(
        note("Here-abc").ends_with(&format!("workspace: {}", existing.display())),
        "{document}"
    );

    let human = run(&sandbox, &["clean", "xcode"]);
    let stdout = String::from_utf8_lossy(&human.stdout);
    let stderr = String::from_utf8_lossy(&human.stderr);
    assert!(
        stdout.contains("App\\u{1b}[31m.xcworkspace") && !stdout.contains('\u{1b}'),
        "the workspace path reached the terminal raw: {stdout:?}"
    );
    assert!(
        stderr.contains("1 DerivedData folder(s), ")
            && stderr.contains("belong to workspaces that no longer exist"),
        "{stderr}"
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "eval/xcode-workspace-preview",
    );

    let apply = run(
        &sandbox,
        &["clean", "xcode", "--apply", "--shred", "--yolo", "--json"],
    );
    let document = json(&apply);
    assert!(apply.status.success(), "{document}");
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: offered.clone(),
            scratch: vec![sandbox.path().join(".local/state/devtrim")],
        },
        "eval/xcode-workspace-apply",
    );
    assert_journaled(&sandbox, "xcode", "shred", &offered);
}

/// The Xcode liveness probe asks for this user's processes only: another
/// user's Xcode, whose open files `lsof` cannot read, must not refuse
/// DerivedData. The control is this user's Xcode with the same unreadable
/// listing, which still refuses.
#[test]
fn eval_another_users_xcode_does_not_refuse_derived_data() {
    let uid = rustix::process::getuid().as_raw();
    let sandbox = Sandbox::in_target("eval-xcode-foreign-user");
    let derived = sandbox.path().join("Library/Developer/Xcode/DerivedData");
    write(&derived.join("App-old/Build/out"), "built");
    age(&derived.join("App-old"), 60);
    sandbox.script("lsof", "exit 1");
    let offered = derived.canonicalize().unwrap().join("App-old");

    sandbox.script(
        "pgrep",
        &format!(
            "case \" $* \" in *\" -U {uid} \"*) exit 1;; esac\ncase \"$*\" in *Xcode*) echo 4242 ;; *) exit 1 ;; esac"
        ),
    );
    let foreign = run(&sandbox, &["clean", "xcode", "--json"]);
    let document = json(&foreign);
    assert_eq!(
        actionable_targets(&document),
        vec![offered],
        "eval/xcode-current-user: another user's Xcode refused DerivedData: {document}"
    );
    assert!(foreign.status.success(), "{document}");

    sandbox.script(
        "pgrep",
        "case \"$*\" in *Xcode*) echo 4242 ;; *) exit 1 ;; esac",
    );
    let own = run(&sandbox, &["clean", "xcode", "--json"]);
    let document = json(&own);
    assert!(
        !own.status.success() && actionable_targets(&document).is_empty(),
        "control: this user's unreadable Xcode must refuse DerivedData: {document}"
    );
}

/// An Xcode support folder that cannot be listed fails the category and
/// names itself.
#[test]
fn eval_an_unlistable_xcode_support_folder_is_named() {
    let sandbox = Sandbox::in_target("eval-xcode-unlistable");
    let support = sandbox
        .path()
        .join("Library/Developer/Xcode/iOS DeviceSupport");
    write(&support.join("17.0 (21A329)/Symbols/dyld"), "symbols");
    let _locked = ModeGuard::unreadable(support.clone());
    let output = run(&sandbox, &["clean", "xcode", "--json"]);
    let document = json(&output);
    assert!(!output.status.success(), "{document}");
    assert!(
        document["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|error| error.as_str().unwrap().contains("iOS DeviceSupport")),
        "eval/xcode-unlistable: the error must name the folder: {document}"
    );
}

// ---------- two windows: active_days and retain_days ----------

/// `active_days` gates regenerable build output; `retain_days` gates what
/// nothing regenerates. Lowering the project window must never shorten how
/// long an installer (or agent history) is kept: unset, retention is the
/// project window or 30 days, whichever is longer.
#[test]
fn eval_the_project_window_never_shortens_retention() {
    let sandbox = Sandbox::in_target("eval-windows");
    let installer = sandbox.path().join("Downloads/old.dmg");
    write(&installer, "installer");
    age(&sandbox.path().join("Downloads"), 20);
    sandbox.script("git", &format!("exec '{}' \"$@\"", real_git().display()));
    let app = sandbox.path().join("dev/app");
    write(&app.join("package.json"), "{}");
    write(&app.join("node_modules/pkg/index.js"), "dependency");
    let ten_days_ago = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 10 * 86_400;
    repo(
        &sandbox,
        &app,
        &format!("@{ten_days_ago} +0000"),
        &["package.json"],
    );
    let installer = installer.canonicalize().unwrap();
    let node_modules = app.join("node_modules").canonicalize().unwrap();
    let dev = sandbox.path().join("dev").canonicalize().unwrap();
    let dev = dev.to_str().unwrap();

    let offered = |config: &str| -> (bool, bool) {
        write(&sandbox.path().join(".config/devtrim.toml"), config);
        let installers = json(&run(&sandbox, &["clean", "installers", "--json"]));
        let projects = json(&run(
            &sandbox,
            &["clean", "node-modules", "--root", dev, "--json"],
        ));
        (
            actionable_targets(&installers) == [installer.clone()],
            actionable_targets(&projects) == [node_modules.clone()],
        )
    };
    // Default: 30 days for both; neither the 20-day installer nor the
    // 10-day-idle project is old enough.
    assert_eq!(offered(""), (false, false), "PV eval/windows: defaults");
    // A 3-day project window frees the build output, and retention stays 30.
    assert_eq!(
        offered("active_days = 3\n"),
        (false, true),
        "PV eval/windows: lowering active_days shortened retention"
    );
    // An explicit retention shorter than the installer's age releases it.
    assert_eq!(
        offered("active_days = 3\nretain_days = 15\n"),
        (true, true),
        "PV eval/windows: retain_days"
    );
}

// ---------- a repository holding the home folder owns nothing ----------

/// A home folder kept as a Git repository (dotfiles) must not become the
/// owner of every project under it that has no repository of its own: its
/// last commit says nothing about a project edited this morning. Such a
/// project is not judged at all; a project with its own stale repository
/// beside it still is.
#[test]
fn eval_a_home_folder_repository_never_owns_the_projects_under_it() {
    let sandbox = Sandbox::in_target("eval-home-repo");
    let home = sandbox.path();
    sandbox.script("git", &format!("exec '{}' \"$@\"", real_git().display()));
    write(&home.join(".zshrc"), "# dotfiles");
    repo(&sandbox, home, STALE, &[".zshrc"]);

    let loose = home.join("dev/loose"); // no repository of its own
    write(&loose.join("package.json"), "{}");
    write(&loose.join("node_modules/pkg/index.js"), "fresh dependency");
    write(&loose.join("Cargo.toml"), "[package]\nname = \"loose\"\n");
    write(&loose.join("target/debug/out"), "fresh build");

    let owned = home.join("dev/owned"); // its own stale repository: still judged
    write(&owned.join("package.json"), "{}");
    write(&owned.join("node_modules/pkg/index.js"), "dependency");
    repo(&sandbox, &owned, STALE, &["package.json"]);

    let dev = home.join("dev").canonicalize().unwrap();
    let root = dev.to_str().unwrap();
    let preview = run(&sandbox, &["purge", "--root", root, "--json"]);
    let document = json(&preview);
    assert_eq!(
        actionable_targets(&document),
        [dev.join("owned/node_modules")],
        "PV eval/home-repo: a repository holding the home folder judged a project: {document}"
    );
    // The skip is named, not silent: the human preview says why the loose
    // project's build output was left out.
    let human = run(&sandbox, &["purge", "--root", root]);
    let stderr = String::from_utf8_lossy(&human.stderr);
    assert!(
        stderr.contains("whose only repository holds the home folder"),
        "the home-folder skip must be disclosed:\n{stderr}"
    );
}

// ---------- each removal is judged again just before it happens ----------

/// The preflight judges every finding before any is removed, but removing the
/// earlier ones takes time. A repository that becomes active in between —
/// here, after its preflight and before its own removal — must keep its
/// build output: each finding is judged again just before the sink.
fn assert_apply_rechecks_each_finding(name: &str, category: &str, output: &str, plant: fn(&Path)) {
    let sandbox = Sandbox::in_target(name);
    let dev = sandbox.path().join("dev");
    // The second repository becomes active only after the first output has
    // actually gone. The trigger is an observable mutation, not a query count.
    sandbox.script(
        "git",
        &format!(
            "case \"$*\" in\n  *ls-files*) exit 0 ;;\nesac\nif [ -d '{}' ] && [ -d '{}' ]; then day=2020-01-01; else day=2999-01-01; fi\ncase \"$*\" in\n  *' -g '*) printf 'HEAD@{{%s}}\\n' \"$day\" ;;\n  *) printf '%s\\n' \"$day\" ;;\nesac",
            dev.join("alpha").join(output).display(),
            dev.join("beta").join(output).display()
        ),
    );
    for project in ["alpha", "beta"] {
        let project = dev.join(project);
        std::fs::create_dir_all(project.join(".git")).unwrap();
        plant(&project);
    }
    let dev = dev.canonicalize().unwrap();
    let root = dev.to_str().unwrap();
    let pristine = Tree::snapshot(sandbox.path());

    let apply = run(
        &sandbox,
        &[
            "clean", category, "--root", root, "--apply", "--shred", "--yolo", "--json",
        ],
    );
    let document = json(&apply);
    let survivors = ["alpha", "beta"]
        .into_iter()
        .filter(|project| dev.join(project).join(output).exists())
        .collect::<Vec<_>>();
    assert_eq!(
        survivors,
        ["beta"],
        "PV eval/apply-recheck: {category} removed build output from the repository that became active: {document}"
    );
    pristine.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed {
            removed: vec![dev.join("alpha").join(output)],
            scratch: vec![sandbox.path().join(".local/state/devtrim")],
        },
        "PV eval/apply-recheck",
    );
    assert!(!apply.status.success(), "{document}");
    assert!(
        document
            .to_string()
            .contains("repo became active after preview"),
        "{document}"
    );
}

#[test]
fn eval_a_repository_that_becomes_active_during_apply_keeps_its_dependencies() {
    assert_apply_rechecks_each_finding(
        "eval-recheck-node-modules",
        "node-modules",
        "node_modules",
        |project| {
            write(&project.join("package.json"), "{}");
            write(&project.join("node_modules/pkg/index.js"), "dependency");
        },
    );
}

#[test]
fn eval_a_repository_that_becomes_active_during_apply_keeps_its_build_output() {
    assert_apply_rechecks_each_finding(
        "eval-recheck-artifacts",
        "artifacts",
        "target",
        |project| {
            write(&project.join("Cargo.toml"), "[package]\nname = \"p\"\n");
            write(&project.join("target/debug/out"), "build output");
        },
    );
}

// ---------- worktrees and .scratch reports ----------

/// The finding for `path` in a report document, by exact path.
fn finding_at<'a>(document: &'a serde_json::Value, path: &Path) -> &'a serde_json::Value {
    document["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["path"].as_str() == Some(path.to_str().unwrap()))
        .unwrap_or_else(|| panic!("no finding for {}: {document}", path.display()))
}

#[test]
fn eval_worktrees_lists_and_sizes_only_worktrees_that_lead_back_to_their_repository() {
    let sandbox = Sandbox::in_target("eval-worktrees");
    sandbox.script("git", &format!("exec '{}' \"$@\"", real_git().display()));
    sandbox.script("pgrep", "exit 1");
    let dev = sandbox.path().join("dev");
    let app = dev.join("app");
    write(&app.join("README"), "readme");
    repo(&sandbox, &app, STALE, &["README"]);
    git(
        &sandbox,
        &app,
        STALE,
        &["worktree", "add", "-q", "-b", "feature", "../app-feature"],
    );
    git(
        &sandbox,
        &app,
        STALE,
        &["worktree", "add", "-q", "--detach", "../app-detached"],
    );
    git(
        &sandbox,
        &app,
        STALE,
        &["worktree", "add", "-q", "-b", "usb", "../app-locked"],
    );
    git(
        &sandbox,
        &app,
        STALE,
        &["worktree", "lock", "--reason", "on usb", "../app-locked"],
    );
    let feature = dev.join("app-feature");
    write(&feature.join("untracked.bin"), &"x".repeat(101));

    // A worktree registered in `app` whose own `.git` leads to another
    // repository, and a genuine one outside the scan roots: both are listed,
    // neither is measured.
    let other = dev.join("other");
    write(&other.join("README"), "other");
    repo(&sandbox, &other, STALE, &["README"]);
    git(
        &sandbox,
        &other,
        STALE,
        &["worktree", "add", "-q", "--detach", "../decoy"],
    );
    let forged = dev.join("decoy");
    write(&forged.join("payload.bin"), &"x".repeat(4096));
    let registration = app.join(".git/worktrees/forged");
    write(
        &registration.join("gitdir"),
        &format!("{}/.git\n", forged.display()),
    );
    write(&registration.join("commondir"), "../..\n");
    let head = std::fs::read_to_string(app.join(".git/HEAD")).unwrap();
    write(&registration.join("HEAD"), &head);
    let outside = sandbox.path().join("elsewhere");
    git(
        &sandbox,
        &app,
        STALE,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            outside.to_str().unwrap(),
        ],
    );
    write(&outside.join("payload.bin"), &"x".repeat(4096));

    // A stray registration in `app` names another repository. It is listed
    // unsized, and that repository is still queried for its own worktrees.
    let zeta = dev.join("zeta");
    write(&zeta.join("README"), "zeta");
    repo(&sandbox, &zeta, STALE, &["README"]);
    let zeta_outside = sandbox.path().join("zeta-elsewhere");
    git(
        &sandbox,
        &zeta,
        STALE,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            zeta_outside.to_str().unwrap(),
        ],
    );
    let stray = app.join(".git/worktrees/stray");
    write(&stray.join("gitdir"), &format!("{}/.git\n", zeta.display()));
    write(&stray.join("commondir"), "../..\n");
    write(&stray.join("HEAD"), &head);

    // A linked worktree whose repository is gone is a skip note, not a failure.
    write(
        &dev.join("orphan/.git"),
        "gitdir: /nonexistent/worktrees/orphan\n",
    );

    let before = Tree::snapshot(sandbox.path());
    let root = dev.to_str().unwrap();
    let output = run(&sandbox, &["worktrees", "--root", root, "--json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document = json(&output);

    let size = |path: &Path| finding_at(&document, path)["size_bytes"].as_u64().unwrap();
    let note = |path: &Path| {
        finding_at(&document, path)["note"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(
        size(&feature),
        on_disk(&[
            &feature.join("README"),
            &feature.join("untracked.bin"),
            &feature.join(".git"),
        ]),
        "{document}"
    );
    assert!(note(&feature).contains("branch feature"), "{document}");
    assert!(
        note(&dev.join("app-detached")).contains("detached"),
        "{document}"
    );
    assert!(
        note(&dev.join("app-locked")).contains("locked: on usb"),
        "{document}"
    );
    assert_eq!(
        size(&app),
        0,
        "the main worktree is the repository, not a copy"
    );
    for unmeasured in [&forged, &outside] {
        assert_eq!(
            size(unmeasured),
            0,
            "PV eval/worktrees-common-dir: {document}"
        );
        assert!(note(unmeasured).contains("not measured"), "{document}");
    }
    assert!(note(&zeta_outside).contains("not measured"), "{document}");
    let last = document["findings"].as_array().unwrap().last().unwrap();
    assert!(last["path"].is_null(), "{document}");
    assert!(
        last["note"]
            .as_str()
            .unwrap()
            .contains("git worktree remove"),
        "{document}"
    );
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "worktrees",
    );

    let human = run(&sandbox, &["worktrees", "--root", root]);
    let stderr = String::from_utf8_lossy(&human.stderr);
    assert!(human.status.success(), "{stderr}");
    assert!(stderr.contains("orphan"), "the orphan is named: {stderr}");
    assert!(stderr.contains("not assessed"), "{stderr}");
    assert!(
        run(&sandbox, &["worktrees", "--apply", "--yolo", "--json"])
            .status
            .code()
            == Some(2)
    );
}

#[test]
fn eval_worktrees_reports_dirty_clean_activity_and_in_use_state() {
    let sandbox = Sandbox::in_target("eval-worktrees-state");
    sandbox.script("git", &format!("exec '{}' \"$@\"", real_git().display()));
    let dev = sandbox.path().join("dev");
    let app = dev.join("app");
    write(&app.join("README"), "readme");
    repo(&sandbox, &app, STALE, &["README"]);
    git(
        &sandbox,
        &app,
        STALE,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "quiet-branch",
            "../app-clean",
        ],
    );
    git(
        &sandbox,
        &app,
        STALE,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "dirty-branch",
            "../app-dirty",
        ],
    );
    git(
        &sandbox,
        &app,
        STALE,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "active-branch",
            "../app-active",
        ],
    );
    let clean = dev.join("app-clean");
    let dirty = dev.join("app-dirty");
    let active = dev.join("app-active");
    write(&dirty.join("untracked-a.txt"), "a");
    write(&dirty.join("untracked-b.txt"), "b");

    let active_real = active.canonicalize().unwrap();
    sandbox.script("pgrep", "printf '4242\\n'");
    sandbox.script(
        "lsof",
        &format!("printf 'p4242\\nfcwd\\nn{}\\n'", active_real.display()),
    );

    let output = run(
        &sandbox,
        &["worktrees", "--root", dev.to_str().unwrap(), "--json"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document = json(&output);
    let note = |path: &Path| {
        finding_at(&document, path)["note"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(note(&clean).contains("clean"), "{document}");
    assert!(note(&clean).contains("last active "), "{document}");
    assert!(note(&dirty).contains("dirty (2 changes)"), "{document}");
    assert!(note(&active).contains("in-use"), "{document}");
    assert!(
        document["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|finding| finding["action"]["type"] == "info"),
        "worktrees must remain strictly read-only: {document}"
    );

    // Hostile repository clean filter with `=` in subsection name plus an
    // inherited `GIT_CONFIG=/dev/null` must never execute filters during scan,
    // even when the filtered tracked file is modified so Git must hash it.
    let marker = sandbox.path().join("filter-ran");
    write(&clean.join(".gitattributes"), "README filter=hostile=eq\n");
    git(&sandbox, &clean, STALE, &["add", ".gitattributes"]);
    git(
        &sandbox,
        &clean,
        STALE,
        &["commit", "-q", "-m", "add hostile attribute"],
    );
    git(
        &sandbox,
        &app,
        STALE,
        &[
            "config",
            "filter.hostile=eq.clean",
            &format!("touch '{}'; cat", marker.display()),
        ],
    );
    // Keep the exact 6-byte size (`"readme"` -> `"README"`) and shift mtime so
    // Git's stat/size shortcut cannot decide dirtiness without entering content
    // conversion (which would execute `filter.hostile=eq.clean` if not blocked).
    write(&clean.join("README"), "README");
    let readme_file = std::fs::File::options()
        .write(true)
        .open(clean.join("README"))
        .unwrap();
    readme_file
        .set_modified(
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
        )
        .unwrap();
    let with_git_config = std::process::Command::new(env!("CARGO_BIN_EXE_devtrim"))
        .args(["worktrees", "--root", dev.to_str().unwrap(), "--json"])
        .env_clear()
        .env("HOME", sandbox.path())
        .env("PATH", sandbox.bin())
        .env("GIT_CONFIG", "/dev/null")
        .output()
        .unwrap();
    assert!(
        with_git_config.status.success(),
        "{}",
        String::from_utf8_lossy(&with_git_config.stderr)
    );
    assert!(
        !marker.exists(),
        "worktrees scan executed a repository clean filter when GIT_CONFIG was set"
    );
    let with_git_config_doc = json(&with_git_config);
    assert!(
        finding_at(&with_git_config_doc, &clean)["note"]
            .as_str()
            .unwrap()
            .contains("dirty (1 change)"),
        "{with_git_config_doc}"
    );
    write(&clean.join("README"), "readme");

    // Fail-closed: if the process probe fails, clean must not be claimed and
    // the error must be reported with nonzero status.
    sandbox.script("pgrep", "printf 'pgrep failed\\n' >&2; exit 2");
    let failed_probe = run(
        &sandbox,
        &["worktrees", "--root", dev.to_str().unwrap(), "--json"],
    );
    assert!(!failed_probe.status.success());
    let failed_doc = json(&failed_probe);
    assert!(
        !finding_at(&failed_doc, &clean)["note"]
            .as_str()
            .unwrap()
            .contains("clean"),
        "a failed process probe must never claim clean: {failed_doc}"
    );
    assert!(
        !failed_doc["errors"].as_array().unwrap().is_empty(),
        "a failed process probe must record an error: {failed_doc}"
    );
}

#[test]
fn eval_leftovers_reports_git_tmp_objdir_and_local_model_stores_read_only() {
    let sandbox = Sandbox::in_target("eval-leftovers-new");
    sandbox.script("git", &format!("exec '{}' \"$@\"", real_git().display()));
    let dev = sandbox.path().join("dev");
    let app = dev.join("app");
    write(&app.join("README"), "readme");
    repo(&sandbox, &app, STALE, &["README"]);
    let tmp_objdir = app.join(".git/objects/tmp_objdir-incoming-abc");
    write(&tmp_objdir.join("pack.tmp"), &"x".repeat(4096));
    // Hostile `.git` pointers: one escaping the scan root and one traversing a
    // symlinked ancestor must both be ignored.
    let outside_git = sandbox.path().join("outside-git");
    let outside_tmp = outside_git.join("objects/tmp_objdir-escape");
    write(&outside_tmp.join("pack.tmp"), &"e".repeat(4096));
    // An unreadable `commondir` outside scan roots must never be opened;
    // opening it would raise a permission error and fail the scan.
    let outside_commondir = outside_git.join("commondir");
    write(&outside_commondir, "..\n");
    write(&dev.join("escape-repo/.git"), "gitdir: ../../outside-git\n");
    write(&dev.join("escape-repo/.scratch/probe/file.txt"), "probe");
    let symlink_target = dev.join("real-gitdir");
    let symlink_tmp = symlink_target.join("objects/tmp_objdir-via-link");
    write(&symlink_tmp.join("pack.tmp"), &"s".repeat(4096));
    write(&dev.join("symlink-repo/.git"), "gitdir: link-git\n");
    std::os::unix::fs::symlink(&symlink_target, dev.join("symlink-repo/link-git")).unwrap();
    let ollama_models = sandbox.path().join(".ollama/models");
    write(&ollama_models.join("blobs/sha256-model"), &"m".repeat(4096));
    let lmstudio_models = sandbox.path().join(".lmstudio/models");
    write(&lmstudio_models.join("org/model.gguf"), &"g".repeat(4096));

    let before = Tree::snapshot(sandbox.path());
    let outside_commondir_guard = ModeGuard::unreadable(outside_commondir);
    let preview = run(
        &sandbox,
        &[
            "clean",
            "leftovers",
            "--root",
            dev.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(preview.status.success(), "{}", json(&preview));
    let document = json(&preview);
    assert!(
        document["errors"].as_array().unwrap().is_empty(),
        "out-of-scope unreadable commondir must never be opened: {document}"
    );
    for expected in [&tmp_objdir, &ollama_models, &lmstudio_models] {
        let finding = finding_at(&document, expected);
        assert_eq!(finding["action"]["type"], "info", "{document}");
        assert!(finding["size_bytes"].as_u64().unwrap() > 0, "{document}");
    }
    let listed_paths: Vec<&str> = document["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|finding| finding["path"].as_str())
        .collect();
    assert!(
        !listed_paths
            .iter()
            .any(|path| path.contains("tmp_objdir-escape") || path.contains("tmp_objdir-via-link")),
        "hostile .git pointers must not escape scan roots or traverse symlinks: {document}"
    );

    let apply = run(
        &sandbox,
        &[
            "clean",
            "leftovers",
            "--root",
            dev.to_str().unwrap(),
            "--apply",
            "--yolo",
            "--json",
        ],
    );
    drop(outside_commondir_guard);
    assert_eq!(apply.status.code(), Some(2), "{}", json(&apply));
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "eval/leftovers-read-only",
    );
}

/// A worktree that cannot be read in full is still listed with its
/// lower-bound size, and the partial read is an error: the run exits nonzero.
#[test]
fn eval_worktrees_partial_read_keeps_the_row_and_fails_the_run() {
    let sandbox = Sandbox::in_target("eval-worktrees-partial");
    sandbox.script("git", &format!("exec '{}' \"$@\"", real_git().display()));
    sandbox.script("pgrep", "exit 1");
    let dev = sandbox.path().join("dev");
    let app = dev.join("app");
    write(&app.join("README"), "readme");
    repo(&sandbox, &app, STALE, &["README"]);
    git(
        &sandbox,
        &app,
        STALE,
        &["worktree", "add", "-q", "-b", "feature", "../app-feature"],
    );
    let feature = dev.join("app-feature");
    write(&feature.join("locked/secret.bin"), &"x".repeat(4096));
    let locked = ModeGuard::unreadable(feature.join("locked"));
    let output = run(
        &sandbox,
        &["worktrees", "--root", dev.to_str().unwrap(), "--json"],
    );
    drop(locked);
    let document = json(&output);
    let row = finding_at(&document, &feature);
    assert!(
        row["note"].as_str().unwrap().contains("lower bound")
            && row["size_bytes"].as_u64().unwrap() > 0,
        "{document}"
    );
    assert!(
        !output.status.success(),
        "eval/worktrees-partial: a partial read exited 0: {document}"
    );
    assert!(
        document["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|error| error.as_str().unwrap().contains(feature.to_str().unwrap())),
        "eval/worktrees-partial: the error must name the worktree: {document}"
    );
}

/// A `.scratch` folder that cannot be read in full keeps its row and its
/// lower-bound size, and the partial read makes the run exit nonzero.
#[test]
fn eval_leftovers_partial_read_keeps_the_row_and_fails_the_run() {
    let sandbox = Sandbox::in_target("eval-leftovers-partial");
    sandbox.script("git", &format!("exec '{}' \"$@\"", real_git().display()));
    let dev = sandbox.path().join("dev");
    let kept = dev.join("kept");
    write(&kept.join(".gitignore"), ".scratch/\n");
    repo(&sandbox, &kept, STALE, &[".gitignore"]);
    let evidence = kept.join(".scratch/evidence");
    write(&evidence.join("run.log"), &"x".repeat(13));
    write(&evidence.join("locked/secret"), "secret");
    let locked = ModeGuard::unreadable(evidence.join("locked"));
    let output = run(
        &sandbox,
        &[
            "clean",
            "leftovers",
            "--root",
            dev.to_str().unwrap(),
            "--json",
        ],
    );
    drop(locked);
    let document = json(&output);
    let row = finding_at(&document, &evidence);
    assert!(
        row["note"].as_str().unwrap().contains("lower bound")
            && row["size_bytes"].as_u64().unwrap() > 0,
        "{document}"
    );
    assert!(
        !output.status.success(),
        "eval/leftovers-partial: a partial read exited 0: {document}"
    );
    assert!(
        document["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|error| error.as_str().unwrap().contains(evidence.to_str().unwrap())),
        "eval/leftovers-partial: the error must name the folder: {document}"
    );
}

/// Agent evidence and agent scratch read in part keep their rows as lower
/// bounds, as `.scratch` rows do, and the run exits nonzero.
#[test]
fn eval_leftovers_agent_hints_read_in_part_keep_their_rows() {
    let sandbox = Sandbox::in_target("eval-leftovers-hints-partial");
    let dev = sandbox.path().join("dev");
    let evidence = dev.join(".supergoal/evidence");
    let scratch = dev.join("codex-worktree.a1b2c3");
    for folder in [&evidence, &scratch] {
        write(&folder.join("run.log"), &"x".repeat(13));
        write(&folder.join("locked/secret"), "secret");
    }
    let locked = [
        ModeGuard::unreadable(evidence.join("locked")),
        ModeGuard::unreadable(scratch.join("locked")),
    ];
    let output = run(
        &sandbox,
        &[
            "clean",
            "leftovers",
            "--root",
            dev.to_str().unwrap(),
            "--json",
        ],
    );
    drop(locked);
    let document = json(&output);
    for folder in [&evidence, &scratch] {
        let row = finding_at(&document, folder);
        assert!(
            row["note"].as_str().unwrap().contains("lower bound")
                && row["size_bytes"].as_u64().unwrap() > 0,
            "{document}"
        );
    }
    assert!(!output.status.success(), "{document}");
}

#[test]
fn eval_leftovers_reports_ignored_scratch_folders_only() {
    let sandbox = Sandbox::in_target("eval-leftovers-scratch");
    sandbox.script("git", &format!("exec '{}' \"$@\"", real_git().display()));
    let dev = sandbox.path().join("dev");

    let kept = dev.join("org/kept");
    write(&kept.join(".gitignore"), ".scratch/\n");
    repo(&sandbox, &kept, STALE, &[".gitignore"]);
    write(&kept.join(".scratch/evidence/run.log"), &"x".repeat(13));
    git(
        &sandbox,
        &kept,
        STALE,
        &["worktree", "add", "-q", "--detach", ".scratch/wt-review"],
    );

    // Near misses: a .scratch Git does not ignore, and one holding a tracked file.
    let open = dev.join("open");
    write(&open.join("README"), "readme");
    repo(&sandbox, &open, STALE, &["README"]);
    write(&open.join(".scratch/notes/todo.md"), "mine");
    let tracked = dev.join("tracked");
    write(&tracked.join(".gitignore"), ".scratch/\n");
    write(&tracked.join(".scratch/keep/committed.txt"), "committed");
    repo(
        &sandbox,
        &tracked,
        STALE,
        &[".gitignore", ".scratch/keep/committed.txt"],
    );

    let before = Tree::snapshot(sandbox.path());
    let output = run(
        &sandbox,
        &[
            "clean",
            "leftovers",
            "--root",
            dev.to_str().unwrap(),
            "--json",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let document = json(&output);
    let mut listed: Vec<PathBuf> = document["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|finding| PathBuf::from(finding["path"].as_str().unwrap()))
        .collect();
    listed.sort();
    assert_eq!(
        listed,
        vec![
            kept.join(".scratch/evidence"),
            kept.join(".scratch/wt-review")
        ],
        "PV eval/leftovers-scratch: {document}"
    );
    assert_eq!(
        finding_at(&document, &kept.join(".scratch/evidence"))["size_bytes"],
        on_disk(&[&kept.join(".scratch/evidence/run.log")])
    );
    assert!(
        finding_at(&document, &kept.join(".scratch/wt-review"))["label"]
            .as_str()
            .unwrap()
            .contains("linked worktree"),
        "{document}"
    );
    before.assert_only(
        &Tree::snapshot(sandbox.path()),
        &Allowed::default(),
        "leftovers",
    );
}

// ---------- what a preview shows ----------

/// Terminal text keeps quotes literal: a scan root named `John's "app"` reads
/// as typed, never `John\'s \"app\"`. Quotes drive nothing in a terminal.
#[test]
fn eval_terminal_output_keeps_quotes_literal() {
    let sandbox = Sandbox::in_target("eval-quotes");
    let root = sandbox.path().join("John's \"app\"");
    std::fs::create_dir_all(&root).unwrap();

    let output = run(
        &sandbox,
        &["clean", "artifacts", "--root", root.to_str().unwrap()],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("scan roots: ~/John's \"app\" (from --root)"),
        "eval/terminal-quotes: the root must print with its quotes as typed: {stderr}"
    );
    assert!(
        !stderr.contains('\\'),
        "eval/terminal-quotes: a quote was escaped: {stderr}"
    );
    assert!(output.status.success(), "{stderr}");
}

/// A bare `~` names the home folder, as `~/` names a folder in it, whether
/// typed as `--root` or set as `roots`; it never reads as a relative folder
/// called `~`. A root that is the home folder is still scanned when named
/// explicitly, and the run says so: the defaults refuse it.
#[test]
fn eval_a_bare_tilde_root_is_the_home_folder_and_is_disclosed() {
    let sandbox = Sandbox::in_target("eval-tilde-root");
    let project = sandbox.path().join("dev/ws");
    stale_next_project(&sandbox, &project);
    let warning = format!(
        "scan root {} is the home folder or contains it",
        sandbox.path().canonicalize().unwrap().display()
    );

    let flagged = run(&sandbox, &["clean", "artifacts", "--root", "~", "--json"]);
    let stderr = String::from_utf8_lossy(&flagged.stderr);
    assert_eq!(
        actionable_targets(&json(&flagged)),
        vec![project.join(".next")],
        "PV eval/tilde-root: `--root ~` must scan the home folder: {stderr}"
    );
    assert!(
        stderr.contains(&warning),
        "PV eval/home-root-warning: an explicit home root must be disclosed: {stderr}"
    );

    write(
        &sandbox.path().join(".config/devtrim.toml"),
        "roots = [\"~\"]\n",
    );
    let configured = run(&sandbox, &["clean", "artifacts", "--json"]);
    let stderr = String::from_utf8_lossy(&configured.stderr);
    assert_eq!(
        actionable_targets(&json(&configured)),
        vec![project.join(".next")],
        "eval/tilde-root: `roots = [\"~\"]` must scan the home folder: {stderr}"
    );
    assert!(
        stderr.contains(&warning),
        "eval/home-root-warning: {stderr}"
    );

    // The control: a root inside the home folder is not the home folder.
    let inside = run(
        &sandbox,
        &["clean", "artifacts", "--root", "~/dev", "--json"],
    );
    let stderr = String::from_utf8_lossy(&inside.stderr);
    assert!(
        !stderr.contains("is the home folder or contains it"),
        "{stderr}"
    );
    assert_eq!(
        actionable_targets(&json(&inside)),
        vec![project.join(".next")]
    );
}

/// A stale repository with corroborated build output.
fn stale_next_project(sandbox: &Sandbox, project: &Path) {
    sandbox.script("git", &support::git_activity("2000-01-01", "2000-01-01"));
    write(&project.join(".git/HEAD"), "ref: refs/heads/main\n");
    write(&project.join("package.json"), "{}");
    write(&project.join(".next/cache/file"), "build output");
}

/// The preview drops a target the sink would refuse as built-in protected,
/// and a scan root inside a protected system folder is named in a warning.
/// The temporary folder resolves inside `/private/var`; the same project under
/// the home folder is the control proving the fixture is offerable.
#[test]
fn eval_preview_drops_targets_inside_a_protected_system_folder() {
    let sandbox = Sandbox::in_target("eval-protected-root");
    let control = sandbox.path().join("dev/ws");
    stale_next_project(&sandbox, &control);
    let offered = run(
        &sandbox,
        &[
            "clean",
            "artifacts",
            "--root",
            control.to_str().unwrap(),
            "--json",
        ],
    );
    assert_eq!(
        actionable_targets(&json(&offered)),
        vec![control.join(".next")],
        "the control: a stale .next outside any protected folder is offered"
    );

    let system = Sandbox::new("eval-protected-root-ws");
    let project = system.path().canonicalize().unwrap().join("ws");
    stale_next_project(&sandbox, &project);
    assert!(
        project.starts_with("/private/var"),
        "the fixture needs a temporary folder inside /private/var: {}",
        project.display()
    );
    let pristine = Tree::snapshot(system.path());
    let preview = run(
        &sandbox,
        &[
            "clean",
            "artifacts",
            "--root",
            project.to_str().unwrap(),
            "--json",
        ],
    );
    let document = json(&preview);
    let stderr = String::from_utf8_lossy(&preview.stderr);
    assert!(
        actionable_targets(&document).is_empty(),
        "eval/protected-preview: a target the sink refuses was previewed: {document}"
    );
    assert!(
        stderr.contains(&format!(
            "scan root {} is inside /private/var, which devtrim never removes from",
            project.display()
        )),
        "eval/protected-preview: the protected scan root must be named: {stderr}"
    );
    assert!(
        stderr.contains(&format!(
            "skipping protected path: {}",
            project.join(".next").display()
        )),
        "eval/protected-preview: the dropped target must be named: {stderr}"
    );
    pristine.assert_only(
        &Tree::snapshot(system.path()),
        &Allowed::default(),
        "eval/protected-preview",
    );
    assert!(preview.status.success(), "{document}");
}
