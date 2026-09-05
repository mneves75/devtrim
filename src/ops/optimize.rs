//! macOS maintenance tasks, as typed commands.
//!
//! This category is deliberately narrow. Rebuilding a Spotlight index, running
//! the periodic scripts, or purging memory all need root and cost far more than
//! they return, so they are refused by omission rather than offered with a
//! warning. What remains is the part that is genuinely disk hygiene: caches the
//! system rebuilds on demand. A DNS flush is absent for the same reason — see
//! `MaintenanceTask` for why `dscacheutil` does not do what it appears to.
//!
//! Every task is a fixed program with fixed arguments and no caller-supplied
//! data at all, so there is no dynamic argument to validate — the authority
//! carries the whole invocation.

use anyhow::Result;
use std::process::Command;

use super::{ApplyOutcome, Finding, Op};
use crate::report::{CommandAuthority, MaintenanceTask};
use crate::safety::Ctx;

pub struct Optimize {
    tasks: Vec<MaintenanceTask>,
}

impl Optimize {
    /// Selects tasks by name, or every task when none are named.
    ///
    /// Selection exists because one confirmation must not cover two unrelated
    /// kinds of risk: a resolver flush and a Launch Services rebuild differ by
    /// orders of magnitude in cost, and `plan_danger` takes the maximum, so
    /// without this the cheap task rides in on the expensive one's prompt.
    pub fn new(names: &[String], apply: bool) -> Result<Self> {
        if names.is_empty() {
            // Previewing everything is useful; applying everything behind one
            // prompt is the thing `--task` exists to prevent, so the shortcut
            // stops at the preview.
            if apply {
                anyhow::bail!(
                    "optimize --apply needs an explicit --task; one confirmation must not cover \
                     unrelated tasks. valid: {}",
                    MaintenanceTask::ALL
                        .iter()
                        .map(|task| task.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            return Ok(Self {
                tasks: MaintenanceTask::ALL.to_vec(),
            });
        }
        let mut tasks = Vec::new();
        for name in names {
            let task = MaintenanceTask::from_name(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown task `{name}`. valid: {}",
                    MaintenanceTask::ALL
                        .iter()
                        .map(|task| task.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            if !tasks.contains(&task) {
                tasks.push(task);
            }
        }
        Ok(Self { tasks })
    }
}

impl Op for Optimize {
    fn name(&self) -> &'static str {
        "optimize"
    }

    fn scan(
        &self,
        _ctx: &Ctx,
        _observations: &super::project::ScanObservations,
    ) -> Result<Vec<Finding>> {
        Ok(self
            .tasks
            .iter()
            .map(|task| {
                Finding::command(
                    task.label(),
                    // These reclaim an amount nothing can know in advance, so
                    // the plan reports zero rather than a guess. The note says
                    // which of them free disk at all.
                    0,
                    task.note(),
                    task.danger(),
                    CommandAuthority::Maintenance(*task),
                )
            })
            .collect())
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let mut outcome = ApplyOutcome::new(self.name());
        for finding in findings {
            if !finding.action.is_actionable() {
                continue;
            }
            let result = (|| -> Result<String> {
                let Some(authority) = finding.command_authority() else {
                    anyhow::bail!("refusing unexpected maintenance action");
                };
                // The authority must be a maintenance task AND its serialized
                // action must still match: a finding whose displayed command
                // was altered after preview is a forgery, not a plan.
                let Some(task) = authority.maintenance_task() else {
                    anyhow::bail!("refusing unexpected maintenance action");
                };
                if finding.action != authority.action() {
                    anyhow::bail!("refusing altered maintenance action");
                }
                let (program, args) = authority.parts();
                let attempt = crate::journal::begin(
                    ctx,
                    crate::journal::JournalRecord::command_attempt(
                        self.name(),
                        program,
                        &args,
                        finding.size_bytes,
                    ),
                )?;
                let result = (|| -> Result<String> {
                    let output = Command::new(program).args(&args).output()?;
                    if !output.status.success() {
                        anyhow::bail!("`{program} {}` failed", args.join(" "));
                    }
                    Ok(format!("reset {}", task.label()))
                })();
                attempt.finish(ctx, result)
            })();
            match result {
                Ok(note) => outcome.record(finding, note),
                Err(error) => {
                    outcome.fail(error);
                    break;
                }
            }
        }
        if outcome.summary.items_touched > 0 {
            outcome.summary.notes.push(
                "caches rebuild on demand; some apps show stale fonts or icons until restarted"
                    .into(),
            );
        }
        Ok(outcome)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Action;
    use std::path::PathBuf;

    fn test_ctx() -> Ctx {
        Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: PathBuf::from("/tmp/devtrim-optimize-test-journal.jsonl"),
            home: PathBuf::from("/tmp"),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Stderr,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        }
    }

    #[test]
    fn selecting_tasks_narrows_the_plan_and_rejects_unknown_names() {
        let all = Optimize::new(&[], false)
            .unwrap()
            .scan(
                &test_ctx(),
                &crate::ops::project::ScanObservations::default(),
            )
            .unwrap();
        assert_eq!(all.len(), MaintenanceTask::ALL.len());

        let one = Optimize::new(&["quicklook".to_string()], true)
            .unwrap()
            .scan(
                &test_ctx(),
                &crate::ops::project::ScanObservations::default(),
            )
            .unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].danger, MaintenanceTask::QuickLookCache.danger());

        // A duplicate selection must not run the task twice.
        let duplicated = Optimize::new(&["quicklook".to_string(), "quicklook".to_string()], true)
            .unwrap()
            .scan(
                &test_ctx(),
                &crate::ops::project::ScanObservations::default(),
            )
            .unwrap();
        assert_eq!(duplicated.len(), 1);

        assert!(Optimize::new(&["nonsense".to_string()], false).is_err());
    }

    /// Applying everything behind one prompt is exactly what selection exists
    /// to prevent, so the unselected shortcut must stop at the preview.
    #[test]
    fn applying_without_a_task_selection_is_refused() {
        assert!(Optimize::new(&[], true).is_err());
        assert!(Optimize::new(&[], false).is_ok());
    }

    #[test]
    fn every_task_is_previewed_as_a_typed_command() {
        let findings = Optimize::new(&[], false)
            .unwrap()
            .scan(
                &test_ctx(),
                &crate::ops::project::ScanObservations::default(),
            )
            .unwrap();
        assert_eq!(findings.len(), MaintenanceTask::ALL.len());
        for finding in &findings {
            assert!(
                matches!(finding.action, Action::Command { .. }),
                "{} must preview as a command",
                finding.label
            );
            assert!(finding.danger >= 1 && finding.danger <= 4);
            assert!(!finding.note.is_empty());
        }
    }

    /// No task may carry caller-supplied data into a process: the argument
    /// vector is fixed per task, which is what removes the dynamic-argument
    /// question entirely.
    #[test]
    fn task_invocations_are_fully_fixed() {
        for task in MaintenanceTask::ALL {
            let (_, args) = CommandAuthority::Maintenance(*task).parts();
            assert!(
                !args.iter().any(|argument| argument == task.name()),
                "task selection must not enter argv"
            );
        }
    }

    /// The expensive and root-requiring tasks are refused by omission. If one
    /// is ever added, this test is where the decision has to be revisited.
    #[test]
    fn root_requiring_tasks_stay_out_of_the_catalog() {
        let programs: Vec<&str> = MaintenanceTask::ALL
            .iter()
            .map(|task| CommandAuthority::Maintenance(*task).parts().0)
            .collect();
        for refused in ["mdutil", "periodic", "purge", "sudo", "dscacheutil"] {
            assert!(
                !programs.iter().any(|program| program.contains(refused)),
                "{refused} needs root or costs hours and must not be offered"
            );
        }
    }

    #[test]
    fn apply_refuses_a_finding_whose_action_was_altered() {
        let mut forged = Finding::command(
            "forged",
            0,
            "forged",
            2,
            CommandAuthority::Maintenance(MaintenanceTask::QuickLookCache),
        );
        // The displayed action no longer matches the authority that issued it.
        forged.action = Action::command("echo", &["altered"]);
        let outcome = Optimize::new(&[], false)
            .unwrap()
            .apply(&[forged], &test_ctx())
            .unwrap();
        assert!(
            !outcome.errors.is_empty(),
            "an altered action must be refused"
        );
    }
}
