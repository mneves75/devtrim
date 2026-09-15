//! Shared Git-project activity and ownership checks for project cleanup ops.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

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

pub(crate) fn owning_repo(path: &Path) -> Result<Option<PathBuf>> {
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
            "old fixture",
        ])
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .current_dir(repo)
        .status()
        .with_context(|| format!("cannot commit old Git fixture {}", repo.display()))?;
    if !committed.success() {
        anyhow::bail!("git commit failed for old fixture {}", repo.display());
    }
    Ok(())
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
    let commit = iso_date(&hardened_git_log(root, git, &["--format=%cs"])?, root)?;
    let reflog = hardened_git_log(root, git, &["-g", "--date=format:%Y-%m-%d", "--format=%gd"])?;
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

/// One `git log -1` against a repository that may be hostile.
fn hardened_git_log(root: &Path, git: &str, format: &[&str]) -> Result<String> {
    // Neutralize repository-controlled config while inspecting an untrusted
    // clone, and ambient repository-selection variables that would make git
    // answer for a different repo than the one that owns the deletion target.
    // Git cannot ignore repository config wholesale, so every path by which a
    // date-only `log` spawns a configured program is closed explicitly:
    // signature display runs `gpg.program`, and a promisor remote lazily
    // fetches a missing commit through its configured `uploadpack`. A git too
    // old to know `--no-lazy-fetch` fails, which refuses rather than trusts.
    let output = Command::new(git)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env_remove("GIT_DISCOVERY_ACROSS_FILESYSTEM")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "log.showSignature=false",
            "--no-optional-locks",
            "--no-lazy-fetch",
            "--no-pager",
            "log",
            "--no-show-signature",
            "-1",
        ])
        .args(format)
        .output()
        .with_context(|| format!("cannot inspect Git activity for {}", root.display()))?;
    if !output.status.success() {
        anyhow::bail!("Git activity check failed for {}", root.display());
    }
    let text = String::from_utf8(output.stdout).context("Git returned a non-UTF-8 date")?;
    Ok(text.trim().to_string())
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

    #[test]
    fn iso_dates_are_correct_and_ordered() {
        assert_eq!(iso_from_epoch_days(0), "1970-01-01");
        assert_eq!(iso_from_epoch_days(19_723), "2024-01-01");
        assert!(iso_days_ago(30) < iso_days_ago(1));
    }

    #[test]
    fn finds_owning_repo_and_handles_orphans() {
        let base = temp("owner");
        let project = base.join("project/sub/node_modules");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(base.join("project/.git")).unwrap();
        assert_eq!(owning_repo(&project).unwrap(), Some(base.join("project")));
        assert_eq!(
            owning_repo(&base.join("orphan/node_modules")).unwrap(),
            None
        );
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
