//! Coding-agent caches and stale session history. Filesystem targets remain Trash-first.
//!
//! Two tiers with different promises, because they are not the same kind of data:
//!
//! * A *regenerable* entry is a cache the agent rebuilds on demand. Removing one
//!   under a live session costs a re-fetch, nothing else, so it is offered
//!   unconditionally at a low danger score.
//! * A *history* entry is a session transcript or a shell snapshot. Nothing
//!   regenerates it. It is therefore offered only after the
//!   configured active window has passed over the whole subtree, and its note
//!   says plainly that the content does not come back.
//!
//! Authentication material (`auth.json`, `.credentials.json`), configuration,
//! memories, skills, agent definitions, installed plugins, and the `.claude.json`
//! backup copies are never in either list and are never traversed as candidates.
//!
//! Stores are left out for two distinct reasons.
//!
//! Some hold live state despite a name that suggests otherwise, and no age gate
//! can see it. `~/.claude/jobs/<id>` is the background-session supervisor's
//! state — `state.json`, `timeline.jsonl`, `tmp` — not job output: a pinned
//! session is kept alive while idle and a shed one is woken from that state, so
//! an idle stretch past the active window would offer a directory a live
//! process still owns. The `pins.json` beside it records exactly that, and a
//! category that has to consult a liveness file to stay safe is one directory
//! too far.
//!
//! Others simply cost more preview than they return, because a preview nobody
//! can read is not a preview. Against the machine this was built on, Codex lane
//! transcripts produced 558 findings for 0.25 GB, its `.tmp` tree 324 for
//! 0.07 GB, the Claude Code file-edit history 123 for 0.10 GB, and the paste
//! cache a comparable count for about 2 MB.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, dir_size, removal_note};
use crate::safety::{Ctx, escalate};

pub struct Agents;

/// Caches an agent rebuilds on demand. Exact paths relative to `$HOME`; nothing
/// is matched by prefix, so a sibling directory can never inherit authority.
const REGENERABLE: &[(&str, &str, u8)] = &[
    ("Claude Code downloads", ".claude/downloads", 2),
    ("Claude Code metadata cache", ".claude/cache", 2),
    ("Codex catalog cache", ".codex/cache", 2),
    ("Pi web-search cache", ".pi/web-search-cache", 2),
    ("OpenCode cache", ".cache/opencode", 2),
];

/// A store whose children are per-session history rather than cache.
struct HistoryRoot {
    label: &'static str,
    /// Exact path relative to `$HOME`.
    relative: &'static str,
    /// Levels below `relative` at which one child becomes one finding. Codex
    /// nests sessions as `<year>/<month>/<day>`, so waiting for a whole year to
    /// go stale would never offer the current one.
    depth: usize,
    /// Whether a regular file at that depth is also a candidate. Roots that hold
    /// loose transcripts set it; roots that hold one directory per session do
    /// not, which is what keeps `jobs/pins.json` out of the plan.
    include_files: bool,
}

const HISTORY: &[HistoryRoot] = &[
    // A shell snapshot is written once per session and sourced by every later
    // shell call in that session; nothing rewrites it if it disappears. It is
    // therefore history, not cache.
    //
    // The age gate is the right signal here, and not for the reason it was
    // wrong for `jobs`: Claude Code sweeps this directory itself once an entry
    // passes its own `cleanupPeriodDays` retention, so age is the vendor's own
    // criterion here and devtrim only reaches the same conclusion sooner when
    // the configured window is shorter, with every finding stating the age it
    // used. `jobs` had no such sweep and did have a liveness file beside it,
    // which is what made age the wrong signal there.
    HistoryRoot {
        label: "Claude Code shell snapshots",
        relative: ".claude/shell-snapshots",
        depth: 1,
        include_files: true,
    },
    HistoryRoot {
        label: "Codex shell snapshots",
        relative: ".codex/shell_snapshots",
        depth: 1,
        include_files: true,
    },
    HistoryRoot {
        label: "Codex session transcripts",
        relative: ".codex/sessions",
        depth: 3,
        include_files: true,
    },
    HistoryRoot {
        label: "Codex archived sessions",
        relative: ".codex/archived_sessions",
        depth: 1,
        include_files: true,
    },
];

const DAY: u64 = 60 * 60 * 24;

impl Op for Agents {
    fn name(&self) -> &'static str {
        "agents"
    }

    fn scan(
        &self,
        ctx: &Ctx,
        _observations: &super::project::ScanObservations,
    ) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        for (label, relative, danger) in REGENERABLE {
            let path = ctx.home.join(relative);
            let size = dir_size(&path)?;
            if size > 0 {
                findings.push(Finding::new(
                    *label,
                    Some(path),
                    size,
                    "rebuilt on demand by the agent",
                    escalate(*danger, size),
                    Action::Trash,
                ));
            }
        }
        for root in HISTORY {
            let base = ctx.home.join(root.relative);
            let mut candidates = Vec::new();
            collect_at_depth(&base, root.depth, root.include_files, &mut candidates)?;
            candidates.sort();
            for path in candidates {
                let Some((age, size)) = history_details(&path, &ctx.home, ctx.active_days)? else {
                    continue;
                };
                if size == 0 {
                    continue;
                }
                findings.push(Finding::new(
                    format!("{}: {}", root.label, relative_display(&path, &base)),
                    Some(path),
                    size,
                    format!(
                        "untouched for {age} days; agent history is not regenerable and cannot be re-downloaded"
                    ),
                    escalate(6, size),
                    Action::Trash,
                ));
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
                let target = finding
                    .target()
                    .ok_or_else(|| anyhow::anyhow!("agent finding missing internal target"))?;
                authorize(target, ctx)?;
                apply_filesystem_finding(self.name(), finding, ctx)
            })()
            .with_context(|| format!("failed to remove {}", finding.label));
            // One refused finding must not abandon the rest of the previewed
            // plan. The age gate is re-read at apply, so a session resumed
            // between preview and apply is an ordinary, expected refusal — and
            // the documented promise is that such a session falls out of the
            // plan, not that it takes every later finding with it. Each failure
            // is still recorded, so the run reports nonzero.
            if let Err(error) = result {
                outcome.fail(error);
                continue;
            }
            outcome.record(finding, removal_note(finding, &finding.label));
        }
        Ok(outcome)
    }
}

/// The scanner is never deletion authority: apply reasserts the full shape of
/// whichever tier the target claims, including the age gate, before the sink
/// sees it.
///
/// The two refusals are reported separately. A target outside both lists is a
/// forged or stale plan; a target inside a history root that no longer passes
/// the gate is the ordinary case of a session resumed between preview and
/// apply, and saying "outside its authorized namespace" would misdescribe it.
fn authorize(target: &Path, ctx: &Ctx) -> Result<()> {
    if is_regenerable_target(target, &ctx.home) {
        return Ok(());
    }
    if owning_history_root(target, &ctx.home).is_none() {
        anyhow::bail!(
            "agent target is outside its authorized namespace: {}",
            target.display()
        );
    }
    if history_details(target, &ctx.home, ctx.active_days)?.is_some() {
        return Ok(());
    }
    anyhow::bail!(
        "agent history no longer meets its preview shape — it became active, lost its session shape, or is now a symlink; refusing {}",
        target.display()
    )
}

fn is_regenerable_target(path: &Path, home: &Path) -> bool {
    REGENERABLE
        .iter()
        .any(|(_, relative, _)| path == home.join(relative))
}

/// Age in days and logical size for an eligible history child, or `None` when
/// the path is not a direct child of a configured root at its configured depth,
/// is a symlink, has the wrong file type, or is still inside the active window.
fn history_details(path: &Path, home: &Path, active_days: u32) -> Result<Option<(u64, u64)>> {
    let Some(root) = owning_history_root(path, home) else {
        return Ok(None);
    };
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", path.display()));
        }
    };
    // A symlink is refused outright: following one would delete a tree outside
    // the authorized root while every path check above still passed.
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Ok(None);
    }
    if !file_type.is_dir() && !(root.include_files && file_type.is_file()) {
        return Ok(None);
    }
    let (size, newest) = crate::safety::dir_stats(path)?;
    let Ok(elapsed) = SystemTime::now().duration_since(newest) else {
        return Ok(None);
    };
    let age = elapsed.as_secs() / DAY;
    Ok((age >= u64::from(active_days)).then_some((age, size)))
}

/// The configured root this path is a child of, at exactly the configured depth.
///
/// Matching is structural rather than by prefix: the path must be `root` plus
/// exactly `depth` normal components, so neither a shallower ancestor (the root
/// itself) nor a deeper descendant can borrow the root's authority.
fn owning_history_root(path: &Path, home: &Path) -> Option<&'static HistoryRoot> {
    HISTORY.iter().find(|root| {
        let base = home.join(root.relative);
        let Ok(relative) = path.strip_prefix(&base) else {
            return false;
        };
        relative.components().count() == root.depth
            && relative
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
    })
}

fn relative_display(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Direct children of `base` exactly `depth` levels down.
///
/// Traversal refuses to descend through a symlink, so a link planted inside an
/// authorized root cannot widen the candidate set to a foreign tree.
fn collect_at_depth(
    base: &Path,
    depth: usize,
    include_files: bool,
    found: &mut Vec<PathBuf>,
) -> Result<()> {
    let entries = match std::fs::read_dir(base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", base.display()));
        }
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot enumerate {}", base.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("cannot inspect {}", path.display()))?;
        if file_type.is_symlink() {
            continue;
        }
        if depth == 1 {
            if file_type.is_dir() || (include_files && file_type.is_file()) {
                found.push(path);
            }
        } else if file_type.is_dir() {
            collect_at_depth(&path, depth - 1, include_files, found)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::project::ScanObservations;
    use std::time::Duration;

    fn test_ctx(home: PathBuf) -> Ctx {
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

    fn write_aged(path: &Path, contents: &str, days: u64) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
        let stale = SystemTime::now() - Duration::from_secs(DAY * days);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(stale)
            .unwrap();
    }

    #[test]
    fn regenerable_caches_are_offered_and_credentials_are_not() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-agents-cache")
            .tempdir()
            .unwrap();
        let home = home.path();
        write_aged(&home.join(".claude/cache/changelog.md"), "# 1.0.0\n", 400);
        // Everything an agent needs to keep must be invisible to the scanner.
        for secret in [
            ".claude/.credentials.json",
            ".claude/settings.json",
            ".codex/auth.json",
            ".codex/config.toml",
            ".claude/backups/.claude.json.backup.1",
        ] {
            write_aged(&home.join(secret), "secret", 400);
        }

        let findings = Agents
            .scan(&test_ctx(home.to_path_buf()), &ScanObservations::default())
            .unwrap();

        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].target(),
            Some(home.join(".claude/cache").as_path())
        );
        assert_eq!(findings[0].action, Action::Trash);
        for secret in [
            ".claude/.credentials.json",
            ".claude/settings.json",
            ".codex/auth.json",
            ".codex/config.toml",
            ".claude/backups/.claude.json.backup.1",
        ] {
            assert!(home.join(secret).exists());
        }
    }

    #[test]
    fn history_is_offered_only_after_the_active_window() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-agents-history")
            .tempdir()
            .unwrap();
        let home = home.path();
        // Codex nests three levels; the day directory is the unit.
        write_aged(
            &home.join(".codex/sessions/2020/01/02/rollout.jsonl"),
            "old",
            400,
        );
        // `pins.json` is a file directly under a directories-only root.
        write_aged(&home.join(".claude/jobs/pins.json"), "{}", 400);
        // Positive control for the same switch: a loose file at depth under a
        // root that does include files must be admitted, so the exclusion above
        // is proven to come from `include_files` and not from file-ness itself.
        write_aged(&home.join(".codex/archived_sessions/old.jsonl"), "old", 400);
        write_aged(&home.join(".codex/archived_sessions/new.jsonl"), "new", 0);

        let findings = Agents
            .scan(&test_ctx(home.to_path_buf()), &ScanObservations::default())
            .unwrap();

        let targets: Vec<_> = findings.iter().filter_map(Finding::target).collect();
        assert!(targets.contains(&home.join(".codex/sessions/2020/01/02").as_path()));
        assert!(!targets.contains(&home.join(".claude/jobs/pins.json").as_path()));
        assert!(targets.contains(&home.join(".codex/archived_sessions/old.jsonl").as_path()));
        assert!(!targets.contains(&home.join(".codex/archived_sessions/new.jsonl").as_path()));
        // The root itself is never a finding, only its children at the configured depth.
        assert!(!targets.contains(&home.join(".codex/sessions").as_path()));
        assert!(!targets.contains(&home.join(".codex/sessions/2020").as_path()));
        assert!(
            findings
                .iter()
                .any(|finding| finding.note.contains("not regenerable"))
        );
    }

    /// `~/.claude/projects` is not a cleanup root at all, and this is the
    /// assertion that keeps it that way. It holds Claude Code auto memory in
    /// `<project>/memory/`, and since the transcripts beside it can originate in
    /// Claude Desktop — which the vendor retains at any age — file age is not
    /// evidence that anything there is finished with. A `.codex` transcript of
    /// identical shape and age is offered in the same run, so this proves an
    /// exclusion rather than an inert fixture.
    #[test]
    fn the_claude_projects_tree_is_never_a_candidate() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-agents-projects")
            .tempdir()
            .unwrap();
        let home = home.path();
        let session = "24bb0c93-fbb9-49b7-99b0-7a97be87baeb";
        let project = home.join(".claude/projects/-repo");
        write_aged(&project.join(format!("{session}.jsonl")), "old", 400);
        write_aged(&project.join("memory/MEMORY.md"), "durable fact", 400);
        let codex = home.join(".codex/archived_sessions");
        write_aged(&codex.join(format!("rollout-{session}.jsonl")), "old", 400);

        let ctx = test_ctx(home.to_path_buf());
        let findings = Agents.scan(&ctx, &ScanObservations::default()).unwrap();
        let targets: Vec<_> = findings.iter().filter_map(Finding::target).collect();

        assert!(
            targets.contains(&codex.join(format!("rollout-{session}.jsonl")).as_path()),
            "a transcript of the same shape and age elsewhere must still be offered"
        );
        for excluded in [
            home.join(".claude/projects"),
            project.clone(),
            project.join("memory"),
        ] {
            assert!(
                !targets.contains(&excluded.as_path()),
                "{}",
                excluded.display()
            );
            let outcome = Agents
                .apply(
                    &[Finding::new(
                        "forged",
                        Some(excluded.clone()),
                        4,
                        "test",
                        6,
                        Action::Trash,
                    )],
                    &ctx,
                )
                .unwrap();
            assert_eq!(outcome.summary.items_touched, 0, "{}", excluded.display());
            assert_eq!(outcome.errors.len(), 1, "{}", excluded.display());
        }
        assert_eq!(
            std::fs::read_to_string(project.join("memory/MEMORY.md")).unwrap(),
            "durable fact"
        );
    }

    /// A shell snapshot is sourced by every later shell call in the session that
    /// wrote it, and nothing rewrites it, so it belongs to the age-gated tier.
    #[test]
    fn shell_snapshots_are_age_gated_rather_than_offered_outright() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-agents-snapshots")
            .tempdir()
            .unwrap();
        let home = home.path();
        let snapshots = home.join(".claude/shell-snapshots");
        write_aged(
            &snapshots.join("snapshot-zsh-1-old.sh"),
            "export A=1\n",
            400,
        );
        write_aged(&snapshots.join("snapshot-zsh-2-live.sh"), "export B=2\n", 0);

        let findings = Agents
            .scan(&test_ctx(home.to_path_buf()), &ScanObservations::default())
            .unwrap();
        let targets: Vec<_> = findings.iter().filter_map(Finding::target).collect();

        assert!(targets.contains(&snapshots.join("snapshot-zsh-1-old.sh").as_path()));
        assert!(!targets.contains(&snapshots.join("snapshot-zsh-2-live.sh").as_path()));
        assert!(
            !targets.contains(&snapshots.as_path()),
            "the directory itself must never be offered wholesale"
        );
    }

    /// A session resumed between preview and apply is an expected refusal, and
    /// the documented promise is that it falls out of the plan — not that it
    /// takes every later finding with it.
    #[test]
    fn a_resumed_session_does_not_block_the_rest_of_the_plan() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-agents-partial-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(&home).unwrap();
        let home = home.canonicalize().unwrap();
        let session = "24bb0c93-fbb9-49b7-99b0-7a97be87baeb";
        let archived = home.join(".codex/archived_sessions");
        let resumed = archived.join(format!("rollout-resumed-{session}.jsonl"));
        let stale = archived.join(format!("rollout-stale-{session}.jsonl"));
        write_aged(&resumed, "old", 400);
        write_aged(&stale, "old", 400);
        let ctx = test_ctx(home.clone());
        let findings = vec![
            Finding::new(
                "resumed",
                Some(resumed.clone()),
                4,
                "test",
                6,
                Action::Shred,
            ),
            Finding::new("stale", Some(stale.clone()), 4, "test", 6, Action::Shred),
        ];
        // The resumed session is written after the plan was built, exactly as a
        // live agent would; its inode is unchanged, so only the age re-read sees it.
        std::fs::write(&resumed, "resumed").unwrap();

        let outcome = Agents.apply(&findings, &ctx).unwrap();

        assert_eq!(
            outcome.errors.len(),
            1,
            "the refusal must still be reported"
        );
        assert!(
            outcome.errors[0].contains("no longer meets its preview shape"),
            "an age refusal must not read as a forged target: {}",
            outcome.errors[0]
        );
        assert_eq!(
            outcome.summary.items_touched, 1,
            "the finding listed after the resumed one must still be removed"
        );
        assert!(resumed.exists(), "the resumed session must survive");
        assert!(!stale.exists());
        crate::ops::remove_test_path(home);
    }

    /// Positive control for the apply-time boundary. The forged targets are
    /// exactly the shapes a compromised or buggy preview could produce: a path
    /// outside every root, the root itself, and a descendant below the
    /// configured depth.
    #[test]
    fn apply_refuses_forged_targets_and_preserves_them() {
        // The deletion sink refuses anything under `/private/var`, so the
        // positive control needs a home the global protection list allows.
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-agents-forged-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(&home).unwrap();
        let home = home.canonicalize().unwrap();
        write_aged(&home.join(".ssh/id_ed25519"), "PRIVATE KEY", 400);
        write_aged(
            &home.join(
                ".codex/archived_sessions/rollout-24bb0c93-fbb9-49b7-99b0-7a97be87baeb.jsonl",
            ),
            "old",
            400,
        );
        write_aged(
            &home.join(".codex/archived_sessions/nested/rollout-24bb0c93-fbb9-49b7-99b0-7a97be87baeb.jsonl"),
            "old",
            400,
        );
        let ctx = test_ctx(home.clone());

        for forged in [
            home.join(".ssh"),
            home.join(".codex"),
            home.join(".codex/archived_sessions"),
            home.join(".codex/archived_sessions/nested/rollout-24bb0c93-fbb9-49b7-99b0-7a97be87baeb.jsonl"),
        ] {
            let finding = Finding::new("forged", Some(forged.clone()), 4, "test", 9, Action::Shred);
            let outcome = Agents.apply(&[finding], &ctx).unwrap();
            assert_eq!(outcome.summary.items_touched, 0, "{}", forged.display());
            assert_eq!(outcome.errors.len(), 1, "{}", forged.display());
            assert!(forged.exists(), "{}", forged.display());
        }
        assert_eq!(
            std::fs::read_to_string(home.join(".ssh/id_ed25519")).unwrap(),
            "PRIVATE KEY"
        );

        // Positive control: the authorized shape at the same depth is accepted,
        // proving the refusals above are the boundary and not a vacuous pass.
        let authorized = home
            .join(".codex/archived_sessions/rollout-24bb0c93-fbb9-49b7-99b0-7a97be87baeb.jsonl");
        let finding = Finding::new(
            "authorized",
            Some(authorized.clone()),
            4,
            "test",
            6,
            Action::Shred,
        );
        let outcome = Agents.apply(&[finding], &ctx).unwrap();
        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
        assert_eq!(outcome.summary.items_touched, 1);
        assert!(!authorized.exists());
        crate::ops::remove_test_path(home);
    }

    /// Apply reasserts age, not just shape. A transcript appended to after
    /// preview keeps its inode and generation, so the sink's identity check
    /// cannot see the change; only re-reading the timestamp can.
    #[test]
    fn history_that_stopped_being_stale_after_preview_is_refused() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-agents-touched")
            .tempdir()
            .unwrap();
        let home = home.path();
        let session = home.join(".codex/sessions/2020/01/02");
        write_aged(&session.join("rollout.jsonl"), "old", 400);
        assert!(history_details(&session, home, 30).unwrap().is_some());

        std::fs::write(session.join("rollout.jsonl"), "resumed").unwrap();

        assert!(
            history_details(&session, home, 30).unwrap().is_none(),
            "a resumed session must fall out of the plan"
        );
    }

    #[test]
    fn a_symlinked_history_child_is_refused() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-agents-symlink")
            .tempdir()
            .unwrap();
        let home = home.path();
        let outside = home.join("outside");
        write_aged(&outside.join("payload"), "keep", 400);
        std::fs::create_dir_all(home.join(".claude/projects")).unwrap();
        let link = home.join(".claude/projects/-linked");
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        assert!(history_details(&link, home, 30).unwrap().is_none());
        let findings = Agents
            .scan(&test_ctx(home.to_path_buf()), &ScanObservations::default())
            .unwrap();
        assert!(findings.is_empty());
        assert!(outside.join("payload").exists());
    }
}
