//! Shared black-box harness: a disposable HOME and PATH per test, the real
//! binary, and an oracle that says what a run changed on disk.
#![allow(
    dead_code,
    reason = "each integration test crate uses its own subset of the harness"
)]

use serde_json::Value;
use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hasher};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);

pub struct Sandbox(PathBuf);

impl Sandbox {
    pub fn new(name: &str) -> Self {
        Self::new_in(std::env::temp_dir(), name)
    }

    pub fn new_in(base: PathBuf, name: &str) -> Self {
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = base.join(format!("devtrim-cli-{name}-{}-{id}", std::process::id()));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        let sandbox = Self(path);
        sandbox.script("pgrep", "exit 1");
        sandbox
    }

    // Permanent-deletion fixtures live here so same-device preflight matches the checkout.
    pub fn in_target(name: &str) -> Self {
        Self::new_in(std::env::current_dir().unwrap().join("target"), name)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    pub fn bin(&self) -> PathBuf {
        let path = self.0.join("bin");
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    pub fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.bin().join(name);
        std::fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        let mut permissions = std::fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).unwrap();
        path
    }

    /// The journal every apply in this sandbox writes.
    pub fn journal(&self) -> PathBuf {
        self.0.join(".local/state/devtrim/journal.jsonl")
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

pub fn run(sandbox: &Sandbox, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_devtrim"))
        .args(args)
        .env("HOME", sandbox.path())
        .env("PATH", sandbox.bin())
        .env_remove("XDG_STATE_HOME")
        .output()
        .unwrap()
}

pub fn json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON: {error}; stdout={}; stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

/// A `git` answering devtrim's two activity queries — HEAD's commit date, then
/// the newest HEAD reflog entry (`-g`) — with `commit` and `reflog`, and its
/// tracked-file check (`ls-files`) with nothing tracked.
pub fn git_activity(commit: &str, reflog: &str) -> String {
    format!(
        "case \"$*\" in\n  *ls-files*) ;;\n  *' -g '*) printf 'HEAD@{{{reflog}}}\\n' ;;\n  *) printf '{commit}\\n' ;;\nesac"
    )
}

/// What one filesystem entry looked like. Directory modification times are
/// left out on purpose: removing a child legitimately changes its parent's.
/// Access times are left out because reading a file to hash it changes them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    kind: &'static str,
    mode: u32,
    inode: u64,
    size: u64,
    mtime: Option<(i64, i64)>,
    content: Option<u64>,
    link: Option<PathBuf>,
}

/// Every entry under a root, observed without following a link.
#[derive(Debug, Clone)]
pub struct Tree {
    root: PathBuf,
    entries: BTreeMap<PathBuf, Entry>,
}

/// The only changes a run may make. Anything else is a violation.
#[derive(Debug, Default)]
pub struct Allowed {
    /// Paths that must exist before and be gone, with their whole subtree, after.
    pub removed: Vec<PathBuf>,
    /// Folders in which entries may appear, change or vanish (the journal,
    /// a stub's argv log). Their contents get their own assertions.
    pub scratch: Vec<PathBuf>,
}

impl Tree {
    /// Observe `root`. Any entry that cannot be observed fails the test:
    /// an incomplete snapshot would make the comparison prove nothing.
    pub fn snapshot(root: &Path) -> Self {
        let mut entries = BTreeMap::new();
        walk(root, &mut entries);
        Self {
            root: root.to_path_buf(),
            entries,
        }
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.entries.contains_key(path)
    }

    /// Compare against a later snapshot of the same root: `Err` names every
    /// change `allowed` does not cover, and every required removal that did
    /// not happen.
    pub fn diff(&self, after: &Self, allowed: &Allowed) -> Result<(), String> {
        assert_eq!(self.root, after.root, "snapshots of different roots");
        let under =
            |path: &Path, roots: &[PathBuf]| roots.iter().any(|root| path.starts_with(root));
        let mut problems = Vec::new();
        for removed in &allowed.removed {
            if !self.entries.contains_key(removed) {
                problems.push(format!(
                    "expected removal was absent before: {}",
                    removed.display()
                ));
            }
            if after.entries.keys().any(|path| path.starts_with(removed)) {
                problems.push(format!("expected removal survived: {}", removed.display()));
            }
        }
        for (path, before) in &self.entries {
            if under(path, &allowed.scratch) || under(path, &allowed.removed) {
                continue;
            }
            match after.entries.get(path) {
                None => problems.push(format!("removed without authority: {}", path.display())),
                Some(now) if now != before => problems.push(format!(
                    "changed: {}\n  before {before:?}\n  after  {now:?}",
                    path.display()
                )),
                Some(_) => {}
            }
        }
        // A directory created on the way to a scratch folder (`.local` and
        // `.local/state` before devtrim's own state folder) is allowed; a
        // file at such a path is not.
        let leads_to_scratch = |path: &Path, entry: &Entry| {
            entry.kind == "dir"
                && allowed
                    .scratch
                    .iter()
                    .any(|scratch| scratch.starts_with(path) && scratch != path)
        };
        for (path, entry) in &after.entries {
            if !self.entries.contains_key(path)
                && !under(path, &allowed.scratch)
                && !leads_to_scratch(path, entry)
            {
                problems.push(format!("created: {}", path.display()));
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(problems.join("\n"))
        }
    }

    /// Panics with every unexplained change, tagged with the boundary `tag`
    /// names so a planted violation is attributed to the assertion it broke.
    pub fn assert_only(&self, after: &Self, allowed: &Allowed, tag: &str) {
        if let Err(problems) = self.diff(after, allowed) {
            panic!("{tag}: the run changed more or less than it said:\n{problems}");
        }
    }
}

fn walk(path: &Path, entries: &mut BTreeMap<PathBuf, Entry>) {
    let metadata = std::fs::symlink_metadata(path)
        .unwrap_or_else(|error| panic!("cannot observe {}: {error}", path.display()));
    let file_type = metadata.file_type();
    let (kind, content, link) = if file_type.is_symlink() {
        let target = std::fs::read_link(path)
            .unwrap_or_else(|error| panic!("cannot read link {}: {error}", path.display()));
        ("symlink", None, Some(target))
    } else if file_type.is_file() {
        let bytes = std::fs::read(path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
        let mut hasher = DefaultHasher::new();
        hasher.write(&bytes);
        ("file", Some(hasher.finish()), None)
    } else if file_type.is_dir() {
        ("dir", None, None)
    } else {
        // A FIFO or device is never opened: reading one could block or consume.
        ("special", None, None)
    };
    let mtime = file_type
        .is_file()
        .then(|| (metadata.mtime(), metadata.mtime_nsec()));
    entries.insert(
        path.to_path_buf(),
        Entry {
            kind,
            mode: metadata.mode(),
            inode: metadata.ino(),
            size: if file_type.is_dir() {
                0
            } else {
                metadata.size()
            },
            mtime,
            content,
            link,
        },
    );
    if file_type.is_dir() {
        let children = std::fs::read_dir(path)
            .unwrap_or_else(|error| panic!("cannot list {}: {error}", path.display()));
        for child in children {
            let child =
                child.unwrap_or_else(|error| panic!("cannot list {}: {error}", path.display()));
            walk(&child.path(), entries);
        }
    }
}

/// One journal line, as the eval reads it.
#[derive(Debug, Clone)]
pub struct JournalLine {
    pub id: String,
    pub phase: String,
    pub op: String,
    pub action: String,
    pub target: Option<String>,
    pub status: Option<String>,
    pub argv: Option<Vec<String>>,
}

pub fn journal_lines(sandbox: &Sandbox) -> Vec<JournalLine> {
    let text = std::fs::read_to_string(sandbox.journal()).unwrap_or_default();
    text.lines()
        .map(|line| {
            let value: Value = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("malformed journal line {line}: {error}"));
            let text = |key: &str| value[key].as_str().map(str::to_owned);
            JournalLine {
                id: text("id").unwrap_or_default(),
                phase: text("phase").unwrap_or_default(),
                op: text("op").unwrap_or_default(),
                action: text("action").unwrap_or_default(),
                target: text("target"),
                status: text("status"),
                argv: value["argv"].as_array().map(|argv| {
                    argv.iter()
                        .map(|arg| arg.as_str().unwrap_or_default().to_owned())
                        .collect()
                }),
            }
        })
        .collect()
}

/// Every target was journaled write-ahead: an attempt, then later a result
/// with the same id, the same action and status `ok`; and nothing else.
pub fn assert_journaled(sandbox: &Sandbox, op: &str, action: &str, targets: &[PathBuf]) {
    let lines = journal_lines(sandbox);
    let mut expected: Vec<String> = targets.iter().map(|t| t.display().to_string()).collect();
    expected.sort();
    let mut seen = Vec::new();
    for (index, attempt) in lines.iter().enumerate() {
        if attempt.phase != "attempt" {
            continue;
        }
        let result = lines[index + 1..]
            .iter()
            .find(|line| line.phase == "result" && line.id == attempt.id)
            .unwrap_or_else(|| panic!("EVAL journal: attempt without a later result: {attempt:?}"));
        assert_eq!(attempt.op, op, "EVAL journal: {attempt:?}");
        assert_eq!(attempt.action, action, "EVAL journal: {attempt:?}");
        assert_eq!(result.action, attempt.action, "EVAL journal: {result:?}");
        assert_eq!(
            result.status.as_deref(),
            Some("ok"),
            "EVAL journal: {result:?}"
        );
        seen.push(attempt.target.clone().unwrap_or_default());
    }
    seen.sort();
    assert_eq!(
        seen, expected,
        "EVAL journal: journaled targets differ from the plan"
    );
}

/// The filesystem targets a preview or apply document plans to remove, sorted,
/// duplicates kept. Display text, read only to compare with the fixture.
pub fn actionable_targets(document: &Value) -> Vec<PathBuf> {
    let mut targets: Vec<PathBuf> = document["findings"]
        .as_array()
        .unwrap_or_else(|| panic!("no findings array: {document}"))
        .iter()
        .filter(|finding| matches!(finding["action"]["type"].as_str(), Some("trash" | "shred")))
        .filter_map(|finding| finding["path"].as_str().map(PathBuf::from))
        .collect();
    targets.sort();
    targets
}

/// devtrim attached to a real terminal through macOS `script(1)`, so it sees
/// an interactive stdin and stdout and asks for confirmation. The test can
/// change the fixture while devtrim waits at the prompt — after the preview,
/// before the apply — which separate preview and apply runs cannot do.
pub struct Interactive {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    output: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

impl Interactive {
    pub fn start(sandbox: &Sandbox, args: &[&str]) -> Self {
        use std::io::Read;
        let mut child = Command::new("/usr/bin/script")
            .args(["-q", "/dev/null", env!("CARGO_BIN_EXE_devtrim")])
            .args(args)
            .env("HOME", sandbox.path())
            .env("PATH", sandbox.bin())
            .env("NO_COLOR", "1")
            .env_remove("XDG_STATE_HOME")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let output = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut stdout = child.stdout.take().unwrap();
        let sink = std::sync::Arc::clone(&output);
        std::thread::spawn(move || {
            let mut buffer = [0u8; 4096];
            while let Ok(count) = stdout.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                sink.lock().unwrap().extend_from_slice(&buffer[..count]);
            }
        });
        let stdin = child.stdin.take();
        Self {
            child,
            stdin,
            output,
        }
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.output.lock().unwrap()).into_owned()
    }

    /// Block until devtrim has printed `needle`, or fail after 30 seconds.
    pub fn wait_for(&mut self, needle: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !self.text().contains(needle) {
            if std::time::Instant::now() > deadline || self.child.try_wait().unwrap().is_some() {
                let _ = self.child.kill();
                panic!("devtrim never printed {needle:?}; output:\n{}", self.text());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    pub fn send(&mut self, line: &str) {
        use std::io::Write;
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(format!("{line}\n").as_bytes()).unwrap();
        stdin.flush().unwrap();
    }

    /// Wait for exit; stdin stays open until then, because `script(1)` turns
    /// its closing into an end-of-file that could reach a pending read.
    pub fn finish(mut self) -> (std::process::ExitStatus, String) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if std::time::Instant::now() > deadline {
                let _ = self.child.kill();
                panic!("devtrim did not exit; output:\n{}", self.text());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        drop(self.stdin.take());
        // Let the reader drain what the process wrote last.
        std::thread::sleep(std::time::Duration::from_millis(100));
        (status, self.text())
    }
}
