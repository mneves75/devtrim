//! Read-only report of each repository's linked worktrees and their sizes; never deletion authority.
//!
//! Whether a worktree is finished with is not decidable from the disk: it can
//! hold uncommitted or ignored work and leave no date behind. So this report
//! measures and names, and never says safe, stale or removable.

use std::collections::{BTreeSet, HashSet};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::ops::project;
use crate::report::{Action, Finding};
use crate::safety::{Ctx, measure_lenient};

/// What the report cannot see, stated once with every run.
pub(crate) const NOT_ASSESSED: &str = "not assessed: uncommitted or untracked work, ignored \
     files, open files and running processes. `git worktree remove <path>` runs the \
     repository's configured programs: use it only in a repository you trust, after your own \
     review";

pub(crate) struct WorktreesReport {
    pub(crate) findings: Vec<Finding>,
    pub(crate) errors: Vec<String>,
    pub(crate) notes: Vec<String>,
}

pub(crate) fn report(ctx: &Ctx) -> WorktreesReport {
    let roots = project::normalized_roots(&ctx.roots);
    let mut report = WorktreesReport {
        findings: Vec::new(),
        errors: Vec::new(),
        notes: Vec::new(),
    };
    let mut scan_roots = Vec::with_capacity(roots.len());
    for root in &roots {
        match std::fs::canonicalize(root) {
            Ok(resolved) => scan_roots.push(resolved),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => report.errors.push(format!(
                "cannot resolve scan root {}: {error}",
                root.display()
            )),
        }
    }
    let mut unread = Vec::new();
    let mut orphans = Vec::new();
    // Every directory already listed, by identity: a linked worktree found
    // by the walk after its repository was listed costs no second query.
    let mut listed = HashSet::new();
    for root in roots {
        let repositories = match project::is_directory_if_present(root).and_then(|present| {
            present
                .then(|| project::repositories_under(root, &mut unread))
                .transpose()
        }) {
            Ok(repositories) => repositories.unwrap_or_default(),
            Err(error) => {
                report.errors.push(format!("{error:#}"));
                continue;
            }
        };
        for repository in repositories {
            if identity(&repository).is_some_and(|found| listed.contains(&found)) {
                continue;
            }
            match project::orphaned_worktree(&repository) {
                Ok(true) => {
                    orphans.push(repository);
                    continue;
                }
                Ok(false) => {}
                Err(error) => {
                    report.errors.push(format!("{error:#}"));
                    continue;
                }
            }
            match list_repository(&repository, &scan_roots, &mut listed) {
                Ok(rows) => report.findings.extend(rows),
                Err(error) => report.errors.push(format!("{error:#}")),
            }
        }
    }
    if !orphans.is_empty() {
        report.notes.push(format!(
            "skipping {} worktree(s) whose repository is gone: {}",
            orphans.len(),
            project::listed_repositories(&orphans)
        ));
    }
    if !unread.is_empty() {
        let finding = project::unread_folders_finding("repositories", &unread);
        report
            .errors
            .extend(finding.scan_error().map(str::to_owned));
    }
    report
}

/// The rows for one repository: its main worktree and each linked one, or
/// nothing when it has no linked worktree.
fn list_repository(
    repository: &Path,
    scan_roots: &[PathBuf],
    listed: &mut HashSet<(u64, u64)>,
) -> Result<Vec<Finding>> {
    let records = parse_porcelain(&project::worktree_listing(repository)?)
        .with_context(|| format!("cannot read the worktree list of {}", repository.display()))?;
    listed.extend(identity(repository));
    listed.extend(records.first().and_then(|main| identity(&main.path)));
    if records.len() < 2 {
        return Ok(Vec::new());
    }
    let common = project::git_common_dir(repository)?;
    let common = identity(&common)
        .with_context(|| format!("cannot inspect the Git directory {}", common.display()))?;
    let paths: BTreeSet<PathBuf> = records.iter().map(|record| record.path.clone()).collect();
    let mut rows = Vec::new();
    for (index, record) in records.iter().enumerate() {
        if index == 0 {
            let (label, note) = if record.bare {
                ("bare repository", "the repository itself; not measured")
            } else {
                ("main worktree", "the repository itself; not measured")
            };
            rows.push(Finding::new(
                label,
                Some(record.path.clone()),
                0,
                note,
                1,
                Action::Info,
            ));
            continue;
        }
        let mut note = record.describe();
        let size = match leads_back(&record.path, scan_roots, common) {
            Ok(()) => {
                listed.extend(identity(&record.path));
                let nested: BTreeSet<PathBuf> = paths
                    .iter()
                    .filter(|path| *path != &record.path)
                    .cloned()
                    .collect();
                let measured = measure_lenient(&record.path, &nested);
                if measured.unread > 0 {
                    note.push_str(&format!(
                        "; lower bound: {} entries could not be read",
                        measured.unread
                    ));
                }
                measured.bytes
            }
            Err(reason) => {
                note.push_str(&format!("; not measured: {reason}"));
                0
            }
        };
        rows.push(Finding::new(
            "linked worktree",
            Some(record.path.clone()),
            size,
            &note,
            1,
            Action::Info,
        ));
    }
    Ok(rows)
}

/// Whether a listed worktree may be measured: a real directory inside a scan
/// root whose `.git` file names a gitdir in this repository's `worktrees`.
/// Anything else is the repository's claim, not something to walk.
fn leads_back(path: &Path, scan_roots: &[PathBuf], common: (u64, u64)) -> Result<(), &'static str> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| "it cannot be inspected")?;
    if !metadata.is_dir() {
        return Err("it is not a directory");
    }
    let canonical = std::fs::canonicalize(path).map_err(|_| "it cannot be resolved")?;
    if !scan_roots.iter().any(|root| canonical.starts_with(root)) {
        return Err("it is outside the scan roots");
    }
    let marker = path.join(".git");
    let not_ours = "its .git does not lead back to this repository";
    let is_file = std::fs::symlink_metadata(&marker).is_ok_and(|metadata| metadata.is_file());
    if !is_file {
        return Err(not_ours);
    }
    let bytes = std::fs::read(&marker).map_err(|_| not_ours)?;
    let gitdir = gitfile_target(&bytes).ok_or(not_ours)?;
    let gitdir = path.join(gitdir);
    let registry = gitdir.parent().ok_or(not_ours)?;
    if registry.file_name() != Some(OsStr::new("worktrees")) {
        return Err(not_ours);
    }
    match registry.parent().and_then(identity) {
        Some(found) if found == common => Ok(()),
        _ => Err(not_ours),
    }
}

/// The path a `.git` file names, read as Git reads it (git v2.51.0
/// `setup.c` `read_gitfile_gently`): the exact prefix `gitdir: `, then every
/// byte but trailing CR and LF.
fn gitfile_target(bytes: &[u8]) -> Option<&OsStr> {
    let mut target = bytes.strip_prefix(b"gitdir: ")?;
    while let [rest @ .., b'\n' | b'\r'] = target {
        target = rest;
    }
    (!target.is_empty()).then(|| OsStr::from_bytes(target))
}

fn identity(path: &Path) -> Option<(u64, u64)> {
    std::fs::metadata(path)
        .ok()
        .map(|metadata| (metadata.dev(), metadata.ino()))
}

/// One record of `git worktree list --porcelain -z`.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Record {
    path: PathBuf,
    branch: Option<String>,
    detached: bool,
    bare: bool,
    locked: Option<String>,
    prunable: Option<String>,
}

impl Record {
    /// The fixed-wording note for a linked worktree: what Git reported.
    fn describe(&self) -> String {
        let mut note = match &self.branch {
            Some(branch) => format!("branch {branch}"),
            None if self.detached => "detached HEAD".to_owned(),
            None => "no branch reported".to_owned(),
        };
        for (word, reason) in [("locked", &self.locked), ("prunable", &self.prunable)] {
            match reason.as_deref() {
                Some("") => note.push_str(&format!("; {word}")),
                Some(reason) => note.push_str(&format!("; {word}: {reason}")),
                None => {}
            }
        }
        note
    }
}

/// Parse NUL-delimited porcelain: each attribute ends in NUL and an empty
/// field ends a record. Unknown attributes are ignored; an attribute before
/// any `worktree` line, a record left unterminated, or no record at all is an
/// error.
pub(crate) fn parse_porcelain(output: &[u8]) -> Result<Vec<Record>> {
    // Every field ends in NUL, so the piece after the last one is not a field;
    // without this, a list cut right after a field would read as complete.
    let Some(fields) = output.strip_suffix(b"\0") else {
        bail!("the worktree list did not end with a NUL");
    };
    let mut records = Vec::new();
    let mut current: Option<Record> = None;
    for field in fields.split(|byte| *byte == 0) {
        if field.is_empty() {
            records.extend(current.take());
            continue;
        }
        if let Some(path) = field.strip_prefix(b"worktree ") {
            if current.is_some() {
                bail!("a worktree record did not end before the next one");
            }
            current = Some(Record {
                path: PathBuf::from(OsStr::from_bytes(path)),
                ..Record::default()
            });
            continue;
        }
        let Some(record) = current.as_mut() else {
            bail!("an attribute came before any worktree line");
        };
        let (key, value) = match field.iter().position(|byte| *byte == b' ') {
            Some(space) => (&field[..space], Some(&field[space + 1..])),
            None => (field, None),
        };
        let text =
            |value: Option<&[u8]>| String::from_utf8_lossy(value.unwrap_or_default()).into_owned();
        match key {
            b"branch" => {
                let reference = text(value);
                let short = reference.strip_prefix("refs/heads/").unwrap_or(&reference);
                record.branch = Some(short.to_owned());
            }
            b"detached" => record.detached = true,
            b"bare" => record.bare = true,
            b"locked" => record.locked = Some(text(value)),
            b"prunable" => record.prunable = Some(text(value)),
            _ => {}
        }
    }
    if current.is_some() {
        bail!("the worktree list ended inside a record");
    }
    if records.is_empty() {
        bail!("Git listed no worktree");
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_records_keep_odd_paths_whole_and_refuse_truncation() {
        let listing = b"worktree /w/main\0HEAD 1111\0branch refs/heads/main\0\0\
worktree /w/line\nbreak\0HEAD 2222\0detached\0locked\0future-attribute x\0\0\
worktree /w/\xffraw\0HEAD 3333\0branch refs/heads/topic\0locked on usb\0prunable gitdir file points to non-existent location\0\0";
        let records = parse_porcelain(listing).unwrap();
        assert_eq!(records.len(), 3);
        assert_eq!(records[1].path, PathBuf::from("/w/line\nbreak"));
        assert_eq!(records[1].describe(), "detached HEAD; locked");
        assert_eq!(records[2].path.as_os_str().as_bytes(), b"/w/\xffraw");
        assert_eq!(
            records[2].describe(),
            "branch topic; locked: on usb; prunable: gitdir file points to non-existent location"
        );
        for broken in [&b"worktree /w/main\0HEAD 1111\0"[..], b"HEAD 1111\0\0", b""] {
            assert!(parse_porcelain(broken).is_err(), "{broken:?}");
        }
    }
}
