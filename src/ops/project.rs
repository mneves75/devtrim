//! Shared Git-project activity and ownership checks for project cleanup ops.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use crate::process::{BoundedCommand as _, QUERY_TIMEOUT};
use crate::report::{Action, Finding};
use crate::safety::is_git_metadata_name;

type ActivityObservation = Arc<OnceLock<std::result::Result<String, String>>>;

/// Observations belong to one preview only; apply always probes again.
#[derive(Default)]
pub(crate) struct ScanObservations {
    process_cwds: OnceLock<std::result::Result<Vec<PathBuf>, String>>,
    commits: Mutex<BTreeMap<PathBuf, ActivityObservation>>,
}

impl ScanObservations {
    pub(crate) fn process_cwds(&self) -> Result<&[PathBuf]> {
        self.process_cwds
            .get_or_init(|| {
                crate::safety::build_process_cwds()
                    .context("cannot verify build-process liveness")
                    .map_err(|error| format!("{error:#}"))
            })
            .as_ref()
            .map(Vec::as_slice)
            .map_err(|error| anyhow::anyhow!("{error}"))
    }

    pub(crate) fn last_activity(&self, root: &Path) -> Result<String> {
        let commit = {
            let mut commits = self
                .commits
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(
                commits
                    .entry(root.to_path_buf())
                    .or_insert_with(|| Arc::new(OnceLock::new())),
            )
        };
        match commit.get_or_init(|| repo_last_activity(root).map_err(|error| format!("{error:#}")))
        {
            Ok(last_activity) => Ok(last_activity.clone()),
            Err(error) => Err(anyhow::anyhow!("{error}")),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_process_cwds(process_cwds: Vec<PathBuf>) -> Self {
        Self {
            process_cwds: OnceLock::from(Ok(process_cwds)),
            commits: Mutex::new(BTreeMap::new()),
        }
    }
}

/// The nearest repository above `path` that can speak for it. A repository
/// at or above the home folder — dotfiles kept as a repository at `~` — is
/// never an owner: its last commit says nothing about a project below it
/// that has no repository of its own, so such a project is not judged at
/// all, at scan or at apply (`safety::holds_home` decides, failing closed).
pub(crate) fn owning_repo(path: &Path, home: &Path) -> Result<Option<PathBuf>> {
    Ok(nearest_repo(path)?.filter(|repo| !crate::safety::holds_home(repo, home)))
}

/// The nearest folder above `path` holding a Git marker, whatever it is: a
/// scan uses it to name what `owning_repo` declined.
pub(crate) fn nearest_repo(path: &Path) -> Result<Option<PathBuf>> {
    let mut current = path.to_path_buf();
    while current.parent().is_some() {
        current.pop();
        if has_git_marker(&current)? {
            return Ok(Some(current));
        }
    }
    Ok(None)
}

pub(crate) fn has_git_marker(path: &Path) -> Result<bool> {
    match std::fs::read_dir(path) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry
                    .with_context(|| format!("cannot read Git marker under {}", path.display()))?;
                if is_git_metadata_name(&entry.file_name()) {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error)
            .with_context(|| format!("cannot inspect Git marker under {}", path.display())),
    }
}

pub(crate) fn normalized_roots(roots: &[PathBuf]) -> Vec<&Path> {
    let mut roots = roots.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    roots.sort_unstable();
    roots.dedup();
    let mut normalized = Vec::new();
    for root in roots {
        if !normalized.iter().any(|parent| root.starts_with(*parent)) {
            normalized.push(root);
        }
    }
    normalized
}

pub(crate) fn is_directory_if_present(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => std::fs::metadata(path)
            .map(|target| target.is_dir())
            .with_context(|| format!("cannot resolve scan root symlink {}", path.display())),
        Ok(metadata) => Ok(metadata.is_dir()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => {
            Err(error).with_context(|| format!("cannot inspect scan root {}", path.display()))
        }
    }
}

pub(crate) fn repo_last_activity(root: &Path) -> Result<String> {
    repo_last_activity_with(root, "git")
}

#[cfg(test)]
pub(crate) fn init_old_git_repo(repo: &Path) -> Result<()> {
    std::fs::create_dir_all(repo)
        .with_context(|| format!("cannot create old Git fixture {}", repo.display()))?;
    let initialized = Command::new("git")
        .args(["init", "-q"])
        .current_dir(repo)
        .status()
        .with_context(|| format!("cannot initialize old Git fixture {}", repo.display()))?;
    if !initialized.success() {
        anyhow::bail!("git init failed for old fixture {}", repo.display());
    }
    commit_old_git_fixture(repo, &[])
}

/// Commits `tracked` (paths relative to `repo`) into an old fixture, dated like
/// its first commit so the repository stays stale while tracking them. Ignore
/// rules and the developer's own Git configuration play no part.
#[cfg(test)]
pub(crate) fn commit_old_git_fixture(repo: &Path, tracked: &[&str]) -> Result<()> {
    let git = |arguments: &[&str]| -> Result<()> {
        let status = Command::new("git")
            .args(arguments)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
            .current_dir(repo)
            .status()
            .with_context(|| format!("cannot run git in old fixture {}", repo.display()))?;
        if !status.success() {
            anyhow::bail!(
                "git {arguments:?} failed for old fixture {}",
                repo.display()
            );
        }
        Ok(())
    };
    if !tracked.is_empty() {
        git(&[&["add", "-f", "--"], tracked].concat())?;
    }
    git(&[
        "-c",
        "user.name=devtrim-test",
        "-c",
        "user.email=devtrim@example.invalid",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.hooksPath=/dev/null",
        "commit",
        "--allow-empty",
        "-q",
        "-m",
        "old fixture",
    ])
}

/// The newest of HEAD's commit date and HEAD's newest reflog entry.
///
/// A commit date alone reads a repository as stale the moment an old project
/// is cloned or an old tag checked out — exactly when its dependencies were
/// just installed. Both of those write the HEAD reflog, so its newest entry is
/// the activity signal; a repository with reflogs disabled falls back to the
/// commit date, which is what it would have been judged by anyway.
///
/// HEAD is read on its own rather than through the reflog walk: the commit a
/// newest reflog entry names need not be HEAD once an entry is deleted or HEAD
/// moves without logging, and the walk would then answer for an older commit.
pub(crate) fn repo_last_activity_with(root: &Path, git: &str) -> Result<String> {
    if !has_git_marker(root)? {
        anyhow::bail!("not a Git repository: {}", root.display());
    }
    // Both dates are rendered in UTC, the clock `iso_days_ago` counts in.
    // `%cs` and `--date=format:` use each entry's recorded offset instead, so
    // west of Greenwich an evening's activity read as the previous day.
    let commit = iso_date(
        &hardened_git_log(root, git, &["--date=format-local:%Y-%m-%d", "--format=%cd"])?,
        root,
    )?;
    let reflog = hardened_git_log(
        root,
        git,
        &["-g", "--date=format-local:%Y-%m-%d", "--format=%gd"],
    )?;
    if reflog.is_empty() {
        return Ok(commit);
    }
    let entry = reflog
        .strip_prefix("HEAD@{")
        .and_then(|selector| selector.strip_suffix('}'))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Git returned an invalid reflog entry for {}",
                root.display()
            )
        })?;
    Ok(commit.max(iso_date(entry, root)?))
}

/// `git -C <root>` against a repository that may be hostile, ready for its
/// subcommand.
///
/// Neutralizes repository-controlled config while inspecting an untrusted
/// clone, and ambient repository-selection variables that would make git
/// answer for a different repo than the one that owns the deletion target.
/// Git cannot ignore repository config wholesale, so every path by which these
/// read-only queries could spawn a configured program is closed explicitly:
/// hooks and fsmonitor here, plus a promisor remote that would lazily fetch a
/// missing object through its configured `uploadpack`; `log` closes signature
/// display (`gpg.program`) itself. A git too old to know `--no-lazy-fetch`
/// fails, which refuses rather than trusts. Ambient pathspec settings are
/// dropped too, so no query's matching depends on the caller's environment.
fn hardened_git(root: &Path, git: &str) -> Command {
    let mut command = Command::new(git);
    command
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env_remove("GIT_DISCOVERY_ACROSS_FILESYSTEM")
        .env_remove("GIT_LITERAL_PATHSPECS")
        .env_remove("GIT_GLOB_PATHSPECS")
        .env_remove("GIT_NOGLOB_PATHSPECS")
        .env_remove("GIT_ICASE_PATHSPECS")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "--no-optional-locks",
            "--no-lazy-fetch",
            "--no-pager",
        ]);
    command
}

/// Whether the repository at `root` tracks any file under `target`, which lies
/// inside it: one directory, checked against a fresh listing of the index.
pub(crate) fn tracks_files_under(root: &Path, target: &Path) -> Result<bool> {
    TrackedIndex::list(root)?.holds_tracked_files(target)
}

/// What one repository's index tracks, listed once and resolved through the
/// volume to the directories that hold it.
///
/// A directory holding tracked files is part of the repository, not build
/// output — CocoaPods recommends committing `Pods`, and Mole V1.56.0 protects
/// such a directory for the same reason (`lib/clean/project.sh`) — so it is
/// never offered or removed. Git cannot say which tracked path names a given
/// directory: it compares spellings, while the volume compares names
/// case-insensitively, normalization-insensitively and with full Unicode case
/// folding (`STRASSE` names a directory created as `Straße`, and `kit` one
/// spelled with a Kelvin sign). So each tracked path's ancestor at the
/// directory's depth is looked up on the volume and compared with the
/// directory by device and inode; a case-sensitive volume keeps different
/// spellings apart. A listing that fails refuses rather than trusts.
pub(crate) struct TrackedIndex {
    root: PathBuf,
    listed: Vec<u8>,
    /// The tracked ancestors at each depth asked about, by device and inode.
    resolved: std::collections::HashMap<usize, std::collections::HashSet<(u64, u64)>>,
}

impl TrackedIndex {
    pub(crate) fn list(root: &Path) -> Result<Self> {
        let mut command = hardened_git(root, "git");
        command.args(["ls-files", "-z"]);
        let listed = hardened_output(command, || {
            format!("Git tracked-file check failed for {}", root.display())
        })?;
        Ok(Self {
            root: root.to_path_buf(),
            listed,
            resolved: std::collections::HashMap::new(),
        })
    }

    /// Whether any tracked path lies inside `target`, a directory inside the
    /// repository.
    pub(crate) fn holds_tracked_files(&mut self, target: &Path) -> Result<bool> {
        use std::os::unix::fs::MetadataExt;

        let relative = target.strip_prefix(&self.root).with_context(|| {
            format!(
                "{} is not inside its repository {}",
                target.display(),
                self.root.display()
            )
        })?;
        let depth = relative.components().count();
        let wanted = std::fs::symlink_metadata(target)
            .with_context(|| format!("cannot inspect {}", target.display()))?;
        if !self.resolved.contains_key(&depth) {
            let identities = self.resolve(depth)?;
            self.resolved.insert(depth, identities);
        }
        Ok(self
            .resolved
            .get(&depth)
            .is_some_and(|identities| identities.contains(&(wanted.dev(), wanted.ino()))))
    }

    /// The device and inode of every tracked path's ancestor `depth`
    /// components below the root, as the volume resolves its spelling.
    fn resolve(&self, depth: usize) -> Result<std::collections::HashSet<(u64, u64)>> {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::MetadataExt;

        let mut ancestors = std::collections::HashSet::new();
        for entry in self.listed.split(|byte| *byte == 0) {
            let entry = Path::new(std::ffi::OsStr::from_bytes(entry));
            if entry.components().count() > depth {
                ancestors.insert(entry.components().take(depth).collect::<PathBuf>());
            }
        }
        let mut identities = std::collections::HashSet::new();
        for ancestor in ancestors {
            let path = self.root.join(&ancestor);
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) => {
                    identities.insert((metadata.dev(), metadata.ino()));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("cannot inspect tracked path {}", path.display())
                    });
                }
            }
        }
        Ok(identities)
    }
}

/// One `git log -1` against a repository that may be hostile.
fn hardened_git_log(root: &Path, git: &str, format: &[&str]) -> Result<String> {
    let mut command = hardened_git(root, git);
    command
        // `format-local` dates render in this zone: UTC, as the cutoff is.
        .env("TZ", "UTC0")
        // Signature display runs the repository's `gpg.program`.
        .args([
            "-c",
            "log.showSignature=false",
            "log",
            "--no-show-signature",
            "-1",
        ])
        .args(format);
    let dates = hardened_output(command, || {
        format!("Git activity check failed for {}", root.display())
    })?;
    let text = String::from_utf8(dates).context("Git returned a non-UTF-8 date")?;
    Ok(text.trim().to_string())
}

/// The stdout of a hardened query. A failure names what failed and Git's own
/// first line of explanation, so a refusal it causes can be diagnosed.
fn hardened_output(mut command: Command, failure: impl Fn() -> String) -> Result<Vec<u8>> {
    let output = command
        .output_within(QUERY_TIMEOUT)
        .with_context(|| format!("{}: cannot run Git", failure()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        match stderr.lines().map(str::trim).find(|line| !line.is_empty()) {
            Some(reason) => anyhow::bail!("{}: {reason}", failure()),
            None => anyhow::bail!("{}", failure()),
        }
    }
    Ok(output.stdout)
}

fn iso_date(date: &str, root: &Path) -> Result<String> {
    if date.len() != 10
        || !date.chars().enumerate().all(|(index, character)| {
            if index == 4 || index == 7 {
                character == '-'
            } else {
                character.is_ascii_digit()
            }
        })
    {
        anyhow::bail!(
            "Git returned an invalid activity date for {}",
            root.display()
        );
    }
    Ok(date.to_string())
}

pub(crate) fn iso_days_ago(days: u32) -> String {
    iso_from_epoch_days(unix_secs().saturating_sub(u64::from(days) * 86_400) / 86_400)
}

pub(crate) fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

pub(crate) fn iso_from_epoch_days(days: u64) -> String {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{year:04}-{month:02}-{day:02}")
}

/// "the last N days", as the activity window reads in skip notes, with where
/// to change it: a window nobody can see reads as an arbitrary refusal.
pub(crate) fn activity_window(days: u32) -> String {
    let window = if days == 1 {
        "the last day".to_owned()
    } else {
        format!("the last {days} days")
    };
    format!(
        "{window} (active_days in {})",
        crate::safety::CONFIG_DISPLAY_PATH
    )
}

/// Repositories named in a skip note, so the operator knows where to act: the
/// first three by path, then how many more.
pub(crate) fn listed_repositories(repos: &[PathBuf]) -> String {
    let mut named = repos
        .iter()
        .take(3)
        .map(|repo| repo.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    if repos.len() > 3 {
        named.push_str(&format!(" and {} more", repos.len() - 3));
    }
    named
}

/// Whether `repo` is a linked worktree whose repository is gone: its `.git` is
/// a regular file naming a `gitdir` that no longer exists. Git fails every
/// query there, so the scanners leave such a worktree out and name it, rather
/// than let one orphan — common under `~/.codex/worktrees` once a repository
/// is deleted — fail the whole category. Nothing in it is offered; apply would
/// refuse it anyway, because its activity cannot be read. Any other shape is
/// not an orphan and goes to Git as before.
pub(crate) fn orphaned_worktree(repo: &Path) -> Result<bool> {
    let marker = repo.join(".git");
    let metadata = match std::fs::symlink_metadata(&marker) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("cannot inspect Git marker {}", marker.display()));
        }
    };
    if !metadata.file_type().is_file() {
        return Ok(false);
    }
    // Parsed as Git parses it (git v2.51.0 `setup.c`, `read_gitfile_gently`):
    // the exact prefix `gitdir: `, then every byte but trailing CR and LF, so a
    // path holding a newline or ending in a space is read whole. Anything Git
    // would reject is not an orphan: it goes to Git, which refuses it.
    let bytes = std::fs::read(&marker)
        .with_context(|| format!("cannot read Git marker {}", marker.display()))?;
    let Some(mut target) = bytes.strip_prefix(b"gitdir: ") else {
        return Ok(false);
    };
    while let [rest @ .., b'\n' | b'\r'] = target {
        target = rest;
    }
    if target.is_empty() {
        return Ok(false);
    }
    let target = std::ffi::OsStr::from_bytes(target);
    match std::fs::symlink_metadata(repo.join(target)) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error)
            .with_context(|| format!("cannot inspect the gitdir named by {}", marker.display())),
    }
}

/// Whether `repo` is a new repository: on an unborn branch (Git glossary:
/// HEAD names a branch that has no commit yet), with no reference of any kind
/// — `git init` before the first commit. The activity query fails there, so
/// the scanners leave such a repository out and name it instead of failing
/// the category; they ask only after that query has failed, so no repository
/// pays for it.
///
/// No commit object may exist (`cat-file --batch-all-objects`, which also
/// lists alternate stores): a repository without one has no history to
/// judge, whatever its references say, while files staged with `git add`
/// before the first commit are only blobs. The listing must also finish
/// without a complaint: Git skips an object folder or pack it cannot read,
/// says so on stderr, and still exits 0, so a silent omission would read as
/// no history. A branch turned into a dangling
/// symbolic reference, a file holding an object ID where the loose-ref folder
/// was, and an orphan checkout beside other branches all keep their commits
/// and their error. Three more answers keep a damaged repository an error
/// rather than a new one. HEAD is symbolic (`symbolic-ref -q HEAD`), so a
/// detached HEAD naming a missing commit is not new. Git finds the reference
/// storage sound (`refs verify`, Git 2.47 and later): a loose-ref folder that
/// became a junk file is damage, although Git's own `log` calls it "no
/// commits yet". And every reference resolves (`for-each-ref --count=1`
/// succeeds), so a branch whose commit was deleted is not new either. Any
/// other answer, or a Git that cannot run or lacks `refs verify`, is not new:
/// the original failure then stands.
pub(crate) fn unborn_branch(repo: &Path) -> bool {
    let git_output = |arguments: &[&str]| {
        let mut command = hardened_git(repo, "git");
        command.args(arguments);
        command.output_within(QUERY_TIMEOUT).ok()
    };
    git_output(&["symbolic-ref", "-q", "HEAD"]).is_some_and(|symbolic| symbolic.status.success())
        && git_output(&["refs", "verify"]).is_some_and(|verified| verified.status.success())
        && git_output(&["for-each-ref", "--count=1"])
            .is_some_and(|references| references.status.success())
        && git_output(&[
            "cat-file",
            "--batch-all-objects",
            "--unordered",
            "--batch-check=%(objecttype)",
        ])
        .is_some_and(|listed| {
            listed.status.success() && listed.stderr.is_empty() && holds_no_commit(&listed.stdout)
        })
}

/// Whether a `git count-objects -v` report describes an object store with
/// nothing in it: zero loose objects, zero packed objects, zero packs, and no
/// alternate store lending objects. Its keys are fixed, not translated; a
/// report missing a count, or one that is not text, proves nothing.
/// Whether a `cat-file --batch-all-objects --batch-check=%(objecttype)`
/// listing holds no commit: every line names an object type, none of them
/// `commit`. Git prints types untranslated; anything else proves nothing.
fn holds_no_commit(listing: &[u8]) -> bool {
    let Ok(listing) = std::str::from_utf8(listing) else {
        return false;
    };
    listing
        .lines()
        .all(|kind| matches!(kind, "blob" | "tree" | "tag"))
}

/// A folder a project walk could not read, and why. Like a scan root that
/// cannot be read, it bounds where the scan looked, never what may go: nothing
/// in it is offered, and the scan says so.
pub(crate) struct UnreadFolder {
    pub(crate) path: PathBuf,
    pub(crate) reason: String,
}

impl UnreadFolder {
    pub(crate) fn new(path: &Path, error: &anyhow::Error) -> Self {
        Self {
            path: path.to_path_buf(),
            reason: error.root_cause().to_string(),
        }
    }

    pub(crate) fn from_walk(error: &walkdir::Error) -> Self {
        let reason = error
            .io_error()
            .map_or_else(|| error.to_string(), ToString::to_string);
        Self {
            path: error.path().map(Path::to_path_buf).unwrap_or_default(),
            reason,
        }
    }
}

/// The error finding for folders a project walk could not read: the scan did
/// not look inside them, so nothing in them is offered, and the run reports
/// it (`CODING_STANDARDS.md` S8) rather than present a shorter plan as a clean
/// one. It names the first three with their reasons, then how many more.
pub(crate) fn unread_folders_finding(subject: &str, folders: &[UnreadFolder]) -> Finding {
    let mut named = folders
        .iter()
        .take(3)
        .map(|folder| format!("{} ({})", folder.path.display(), folder.reason))
        .collect::<Vec<_>>()
        .join(", ");
    if folders.len() > 3 {
        named.push_str(&format!(" and {} more", folders.len() - 3));
    }
    error_finding(
        format!("{subject}: unreadable folders"),
        format!(
            "{} folder(s) could not be read while looking for {subject}, so nothing in them is offered: {named}",
            folders.len()
        ),
    )
}

/// The error finding for something a scan found but could not judge: the
/// repository or candidate at `path` offers nothing, while the rest of the
/// category is judged as usual.
pub(crate) fn unjudged_finding(subject: &str, path: &Path, error: &anyhow::Error) -> Finding {
    error_finding(
        format!("{subject} not judged"),
        format!("cannot judge {subject} in {}: {error:#}", path.display()),
    )
}

/// A finding that offers nothing and carries no target; its `scan_error`
/// makes the run report failure.
fn error_finding(label: String, message: String) -> Finding {
    Finding::new(label, None, 0, &message, 5, Action::Info).with_scan_error(message)
}

pub(crate) fn repo_has_active_build(repo: &Path, process_cwds: &[PathBuf]) -> bool {
    process_cwds.iter().any(|cwd| cwd.starts_with(repo))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("devtrim-{name}-{}", std::process::id()));
        crate::ops::remove_test_path(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// An unborn branch (Git glossary: HEAD names a branch that has no commit
    /// yet — `git init` before the first commit, or `checkout --orphan`) is a
    /// state Git defines, and the activity query fails there. Only that state
    /// is recognized: a HEAD naming a commit whose object is gone and a
    /// detached HEAD both stay what they are, so their failures still refuse.
    /// Removes every loose object, as a damaged or emptied store would be.
    fn empty_object_store(repo: &Path) {
        for entry in std::fs::read_dir(repo.join(".git/objects")).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name().len() == 2 {
                std::fs::remove_dir_all(entry.path()).unwrap();
            }
        }
    }

    #[test]
    fn an_unborn_branch_is_recognized_and_a_dangling_head_is_not() {
        let base = temp("unborn");
        let git = |repo: &Path, arguments: &[&str]| {
            let status = Command::new("git")
                .args(arguments)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .current_dir(repo)
                .status()
                .unwrap();
            assert!(status.success(), "git {arguments:?}");
        };
        let fresh = base.join("fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        git(&fresh, &["init", "-q"]);
        assert!(repo_last_activity(&fresh).is_err());
        assert!(
            unborn_branch(&fresh),
            "PV project/unborn-branch: a repository without commits was not recognized"
        );

        // `git add` before the first commit stores blobs, not commits: the
        // repository on the owner's Mac that this case comes from held 27.
        let staged = base.join("staged");
        std::fs::create_dir_all(&staged).unwrap();
        git(&staged, &["init", "-q"]);
        std::fs::write(staged.join("README.md"), "draft\n").unwrap();
        git(&staged, &["add", "README.md"]);
        assert!(repo_last_activity(&staged).is_err());
        assert!(
            unborn_branch(&staged),
            "PV project/unborn-no-commit: staged files made a new repository read as having history"
        );

        // An orphan checkout beside other branches is not a new repository:
        // its references show history exists, so its failure stands.
        let orphan = base.join("orphan");
        init_old_git_repo(&orphan).unwrap();
        git(&orphan, &["checkout", "-q", "--orphan", "fresh-root"]);
        assert!(repo_last_activity(&orphan).is_err());
        assert!(
            !unborn_branch(&orphan),
            "PV project/unborn-no-commit: an orphan checkout beside other branches read as new"
        );

        // The same orphan checkout with its commit's loose-object folder
        // searchable but not listable: Git still reads the commit by name, but
        // the object listing silently leaves it out and complains on stderr.
        let unlisted = base.join("unlisted");
        init_old_git_repo(&unlisted).unwrap();
        git(&unlisted, &["checkout", "-q", "--orphan", "fresh-root"]);
        let commit = String::from_utf8(
            Command::new("git")
                .args([
                    "for-each-ref",
                    "--count=1",
                    "--format=%(objectname)",
                    "refs/heads",
                ])
                .current_dir(&unlisted)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap();
        let fanout = unlisted.join(".git/objects").join(&commit.trim()[..2]);
        assert!(
            fanout.is_dir(),
            "the commit must be a loose object: {}",
            fanout.display()
        );
        let _fanout = crate::ops::Unreadable::new(&fanout, 0o100);
        assert!(repo_last_activity(&unlisted).is_err());
        assert!(
            !unborn_branch(&unlisted),
            "PV project/unborn-complete-listing: an incomplete object listing read as no history"
        );
        drop(_fanout);

        let dangling = base.join("dangling");
        init_old_git_repo(&dangling).unwrap();
        empty_object_store(&dangling);
        assert!(repo_last_activity(&dangling).is_err());
        assert!(
            !unborn_branch(&dangling),
            "PV project/unborn-dangling: a HEAD naming a missing commit read as unborn"
        );

        // Git itself reads a repository whose loose-ref folder became a file
        // as unborn (`log` says "no commits yet"); with its objects gone too,
        // only `refs verify` sees that its reference storage is broken.
        let broken = base.join("broken-refs");
        init_old_git_repo(&broken).unwrap();
        empty_object_store(&broken);
        std::fs::remove_dir_all(broken.join(".git/refs/heads")).unwrap();
        std::fs::write(broken.join(".git/refs/heads"), "junk\n").unwrap();
        std::fs::remove_file(broken.join(".git/packed-refs")).ok();
        assert!(repo_last_activity(&broken).is_err());
        assert!(
            !unborn_branch(&broken),
            "PV project/unborn-broken-refs: broken reference storage read as unborn"
        );

        // The same folder replaced by a file holding a valid object ID passes
        // `refs verify` (it is a well-formed reference named `refs/heads`),
        // but it is a reference, so the repository is not new.
        let collided = base.join("collided-refs");
        init_old_git_repo(&collided).unwrap();
        let head = std::fs::read_to_string(collided.join(".git/refs/heads/master"))
            .or_else(|_| std::fs::read_to_string(collided.join(".git/refs/heads/main")))
            .unwrap();
        std::fs::remove_dir_all(collided.join(".git/refs/heads")).unwrap();
        std::fs::write(collided.join(".git/refs/heads"), head).unwrap();
        std::fs::remove_file(collided.join(".git/packed-refs")).ok();
        assert!(repo_last_activity(&collided).is_err());
        assert!(
            !unborn_branch(&collided),
            "PV project/unborn-no-commit: an obstructed branch path read as new"
        );

        // The only branch turned into a symbolic reference to a missing one:
        // HEAD stays symbolic, `refs verify` accepts it, and `for-each-ref`
        // leaves a dangling symbolic reference out. The commits it held are
        // still in the object store, so the repository is not new.
        let dangling_symbolic = base.join("dangling-symbolic");
        init_old_git_repo(&dangling_symbolic).unwrap();
        let branch = ["master", "main"]
            .into_iter()
            .map(|name| dangling_symbolic.join(".git/refs/heads").join(name))
            .find(|path| path.exists())
            .unwrap();
        std::fs::write(&branch, "ref: refs/heads/missing\n").unwrap();
        std::fs::remove_file(dangling_symbolic.join(".git/packed-refs")).ok();
        assert!(repo_last_activity(&dangling_symbolic).is_err());
        assert!(
            !unborn_branch(&dangling_symbolic),
            "PV project/unborn-no-commit: a repository holding commits read as new"
        );

        let born = base.join("born");
        init_old_git_repo(&born).unwrap();
        assert!(!unborn_branch(&born), "a repository with a commit is born");

        let detached = base.join("detached");
        init_old_git_repo(&detached).unwrap();
        git(&detached, &["checkout", "-q", "--detach"]);
        assert!(!unborn_branch(&detached), "a detached HEAD is not unborn");
        crate::ops::remove_test_path(base);
    }

    /// Only a listing of object types that holds no commit proves there is
    /// no history; anything that is not such a listing proves nothing.
    #[test]
    fn only_a_listing_without_a_commit_object_proves_no_history() {
        for listing in [&b""[..], b"blob\n", b"blob\nblob\ntree\n"] {
            assert!(
                holds_no_commit(listing),
                "{}",
                String::from_utf8_lossy(listing)
            );
        }
        for listing in [
            &b"blob\ncommit\n"[..],
            b"commit\n",
            b"blob\nmissing\n",
            b"blob\n\n",
            b"\xff\n",
        ] {
            assert!(
                !holds_no_commit(listing),
                "{}",
                String::from_utf8_lossy(listing)
            );
        }
    }

    /// Git records a worktree's `gitdir` as raw path bytes, so a name that is
    /// not UTF-8 must still be judged, not fail the category it sits in.
    #[test]
    fn orphaned_worktrees_are_judged_by_their_gitdir_bytes() {
        let base = temp("orphan-bytes");
        let orphan = base.join("orphan");
        std::fs::create_dir_all(&orphan).unwrap();
        let mut marker = b"gitdir: ".to_vec();
        marker.extend_from_slice(base.join("gone").as_os_str().as_encoded_bytes());
        marker.extend_from_slice(b"/\xff\n");
        std::fs::write(orphan.join(".git"), marker).unwrap();
        let live = base.join("live");
        std::fs::create_dir_all(base.join("main/.git/worktrees/live")).unwrap();
        std::fs::create_dir_all(&live).unwrap();
        std::fs::write(
            live.join(".git"),
            format!(
                "gitdir: {}\n",
                base.join("main/.git/worktrees/live").display()
            ),
        )
        .unwrap();
        let repository = base.join("repository");
        std::fs::create_dir_all(repository.join(".git")).unwrap();
        // Git keeps every byte after `gitdir: ` but trailing CR/LF, so a
        // separate Git directory named with a newline or a trailing space is
        // live, not gone (git v2.51.0 `setup.c`, `read_gitfile_gently`).
        let unusual = base.join("odd");
        let gitdir = base.join("main/.git/worktrees/odd\nname ");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::create_dir_all(&unusual).unwrap();
        let mut marker = b"gitdir: ".to_vec();
        marker.extend_from_slice(gitdir.as_os_str().as_encoded_bytes());
        marker.extend_from_slice(b"\r\n");
        std::fs::write(unusual.join(".git"), marker).unwrap();

        assert!(orphaned_worktree(&orphan).unwrap());
        assert!(!orphaned_worktree(&live).unwrap());
        assert!(
            !orphaned_worktree(&unusual).unwrap(),
            "a live worktree was misread as orphaned"
        );
        assert!(!orphaned_worktree(&repository).unwrap());
        crate::ops::remove_test_path(base);
    }

    #[test]
    fn iso_dates_are_correct_and_ordered() {
        assert_eq!(iso_from_epoch_days(0), "1970-01-01");
        assert_eq!(iso_from_epoch_days(19_723), "2024-01-01");
        assert!(iso_days_ago(30) < iso_days_ago(1));
    }

    #[test]
    fn finds_owning_repo_and_handles_orphans() {
        let base = temp("owner");
        let home = base.join("home");
        let project = base.join("project/sub/node_modules");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(base.join("project/.git")).unwrap();
        assert_eq!(
            owning_repo(&project, &home).unwrap(),
            Some(base.join("project"))
        );
        assert_eq!(
            owning_repo(&base.join("orphan/node_modules"), &home).unwrap(),
            None
        );
        // A repository at the home folder, or above it, owns nothing.
        let loose = home.join("dev/loose/node_modules");
        std::fs::create_dir_all(&loose).unwrap();
        std::fs::create_dir_all(home.join(".git")).unwrap();
        assert_eq!(owning_repo(&loose, &home).unwrap(), None);
        std::fs::remove_dir_all(home.join(".git")).unwrap();
        std::fs::create_dir_all(base.join(".git")).unwrap();
        assert_eq!(owning_repo(&loose, &home).unwrap(), None);
        crate::ops::remove_test_path(base);
    }

    #[test]
    fn detects_file_and_directory_git_markers() {
        let base = temp("git-marker");
        let file_target = base.join("file-target");
        let directory_target = base.join("directory-target");
        let case_targets = [
            ("upper-target", ".GIT"),
            ("title-target", ".Git"),
            ("mixed-target", ".gIt"),
        ];
        let normal_target = base.join("normal-target");
        std::fs::create_dir_all(&file_target).unwrap();
        std::fs::create_dir_all(directory_target.join(".git")).unwrap();
        for (directory, marker) in case_targets {
            std::fs::create_dir_all(base.join(directory).join(marker)).unwrap();
        }
        std::fs::create_dir_all(normal_target.join("git")).unwrap();
        std::fs::write(file_target.join(".git"), "gitdir: elsewhere").unwrap();

        assert!(has_git_marker(&file_target).unwrap());
        assert!(has_git_marker(&directory_target).unwrap());
        for (directory, _) in case_targets {
            assert!(has_git_marker(&base.join(directory)).unwrap());
        }
        assert!(!has_git_marker(&normal_target).unwrap());
        assert!(!has_git_marker(&base.join("missing")).unwrap());
        crate::ops::remove_test_path(base);
    }

    #[test]
    fn normalizes_duplicate_and_descendant_roots() {
        let roots = vec![
            PathBuf::from("/tmp/work/project"),
            PathBuf::from("/tmp/work"),
            PathBuf::from("/tmp/work"),
            PathBuf::from("/tmp/other"),
        ];

        assert_eq!(
            normalized_roots(&roots),
            vec![Path::new("/tmp/other"), Path::new("/tmp/work")]
        );
    }

    #[test]
    fn directory_roots_follow_readable_symlinks_and_reject_broken_ones() {
        let base = temp("root-symlink");
        let target = base.join("target");
        let readable = base.join("readable");
        let broken = base.join("broken");
        std::fs::create_dir_all(&target).unwrap();
        symlink(&target, &readable).unwrap();
        symlink(base.join("missing-target"), &broken).unwrap();

        assert!(is_directory_if_present(&readable).unwrap());
        assert!(!is_directory_if_present(&base.join("missing-root")).unwrap());
        assert!(is_directory_if_present(&broken).is_err());
        crate::ops::remove_test_path(base);
    }

    /// A Git query that never answers fails that repository, naming the
    /// program and the limit, instead of hanging the scan that asked.
    #[test]
    fn a_hung_git_query_fails_closed_naming_the_command_and_limit() {
        use std::os::unix::fs::PermissionsExt;

        let base = temp("git-hang");
        std::fs::create_dir_all(base.join(".git")).unwrap();
        let hung = base.join("hung-git");
        std::fs::write(&hung, "#!/bin/sh\nexec sleep 30\n").unwrap();
        std::fs::set_permissions(&hung, std::fs::Permissions::from_mode(0o755)).unwrap();

        let _cap = crate::process::LimitCap::new(std::time::Duration::from_millis(400));
        let error = repo_last_activity_with(&base, hung.to_str().unwrap()).unwrap_err();
        let message = format!("{error:#}");
        assert!(
            message.contains("hung-git timed out after 400ms"),
            "{message}"
        );
        assert!(message.contains("Git activity check failed"), "{message}");
        crate::ops::remove_test_path(base);
    }

    #[test]
    fn git_failure_is_not_stale() {
        let base = temp("git-fail");
        std::fs::create_dir_all(base.join(".git")).unwrap();
        assert!(repo_last_activity_with(&base, "/usr/bin/false").is_err());
        crate::ops::remove_test_path(base);
    }

    fn git_in(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "git {args:?} failed: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    /// A program the hostile config names; it proves execution by leaving a
    /// marker beside itself.
    fn planted_program(base: &Path) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let marker = base.join("executed");
        let program = base.join("planted");
        std::fs::write(
            &program,
            format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        (program, marker)
    }

    /// Plain `git log`, as a caller without the hardening would run it. Used
    /// as the positive control that each fixture really is armed.
    fn unhardened_log(repo: &Path) {
        Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["log", "-1", "--format=%cs"])
            .output()
            .unwrap();
    }

    /// A failed Git query says why, in Git's own words, so a refusal it causes
    /// can be diagnosed; here the marker is an empty directory Git rejects.
    #[test]
    fn a_failed_git_query_names_gits_reason() {
        let base = temp("git-reason");
        let repo = base.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("target")).unwrap();

        for error in [
            repo_last_activity(&repo).unwrap_err(),
            tracks_files_under(&repo, &repo.join("target")).unwrap_err(),
        ] {
            let message = format!("{error:#}");
            assert!(message.contains("not a git repository"), "{message}");
        }
        crate::ops::remove_test_path(base);
    }

    #[test]
    fn activity_probe_never_runs_a_repository_configured_signature_program() {
        let base = temp("git-signature-program");
        let repo = base.join("repo");
        init_old_git_repo(&repo).unwrap();
        let (program, marker) = planted_program(&base);
        let tree = git_in(&repo, &["rev-parse", "HEAD^{tree}"]);
        let signed = format!(
            "tree {tree}\nauthor a <a@example.invalid> 946684800 +0000\ncommitter a <a@example.invalid> 946684800 +0000\ngpgsig -----BEGIN PGP SIGNATURE-----\n \n -----END PGP SIGNATURE-----\n\nsigned\n"
        );
        std::fs::write(base.join("commit"), signed).unwrap();
        let commit = git_in(
            &repo,
            &[
                "hash-object",
                "-t",
                "commit",
                "-w",
                base.join("commit").to_str().unwrap(),
            ],
        );
        git_in(&repo, &["update-ref", "HEAD", &commit]);
        git_in(&repo, &["config", "log.showSignature", "true"]);
        git_in(&repo, &["config", "gpg.program", program.to_str().unwrap()]);

        unhardened_log(&repo);
        assert!(marker.exists(), "fixture must arm the signature program");
        std::fs::remove_file(&marker).unwrap();

        let date = repo_last_activity(&repo);
        assert!(
            !marker.exists(),
            "PV git/signature-program: the activity probe ran a repository-configured program"
        );
        // `update-ref` wrote today's reflog entry; the probe still answers.
        assert_eq!(date.unwrap(), iso_days_ago(0));
        crate::ops::remove_test_path(base);
    }

    #[test]
    fn activity_probe_never_lazily_fetches_through_a_repository_configured_transport() {
        let base = temp("git-lazy-fetch");
        let repo = base.join("repo");
        init_old_git_repo(&repo).unwrap();
        let (program, marker) = planted_program(&base);
        git_in(&repo, &["config", "core.repositoryformatversion", "1"]);
        git_in(&repo, &["config", "extensions.partialClone", "origin"]);
        git_in(
            &repo,
            &["config", "remote.origin.url", base.to_str().unwrap()],
        );
        git_in(&repo, &["config", "remote.origin.promisor", "true"]);
        git_in(
            &repo,
            &[
                "config",
                "remote.origin.uploadpack",
                program.to_str().unwrap(),
            ],
        );
        let missing = "0123456789abcdef0123456789abcdef01234567";
        std::fs::write(repo.join(".git/HEAD"), format!("{missing}\n")).unwrap();

        unhardened_log(&repo);
        assert!(marker.exists(), "fixture must arm the promisor transport");
        std::fs::remove_file(&marker).unwrap();

        let date = repo_last_activity(&repo);
        assert!(
            !marker.exists(),
            "PV git/lazy-fetch-transport: the activity probe ran a repository-configured transport"
        );
        assert!(
            date.is_err(),
            "a missing commit is unknown activity, not staleness"
        );
        crate::ops::remove_test_path(base);
    }

    #[test]
    fn a_fresh_checkout_of_an_old_commit_is_active_not_stale() {
        let base = temp("git-reflog-activity");
        let repo = base.join("repo");
        init_old_git_repo(&repo).unwrap();
        assert_eq!(repo_last_activity(&repo).unwrap(), "2000-01-01");

        // Checking out writes the HEAD reflog now, as a clone or a switch to
        // an old tag does; the commit it lands on is still from 2000.
        git_in(&repo, &["checkout", "-q", "-b", "fresh"]);
        assert_eq!(
            repo_last_activity(&repo).unwrap(),
            iso_days_ago(0),
            "PV git/reflog-activity: a just-checked-out old commit was judged stale"
        );
        crate::ops::remove_test_path(base);
    }

    #[test]
    fn a_recent_head_commit_counts_even_when_the_reflog_does_not_name_it() {
        let base = temp("git-head-beyond-reflog");
        let repo = base.join("repo");
        init_old_git_repo(&repo).unwrap();
        git_in(
            &repo,
            &[
                "-c",
                "user.name=devtrim-test",
                "-c",
                "user.email=devtrim@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "fresh",
            ],
        );
        // Deleting the newest entry leaves HEAD fresh while the reflog's newest
        // surviving entry names the 2000 commit.
        git_in(&repo, &["reflog", "delete", "HEAD@{0}"]);
        assert_eq!(
            git_in(&repo, &["log", "-g", "-1", "--format=%cs"]),
            "2000-01-01",
            "fixture: the newest reflog entry must name the old commit"
        );
        assert_eq!(
            repo_last_activity(&repo).unwrap(),
            iso_days_ago(0),
            "PV git/head-commit: a fresh HEAD was judged by an older reflog entry"
        );
        crate::ops::remove_test_path(base);
    }

    /// The cutoff counts UTC days. Read in the commit's own zone, a commit made
    /// at 23:30 at -03:00 — already the next day in UTC — landed a day early,
    /// and the probe tests failed every evening west of Greenwich.
    #[test]
    fn activity_dates_are_read_in_utc_like_the_cutoff() {
        use std::os::unix::fs::PermissionsExt;

        let base = temp("git-utc-dates");
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git_in(&repo, &["init", "-q"]);
        let committed = Command::new("git")
            .args([
                "-c",
                "user.name=devtrim-test",
                "-c",
                "user.email=devtrim@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "late evening",
            ])
            .env("GIT_AUTHOR_DATE", "2020-01-01T23:30:00-0300")
            .env("GIT_COMMITTER_DATE", "2020-01-01T23:30:00-0300")
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(committed.success());
        crate::ops::remove_test_path(repo.join(".git/logs"));
        // Stands in for a machine west of Greenwich whatever zone this one is
        // in; only a probe that pins UTC itself escapes it.
        let wrapper = base.join("git");
        std::fs::write(
            &wrapper,
            "#!/bin/sh\n[ \"$TZ\" = UTC0 ] || TZ=America/Sao_Paulo\nexport TZ\nexec git \"$@\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(
            repo_last_activity_with(&repo, wrapper.to_str().unwrap()).unwrap(),
            "2020-01-02",
            "PV git/utc-dates: activity was read in a local zone, not the cutoff's UTC"
        );
        crate::ops::remove_test_path(base);
    }

    #[test]
    fn a_repository_without_reflogs_is_judged_by_its_commit_date() {
        let base = temp("git-no-reflog");
        let repo = base.join("repo");
        init_old_git_repo(&repo).unwrap();
        crate::ops::remove_test_path(repo.join(".git/logs"));
        assert_eq!(repo_last_activity(&repo).unwrap(), "2000-01-01");
        crate::ops::remove_test_path(base);
    }

    #[test]
    fn repo_owns_equal_and_descendant_process_cwds_only() {
        let repo = Path::new("/Users/example/dev/project");
        assert!(repo_has_active_build(repo, &[repo.to_path_buf()]));
        assert!(repo_has_active_build(repo, &[repo.join("packages/app")]));
        assert!(!repo_has_active_build(
            repo,
            &[PathBuf::from("/Users/example/dev/project-other")]
        ));
    }
}
