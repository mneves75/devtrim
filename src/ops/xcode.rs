//! Xcode support files. Archives are deliberately exempt release artifacts.

use anyhow::Result;

use super::{Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, dir_size};
use crate::safety::{Ctx, escalate, xcodebuild_running};

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
        self.scan_with_xcodebuild_state(ctx, xcodebuild_running())
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let needs_probe = findings.iter().any(|finding| {
            matches!(finding.action, Action::Trash | Action::Shred)
                && finding
                    .target()
                    .is_some_and(|path| is_derived_data_child(path, &ctx.home))
        });
        let xcodebuild_state = needs_probe.then(xcodebuild_running);
        self.apply_with_xcodebuild_state(findings, ctx, xcodebuild_state)
    }
}

impl Xcode {
    fn scan_with_xcodebuild_state(
        &self,
        ctx: &Ctx,
        xcodebuild_state: Result<bool>,
    ) -> Result<Vec<Finding>> {
        let derived_data_safe = match xcodebuild_state {
            Ok(true) => {
                ctx.diagnostic(
                    "info",
                    "xcodebuild is running; skipping DerivedData while the build process is active",
                );
                false
            }
            Ok(false) => true,
            // A failed probe must be visible to automation, not a silently
            // smaller plan with exit 0.
            Err(error) => return Err(error.context("cannot verify xcodebuild activity")),
        };
        let mut findings = Vec::new();
        for (label, relative, note) in TARGETS {
            if *label == "DerivedData" && !derived_data_safe {
                continue;
            }
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
    fn apply_with_xcodebuild_state(
        &self,
        findings: &[Finding],
        ctx: &Ctx,
        xcodebuild_state: Option<Result<bool>>,
    ) -> Result<ApplyOutcome> {
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
                if is_derived_data_child(path, &ctx.home) {
                    match xcodebuild_state.as_ref() {
                        Some(Ok(false)) => {}
                        Some(Ok(true)) => {
                            anyhow::bail!(
                                "xcodebuild is running; refusing DerivedData target {}",
                                path.display()
                            );
                        }
                        Some(Err(error)) => {
                            anyhow::bail!(
                                "cannot verify xcodebuild activity; refusing DerivedData target {}: {error:#}",
                                path.display()
                            );
                        }
                        None => anyhow::bail!(
                            "missing xcodebuild liveness result; refusing DerivedData target {}",
                            path.display()
                        ),
                    }
                }
                apply_filesystem_finding(self.name(), finding, ctx)?;
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

fn is_derived_data_child(path: &std::path::Path, home: &std::path::Path) -> bool {
    path.parent() == Some(home.join("Library/Developer/Xcode/DerivedData").as_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_context(home: std::path::PathBuf) -> Ctx {
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
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        }
    }

    #[test]
    fn scan_skips_derived_data_when_xcodebuild_is_running_or_unknown() {
        let home =
            std::env::temp_dir().join(format!("devtrim-xcode-scan-live-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(home.join("Library/Developer/Xcode/DerivedData/project")).unwrap();
        std::fs::create_dir_all(home.join("Library/Developer/Xcode/iOS DeviceSupport/device"))
            .unwrap();
        let ctx = test_context(home.clone());

        let running = Xcode.scan_with_xcodebuild_state(&ctx, Ok(true)).unwrap();
        assert_eq!(running.len(), 1);
        assert!(running[0].label.starts_with("iOS DeviceSupport"));
        assert!(
            ctx.take_diagnostics()
                .iter()
                .any(|message| message.contains("build process is active"))
        );

        let unknown = Xcode
            .scan_with_xcodebuild_state(&ctx, Err(anyhow::anyhow!("probe failed")))
            .unwrap_err();
        assert!(
            unknown
                .to_string()
                .contains("cannot verify xcodebuild activity")
        );
        crate::ops::remove_test_path(home);
    }

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
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home: home.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Stderr,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };

        let outcome = Xcode.apply(&[finding], &ctx).unwrap();
        assert!(outcome.errors.is_empty());
        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.summary.bytes_freed_estimate, 0);
        assert!(archive.join("sentinel").exists());

        crate::ops::remove_test_path(home);
    }

    #[test]
    fn forged_actionable_archive_is_rejected() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-xcode-forged-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        let archive = home.join("Library/Developer/Xcode/Archives/release.xcarchive");
        std::fs::create_dir_all(&archive).unwrap();
        let home = home.canonicalize().unwrap();
        let archive = home.join("Library/Developer/Xcode/Archives/release.xcarchive");
        let sentinel = archive.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let finding = Finding::new(
            "forged Xcode Archive",
            Some(archive),
            4,
            "test",
            9,
            Action::Shred,
        );
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home: home.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Stderr,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };

        let outcome = Xcode.apply(&[finding], &ctx).unwrap();

        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
        assert!(sentinel.exists());
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn derived_data_apply_refuses_running_or_unknown_xcodebuild() {
        let home = std::env::temp_dir().join(format!("devtrim-xcode-live-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        let target = home.join("Library/Developer/Xcode/DerivedData/project");
        std::fs::create_dir_all(&target).unwrap();
        let sentinel = target.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let finding = Finding::new(
            "DerivedData: project",
            Some(target.clone()),
            4,
            "test",
            9,
            Action::Shred,
        );
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home: home.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Stderr,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };

        let running = Xcode
            .apply_with_xcodebuild_state(std::slice::from_ref(&finding), &ctx, Some(Ok(true)))
            .unwrap();
        assert!(running.errors[0].contains("xcodebuild is running"));
        assert!(sentinel.exists());

        let unknown = Xcode
            .apply_with_xcodebuild_state(
                &[finding],
                &ctx,
                Some(Err(anyhow::anyhow!("probe failed"))),
            )
            .unwrap();
        assert!(unknown.errors[0].contains("cannot verify xcodebuild activity"));
        assert!(sentinel.exists());
        assert!(!is_derived_data_child(
            &home.join("Library/Developer/Xcode/iOS DeviceSupport/device"),
            &home
        ));
        crate::ops::remove_test_path(home);
    }
}
