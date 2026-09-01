//! Shared Git-project activity and ownership checks for project cleanup ops.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::safety::is_git_metadata_name;

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

pub(crate) fn repo_last_commit(root: &Path) -> Result<String> {
    repo_last_commit_with(root, "git")
}

pub(crate) fn repo_last_commit_with(root: &Path, git: &str) -> Result<String> {
    if !has_git_marker(root)? {
        anyhow::bail!("not a Git repository: {}", root.display());
    }
    // Neutralize repository-controlled config while inspecting an untrusted
    // clone, and ambient repository-selection variables that would make git
    // answer for a different repo than the one that owns the deletion target.
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
            "--no-optional-locks",
            "log",
            "-1",
            "--format=%cs",
        ])
        .output()
        .with_context(|| format!("cannot inspect Git activity for {}", root.display()))?;
    if !output.status.success() {
        anyhow::bail!("Git activity check failed for {}", root.display());
    }
    let date = String::from_utf8(output.stdout).context("Git returned a non-UTF-8 date")?;
    let date = date.trim();
    if date.len() != 10
        || !date.chars().enumerate().all(|(index, character)| {
            if index == 4 || index == 7 {
                character == '-'
            } else {
                character.is_ascii_digit()
            }
        })
    {
        anyhow::bail!("Git returned an invalid commit date for {}", root.display());
    }
    Ok(date.to_string())
}

pub(crate) fn iso_days_ago(days: u32) -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    iso_from_epoch_days(seconds.saturating_sub(u64::from(days) * 86_400) / 86_400)
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
    process_cwds
        .iter()
        .any(|cwd| cwd == repo || cwd.starts_with(repo))
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
        assert!(repo_last_commit_with(&base, "/usr/bin/false").is_err());
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
