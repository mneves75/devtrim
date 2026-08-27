//! Read-only hints for possible agent leftovers.
//! Whole worktree staleness is not decidable, so this category never deletes them.

use anyhow::{Context, Result};

use super::project::is_directory_if_present;
use super::{Action, ApplyOutcome, Finding, Op, dir_size};
use crate::safety::Ctx;

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

    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        for root in &ctx.roots {
            if !is_directory_if_present(root)? {
                continue;
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
                    findings.push(Finding::new(
                        format!(
                            "possible agent scratch `{}`",
                            entry.file_name().to_string_lossy()
                        ),
                        Some(path.clone()),
                        dir_size(&path)?,
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
                        findings.push(Finding::new(
                            format!("supergoal {child} artifacts"),
                            Some(path.clone()),
                            dir_size(&path)?,
                            "review manually; completion cannot be inferred from a path",
                            1,
                            Action::Info,
                        ));
                    }
                }
            }
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
