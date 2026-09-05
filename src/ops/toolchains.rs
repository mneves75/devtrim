//! swift.org toolchains under ~/Library/Developer/Toolchains.
//! Cleanup proceeds only when a valid swift-latest target and all symlink references are known.

use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::project::is_directory_if_present;
use super::{Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, dir_size, removal_note};
use crate::safety::{Ctx, escalate};

pub struct Toolchains;

impl Op for Toolchains {
    fn name(&self) -> &'static str {
        "toolchains"
    }

    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>> {
        let directory = ctx.home.join("Library/Developer/Toolchains");
        if !is_directory_if_present(&directory)? {
            return Ok(Vec::new());
        }
        let preserved = preserved_targets(&directory)?;
        let mut findings = Vec::new();
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if !file_type.is_dir()
                || path
                    .extension()
                    .is_none_or(|extension| extension != "xctoolchain")
            {
                continue;
            }
            let canonical = path
                .canonicalize()
                .with_context(|| format!("cannot verify toolchain {}", path.display()))?;
            if preserved.contains(&canonical) {
                continue;
            }
            let size = dir_size(&path)?;
            findings.push(Finding::new(
                format!(
                    "Swift toolchain {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
                Some(path),
                size,
                "not referenced by any verified Toolchains symlink; reinstallable from swift.org",
                escalate(6, size),
                Action::Trash,
            ));
        }
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let directory = ctx.home.join("Library/Developer/Toolchains");
        let preserved = preserved_targets(&directory)?;
        let mut outcome = ApplyOutcome::new(self.name());
        for finding in findings {
            if !matches!(finding.action, Action::Trash | Action::Shred) {
                continue;
            }
            let result = (|| -> Result<String> {
                let path = finding
                    .target()
                    .ok_or_else(|| anyhow::anyhow!("toolchain finding missing internal target"))?;
                authorize_toolchain_target(path, &directory, &preserved)?;
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

fn preserved_targets(directory: &Path) -> Result<BTreeSet<PathBuf>> {
    let canonical_directory = directory
        .canonicalize()
        .with_context(|| format!("cannot verify {}", directory.display()))?;
    let latest = directory.join("swift-latest.xctoolchain");
    let latest_type = std::fs::symlink_metadata(&latest)
        .with_context(|| {
            format!(
                "cannot prove required Swift toolchain link {}",
                latest.display()
            )
        })?
        .file_type();
    if !latest_type.is_symlink() {
        anyhow::bail!(
            "required Swift toolchain reference is not a symlink: {}",
            latest.display()
        );
    }
    let mut preserved = BTreeSet::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_symlink() {
            continue;
        }
        let link = entry.path();
        let target = std::fs::read_link(&link)
            .with_context(|| format!("cannot read symlink {}", link.display()))?;
        let target = if target.is_absolute() {
            target
        } else {
            directory.join(target)
        };
        let canonical = target.canonicalize().with_context(|| {
            format!(
                "cannot resolve Swift toolchain reference {}",
                link.display()
            )
        })?;
        if canonical.parent() != Some(canonical_directory.as_path())
            || !canonical.is_dir()
            || canonical
                .extension()
                .is_none_or(|extension| extension != "xctoolchain")
        {
            anyhow::bail!(
                "Swift toolchain reference escapes the verified Toolchains directory: {}",
                link.display()
            );
        }
        preserved.insert(canonical);
    }
    let latest_target = latest.canonicalize().with_context(|| {
        format!(
            "cannot resolve required Swift toolchain link {}",
            latest.display()
        )
    })?;
    if !preserved.contains(&latest_target) {
        anyhow::bail!(
            "required Swift toolchain reference is not in the verified target set: {}",
            latest.display()
        );
    }
    Ok(preserved)
}

fn authorize_toolchain_target(
    path: &Path,
    directory: &Path,
    preserved: &BTreeSet<PathBuf>,
) -> Result<PathBuf> {
    if path.parent() != Some(directory) {
        anyhow::bail!(
            "refusing toolchain target outside the direct Toolchains children: {}",
            path.display()
        );
    }
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("cannot re-verify toolchain {}", path.display()))?;
    if !metadata.file_type().is_dir()
        || path
            .extension()
            .is_none_or(|extension| extension != "xctoolchain")
    {
        anyhow::bail!(
            "refusing non-directory or non-.xctoolchain target: {}",
            path.display()
        );
    }
    let canonical_directory = directory
        .canonicalize()
        .with_context(|| format!("cannot re-verify {}", directory.display()))?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("cannot re-verify toolchain {}", path.display()))?;
    if canonical.parent() != Some(canonical_directory.as_path()) {
        anyhow::bail!(
            "refusing toolchain outside the verified Toolchains directory: {}",
            path.display()
        );
    }
    if preserved.contains(&canonical) {
        anyhow::bail!(
            "toolchain became referenced after preview: {}",
            path.display()
        );
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn temp(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("devtrim-toolchains-{name}-{}", std::process::id()));
        crate::ops::remove_test_path(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn target_temp(name: &str) -> PathBuf {
        let path = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-toolchains-{name}-{}", std::process::id()));
        crate::ops::remove_test_path(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn test_context(home: PathBuf) -> Ctx {
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
    fn missing_or_broken_latest_fails_closed() {
        let directory = temp("missing");
        std::fs::create_dir_all(directory.join("swift-1.xctoolchain")).unwrap();
        assert!(preserved_targets(&directory).is_err());
        symlink(
            "missing.xctoolchain",
            directory.join("swift-latest.xctoolchain"),
        )
        .unwrap();
        assert!(preserved_targets(&directory).is_err());
        crate::ops::remove_test_path(directory);
    }

    #[test]
    fn preserves_every_symlink_target() {
        let directory = temp("preserved");
        let first = directory.join("swift-1.xctoolchain");
        let second = directory.join("swift-2.xctoolchain");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        symlink(
            "swift-2.xctoolchain",
            directory.join("swift-latest.xctoolchain"),
        )
        .unwrap();
        symlink("swift-1.xctoolchain", directory.join("custom.xctoolchain")).unwrap();
        let preserved = preserved_targets(&directory).unwrap();
        assert_eq!(preserved.len(), 2);
        crate::ops::remove_test_path(directory);
    }

    #[test]
    fn apply_accepts_only_direct_unreferenced_toolchain_directories() {
        let root = target_temp("apply-direct");
        let home = root.canonicalize().unwrap();
        let directory = home.join("Library/Developer/Toolchains");
        let preserved = directory.join("swift-2.xctoolchain");
        let stale = directory.join("swift-1.xctoolchain");
        std::fs::create_dir_all(&preserved).unwrap();
        std::fs::create_dir_all(&stale).unwrap();
        symlink(
            "swift-2.xctoolchain",
            directory.join("swift-latest.xctoolchain"),
        )
        .unwrap();
        let finding = Finding::new(
            "stale Swift toolchain",
            Some(stale.clone()),
            0,
            "test",
            9,
            Action::Shred,
        );
        let outcome = Toolchains
            .apply(&[finding], &test_context(home.clone()))
            .unwrap();

        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
        assert_eq!(outcome.summary.items_touched, 1);
        assert!(!stale.exists());
        assert!(preserved.exists());
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn apply_rejects_forged_nested_toolchain_target() {
        let root = target_temp("apply-nested");
        let home = root.canonicalize().unwrap();
        let directory = home.join("Library/Developer/Toolchains");
        let preserved = directory.join("swift-2.xctoolchain");
        let nested = directory.join("container/swift-1.xctoolchain");
        std::fs::create_dir_all(&preserved).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        symlink(
            "swift-2.xctoolchain",
            directory.join("swift-latest.xctoolchain"),
        )
        .unwrap();
        let finding = Finding::new(
            "forged nested Swift toolchain",
            Some(nested.clone()),
            0,
            "test",
            9,
            Action::Shred,
        );
        let outcome = Toolchains
            .apply(&[finding], &test_context(home.clone()))
            .unwrap();

        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
        assert!(outcome.errors[0].contains("direct Toolchains children"));
        assert!(nested.exists());
        crate::ops::remove_test_path(root);
    }
}
