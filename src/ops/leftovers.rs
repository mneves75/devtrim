//! Read-only hints for possible agent leftovers.
//! Whole worktree staleness is not decidable, so this category never deletes them.

use anyhow::{Context, Result};

use std::path::Path;

use super::project::{self, is_directory_if_present};
use super::{Action, ApplyOutcome, Finding, Op, dir_size};
use crate::safety::Ctx;
use crate::worktrees::{measure_lenient, repositories_under};

pub struct Leftovers;

fn is_agent_scratch(name: &str) -> bool {
    let Some((prefix, suffix)) = name.rsplit_once('.') else {
        return false;
    };
    !prefix.is_empty()
        && (prefix.starts_with("kimi-") || matches!(prefix, "codex-worktree" | "claude-worktree"))
        && suffix.len() == 6
        && suffix
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
}

impl Op for Leftovers {
    fn name(&self) -> &'static str {
        "leftovers"
    }

    fn scans_roots(&self) -> bool {
        true
    }

    fn scan(
        &self,
        ctx: &Ctx,
        _observations: &super::project::ScanObservations,
    ) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        let mut unread = Vec::new();
        for root in project::normalized_roots(&ctx.roots) {
            if !is_directory_if_present(root)? {
                continue;
            }
            for repository in repositories_under(root, &mut unread)? {
                if let Err(error) = scratch_findings(&repository, &mut findings) {
                    findings.push(project::unjudged_finding(".scratch", &repository, &error));
                }
            }
            for result in std::fs::read_dir(root)
                .with_context(|| format!("cannot scan leftovers under {}", root.display()))?
            {
                let entry = result
                    .with_context(|| format!("cannot scan leftovers under {}", root.display()))?;
                let path = entry.path();
                let file_type = entry.file_type().with_context(|| {
                    format!("cannot inspect leftover candidate {}", path.display())
                })?;
                if file_type.is_dir() && is_agent_scratch(&entry.file_name().to_string_lossy()) {
                    let size = dir_size(&path)?;
                    findings.push(Finding::new(
                        format!(
                            "possible agent scratch `{}`",
                            entry.file_name().to_string_lossy()
                        ),
                        Some(path),
                        size,
                        "review manually; worktree staleness cannot be proven",
                        1,
                        Action::Info,
                    ));
                }
            }
            for entry in walkdir::WalkDir::new(root)
                .max_depth(2)
                .follow_links(false)
                .into_iter()
            {
                let entry = entry
                    .with_context(|| format!("cannot scan leftovers under {}", root.display()))?;
                if entry.file_name() != ".supergoal" || !entry.file_type().is_dir() {
                    continue;
                }
                for child in ["evidence", "perf"] {
                    let path = entry.path().join(child);
                    if is_directory_if_present(&path)? {
                        let size = dir_size(&path)?;
                        findings.push(Finding::new(
                            format!("supergoal {child} artifacts"),
                            Some(path),
                            size,
                            "review manually; completion cannot be inferred from a path",
                            1,
                            Action::Info,
                        ));
                    }
                }
            }
        }
        if !unread.is_empty() {
            findings.push(project::unread_folders_finding("repositories", &unread));
        }
        Ok(findings)
    }

    fn apply(&self, _findings: &[Finding], _ctx: &Ctx) -> Result<ApplyOutcome> {
        let mut outcome = ApplyOutcome::new(self.name());
        outcome
            .summary
            .notes
            .push("leftovers are report-only; review them manually".into());
        Ok(outcome)
    }
}

/// The direct real-directory children of a repository's top-level `.scratch`,
/// when Git ignores that folder and tracks nothing in it. Git runs only when
/// the folder exists; anything that is not ignored, holds a tracked file or is
/// a link is somebody's working data and is not listed.
fn scratch_findings(repository: &Path, findings: &mut Vec<Finding>) -> Result<()> {
    let scratch = repository.join(".scratch");
    match std::fs::symlink_metadata(&scratch) {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", scratch.display()));
        }
    }
    if !project::ignores_path(repository, ".scratch/")?
        || project::tracks_files_under(repository, &scratch)?
    {
        return Ok(());
    }
    let mut children = Vec::new();
    for entry in
        std::fs::read_dir(&scratch).with_context(|| format!("cannot read {}", scratch.display()))?
    {
        let entry = entry.with_context(|| format!("cannot read {}", scratch.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("cannot inspect {}", entry.path().display()))?;
        if file_type.is_dir() {
            children.push(entry.path());
        }
    }
    children.sort();
    for child in children {
        let name = child.file_name().unwrap_or_default().to_string_lossy();
        let linked = std::fs::symlink_metadata(child.join(".git")).is_ok_and(|meta| meta.is_file());
        let label = if linked {
            format!("linked worktree in .scratch `{name}`")
        } else {
            format!("repository scratch `{name}`")
        };
        let measured = measure_lenient(&child, &Default::default());
        let mut note = "review manually; ignored files are still your data".to_owned();
        if measured.unread > 0 {
            note.push_str(&format!(
                "; lower bound: {} entries could not be read",
                measured.unread
            ));
        }
        findings.push(Finding::new(
            label,
            Some(child.clone()),
            measured.bytes,
            &note,
            1,
            Action::Info,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_pattern_is_only_a_hint() {
        assert!(is_agent_scratch("kimi-plan005-validation.RA8ERh"));
        assert!(!is_agent_scratch("my-project"));
        assert!(!is_agent_scratch("src.main"));
    }
}
