//! Read-only visibility into the largest shallow directory trees.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::report::{Action, Finding};
use crate::safety::Ctx;

pub(crate) struct LargestResult {
    pub(crate) findings: Vec<Finding>,
    pub(crate) errors: Vec<String>,
}

pub(crate) fn scan(ctx: &Ctx, top: Option<usize>) -> LargestResult {
    scan_roots(&ctx.roots, top)
}

fn scan_roots(roots: &[PathBuf], top: Option<usize>) -> LargestResult {
    let mut totals = BTreeMap::<PathBuf, u64>::new();
    let mut skipped = 0usize;
    let mut usage = crate::safety::DiskUsage::default();

    // Overlapping roots (e.g. ~/work and ~/work/project) would count the same
    // files twice; keep only roots not contained in another configured root.
    for root in crate::ops::project::normalized_roots(roots) {
        for result in walkdir::WalkDir::new(root)
            .follow_links(false)
            .follow_root_links(false)
        {
            let entry = match result {
                Ok(entry) => entry,
                Err(_) => {
                    // Report-only traversal never grants deletion authority, so
                    // partial visibility is useful when its lower bound is disclosed.
                    skipped = skipped.saturating_add(1);
                    continue;
                }
            };
            if entry.file_type().is_dir() && (1..=2).contains(&entry.depth()) {
                totals.entry(entry.path().to_path_buf()).or_default();
                continue;
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let bytes = match entry
                .metadata()
                .map_err(anyhow::Error::from)
                .and_then(|metadata| usage.add(&metadata, entry.path()))
            {
                Ok(bytes) => bytes,
                Err(_) => {
                    skipped = skipped.saturating_add(1);
                    continue;
                }
            };
            add_to_shallow_ancestors(root, entry.path(), bytes, &mut totals);
        }
    }

    let mut ranked = totals.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|(left_path, left_size), (right_path, right_size)| {
        right_size
            .cmp(left_size)
            .then_with(|| left_path.cmp(right_path))
    });
    let limit = top.unwrap_or(20).clamp(1, 100);
    let note = if skipped > 0 {
        "report-only; sizes are estimated allocated bytes and lower bounds: unreadable entries were skipped"
    } else {
        "report-only; sizes are estimated allocated bytes"
    };
    let findings = ranked
        .into_iter()
        .take(limit)
        .map(|(path, size)| {
            Finding::new("large directory", Some(path), size, note, 1, Action::Info)
        })
        .collect();
    let errors = (skipped > 0)
        .then(|| format!("skipped {skipped} unreadable entries; totals are lower bounds"))
        .into_iter()
        .collect();

    LargestResult { findings, errors }
}

fn add_to_shallow_ancestors(
    root: &Path,
    file: &Path,
    bytes: u64,
    totals: &mut BTreeMap<PathBuf, u64>,
) {
    let Ok(relative) = file.strip_prefix(root) else {
        return;
    };
    let Some(parent) = relative.parent() else {
        return;
    };
    let mut directory = root.to_path_buf();
    for component in parent.components().take(2) {
        directory.push(component.as_os_str());
        let total = totals.entry(directory.clone()).or_default();
        *total = total.saturating_add(bytes);
    }
}
