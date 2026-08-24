//! Regenerable download caches. Filesystem targets remain Trash-first.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::{Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, dir_size};
use crate::report::TargetAuthority;
use crate::safety::{Ctx, escalate};

pub struct Caches;

const CACHES: &[(&str, &str)] = &[
    ("huggingface model cache", ".cache/huggingface"),
    ("uv package cache", ".cache/uv"),
    ("node core cache", ".cache/node"),
];

impl Op for Caches {
    fn name(&self) -> &'static str {
        "caches"
    }

    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        for (label, relative) in CACHES {
            let path = ctx.home.join(relative);
            let size = dir_size(&path)?;
            if size > 0 {
                findings.push(cache_finding(label, path, size, 3));
            }
        }
        if let Some(path) = owner_cache_path("npm", &["config", "get", "cache"], &ctx.home)? {
            let size = dir_size(&path)?;
            if size > 0 {
                findings.push(
                    cache_finding("npm download cache", path, size, 2)
                        .with_authority(TargetAuthority::NpmCache),
                );
            }
        }
        if let Some(path) = owner_cache_path("brew", &["--cache"], &ctx.home)? {
            let size = dir_size(&path)?;
            if size > 0 {
                findings.push(
                    cache_finding("homebrew downloads cache", path, size, 1)
                        .with_authority(TargetAuthority::BrewCache),
                );
            }
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
                authorize_cache_finding(finding, &ctx.home)?;
                apply_filesystem_finding(finding, ctx)
            })()
            .with_context(|| format!("failed to remove {}", finding.label));
            if let Err(error) = result {
                outcome.fail(error);
                break;
            }
            outcome.record(
                finding,
                format!(
                    "{} {}",
                    if finding.action == Action::Shred {
                        "permanently deleted"
                    } else {
                        "trashed"
                    },
                    finding.label
                ),
            );
        }
        Ok(outcome)
    }
}

fn cache_finding(label: &str, path: PathBuf, size: u64, danger: u8) -> Finding {
    Finding::new(
        label,
        Some(path),
        size,
        "re-downloads automatically on next use",
        escalate(danger, size),
        Action::Trash,
    )
}

fn authorize_cache_finding(finding: &Finding, home: &Path) -> Result<()> {
    let target = finding
        .target()
        .ok_or_else(|| anyhow::anyhow!("cache finding missing internal target"))?;
    let authorized = match finding.authority() {
        TargetAuthority::Standard => is_builtin_cache_root(target, home),
        TargetAuthority::NpmCache => is_eligible_owner_cache("npm", target, home),
        TargetAuthority::BrewCache => is_eligible_owner_cache("brew", target, home),
    };
    if !authorized {
        anyhow::bail!(
            "cache target is outside its authorized namespace: {}",
            target.display()
        );
    }
    Ok(())
}

fn is_builtin_cache_root(path: &Path, home: &Path) -> bool {
    CACHES
        .iter()
        .any(|(_, relative)| path == home.join(relative))
}
/// Owner-reported paths are trusted only inside the owner's exact cache namespace.
fn is_eligible_owner_cache(program: &str, path: &Path, home: &Path) -> bool {
    let (Some(path), Some(home)) = (normalized_absolute(path), normalized_absolute(home)) else {
        return false;
    };
    match program {
        "npm" => is_within(&path, &home.join(".npm")) || is_within(&path, &home.join(".cache/npm")),
        "brew" => is_within(&path, &home.join("Library/Caches/Homebrew")),
        _ => false,
    }
}

fn normalized_absolute(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => return None,
            std::path::Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    Some(normalized)
}

fn is_within(path: &Path, root: &Path) -> bool {
    path == root || path.starts_with(root)
}

fn owner_cache_path(program: &str, args: &[&str], home: &Path) -> Result<Option<PathBuf>> {
    let Some(path) = command_path(program, args)? else {
        return Ok(None);
    };
    if !is_eligible_owner_cache(program, &path, home) {
        eprintln!(
            "warn `{program}` reports cache root {} outside a home cache location; skipping",
            path.display()
        );
        return Ok(None);
    }
    Ok(Some(path))
}

fn command_path(program: &str, args: &[&str]) -> Result<Option<PathBuf>> {
    let output = match std::process::Command::new(program).args(args).output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !output.status.success() {
        anyhow::bail!("`{program} {}` failed", args.join(" "));
    }
    let value = String::from_utf8(output.stdout).context("command returned non-UTF-8 path")?;
    let value = value.trim();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(value)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_cache_roots_are_program_specific_and_normalized() {
        let home = Path::new("/Users/example");
        for path in [
            home.join(".npm"),
            home.join(".npm/_cacache"),
            home.join(".cache/npm"),
            home.join(".cache/npm/content"),
        ] {
            assert!(is_eligible_owner_cache("npm", &path, home));
        }
        for path in [
            home.join("Library/Caches/Homebrew"),
            home.join("Library/Caches/Homebrew/downloads"),
        ] {
            assert!(is_eligible_owner_cache("brew", &path, home));
        }
        for path in [
            home.to_path_buf(),
            home.join(".cache"),
            home.join(".cache/uv"),
            home.join(".aws"),
            home.join(".config"),
            home.join(".local"),
            home.join("Documents"),
            home.join("Library/Caches"),
            home.join(".npm/../.ssh"),
            home.join(".cache/npm/../../.ssh"),
            PathBuf::from("/tmp/cache"),
            PathBuf::from(".npm"),
        ] {
            assert!(!is_eligible_owner_cache("npm", &path, home));
            assert!(!is_eligible_owner_cache("brew", &path, home));
        }
        assert!(!is_eligible_owner_cache("brew", &home.join(".npm"), home));
        assert!(!is_eligible_owner_cache(
            "npm",
            &home.join("Library/Caches/Homebrew"),
            home,
        ));
    }
    #[test]
    fn apply_reasserts_owner_namespace_and_preserves_sentinel() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-cache-auth-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(home.join(".aws")).unwrap();
        let home = home.canonicalize().unwrap();
        let sentinel = home.join(".aws/sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let finding = Finding::new(
            "forged npm cache",
            Some(home.join(".aws")),
            4,
            "test",
            9,
            Action::Shred,
        )
        .with_authority(TargetAuthority::NpmCache);
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            home: home.clone(),
            interactive: false,
        };

        let outcome = Caches.apply(&[finding], &ctx).unwrap();
        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
        assert!(sentinel.exists());
        crate::ops::remove_test_path(home);
    }
}
