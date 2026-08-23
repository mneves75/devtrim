//! swift.org toolchains under ~/Library/Developer/Toolchains.
//! Cleanup proceeds only when a valid swift-latest target and all symlink references are known.

use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::{Action, Finding, Op, Summary, dir_size, remove_path};
use crate::safety::{Ctx, escalate};

pub struct Toolchains;

impl Op for Toolchains {
    fn name(&self) -> &'static str {
        "toolchains"
    }

    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>> {
        let directory = ctx.home.join("Library/Developer/Toolchains");
        if !directory.is_dir() {
            return Ok(Vec::new());
        }
        let Some(preserved) = preserved_targets(&directory)? else {
            return Ok(Vec::new());
        };
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
            let size = dir_size(&path);
            findings.push(Finding {
                label: format!(
                    "Swift toolchain {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
                path: Some(path.display().to_string()),
                size_bytes: size,
                note: "not referenced by any verified Toolchains symlink; reinstallable from swift.org"
                    .into(),
                danger: escalate(6, size),
                action: Action::Trash,
            });
        }
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<Summary> {
        let directory = ctx.home.join("Library/Developer/Toolchains");
        let preserved = preserved_targets(&directory)?
            .ok_or_else(|| anyhow::anyhow!("cannot prove a preserved Swift toolchain"))?;
        let mut notes = Vec::new();
        let mut touched = 0usize;
        let mut bytes = 0u64;
        for finding in findings {
            if !matches!(finding.action, Action::Trash | Action::Shred) {
                continue;
            }
            let path = PathBuf::from(
                finding
                    .path
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("toolchain finding missing path"))?,
            );
            let canonical = path
                .canonicalize()
                .with_context(|| format!("cannot re-verify toolchain {}", path.display()))?;
            if preserved.contains(&canonical) {
                anyhow::bail!(
                    "toolchain became referenced after preview: {}",
                    path.display()
                );
            }
            remove_path(&path, ctx)?;
            touched += 1;
            bytes += finding.size_bytes;
            notes.push(format!(
                "{} {}",
                if ctx.shred {
                    "permanently deleted"
                } else {
                    "trashed"
                },
                path.display()
            ));
        }
        Ok(Summary {
            op: self.name().into(),
            items_touched: touched,
            bytes_freed_estimate: bytes,
            notes,
        })
    }
}

fn preserved_targets(directory: &Path) -> Result<Option<BTreeSet<PathBuf>>> {
    let canonical_directory = directory
        .canonicalize()
        .with_context(|| format!("cannot verify {}", directory.display()))?;
    let latest = directory.join("swift-latest.xctoolchain");
    if !latest.is_symlink() {
        return Ok(None);
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
        let canonical = match target.canonicalize() {
            Ok(canonical) => canonical,
            Err(_) => return Ok(None),
        };
        if canonical.parent() != Some(canonical_directory.as_path())
            || !canonical.is_dir()
            || canonical
                .extension()
                .is_none_or(|extension| extension != "xctoolchain")
        {
            return Ok(None);
        }
        preserved.insert(canonical);
    }
    let latest_target = latest.canonicalize().ok();
    if latest_target
        .as_ref()
        .is_none_or(|target| !preserved.contains(target))
    {
        return Ok(None);
    }
    Ok(Some(preserved))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn temp(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("devtrim-toolchains-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn missing_or_broken_latest_fails_closed() {
        let directory = temp("missing");
        std::fs::create_dir_all(directory.join("swift-1.xctoolchain")).unwrap();
        assert!(preserved_targets(&directory).unwrap().is_none());
        symlink(
            "missing.xctoolchain",
            directory.join("swift-latest.xctoolchain"),
        )
        .unwrap();
        assert!(preserved_targets(&directory).unwrap().is_none());
        std::fs::remove_dir_all(directory).ok();
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
        let preserved = preserved_targets(&directory).unwrap().unwrap();
        assert_eq!(preserved.len(), 2);
        std::fs::remove_dir_all(directory).ok();
    }
}
