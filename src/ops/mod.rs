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

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub use crate::report::{Action, Finding, Summary};
use crate::safety::{Ctx, FileIdentity, VerifiedTarget};

static QUARANTINE_ATTEMPT: AtomicU64 = AtomicU64::new(0);

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
    let attempt = crate::journal::begin(
        ctx,
        crate::journal::JournalRecord::filesystem_attempt(op, action, target, finding.size_bytes),
    )
    .with_context(|| format!("cannot write apply journal: {}", ctx.journal_path.display()))?;
    let result = crate::safety::validate_path_for_deletion(target, &ctx.home, &ctx.protect)
        .and_then(|verified| {
            let expected = finding
                .identity()
                .ok_or_else(|| anyhow::anyhow!("finding lacks preview identity"))?;
            remove_path(verified, permanent, expected)
        });
    attempt.finish(ctx, result)
}

fn remove_path(target: VerifiedTarget, permanent: bool, expected: FileIdentity) -> Result<()> {
    let path = target.into_path();
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("refusing target without parent: {}", path.display()))?;
    let leaf = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("refusing target without leaf: {}", path.display()))?;
    let leaf = Path::new(leaf);

    // Validation rejected symlinked ancestors. The open parent anchors identity
    // checks and permanent deletion; Trash remains the documented path-based
    // exception.
    let dir = cap_std::fs::Dir::open_ambient_dir(parent, cap_std::ambient_authority())
        .with_context(|| format!("cannot open target parent: {}", parent.display()))?;
    let actual = file_identity_at(&dir, leaf)
        .with_context(|| format!("cannot inspect deletion target: {}", path.display()))?;
    if actual != expected {
        anyhow::bail!("target identity changed after preview; refusing");
    }

    let deletion_device = if permanent {
        let parent_identity = file_identity_for_dir(&dir)
            .with_context(|| format!("cannot inspect target parent: {}", parent.display()))?;
        ensure_same_device(actual, parent_identity.dev, &path)?;
        parent_identity.dev
    } else {
        expected.dev
    };

    if !permanent {
        let metadata = dir
            .symlink_metadata(leaf)
            .with_context(|| format!("cannot inspect deletion target: {}", path.display()))?;
        if metadata.is_dir() {
            let target_dir = dir
                .open_dir(leaf)
                .with_context(|| format!("cannot open deletion target: {}", path.display()))?;
            let handle_identity = file_identity_for_dir(&target_dir)
                .with_context(|| format!("cannot inspect deletion target: {}", path.display()))?;
            if handle_identity != expected {
                anyhow::bail!(
                    "directory identity changed during Trash preflight: {}",
                    path.display()
                );
            }
            preflight_same_device_tree(&target_dir, deletion_device, &path)
                .context("Trash deletion preflight failed")?;
        }
        let final_identity = file_identity_at(&dir, leaf)
            .with_context(|| format!("cannot recheck deletion target: {}", path.display()))?;
        if final_identity != expected {
            anyhow::bail!(
                "target identity changed during Trash preflight: {}",
                path.display()
            );
        }
        // macOS Trash has no descriptor-relative API; a residual rename window
        // remains after this final parent-anchored identity check.
        trash::delete(&path)?;
        return Ok(());
    }

    let quarantine_name = next_quarantine_name(&dir)?;
    let quarantine_path = parent.join(&quarantine_name);
    dir.rename(leaf, &dir, &quarantine_name)
        .with_context(|| format!("cannot quarantine deletion target: {}", path.display()))?;
    let quarantined =
        verify_quarantined_target(&dir, leaf, &quarantine_name, &quarantine_path, expected)?;
    if quarantined.is_dir() {
        // Bind the recursive deletion to the verified object itself: open the
        // quarantined directory, re-verify identity on the open handle, and
        // delete through that handle. Even a raced rename of the quarantine
        // name can no longer redirect the recursion to a different tree.
        let target_dir = match dir.open_dir(&quarantine_name) {
            Ok(target_dir) => target_dir,
            Err(error) => {
                if let Err(restore_error) =
                    restore_quarantined_target(&dir, leaf, &quarantine_name, &quarantine_path)
                {
                    anyhow::bail!(
                        "cannot open quarantined directory {}; restore failed and nothing was deleted: {restore_error:#} (open error: {error})",
                        quarantine_path.display()
                    );
                }
                anyhow::bail!(
                    "cannot open quarantined directory {}; entry was restored: {error}",
                    quarantine_path.display()
                );
            }
        };
        let mut removal_started = false;
        let removal_result = (|| -> Result<()> {
            let handle_identity = file_identity_for_dir(&target_dir).with_context(|| {
                format!(
                    "cannot verify quarantined directory handle: {}",
                    quarantine_path.display()
                )
            })?;
            if handle_identity != expected {
                anyhow::bail!("quarantined directory identity changed");
            }
            preflight_same_device_tree(&target_dir, deletion_device, &quarantine_path)
                .context("permanent deletion preflight failed")?;
            struct RemovalFrame {
                dir: cap_std::fs::Dir,
                path: PathBuf,
                identity: FileIdentity,
                names: Vec<std::ffi::OsString>,
                next: usize,
            }
            enum RemovalStep {
                Continue,
                Descend(RemovalFrame),
                Finish,
            }

            let root_identity = file_identity_for_dir(&target_dir).with_context(|| {
                format!(
                    "cannot inspect open directory: {}",
                    quarantine_path.display()
                )
            })?;
            ensure_same_device(root_identity, deletion_device, &quarantine_path)?;
            refuse_git_repository_root_handle(&target_dir, &quarantine_path)?;
            let names = directory_entry_names(&target_dir, &quarantine_path)?;
            if names.iter().any(|name| name == ".git") {
                anyhow::bail!(
                    "refusing Git repository/worktree root: {}",
                    quarantine_path.display()
                );
            }
            let mut frames = vec![RemovalFrame {
                dir: target_dir,
                path: quarantine_path.clone(),
                identity: root_identity,
                names,
                next: 0,
            }];
            loop {
                let step = {
                    let Some(frame) = frames.last_mut() else {
                        break;
                    };
                    if frame.next < frame.names.len() {
                        let name = frame.names[frame.next].clone();
                        frame.next += 1;
                        let child_path = frame.path.join(&name);
                        let metadata = frame.dir.symlink_metadata(&name).with_context(|| {
                            format!("cannot inspect entry: {}", child_path.display())
                        })?;
                        let entry_identity = file_identity_at(&frame.dir, Path::new(&name))
                            .with_context(|| {
                                format!("cannot inspect entry identity: {}", child_path.display())
                            })?;
                        ensure_same_device(entry_identity, deletion_device, &child_path)?;
                        if metadata.is_dir() {
                            let child = frame.dir.open_dir(&name).with_context(|| {
                                format!("cannot open directory: {}", child_path.display())
                            })?;
                            let opened_identity =
                                file_identity_for_dir(&child).with_context(|| {
                                    format!(
                                        "cannot inspect open directory: {}",
                                        child_path.display()
                                    )
                                })?;
                            ensure_same_device(opened_identity, deletion_device, &child_path)?;
                            if opened_identity != entry_identity {
                                anyhow::bail!(
                                    "directory identity changed during permanent deletion: {}",
                                    child_path.display()
                                );
                            }
                            refuse_git_repository_root_handle(&child, &child_path)?;
                            let names = directory_entry_names(&child, &child_path)?;
                            RemovalStep::Descend(RemovalFrame {
                                dir: child,
                                path: child_path,
                                identity: opened_identity,
                                names,
                                next: 0,
                            })
                        } else {
                            let current_identity = file_identity_at(&frame.dir, Path::new(&name))
                                .with_context(|| {
                                format!("cannot recheck entry identity: {}", child_path.display())
                            })?;
                            ensure_same_device(current_identity, deletion_device, &child_path)?;
                            if current_identity != entry_identity {
                                anyhow::bail!(
                                    "entry identity changed during permanent deletion: {}",
                                    child_path.display()
                                );
                            }
                            removal_started = true;
                            frame.dir.remove_file(&name).with_context(|| {
                                format!("cannot delete entry: {}", child_path.display())
                            })?;
                            RemovalStep::Continue
                        }
                    } else {
                        refuse_git_repository_root_handle(&frame.dir, &frame.path)?;
                        let final_identity =
                            file_identity_for_dir(&frame.dir).with_context(|| {
                                format!("cannot recheck open directory: {}", frame.path.display())
                            })?;
                        ensure_same_device(final_identity, deletion_device, &frame.path)?;
                        if final_identity != frame.identity {
                            anyhow::bail!(
                                "directory identity changed during permanent deletion: {}",
                                frame.path.display()
                            );
                        }
                        RemovalStep::Finish
                    }
                };
                match step {
                    RemovalStep::Continue => {}
                    RemovalStep::Descend(frame) => frames.push(frame),
                    RemovalStep::Finish => {
                        let Some(frame) = frames.pop() else {
                            anyhow::bail!("deletion traversal stack unexpectedly empty");
                        };
                        removal_started = true;
                        frame.dir.remove_open_dir().with_context(|| {
                            format!("cannot delete open directory: {}", frame.path.display())
                        })?;
                    }
                }
            }
            Ok(())
        })();
        if let Err(error) = removal_result {
            if removal_started {
                return Err(error);
            }
            if let Err(restore_error) =
                restore_quarantined_target(&dir, leaf, &quarantine_name, &quarantine_path)
            {
                anyhow::bail!(
                    "permanent deletion preparation refused {}; restore failed and nothing was deleted: {restore_error:#} (preparation error: {error:#})",
                    quarantine_path.display()
                );
            }
            anyhow::bail!(
                "permanent deletion preparation refused {}; entry was restored: {error:#}",
                quarantine_path.display()
            );
        }
    } else {
        // macOS has no fd-relative unlink, so the final single-entry removal is
        // by the private, unpredictable quarantine name after re-verification.
        dir.remove_file(&quarantine_name).with_context(|| {
            format!(
                "cannot delete quarantined entry: {}",
                quarantine_path.display()
            )
        })?;
    }
    Ok(())
}

fn file_identity_at(dir: &cap_std::fs::Dir, path: &Path) -> Result<FileIdentity> {
    let metadata = rustix::fs::statat(dir, path, rustix::fs::AtFlags::SYMLINK_NOFOLLOW)?;
    Ok(FileIdentity::from_rustix_stat(&metadata))
}

fn file_identity_for_dir(dir: &cap_std::fs::Dir) -> Result<FileIdentity> {
    let metadata = rustix::fs::fstat(dir)?;
    Ok(FileIdentity::from_rustix_stat(&metadata))
}

fn ensure_same_device(identity: FileIdentity, expected_device: u64, path: &Path) -> Result<()> {
    if identity.dev != expected_device {
        anyhow::bail!(
            "refusing foreign filesystem device at {} (device {}, expected {})",
            path.display(),
            identity.dev,
            expected_device
        );
    }
    Ok(())
}

fn refuse_git_repository_root_handle(dir: &cap_std::fs::Dir, path: &Path) -> Result<()> {
    match dir.symlink_metadata(Path::new(".git")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => anyhow::bail!("refusing Git repository/worktree root: {}", path.display()),
        Err(error) => Err(error)
            .with_context(|| format!("cannot inspect Git marker under {}", path.display())),
    }
}

fn directory_entry_names(dir: &cap_std::fs::Dir, path: &Path) -> Result<Vec<std::ffi::OsString>> {
    dir.entries()
        .with_context(|| format!("cannot read directory: {}", path.display()))?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name())
                .with_context(|| format!("cannot read directory entry under {}", path.display()))
        })
        .collect()
}

fn preflight_same_device_tree(
    dir: &cap_std::fs::Dir,
    expected_device: u64,
    path: &Path,
) -> Result<()> {
    let root_identity = file_identity_for_dir(dir)
        .with_context(|| format!("cannot inspect open directory: {}", path.display()))?;
    ensure_same_device(root_identity, expected_device, path)?;
    refuse_git_repository_root_handle(dir, path)?;

    for name in directory_entry_names(dir, path)? {
        if name == ".git" {
            anyhow::bail!("refusing Git repository/worktree root: {}", path.display());
        }
        let child_path = path.join(&name);
        let metadata = dir
            .symlink_metadata(&name)
            .with_context(|| format!("cannot inspect entry: {}", child_path.display()))?;
        let entry_identity = file_identity_at(dir, Path::new(&name))
            .with_context(|| format!("cannot inspect entry identity: {}", child_path.display()))?;
        ensure_same_device(entry_identity, expected_device, &child_path)?;
        if metadata.is_dir() {
            let child = dir
                .open_dir(&name)
                .with_context(|| format!("cannot open directory: {}", child_path.display()))?;
            let opened_identity = file_identity_for_dir(&child).with_context(|| {
                format!("cannot inspect open directory: {}", child_path.display())
            })?;
            ensure_same_device(opened_identity, expected_device, &child_path)?;
            if opened_identity != entry_identity {
                anyhow::bail!(
                    "directory identity changed during deletion preflight: {}",
                    child_path.display()
                );
            }
            preflight_same_device_tree(&child, expected_device, &child_path)?;
        }
    }
    Ok(())
}

fn next_quarantine_name(dir: &cap_std::fs::Dir) -> Result<PathBuf> {
    for _ in 0..128 {
        let attempt = QUARANTINE_ATTEMPT.fetch_add(1, Ordering::Relaxed);
        // RandomState carries process-random SipHash keys, making the name
        // unpredictable to a concurrent process without adding a dependency.
        let token = {
            use std::hash::{BuildHasher, Hasher};
            std::collections::hash_map::RandomState::new()
                .build_hasher()
                .finish()
        };
        let candidate = PathBuf::from(format!(
            ".devtrim-quarantine-{}-{token:016x}-{attempt}",
            std::process::id()
        ));
        match dir.symlink_metadata(&candidate) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "cannot inspect quarantine name candidate: {}",
                        candidate.display()
                    )
                });
            }
        }
    }
    anyhow::bail!("cannot allocate a private quarantine name")
}

fn verify_quarantined_target(
    dir: &cap_std::fs::Dir,
    leaf: &Path,
    quarantine_name: &Path,
    quarantine_path: &Path,
    expected: FileIdentity,
) -> Result<cap_std::fs::Metadata> {
    let metadata = match dir.symlink_metadata(quarantine_name) {
        Ok(metadata) => metadata,
        Err(error) => {
            if let Err(restore_error) =
                restore_quarantined_target(dir, leaf, quarantine_name, quarantine_path)
            {
                anyhow::bail!(
                    "cannot inspect quarantined target {}; restore failed and nothing was deleted: {restore_error:#}",
                    quarantine_path.display()
                );
            }
            return Err(error).with_context(|| {
                format!(
                    "cannot inspect quarantined target: {}",
                    quarantine_path.display()
                )
            });
        }
    };
    let actual = match file_identity_at(dir, quarantine_name) {
        Ok(identity) => identity,
        Err(error) => {
            if let Err(restore_error) =
                restore_quarantined_target(dir, leaf, quarantine_name, quarantine_path)
            {
                anyhow::bail!(
                    "cannot inspect quarantined target identity {}; restore failed and nothing was deleted: {restore_error:#} (identity error: {error:#})",
                    quarantine_path.display()
                );
            }
            anyhow::bail!(
                "cannot inspect quarantined target identity {}; entry was restored: {error:#}",
                quarantine_path.display()
            );
        }
    };
    if actual == expected {
        return Ok(metadata);
    }

    if let Err(restore_error) =
        restore_quarantined_target(dir, leaf, quarantine_name, quarantine_path)
    {
        anyhow::bail!(
            "target identity changed after quarantine; cannot restore {}; nothing was deleted: {restore_error:#}",
            quarantine_path.display()
        );
    }
    anyhow::bail!("target identity changed after quarantine; restored original name and refusing")
}

fn restore_quarantined_target(
    dir: &cap_std::fs::Dir,
    leaf: &Path,
    quarantine_name: &Path,
    quarantine_path: &Path,
) -> Result<()> {
    match dir.symlink_metadata(leaf) {
        Ok(_) => anyhow::bail!(
            "cannot restore {} because the original name is occupied",
            quarantine_path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "cannot verify the original name before restoring {}",
                    quarantine_path.display()
                )
            });
        }
    }
    dir.rename(quarantine_name, dir, leaf).with_context(|| {
        format!(
            "cannot restore quarantined target {}; nothing was deleted",
            quarantine_path.display()
        )
    })
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
    use std::process::Command;

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
    fn permanent_sink_quarantines_and_deletes_a_verified_normal_path() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-normal-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(home.join("dev/node_modules/nested")).unwrap();
        std::fs::write(home.join("dev/node_modules/nested/payload"), "delete").unwrap();
        std::fs::create_dir_all(home.join("dev/outside")).unwrap();
        std::fs::write(home.join("dev/outside/sentinel"), "keep").unwrap();
        symlink(home.join("dev/outside"), home.join("dev/node_modules/link")).unwrap();
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
        assert_eq!(
            std::fs::read_to_string(home.join("dev/outside/sentinel")).unwrap(),
            "keep"
        );
        assert!(std::fs::read_dir(home.join("dev")).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".devtrim-quarantine-")
        }));
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
    fn permanent_sink_rechecks_git_marker_after_validation() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-git-recheck-{}", std::process::id()));
        remove_test_path(&home);
        let target = home.join("dev/cache");
        std::fs::create_dir_all(&target).unwrap();
        let home = home.canonicalize().unwrap();
        let target = home.join("dev/cache");
        let finding = Finding::new("cache", Some(target.clone()), 0, "test", 9, Action::Shred);
        let expected = finding.identity().unwrap();
        let verified = crate::safety::validate_path_for_deletion(&target, &home, &[]).unwrap();
        std::fs::write(target.join(".git"), "gitdir: elsewhere\n").unwrap();

        let error = remove_path(verified, true, expected).unwrap_err();

        assert!(error.to_string().contains("Git repository/worktree root"));
        assert_eq!(
            std::fs::read_to_string(target.join(".git")).unwrap(),
            "gitdir: elsewhere\n"
        );
        assert!(std::fs::read_dir(home.join("dev")).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".devtrim-quarantine-")
        }));
        remove_test_path(home);
    }

    #[test]
    fn trash_sink_refuses_nested_git_worktree_before_mutation() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-trash-nested-git-{}", std::process::id()));
        remove_test_path(&home);
        let target = home.join("dev/cache");
        let nested = target.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            nested.join(".git"),
            "gitdir: ../repository/.git/worktrees/nested\n",
        )
        .unwrap();
        let sentinel = target.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let home = home.canonicalize().unwrap();
        let target = home.join("dev/cache");
        let finding = Finding::new("cache", Some(target.clone()), 0, "test", 5, Action::Trash);
        let expected = finding.identity().unwrap();
        let verified = crate::safety::validate_path_for_deletion(&target, &home, &[]).unwrap();

        let error = remove_path(verified, false, expected).unwrap_err();

        assert!(format!("{error:#}").contains("Git repository/worktree root"));
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "keep");
        assert_eq!(
            std::fs::read_to_string(target.join("nested/.git")).unwrap(),
            "gitdir: ../repository/.git/worktrees/nested\n"
        );
        assert!(target.is_dir());
        remove_test_path(home);
    }

    #[test]
    fn permanent_sink_refuses_nested_git_worktree_before_deletion() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-nested-git-{}", std::process::id()));
        remove_test_path(&home);
        let target = home.join("dev/cache");
        let nested = target.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(
            nested.join(".git"),
            "gitdir: ../repository/.git/worktrees/nested\n",
        )
        .unwrap();
        let sentinel = target.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let home = home.canonicalize().unwrap();
        let target = home.join("dev/cache");
        let finding = Finding::new("cache", Some(target.clone()), 0, "test", 9, Action::Shred);
        let expected = finding.identity().unwrap();
        let verified = crate::safety::validate_path_for_deletion(&target, &home, &[]).unwrap();

        let error = remove_path(verified, true, expected).unwrap_err();

        assert!(error.to_string().contains("Git repository/worktree root"));
        assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "keep");
        assert_eq!(
            std::fs::read_to_string(target.join("nested/.git")).unwrap(),
            "gitdir: ../repository/.git/worktrees/nested\n"
        );
        assert!(target.is_dir());
        assert!(std::fs::read_dir(home.join("dev")).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".devtrim-quarantine-")
        }));
        remove_test_path(home);
    }

    #[test]
    fn same_device_preflight_refuses_a_foreign_directory_before_deletion() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-foreign-device-{}", std::process::id()));
        remove_test_path(&home);
        let target = home.join("cache");
        std::fs::create_dir_all(&target).unwrap();
        let sentinel = target.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let home = home.canonicalize().unwrap();
        let target = home.join("cache");
        let target_dir =
            cap_std::fs::Dir::open_ambient_dir(&target, cap_std::ambient_authority()).unwrap();
        let actual_device = file_identity_for_dir(&target_dir).unwrap().dev;

        let error = preflight_same_device_tree(&target_dir, actual_device.wrapping_add(1), &target)
            .unwrap_err();

        assert!(error.to_string().contains("foreign filesystem device"));
        assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "keep");
        remove_test_path(home);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn preview_and_handle_relative_identity_include_the_same_generation() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-generation-{}", std::process::id()));
        remove_test_path(&home);
        std::fs::create_dir_all(&home).unwrap();
        let target = home.join("cache");
        std::fs::write(&target, "content").unwrap();
        let home = home.canonicalize().unwrap();
        let target = home.join("cache");
        let preview = Finding::new("cache", Some(target), 0, "test", 9, Action::Shred)
            .identity()
            .unwrap();
        let dir = cap_std::fs::Dir::open_ambient_dir(&home, cap_std::ambient_authority()).unwrap();

        let handle_relative = file_identity_at(&dir, Path::new("cache")).unwrap();

        assert_eq!(preview, handle_relative);
        assert_eq!(preview.generation, handle_relative.generation);
        remove_test_path(home);
    }

    #[test]
    fn quarantine_identity_mismatch_restores_the_original_name() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-quarantine-restore-{}", std::process::id()));
        remove_test_path(&home);
        std::fs::create_dir_all(home.join("dev")).unwrap();
        let home = home.canonicalize().unwrap();
        let parent = home.join("dev");
        let target = parent.join("cache");
        std::fs::write(&target, "original").unwrap();
        let expected = Finding::new("cache", Some(target.clone()), 0, "test", 9, Action::Shred)
            .identity()
            .unwrap();
        let mismatched = FileIdentity {
            ino: expected.ino.wrapping_add(1),
            ..expected
        };
        let dir =
            cap_std::fs::Dir::open_ambient_dir(&parent, cap_std::ambient_authority()).unwrap();
        let leaf = Path::new("cache");
        let quarantine_name = PathBuf::from(format!(
            ".devtrim-quarantine-test-restore-{}",
            std::process::id()
        ));
        let quarantine_path = parent.join(&quarantine_name);
        dir.rename(leaf, &dir, &quarantine_name).unwrap();

        let error =
            verify_quarantined_target(&dir, leaf, &quarantine_name, &quarantine_path, mismatched)
                .unwrap_err();

        assert!(error.to_string().contains("identity changed"));
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        assert!(std::fs::symlink_metadata(&quarantine_path).is_err());
        remove_test_path(home);
    }

    #[test]
    fn quarantine_restore_failure_preserves_both_names_and_reports_quarantine_path() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-quarantine-held-{}", std::process::id()));
        remove_test_path(&home);
        std::fs::create_dir_all(home.join("dev")).unwrap();
        let home = home.canonicalize().unwrap();
        let parent = home.join("dev");
        let target = parent.join("cache");
        std::fs::write(&target, "quarantined").unwrap();
        let expected = Finding::new("cache", Some(target.clone()), 0, "test", 9, Action::Shred)
            .identity()
            .unwrap();
        let mismatched = FileIdentity {
            ino: expected.ino.wrapping_add(1),
            ..expected
        };
        let dir =
            cap_std::fs::Dir::open_ambient_dir(&parent, cap_std::ambient_authority()).unwrap();
        let leaf = Path::new("cache");
        let quarantine_name = PathBuf::from(format!(
            ".devtrim-quarantine-test-held-{}",
            std::process::id()
        ));
        let quarantine_path = parent.join(&quarantine_name);
        dir.rename(leaf, &dir, &quarantine_name).unwrap();
        std::fs::write(&target, "replacement").unwrap();

        let error =
            verify_quarantined_target(&dir, leaf, &quarantine_name, &quarantine_path, mismatched)
                .unwrap_err();

        assert!(
            error
                .to_string()
                .contains(&quarantine_path.to_string_lossy().into_owned())
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "replacement");
        assert_eq!(
            std::fs::read_to_string(&quarantine_path).unwrap(),
            "quarantined"
        );
        remove_test_path(home);
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
        match std::fs::create_dir_all(&target) {
            Ok(()) => {}
            Err(_) => {
                std::fs::remove_dir_all(home).ok();
                return;
            }
        }
        let finding = Finding::new(
            "non-UTF-8 cache",
            Some(target.clone()),
            0,
            "test",
            9,
            Action::Shred,
        );
        assert_eq!(finding.target(), Some(target.as_path()));
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
    fn shared_sink_refuses_directory_identity_swap() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-dir-swap-{}", std::process::id()));
        remove_test_path(&home);
        let target = home.join("dev/cache");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("original"), "keep original").unwrap();
        let home = home.canonicalize().unwrap();
        let target = home.join("dev/cache");
        let moved = home.join("dev/cache-moved");
        let finding = Finding::new("cache", Some(target.clone()), 0, "test", 9, Action::Shred);
        std::fs::rename(&target, &moved).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        let sentinel = target.join("sentinel");
        std::fs::write(&sentinel, "keep replacement").unwrap();

        let error = apply_filesystem_finding("test", &finding, &context(home.clone())).unwrap_err();

        assert!(error.to_string().contains("identity changed"));
        assert_eq!(
            std::fs::read_to_string(sentinel).unwrap(),
            "keep replacement"
        );
        assert_eq!(
            std::fs::read_to_string(moved.join("original")).unwrap(),
            "keep original"
        );
        remove_test_path(home);
    }

    #[test]
    fn shared_sink_refuses_file_swap_to_symlink() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-file-swap-{}", std::process::id()));
        remove_test_path(&home);
        std::fs::create_dir_all(home.join("dev")).unwrap();
        std::fs::create_dir_all(home.join("Library")).unwrap();
        let target = home.join("dev/cache-file");
        let protected = home.join("Library/protected");
        std::fs::write(&target, "previewed").unwrap();
        std::fs::write(&protected, "keep protected").unwrap();
        let home = home.canonicalize().unwrap();
        let target = home.join("dev/cache-file");
        let protected = home.join("Library/protected");
        let finding = Finding::new(
            "cache file",
            Some(target.clone()),
            0,
            "test",
            9,
            Action::Shred,
        );
        std::fs::remove_file(&target).unwrap();
        symlink(&protected, &target).unwrap();

        let error = apply_filesystem_finding("test", &finding, &context(home.clone())).unwrap_err();

        assert!(error.to_string().contains("identity changed"));
        assert!(
            std::fs::symlink_metadata(&target)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(protected).unwrap(),
            "keep protected"
        );
        remove_test_path(home);
    }

    #[test]
    fn shared_sink_refuses_finding_without_preview_identity() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-missing-identity-{}", std::process::id()));
        remove_test_path(&home);
        std::fs::create_dir_all(home.join("dev")).unwrap();
        let home = home.canonicalize().unwrap();
        let target = home.join("dev/missing");
        let finding = Finding::new("missing", Some(target), 0, "test", 9, Action::Shred);

        let error = apply_filesystem_finding("test", &finding, &context(home.clone())).unwrap_err();

        assert!(error.to_string().contains("finding lacks preview identity"));
        remove_test_path(home);
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
    fn permanent_trash_purge_deletes_a_fifo() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-trash-fifo-{}", std::process::id()));
        remove_test_path(&home);
        std::fs::create_dir_all(home.join(".Trash")).unwrap();
        let home = home.canonicalize().unwrap();
        let fifo = home.join(".Trash/pipe");
        let status = Command::new("mkfifo").arg(&fifo).status().unwrap();
        assert!(status.success());
        let ctx = context(home.clone());
        let findings = trash_findings(&ctx).unwrap();

        let outcome = purge_trash(&findings, &ctx).unwrap();

        assert_eq!(outcome.summary.items_touched, 1);
        assert!(outcome.errors.is_empty());
        assert!(std::fs::symlink_metadata(&fifo).is_err());
        remove_test_path(home);
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

        let attempt = crate::journal::begin(
            &ctx,
            crate::journal::JournalRecord::filesystem_attempt(
                "caches",
                "trash",
                &target,
                finding.size_bytes,
            ),
        )
        .unwrap();
        attempt.finish(&ctx, Ok(())).unwrap();

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
