//! Shared Git-project activity and ownership checks for project cleanup ops.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
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

/// Every directory under `root` holding a Git marker — a repository or a
/// linked worktree — not entered further. Folders the walk cannot read are
/// recorded in `unread`; only the root itself failing fails it.
pub(crate) fn repositories_under(
    root: &Path,
    unread: &mut Vec<UnreadFolder>,
) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut entries = walkdir::WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter();
    while let Some(result) = entries.next() {
        let entry = match result {
            Ok(entry) => entry,
            Err(error) if error.depth() > 0 => {
                unread.push(UnreadFolder::from_walk(&error));
                continue;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("cannot look for repositories under {}", root.display())
                });
            }
        };
        if !entry.file_type().is_dir() {
            continue;
        }
        if is_git_metadata_name(entry.file_name()) || super::is_node_modules_name(entry.file_name())
        {
            entries.skip_current_dir();
            continue;
        }
        match std::fs::symlink_metadata(entry.path().join(".git")) {
            Ok(_) => {
                found.push(entry.path().to_path_buf());
                entries.skip_current_dir();
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                unread.push(UnreadFolder::new(entry.path(), &error.into()));
                entries.skip_current_dir();
            }
        }
    }
    Ok(found)
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
fn repo_last_activity_with(root: &Path, git: &str) -> Result<String> {
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
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_COUNT")
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

/// `git worktree list --porcelain -z` for the repository holding `repo`,
/// through the hardened invocation. Every path it lists is the repository's
/// own data, not a fact about the disk.
pub(crate) fn worktree_listing(repo: &Path) -> Result<Vec<u8>> {
    let mut command = hardened_git(repo, "git");
    command.args(["worktree", "list", "--porcelain", "-z"]);
    hardened_output(command, || {
        format!("cannot list the worktrees of {}", repo.display())
    })
}

/// Number of dirty paths (staged, unstaged, or untracked) in `worktree`.
///
/// `git status` hashes a modified file through any content filter an
/// attribute selects and the configuration defines, and the repository's
/// configuration and `info/attributes` can change while devtrim reads them, so
/// no list of keys read beforehand can neutralize them all. The worktree side
/// therefore runs through [`isolated_git_dir`], a Git directory devtrim owns
/// whose only configuration is the one it wrote: Git reads the repository's
/// index (`GIT_INDEX_FILE`) and its `info/exclude` as data, never its config or
/// attributes, so no filter or driver a repository names can be selected. That
/// directory's HEAD is unborn, so the staged side comes from the repository
/// itself through `diff-index --cached`, which compares the index with HEAD's
/// tree and never converts file content. Both run with `--no-optional-locks`
/// through `hardened_git`, so the index on disk is never refreshed, and skip
/// submodules.
pub(crate) fn worktree_dirty_count(worktree: &Path) -> Result<usize> {
    use std::ffi::{OsStr, OsString};

    if !has_git_marker(worktree)? {
        anyhow::bail!("not a Git worktree: {}", worktree.display());
    }
    let dot_git = worktree.join(".git");
    let dot_git_metadata = std::fs::symlink_metadata(&dot_git)
        .with_context(|| format!("cannot inspect Git marker {}", dot_git.display()))?;
    if !dot_git_metadata.is_dir() && !dot_git_metadata.is_file() {
        anyhow::bail!("refusing non-regular Git marker {}", dot_git.display());
    }
    let pinned = |command: &mut Command| {
        command
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .arg("--git-dir")
            .arg(&dot_git)
            .arg("--work-tree")
            .arg(worktree);
    };
    // Paths and the object format only: `rev-parse` converts no content.
    let mut locate = hardened_git(worktree, "git");
    pinned(&mut locate);
    locate.args([
        "rev-parse",
        "--path-format=absolute",
        "--show-object-format",
        "--git-path",
        "index",
        "--git-path",
        "info/exclude",
    ]);
    let located = hardened_output(locate, || {
        format!("cannot locate the Git index of {}", worktree.display())
    })?;
    let located = located.strip_suffix(b"\n").unwrap_or(&located);
    let [format, index, exclude] = located.split(|byte| *byte == b'\n').collect::<Vec<_>>()[..]
    else {
        anyhow::bail!("unexpected Git paths for {}", worktree.display());
    };
    let sha256 = match format {
        b"sha1" => false,
        b"sha256" => true,
        _ => anyhow::bail!("unknown Git object format for {}", worktree.display()),
    };
    let (index, exclude) = (
        Path::new(OsStr::from_bytes(index)),
        Path::new(OsStr::from_bytes(exclude)),
    );
    if !index.is_absolute() || !exclude.is_absolute() {
        anyhow::bail!("Git named a relative index for {}", worktree.display());
    }
    let empty_tree = if sha256 {
        "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321"
    } else {
        "4b825dc642cb6eb9a060e54bf8d69288fbee4904"
    };

    let mut status = hardened_git(worktree, "git");
    status
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_INDEX_FILE", index)
        .arg("--git-dir")
        .arg(isolated_git_dir(&std::env::temp_dir(), sha256)?)
        .arg("--work-tree")
        .arg(worktree)
        .arg("--attr-source")
        .arg(empty_tree)
        .arg("-c")
        .arg("core.attributesFile=/dev/null")
        .arg("-c")
        .arg({
            let mut setting = OsString::from("core.excludesFile=");
            setting.push(exclude);
            setting
        })
        .args([
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=normal",
            "--ignore-submodules=all",
        ]);
    let listed = hardened_output(status, || {
        format!("cannot check working-tree status of {}", worktree.display())
    })?;
    let entries = parse_porcelain_status(&listed)
        .with_context(|| format!("invalid Git status output for {}", worktree.display()))?;

    // Against the isolated directory's unborn HEAD every tracked path reads
    // staged, so only the worktree column and untracked paths count from it.
    let mut dirty: std::collections::BTreeSet<&[u8]> = entries
        .iter()
        .filter(|entry| entry.worktree != b' ')
        .map(|entry| entry.path)
        .collect();
    let mut born = hardened_git(worktree, "git");
    pinned(&mut born);
    born.args(["rev-parse", "--quiet", "--verify", "HEAD^{commit}"]);
    let born = born
        .output_within(QUERY_TIMEOUT)
        .with_context(|| format!("cannot read HEAD of {}", worktree.display()))?;
    let staged;
    if born.status.success() {
        let mut command = hardened_git(worktree, "git");
        pinned(&mut command);
        command.args([
            "diff-index",
            "--cached",
            "--name-only",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules=all",
            "HEAD",
            "--",
        ]);
        staged = hardened_output(command, || {
            format!("cannot compare the index of {}", worktree.display())
        })?;
        dirty.extend(
            staged
                .split(|byte| *byte == 0)
                .filter(|path| !path.is_empty()),
        );
    } else if born.status.code() == Some(1) && born.stdout.is_empty() && unborn_branch(worktree) {
        // No commit yet: everything the index holds is staged. A HEAD that
        // fails to resolve for any other reason (a branch naming a missing
        // commit) is damage, and the count refuses.
        dirty.extend(
            entries
                .iter()
                .filter(|entry| entry.index != b' ' && entry.index != b'?')
                .map(|entry| entry.path),
        );
    } else {
        anyhow::bail!("cannot read HEAD of {}", worktree.display());
    }
    Ok(dirty.len())
}

/// A Git directory devtrim owns, holding only the configuration it writes
/// here, an unborn HEAD, and empty `refs` and `objects`: `git status` run
/// through it reads no repository's config, hooks or attributes. It lives in
/// the per-user temporary folder, is created once per object format and kept,
/// and is refused unless every part of it belongs to the current user, no one
/// else may write to it, and it holds exactly those four entries. Its folder
/// must belong to the current user with no group or other write, and every
/// folder above it to that user or root, writable by others only when sticky,
/// so no other account can create, rename or replace any part of it.
fn isolated_git_dir(base: &Path, sha256: bool) -> Result<PathBuf> {
    use std::io::Write as _;
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};

    let uid = rustix::process::getuid().as_raw();
    let base = base
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", base.display()))?;
    for (depth, folder) in base.ancestors().enumerate() {
        let metadata = std::fs::symlink_metadata(folder)
            .with_context(|| format!("cannot inspect {}", folder.display()))?;
        let others_write = metadata.mode() & 0o022 != 0;
        let trusted = if depth == 0 {
            metadata.is_dir() && metadata.uid() == uid && !others_write
        } else {
            metadata.is_dir()
                && (metadata.uid() == uid || metadata.uid() == 0)
                && (!others_write || metadata.mode() & 0o1000 != 0)
        };
        if !trusted {
            anyhow::bail!(
                "refusing Git probe parent {}: another account could change it",
                folder.display()
            );
        }
    }
    let format = if sha256 { "sha256" } else { "sha1" };
    let dir = base.join(format!("devtrim-git-probe-{uid}-{format}"));
    let config: &[u8] = if sha256 {
        b"[core]\n\trepositoryformatversion = 1\n\tbare = false\n[extensions]\n\tobjectformat = sha256\n"
    } else {
        b"[core]\n\trepositoryformatversion = 0\n\tbare = false\n"
    };
    let owned = |path: &Path, directory: bool| -> Result<()> {
        let metadata = std::fs::symlink_metadata(path)
            .with_context(|| format!("cannot inspect {}", path.display()))?;
        let kind = if directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        };
        if !kind || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
            anyhow::bail!("refusing Git probe directory part {}", path.display());
        }
        Ok(())
    };
    for part in [dir.clone(), dir.join("refs"), dir.join("objects")] {
        match std::fs::DirBuilder::new().mode(0o700).create(&part) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| format!("cannot create {}", part.display()));
            }
        }
        owned(&part, true)?;
    }
    for (leaf, expected) in [
        ("HEAD", &b"ref: refs/heads/devtrim-unborn\n"[..]),
        ("config", config),
    ] {
        let path = dir.join(leaf);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc_nofollow())
            .open(&path)
        {
            Ok(mut file) => file
                .write_all(expected)
                .with_context(|| format!("cannot write {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| format!("cannot create {}", path.display()));
            }
        }
        owned(&path, false)?;
        let found =
            std::fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
        if found != expected {
            anyhow::bail!(
                "refusing Git probe file {} with unexpected content",
                path.display()
            );
        }
    }
    // Git would read attributes and excludes from `info`, packed references
    // and alternates from their own files: anything devtrim did not write.
    let entries = |folder: &Path| -> Result<Vec<std::ffi::OsString>> {
        let mut names = std::fs::read_dir(folder)
            .with_context(|| format!("cannot list {}", folder.display()))?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<Vec<_>>>()
            .with_context(|| format!("cannot list {}", folder.display()))?;
        names.sort();
        Ok(names)
    };
    if entries(&dir)? != ["HEAD", "config", "objects", "refs"]
        || !entries(&dir.join("objects"))?.is_empty()
        || !entries(&dir.join("refs"))?.is_empty()
    {
        anyhow::bail!(
            "refusing Git probe directory {} holding entries devtrim did not write",
            dir.display()
        );
    }
    // Mode bits are not the whole story on macOS: an ACL `allow` entry can let
    // another account write to or replace a folder its mode denies. `ls -e`
    // lists every entry; a `deny` entry, such as the home folder's own
    // `group:everyone deny delete`, only narrows access and is accepted.
    let mut acl = Command::new("/bin/ls");
    acl.arg("-lde").arg("--").args(base.ancestors());
    for part in ["HEAD", "config", "objects", "refs"] {
        acl.arg(dir.join(part));
    }
    acl.arg(&dir);
    let listed = acl
        .output_within(QUERY_TIMEOUT)
        .with_context(|| format!("cannot read the ACLs above {}", dir.display()))?;
    if !listed.status.success() {
        anyhow::bail!("cannot read the ACLs above {}", dir.display());
    }
    if listed.stdout.split(|byte| *byte == b'\n').any(acl_allows) {
        anyhow::bail!(
            "refusing Git probe directory {}: an ACL lets another account change it",
            dir.display()
        );
    }
    Ok(dir)
}

/// Whether one `ls -le` line is an ACL entry that grants access: an index,
/// `: `, then `allow` among its words. Any other line is a file's own line.
fn acl_allows(line: &[u8]) -> bool {
    let entry = line.trim_ascii_start();
    let digits = entry
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    digits > 0
        && entry[digits..].starts_with(b": ")
        && entry
            .split(|byte| *byte == b' ')
            .any(|word| word == b"allow")
}

/// `O_NOFOLLOW` for [`std::os::unix::fs::OpenOptionsExt::custom_flags`].
fn libc_nofollow() -> i32 {
    rustix::fs::OFlags::NOFOLLOW.bits() as i32
}

/// One `git status --porcelain=v1 -z` record: its two status columns and path.
struct StatusEntry<'a> {
    index: u8,
    worktree: u8,
    path: &'a [u8],
}

/// Parses NUL-delimited `git status --porcelain=v1 -z` records; a rename or
/// copy carries its original path as a second field, which is consumed.
fn parse_porcelain_status(output: &[u8]) -> Result<Vec<StatusEntry<'_>>> {
    if output.is_empty() {
        return Ok(Vec::new());
    }
    let Some(records) = output.strip_suffix(b"\0") else {
        anyhow::bail!("porcelain status output did not end with a NUL");
    };
    let mut entries = Vec::new();
    let mut fields = records.split(|byte| *byte == 0);
    while let Some(entry) = fields.next() {
        if entry.len() < 4 || entry[2] != b' ' {
            anyhow::bail!("malformed porcelain status entry");
        }
        entries.push(StatusEntry {
            index: entry[0],
            worktree: entry[1],
            path: &entry[3..],
        });
        if matches!(entry[0], b'R' | b'C') || matches!(entry[1], b'R' | b'C') {
            let Some(original) = fields.next() else {
                anyhow::bail!("truncated rename or copy entry in porcelain status");
            };
            if original.is_empty() {
                anyhow::bail!("empty original path in porcelain status");
            }
        }
    }
    Ok(entries)
}

/// The absolute common directory (`.git` of the main worktree) of the
/// repository holding `repo`.
pub(crate) fn git_common_dir(repo: &Path) -> Result<PathBuf> {
    let mut command = hardened_git(repo, "git");
    command.args(["rev-parse", "--path-format=absolute", "--git-common-dir"]);
    let mut listed = hardened_output(command, || {
        format!("cannot find the Git directory of {}", repo.display())
    })?;
    if listed.last() == Some(&b'\n') {
        listed.pop();
    }
    if !listed.starts_with(b"/") {
        anyhow::bail!(
            "Git named no absolute common directory for {}",
            repo.display()
        );
    }
    Ok(PathBuf::from(std::ffi::OsString::from_vec(listed)))
}

/// Whether Git ignores `relative` in the repository at `repo`
/// (`check-ignore` exits 0 for ignored, 1 for not ignored; anything else is a
/// failure, never an answer).
pub(crate) fn ignores_path(repo: &Path, relative: &str) -> Result<bool> {
    let mut command = hardened_git(repo, "git");
    command.args(["check-ignore", "-q", "--", relative]);
    let output = command
        .output_within(QUERY_TIMEOUT)
        .with_context(|| format!("cannot run Git in {}", repo.display()))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let reason = stderr.lines().map(str::trim).find(|line| !line.is_empty());
            anyhow::bail!(
                "cannot tell whether Git ignores {relative} in {}{}",
                repo.display(),
                reason
                    .map(|reason| format!(": {reason}"))
                    .unwrap_or_default()
            )
        }
    }
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
pub(crate) const MAX_GIT_POINTER_BYTES: u64 = 4096;

/// Reads a Git pointer file (`.git` or `commondir`) without following symlinks
/// or blocking on non-regular files, checking the already-open descriptor and
/// refusing files larger than `MAX_GIT_POINTER_BYTES`. Returns `Ok(None)` when
/// the path is absent, a symlink, or not a regular file.
pub(crate) fn read_bounded_pointer(path: &Path) -> Result<Option<Vec<u8>>> {
    use rustix::fs::{Mode, OFlags, open};

    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let descriptor = match open(path, flags, Mode::empty()) {
        Ok(descriptor) => descriptor,
        Err(rustix::io::Errno::NOENT | rustix::io::Errno::NOTDIR | rustix::io::Errno::LOOP) => {
            return Ok(None);
        }
        Err(errno) => {
            return Err(std::io::Error::from(errno))
                .with_context(|| format!("cannot read {}", path.display()));
        }
    };
    read_bounded_pointer_file(std::fs::File::from(descriptor), path)
}

/// Same as `read_bounded_pointer`, opened relative to an already-open directory
/// descriptor via `openat` so ancestor components cannot be swapped between
/// containment validation and the read.
pub(crate) fn read_bounded_pointer_at(
    directory: &std::fs::File,
    name: &str,
    display_path: &Path,
) -> Result<Option<Vec<u8>>> {
    use rustix::fs::{Mode, OFlags, openat};

    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let descriptor = match openat(directory, name, flags, Mode::empty()) {
        Ok(descriptor) => descriptor,
        Err(rustix::io::Errno::NOENT | rustix::io::Errno::NOTDIR | rustix::io::Errno::LOOP) => {
            return Ok(None);
        }
        Err(errno) => {
            return Err(std::io::Error::from(errno))
                .with_context(|| format!("cannot read {}", display_path.display()));
        }
    };
    read_bounded_pointer_file(std::fs::File::from(descriptor), display_path)
}

fn read_bounded_pointer_file(file: std::fs::File, display_path: &Path) -> Result<Option<Vec<u8>>> {
    use std::io::Read as _;

    let metadata = file
        .metadata()
        .with_context(|| format!("cannot inspect {}", display_path.display()))?;
    if !metadata.is_file() {
        return Ok(None);
    }
    let mut buffer = Vec::new();
    file.take(MAX_GIT_POINTER_BYTES + 1)
        .read_to_end(&mut buffer)
        .with_context(|| format!("cannot read {}", display_path.display()))?;
    if buffer.len() as u64 > MAX_GIT_POINTER_BYTES {
        anyhow::bail!(
            "Git pointer {} exceeds {MAX_GIT_POINTER_BYTES} bytes",
            display_path.display()
        );
    }
    Ok(Some(buffer))
}

/// refuse it anyway, because its activity cannot be read. Any other shape is
/// not an orphan and goes to Git as before.
pub(crate) fn orphaned_worktree(repo: &Path) -> Result<bool> {
    let marker = repo.join(".git");
    // Anything Git would reject is not an orphan: it goes to Git, which
    // refuses it.
    let Some(bytes) = read_bounded_pointer(&marker)? else {
        return Ok(false);
    };
    let Some(target) = gitfile_target(&bytes) else {
        return Ok(false);
    };
    match std::fs::symlink_metadata(repo.join(target)) {
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error)
            .with_context(|| format!("cannot inspect the gitdir named by {}", marker.display())),
    }
}

/// The path a `.git` file names, read as Git reads it (git v2.51.0
/// `setup.c` `read_gitfile_gently`): the exact prefix `gitdir: `, then every
/// byte but trailing CR and LF, so a path holding a newline or ending in a
/// space is read whole. `None` when the prefix is missing or nothing follows.
/// A relative path is relative to the folder holding the `.git` file.
pub(crate) fn gitfile_target(bytes: &[u8]) -> Option<&std::ffi::OsStr> {
    let mut target = bytes.strip_prefix(b"gitdir: ")?;
    while let [rest @ .., b'\n' | b'\r'] = target {
        target = rest;
    }
    (!target.is_empty()).then(|| std::ffi::OsStr::from_bytes(target))
}

/// Whether `repo` is a new repository: on an unborn branch (Git glossary:
/// HEAD names a branch that has no commit yet) and holding no commit object
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
/// no history. A branch turned into a dangling symbolic reference, a file
/// holding an object ID where the loose-ref folder was, and an orphan
/// checkout beside other branches all keep their commits and their error. Three more answers keep a damaged repository an error
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
pub(crate) fn error_finding(label: String, message: String) -> Finding {
    Finding::new(label, None, 0, &message, 5, Action::Info).with_scan_error(message)
}

/// Whether a build process works in `repo`: its working directory is `repo` or
/// lies below it, or `repo` is a submodule and the build works anywhere in its
/// superproject outside the repositories nested there, since the
/// superproject's build reads the installs of the submodules it records.
/// Spelling alone misses one folder reached two ways — `lsof` names a working
/// directory its own way, through the `/System/Volumes/Data` firmlink or in
/// another case than the scan walked — so folders are also compared by device
/// and inode. With no build process there is nothing to ask; a repository or
/// superproject that cannot be read is an error for that repository, never a
/// busy build.
pub(crate) fn repo_has_active_build(
    repo: &Path,
    process_cwds: &[PathBuf],
    home: &Path,
) -> Result<bool> {
    use std::os::unix::fs::MetadataExt as _;

    if process_cwds.is_empty() {
        return Ok(false);
    }
    let identity_of = |path: &Path| {
        std::fs::metadata(path)
            .ok()
            .map(|metadata| (metadata.dev(), metadata.ino()))
    };
    let read_identity = |path: &Path| {
        identity_of(path).with_context(|| {
            format!(
                "cannot read {} to compare it with build processes",
                path.display()
            )
        })
    };
    if process_cwds.iter().any(|cwd| cwd.starts_with(repo)) {
        return Ok(true);
    }
    let identity = read_identity(repo)?;
    if process_cwds.iter().any(|cwd| {
        cwd.ancestors()
            .any(|folder| identity_of(folder) == Some(identity))
    }) {
        return Ok(true);
    }
    // A build works on a superproject when its working directory lies inside
    // it, reached by spelling or identity, and outside every repository nested
    // there: a worktree kept in an ignored folder is its own project. A folder
    // whose Git marker cannot be read counts as no repository, so the build
    // still counts.
    let builds_in = |superproject: &Path| -> Result<bool> {
        let identity = read_identity(superproject)?;
        Ok(process_cwds.iter().any(|cwd| {
            for folder in cwd.ancestors() {
                if folder == superproject || identity_of(folder) == Some(identity) {
                    return true;
                }
                if matches!(has_git_marker(folder), Ok(true)) {
                    return false;
                }
            }
            false
        }))
    };
    // Up through each superproject that records the repository below it as a
    // submodule, to the first that does not; a build working on any of them
    // reads what that superproject records.
    let mut submodule = repo.to_path_buf();
    loop {
        let Some(superproject) = owning_repo(&submodule, home)? else {
            return Ok(false);
        };
        if !records_submodule(&superproject, &submodule)? {
            return Ok(false);
        }
        if builds_in(&superproject)? {
            return Ok(true);
        }
        submodule = superproject;
    }
}

/// Whether `superproject`'s index records `submodule`, a folder inside it, as
/// a submodule: a gitlink (mode `160000`) at its path. A nested repository it
/// does not record — a worktree kept in an ignored folder, a clone inside a
/// checkout — is its own project, which a build in the outer one does not read.
fn records_submodule(superproject: &Path, submodule: &Path) -> Result<bool> {
    let relative = submodule.strip_prefix(superproject).with_context(|| {
        format!(
            "{} is not inside {}",
            submodule.display(),
            superproject.display()
        )
    })?;
    let mut command = hardened_git(superproject, "git");
    command
        .args(["--literal-pathspecs", "ls-files", "--stage", "-z", "--"])
        .arg(relative);
    let listed = hardened_output(command, || {
        format!("Git submodule check failed for {}", superproject.display())
    })?;
    Ok(listed
        .split(|byte| *byte == 0)
        .any(|entry| entry.starts_with(b"160000 ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("devtrim-{name}-{}", std::process::id()));
        crate::ops::remove_test_path(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    /// Removes every loose object, as a damaged or emptied store would be.
    fn empty_object_store(repo: &Path) {
        for entry in std::fs::read_dir(repo.join(".git/objects")).unwrap() {
            let entry = entry.unwrap();
            if entry.file_name().len() == 2 {
                std::fs::remove_dir_all(entry.path()).unwrap();
            }
        }
    }

    /// A new repository (Git glossary: HEAD names a branch that has no commit
    /// yet — `git init` before the first commit) is a state Git defines, and
    /// the activity query fails there. Only that state is recognized: an
    /// orphan checkout beside other branches, a HEAD naming a commit whose
    /// object is gone and a detached HEAD all stay what they are, so their
    /// failures still refuse.
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

        // `git add` before the first commit stores blobs, not commits.
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

    #[test]
    fn iso_dates_are_correct_and_ordered() {
        assert_eq!(iso_from_epoch_days(0), "1970-01-01");
        assert_eq!(iso_from_epoch_days(19_723), "2024-01-01");
        assert!(iso_days_ago(30) < iso_days_ago(1));
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
    fn porcelain_status_counts_renames_and_copies_once_and_refuses_truncation() {
        assert!(parse_porcelain_status(b"").unwrap().is_empty());
        let entries = parse_porcelain_status(
            b" M src/lib.rs\0?? notes.txt\0R  new name.rs\0old name.rs\0CM copy\nfile.rs\0orig\nfile.rs\0",
        )
        .unwrap();
        let read: Vec<_> = entries
            .iter()
            .map(|entry| (entry.index, entry.worktree, entry.path))
            .collect();
        assert_eq!(
            read,
            [
                (b' ', b'M', &b"src/lib.rs"[..]),
                (b'?', b'?', &b"notes.txt"[..]),
                (b'R', b' ', &b"new name.rs"[..]),
                (b'C', b'M', &b"copy\nfile.rs"[..]),
            ]
        );
        for malformed in [
            &b" M missing-nul.rs"[..],
            &b"M\0"[..],
            &b"MMno-space.rs\0"[..],
            &b"R  only-destination.rs\0"[..],
            &b"C  copy.rs\0\0"[..],
        ] {
            assert!(
                parse_porcelain_status(malformed).is_err(),
                "accepted malformed status: {malformed:?}"
            );
        }
    }

    #[test]
    fn the_isolated_git_dir_refuses_anything_it_did_not_write_alone() {
        use std::os::unix::fs::PermissionsExt as _;
        let fixture = crate::ops::TestFixture::new("isolated-git-dir");
        let base = fixture.path();
        std::fs::create_dir_all(base).unwrap();
        std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o700)).unwrap();
        let dir = isolated_git_dir(base, false).unwrap();
        assert_eq!(
            isolated_git_dir(base, false).unwrap(),
            dir,
            "reused as written"
        );
        assert_ne!(
            isolated_git_dir(base, true).unwrap(),
            dir,
            "one per object format"
        );

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o770)).unwrap();
        assert!(isolated_git_dir(base, false).is_err(), "group-writable");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();

        std::fs::create_dir(dir.join("info")).unwrap();
        assert!(isolated_git_dir(base, false).is_err(), "an info folder");
        crate::ops::remove_test_path(dir.join("info"));
        std::fs::write(dir.join("packed-refs"), "").unwrap();
        assert!(isolated_git_dir(base, false).is_err(), "a packed-refs file");
        crate::ops::remove_test_path(dir.join("packed-refs"));
        std::fs::create_dir(dir.join("objects/info")).unwrap();
        assert!(
            isolated_git_dir(base, false).is_err(),
            "an objects/info folder"
        );
        crate::ops::remove_test_path(dir.join("objects/info"));
        assert_eq!(isolated_git_dir(base, false).unwrap(), dir, "restored");

        std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            isolated_git_dir(base, false).is_err(),
            "PV git/probe-parent: a world-writable parent"
        );
        std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o1777)).unwrap();
        assert!(
            isolated_git_dir(base, false).is_err(),
            "a sticky parent is still shared: the probe's own folder must be private"
        );
        std::fs::set_permissions(base, std::fs::Permissions::from_mode(0o700)).unwrap();
        let shared = base.join("shared");
        let private = shared.join("private");
        std::fs::create_dir_all(&private).unwrap();
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            isolated_git_dir(&private, false).is_err(),
            "a folder above it others may rename entries in"
        );
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o1777)).unwrap();
        assert!(
            isolated_git_dir(&private, false).is_ok(),
            "a sticky folder above it"
        );
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o700)).unwrap();

        let chmod = |entry: &str, path: &Path| {
            let status = Command::new("/bin/chmod")
                .arg(entry)
                .arg(path)
                .status()
                .unwrap();
            assert!(status.success(), "chmod {entry} failed");
        };
        let acl = |entry: &str, path: &Path| {
            let status = Command::new("/bin/chmod")
                .args(["+a", entry])
                .arg(path)
                .status()
                .unwrap();
            assert!(status.success(), "chmod +a {entry} failed");
        };
        // A `deny delete` entry left behind by a failing assertion would make
        // the fixture undeletable, so the entries are cleared as it unwinds.
        struct ClearAcls(Vec<PathBuf>);
        impl Drop for ClearAcls {
            fn drop(&mut self) {
                for path in &self.0 {
                    let _ = Command::new("/bin/chmod").arg("-N").arg(path).status();
                }
            }
        }
        let _clear = ClearAcls(vec![shared.clone()]);
        acl("group:everyone deny delete", &shared);
        assert!(
            isolated_git_dir(&private, false).is_ok(),
            "a deny entry only narrows access"
        );
        acl(
            "group:everyone allow add_file,add_subdirectory,delete_child",
            &shared,
        );
        assert!(
            isolated_git_dir(&private, false).is_err(),
            "PV git/probe-acl: an ACL above it lets another account change it"
        );
        chmod("-N", &shared);
        let probe = isolated_git_dir(&private, false).unwrap();
        acl("group:everyone allow write", &probe.join("config"));
        assert!(
            isolated_git_dir(&private, false).is_err(),
            "an ACL on a part of the probe itself"
        );
        chmod("-N", &probe.join("config"));
        assert!(isolated_git_dir(&private, false).is_ok(), "ACLs removed");

        std::fs::write(dir.join("config"), "[filter \"x\"]\n\tclean = touch\n").unwrap();
        assert!(
            isolated_git_dir(base, false).is_err(),
            "a config it did not write"
        );
    }

    #[test]
    fn worktree_dirty_count_counts_each_path_once_and_refuses_a_damaged_head() {
        let fixture = crate::ops::TestFixture::new("dirty-count-heads");
        let repo = fixture.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
        git(&["add", "staged.txt"]);
        std::fs::write(repo.join("untracked.txt"), "new\n").unwrap();
        assert_eq!(
            worktree_dirty_count(&repo).unwrap(),
            2,
            "no commit yet: the staged file and the untracked one"
        );

        git(&["commit", "-q", "-m", "initial"]);
        std::fs::write(repo.join("staged.txt"), "changed\n").unwrap();
        git(&["add", "staged.txt"]);
        std::fs::write(repo.join("staged.txt"), "changed again\n").unwrap();
        assert_eq!(
            worktree_dirty_count(&repo).unwrap(),
            2,
            "a path staged and changed again counts once, beside the untracked one"
        );

        let missing = "0123456789abcdef0123456789abcdef01234567\n";
        std::fs::write(repo.join(".git/refs/heads/main"), missing).unwrap();
        assert!(
            worktree_dirty_count(&repo).is_err(),
            "PV git/damaged-head: a branch naming a missing commit is damage, not a repository with no commit"
        );
    }

    #[test]
    fn worktree_dirty_count_never_runs_repository_configured_clean_filters() {
        let fixture = crate::ops::TestFixture::new("dirty-clean-filter");
        let base = fixture.path();
        let repo = base.join("repo");
        let marker = base.join("filter-ran");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "test@example.com")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "test@example.com")
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?} failed");
        };
        git(&["init", "-q"]);
        std::fs::write(repo.join(".gitattributes"), "* filter=hostile=eq\n").unwrap();
        std::fs::write(repo.join("tracked.txt"), "before\n").unwrap();
        git(&["add", ".gitattributes", "tracked.txt"]);
        git(&["commit", "-q", "-m", "initial"]);
        let filter_cmd = format!("touch '{}' && cat", marker.display());
        git(&["config", "filter.hostile=eq.clean", &filter_cmd]);
        git(&["config", "filter.hostile=eq.required", "true"]);
        std::fs::write(repo.join("tracked.txt"), "after\n").unwrap();

        let dirty = worktree_dirty_count(&repo).unwrap();
        assert_eq!(dirty, 1);
        assert!(
            !marker.exists(),
            "worktree_dirty_count executed repository-configured filter.clean"
        );

        // A hostile core.worktree pointing to a dirty external folder must be
        // ignored so status stays bound to the validated worktree directory.
        let outside = base.join("outside-dirty");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("extra1.txt"), "1").unwrap();
        std::fs::write(outside.join("extra2.txt"), "2").unwrap();
        std::fs::write(repo.join("tracked.txt"), "before\n").unwrap();
        git(&["config", "core.worktree", outside.to_str().unwrap()]);
        assert_eq!(
            worktree_dirty_count(&repo).unwrap(),
            0,
            "worktree_dirty_count followed hostile core.worktree outside validated directory"
        );
    }
}
