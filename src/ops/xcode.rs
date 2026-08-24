//! Xcode support files. Archives are deliberately exempt release artifacts.

use anyhow::Result;

use super::{Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, dir_size};
use crate::safety::{Ctx, escalate};

pub struct Xcode;

const TARGETS: &[(&str, &str, &str)] = &[
    (
        "iOS DeviceSupport",
        "Developer/Xcode/iOS DeviceSupport",
        "symbol cache; rebuilt on next device connect/debug",
    ),
    (
        "DerivedData",
        "Developer/Xcode/DerivedData",
        "build output; rebuilt on next build",
    ),
];

impl Op for Xcode {
    fn name(&self) -> &'static str {
        "xcode"
    }

    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        for (label, relative, note) in TARGETS {
            let directory = ctx.home.join("Library").join(relative);
            let entries = match std::fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            for entry in entries {
                let path = entry?.path();
                let size = dir_size(&path)?;
                findings.push(Finding::new(
                    format!(
                        "{label}: {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    Some(path),
                    size,
                    *note,
                    escalate(4, size),
                    Action::Trash,
                ));
            }
        }
        let archives = ctx.home.join("Library/Developer/Xcode/Archives");
        let archive_size = dir_size(&archives)?;
        if archive_size > 0 {
            findings.push(Finding::new(
                "Xcode Archives",
                Some(archives),
                archive_size,
                "EXCLUDED: release artifacts; listed for visibility only",
                0,
                Action::None,
            ));
        }
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let mut outcome = ApplyOutcome::new(self.name());
        for finding in findings {
            if !matches!(finding.action, Action::Trash | Action::Shred) {
                if finding.action == Action::None {
                    outcome
                        .summary
                        .notes
                        .push("skipped Xcode Archives by design".into());
                }
                continue;
            }
            let result = (|| -> Result<String> {
                let path = finding
                    .target()
                    .ok_or_else(|| anyhow::anyhow!("Xcode finding missing internal target"))?;
                apply_filesystem_finding(finding, ctx)?;
                Ok(format!(
                    "{} {}",
                    if finding.action == Action::Shred {
                        "permanently deleted"
                    } else {
                        "trashed"
                    },
                    path.display()
                ))
            })();
            match result {
                Ok(note) => outcome.record(finding, note),
                Err(error) => {
                    outcome.fail(error);
                    break;
                }
            }
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archives_are_never_applied_or_counted() {
        let home = std::env::temp_dir().join(format!("devtrim-xcode-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        let archive = home.join("Library/Developer/Xcode/Archives/release.xcarchive");
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::write(archive.join("sentinel"), "keep").unwrap();
        let finding = Finding::new(
            "Xcode Archives",
            Some(archive.clone()),
            4,
            "excluded",
            0,
            Action::None,
        );
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            home: home.clone(),
            interactive: false,
        };

        let outcome = Xcode.apply(&[finding], &ctx).unwrap();
        assert!(outcome.errors.is_empty());
        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.summary.bytes_freed_estimate, 0);
        assert!(archive.join("sentinel").exists());

        crate::ops::remove_test_path(home);
    }
}
