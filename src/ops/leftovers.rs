//! AI-agent scratch leftovers: validation dirs with random suffixes,
//! .supergoal evidence/perf trees, stale xcresult bundles.

use anyhow::Result;
use std::path::PathBuf;

use super::{Finding, Op, Summary, dir_size, remove_path};
use crate::safety::{Ctx, escalate};

pub struct Leftovers;

/// Directory names that look like agent worktree scratch: prefix.suffix6
fn is_agent_scratch(name: &str) -> bool {
    let Some((prefix, suffix)) = name.rsplit_once('.') else {
        return false;
    };
    !prefix.is_empty()
        && (prefix.starts_with("kimi-")
            || matches!(prefix, "codex-worktree" | "claude-worktree"))
        && suffix.len() == 6
        && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

impl Op for Leftovers {
    fn name(&self) -> &'static str {
        "leftovers"
    }
    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>> {
        let mut out = Vec::new();
        for root in &ctx.roots {
            if !root.is_dir() {
                continue;
            }
            // Depth-1 scratch dirs (random-suffix naming)
            if let Ok(entries) = std::fs::read_dir(root) {
                for e in entries.flatten() {
                    let p = e.path();
                    if p.is_dir() && is_agent_scratch(&e.file_name().to_string_lossy()) {
                        let size = dir_size(&p);
                        out.push(Finding {
                            label: format!("agent scratch dir `{}`", e.file_name().to_string_lossy()),
                            path: Some(p.display().to_string()),
                            size_bytes: size,
                            note: "matches agent worktree scratch pattern (prefix.abc123)".into(),
                            danger: escalate(5, size),
                            action: "trash".into(),
                        });
                    }
                }
            }
            // .supergoal evidence/perf under any project (depth 2)
            for e in walkdir::WalkDir::new(root)
                .max_depth(2)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                if e.file_name() == ".supergoal" && e.file_type().is_dir() {
                    for sub in ["evidence", "perf"] {
                        let p = e.path().join(sub);
                        if p.is_dir() {
                            let size = dir_size(&p);
                            if size > 0 {
                                out.push(Finding {
                                    label: format!("supergoal {sub} artifacts"),
                                    path: Some(p.display().to_string()),
                                    size_bytes: size,
                                    note: "past mission evidence; mission already delivered".into(),
                                    danger: escalate(5, size),
                                    action: "trash".into(),
                                });
                            }
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<Summary> {
        let mut notes = Vec::new();
        let mut touched = 0usize;
        let mut bytes = 0u64;
        for f in findings {
            let Some(path) = &f.path else { continue };
            remove_path(PathBuf::from(path).as_path(), ctx)?;
            bytes += f.size_bytes;
            touched += 1;
            notes.push(format!("trashed {path}"));
        }
        Ok(Summary {
            op: self.name().into(),
            items_touched: touched,
            bytes_freed_estimate: bytes,
            notes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_pattern_matches_session_examples() {
        assert!(is_agent_scratch("kimi-plan005-validation-resume.3rjuOR"));
        assert!(is_agent_scratch("kimi-plan005-validation.RA8ERh"));
        assert!(!is_agent_scratch("my-project"));          // no dot suffix pair
        assert!(!is_agent_scratch("src.main"));             // suffix not 6 alnum
        assert!(!is_agent_scratch(".hidden.xxxxxx"));       // empty prefix
    }
}
