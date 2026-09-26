//! Xcode support files. Archives are deliberately exempt release artifacts.

use anyhow::{Context, Result};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use super::project::has_git_marker;
use super::{Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, dir_size, removal_note};
use crate::safety::{Ctx, escalate, xcode_build_running};

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
        "debug symbols copied from a device; Xcode copies them again only from a connected device running this exact OS build, so a build no device still runs is needed only to symbolicate its crash logs",
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
        self.scan_with_xcode_build_state(ctx, xcode_build_running())
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let needs_probe = findings.iter().any(|finding| {
            matches!(finding.action, Action::Trash | Action::Shred)
                && finding.target().is_some_and(|path| {
                    authorize_xcode_target(path, &ctx.home)
                        .is_ok_and(|kind| kind == XcodeTargetKind::DerivedData)
                })
        });
        let xcode_build_state = needs_probe.then(xcode_build_running);
        self.apply_with_xcode_build_state(findings, ctx, xcode_build_state)
    }
}

impl Xcode {
    fn scan_with_xcode_build_state(
        &self,
        ctx: &Ctx,
        xcode_build_state: Result<bool>,
    ) -> Result<Vec<Finding>> {
        let derived_data_safe = match xcode_build_state {
            Ok(true) => {
                ctx.diagnostic(
                    "info",
                    "Xcode or an Xcode build is running; skipping DerivedData while it is active",
                );
                false
            }
            Ok(false) => true,
            // A failed probe must be visible to automation, not a silently
            // smaller plan with exit 0.
            Err(error) => return Err(error.context("cannot verify Xcode build activity")),
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
                let entry = entry.with_context(|| {
                    format!(
                        "cannot read Xcode support directory {}",
                        directory.display()
                    )
                })?;
                // Only a real directory is a symbol or build tree. Finder's
                // `.DS_Store` sits beside them, and a symlink child would lend
                // this category's authority to whatever it names.
                let file_type = entry.file_type().with_context(|| {
                    format!(
                        "cannot inspect Xcode support entry {}",
                        entry.path().display()
                    )
                })?;
                if !file_type.is_dir() {
                    continue;
                }
                let path = entry.path();
                if *label == "DerivedData" && is_package_folder(&path)? {
                    let folder = path.file_name().unwrap_or_default().to_string_lossy();
                    let package_note =
                        format!("{note}. SwiftPM package checkouts here are Git clones and stay");
                    for child in package_folder_children(&path)? {
                        let size = dir_size(&child)?;
                        findings.push(Finding::new(
                            format!(
                                "{label}: {folder}/{}",
                                child.file_name().unwrap_or_default().to_string_lossy()
                            ),
                            Some(child),
                            size,
                            package_note.clone(),
                            escalate(4, size),
                            Action::Trash,
                        ));
                    }
                    continue;
                }
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
    fn apply_with_xcode_build_state(
        &self,
        findings: &[Finding],
        ctx: &Ctx,
        xcode_build_state: Option<Result<bool>>,
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
                    match xcode_build_state.as_ref() {
                        Some(Ok(false)) => {}
                        Some(Ok(true)) => {
                            anyhow::bail!(
                                "Xcode or an Xcode build is running; refusing DerivedData target {}",
                                path.display()
                            );
                        }
                        Some(Err(error)) => {
                            anyhow::bail!(
                                "cannot verify Xcode build activity; refusing DerivedData target {}: {error:#}",
                                path.display()
                            );
                        }
                        None => anyhow::bail!(
                            "missing Xcode build liveness result; refusing DerivedData target {}",
                            path.display()
                        ),
                    }
                }
                apply_filesystem_finding(self.name(), finding, ctx)?;
                Ok(removal_note(finding, path.display()))
            })();
            // A refusal costs only its own finding: a DerivedData folder that
            // holds a Git repository is refused by the sink, and stopping there
            // would strand every build tree after it. Each failure is recorded,
            // so the run still reports nonzero.
            match result {
                Ok(note) => outcome.record(finding, note),
                Err(error) => outcome.fail(error),
            }
        }
        Ok(outcome)
    }
}

/// Whether a DerivedData folder is offered directory by directory: it holds
/// SwiftPM package checkouts — Git clones, which the deletion sink never
/// removes — and is not itself a Git repository or worktree. A repository's
/// directories are its worktree, which splitting would hand to the sink one by
/// one past the marker that makes it refuse the whole folder.
fn is_package_folder(folder: &Path) -> Result<bool> {
    Ok(holds_package_checkouts(folder)? && !has_git_marker(folder)?)
}

/// Whether a DerivedData folder holds SwiftPM package checkouts.
fn holds_package_checkouts(folder: &Path) -> Result<bool> {
    let checkouts = folder.join("SourcePackages/checkouts");
    match std::fs::symlink_metadata(&checkouts) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => return Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", checkouts.display()));
        }
    }
    let first = std::fs::read_dir(&checkouts)
        .with_context(|| format!("cannot read {}", checkouts.display()))?
        .next()
        .transpose()
        .with_context(|| format!("cannot read {}", checkouts.display()))?;
    Ok(first.is_some())
}

/// Every real directory of a DerivedData folder holding package checkouts but
/// `SourcePackages`, which holds them: all of it is build output Xcode rebuilds.
fn package_folder_children(folder: &Path) -> Result<Vec<PathBuf>> {
    let mut children = Vec::new();
    for entry in std::fs::read_dir(folder)
        .with_context(|| format!("cannot read DerivedData folder {}", folder.display()))?
    {
        let entry = entry
            .with_context(|| format!("cannot read DerivedData folder {}", folder.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("cannot inspect {}", entry.path().display()))?;
        if file_type.is_dir() && !is_source_packages(&entry.file_name()) {
            children.push(entry.path());
        }
    }
    children.sort();
    Ok(children)
}

/// `SourcePackages` in any ASCII case, which names the same directory on a
/// case-insensitive volume.
fn is_source_packages(name: &OsStr) -> bool {
    name.as_encoded_bytes()
        .eq_ignore_ascii_case(b"SourcePackages")
}

/// Refuses `SourcePackages` in a DerivedData folder: its checkouts are Git
/// clones, which devtrim never deletes, however the finding reached apply.
fn refuse_package_checkouts(path: &Path, derived_data: &Path) -> Result<()> {
    let in_folder = path
        .parent()
        .is_some_and(|folder| folder.parent() == Some(derived_data));
    if in_folder && path.file_name().is_some_and(is_source_packages) {
        anyhow::bail!(
            "refusing the SwiftPM package checkouts at {}: they are Git clones",
            path.display()
        );
    }
    Ok(())
}

/// Whether `path` is a directory a scan offers inside a DerivedData folder
/// holding package checkouts: a direct child of a real folder that still
/// holds them and is still no repository.
fn is_package_folder_child(path: &Path, derived_data: &Path) -> Result<bool> {
    let Some(folder) = path
        .parent()
        .filter(|folder| folder.parent() == Some(derived_data))
    else {
        return Ok(false);
    };
    let folder_is_directory = std::fs::symlink_metadata(folder)
        .with_context(|| format!("cannot inspect DerivedData folder {}", folder.display()))?
        .file_type()
        .is_dir();
    Ok(folder_is_directory && is_package_folder(folder)?)
}

fn authorize_xcode_target(path: &Path, home: &Path) -> Result<XcodeTargetKind> {
    let device_support = home.join("Library/Developer/Xcode/iOS DeviceSupport");
    let derived_data = home.join("Library/Developer/Xcode/DerivedData");
    refuse_package_checkouts(path, &derived_data)?;
    let kind = if path.parent() == Some(device_support.as_path()) {
        XcodeTargetKind::DeviceSupport
    } else if path.parent() == Some(derived_data.as_path())
        || is_package_folder_child(path, &derived_data)?
    {
        XcodeTargetKind::DerivedData
    } else {
        anyhow::bail!(
            "refusing Xcode target outside direct DeviceSupport or DerivedData children: {}",
            path.display()
        )
    };
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("cannot inspect Xcode target {}", path.display()))?;
    if !metadata.file_type().is_dir() {
        anyhow::bail!("refusing non-directory Xcode target: {}", path.display());
    }
    Ok(kind)
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
    fn scan_skips_derived_data_when_an_xcode_build_is_running_or_unknown() {
        let home =
            std::env::temp_dir().join(format!("devtrim-xcode-scan-live-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(home.join("Library/Developer/Xcode/DerivedData/project")).unwrap();
        std::fs::create_dir_all(home.join("Library/Developer/Xcode/iOS DeviceSupport/device"))
            .unwrap();
        let ctx = test_context(home.clone());

        let running = Xcode.scan_with_xcode_build_state(&ctx, Ok(true)).unwrap();
        assert_eq!(running.len(), 1);
        assert!(running[0].label.starts_with("iOS DeviceSupport"));
        assert!(
            ctx.take_diagnostics()
                .iter()
                .any(|message| message.contains("skipping DerivedData while it is active"))
        );

        let unknown = Xcode
            .scan_with_xcode_build_state(&ctx, Err(anyhow::anyhow!("probe failed")))
            .unwrap_err();
        assert!(
            unknown
                .to_string()
                .contains("cannot verify Xcode build activity")
        );
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn scan_offers_only_real_directories_as_xcode_support_children() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-xcode-shape-scan-{}", std::process::id()));
        crate::ops::remove_test_path(&root);
        let device_support = root.join("Library/Developer/Xcode/iOS DeviceSupport");
        let derived_data = root.join("Library/Developer/Xcode/DerivedData");
        std::fs::create_dir_all(device_support.join("iPhone18,2 27.0 (24A437)")).unwrap();
        std::fs::create_dir_all(derived_data.join("project-abc")).unwrap();
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        // Finder writes `.DS_Store` beside the symbol directories; it is not a
        // symbol cache, and a symlink child would borrow the category's authority
        // for whatever it points at.
        std::fs::write(device_support.join(".DS_Store"), "finder").unwrap();
        std::fs::write(derived_data.join(".DS_Store"), "finder").unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), device_support.join("linked")).unwrap();
        let home = root.canonicalize().unwrap();

        let findings = Xcode
            .scan_with_xcode_build_state(&test_context(home.clone()), Ok(false))
            .unwrap();

        let mut labels: Vec<_> = findings
            .iter()
            .filter(|finding| finding.action != Action::None)
            .map(|finding| finding.label.as_str())
            .collect();
        labels.sort_unstable();
        assert_eq!(
            labels,
            [
                "DerivedData: project-abc",
                "iOS DeviceSupport: iPhone18,2 27.0 (24A437)"
            ]
        );
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn apply_refuses_a_non_directory_xcode_support_child() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-xcode-shape-apply-{}", std::process::id()));
        crate::ops::remove_test_path(&root);
        let device_support = root.join("Library/Developer/Xcode/iOS DeviceSupport");
        std::fs::create_dir_all(device_support.join("real-build")).unwrap();
        std::fs::create_dir_all(root.join("elsewhere")).unwrap();
        std::fs::write(device_support.join(".DS_Store"), "keep").unwrap();
        std::fs::write(root.join("elsewhere/sentinel"), "keep").unwrap();
        std::os::unix::fs::symlink(root.join("elsewhere"), device_support.join("linked")).unwrap();
        let home = root.canonicalize().unwrap();
        let device_support = home.join("Library/Developer/Xcode/iOS DeviceSupport");
        let ctx = test_context(home.clone());
        let finding = |name: &str| {
            Finding::new(
                format!("iOS DeviceSupport: {name}"),
                Some(device_support.join(name)),
                4,
                "test",
                4,
                Action::Shred,
            )
        };

        for name in [".DS_Store", "linked"] {
            let outcome = Xcode
                .apply_with_xcode_build_state(&[finding(name)], &ctx, None)
                .unwrap();
            assert_eq!(
                outcome.summary.items_touched, 0,
                "PV xcode/non-directory-target: {name} was touched"
            );
            assert_eq!(
                outcome.errors.len(),
                1,
                "PV xcode/non-directory-target: {name}"
            );
            assert!(
                outcome.errors[0].contains("refusing non-directory Xcode target"),
                "PV xcode/non-directory-target: {name}: {:?}",
                outcome.errors
            );
        }
        assert_eq!(
            std::fs::read_to_string(device_support.join(".DS_Store")).unwrap(),
            "keep"
        );
        assert_eq!(
            std::fs::read_to_string(home.join("elsewhere/sentinel")).unwrap(),
            "keep"
        );

        // Positive control: the same apply removes a real directory child.
        let outcome = Xcode
            .apply_with_xcode_build_state(&[finding("real-build")], &ctx, None)
            .unwrap();
        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
        assert_eq!(outcome.summary.items_touched, 1);
        assert!(!device_support.join("real-build").exists());
        crate::ops::remove_test_path(root);
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
            .apply_with_xcode_build_state(&[finding], &test_context(home.clone()), None)
            .unwrap();

        assert!(outcome.errors.is_empty());
        assert_eq!(outcome.summary.items_touched, 1);
        assert!(!target.exists());
        crate::ops::remove_test_path(root);
    }

    /// A DerivedData folder that holds a Git repository — here one forged over
    /// a package checkout, which a scan offers only directory by directory —
    /// is refused by the sink. That refusal must cost only its own folder, not
    /// every build tree after it in the plan.
    #[test]
    fn a_refused_derived_data_folder_does_not_block_the_rest_of_the_plan() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-xcode-continue-{}", std::process::id()));
        crate::ops::remove_test_path(&root);
        let derived_data = root.join("Library/Developer/Xcode/DerivedData");
        let checkout = derived_data.join("WithPackages/SourcePackages/checkouts/package/.git");
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::write(checkout.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir_all(derived_data.join("Plain/Build")).unwrap();
        std::fs::write(derived_data.join("Plain/Build/output"), "remove").unwrap();
        let home = root.canonicalize().unwrap();
        let derived_data = home.join("Library/Developer/Xcode/DerivedData");
        let finding = |name: &str| {
            Finding::new(
                format!("DerivedData: {name}"),
                Some(derived_data.join(name)),
                4,
                "test",
                9,
                Action::Shred,
            )
        };

        let outcome = Xcode
            .apply_with_xcode_build_state(
                &[finding("WithPackages"), finding("Plain")],
                &test_context(home.clone()),
                Some(Ok(false)),
            )
            .unwrap();

        assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
        assert!(
            derived_data
                .join("WithPackages/SourcePackages/checkouts/package/.git/HEAD")
                .exists()
        );
        assert_eq!(
            outcome.summary.items_touched, 1,
            "PV xcode/continue-past-refusal: {:?}",
            outcome.errors
        );
        assert!(
            !derived_data.join("Plain").exists(),
            "PV xcode/continue-past-refusal: a refused folder blocked the DerivedData after it"
        );
        crate::ops::remove_test_path(root);
    }

    /// A DerivedData folder of a project using a package: Xcode's checkout of
    /// it is a Git clone beside the folder's build output.
    fn package_folder(folder: &Path) {
        let checkout = folder.join("SourcePackages/checkouts/Example");
        std::fs::create_dir_all(checkout.join(".git/objects")).unwrap();
        std::fs::write(checkout.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(checkout.join("Package.swift"), "// package\n").unwrap();
        std::fs::create_dir_all(folder.join("SourcePackages/repositories/Example-1a2b/objects"))
            .unwrap();
        for child in [
            "Build/Products/Debug",
            "Index.noindex/DataStore",
            "Logs/Build",
        ] {
            std::fs::create_dir_all(folder.join(child)).unwrap();
            std::fs::write(folder.join(child).join("output"), "rebuilt").unwrap();
        }
        std::fs::write(folder.join("info.plist"), "<plist/>").unwrap();
    }

    /// The sink never removes a Git repository, and each package checkout
    /// Xcode keeps in a DerivedData folder is one, so such a folder could
    /// never be cleaned. A scan offers its build output instead — every
    /// directory but `SourcePackages` — and apply removes exactly that.
    #[test]
    fn a_derived_data_folder_holding_package_checkouts_is_cleaned_around_them() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-xcode-packages-{}", std::process::id()));
        crate::ops::remove_test_path(&root);
        std::fs::create_dir_all(&root).unwrap();
        let home = root.canonicalize().unwrap();
        let derived_data = home.join("Library/Developer/Xcode/DerivedData");
        package_folder(&derived_data.join("App-pkg"));
        std::fs::create_dir_all(derived_data.join("App-plain/Build")).unwrap();
        std::fs::write(derived_data.join("App-plain/Build/output"), "rebuilt").unwrap();
        let ctx = test_context(home.clone());

        let mut findings = Xcode.scan_with_xcode_build_state(&ctx, Ok(false)).unwrap();

        let mut offered: Vec<_> = findings
            .iter()
            .filter_map(Finding::target)
            .map(|path| path.strip_prefix(&derived_data).unwrap().to_path_buf())
            .collect();
        offered.sort();
        assert!(
            !offered.contains(&PathBuf::from("App-pkg/SourcePackages")),
            "PV xcode/package-checkouts-offered: {offered:?}"
        );
        assert_eq!(
            offered,
            [
                "App-pkg/Build",
                "App-pkg/Index.noindex",
                "App-pkg/Logs",
                "App-plain"
            ]
            .map(PathBuf::from)
        );
        crate::report::effective_actions(&mut findings, true);
        let outcome = Xcode
            .apply_with_xcode_build_state(&findings, &ctx, Some(Ok(false)))
            .unwrap();
        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
        assert_eq!(outcome.summary.items_touched, 4);
        assert!(!derived_data.join("App-pkg/Build").exists());
        assert!(!derived_data.join("App-plain").exists());
        assert!(
            derived_data
                .join("App-pkg/SourcePackages/checkouts/Example/.git/HEAD")
                .exists()
        );
        assert!(derived_data.join("App-pkg/info.plist").exists());
        crate::ops::remove_test_path(root);
    }

    /// A DerivedData folder that is itself a Git repository is never split:
    /// its directories are that repository's worktree, which a whole-folder
    /// deletion would refuse. Apply refuses a forged directory of it too.
    #[test]
    fn a_derived_data_folder_that_is_a_repository_is_never_split() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!(
                "devtrim-xcode-repository-folder-{}",
                std::process::id()
            ));
        crate::ops::remove_test_path(&root);
        std::fs::create_dir_all(&root).unwrap();
        let home = root.canonicalize().unwrap();
        let folder = home.join("Library/Developer/Xcode/DerivedData/App-repo");
        package_folder(&folder);
        std::fs::create_dir_all(folder.join(".git")).unwrap();
        std::fs::write(folder.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        let ctx = test_context(home.clone());

        let findings = Xcode.scan_with_xcode_build_state(&ctx, Ok(false)).unwrap();

        let split: Vec<_> = findings
            .iter()
            .filter_map(Finding::target)
            .filter(|path| path.parent() == Some(folder.as_path()))
            .collect();
        assert!(
            split.is_empty(),
            "PV xcode/repository-folder-scan: a repository's directories were offered: {split:?}"
        );
        let forged = Finding::new(
            "forged directory of a repository",
            Some(folder.join("Build")),
            4,
            "test",
            9,
            Action::Shred,
        );
        let outcome = Xcode
            .apply_with_xcode_build_state(&[forged], &ctx, Some(Ok(false)))
            .unwrap();
        assert!(
            outcome.summary.items_touched == 0
                && folder.join("Build/Products/Debug/output").exists(),
            "PV xcode/repository-folder-apply: {outcome:?}"
        );
        crate::ops::remove_test_path(root);
    }

    /// The checkouts are Git clones, so a finding naming the directory that
    /// holds them is refused in any spelling, however it reached apply.
    #[test]
    fn apply_never_removes_the_package_checkouts() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!(
                "devtrim-xcode-packages-kept-{}",
                std::process::id()
            ));
        crate::ops::remove_test_path(&root);
        std::fs::create_dir_all(&root).unwrap();
        let home = root.canonicalize().unwrap();
        let folder = home.join("Library/Developer/Xcode/DerivedData/App-pkg");
        package_folder(&folder);
        let ctx = test_context(home.clone());

        for name in ["SourcePackages", "sourcepackages"] {
            let forged = Finding::new(
                "forged package checkouts",
                Some(folder.join(name)),
                4,
                "test",
                9,
                Action::Shred,
            );
            let outcome = Xcode
                .apply_with_xcode_build_state(&[forged], &ctx, Some(Ok(false)))
                .unwrap();
            assert!(
                outcome.summary.items_touched == 0
                    && outcome
                        .errors
                        .iter()
                        .any(|error| error.contains("SwiftPM package checkouts")),
                "PV xcode/package-checkouts-kept: {name}: {outcome:?}"
            );
        }
        assert!(
            folder
                .join("SourcePackages/checkouts/Example/.git/HEAD")
                .exists()
        );
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
            .apply_with_xcode_build_state(&[finding], &test_context(home.clone()), None)
            .unwrap();

        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
        assert!(outcome.errors[0].contains("outside direct DeviceSupport or DerivedData"));
        assert!(sentinel.exists());
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn derived_data_apply_refuses_a_running_or_unknown_xcode_build() {
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
            .apply_with_xcode_build_state(std::slice::from_ref(&finding), &ctx, Some(Ok(true)))
            .unwrap();
        assert!(running.errors[0].contains("an Xcode build is running"));
        assert!(sentinel.exists());

        let unknown = Xcode
            .apply_with_xcode_build_state(
                &[finding],
                &ctx,
                Some(Err(anyhow::anyhow!("probe failed"))),
            )
            .unwrap();
        assert!(unknown.errors[0].contains("cannot verify Xcode build activity"));
        assert!(sentinel.exists());
        crate::ops::remove_test_path(home);
    }
}
