//! Metadata-only application storage inventory; never deletion authority.

use std::collections::{BTreeSet, HashSet};
use std::ffi::OsStr;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};

use crate::report::{Action, Finding};
use crate::safety::DiskUsage;

const WHATSAPP_CONTAINERS: &[&str] = &[
    "group.net.whatsapp.WhatsApp.shared",
    "group.net.whatsapp.WhatsApp.private",
    "group.net.whatsapp.WhatsAppSMB.shared",
    "group.net.whatsapp.family",
];
const DATABASE_FILES: &[&str] = &[
    "ChatStorage.sqlite",
    "ChatStorage.sqlite-wal",
    "ChatStorage.sqlite-shm",
];
const DETAIL_LIMIT: usize = 100;
const ERROR_LIMIT: usize = 32;

pub(crate) struct AppStorageReport {
    pub(crate) findings: Vec<Finding>,
    pub(crate) errors: Vec<String>,
}

#[derive(Default)]
struct Inventory {
    findings: Vec<Finding>,
    errors: Vec<String>,
    omitted_errors: usize,
    usage: DiskUsage,
    measured_files: HashSet<(u64, u64)>,
    measured_bundles: HashSet<(u64, u64)>,
    skipped_links: usize,
    skipped_devices: usize,
    skipped_directories: usize,
}

pub(crate) fn report(home: &Path, roots: &[String]) -> AppStorageReport {
    let mut inventory = Inventory::default();
    for name in WHATSAPP_CONTAINERS {
        let container = home.join("Library/Group Containers").join(name);
        match checked_directory(&container, Some(home), true) {
            Ok(Some(_)) => inventory.whatsapp(&container),
            Ok(None) => {}
            Err(error) => inventory.error(error),
        }
    }
    let mut resolved_roots = BTreeSet::new();
    for raw in roots {
        match explicit_root(home, raw) {
            Ok(root) => {
                resolved_roots.insert(root);
            }
            Err(error) => inventory.error(error),
        }
    }
    let mut bundles = Vec::new();
    let mut omitted_count = 0usize;
    let mut omitted_bytes = 0u64;
    for root in &resolved_roots {
        inventory.screenstudio(root, &mut bundles, &mut omitted_count, &mut omitted_bytes);
    }
    inventory.findings.extend(bundles);
    if omitted_count > 0 {
        inventory.findings.push(Finding::new(
            "Screen Studio bundles omitted",
            None,
            omitted_bytes,
            format!("{omitted_count} additional bundles measured; retained detail is limited to {DETAIL_LIMIT}; not deletion authority"),
            1,
            Action::Info,
        ));
    }
    inventory.findings.push(Finding::new(
        "app-storage scope",
        None,
        0,
        format!(
            "Metadata only, live snapshot: files may change during measurement; no message, media or database contents opened. WhatsApp: four known Group Containers only. Screen Studio: saved .screenstudio bundles under explicit --root paths only ({} root(s)); no implicit home or configured-root search. Estimated allocated regular-file bytes; hard links counted once across the report, APFS clones may share blocks. Symlinks and foreign devices are not followed ({} links, {} device boundaries skipped). Screen Studio excludes .git, node_modules, target, .build, .next, .turbo, .venv, venv, .cache and .app/.framework/.photoslibrary/.xcarchive packages ({} directories skipped). Errors make totals lower bounds; not deletion authority",
            resolved_roots.len(), inventory.skipped_links, inventory.skipped_devices, inventory.skipped_directories
        ),
        1,
        Action::Info,
    ));
    if inventory.omitted_errors > 0 {
        inventory.errors.push(format!(
            "{} additional errors omitted; totals are lower bounds",
            inventory.omitted_errors
        ));
    }
    AppStorageReport {
        findings: inventory.findings,
        errors: inventory.errors,
    }
}

impl Inventory {
    fn error(&mut self, error: String) {
        if self.errors.len() < ERROR_LIMIT {
            self.errors.push(error);
        } else {
            self.omitted_errors = self.omitted_errors.saturating_add(1);
        }
    }

    fn whatsapp(&mut self, container: &Path) {
        let mut totals = [0u64; 5];
        let mut present = [false; 5];
        present[4] = true;
        let mut entries = walkdir::WalkDir::new(container)
            .follow_links(false)
            .follow_root_links(false)
            .same_file_system(true)
            .sort_by_file_name()
            .into_iter();
        let Some(device) = self.device(container) else {
            return;
        };
        while let Some(result) = entries.next() {
            let entry = match result {
                Ok(entry) => entry,
                Err(error) => {
                    self.error(format!("cannot read WhatsApp storage: {error}"));
                    continue;
                }
            };
            let Some(metadata) = self.metadata(entry.path(), device) else {
                if entry.file_type().is_dir() {
                    entries.skip_current_dir();
                }
                continue;
            };
            let Ok(relative) = entry.path().strip_prefix(container) else {
                self.error(format!(
                    "WhatsApp entry outside container: {}",
                    entry.path().display()
                ));
                continue;
            };
            if relative == Path::new("Message/Media") {
                if !metadata.is_dir() {
                    self.error(format!(
                        "WhatsApp media is not a directory: {}",
                        entry.path().display()
                    ));
                    continue;
                }
                present[0] = true;
            }
            let database = DATABASE_FILES
                .iter()
                .position(|name| relative == Path::new(name));
            if let Some(index) = database {
                if !metadata.is_file() {
                    self.error(format!(
                        "WhatsApp database is not a regular file: {}",
                        entry.path().display()
                    ));
                    if metadata.is_dir() {
                        entries.skip_current_dir();
                    }
                    continue;
                }
                present[index + 1] = true;
            }
            if !metadata.is_file() {
                if !metadata.is_dir() {
                    self.error(format!(
                        "unsupported WhatsApp storage entry: {}",
                        entry.path().display()
                    ));
                }
                continue;
            }
            let bucket = if relative.starts_with("Message/Media") {
                0
            } else {
                database.map_or(4, |index| index + 1)
            };
            self.add_file(&mut totals[bucket], &metadata, entry.path());
        }
        for (index, (bytes, present)) in totals.into_iter().zip(present).enumerate() {
            if !present {
                continue;
            }
            let (label, path) = match index {
                0 => ("WhatsApp media", container.join("Message/Media")),
                1..=3 => (
                    "WhatsApp database",
                    container.join(DATABASE_FILES[index - 1]),
                ),
                _ => ("WhatsApp other data", container.to_path_buf()),
            };
            self.findings.push(Finding::new(
                label,
                Some(path),
                bytes,
                "metadata-only live snapshot; categories do not overlap; not deletion authority",
                1,
                Action::Info,
            ));
        }
    }

    fn screenstudio(
        &mut self,
        root: &Path,
        bundles: &mut Vec<Finding>,
        omitted_count: &mut usize,
        omitted_bytes: &mut u64,
    ) {
        if root.ancestors().any(excluded_directory) {
            self.skipped_directories = self.skipped_directories.saturating_add(1);
            self.error(format!(
                "Screen Studio search root is inside an excluded directory scope: {}",
                root.display()
            ));
            return;
        }
        let Some(device) = self.device(root) else {
            return;
        };
        let mut entries = walkdir::WalkDir::new(root)
            .follow_links(false)
            .follow_root_links(false)
            .same_file_system(true)
            .sort_by_file_name()
            .into_iter();
        while let Some(result) = entries.next() {
            let entry = match result {
                Ok(entry) => entry,
                Err(error) => {
                    self.error(format!("cannot discover Screen Studio bundles: {error}"));
                    continue;
                }
            };
            let Some(metadata) = self.metadata(entry.path(), device) else {
                if entry.file_type().is_dir() {
                    entries.skip_current_dir();
                }
                continue;
            };
            if metadata.is_dir() && excluded_directory(entry.path()) {
                self.skipped_directories = self.skipped_directories.saturating_add(1);
                entries.skip_current_dir();
                continue;
            }
            if !has_extension(entry.path(), "screenstudio") {
                continue;
            }
            if !metadata.is_dir() {
                self.error(format!(
                    "Screen Studio bundle is not a directory: {}",
                    entry.path().display()
                ));
                continue;
            }
            entries.skip_current_dir();
            if !self
                .measured_bundles
                .insert((metadata.dev(), metadata.ino()))
            {
                continue;
            }
            let bytes = self.bundle_size(entry.path(), device);
            bundles.push(Finding::new(
                "Screen Studio saved project",
                Some(entry.path().to_path_buf()),
                bytes,
                "saved .screenstudio bundle, not an application cache; metadata-only live snapshot; not deletion authority",
                1,
                Action::Info,
            ));
            bundles.sort_by(|left, right| {
                right
                    .size_bytes
                    .cmp(&left.size_bytes)
                    .then_with(|| left.path.cmp(&right.path))
            });
            if bundles.len() > DETAIL_LIMIT
                && let Some(omitted) = bundles.pop()
            {
                *omitted_count = omitted_count.saturating_add(1);
                self.add_bytes(omitted_bytes, omitted.size_bytes, root);
            }
        }
    }

    fn bundle_size(&mut self, root: &Path, device: u64) -> u64 {
        let mut bytes = 0;
        let mut entries = walkdir::WalkDir::new(root)
            .follow_links(false)
            .follow_root_links(false)
            .same_file_system(true)
            .sort_by_file_name()
            .into_iter();
        while let Some(result) = entries.next() {
            let entry = match result {
                Ok(entry) => entry,
                Err(error) => {
                    self.error(format!("cannot measure Screen Studio bundle: {error}"));
                    continue;
                }
            };
            let Some(metadata) = self.metadata(entry.path(), device) else {
                if entry.file_type().is_dir() {
                    entries.skip_current_dir();
                }
                continue;
            };
            if entry.depth() > 0 && metadata.is_dir() && excluded_directory(entry.path()) {
                self.skipped_directories = self.skipped_directories.saturating_add(1);
                entries.skip_current_dir();
            } else if metadata.is_file() {
                self.add_file(&mut bytes, &metadata, entry.path());
            } else if !metadata.is_dir() {
                self.error(format!(
                    "unsupported Screen Studio storage entry: {}",
                    entry.path().display()
                ));
            }
        }
        bytes
    }

    fn device(&mut self, path: &Path) -> Option<u64> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_dir() => Some(metadata.dev()),
            Ok(_) => {
                self.error(format!(
                    "storage directory changed type: {}",
                    path.display()
                ));
                None
            }
            Err(error) => {
                self.error(format!(
                    "cannot inspect storage directory {}: {error}",
                    path.display()
                ));
                None
            }
        }
    }

    fn metadata(&mut self, path: &Path, device: u64) -> Option<std::fs::Metadata> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_symlink() => {
                self.skipped_links = self.skipped_links.saturating_add(1);
                None
            }
            Ok(metadata) if metadata.dev() != device => {
                self.skipped_devices = self.skipped_devices.saturating_add(1);
                None
            }
            Ok(metadata) => Some(metadata),
            Err(error) => {
                self.error(format!(
                    "cannot inspect storage entry {}: {error}",
                    path.display()
                ));
                None
            }
        }
    }

    fn add_file(&mut self, total: &mut u64, metadata: &std::fs::Metadata, path: &Path) {
        // A firmlink can expose a single-link file through several root spellings.
        if !self.measured_files.insert((metadata.dev(), metadata.ino())) {
            return;
        }
        match self.usage.add(metadata, path) {
            Ok(bytes) => self.add_bytes(total, bytes, path),
            Err(error) => self.error(format!("{error:#}")),
        }
    }

    fn add_bytes(&mut self, total: &mut u64, bytes: u64, path: &Path) {
        match total.checked_add(bytes) {
            Some(sum) => *total = sum,
            None => self.error(format!("allocated size overflow under {}", path.display())),
        }
    }
}

fn explicit_root(home: &Path, raw: &str) -> Result<PathBuf, String> {
    if raw.trim().is_empty() {
        return Err("app-storage --root must not be blank".to_owned());
    }
    let expanded = if raw == "~" {
        home.to_path_buf()
    } else if let Some(relative) = raw.strip_prefix("~/") {
        home.join(relative)
    } else {
        PathBuf::from(raw)
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map_err(|error| format!("cannot resolve app-storage --root: {error}"))?
            .join(expanded)
    };
    checked_directory(&absolute, None, false)?;
    std::fs::canonicalize(&absolute).map_err(|error| {
        format!(
            "cannot resolve app-storage --root {}: {error}",
            absolute.display()
        )
    })
}

/// Inspect each component before canonicalization could hide a symlink.
fn checked_directory(
    path: &Path,
    home: Option<&Path>,
    missing_ok: bool,
) -> Result<Option<std::fs::Metadata>, String> {
    let mut checked = home.map_or_else(PathBuf::new, Path::to_path_buf);
    let relative = if let Some(home) = home {
        path.strip_prefix(home)
            .map_err(|error| format!("storage path outside home: {error}"))?
    } else {
        path
    };
    let mut last = None;
    for component in relative.components() {
        if component == Component::CurDir {
            continue;
        }
        if component == Component::ParentDir {
            return Err(format!(
                "storage root has a parent component: {}",
                path.display()
            ));
        }
        checked.push(component);
        match std::fs::symlink_metadata(&checked) {
            Ok(metadata) if metadata.is_symlink() => {
                return Err(format!(
                    "refusing symlink in storage root: {}",
                    checked.display()
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(format!(
                    "storage root is not a directory: {}",
                    checked.display()
                ));
            }
            Ok(metadata) => last = Some(metadata),
            Err(error) if missing_ok && error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => {
                return Err(format!(
                    "cannot inspect storage root {}: {error}",
                    checked.display()
                ));
            }
        }
    }
    Ok(last)
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .is_some_and(|found| found.eq_ignore_ascii_case(extension))
}

fn excluded_directory(path: &Path) -> bool {
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or_default();
    [
        ".git",
        "node_modules",
        "target",
        ".build",
        ".next",
        ".turbo",
        ".venv",
        "venv",
        ".cache",
    ]
    .iter()
    .any(|excluded| name.eq_ignore_ascii_case(excluded))
        || ["app", "framework", "photoslibrary", "xcarchive"]
            .iter()
            .any(|extension| has_extension(path, extension))
}
