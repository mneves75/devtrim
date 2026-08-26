//! Op registry: every category is scan-then-apply with a danger score.

pub mod artifacts;
pub mod caches;
pub mod docker;
pub mod icloud;
pub mod leftovers;
pub mod node_modules;
pub(crate) mod project;
pub mod simulators;
pub mod toolchains;
pub mod xcode;

use anyhow::Result;
use std::path::Path;

pub use crate::report::{Action, Finding, Summary};
use crate::safety::{Ctx, VerifiedTarget};

pub use icloud::icloud_status;

pub trait Op {
    fn name(&self) -> &'static str;
    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>>;
    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome>;
}

#[derive(Debug)]
pub struct ApplyOutcome {
    pub summary: Summary,
    pub errors: Vec<String>,
}

impl ApplyOutcome {
    pub fn new(operation: &str) -> Self {
        Self {
            summary: Summary {
                op: operation.into(),
                items_touched: 0,
                bytes_freed_estimate: 0,
                notes: Vec::new(),
            },
            errors: Vec::new(),
        }
    }

    pub fn record(&mut self, finding: &Finding, note: String) {
        self.summary.items_touched = self.summary.items_touched.saturating_add(1);
        self.summary.bytes_freed_estimate = self
            .summary
            .bytes_freed_estimate
            .saturating_add(finding.size_bytes);
        self.summary.notes.push(note);
    }

    pub fn fail(&mut self, error: anyhow::Error) {
        self.errors.push(format!("{error:#}"));
    }
}
pub struct ScanResult {
    pub findings: Vec<Finding>,
    pub errors: Vec<String>,
}

pub fn all() -> Vec<Box<dyn Op>> {
    vec![
        Box::new(caches::Caches),
        Box::new(node_modules::NodeModules),
        Box::new(artifacts::Artifacts),
        Box::new(simulators::Simulators),
        Box::new(xcode::Xcode),
        Box::new(docker::Docker),
        Box::new(toolchains::Toolchains),
        Box::new(leftovers::Leftovers),
    ]
}

pub fn names() -> Vec<&'static str> {
    all().iter().map(|operation| operation.name()).collect()
}

pub fn by_name(name: &str) -> Option<Box<dyn Op>> {
    all().into_iter().find(|operation| operation.name() == name)
}

pub fn scan_all(ctx: &Ctx) -> ScanResult {
    let mut findings = Vec::new();
    let mut errors = Vec::new();
    for operation in all() {
        match operation.scan(ctx) {
            Ok(mut operation_findings) => {
                filter_protected_findings(&mut operation_findings, ctx);
                findings.append(&mut operation_findings);
            }
            Err(error) => errors.push(format!("{}: {error:#}", operation.name())),
        }
    }
    ScanResult { findings, errors }
}

pub fn dir_size(path: &Path) -> Result<u64> {
    crate::safety::dir_size(path)
}

pub fn filter_protected_findings(findings: &mut Vec<Finding>, ctx: &Ctx) {
    findings.retain(|finding| {
        let protected = finding
            .action
            .is_actionable()
            .then(|| finding.target())
            .flatten()
            .filter(|target| crate::safety::is_config_protected(target, &ctx.protect));
        if let Some(target) = protected {
            ctx.diagnostic(
                "info",
                format!("skipping protected path: {}", target.display()),
            );
            false
        } else {
            true
        }
    });
}

pub fn apply_filesystem_finding(op: &str, finding: &Finding, ctx: &Ctx) -> Result<()> {
    let (permanent, action) = match finding.action {
        Action::Trash => (false, "trash"),
        Action::Shred => (true, "shred"),
        _ => anyhow::bail!("refusing non-filesystem action at deletion sink"),
    };
    let target = finding
        .target()
        .ok_or_else(|| anyhow::anyhow!("filesystem finding missing internal target"))?;
    let attempt =
        crate::journal::JournalRecord::filesystem_attempt(op, action, target, finding.size_bytes);
    crate::journal::append(ctx, &attempt)?;
    let result = crate::safety::validate_path_for_deletion(target, &ctx.home, &ctx.protect)
        .and_then(|verified| remove_path(verified, permanent));
    crate::journal::finish(ctx, &attempt, result)
}

fn remove_path(target: VerifiedTarget, permanent: bool) -> Result<()> {
    let path = target.into_path();
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() {
        if permanent {
            std::fs::remove_file(&path)?;
        } else {
            trash::delete(&path)?;
        }
        return Ok(());
    }
    if permanent && metadata.is_dir() {
        std::fs::remove_dir_all(&path)?;
    } else if permanent {
        std::fs::remove_file(&path)?;
    } else {
        trash::delete(&path)?;
    }
    Ok(())
}

pub fn trash_findings(ctx: &Ctx) -> Result<Vec<Finding>> {
    let directory = crate::safety::validate_trash_root(&ctx.home)?;
    let mut entries = std::fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    entries
        .into_iter()
        .map(|entry| {
            let path = entry.path();
            let size = dir_size(&path)?;
            Ok(Finding::new(
                format!("Trash item: {}", entry.file_name().to_string_lossy()),
                Some(path),
                size,
                "permanent purge; Finder recovery is no longer available afterward",
                9,
                Action::Shred,
            ))
        })
        .collect()
}

pub fn purge_trash(findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
    let directory = crate::safety::validate_trash_root(&ctx.home)?;
    let mut outcome = ApplyOutcome::new("trash-empty");
    for finding in findings {
        let result = (|| -> Result<()> {
            if finding.action != Action::Shred {
                anyhow::bail!("refusing non-permanent Trash action");
            }
            let target = finding
                .target()
                .ok_or_else(|| anyhow::anyhow!("Trash finding missing internal target"))?;
            if target.parent() != Some(directory.as_path()) {
                anyhow::bail!("refusing target outside the previewed Trash root");
            }
            apply_filesystem_finding("trash-empty", finding, ctx)
        })();
        if let Err(error) = result {
            outcome.fail(error);
            break;
        }
        outcome.record(finding, format!("permanently deleted {}", finding.label));
    }
    Ok(outcome)
}

#[cfg(test)]
pub(crate) fn remove_test_path(path: impl AsRef<Path>) {
    let path = path.as_ref();
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_dir() {
        std::fs::remove_dir_all(path).ok();
    } else {
        std::fs::remove_file(path).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    use std::path::PathBuf;

    fn context(home: PathBuf) -> Ctx {
        Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: vec![],
            active_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home,
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        }
    }

    #[test]
    fn shared_sink_deletes_a_verified_normal_path() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-normal-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(home.join("dev/node_modules")).unwrap();
        let home = home.canonicalize().unwrap();
        let target = home.join("dev/node_modules");
        let finding = Finding::new(
            "node_modules",
            Some(target.clone()),
            0,
            "test",
            9,
            Action::Shred,
        );
        apply_filesystem_finding("test", &finding, &context(home.clone())).unwrap();
        assert!(!target.exists());
        let records = std::fs::read_to_string(home.join("journal.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records[0]["action"], "shred");
        assert_eq!(records[1]["status"], "ok");
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn shared_sink_uses_exact_non_utf8_target_identity() {
        let home = std::env::current_dir()
            .unwrap()
            .canonicalize()
            .unwrap()
            .join("target")
            .join(format!("devtrim-exact-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(&home).unwrap();
        let target = home.join(std::ffi::OsString::from_vec(vec![b'c', 0xff]));
        assert!(target.to_str().is_none());
        let finding = Finding::new(
            "non-UTF-8 cache",
            Some(target.clone()),
            0,
            "test",
            9,
            Action::Shred,
        );
        assert_eq!(finding.target(), Some(target.as_path()));
        match std::fs::create_dir_all(&target) {
            Ok(()) => {}
            Err(_) => {
                std::fs::remove_dir_all(home).ok();
                return;
            }
        }
        apply_filesystem_finding("test", &finding, &context(home.clone())).unwrap();
        assert!(!target.exists());
        let first = std::fs::read_to_string(home.join("journal.jsonl"))
            .unwrap()
            .lines()
            .next()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .unwrap();
        assert_eq!(first["target_lossy"], true);
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn shared_sink_rejects_missing_targets_and_non_filesystem_actions() {
        let home = std::env::current_dir()
            .unwrap()
            .canonicalize()
            .unwrap()
            .join("target")
            .join(format!("devtrim-reject-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(&home).unwrap();
        let target = home.join("sentinel");
        std::fs::write(&target, "keep").unwrap();
        let missing = Finding::new("missing", None, 0, "test", 9, Action::Shred);
        assert!(apply_filesystem_finding("test", &missing, &context(home.clone())).is_err());

        let command = Finding::new(
            "wrong action",
            Some(target.clone()),
            0,
            "test",
            1,
            Action::Info,
        );
        assert!(apply_filesystem_finding("test", &command, &context(home.clone())).is_err());
        assert!(target.exists());
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn shared_sink_rejects_symlinked_ancestor() {
        let home = std::env::temp_dir().join(format!("devtrim-sink-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(home.join("dev")).unwrap();
        std::fs::create_dir_all(home.join("Library/node_modules")).unwrap();
        symlink(home.join("Library"), home.join("dev/link")).unwrap();
        let target = home.join("dev/link/node_modules");
        let finding = Finding::new("node_modules", Some(target), 0, "test", 9, Action::Shred);
        assert!(apply_filesystem_finding("test", &finding, &context(home.clone())).is_err());
        assert!(home.join("Library/node_modules").exists());
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn purge_trash_refuses_symlinked_root_and_preserves_sentinel() {
        let home = std::env::temp_dir().join(format!("devtrim-purge-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        let outside = home.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let sentinel = outside.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        symlink(&outside, home.join(".Trash")).unwrap();

        assert!(purge_trash(&[], &context(home.clone())).is_err());
        assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "keep");

        std::fs::remove_file(home.join(".Trash")).ok();
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn purge_trash_consumes_only_exact_previewed_children() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-trash-plan-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        let trash = home.join(".Trash");
        std::fs::create_dir_all(&trash).unwrap();
        let home = home.canonicalize().unwrap();
        let trash = home.join(".Trash");
        let previewed = trash.join("previewed");
        let added_later = trash.join("added-later");
        std::fs::write(&previewed, "remove").unwrap();
        let ctx = context(home.clone());
        let findings = trash_findings(&ctx).unwrap();
        std::fs::write(&added_later, "keep").unwrap();

        let outcome = purge_trash(&findings, &ctx).unwrap();

        assert_eq!(outcome.summary.items_touched, 1);
        assert!(!previewed.exists());
        assert_eq!(std::fs::read_to_string(&added_later).unwrap(), "keep");
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn purge_trash_rejects_a_forged_target_outside_trash() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-trash-forged-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(home.join(".Trash")).unwrap();
        let home = home.canonicalize().unwrap();
        let sentinel = home.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let finding = Finding::new(
            "forged",
            Some(sentinel.clone()),
            4,
            "test",
            9,
            Action::Shred,
        );

        let outcome = purge_trash(&[finding], &context(home.clone())).unwrap();

        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "keep");
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn apply_outcome_size_saturates_instead_of_wrapping() {
        let finding = Finding::new("huge", None, u64::MAX, "test", 1, Action::Info);
        let mut outcome = ApplyOutcome::new("test");

        outcome.record(&finding, "first".into());
        outcome.record(&finding, "second".into());

        assert_eq!(outcome.summary.items_touched, 2);
        assert_eq!(outcome.summary.bytes_freed_estimate, u64::MAX);
    }

    #[test]
    fn journal_records_successful_trash_attempt_and_result() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-journal-trash-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(home.join("cache")).unwrap();
        let home = home.canonicalize().unwrap();
        let target = home.join("cache");
        let finding = Finding::new("cache", Some(target.clone()), 4, "test", 3, Action::Trash);
        let ctx = context(home.clone());

        let attempt = crate::journal::JournalRecord::filesystem_attempt(
            "caches",
            "trash",
            &target,
            finding.size_bytes,
        );
        crate::journal::append(&ctx, &attempt).unwrap();
        crate::journal::finish(&ctx, &attempt, Ok(())).unwrap();

        let records = std::fs::read_to_string(&ctx.journal_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["phase"], "attempt");
        assert_eq!(records[0]["action"], "trash");
        assert_eq!(records[1]["phase"], "result");
        assert_eq!(records[1]["status"], "ok");
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn journal_records_refused_deletion_as_error() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-journal-refused-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(home.join("protected/child")).unwrap();
        let home = home.canonicalize().unwrap();
        let target = home.join("protected/child");
        let sentinel = target.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let finding = Finding::new(
            "protected",
            Some(target.clone()),
            4,
            "test",
            9,
            Action::Shred,
        );
        let mut ctx = context(home.clone());
        ctx.protect = vec![home.join("PROTECTED")];

        let error = apply_filesystem_finding("test", &finding, &ctx).unwrap_err();

        assert!(error.to_string().contains("configured protected"));
        assert!(sentinel.exists());
        let records = std::fs::read_to_string(&ctx.journal_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0]["phase"], "attempt");
        assert_eq!(records[1]["status"], "error");
        assert!(records[1]["error"].as_str().unwrap().contains("protected"));
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn journal_write_failure_aborts_before_deletion() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-journal-unwritable-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(home.join("target")).unwrap();
        std::fs::write(home.join("journal-parent"), "not a directory").unwrap();
        let home = home.canonicalize().unwrap();
        let target = home.join("target");
        let sentinel = target.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let finding = Finding::new("target", Some(target), 4, "test", 9, Action::Shred);
        let mut ctx = context(home.clone());
        ctx.journal_path = home.join("journal-parent/journal.jsonl");

        let error = apply_filesystem_finding("test", &finding, &ctx).unwrap_err();

        assert!(error.to_string().contains("cannot write apply journal"));
        assert!(error.to_string().contains("journal.jsonl"));
        assert!(sentinel.exists());
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn preview_filter_drops_only_protected_actionable_findings() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-filter-protected-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(home.join("protected")).unwrap();
        let home = home.canonicalize().unwrap();
        let target = home.join("protected");
        let mut ctx = context(home.clone());
        ctx.protect = vec![target.clone()];
        let mut findings = vec![
            Finding::new(
                "actionable",
                Some(target.clone()),
                4,
                "test",
                3,
                Action::Trash,
            ),
            Finding::new("informational", Some(target), 4, "test", 0, Action::Info),
        ];

        filter_protected_findings(&mut findings, &ctx);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].label, "informational");
        assert!(
            ctx.take_diagnostics()
                .iter()
                .any(|message| message.contains("skipping protected path"))
        );
        crate::ops::remove_test_path(home);
    }
}
