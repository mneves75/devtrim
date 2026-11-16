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
    let roots = crate::ops::project::normalized_roots(roots);
    for &root in &roots {
        // One device, as every other report walk: a mounted volume below a
        // root is not part of the root's disk usage.
        for result in walkdir::WalkDir::new(root)
            .follow_links(false)
            .follow_root_links(false)
            .same_file_system(true)
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
            if let Some(folder) = entry.path().parent() {
                add_to_shallow_ancestors(root, folder, bytes, &mut totals);
            }
        }
    }
    // Blocks clones share go to the folders holding every clone of the family:
    // only deleting one of those frees them.
    for (folder, shared) in usage.take_whole_families() {
        if let Some(root) = roots.iter().find(|root| folder.starts_with(root)) {
            add_to_shallow_ancestors(root, &folder, shared, &mut totals);
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
        "report-only; sizes are estimated bytes deleting each folder frees, and lower bounds: unreadable entries were skipped"
    } else {
        "report-only; sizes are estimated bytes deleting each folder frees"
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

/// Adds `bytes` to `folder` and its ancestors at depth one and two below `root`.
fn add_to_shallow_ancestors(
    root: &Path,
    folder: &Path,
    bytes: u64,
    totals: &mut BTreeMap<PathBuf, u64>,
) {
    let Ok(relative) = folder.strip_prefix(root) else {
        return;
    };
    let mut directory = root.to_path_buf();
    for component in relative.components().take(2) {
        directory.push(component.as_os_str());
        let total = totals.entry(directory.clone()).or_default();
        *total = total.saturating_add(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{CLONED_FILE, TestFixture, clone_file, write_incompressible};

    /// Deleting a folder frees the blocks clones share only when it holds
    /// every clone, so only that folder is credited with them.
    #[test]
    fn shared_clone_blocks_count_for_the_folder_holding_every_clone() {
        let fixture = TestFixture::new("devtrim-largest-clones");
        let root = fixture.path().join("root");
        for folder in ["whole/x", "whole/y", "split", "other"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        write_incompressible(&root.join("whole/x/source"));
        clone_file(&root.join("whole/x/source"), &root.join("whole/y/clone"));
        write_incompressible(&root.join("split/source"));
        clone_file(&root.join("split/source"), &root.join("other/clone"));

        let result = scan_roots(std::slice::from_ref(&root), Some(100));
        let size = |name: &str| {
            let path = root.join(name);
            result
                .findings
                .iter()
                .find(|finding| finding.path.as_deref() == path.to_str())
                .unwrap()
                .size_bytes
        };
        let whole = size("whole");
        assert!(
            (CLONED_FILE * 9 / 10..CLONED_FILE * 11 / 10).contains(&whole),
            "a folder holding a file and its clone measured {whole} bytes"
        );
        for name in ["whole/x", "whole/y", "split", "other"] {
            let part = size(name);
            assert!(
                part < CLONED_FILE / 10,
                "{name}, holding one clone of two, measured {part} bytes"
            );
        }
    }
}
