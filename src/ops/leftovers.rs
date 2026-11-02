//! Read-only hints for possible agent leftovers.
//! Whole worktree staleness is not decidable, so this category never deletes them.

use anyhow::{Context, Result};

use std::path::Path;

use super::project::{self, is_directory_if_present};
use super::{Action, ApplyOutcome, Finding, Op, dir_size};
use crate::safety::Ctx;
use crate::safety::measure_lenient;

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
            for repository in project::repositories_under(root, &mut unread)? {
                // Git fails every query in a worktree whose repository is
                // gone; there is nothing of its own to judge, as in `worktrees`.
                if project::orphaned_worktree(&repository).unwrap_or(false) {
                    continue;
                }
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
                    let size = match dir_size(&path) {
                        Ok(size) => size,
                        Err(error) => {
                            unread_once(&mut unread, &path, &error);
                            continue;
                        }
                    };
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
                // A folder the walk cannot read is named once, beside the
                // repositories walk's own unread folders (S8).
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        let path = error.path().unwrap_or(root).to_path_buf();
                        unread_once(&mut unread, &path, &anyhow::Error::from(error));
                        continue;
                    }
                };
                if entry.file_name() != ".supergoal" || !entry.file_type().is_dir() {
                    continue;
                }
                for child in ["evidence", "perf"] {
                    let path = entry.path().join(child);
                    if is_directory_if_present(&path)? {
                        let size = match dir_size(&path) {
                            Ok(size) => size,
                            Err(error) => {
                                unread_once(&mut unread, &path, &error);
                                continue;
                            }
                        };
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
            findings.push(project::unread_folders_finding("leftovers", &unread));
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
        let linked =
            std::fs::symlink_metadata(child.join(".git")).is_ok_and(|metadata| metadata.is_file());
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

/// Records a folder this scan could not read, once however many walks reach it.
fn unread_once(unread: &mut Vec<project::UnreadFolder>, path: &Path, error: &anyhow::Error) {
    if !unread.iter().any(|folder| folder.path == path) {
        unread.push(project::UnreadFolder::new(path, error));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder below a root that cannot be read is named once in one error
    /// finding; everything else under the root is still reported.
    #[test]
    fn an_unreadable_folder_withholds_only_itself() {
        let fixture = crate::ops::TestFixture::new("devtrim-leftovers-unread");
        std::fs::create_dir_all(fixture.path()).unwrap();
        let home = fixture.path().canonicalize().unwrap();
        let root = home.join("dev");
        let evidence = root.join(".supergoal/evidence");
        std::fs::create_dir_all(&evidence).unwrap();
        std::fs::write(evidence.join("run.log"), "log").unwrap();
        let sealed = crate::ops::Unreadable::new(&root.join("sealed"), 0o000);
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: vec![root.clone()],
            roots_origin: crate::safety::RootsOrigin::Flag,
            active_days: 30,
            retain_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home: home.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
            apply_results: Default::default(),
        };

        let findings = Leftovers.scan(&ctx, &Default::default()).unwrap();

        drop(sealed);
        let reported = findings
            .iter()
            .filter_map(Finding::target)
            .collect::<Vec<_>>();
        assert_eq!(reported, vec![evidence.as_path()]);
        let errors = findings
            .iter()
            .filter_map(Finding::scan_error)
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 1, "{findings:?}");
        assert_eq!(errors[0].matches("sealed").count(), 1, "{errors:?}");
    }
}
