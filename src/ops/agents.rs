//! Coding-agent caches and stale session history. Filesystem targets remain Trash-first.
//!
//! Two tiers with different promises, because they are not the same kind of data:
//!
//! * A *regenerable* entry is a cache the agent rebuilds on demand, so it is
//!   offered unconditionally at a low danger score. Trash-first is what makes
//!   that safe: no vendor documents these as removable *mid-session*, and this
//!   tier claims only that the content comes back, not that a running agent
//!   will not notice.
//! * A *history* entry is a session transcript or a shell snapshot. Nothing
//!   regenerates it. It is therefore offered only after the
//!   configured active window has passed over the whole subtree, and its note
//!   says plainly that the content does not come back.
//!
//! Authentication material (`auth.json`, `.credentials.json`), configuration,
//! memories, skills, agent definitions, installed plugins, and the `.claude.json`
//! backup copies are never in either list and are never traversed as candidates.
//!
//! Two whole trees under `~/.claude` are excluded, and they are the exclusions
//! to know about. `projects` holds auto memory keyed by repository root, beside
//! transcripts the vendor retains at any age when the session came from Claude
//! Desktop — age is not evidence there, and telling the retained ones apart
//! would mean reading transcript contents. `jobs` is the background-session
//! supervisor's live state, with a `pins.json` beside it naming the sessions
//! kept alive while idle. Both cases share one rule: a closed category that has
//! to consult a liveness signal to stay safe has gone one directory too far.
//!
//! Other stores are absent only because they cost more preview than they
//! return, a preview nobody can read being no preview at all. Against the
//! machine this was built on, Codex lane transcripts produced 558 findings for
//! 0.25 GB, its `.tmp` tree 324 for 0.07 GB, the Claude Code file-edit history
//! 123 for 0.10 GB, and the paste cache a comparable count for about 2 MB.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, dir_size, removal_note};
use crate::safety::{Ctx, escalate};

pub struct Agents;

/// Caches an agent rebuilds on demand. Exact paths relative to `$HOME`; nothing
/// is matched by prefix, so a sibling directory can never inherit authority.
///
/// Each entry names the evidence it rests on, because this list is the only
/// thing guarding these paths: there is no age gate behind it and no
/// corroboration signal, only Trash-first.
///
/// Documented by their vendors: `.claude/cache` holds the changelog and model
/// catalogue Claude Code refreshes in the background; Pi's web-search cache
/// self-evicts on a one-hour lifetime with fixed entry and size limits; and
/// OpenCode's own troubleshooting guide prescribes removing its cache as a reset
/// step, with persistent data kept under a different tree.
///
/// Rests on direct inspection only: `.codex/cache`, which no OpenAI
/// documentation describes. Every child observed there was a hash-named JSON
/// catalogue re-fetched from the network (`codex_app_directory`,
/// `codex_apps_server_info`, `codex_apps_tools`, `remote_plugin_catalog`).
/// Codex keeps its model catalogue in `models_cache.json` and plugin bundles in
/// `plugins/cache/`, neither of which is this directory.
///
/// `.claude/downloads` was dropped for having neither: undocumented and empty
/// wherever it could be examined, so nothing could say what it holds.
const REGENERABLE: &[(&str, &str)] = &[
    ("Claude Code metadata cache", ".claude/cache"),
    ("Codex catalog cache", ".codex/cache"),
    ("Pi web-search cache", ".pi/web-search-cache"),
    ("OpenCode cache", ".cache/opencode"),
];

/// Every regenerable entry shares one danger score: the cost of removing any of
/// them is a re-fetch. A per-entry column would be a knob with one value.
const REGENERABLE_DANGER: u8 = 2;

/// A store whose children are per-session history rather than cache.
struct HistoryRoot {
    label: &'static str,
    /// Exact path relative to `$HOME`.
    relative: &'static str,
    /// Levels below `relative` at which one child becomes one finding. Codex
    /// nests sessions as `<year>/<month>/<day>`, so waiting for a whole year to
    /// go stale would never offer the current one.
    depth: usize,
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
    },
    HistoryRoot {
        label: "Codex shell snapshots",
        relative: ".codex/shell_snapshots",
        depth: 1,
    },
    HistoryRoot {
        label: "Codex session transcripts",
        relative: ".codex/sessions",
        depth: 3,
    },
    HistoryRoot {
        label: "Codex archived sessions",
        relative: ".codex/archived_sessions",
        depth: 1,
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
        for (label, relative) in REGENERABLE {
            let path = ctx.home.join(relative);
            let size = dir_size(&path)?;
            if size > 0 {
                findings.push(Finding::new(
                    *label,
                    Some(path),
                    size,
                    "rebuilt on demand by the agent",
                    escalate(REGENERABLE_DANGER, size),
                    Action::Trash,
                ));
            }
        }
        for root in HISTORY {
            let base = ctx.home.join(root.relative);
            let mut candidates = Vec::new();
            collect_at_depth(&base, root.depth, &mut candidates)?;
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
    if !is_history_child(target, &ctx.home) {
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
        .any(|(_, relative)| path == home.join(relative))
}

/// Age in days and logical size for an eligible history child, or `None` when
/// the path is not a direct child of a configured root at its configured depth,
/// is a symlink, has the wrong file type, or is still inside the active window.
fn history_details(path: &Path, home: &Path, active_days: u32) -> Result<Option<(u64, u64)>> {
    if !is_history_child(path, home) {
        return Ok(None);
    }
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
    // Every root holds either one directory or one loose file per session, so
    // both are candidates; anything else — a socket, a device node — is not.
    if !file_type.is_dir() && !file_type.is_file() {
        return Ok(None);
    }
    let (size, newest) = crate::safety::dir_stats(path)?;
    // No timestamp means a file in this subtree would not say when it changed,
    // so its staleness is unknown and the gate fails closed.
    let Some(newest) = newest else {
        return Ok(None);
    };
    let Ok(elapsed) = SystemTime::now().duration_since(newest) else {
        return Ok(None);
    };
    let age = elapsed.as_secs() / DAY;
    Ok((age >= u64::from(active_days)).then_some((age, size)))
}

/// Whether this path is a child of a configured root at exactly that root's depth.
///
/// Matching is structural rather than by prefix: the path must be `root` plus
/// exactly `depth` normal components, so neither a shallower ancestor (the root
/// itself) nor a deeper descendant can borrow the root's authority.
fn is_history_child(path: &Path, home: &Path) -> bool {
    HISTORY.iter().any(|root| {
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
fn collect_at_depth(base: &Path, depth: usize, found: &mut Vec<PathBuf>) -> Result<()> {
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
            if file_type.is_dir() || file_type.is_file() {
                found.push(path);
            }
        } else if file_type.is_dir() {
            collect_at_depth(&path, depth - 1, found)?;
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
        // A loose file at the configured depth is a candidate; the fresh one
        // beside it is the control proving the age gate is what excludes it.
        write_aged(&home.join(".codex/archived_sessions/old.jsonl"), "old", 400);
        write_aged(&home.join(".codex/archived_sessions/new.jsonl"), "new", 0);

        let findings = Agents
            .scan(&test_ctx(home.to_path_buf()), &ScanObservations::default())
            .unwrap();

        let targets: Vec<_> = findings.iter().filter_map(Finding::target).collect();
        assert!(targets.contains(&home.join(".codex/sessions/2020/01/02").as_path()));
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

    /// SECURITY.md states as a non-negotiable boundary that `~/.claude/projects`
    /// and `~/.claude/jobs` are not cleanup roots, so nothing beneath either can
    /// become a target. Both were roots at some point during development and
    /// both were retired after review found live or unjudgeable data inside, so
    /// the boundary needs to be executable rather than prose: re-adding either
    /// path to a list must fail here. The structural half catches it at the
    /// list, the behavioural half at the scan, and the `.codex` fixture is the
    /// control proving the scan was capable of returning something.
    #[test]
    fn the_retired_claude_trees_can_never_become_roots_again() {
        for retired in [".claude/projects", ".claude/jobs"] {
            assert!(
                !REGENERABLE
                    .iter()
                    .any(|(_, relative)| relative.starts_with(retired)),
                "{retired} must never be a regenerable entry"
            );
            assert!(
                !HISTORY
                    .iter()
                    .any(|root| root.relative.starts_with(retired)),
                "{retired} must never be a history root"
            );
        }

        let home = tempfile::Builder::new()
            .prefix("devtrim-agents-retired")
            .tempdir()
            .unwrap();
        let home = home.path();
        let session = "24bb0c93-fbb9-49b7-99b0-7a97be87baeb";
        write_aged(
            &home.join(format!(".claude/projects/-repo/{session}.jsonl")),
            "old",
            400,
        );
        write_aged(
            &home.join(".claude/projects/-repo/memory/MEMORY.md"),
            "durable fact",
            400,
        );
        write_aged(&home.join(".claude/jobs/pins.json"), "[]", 400);
        write_aged(&home.join(".claude/jobs/abc123/state.json"), "{}", 400);
        let control = home.join(format!(".codex/archived_sessions/rollout-{session}.jsonl"));
        write_aged(&control, "old", 400);

        let findings = Agents
            .scan(&test_ctx(home.to_path_buf()), &ScanObservations::default())
            .unwrap();
        let targets: Vec<_> = findings.iter().filter_map(Finding::target).collect();

        assert!(
            targets.contains(&control.as_path()),
            "control: a stale transcript under a live root must still be offered"
        );
        // Both directions matter. A target *beneath* a retired tree deletes part
        // of it; a target that is an *ancestor* of one — a `.claude` entry, say —
        // deletes the whole thing while never starting with the retired path.
        for retired in [home.join(".claude/projects"), home.join(".claude/jobs")] {
            for target in &targets {
                assert!(
                    !target.starts_with(&retired) && !retired.starts_with(target),
                    "{} must not be reachable through {}",
                    retired.display(),
                    target.display()
                );
            }
        }

        // Scanning cannot delete, so survival has to be proven against apply —
        // and the refusal has to name the retired tree. A memory file is a
        // regular file with an ordinary Trash action, so neither the file-type
        // gate nor the action check can stand in for the boundary; only the
        // namespace check can refuse it.
        let memory = home.join(".claude/projects/-repo/memory/MEMORY.md");
        let outcome = Agents
            .apply(
                &[Finding::new(
                    "forged",
                    Some(memory.clone()),
                    4,
                    "test",
                    6,
                    Action::Trash,
                )],
                &test_ctx(home.to_path_buf()),
            )
            .unwrap();
        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
        // Keyed on the reason phrase, not on `.claude/projects`: that string is
        // a substring of the forged target path, so any refusal echoing the path
        // would satisfy it and the check would quietly decay to "declined
        // somehow" — the vacuity this assertion exists to prevent.
        assert!(
            outcome.errors[0].contains("outside its authorized namespace"),
            "the refusal must be the namespace boundary, not some other decline: {}",
            outcome.errors[0]
        );
        assert_eq!(std::fs::read_to_string(&memory).unwrap(), "durable fact");
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

    /// A symlink planted inside a live root must be refused because it is a
    /// symlink, not because it happens to sit somewhere unscanned. The link is
    /// therefore placed under `.codex/archived_sessions` — a root that is
    /// actually traversed — and an ordinary stale file beside it is the positive
    /// control: if the scan returned nothing at all, the refusal would prove
    /// nothing about symlinks.
    #[test]
    fn a_symlinked_history_child_is_refused() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-agents-symlink")
            .tempdir()
            .unwrap();
        let home = home.path();
        // The link must resolve to a *regular file*. Pointed at a directory it
        // would also trip the wrong-file-type check, so removing the symlink
        // branch would leave this test green while a symlink to a stale file
        // outside the root was followed and offered.
        let outside = home.join("outside");
        let payload = outside.join("payload");
        write_aged(&payload, "keep", 400);
        let root = home.join(".codex/archived_sessions");
        let genuine = root.join("rollout-24bb0c93-fbb9-49b7-99b0-7a97be87baeb.jsonl");
        write_aged(&genuine, "old", 400);
        let link = root.join("rollout-linked.jsonl");
        std::os::unix::fs::symlink(&payload, &link).unwrap();
        // Age the link itself, not just its target: a fresh link inode would let
        // the age gate refuse it, so deleting the symlink check could leave this
        // test green while a stale symlink out of the root was followed.
        let stale = rustix::fs::Timespec {
            tv_sec: i64::try_from(
                (SystemTime::now() - Duration::from_secs(DAY * 400))
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            )
            .unwrap(),
            tv_nsec: 0,
        };
        rustix::fs::utimensat(
            rustix::fs::CWD,
            &link,
            &rustix::fs::Timestamps {
                last_access: stale,
                last_modification: stale,
            },
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .unwrap();

        assert!(is_history_child(&link, home), "the fixture must be scanned");
        assert!(history_details(&link, home, 30).unwrap().is_none());

        let ctx = test_ctx(home.to_path_buf());
        let findings = Agents.scan(&ctx, &ScanObservations::default()).unwrap();
        let targets: Vec<_> = findings.iter().filter_map(Finding::target).collect();
        assert!(
            targets.contains(&genuine.as_path()),
            "positive control: the real stale transcript must be offered"
        );
        assert!(!targets.contains(&link.as_path()));

        // Apply is the boundary that matters: a forged finding naming the link
        // must be refused rather than followed out of the root.
        let outcome = Agents
            .apply(
                &[Finding::new(
                    "forged",
                    Some(link.clone()),
                    4,
                    "test",
                    6,
                    Action::Shred,
                )],
                &ctx,
            )
            .unwrap();
        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
        assert!(
            outcome.errors[0].contains("symlink"),
            "the refusal must name the symlink, not some incidental check: {}",
            outcome.errors[0]
        );
        assert_eq!(std::fs::read_to_string(&payload).unwrap(), "keep");
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    }
}
