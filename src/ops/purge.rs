//! Project build output in one plan: stale `node_modules` and corroborated
//! build artifacts across every scanned repository, grouped by project with
//! the largest project first.
//!
//! It adds no authority of its own. Every finding comes from the
//! `node-modules` or `artifacts` scanner, with that category's staleness and
//! build-liveness gates, and is applied by the same category, which reasserts
//! its exact target shape — so a finding routed to the wrong one is refused,
//! never deleted. Ambiguous names such as `build` or `dist` stay unmatched.

use anyhow::Result;
use std::collections::BTreeMap;
use std::path::Path;

use super::artifacts::Artifacts;
use super::node_modules::NodeModules;
use super::project::ScanObservations;
use super::{ApplyOutcome, Finding, Op, is_node_modules_name};
use crate::safety::Ctx;

pub struct Purge;

impl Op for Purge {
    fn name(&self) -> &'static str {
        "purge"
    }

    fn scan(&self, ctx: &Ctx, observations: &ScanObservations) -> Result<Vec<Finding>> {
        let mut findings = NodeModules.scan(ctx, observations)?;
        findings.extend(Artifacts.scan(ctx, observations)?);
        order_by_project(&mut findings);
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let (node_modules, artifacts): (Vec<Finding>, Vec<Finding>) =
            findings.iter().cloned().partition(|finding| {
                finding
                    .target()
                    .and_then(Path::file_name)
                    .is_some_and(is_node_modules_name)
            });
        let mut outcome = ApplyOutcome::new(self.name());
        // The two categories authorize their batches independently, so one
        // refusing must not keep the other's unrelated targets.
        for (category, plan) in [
            (&NodeModules as &dyn Op, node_modules),
            (&Artifacts, artifacts),
        ] {
            if plan.is_empty() {
                continue;
            }
            match category.apply(&plan, ctx) {
                Ok(part) => outcome.merge(part),
                Err(error) => outcome.fail(error.context(category.name())),
            }
        }
        Ok(outcome)
    }
}

/// Largest project first, then largest target within it; ties break on the
/// path so the order is the same on every run.
fn order_by_project(findings: &mut [Finding]) {
    let mut totals: BTreeMap<Option<String>, u64> = BTreeMap::new();
    for finding in findings.iter() {
        let total = totals.entry(finding.project.clone()).or_default();
        *total = total.saturating_add(finding.size_bytes);
    }
    let total = |finding: &Finding| totals.get(&finding.project).copied().unwrap_or(0);
    findings.sort_by(|left, right| {
        total(right)
            .cmp(&total(left))
            .then_with(|| left.project.cmp(&right.project))
            .then_with(|| right.size_bytes.cmp(&left.size_bytes))
            .then_with(|| left.path.cmp(&right.path))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Action;
    use std::path::PathBuf;

    fn finding(path: &str, size: u64, project: &str) -> Finding {
        Finding::new(
            "test",
            Some(PathBuf::from(path)),
            size,
            "test",
            5,
            Action::Trash,
        )
        .with_project(Path::new(project))
    }

    #[test]
    fn projects_are_ordered_by_their_total_then_targets_by_size() {
        let mut findings = vec![
            finding("/dev/a/node_modules", 10, "/dev/a"),
            finding("/dev/b/node_modules", 6, "/dev/b"),
            finding("/dev/b/target", 7, "/dev/b"),
            finding("/dev/c/target", 13, "/dev/c"),
        ];

        order_by_project(&mut findings);

        let order: Vec<_> = findings
            .iter()
            .map(|finding| finding.path.as_deref().unwrap())
            .collect();
        assert_eq!(
            order,
            [
                "/dev/b/target",
                "/dev/b/node_modules",
                "/dev/c/target",
                "/dev/a/node_modules"
            ]
        );
    }

    /// Routing trusts nothing but the leaf name, and each category reasserts its
    /// own shape at apply; a finding sent to the wrong one must come back as a
    /// refusal with the target intact.
    #[test]
    fn a_misrouted_finding_is_refused_by_the_category_that_receives_it() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-purge-route-{}", std::process::id()));
        crate::ops::remove_test_path(&root);
        let decoy = root.join("dev/project/src");
        std::fs::create_dir_all(root.join("dev/project/.git")).unwrap();
        std::fs::create_dir_all(&decoy).unwrap();
        std::fs::write(decoy.join("main.rs"), "fn main() {}").unwrap();
        let root = root.canonicalize().unwrap();
        let decoy = root.join("dev/project/src");
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: vec![root.join("dev")],
            active_days: 30,
            protect: Vec::new(),
            journal_path: root.join("journal.jsonl"),
            home: root.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };
        let mut plan = vec![finding(
            decoy.to_str().unwrap(),
            12,
            root.join("dev/project").to_str().unwrap(),
        )];
        crate::report::effective_actions(&mut plan, true);

        let outcome = Purge.apply(&plan, &ctx).unwrap();

        assert_eq!(outcome.summary.items_touched, 0);
        assert!(!outcome.errors.is_empty());
        assert!(decoy.join("main.rs").exists());
        crate::ops::remove_test_path(root);
    }
}
