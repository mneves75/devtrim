//! Read-only visibility into the largest shallow directory trees.

use std::collections::BTreeMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::report::{Action, Finding};
use crate::safety::Ctx;

pub(crate) struct LargestResult {
    pub(crate) findings: Vec<Finding>,
    pub(crate) errors: Vec<String>,
}

struct BulkUnsupported;

struct LinkedFile {
    path: PathBuf,
    sub_index: Option<usize>,
    dev: u64,
    ino: u64,
    allocated: u64,
}

struct TopDirMeasurement {
    path: PathBuf,
    bytes: u64,
    sub_totals: Vec<(PathBuf, u64)>,
    linked: Vec<LinkedFile>,
    skipped: usize,
}

pub(crate) fn scan(ctx: &Ctx, top: Option<usize>) -> LargestResult {
    scan_roots(&ctx.roots, top)
}

fn scan_roots(roots: &[PathBuf], top: Option<usize>) -> LargestResult {
    let mut totals = BTreeMap::<PathBuf, u64>::new();
    let mut skipped = 0usize;
    let mut extra_errors = Vec::new();
    let mut usage = crate::safety::DiskUsage::default();

    // Overlapping roots (e.g. ~/work and ~/work/project) would count the same
    // files twice; keep only roots not contained in another configured root.
    for root in crate::ops::project::normalized_roots(roots) {
        scan_root(
            root,
            &mut totals,
            &mut skipped,
            &mut extra_errors,
            &mut usage,
        );
    }

    let mut ranked = totals.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|(left_path, left_size), (right_path, right_size)| {
        right_size
            .cmp(left_size)
            .then_with(|| left_path.cmp(right_path))
    });
    let limit = top.unwrap_or(20).clamp(1, 100);
    let note = if skipped > 0 || !extra_errors.is_empty() {
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
    let mut errors = Vec::new();
    if skipped > 0 {
        errors.push(format!(
            "skipped {skipped} unreadable entries; totals are lower bounds"
        ));
    }
    errors.extend(extra_errors);

    LargestResult { findings, errors }
}

fn scan_root(
    root: &Path,
    totals: &mut BTreeMap<PathBuf, u64>,
    skipped: &mut usize,
    extra_errors: &mut Vec<String>,
    usage: &mut crate::safety::DiskUsage,
) {
    let Ok(metadata) = std::fs::symlink_metadata(root) else {
        *skipped = skipped.saturating_add(1);
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    if metadata.file_type().is_file() {
        if usage.add(&metadata, root).is_err() {
            *skipped = skipped.saturating_add(1);
        }
        return;
    }
    if !metadata.file_type().is_dir() {
        return;
    }
    if scan_root_bulk(root, metadata.dev(), totals, skipped, extra_errors, usage).is_err() {
        scan_root_walkdir(root, totals, skipped, usage);
    }
}

fn scan_root_bulk(
    root: &Path,
    root_dev: u64,
    totals: &mut BTreeMap<PathBuf, u64>,
    skipped: &mut usize,
    extra_errors: &mut Vec<String>,
    usage: &mut crate::safety::DiskUsage,
) -> Result<(), BulkUnsupported> {
    let mut buffer = apfs_size::BulkBuffer::new();
    let mut top_dirs = Vec::new();
    let mut foreign_top_dirs = Vec::new();
    let mut root_skipped = 0usize;
    let visit =
        apfs_size::visit_dir_bulk(root, apfs_size::BulkMode::SizeOnly, &mut buffer, |entry| {
            if entry.kind == apfs_size::EntryKind::Directory {
                let child = root.join(entry.name);
                if !entry.mountpoint && entry.dev == root_dev {
                    top_dirs.push(child);
                } else {
                    foreign_top_dirs.push(child);
                }
            }
            Ok::<(), std::convert::Infallible>(())
        });
    match visit {
        Ok(()) => {}
        Err(apfs_size::BulkVisitError::Bulk(apfs_size::BulkError::Unsupported)) => {
            return Err(BulkUnsupported);
        }
        Err(apfs_size::BulkVisitError::Bulk(apfs_size::BulkError::Io(_))) => {
            root_skipped = root_skipped.saturating_add(1);
        }
        Err(apfs_size::BulkVisitError::Visitor(never)) => match never {},
    }

    let (mut measurements, worker_panics) = measure_top_dirs(&top_dirs, root_dev, &mut buffer)?;
    for directory in foreign_top_dirs {
        totals.entry(directory).or_default();
    }
    *skipped = skipped.saturating_add(root_skipped);
    if worker_panics > 0 {
        *skipped = skipped.saturating_add(worker_panics);
        extra_errors.push(format!(
            "{worker_panics} directory scan worker thread(s) panicked; totals are lower bounds"
        ));
    }
    measurements.sort_by(|left, right| left.path.cmp(&right.path));
    for measurement in measurements {
        merge_top_measurement(measurement, totals, skipped, usage);
    }
    Ok(())
}

fn measure_top_dirs(
    top_dirs: &[PathBuf],
    root_dev: u64,
    primary_buffer: &mut apfs_size::BulkBuffer,
) -> Result<(Vec<TopDirMeasurement>, usize), BulkUnsupported> {
    let max_workers = std::thread::available_parallelism()
        .map_or(4, std::num::NonZeroUsize::get)
        .min(8);
    let workers = top_dirs.len().min(max_workers);
    if workers <= 1 {
        let mut measurements = Vec::with_capacity(top_dirs.len());
        for directory in top_dirs {
            measurements.push(measure_top_dir(
                directory.clone(),
                root_dev,
                primary_buffer,
            )?);
        }
        return Ok((measurements, 0));
    }

    let next_index = AtomicUsize::new(0);
    let unsupported = AtomicBool::new(false);
    let run_worker = |worker_buffer: &mut apfs_size::BulkBuffer| {
        let mut local = Vec::new();
        while !unsupported.load(Ordering::Relaxed) {
            let index = next_index.fetch_add(1, Ordering::Relaxed);
            let Some(directory) = top_dirs.get(index) else {
                break;
            };
            match measure_top_dir(directory.clone(), root_dev, worker_buffer) {
                Ok(measurement) => local.push(measurement),
                Err(BulkUnsupported) => {
                    unsupported.store(true, Ordering::Relaxed);
                    return Err(BulkUnsupported);
                }
            }
        }
        Ok(local)
    };
    let mut panicked = 0usize;
    let (first_result, spawned_results) = std::thread::scope(|scope| {
        let handles: Vec<_> = (1..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut worker_buffer = apfs_size::BulkBuffer::new();
                    run_worker(&mut worker_buffer)
                })
            })
            .collect();
        let first_result = run_worker(primary_buffer);
        let mut spawned_results = Vec::with_capacity(handles.len());
        for handle in handles {
            match handle.join() {
                Ok(result) => spawned_results.push(result),
                Err(_) => panicked = panicked.saturating_add(1),
            }
        }
        (first_result, spawned_results)
    });
    if unsupported.load(Ordering::Relaxed) {
        return Err(BulkUnsupported);
    }
    let mut measurements = first_result?;
    for batch in spawned_results {
        measurements.extend(batch?);
    }
    Ok((measurements, panicked))
}

fn measure_top_dir(
    top_path: PathBuf,
    root_dev: u64,
    buffer: &mut apfs_size::BulkBuffer,
) -> Result<TopDirMeasurement, BulkUnsupported> {
    let mut top_bytes = 0u64;
    let mut sub_totals = Vec::new();
    let mut walk_indices = Vec::new();
    let mut linked = Vec::new();
    let mut skipped = 0usize;

    let visit =
        apfs_size::visit_dir_bulk(&top_path, apfs_size::BulkMode::SizeOnly, buffer, |entry| {
            match entry.kind {
                apfs_size::EntryKind::Directory => {
                    let sub_index = sub_totals.len();
                    sub_totals.push((top_path.join(entry.name), 0u64));
                    if !entry.mountpoint && entry.dev == root_dev {
                        walk_indices.push(sub_index);
                    }
                }
                apfs_size::EntryKind::File => {
                    if entry.dev != root_dev {
                        skipped = skipped.saturating_add(1);
                        return Ok(());
                    }
                    if entry.nlink > 1 {
                        linked.push(LinkedFile {
                            path: top_path.join(entry.name),
                            sub_index: None,
                            dev: entry.dev,
                            ino: entry.ino,
                            allocated: entry.allocated,
                        });
                    } else {
                        top_bytes = top_bytes.saturating_add(entry.allocated);
                    }
                }
                apfs_size::EntryKind::Other => {}
            }
            Ok::<(), std::convert::Infallible>(())
        });
    match visit {
        Ok(()) => {}
        Err(apfs_size::BulkVisitError::Bulk(apfs_size::BulkError::Unsupported)) => {
            return Err(BulkUnsupported);
        }
        Err(apfs_size::BulkVisitError::Bulk(apfs_size::BulkError::Io(_))) => {
            skipped = skipped.saturating_add(1);
        }
        Err(apfs_size::BulkVisitError::Visitor(never)) => match never {},
    }

    for sub_index in walk_indices {
        let sub_bytes = measure_sub_dir(
            &sub_totals[sub_index].0,
            sub_index,
            root_dev,
            buffer,
            &mut linked,
            &mut skipped,
        )?;
        sub_totals[sub_index].1 = sub_bytes;
        top_bytes = top_bytes.saturating_add(sub_bytes);
    }

    Ok(TopDirMeasurement {
        path: top_path,
        bytes: top_bytes,
        sub_totals,
        linked,
        skipped,
    })
}

fn measure_sub_dir(
    root: &Path,
    sub_index: usize,
    root_dev: u64,
    buffer: &mut apfs_size::BulkBuffer,
    linked: &mut Vec<LinkedFile>,
    skipped: &mut usize,
) -> Result<u64, BulkUnsupported> {
    let mut bytes = 0u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let visit =
            apfs_size::visit_dir_bulk(&directory, apfs_size::BulkMode::SizeOnly, buffer, |entry| {
                match entry.kind {
                    apfs_size::EntryKind::Directory => {
                        if !entry.mountpoint && entry.dev == root_dev {
                            pending.push(directory.join(entry.name));
                        }
                    }
                    apfs_size::EntryKind::File => {
                        if entry.dev != root_dev {
                            *skipped = skipped.saturating_add(1);
                            return Ok(());
                        }
                        if entry.nlink > 1 {
                            linked.push(LinkedFile {
                                path: directory.join(entry.name),
                                sub_index: Some(sub_index),
                                dev: entry.dev,
                                ino: entry.ino,
                                allocated: entry.allocated,
                            });
                        } else {
                            bytes = bytes.saturating_add(entry.allocated);
                        }
                    }
                    apfs_size::EntryKind::Other => {}
                }
                Ok::<(), std::convert::Infallible>(())
            });
        match visit {
            Ok(()) => {}
            Err(apfs_size::BulkVisitError::Bulk(apfs_size::BulkError::Unsupported)) => {
                return Err(BulkUnsupported);
            }
            Err(apfs_size::BulkVisitError::Bulk(apfs_size::BulkError::Io(_))) => {
                *skipped = skipped.saturating_add(1);
            }
            Err(apfs_size::BulkVisitError::Visitor(never)) => match never {},
        }
    }
    Ok(bytes)
}

fn merge_top_measurement(
    mut measurement: TopDirMeasurement,
    totals: &mut BTreeMap<PathBuf, u64>,
    skipped: &mut usize,
    usage: &mut crate::safety::DiskUsage,
) {
    *skipped = skipped.saturating_add(measurement.skipped);
    if !measurement.linked.is_empty() {
        let sub_totals = &mut measurement.sub_totals;
        measurement.linked.sort_by(|left, right| {
            left.path
                .components()
                .cmp(right.path.components())
                .then_with(|| left.dev.cmp(&right.dev))
                .then_with(|| left.ino.cmp(&right.ino))
        });
        for link in measurement.linked {
            if usage.record_hard_link(link.dev, link.ino) {
                measurement.bytes = measurement.bytes.saturating_add(link.allocated);
                if let Some(sub_index) = link.sub_index {
                    sub_totals[sub_index].1 =
                        sub_totals[sub_index].1.saturating_add(link.allocated);
                }
            }
        }
    }
    let total = totals.entry(measurement.path).or_default();
    *total = total.saturating_add(measurement.bytes);
    for (sub_path, sub_bytes) in measurement.sub_totals {
        let sub_total = totals.entry(sub_path).or_default();
        *sub_total = sub_total.saturating_add(sub_bytes);
    }
}

fn scan_root_walkdir(
    root: &Path,
    totals: &mut BTreeMap<PathBuf, u64>,
    skipped: &mut usize,
    usage: &mut crate::safety::DiskUsage,
) {
    // One device, as every other report walk: a mounted volume below a
    // root is not part of the root's disk usage.
    for result in walkdir::WalkDir::new(root)
        .follow_links(false)
        .follow_root_links(false)
        .same_file_system(true)
        .sort_by_file_name()
    {
        let entry = match result {
            Ok(entry) => entry,
            Err(_) => {
                // Report-only traversal never grants deletion authority, so
                // partial visibility is useful when its lower bound is disclosed.
                *skipped = skipped.saturating_add(1);
                continue;
            }
        };
        if entry.file_type().is_dir() && (1..=2).contains(&entry.depth()) {
            totals.entry(entry.path().to_path_buf()).or_default();
            continue;
        }
        if !entry.file_type().is_file() || entry.depth() < 2 {
            continue;
        }
        let bytes = match entry
            .metadata()
            .map_err(anyhow::Error::from)
            .and_then(|metadata| usage.add(&metadata, entry.path()))
        {
            Ok(bytes) => bytes,
            Err(_) => {
                *skipped = skipped.saturating_add(1);
                continue;
            }
        };
        add_to_shallow_ancestors(root, entry.path(), bytes, totals);
    }
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
    use super::*;
    use crate::ops::{CLONED_FILE, TestFixture, clone_file, write_incompressible};

    /// A clone holds every block of the file in its allocation, so each
    /// folder counts every clone it holds in full, where it is, as `du` does.
    #[test]
    fn each_folder_counts_the_allocated_blocks_of_the_clones_it_holds() {
        let fixture = TestFixture::new("devtrim-largest-clones");
        let root = fixture.path().join("root");
        for folder in ["whole/x", "whole/y", "split", "other"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        write_incompressible(&root.join("whole/x/source"));
        clone_file(&root.join("whole/x/source"), &root.join("whole/y/clone"));
        write_incompressible(&root.join("split/source"));
        clone_file(&root.join("split/source"), &root.join("other/clone"));

        let _trust = crate::safety::AssumeSnapshotFree::new(true);
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
        let copies = |count: u64| CLONED_FILE * count..CLONED_FILE * count * 11 / 10;
        let whole = size("whole");
        assert!(
            copies(2).contains(&whole),
            "a folder holding a file and its clone measured {whole} bytes"
        );
        for name in ["whole/x", "whole/y", "split", "other"] {
            let part = size(name);
            assert!(
                copies(1).contains(&part),
                "{name}, holding one clone of two, measured {part} bytes"
            );
        }
    }

    #[test]
    fn bulk_scan_deduplicates_hard_links_across_parallel_subtrees_and_counts_unreadable_dirs() {
        let fixture = TestFixture::new("devtrim-largest-hardlinks");
        let root = fixture.path().join("root");
        std::fs::create_dir_all(root.join("alpha/a-sub")).unwrap();
        std::fs::create_dir_all(root.join("alpha/sub")).unwrap();
        std::fs::create_dir_all(root.join("beta/sub")).unwrap();
        std::fs::write(root.join("top-file"), vec![b't'; 128 * 1024]).unwrap();
        let source = root.join("alpha/sub/shared.bin");
        std::fs::write(&source, vec![b'a'; 128 * 1024]).unwrap();
        std::fs::hard_link(&source, root.join("beta/sub/shared.bin")).unwrap();
        // Direct file sorting BEFORE a sibling directory (`direct.bin` < `sub`).
        let direct_alpha = root.join("alpha/direct.bin");
        std::fs::write(&direct_alpha, vec![b'd'; 64 * 1024]).unwrap();
        std::fs::hard_link(&direct_alpha, root.join("alpha/sub/direct.bin")).unwrap();
        std::fs::hard_link(&direct_alpha, root.join("root-hardlink.bin")).unwrap();
        // Nested directory sorting BEFORE a direct sibling file (`a-sub` < `z-direct.bin`).
        let nested_first = root.join("alpha/a-sub/nested-first.bin");
        std::fs::write(&nested_first, vec![b'n'; 32 * 1024]).unwrap();
        std::fs::hard_link(&nested_first, root.join("alpha/z-direct.bin")).unwrap();
        let _sealed = crate::ops::Unreadable::new(&root.join("beta/locked"), 0o000);

        let result = scan_roots(std::slice::from_ref(&root), Some(100));
        let mut walkdir_totals = BTreeMap::new();
        let mut walkdir_skipped = 0usize;
        let mut walkdir_usage = crate::safety::DiskUsage::default();
        scan_root_walkdir(
            &root,
            &mut walkdir_totals,
            &mut walkdir_skipped,
            &mut walkdir_usage,
        );
        let shared_bytes =
            crate::safety::allocated_bytes(&std::fs::symlink_metadata(&source).unwrap(), &source)
                .unwrap();
        let direct_bytes = crate::safety::allocated_bytes(
            &std::fs::symlink_metadata(&direct_alpha).unwrap(),
            &direct_alpha,
        )
        .unwrap();
        let nested_first_bytes = crate::safety::allocated_bytes(
            &std::fs::symlink_metadata(&nested_first).unwrap(),
            &nested_first,
        )
        .unwrap();
        let bulk_totals: BTreeMap<PathBuf, u64> = result
            .findings
            .iter()
            .map(|finding| {
                (
                    PathBuf::from(finding.path.as_deref().unwrap()),
                    finding.size_bytes,
                )
            })
            .collect();
        let size = |name: &str| bulk_totals[&root.join(name)];

        assert_eq!(
            size("alpha"),
            shared_bytes + direct_bytes + nested_first_bytes
        );
        assert_eq!(size("alpha/a-sub"), nested_first_bytes);
        assert_eq!(size("alpha/sub"), shared_bytes);
        assert_eq!(size("beta"), 0);
        assert_eq!(size("beta/sub"), 0);
        assert_eq!(size("beta/locked"), 0);
        assert_eq!(
            bulk_totals, walkdir_totals,
            "bulk walk and walkdir fallback must agree on every parent and child bucket"
        );
        assert_eq!(walkdir_skipped, 1);
        assert!(
            result
                .findings
                .iter()
                .all(|finding| finding.path.as_deref() != root.join("top-file").to_str()),
            "depth-1 regular files must not appear as directory buckets"
        );
        assert_eq!(result.errors.len(), 1);
        assert!(result.errors[0].contains("1 unreadable"));
    }
}
