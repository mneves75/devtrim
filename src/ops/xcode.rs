//! Xcode support files. Archives are deliberately exempt release artifacts.

use anyhow::Result;

use super::{Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, dir_size, removal_note};
use crate::safety::{Ctx, escalate, xcodebuild_running};

pub struct Xcode;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum XcodeTargetKind {
    DeviceSupport,
    DerivedData,
}

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

    fn scan(
        &self,
        ctx: &Ctx,
        _observations: &super::project::ScanObservations,
    ) -> Result<Vec<Finding>> {
        self.scan_with_xcodebuild_state(ctx, xcodebuild_running())
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let needs_probe = findings.iter().any(|finding| {
            matches!(finding.action, Action::Trash | Action::Shred)
                && finding.target().is_some_and(|path| {
                    authorize_xcode_target(path, &ctx.home)
                        .is_ok_and(|kind| kind == XcodeTargetKind::DerivedData)
                })
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
                let target_kind = authorize_xcode_target(path, &ctx.home)?;
                if target_kind == XcodeTargetKind::DerivedData {
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
                Ok(removal_note(finding, path.display()))
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

fn authorize_xcode_target(
    path: &std::path::Path,
    home: &std::path::Path,
) -> Result<XcodeTargetKind> {
    let device_support = home.join("Library/Developer/Xcode/iOS DeviceSupport");
    if path.parent() == Some(device_support.as_path()) {
        return Ok(XcodeTargetKind::DeviceSupport);
    }
    let derived_data = home.join("Library/Developer/Xcode/DerivedData");
    if path.parent() == Some(derived_data.as_path()) {
        return Ok(XcodeTargetKind::DerivedData);
    }
    anyhow::bail!(
        "refusing Xcode target outside direct DeviceSupport or DerivedData children: {}",
        path.display()
    )
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
    fn direct_device_support_child_can_be_applied() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-xcode-direct-{}", std::process::id()));
        crate::ops::remove_test_path(&root);
        let target = root.join("Library/Developer/Xcode/iOS DeviceSupport/device");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("sentinel"), "remove").unwrap();
        let home = root.canonicalize().unwrap();
        let target = home.join("Library/Developer/Xcode/iOS DeviceSupport/device");
        let finding = Finding::new(
            "DeviceSupport: device",
            Some(target.clone()),
            6,
            "test",
            9,
            Action::Shred,
        );

        let outcome = Xcode
            .apply_with_xcodebuild_state(&[finding], &test_context(home.clone()), None)
            .unwrap();

        assert!(outcome.errors.is_empty());
        assert_eq!(outcome.summary.items_touched, 1);
        assert!(!target.exists());
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn forged_nested_derived_data_target_is_rejected_before_liveness() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-xcode-nested-{}", std::process::id()));
        crate::ops::remove_test_path(&root);
        let target = root.join("Library/Developer/Xcode/DerivedData/project/nested");
        std::fs::create_dir_all(&target).unwrap();
        let sentinel = target.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let home = root.canonicalize().unwrap();
        let target = home.join("Library/Developer/Xcode/DerivedData/project/nested");
        let finding = Finding::new(
            "forged nested DerivedData",
            Some(target),
            4,
            "test",
            9,
            Action::Shred,
        );

        let outcome = Xcode
            .apply_with_xcodebuild_state(&[finding], &test_context(home.clone()), None)
            .unwrap();

        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
        assert!(outcome.errors[0].contains("outside direct DeviceSupport or DerivedData"));
        assert!(sentinel.exists());
        crate::ops::remove_test_path(root);
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
        crate::ops::remove_test_path(home);
    }
}
