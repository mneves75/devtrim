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
    Allowed, Interactive, Sandbox, Tree, actionable_targets, assert_journaled, json, run,
};

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

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

// ---------- clean caches ----------

/// The `clean caches` fixture: every listed cache, planted, and the near
/// misses its documentation excludes. Returns the targets it must remove.
fn caches_fixture(sandbox: &Sandbox) -> Vec<PathBuf> {
    let home = sandbox.path();
    sandbox.script("npm", "printf '%s\\n' \"$HOME/.npm\"");
    sandbox.script("brew", "printf '%s\\n' \"$HOME/Library/Caches/Homebrew\"");
    let targets: Vec<PathBuf> = [
        ".cache/huggingface/hub",
        ".cache/uv",
        ".cache/node",
        ".bun/install/cache",
        ".cache/gh",
        ".cargo/registry/cache",
        ".cargo/registry/src",
        "Library/Caches/pip",
        "Library/Caches/go-build",
        "Library/Caches/pnpm",
        "Library/Caches/org.swift.swiftpm",
        ".npm",
        // Homebrew's cache holds a Git clone, so it is offered as its other children.
        "Library/Caches/Homebrew/downloads",
        "Library/Caches/Homebrew/bootsnap",
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
        "Library/pnpm/store/v10/index.json", // the pnpm store, unlike its metadata cache
        "Library/Caches/JetBrains/IDEA/LocalHistory/changes", // IDE system dir with LocalHistory
        "Library/Caches/deno/location_data/kv", // DENO_DIR holds KV databases
        "Library/Caches/ms-playwright-extra/x", // a neighbour sharing a listed prefix
        "Library/Caches/com.apple.Safari/x", // an unlisted Library cache
        ".cargo/registry/index/x",           // cargo's index is not listed
        ".cargo/bin/cargo",
        "Library/Caches/Homebrew/omlx--git/.git/HEAD", // a Homebrew Git clone stays
        "Library/Caches/Homebrew/omlx--git/README",
    ] {
        write(&home.join(relative), "keep");
    }
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
        "PV eval/caches-preview",
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
            scratch: vec![sandbox.path().join(".local")],
        },
        "PV eval/caches-apply",
    );
    assert_journaled(&sandbox, "caches", "shred", &targets);
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
        ".cache/node",
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
    std::fs::remove_dir_all(home.join(".cache/node")).unwrap();
    write(
        &home.join(".cache/node/payload.bin"),
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
        std::fs::read_to_string(home.join(".cache/node/payload.bin"))
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

fn real_git() -> PathBuf {
    let resolved = std::process::Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(
        resolved.status.success(),
        "git is required for the project evals"
    );
    PathBuf::from(String::from_utf8(resolved.stdout).unwrap().trim())
}

/// Run fixture-building Git in `repo` as of `date`, isolated from the
/// developer's own Git configuration.
fn git(sandbox: &Sandbox, repo: &Path, date: &str, args: &[&str]) {
    let output = std::process::Command::new(real_git())
        .args([
            "-c",
            "user.name=devtrim-eval",
            "-c",
            "user.email=eval@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(repo)
        .env("HOME", sandbox.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
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
        git(sandbox, &hostile, STALE, &["config", &key, &value]);
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
        "PV eval/projects-preview",
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
            scratch: vec![sandbox.path().join(".local")],
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

#[test]
fn eval_purge_is_exactly_node_modules_plus_artifacts() {
    eval_project_category("eval-purge", &["purge"], None, |p| {
        let mut both = [p.node_modules.clone(), p.artifacts.clone()].concat();
        both.sort();
        both
    });
}

// ---------- read-only commands and previews change nothing ----------

/// Every report-only command and every preview, run against a fixture holding
/// targets for every category, leaves it byte for byte as it was — first
/// with no devtrim state at all, then with an existing journal.
#[test]
fn eval_read_only_commands_and_previews_change_nothing() {
    let sandbox = Sandbox::in_target("eval-read-only");
    caches_fixture(&sandbox);
    let projects = projects_fixture(&sandbox);
    write(&sandbox.path().join("Downloads/old.dmg"), "installer");
    let root = projects.dev.to_str().unwrap().to_owned();
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
        vec!["icloud", "--json"],
        vec!["history", "--json"],
        vec!["analyze", dev, "--json"],
        vec!["status", "--json"],
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
            run(&sandbox, args);
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
        "PV eval/xcode-preview",
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
            scratch: vec![sandbox.path().join(".local")],
        },
        "PV eval/xcode-apply",
    );
    assert!(apply.status.success(), "{document}");
    assert_journaled(&sandbox, "xcode", "shred", &expected);
}
