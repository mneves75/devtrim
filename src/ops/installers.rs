//! Downloaded installer archives. Direct children only; Trash-first.
//!
//! An installer is finished work: once the application is installed the archive
//! is dead weight, and it is re-obtainable from its vendor. It is still not a
//! cache — nothing re-downloads it automatically — so the note says so and the
//! danger score sits above the regenerable caches.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::project::{UnreadFolder, error_finding, unjudged_finding};
use super::{Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, removal_note};
use crate::safety::{Ctx, escalate};

pub(crate) struct Installers;

/// Directories whose direct children may be considered. Scanning is deliberately
/// non-recursive: `Downloads` routinely contains extracted project trees, and an
/// installer-shaped file inside one of those is not loose clutter.
const INSTALLER_DIRECTORIES: &[&str] = &["Downloads", "Desktop"];

/// Closed list of installer container extensions, matched ASCII-case-insensitively
/// because macOS volumes are case-insensitive by default. Archive formats that also
/// carry source or user data (`zip`, `tar`, `gz`) are deliberately absent.
const INSTALLER_EXTENSIONS: &[&str] = &["dmg", "pkg", "mpkg", "iso", "xip"];

fn has_installer_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            INSTALLER_EXTENSIONS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
}

/// What [`judge`] makes of one path.
enum Judgement {
    /// Age in days and allocated size.
    Eligible(u64, u64),
    /// Not a direct child of an installer folder with an installer extension.
    OutsideNamespace,
    /// A symlink, directory or special file at an installer's name.
    NotAFile,
    /// Modified within the retention window.
    Recent,
    /// Modified in the future, so its age is unknown.
    FutureModification,
}

/// Judge `path` as an installer, re-read at apply time.
///
/// The scanner is never deletion authority: this predicate is the one the sink
/// trusts, so it repeats every structural condition instead of assuming the
/// finding was built correctly.
fn judge(path: &Path, home: &Path, retain_days: u32) -> Result<Judgement> {
    let Some(parent) = path.parent() else {
        return Ok(Judgement::OutsideNamespace);
    };
    if !INSTALLER_DIRECTORIES
        .iter()
        .any(|directory| parent == home.join(directory))
    {
        return Ok(Judgement::OutsideNamespace);
    }
    if !has_installer_extension(path) {
        return Ok(Judgement::OutsideNamespace);
    }
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("cannot inspect {}", path.display()))?;
    // A symlink is refused outright: following one would delete a file outside
    // the authorized directory while every path check above still passed.
    if !metadata.file_type().is_file() {
        return Ok(Judgement::NotAFile);
    }
    // Age is part of the shape the scanner promised, so apply has to reassert
    // it too. An archive modified in place after preview keeps its inode and
    // generation, so the sink's identity check cannot see it; without this the
    // plan would delete a file that is no longer stale.
    let modified = metadata
        .modified()
        .with_context(|| format!("cannot read modification time of {}", path.display()))?;
    let Ok(elapsed) = SystemTime::now().duration_since(modified) else {
        return Ok(Judgement::FutureModification);
    };
    let age = elapsed.as_secs() / Duration::from_secs(60 * 60 * 24).as_secs();
    if age < u64::from(retain_days) {
        return Ok(Judgement::Recent);
    }
    Ok(Judgement::Eligible(
        age,
        crate::safety::allocated_bytes(&metadata, path)?,
    ))
}

/// Age in days and allocated size for an eligible installer.
fn installer_details(path: &Path, home: &Path, retain_days: u32) -> Result<Option<(u64, u64)>> {
    Ok(match judge(path, home, retain_days)? {
        Judgement::Eligible(age, size) => Some((age, size)),
        _ => None,
    })
}

/// Why apply refuses `path`. A forged or stale target is outside the
/// namespace; an archive touched since the preview is the ordinary drift case,
/// and saying "outside its authorized namespace" would misdescribe it.
fn refusal(path: &Path, home: &Path, retain_days: u32) -> Result<String> {
    let shown = path.display();
    Ok(match judge(path, home, retain_days)? {
        Judgement::OutsideNamespace | Judgement::Eligible(..) => {
            format!("installer target is outside its authorized namespace: {shown}")
        }
        Judgement::NotAFile => {
            format!("installer is no longer a regular file since the preview; refusing {shown}")
        }
        Judgement::Recent => format!(
            "installer changed since the preview — it was modified within the {retain_days}-day retention window; refusing {shown}"
        ),
        Judgement::FutureModification => {
            format!("installer modification time is in the future; refusing {shown}")
        }
    })
}

/// One installer folder. A folder that cannot be inspected or listed is one
/// error naming it, and archives whose metadata cannot be read are one error
/// naming them (`CODING_STANDARDS.md` S8); the other folder is still judged.
fn scan_directory(directory: &Path, ctx: &Ctx, findings: &mut Vec<Finding>) {
    let mut unread = Vec::new();
    if let Err(error) = scan_entries(directory, ctx, findings, &mut unread) {
        findings.push(unjudged_finding("installers", directory, &error));
    }
    if unread.is_empty() {
        return;
    }
    let mut named = unread
        .iter()
        .take(3)
        .map(|entry| format!("{} ({})", entry.path.display(), entry.reason))
        .collect::<Vec<_>>()
        .join(", ");
    if unread.len() > 3 {
        named.push_str(&format!(" and {} more", unread.len() - 3));
    }
    findings.push(error_finding(
        "installers not judged".to_string(),
        format!(
            "cannot judge {} installer(s) in {}, so they are not offered: {named}",
            unread.len(),
            directory.display()
        ),
    ));
}

fn scan_entries(
    directory: &Path,
    ctx: &Ctx,
    findings: &mut Vec<Finding>,
    unread: &mut Vec<UnreadFolder>,
) -> Result<()> {
    // The sink refuses a target below a link, so a linked folder is named
    // rather than previewed with archives apply would refuse.
    match std::fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            ctx.diagnostic(
                "warn",
                format!(
                    "{} is a link; skipping the installers in it",
                    directory.display()
                ),
            );
            return Ok(());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", directory.display()));
        }
    }
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", directory.display()));
        }
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot enumerate {}", directory.display()))?;
        scan_entry(entry.path(), ctx, findings, unread);
    }
    Ok(())
}

/// One direct child of an installer folder. An entry removed since the
/// listing is skipped; one whose metadata cannot be read is recorded in
/// `unread` and offers nothing.
fn scan_entry(
    path: PathBuf,
    ctx: &Ctx,
    findings: &mut Vec<Finding>,
    unread: &mut Vec<UnreadFolder>,
) {
    let (age, size) = match installer_details(&path, &ctx.home, ctx.retain_days) {
        Ok(Some(details)) => details,
        Ok(None) => return,
        Err(error)
            if error
                .root_cause()
                .downcast_ref::<std::io::Error>()
                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound) =>
        {
            return;
        }
        Err(error) => {
            unread.push(UnreadFolder::new(&path, &error));
            return;
        }
    };
    if size == 0 {
        return;
    }
    findings.push(Finding::new(
        format!(
            "installer archive: {}",
            path.file_name()
                .map_or_else(|| path.display().to_string(), |name| name
                    .to_string_lossy()
                    .into_owned())
        ),
        Some(path),
        size,
        format!(
            "untouched for {age} days; installers do not re-download automatically, re-obtain from the vendor if needed"
        ),
        escalate(3, size),
        Action::Trash,
    ));
}

impl Op for Installers {
    fn name(&self) -> &'static str {
        "installers"
    }

    fn scan(
        &self,
        ctx: &Ctx,
        _observations: &super::project::ScanObservations,
    ) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        for directory in INSTALLER_DIRECTORIES {
            scan_directory(&ctx.home.join(directory), ctx, &mut findings);
        }
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let mut outcome = ApplyOutcome::new(self.name());
        for finding in findings {
            if !matches!(finding.action, Action::Trash | Action::Shred) {
                continue;
            }
            let result = (|| -> Result<()> {
                let target = finding
                    .target()
                    .ok_or_else(|| anyhow::anyhow!("installer finding missing internal target"))?;
                if installer_details(target, &ctx.home, ctx.retain_days)?.is_none() {
                    anyhow::bail!(refusal(target, &ctx.home, ctx.retain_days)?);
                }
                apply_filesystem_finding(self.name(), finding, ctx)
            })()
            .with_context(|| format!("failed to remove {}", finding.label));
            // Archives are unrelated files: one touched since the preview must
            // not strand the others. Every failure is recorded, so the run
            // still reports nonzero.
            if let Err(error) = result {
                outcome.fail(error);
                continue;
            }
            outcome.record(finding, removal_note(finding, &finding.label));
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_ctx(home: PathBuf) -> Ctx {
        Ctx {
            diagnostic_output: crate::safety::DiagnosticOutput::Stderr,
            ..Ctx::for_test(home)
        }
    }

    fn stale_file(path: &Path, bytes: usize) {
        std::fs::write(path, vec![b'x'; bytes]).unwrap();
        let stale = SystemTime::now() - Duration::from_secs(60 * 60 * 24 * 400);
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(stale).unwrap();
    }

    /// A `Downloads` reached through a link is not the folder apply accepts:
    /// the sink refuses a target with a symlinked ancestor, so the scan names
    /// the link instead of previewing archives that apply would refuse.
    #[test]
    fn a_linked_installer_folder_is_named_not_offered() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-installers-linked")
            .tempdir()
            .unwrap();
        let elsewhere = home.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        stale_file(&elsewhere.join("Old.dmg"), 2048);
        std::os::unix::fs::symlink(&elsewhere, home.path().join("Downloads")).unwrap();
        let mut ctx = test_ctx(home.path().to_path_buf());
        ctx.diagnostic_output = crate::safety::DiagnosticOutput::Capture;

        let findings = Installers
            .scan(&ctx, &crate::ops::project::ScanObservations::default())
            .unwrap();

        assert!(findings.is_empty(), "{findings:?}");
        let diagnostics = ctx.take_diagnostics();
        assert!(
            diagnostics
                .iter()
                .any(|line| line.starts_with("warn:") && line.contains("Downloads")),
            "{diagnostics:?}"
        );

        std::fs::remove_file(home.path().join("Downloads")).unwrap();
        std::fs::rename(&elsewhere, home.path().join("Downloads")).unwrap();
        let findings = Installers
            .scan(&ctx, &crate::ops::project::ScanObservations::default())
            .unwrap();
        assert_eq!(findings.len(), 1, "positive control: {findings:?}");
    }

    /// An installer archive is not regenerated by any tool, so the project
    /// window never decides it: with a one-day `active_days` and the default
    /// retention, a five-day-old installer stays; only `retain_days` offers it.
    #[test]
    fn installers_follow_the_retention_window_not_the_project_window() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-installers-retention")
            .tempdir()
            .unwrap();
        let downloads = home.path().join("Downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        let installer = downloads.join("Recent.dmg");
        std::fs::write(&installer, vec![b'x'; 2048]).unwrap();
        let five_days = SystemTime::now() - Duration::from_secs(60 * 60 * 24 * 5);
        std::fs::File::options()
            .write(true)
            .open(&installer)
            .unwrap()
            .set_modified(five_days)
            .unwrap();
        let mut ctx = test_ctx(home.path().to_path_buf());
        ctx.active_days = 1;
        ctx.retain_days = 30;

        let findings = Installers
            .scan(&ctx, &crate::ops::project::ScanObservations::default())
            .unwrap();
        assert!(
            findings.is_empty(),
            "PV installers/retention-window: a short project window exposed an installer: {findings:?}"
        );

        ctx.retain_days = 3;
        let findings = Installers
            .scan(&ctx, &crate::ops::project::ScanObservations::default())
            .unwrap();
        assert_eq!(findings.len(), 1, "positive control: {findings:?}");
    }

    /// Positive control for the apply-time boundary: the scanner shape alone must
    /// never be enough, so a forged finding pointing outside the authorized
    /// directories has to be refused even though its extension is approved.
    #[test]
    fn apply_refuses_a_target_outside_the_authorized_directories() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-installers-forged")
            .tempdir()
            .unwrap();
        let elsewhere = home.path().join("Projects");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let forged = elsewhere.join("Tool.dmg");
        stale_file(&forged, 2048);
        let finding = Finding::new(
            "installer archive: Tool.dmg",
            Some(forged.clone()),
            2048,
            "test",
            3,
            Action::Trash,
        );

        let outcome = Installers
            .apply(&[finding], &test_ctx(home.path().to_path_buf()))
            .unwrap();

        assert_eq!(outcome.summary.items_touched, 0, "{outcome:?}");
        assert_eq!(outcome.errors.len(), 1, "{outcome:?}");
        assert!(
            outcome.errors[0].contains("outside its authorized namespace"),
            "{outcome:?}"
        );
        assert!(forged.exists());

        // The eligible control stays at the predicate: applying it would reach
        // the real Trash, which no test touches.

        let authorized = home.path().join("Downloads");
        std::fs::create_dir_all(&authorized).unwrap();
        let real = authorized.join("Tool.dmg");
        stale_file(&real, 2048);
        assert!(installer_details(&real, home.path(), 30).unwrap().is_some());
    }

    /// A folder that cannot be listed (macOS privacy answers EPERM for
    /// `Desktop`) is one error naming it; the archives in the other folder are
    /// still offered.
    #[test]
    fn an_unreadable_installer_folder_withholds_only_itself() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-installers-unread")
            .tempdir()
            .unwrap();
        let downloads = home.path().join("Downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        stale_file(&downloads.join("Old.dmg"), 2048);
        let desktop = home.path().join("Desktop");
        let sealed = crate::ops::Unreadable::new(&desktop, 0o000);

        let findings = Installers.scan(
            &test_ctx(home.path().to_path_buf()),
            &crate::ops::project::ScanObservations::default(),
        );

        drop(sealed);
        let findings = findings.unwrap();
        let offered = findings
            .iter()
            .filter_map(Finding::target)
            .collect::<Vec<_>>();
        assert_eq!(offered, vec![downloads.join("Old.dmg").as_path()]);
        let errors = findings
            .iter()
            .filter_map(Finding::scan_error)
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 1, "{findings:?}");
        assert!(
            errors[0].contains(&*desktop.to_string_lossy()),
            "{errors:?}"
        );
    }

    /// A folder that can be listed but not searched fails each entry's
    /// metadata read: that folder's archives become one error naming the
    /// folder, and the other folder's archives are still offered.
    #[test]
    fn an_uninspectable_installer_withholds_only_its_folder() {
        use std::os::unix::fs::PermissionsExt as _;

        let home = tempfile::Builder::new()
            .prefix("devtrim-installers-entry")
            .tempdir()
            .unwrap();
        let desktop = home.path().join("Desktop");
        std::fs::create_dir_all(&desktop).unwrap();
        stale_file(&desktop.join("Kept.pkg"), 2048);
        let downloads = home.path().join("Downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        stale_file(&downloads.join("First.dmg"), 2048);
        stale_file(&downloads.join("Second.iso"), 2048);
        // `Unreadable` restores the folder on drop, even when a test panics.
        let sealed = crate::ops::Unreadable::new(&downloads, 0o000);
        std::fs::set_permissions(&downloads, std::fs::Permissions::from_mode(0o444)).unwrap();

        let findings = Installers.scan(
            &test_ctx(home.path().to_path_buf()),
            &crate::ops::project::ScanObservations::default(),
        );

        drop(sealed);
        let findings = findings.unwrap();
        let offered = findings
            .iter()
            .filter_map(Finding::target)
            .collect::<Vec<_>>();
        assert_eq!(offered, vec![desktop.join("Kept.pkg").as_path()]);
        let errors = findings
            .iter()
            .filter_map(Finding::scan_error)
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 1, "{findings:?}");
        assert!(errors[0].contains("First.dmg"), "{errors:?}");
        assert!(errors[0].contains("Second.iso"), "{errors:?}");
    }

    /// An entry that vanished between the listing and its metadata read is
    /// simply gone: no finding, no error.
    #[test]
    fn a_vanished_installer_is_skipped() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-installers-vanished")
            .tempdir()
            .unwrap();
        let downloads = home.path().join("Downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        let ctx = test_ctx(home.path().to_path_buf());
        let mut findings = Vec::new();
        let mut unread = Vec::new();

        scan_entry(downloads.join("Gone.dmg"), &ctx, &mut findings, &mut unread);

        assert!(findings.is_empty() && unread.is_empty(), "{findings:?}");

        stale_file(&downloads.join("Here.dmg"), 2048);
        scan_entry(downloads.join("Here.dmg"), &ctx, &mut findings, &mut unread);
        assert_eq!(findings.len(), 1, "positive control: {findings:?}");
    }
}
