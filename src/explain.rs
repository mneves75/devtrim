//! `devtrim explain <PATH>`: why a path is or is not offered. Report-only — one
//! fresh scan and the protection verdicts; it never applies and never creates
//! deletion authority.

use anyhow::{Context, Result};
use colored::Colorize;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::report::{self, Finding};
use crate::{ops, safety::Ctx};

#[derive(serde::Serialize)]
struct Document<'a> {
    operation: &'static str,
    path: String,
    protected: Protected,
    findings: Vec<&'a Finding>,
    errors: &'a [String],
}

#[derive(serde::Serialize)]
struct Protected {
    builtin: bool,
    config: Option<String>,
}

/// How an offered target relates to the explained path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Relation {
    Exact,
    /// The target holds the path, so removing it would remove the path.
    Holds,
    /// The target lies inside the path.
    Inside,
}

pub(crate) fn run(ctx: &Ctx, path: &Path) -> Result<ExitCode> {
    let path = resolve(path)?;
    let builtin = crate::safety::protection_reason(&path, &ctx.home);
    let config = crate::safety::config_protect_entry(&path, &ctx.protect).map(Path::to_path_buf);
    let scan = ops::scan_all(ctx);
    let subject = Subject::of(&path);
    let offers = scan
        .sections
        .iter()
        .flat_map(|section| {
            scan.findings[section.range.clone()]
                .iter()
                .filter_map(|finding| {
                    let target = finding.target()?;
                    subject
                        .relation(&Target::of(target))
                        .map(|relation| (section.category, finding, relation))
                })
        })
        .collect::<Vec<_>>();

    if ctx.json {
        let document = Document {
            operation: "explain",
            path: path.display().to_string(),
            protected: Protected {
                builtin: builtin.is_some(),
                config: config.as_ref().map(|entry| entry.display().to_string()),
            },
            findings: offers.iter().map(|(_, finding, _)| *finding).collect(),
            errors: &scan.errors,
        };
        let mut output =
            serde_json::to_vec_pretty(&document).context("cannot serialize the explanation")?;
        output.push(b'\n');
        report::write_stdout(&output)?;
    } else {
        let shown = report::terminal_safe(&path.display().to_string());
        report::print_line(&format!("{} {shown}", "explain".bold()))?;
        report::print_line(&match &builtin {
            Some(reason) => format!(
                "  built-in protection: {} ({})",
                "refuses it".red(),
                report::terminal_safe(reason)
            ),
            None => "  built-in protection: does not refuse it".to_owned(),
        })?;
        report::print_line(&match &config {
            Some(entry) => format!(
                "  configured protect: {} (entry {})",
                "refuses it".red(),
                report::terminal_safe(&entry.display().to_string())
            ),
            None => "  configured protect: no entry refuses it".to_owned(),
        })?;
        for (category, finding, relation) in &offers {
            let relation = match relation {
                Relation::Exact => "this path",
                Relation::Holds => "holds this path",
                Relation::Inside => "inside this path",
            };
            let size = match finding.frees_at_least_text() {
                Some(text) => format!("{} ({text})", report::gb(finding.size_bytes)),
                None => report::gb(finding.size_bytes),
            };
            report::print_line(&format!(
                "  {} {}: {} ({relation})\n    {size} · {} · danger:{} · {}",
                "offered by".green(),
                category,
                report::terminal_safe(finding.path.as_deref().unwrap_or_default()),
                report::human_action_display(&finding.action),
                finding.danger,
                report::terminal_safe(&finding.note)
            ))?;
        }
        if offers.is_empty() {
            report::print_line(&format!("  no category offers {shown}"))?;
            let outside = !ctx
                .roots
                .iter()
                .any(|root| path.starts_with(root) || root.starts_with(&path));
            if outside {
                report::print_line(&format!(
                    "  {shown} is outside every scan root; {}",
                    report::terminal_safe(&ctx.roots_note())
                ))?;
            }
        }
        for error in &scan.errors {
            eprintln!("{} {}", "warn".yellow(), report::terminal_safe(error));
        }
    }
    Ok(if scan.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// `path` made absolute against the current directory, with its folder
/// resolved as the scanners resolve theirs so it compares with their
/// canonical targets. The last component is never followed: a link is
/// explained as itself, as the deletion sink would judge it.
pub(crate) fn resolve(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("cannot read the current directory")?
            .join(path)
    };
    let literal = crate::safety::clean(&absolute);
    let (Some(parent), Some(leaf)) = (literal.parent(), literal.file_name()) else {
        return Ok(literal);
    };
    Ok(parent
        .canonicalize()
        .map_or(literal.clone(), |parent| parent.join(leaf)))
}

/// Component-wise, never by string prefix: `/a/bc` is not inside `/a/b`.
fn relation(target: &Path, path: &Path) -> Option<Relation> {
    if target == path {
        Some(Relation::Exact)
    } else if path.starts_with(target) {
        Some(Relation::Holds)
    } else if target.starts_with(path) {
        Some(Relation::Inside)
    } else {
        None
    }
}

/// A path compared with offered targets: its spelling, and the device and
/// inode of the path and of each folder above it, so a target spelled
/// differently from the path — a case variant on a case-insensitive volume —
/// still relates to it. The path itself is read without following a link; a
/// regular file with further hard links is left to its spelling, since
/// another link is a different name the target's removal keeps.
pub(crate) struct Subject {
    path: PathBuf,
    own: Option<(u64, u64)>,
    above: Vec<(u64, u64)>,
}

impl Subject {
    pub(crate) fn of(path: &Path) -> Self {
        use std::os::unix::fs::MetadataExt as _;

        let own = std::fs::symlink_metadata(path)
            .ok()
            .filter(|metadata| !(metadata.is_file() && metadata.nlink() > 1))
            .map(|metadata| (metadata.dev(), metadata.ino()));
        let above = match own {
            Some(_) => path.ancestors().skip(1).filter_map(identity).collect(),
            None => Vec::new(),
        };
        Self {
            path: path.to_path_buf(),
            own,
            above,
        }
    }

    /// By whole components first, then by identity.
    pub(crate) fn relation(&self, target: &Target) -> Option<Relation> {
        relation(&target.path, &self.path).or_else(|| {
            let path = self.own?;
            let held = target.own?;
            if held == path {
                Some(Relation::Exact)
            } else if self.above.contains(&held) {
                Some(Relation::Holds)
            } else if target.above.contains(&path) {
                Some(Relation::Inside)
            } else {
                None
            }
        })
    }
}

/// An offered target with its identities read once, so one target can be
/// compared with many paths without reading the volume again.
pub(crate) struct Target {
    path: PathBuf,
    own: Option<(u64, u64)>,
    above: Vec<(u64, u64)>,
}

impl Target {
    pub(crate) fn of(path: &Path) -> Self {
        let own = identity(path);
        let above = match own {
            Some(_) => path.ancestors().skip(1).filter_map(identity).collect(),
            None => Vec::new(),
        };
        Self {
            path: path.to_path_buf(),
            own,
            above,
        }
    }
}

/// Device and inode of `path` itself, a link not followed.
fn identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt as _;

    std::fs::symlink_metadata(path)
        .ok()
        .map(|metadata| (metadata.dev(), metadata.ino()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_relates_to_the_path_by_whole_components() {
        let relate = |target: &str, path: &str| {
            relation(Path::new(target), Path::new(path)).map(|relation| match relation {
                Relation::Exact => "exact",
                Relation::Holds => "holds",
                Relation::Inside => "inside",
            })
        };
        assert_eq!(relate("/a/b", "/a/b/"), Some("exact"));
        assert_eq!(relate("/a/b", "/a/b/c"), Some("holds"));
        assert_eq!(relate("/a/b/c", "/a/b"), Some("inside"));
        assert_eq!(relate("/a/b", "/a/bc"), None);
        assert_eq!(relate("/a/bc", "/a/b"), None);
    }
}
