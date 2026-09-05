//! Downloaded installer archives. Direct children only; Trash-first.
//!
//! An installer is finished work: once the application is installed the archive
//! is dead weight, and it is re-obtainable from its vendor. It is still not a
//! cache — nothing re-downloads it automatically — so the note says so and the
//! danger score sits above the regenerable caches.

use anyhow::{Context, Result};
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::{Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, removal_note};
use crate::safety::{Ctx, escalate};

pub struct Installers;

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

/// Age in days and logical size for an eligible installer, re-read at apply time.
///
/// The scanner is never deletion authority: this predicate is the one the sink
/// trusts, so it repeats every structural condition instead of assuming the
/// finding was built correctly.
fn installer_details(path: &Path, home: &Path, active_days: u32) -> Result<Option<(u64, u64)>> {
    let Some(parent) = path.parent() else {
        return Ok(None);
    };
    if !INSTALLER_DIRECTORIES
        .iter()
        .any(|directory| parent == home.join(directory))
    {
        return Ok(None);
    }
    if !has_installer_extension(path) {
        return Ok(None);
    }
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("cannot inspect {}", path.display()))?;
    // A symlink is refused outright: following one would delete a file outside
    // the authorized directory while every path check above still passed.
    if !metadata.file_type().is_file() {
        return Ok(None);
    }
    // Age is part of the shape the scanner promised, so apply has to reassert
    // it too. An archive modified in place after preview keeps its inode and
    // generation, so the sink's identity check cannot see it; without this the
    // plan would delete a file that is no longer stale.
    let modified = metadata
        .modified()
        .with_context(|| format!("cannot read modification time of {}", path.display()))?;
    let Ok(elapsed) = SystemTime::now().duration_since(modified) else {
        return Ok(None);
    };
    let age = elapsed.as_secs() / Duration::from_secs(60 * 60 * 24).as_secs();
    Ok((age >= u64::from(active_days)).then_some((age, metadata.len())))
}

fn scan_directory(directory: &Path, ctx: &Ctx, findings: &mut Vec<Finding>) -> Result<()> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", directory.display()));
        }
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot enumerate {}", directory.display()))?;
        let path = entry.path();
        let Some((age, size)) = installer_details(&path, &ctx.home, ctx.active_days)? else {
            continue;
        };
        if size == 0 {
            continue;
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
    Ok(())
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
            scan_directory(&ctx.home.join(directory), ctx, &mut findings)?;
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
                if installer_details(target, &ctx.home, ctx.active_days)?.is_none() {
                    anyhow::bail!(
                        "installer target is outside its authorized namespace: {}",
                        target.display()
                    );
                }
                apply_filesystem_finding(self.name(), finding, ctx)
            })()
            .with_context(|| format!("failed to remove {}", finding.label));
            if let Err(error) = result {
                outcome.fail(error);
                break;
            }
            outcome.record(finding, removal_note(finding, &finding.label));
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    fn test_ctx(home: PathBuf) -> Ctx {
        Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home,
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Stderr,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        }
    }

    fn stale_file(path: &Path, bytes: usize) {
        std::fs::write(path, vec![b'x'; bytes]).unwrap();
        let stale = SystemTime::now() - Duration::from_secs(60 * 60 * 24 * 400);
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(stale).unwrap();
    }

    #[test]
    fn finds_stale_installers_and_skips_recent_and_foreign_extensions() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-installers")
            .tempdir()
            .unwrap();
        let downloads = home.path().join("Downloads");
        std::fs::create_dir_all(&downloads).unwrap();

        stale_file(&downloads.join("Tool.dmg"), 2048);
        // Case variants must match on a case-insensitive volume.
        stale_file(&downloads.join("Other.PKG"), 2048);
        // A stale archive that is not an installer container stays out.
        stale_file(&downloads.join("sources.zip"), 2048);
        // A fresh installer is presumed mid-install.
        std::fs::write(downloads.join("Fresh.dmg"), vec![b'x'; 2048]).unwrap();

        let ctx = test_ctx(home.path().to_path_buf());
        let findings = Installers
            .scan(&ctx, &crate::ops::project::ScanObservations::default())
            .unwrap();

        let mut labels: Vec<_> = findings
            .iter()
            .filter_map(|finding| finding.target())
            .filter_map(|target| target.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .collect();
        labels.sort();
        assert_eq!(
            labels,
            vec!["Other.PKG".to_string(), "Tool.dmg".to_string()]
        );
        assert!(
            findings
                .iter()
                .all(|finding| finding.action == Action::Trash)
        );
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

        assert!(
            installer_details(&forged, home.path(), 30)
                .unwrap()
                .is_none()
        );

        let authorized = home.path().join("Downloads");
        std::fs::create_dir_all(&authorized).unwrap();
        let real = authorized.join("Tool.dmg");
        stale_file(&real, 2048);
        assert!(installer_details(&real, home.path(), 30).unwrap().is_some());
    }

    /// Apply reasserts age, not just shape. An archive touched in place after
    /// preview keeps its inode and generation, so the sink's identity check
    /// cannot see the change; only re-reading the timestamp can.
    #[test]
    fn an_installer_that_stopped_being_stale_after_preview_is_refused() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-installers-touched")
            .tempdir()
            .unwrap();
        let downloads = home.path().join("Downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        let path = downloads.join("Tool.dmg");
        stale_file(&path, 2048);
        assert!(installer_details(&path, home.path(), 30).unwrap().is_some());

        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::now())
            .unwrap();
        assert!(
            installer_details(&path, home.path(), 30).unwrap().is_none(),
            "a freshly touched archive must fall out of the plan"
        );
    }

    #[test]
    fn a_symlinked_installer_is_refused() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-installers-symlink")
            .tempdir()
            .unwrap();
        let downloads = home.path().join("Downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        let real = home.path().join("real.dmg");
        stale_file(&real, 2048);
        let link = downloads.join("Link.dmg");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        assert!(installer_details(&link, home.path(), 30).unwrap().is_none());
        let ctx = test_ctx(home.path().to_path_buf());
        assert!(
            Installers
                .scan(&ctx, &crate::ops::project::ScanObservations::default(),)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn nested_installers_are_not_scanned() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-installers-nested")
            .tempdir()
            .unwrap();
        let nested = home.path().join("Downloads/extracted-project");
        std::fs::create_dir_all(&nested).unwrap();
        stale_file(&nested.join("Bundled.pkg"), 2048);

        let ctx = test_ctx(home.path().to_path_buf());
        assert!(
            Installers
                .scan(&ctx, &crate::ops::project::ScanObservations::default(),)
                .unwrap()
                .is_empty()
        );
    }
}
