//! Extra copies of identical files.
//!
//! The search is explicit (`devtrim duplicates`). It is not part of `scan`:
//! hashing every project tree on a routine preview would make that command
//! unbounded. Nothing here is regenerable cache. The plan keeps one copy and
//! offers the others for Trash. `--shred` is rejected, and apply refuses a
//! permanent action if one is presented anyway.
//!
//! Hard links are one inode and are counted once, so a second name is not an
//! extra copy. APFS clone IDs are not read: `stat` does not expose them, and
//! reading them needs `getattrlist`, which this crate cannot call because
//! `unsafe_code` is forbidden. Allocated size therefore still counts a clone
//! in full, and every note says so.
//!
//! Apply re-reads both files without following links, requires the same bytes
//! and the preview hash, and only then uses the shared deletion sink. A copy
//! that drifted, or a keeper that is gone, refuses that finding and leaves
//! the file in place.

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::project::{self, UnreadFolder};
use super::{Action, ApplyOutcome, Finding, Op};
use crate::report::DuplicateProof;
use crate::safety::{self, Ctx};

pub const DEFAULT_MIN_BYTES: u64 = 1024 * 1024;
const PREFIX_BYTES: usize = 64 * 1024;
const MAX_GROUPS: usize = 200;
const READ_CHUNK: usize = 64 * 1024;

/// Package and dependency directories. A file inside one is not an independent
/// copy; removing it breaks the package that owns it.
const SKIP_DIRS: &[&[u8]] = &[
    b"bower_components",
    b"Pods",
    b"Carthage",
    b"DerivedData",
    b"target",
    b"venv",
    b"site-packages",
    b"vendor",
    b"__pycache__",
];

/// Document and application packages. Their internals are one document.
const BUNDLE_EXTENSIONS: &[&str] = &[
    "app",
    "appex",
    "bundle",
    "framework",
    "kext",
    "plugin",
    "xpc",
    "pkg",
    "mpkg",
    "photoslibrary",
    "musiclibrary",
    "tvlibrary",
    "imovielibrary",
    "fcpbundle",
    "logicx",
    "band",
    "xcodeproj",
    "xcworkspace",
    "xcarchive",
    "playground",
    "sparsebundle",
    "pages",
    "numbers",
    "key",
    "rtfd",
    "lrlibrary",
    "lrdata",
];

pub struct Duplicates {
    min_bytes: u64,
}

impl Duplicates {
    pub fn new(min_bytes: u64) -> Self {
        Self {
            min_bytes: min_bytes.max(1),
        }
    }
}

struct Candidate {
    path: PathBuf,
    logical_len: u64,
    allocated: u64,
    modified: Option<SystemTime>,
}

struct PreparedGroup {
    hash: [u8; 32],
    keeper: PathBuf,
    extras: Vec<Candidate>,
}

impl Op for Duplicates {
    fn name(&self) -> &'static str {
        "duplicates"
    }

    fn scans_roots(&self) -> bool {
        true
    }

    fn scan(&self, ctx: &Ctx, _observations: &project::ScanObservations) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        let mut unread_dirs = Vec::new();
        let mut unread_files = Vec::new();
        let mut candidates = Vec::new();
        for root in project::normalized_roots(&ctx.roots) {
            match scan_root(
                root,
                &ctx.home,
                self.min_bytes,
                &mut candidates,
                &mut unread_dirs,
                &mut unread_files,
            ) {
                Ok(()) => {}
                Err(error) => {
                    findings.push(error_finding(
                        "duplicates: scan root",
                        format!("{}: {error:#}", root.display()),
                    ));
                }
            }
        }
        let groups = group_candidates(candidates, &ctx.home, &mut unread_files);
        let (groups, omitted) = take_groups(groups, MAX_GROUPS);
        for group in groups {
            let hash = hex(&group.hash);
            for extra in group.extras {
                let proof = DuplicateProof {
                    keeper: group.keeper.clone(),
                    sha256: group.hash,
                    logical_len: extra.logical_len,
                };
                findings.push(
                    Finding::new(
                        format!(
                            "extra copy of {}",
                            extra
                                .path
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_else(|| extra.path.display().to_string())
                        ),
                        Some(extra.path),
                        extra.allocated,
                        format!(
                            "identical bytes (sha256 {hash}); keeping {}; allocated size counts an APFS clone in full, because clone IDs are not read",
                            proof.keeper.display()
                        ),
                        safety::escalate(5, extra.allocated),
                        Action::Trash,
                    )
                    .with_duplicate_proof(proof),
                );
            }
        }
        findings.sort_by(|left, right| {
            right
                .size_bytes
                .cmp(&left.size_bytes)
                .then(left.path.cmp(&right.path))
        });
        if !unread_dirs.is_empty() {
            findings.push(project::unread_folders_finding("duplicates", &unread_dirs));
        }
        if !unread_files.is_empty() {
            findings.push(error_finding(
                "duplicates: unreadable files",
                unread_list("file", &unread_files),
            ));
        }
        if omitted > 0 {
            findings.push(error_finding(
                "duplicates: list capped",
                format!(
                    "{MAX_GROUPS} groups are listed and {omitted} more were omitted; nothing omitted is offered"
                ),
            ));
        }
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let mut outcome = ApplyOutcome::new(self.name());
        for finding in findings {
            if !finding.action.is_actionable() {
                continue;
            }
            let result = apply_one(finding, ctx);
            match result {
                Ok(note) => outcome.record(finding, note),
                Err(error) => outcome.fail(error),
            }
        }
        Ok(outcome)
    }
}

fn apply_one(finding: &Finding, ctx: &Ctx) -> Result<String> {
    if finding.action != Action::Trash {
        bail!("duplicates refuses permanent deletion; move the extra copy to Trash");
    }
    let proof = finding
        .duplicate_proof()
        .context("duplicate finding is missing its keeper proof")?;
    let target = finding
        .target()
        .context("duplicate finding is missing its internal target")?;
    confirm_pair(target, proof).with_context(|| {
        format!(
            "refusing {}: it no longer matches the copy kept at {}",
            target.display(),
            proof.keeper.display()
        )
    })?;
    super::apply_filesystem_finding("duplicates", finding, ctx)?;
    Ok(format!(
        "trashed extra copy; kept {}",
        proof.keeper.display()
    ))
}

fn confirm_pair(target: &Path, proof: &DuplicateProof) -> Result<()> {
    let target_meta = std::fs::symlink_metadata(target)
        .with_context(|| format!("cannot inspect {}", target.display()))?;
    let keeper_meta = std::fs::symlink_metadata(&proof.keeper)
        .with_context(|| format!("cannot inspect {}", proof.keeper.display()))?;
    if !target_meta.file_type().is_file() || !keeper_meta.file_type().is_file() {
        bail!("both copies must be regular files");
    }
    if target_meta.len() != proof.logical_len || keeper_meta.len() != proof.logical_len {
        bail!("the length changed after the preview");
    }
    if target_meta.dev() == keeper_meta.dev() && target_meta.ino() == keeper_meta.ino() {
        bail!("both names are the same inode");
    }
    if same_clean_path(target, &proof.keeper) {
        bail!("the extra copy and the kept copy are the same path");
    }
    let mut target_file = open_regular(target)?;
    let mut keeper_file = open_regular(&proof.keeper)?;
    if !target_file
        .metadata()
        .context("cannot inspect the extra copy")?
        .is_file()
        || !keeper_file
            .metadata()
            .context("cannot inspect the kept copy")?
            .is_file()
    {
        bail!("both copies must stay regular files");
    }
    let mut target_hash = Sha256::new();
    let mut keeper_hash = Sha256::new();
    let mut target_buf = vec![0u8; READ_CHUNK];
    let mut keeper_buf = vec![0u8; READ_CHUNK];
    loop {
        let target_n = read_full(&mut target_file, &mut target_buf)?;
        let keeper_n = read_full(&mut keeper_file, &mut keeper_buf)?;
        if target_n != keeper_n || target_buf[..target_n] != keeper_buf[..keeper_n] {
            bail!("the bytes differ");
        }
        target_hash.update(&target_buf[..target_n]);
        keeper_hash.update(&keeper_buf[..keeper_n]);
        if target_n == 0 {
            break;
        }
    }
    let target_digest: [u8; 32] = target_hash.finalize().into();
    let keeper_digest: [u8; 32] = keeper_hash.finalize().into();
    if target_digest != proof.sha256 || keeper_digest != proof.sha256 {
        bail!("the bytes no longer match the preview hash");
    }
    Ok(())
}

fn same_clean_path(left: &Path, right: &Path) -> bool {
    safety::clean(left) == safety::clean(right)
}

fn open_regular(path: &Path) -> Result<File> {
    use rustix::fs::{Mode, OFlags};

    let descriptor = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .with_context(|| format!("cannot open {}", path.display()))?;
    Ok(File::from(descriptor))
}

fn read_full(file: &mut File, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match file.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

fn scan_root(
    root: &Path,
    home: &Path,
    min_bytes: u64,
    candidates: &mut Vec<Candidate>,
    unread_dirs: &mut Vec<UnreadFolder>,
    unread_files: &mut Vec<UnreadFolder>,
) -> Result<()> {
    let metadata = std::fs::symlink_metadata(root)
        .with_context(|| format!("cannot inspect {}", root.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("refusing a root that is not a real directory");
    }
    if reached_through_symlink(root)? {
        bail!("refusing a root reached through a symlink");
    }
    let device = metadata.dev();
    let mut seen_inodes = HashSet::new();
    let mut symlink_parents: HashMap<PathBuf, bool> = HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) => {
                unread_dirs.push(UnreadFolder::new(&dir, &error.into()));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    unread_dirs.push(UnreadFolder::new(&dir, &error.into()));
                    continue;
                }
            };
            let path = entry.path();
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    unread_files.push(UnreadFolder::new(&path, &error.into()));
                    continue;
                }
            };
            if metadata.dev() != device || metadata.file_type().is_symlink() {
                continue;
            }
            if metadata.is_dir() {
                if !skip_dir(&entry.file_name(), &path, home) {
                    stack.push(path);
                }
                continue;
            }
            if !metadata.is_file() || entry.file_name().as_bytes().starts_with(b".") {
                continue;
            }
            if metadata.len() < min_bytes || !seen_inodes.insert((metadata.dev(), metadata.ino())) {
                continue;
            }
            let parent = path.parent().unwrap_or(&dir);
            let linked = match symlink_parents.get(parent) {
                Some(linked) => *linked,
                None => {
                    let linked = match reached_through_symlink(parent) {
                        Ok(linked) => linked,
                        Err(error) => {
                            unread_files.push(UnreadFolder::new(parent, &error));
                            true
                        }
                    };
                    symlink_parents.insert(parent.to_path_buf(), linked);
                    linked
                }
            };
            if linked {
                continue;
            }
            let allocated = match safety::allocated_bytes(&metadata, &path) {
                Ok(allocated) => allocated,
                Err(error) => {
                    unread_files.push(UnreadFolder::new(&path, &error));
                    continue;
                }
            };
            if allocated == 0 {
                continue;
            }
            candidates.push(Candidate {
                modified: metadata.modified().ok(),
                logical_len: metadata.len(),
                allocated,
                path,
            });
        }
    }
    Ok(())
}

fn reached_through_symlink(path: &Path) -> Result<bool> {
    let literal = safety::clean(path);
    let Some(parent) = literal.parent() else {
        return Ok(true);
    };
    let Some(leaf) = literal.file_name() else {
        return Ok(true);
    };
    if parent.as_os_str().is_empty() {
        return Ok(false);
    }
    let resolved_parent = parent
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", parent.display()))?;
    Ok(safety::clean(&resolved_parent.join(leaf)) != literal)
}

fn skip_dir(name: &std::ffi::OsStr, path: &Path, home: &Path) -> bool {
    let bytes = name.as_bytes();
    if bytes.starts_with(b".") || safety::is_git_metadata_name(name) {
        return true;
    }
    if super::is_node_modules_name(name)
        || SKIP_DIRS
            .iter()
            .any(|skip| bytes.eq_ignore_ascii_case(skip))
    {
        return true;
    }
    if path == home.join("Library")
        || path == home.join("Applications")
        || path == home.join("go").join("pkg")
    {
        return true;
    }
    Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            BUNDLE_EXTENSIONS
                .iter()
                .any(|bundle| extension.eq_ignore_ascii_case(bundle))
        })
}

fn group_candidates(
    candidates: Vec<Candidate>,
    home: &Path,
    unread_files: &mut Vec<UnreadFolder>,
) -> Vec<PreparedGroup> {
    let mut by_size: BTreeMap<u64, Vec<Candidate>> = BTreeMap::new();
    for candidate in candidates {
        by_size
            .entry(candidate.logical_len)
            .or_default()
            .push(candidate);
    }
    let mut groups = Vec::new();
    for (logical_len, same_size) in by_size {
        if same_size.len() < 2 {
            continue;
        }
        let whole = logical_len as usize <= PREFIX_BYTES;
        let mut by_prefix: BTreeMap<String, Vec<Candidate>> = BTreeMap::new();
        for candidate in same_size {
            match hash_file(&candidate.path, Some(PREFIX_BYTES)) {
                Ok(hash) => by_prefix.entry(hex(&hash)).or_default().push(candidate),
                Err(error) => unread_files.push(UnreadFolder::new(&candidate.path, &error)),
            }
        }
        for (prefix, same_prefix) in by_prefix {
            if same_prefix.len() < 2 {
                continue;
            }
            let mut by_full: BTreeMap<[u8; 32], Vec<Candidate>> = BTreeMap::new();
            if whole {
                let mut hash = [0u8; 32];
                if decode_hex(&prefix, &mut hash) {
                    by_full.insert(hash, same_prefix);
                }
                // A prefix that is not valid hex is a bug in `hex`; drop the group.
            } else {
                for candidate in same_prefix {
                    match hash_file(&candidate.path, None) {
                        Ok(hash) => by_full.entry(hash).or_default().push(candidate),
                        Err(error) => unread_files.push(UnreadFolder::new(&candidate.path, &error)),
                    }
                }
            }
            for (hash, mut same) in by_full {
                if same.len() < 2 {
                    continue;
                }
                let keeper_index = keeper_index(&same, home);
                let keeper = same[keeper_index].path.clone();
                same.swap_remove(keeper_index);
                groups.push(PreparedGroup {
                    hash,
                    keeper,
                    extras: same,
                });
            }
        }
    }
    groups
}

fn keeper_index(files: &[Candidate], home: &Path) -> usize {
    let downloads = home.join("Downloads");
    files
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| {
            keeper_rank(left, &downloads).cmp(&keeper_rank(right, &downloads))
        })
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn keeper_rank(file: &Candidate, downloads: &Path) -> (bool, i64, usize, PathBuf) {
    let in_downloads = file.path.starts_with(downloads);
    let modified = file
        .modified
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(i64::MAX);
    (
        in_downloads,
        modified,
        file.path.as_os_str().len(),
        file.path.clone(),
    )
}

fn take_groups(mut groups: Vec<PreparedGroup>, max: usize) -> (Vec<PreparedGroup>, usize) {
    groups.sort_by(|left, right| {
        let left_bytes: u64 = left.extras.iter().map(|extra| extra.allocated).sum();
        let right_bytes: u64 = right.extras.iter().map(|extra| extra.allocated).sum();
        right_bytes
            .cmp(&left_bytes)
            .then(left.keeper.cmp(&right.keeper))
    });
    let omitted = groups.len().saturating_sub(max);
    groups.truncate(max);
    (groups, omitted)
}

fn hash_file(path: &Path, limit: Option<usize>) -> Result<[u8; 32]> {
    let mut file = open_regular(path)?;
    if !file
        .metadata()
        .with_context(|| format!("cannot inspect {}", path.display()))?
        .is_file()
    {
        bail!("{} is not a regular file", path.display());
    }
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; READ_CHUNK];
    let mut left = limit.unwrap_or(usize::MAX);
    while left > 0 {
        let want = buf.len().min(left);
        let count = file
            .read(&mut buf[..want])
            .with_context(|| format!("cannot read {}", path.display()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buf[..count]);
        left -= count;
    }
    Ok(hasher.finalize().into())
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0xf)]));
    }
    out
}

fn decode_hex(text: &str, out: &mut [u8; 32]) -> bool {
    if text.len() != 64 {
        return false;
    }
    let bytes = text.as_bytes();
    for (index, slot) in out.iter_mut().enumerate() {
        let high = hex_value(bytes[index * 2]);
        let low = hex_value(bytes[index * 2 + 1]);
        let (Some(high), Some(low)) = (high, low) else {
            return false;
        };
        *slot = (high << 4) | low;
    }
    true
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn error_finding(label: &str, message: String) -> Finding {
    Finding::new(label, None, 0, message.clone(), 1, Action::Info).with_scan_error(message)
}

fn unread_list(kind: &str, folders: &[UnreadFolder]) -> String {
    let mut named = folders
        .iter()
        .take(3)
        .map(|folder| format!("{} ({})", folder.path.display(), folder.reason))
        .collect::<Vec<_>>()
        .join(", ");
    if folders.len() > 3 {
        named.push_str(&format!(" and {} more", folders.len() - 3));
    }
    format!(
        "{} {kind}(s) could not be read, so nothing depending on them is offered: {named}",
        folders.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ctx(home: &Path, root: &Path) -> Ctx {
        Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: vec![root.to_path_buf()],
            roots_origin: crate::safety::RootsOrigin::Flag,
            active_days: 30,
            retain_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home: home.to_path_buf(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Stderr,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
            apply_results: Default::default(),
        }
    }

    fn write_file(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn age(path: &Path, days: u64) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(SystemTime::now() - Duration::from_secs(days * 24 * 60 * 60))
            .unwrap();
    }

    fn scan(home: &Path, root: &Path) -> Vec<Finding> {
        Duplicates::new(1)
            .scan(&ctx(home, root), &project::ScanObservations::default())
            .unwrap()
    }

    fn actionable(findings: &[Finding]) -> Vec<&Path> {
        findings
            .iter()
            .filter(|finding| finding.action == Action::Trash)
            .filter_map(Finding::target)
            .collect()
    }

    #[test]
    fn groups_identical_files_and_keeps_the_one_outside_downloads() {
        let fixture = crate::ops::TestFixture::new("devtrim-duplicates-keep");
        let home = fixture.path();
        let body = vec![7u8; 8_000];
        write_file(&home.join("Downloads/copy.bin"), &body);
        write_file(&home.join("keep/copy.bin"), &body);
        let mut different = body.clone();
        *different.last_mut().unwrap() = 9;
        write_file(&home.join("other/different.bin"), &different);
        age(&home.join("Downloads/copy.bin"), 30);
        age(&home.join("keep/copy.bin"), 1);

        let findings = scan(home, home);
        let paths = actionable(&findings);
        assert_eq!(paths, vec![home.join("Downloads/copy.bin").as_path()]);
        let proof = findings
            .iter()
            .find(|finding| finding.action == Action::Trash)
            .unwrap()
            .duplicate_proof()
            .unwrap();
        assert_eq!(proof.keeper, home.join("keep/copy.bin"));
        assert!(
            findings
                .iter()
                .all(|finding| finding.action != Action::Shred)
        );
    }

    #[test]
    fn a_shared_prefix_is_not_a_duplicate_when_the_tail_differs() {
        let fixture = crate::ops::TestFixture::new("devtrim-duplicates-prefix");
        let home = fixture.path();
        let root = home.join("proj");
        let left = vec![4u8; PREFIX_BYTES + 32];
        let mut right = left.clone();
        *right.last_mut().unwrap() = 5;
        write_file(&root.join("left.bin"), &left);
        write_file(&root.join("right.bin"), &right);
        assert!(actionable(&scan(home, &root)).is_empty());
    }

    #[test]
    fn skips_hard_links_hidden_dirs_bundles_symlinks_and_dependency_dirs() {
        let fixture = crate::ops::TestFixture::new("devtrim-duplicates-skip");
        let home = fixture.path();
        let root = home.join("proj");
        let body = vec![3u8; 4_000];
        write_file(&root.join("real.bin"), &body);
        std::fs::hard_link(root.join("real.bin"), root.join("link.bin")).unwrap();
        write_file(&root.join(".hidden/real.bin"), &body);
        write_file(&root.join("Thing.app/Contents/real.bin"), &body);
        write_file(&root.join("node_modules/pkg/real.bin"), &body);
        write_file(&root.join("target/real.bin"), &body);
        std::os::unix::fs::symlink(root.join("real.bin"), root.join("real-sym")).unwrap();
        write_file(&root.join("second.bin"), &body);
        let findings = scan(home, &root);
        let paths = actionable(&findings);
        assert_eq!(paths.len(), 1, "{paths:?}");
        assert!(paths.iter().all(|path| !path.ends_with("link.bin")));
        assert!(paths.iter().all(|path| !path.ends_with("real-sym")));
    }

    #[test]
    fn apply_trashes_only_the_extra_copy_and_rechecks_bytes() {
        let fixture = crate::ops::TestFixture::new("devtrim-duplicates-apply");
        let home = fixture.path();
        let root = home.join("proj");
        let body = vec![6u8; 6_000];
        let keeper = root.join("keep.bin");
        let extra = root.join("extra.bin");
        let drifted = root.join("drifted.bin");
        write_file(&keeper, &body);
        write_file(&extra, &body);
        write_file(&drifted, &body);
        age(&keeper, 10);
        let context = ctx(home, &root);
        let findings = Duplicates::new(1)
            .scan(&context, &project::ScanObservations::default())
            .unwrap();
        assert_eq!(actionable(&findings).len(), 2);

        let mut shredded = findings.clone();
        for finding in &mut shredded {
            if finding.action == Action::Trash {
                finding.action = Action::Shred;
            }
        }
        let refused = Duplicates::new(1).apply(&shredded, &context).unwrap();
        assert!(!refused.errors.is_empty());
        assert!(extra.exists() && drifted.exists() && keeper.exists());

        write_file(&drifted, &vec![1u8; 6_000]);
        let outcome = Duplicates::new(1).apply(&findings, &context).unwrap();
        assert!(keeper.exists(), "the kept copy must stay");
        assert!(!extra.exists(), "the unchanged extra copy is trashed");
        assert!(drifted.exists(), "a copy whose bytes changed stays");
        assert_eq!(outcome.summary.items_touched, 1);
        assert!(!outcome.errors.is_empty());
        let journal = std::fs::read_to_string(&context.journal_path).unwrap();
        assert!(journal.contains("\"action\":\"trash\""));
        assert!(journal.contains("extra.bin"));
        assert!(!journal.contains("\"action\":\"shred\""));
    }

    #[test]
    fn a_changed_keeper_blocks_the_extra_copy() {
        let fixture = crate::ops::TestFixture::new("devtrim-duplicates-keeper");
        let home = fixture.path();
        let root = home.join("proj");
        let body = vec![8u8; 5_000];
        write_file(&root.join("keep.bin"), &body);
        write_file(&root.join("extra.bin"), &body);
        age(&root.join("keep.bin"), 4);
        let context = ctx(home, &root);
        let findings = Duplicates::new(1)
            .scan(&context, &project::ScanObservations::default())
            .unwrap();
        write_file(&root.join("keep.bin"), &vec![2u8; 5_000]);
        let outcome = Duplicates::new(1).apply(&findings, &context).unwrap();
        assert!(root.join("extra.bin").exists());
        assert!(!outcome.errors.is_empty());
        assert_eq!(outcome.summary.items_touched, 0);
    }

    #[test]
    fn refuses_a_symlinked_root() {
        let fixture = crate::ops::TestFixture::new("devtrim-duplicates-root-link");
        let home = fixture.path();
        let real = home.join("real");
        let link = home.join("link");
        write_file(&real.join("a.bin"), &vec![1u8; 3_000]);
        write_file(&real.join("b.bin"), &vec![1u8; 3_000]);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let findings = scan(home, &link);
        assert!(actionable(&findings).is_empty());
        assert!(
            findings
                .iter()
                .any(|finding| finding.scan_error().is_some())
        );
    }

    #[test]
    fn default_minimum_matches_the_command() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from(["devtrim", "duplicates"]).unwrap();
        match cli.command {
            Some(crate::cli::Command::Duplicates { min_bytes }) => {
                assert_eq!(min_bytes, DEFAULT_MIN_BYTES);
            }
            other => panic!("unexpected command {other:?}"),
        }
    }

    #[test]
    fn capping_groups_drops_the_smaller_ones() {
        let make = |keeper: &str, bytes: u64| PreparedGroup {
            hash: [0; 32],
            keeper: PathBuf::from(keeper),
            extras: vec![Candidate {
                path: PathBuf::from(format!("{keeper}-extra")),
                logical_len: bytes,
                allocated: bytes,
                modified: None,
            }],
        };
        let (kept, omitted) = take_groups(vec![make("a", 10), make("b", 50), make("c", 20)], 2);
        assert_eq!(omitted, 1);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].keeper, PathBuf::from("b"));
    }
}
