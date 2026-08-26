//! Shared Git-project activity and ownership checks for project cleanup ops.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn owning_repo(path: &Path) -> Option<PathBuf> {
    let mut current = path.to_path_buf();
    while current.parent().is_some() {
        current.pop();
        if current.join(".git").exists() {
            return Some(current);
        }
    }
    None
}

pub(crate) fn repo_last_commit(root: &Path) -> Result<String> {
    repo_last_commit_with(root, "git")
}

pub(crate) fn repo_last_commit_with(root: &Path, git: &str) -> Result<String> {
    if !root.join(".git").exists() {
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
        assert_eq!(owning_repo(&project), Some(base.join("project")));
        assert_eq!(owning_repo(&base.join("orphan/node_modules")), None);
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
