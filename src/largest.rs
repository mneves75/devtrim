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
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(_) => {
                    skipped = skipped.saturating_add(1);
                    continue;
                }
            };
            add_to_shallow_ancestors(root, entry.path(), metadata.len(), &mut totals);
        }
    }

    let mut ranked = totals.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|(left_path, left_size), (right_path, right_size)| {
        right_size
            .cmp(left_size)
            .then_with(|| left_path.cmp(right_path))
    });
    let limit = top.unwrap_or(20).clamp(1, 100);
    let findings = ranked
        .into_iter()
        .take(limit)
        .map(|(path, size)| {
            Finding::new(
                "large directory",
                Some(path),
                size,
                "report-only; sizes are estimated logical bytes (lower bound when entries were skipped)",
                1,
                Action::Info,
            )
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

#[cfg(test)]
mod tests {
    #[test]
    fn overlapping_roots_count_each_file_once() {
        let base =
            std::env::temp_dir().join(format!("devtrim-largest-overlap-{}", std::process::id()));
        crate::ops::remove_test_path(&base);
        let nested = base.join("work/project/cache");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("blob"), vec![0u8; 1024]).unwrap();

        let once = super::scan_roots(&[base.join("work")], Some(50));
        let twice = super::scan_roots(
            &[
                base.join("work"),
                base.join("work/project"),
                base.join("work"),
            ],
            Some(50),
        );
        let total = |result: &super::LargestResult| {
            result
                .findings
                .iter()
                .map(|finding| finding.size_bytes)
                .max()
                .unwrap_or(0)
        };
        assert_eq!(total(&once), 1024);
        assert_eq!(total(&twice), 1024);
        crate::ops::remove_test_path(base);
    }

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("devtrim-largest-{name}-{}", std::process::id()));
        crate::ops::remove_test_path(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn ranks_depth_one_and_two_totals_and_clamps_top() {
        let root = temp("ranking");
        std::fs::create_dir_all(root.join("alpha/deep/third")).unwrap();
        std::fs::create_dir_all(root.join("beta")).unwrap();
        std::fs::write(root.join("alpha/top"), vec![0; 5]).unwrap();
        std::fs::write(root.join("alpha/deep/nested"), vec![0; 10]).unwrap();
        std::fs::write(root.join("alpha/deep/third/lower"), vec![0; 2]).unwrap();
        std::fs::write(root.join("beta/file"), vec![0; 7]).unwrap();

        let result = scan_roots(std::slice::from_ref(&root), None);
        let ranked = result
            .findings
            .iter()
            .map(|finding| (finding.target().unwrap().to_path_buf(), finding.size_bytes))
            .collect::<Vec<_>>();
        assert_eq!(
            ranked,
            vec![
                (root.join("alpha"), 17),
                (root.join("alpha/deep"), 12),
                (root.join("beta"), 7),
            ]
        );
        assert!(result.errors.is_empty());

        let one = scan_roots(std::slice::from_ref(&root), Some(0));
        assert_eq!(one.findings.len(), 1);

        for index in 0..105 {
            let directory = root.join(format!("extra-{index:03}"));
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("file"), [0]).unwrap();
        }
        let hundred = scan_roots(std::slice::from_ref(&root), Some(usize::MAX));
        assert_eq!(hundred.findings.len(), 100);
        crate::ops::remove_test_path(root);
    }
}
