//! Regenerable download caches. Filesystem targets remain Trash-first.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::{Action, Finding, Op, Summary, dir_size, remove_path};
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
            let size = dir_size(&path);
            if size > 0 {
                findings.push(cache_finding(label, path, size, 3));
            }
        }
        if let Some(path) = owner_cache_path("npm", &["config", "get", "cache"], &ctx.home)? {
            let size = dir_size(&path);
            if size > 0 {
                findings.push(cache_finding("npm download cache", path, size, 2));
            }
        }
        if let Some(path) = owner_cache_path("brew", &["--cache"], &ctx.home)? {
            let size = dir_size(&path);
            if size > 0 {
                findings.push(cache_finding("homebrew downloads cache", path, size, 1));
            }
        }
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<Summary> {
        let mut notes = Vec::new();
        let mut touched = 0usize;
        let mut bytes = 0u64;
        for finding in findings {
            if !matches!(finding.action, Action::Trash | Action::Shred) {
                continue;
            }
            let path = finding
                .path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("cache finding missing path"))?;
            remove_path(std::path::Path::new(path), ctx)
                .with_context(|| format!("failed to remove {}", finding.label))?;
            bytes += finding.size_bytes;
            touched += 1;
            notes.push(format!(
                "{} {}",
                if ctx.shred {
                    "permanently deleted"
                } else {
                    "trashed"
                },
                finding.label
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

fn cache_finding(label: &str, path: PathBuf, size: u64, danger: u8) -> Finding {
    Finding {
        label: label.into(),
        path: Some(path.display().to_string()),
        size_bytes: size,
        note: "re-downloads automatically on next use".into(),
        danger: escalate(danger, size),
        action: Action::Trash,
    }
}

/// A cache root reported by an owner tool is user-controlled configuration, so it is
/// only eligible when it looks like a cache location inside the user's home. Anything
/// else (for example an `.npmrc` pointing at `~/Documents`) is refused, not deleted.
fn is_eligible_cache_root(path: &Path, home: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(home) else {
        return false;
    };
    let mut components = relative.iter();
    let Some(first) = components.next() else {
        return false;
    };
    let first = first.to_string_lossy();
    first.starts_with('.')
        || (first == "Library" && components.next().is_some_and(|c| c == "Caches"))
}

fn owner_cache_path(program: &str, args: &[&str], home: &Path) -> Result<Option<PathBuf>> {
    let Some(path) = command_path(program, args)? else {
        return Ok(None);
    };
    if !is_eligible_cache_root(&path, home) {
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
    fn only_home_cache_roots_are_eligible() {
        let home = Path::new("/Users/example");
        assert!(is_eligible_cache_root(&home.join(".npm"), home));
        assert!(is_eligible_cache_root(&home.join(".cache/uv"), home));
        assert!(is_eligible_cache_root(
            &home.join("Library/Caches/Homebrew"),
            home
        ));
        assert!(!is_eligible_cache_root(&home.join("Documents"), home));
        assert!(!is_eligible_cache_root(
            &home.join("Library/Application Support"),
            home
        ));
        assert!(!is_eligible_cache_root(Path::new("/tmp/cache"), home));
        assert!(!is_eligible_cache_root(home, home));
    }
}
