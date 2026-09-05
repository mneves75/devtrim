//! Regenerable project artifacts in conclusively stale Git repositories.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::project::{
    ScanObservations, has_git_marker, is_directory_if_present, iso_days_ago, normalized_roots,
    owning_repo, repo_has_active_build, repo_last_commit,
};
use super::{
    Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, dir_size,
    has_node_modules_ancestor, is_node_modules_name, removal_note,
};
use crate::safety::{Ctx, build_process_cwds, escalate, is_git_metadata_name};

const CACHEDIR_SIGNATURE: &[u8; 43] = b"Signature: 8a477f597d28d172789f06886806bc55";
const EXCLUDED_NAMES: &[&str] = &[
    "build",
    "dist",
    "out",
    "vendor",
    "bin",
    "obj",
    "coverage",
    "node_modules",
    "DerivedData",
];

pub struct Artifacts;

#[derive(Debug)]
struct ArtifactCandidate {
    path: PathBuf,
    evidence: ArtifactEvidence,
}

#[derive(Debug)]
struct ArtifactEvidence {
    label: String,
    corroboration: String,
}

impl Op for Artifacts {
    fn name(&self) -> &'static str {
        "artifacts"
    }

    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>> {
        self.scan_with_observations(ctx, &mut ScanObservations::default())
    }

    fn scan_with_observations(
        &self,
        ctx: &Ctx,
        observations: &mut ScanObservations,
    ) -> Result<Vec<Finding>> {
        observations.process_cwds()?;
        let cutoff = iso_days_ago(ctx.active_days);
        let mut groups: BTreeMap<PathBuf, Vec<ArtifactCandidate>> = BTreeMap::new();
        for root in normalized_roots(&ctx.roots) {
            if !is_directory_if_present(root)? {
                continue;
            }
            for candidate in find_artifacts(root)? {
                if let Some(owner) = owning_repo(&candidate.path)? {
                    groups.entry(owner).or_default().push(candidate);
                }
            }
        }

        let mut findings = Vec::new();
        let mut active = 0usize;
        let mut build_active = 0usize;
        for (owner, candidates) in groups {
            if repo_has_active_build(&owner, observations.process_cwds()?) {
                build_active = build_active.saturating_add(candidates.len());
                continue;
            }
            let last_commit = observations.last_commit(&owner)?;
            if last_commit > cutoff.as_str() {
                active = active.saturating_add(candidates.len());
                continue;
            }
            for candidate in candidates {
                let size = dir_size(&candidate.path)?;
                findings.push(Finding::new(
                    candidate.evidence.label,
                    Some(candidate.path),
                    size,
                    format!(
                        "repo last committed {last_commit}; corroboration: {}",
                        candidate.evidence.corroboration
                    ),
                    escalate(5, size),
                    Action::Trash,
                ));
            }
        }
        if active > 0 && !ctx.json {
            ctx.diagnostic(
                "info",
                format!("skipping {active} artifact directories in active repos"),
            );
        }
        if build_active > 0 && !ctx.json {
            ctx.diagnostic(
                "info",
                format!(
                    "skipping {build_active} artifact directories because a build process is active"
                ),
            );
        }
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        self.apply_with_process_cwds(findings, ctx, build_process_cwds())
    }
}

impl Artifacts {
    #[cfg(test)]
    fn scan_with_process_cwds(&self, ctx: &Ctx, process_cwds: &[PathBuf]) -> Result<Vec<Finding>> {
        self.scan_with_observations(
            ctx,
            &mut ScanObservations::with_process_cwds(process_cwds.to_vec()),
        )
    }

    fn apply_with_process_cwds(
        &self,
        findings: &[Finding],
        ctx: &Ctx,
        process_cwds: Result<Vec<PathBuf>>,
    ) -> Result<ApplyOutcome> {
        let cutoff = iso_days_ago(ctx.active_days);
        let mut outcome = ApplyOutcome::new(self.name());
        let process_cwds = match process_cwds {
            Ok(process_cwds) => process_cwds,
            Err(error) => {
                outcome.fail(error.context("cannot verify build-process liveness"));
                return Ok(outcome);
            }
        };
        let mut ready = Vec::new();
        for finding in findings {
            if !matches!(finding.action, Action::Trash | Action::Shred) {
                continue;
            }
            let Some(path) = finding.target() else {
                outcome.fail(anyhow::anyhow!("artifact finding missing internal target"));
                return Ok(outcome);
            };
            match std::fs::symlink_metadata(path) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    outcome
                        .summary
                        .notes
                        .push(format!("skipped vanished {}", path.display()));
                    continue;
                }
                Err(error) => {
                    outcome.fail(
                        anyhow::Error::new(error)
                            .context(format!("cannot inspect artifact target {}", path.display())),
                    );
                    return Ok(outcome);
                }
            }
            let result = (|| -> Result<()> {
                if has_git_marker(path)? {
                    anyhow::bail!(
                        "target gained its own Git marker after preview; refusing {}",
                        path.display()
                    );
                }
                if has_node_modules_ancestor(path) {
                    anyhow::bail!(
                        "refusing artifact target under an excluded node_modules ancestor: {}",
                        path.display()
                    );
                }
                let owner = owning_repo(path)?.ok_or_else(|| {
                    anyhow::anyhow!("cannot prove Git owner for {}", path.display())
                })?;
                if artifact_evidence(path)?.is_none() {
                    anyhow::bail!(
                        "artifact corroboration changed after preview; refusing {}",
                        path.display()
                    );
                }
                if repo_has_active_build(&owner, &process_cwds) {
                    anyhow::bail!(
                        "build process active in {}; refusing {}",
                        owner.display(),
                        path.display()
                    );
                }
                let last_commit = repo_last_commit(&owner)?;
                if last_commit > cutoff {
                    anyhow::bail!(
                        "repo became active after preview; refusing {}",
                        path.display()
                    );
                }
                Ok(())
            })();
            if let Err(error) = result {
                outcome.fail(error);
                return Ok(outcome);
            }
            ready.push((finding, path));
        }

        for (finding, path) in ready {
            match apply_filesystem_finding(self.name(), finding, ctx) {
                Ok(()) => outcome.record(finding, removal_note(finding, path.display())),
                Err(error) => {
                    outcome.fail(error);
                    break;
                }
            }
        }
        Ok(outcome)
    }
}

fn find_artifacts(root: &Path) -> Result<Vec<ArtifactCandidate>> {
    let mut found = Vec::new();
    let mut entries = walkdir::WalkDir::new(root).follow_links(false).into_iter();
    while let Some(result) = entries.next() {
        let entry =
            result.with_context(|| format!("cannot scan artifacts under {}", root.display()))?;
        if !entry.file_type().is_dir() {
            continue;
        }
        if is_git_metadata_name(entry.file_name()) || is_node_modules_name(entry.file_name()) {
            entries.skip_current_dir();
            continue;
        }
        if let Some(evidence) = artifact_evidence(entry.path())? {
            if has_git_marker(entry.path())? {
                entries.skip_current_dir();
                continue;
            }
            found.push(ArtifactCandidate {
                path: entry.path().to_path_buf(),
                evidence,
            });
            entries.skip_current_dir();
        }
    }
    found.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(found)
}

fn artifact_evidence(path: &Path) -> Result<Option<ArtifactEvidence>> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("cannot inspect artifact target {}", path.display()));
        }
    };
    if !metadata.file_type().is_dir() {
        return Ok(None);
    }
    let name = path.file_name().and_then(|name| name.to_str());
    // macOS filesystems are commonly case-insensitive and case-preserving, so
    // `Build` or `DIST` must hit the ambiguous-name denylist exactly like
    // `build`, before any positive evidence (including CACHEDIR.TAG) is read.
    if name.is_some_and(|name| {
        EXCLUDED_NAMES
            .iter()
            .any(|excluded| name.eq_ignore_ascii_case(excluded))
    }) {
        return Ok(None);
    }

    let named: Result<Option<String>> = match name {
        Some("target") => sibling_evidence(path, &["Cargo.toml"]),
        Some(".venv" | "venv") => contained_evidence(path, "pyvenv.cfg"),
        Some(name @ ("__pycache__" | ".pytest_cache" | ".mypy_cache" | ".ruff_cache")) => {
            Ok(Some(format!("directory name {name}")))
        }
        Some(".tox") => sibling_evidence(path, &["tox.ini", "setup.cfg", "pyproject.toml"]),
        Some(".nox") => sibling_evidence(path, &["noxfile.py"]),
        Some(
            ".next" | ".nuxt" | ".turbo" | ".parcel-cache" | ".svelte-kit" | ".astro" | ".expo"
            | ".angular",
        ) => sibling_evidence(path, &["package.json"]),
        Some("Pods") => sibling_evidence(path, &["Podfile"]),
        Some(".gradle") => sibling_evidence(
            path,
            &[
                "settings.gradle",
                "settings.gradle.kts",
                "build.gradle",
                "build.gradle.kts",
            ],
        ),
        Some(".build") => sibling_evidence(path, &["Package.swift"]),
        Some(".dart_tool") => sibling_evidence(path, &["pubspec.yaml"]),
        Some(".zig-cache" | "zig-out") => sibling_evidence(path, &["build.zig"]),
        _ => Ok(None),
    };
    if let Some(corroboration) = named? {
        let name = name.unwrap_or("artifact");
        return Ok(Some(ArtifactEvidence {
            label: format!("stale {name} artifacts"),
            corroboration,
        }));
    }
    if cachedir_tag_matches(path)? {
        return Ok(Some(ArtifactEvidence {
            label: "stale CACHEDIR.TAG cache".into(),
            corroboration: "CACHEDIR.TAG signature".into(),
        }));
    }
    Ok(None)
}

fn sibling_evidence(path: &Path, filenames: &[&str]) -> Result<Option<String>> {
    let Some(parent) = path.parent() else {
        return Ok(None);
    };
    for filename in filenames {
        if regular_file(&parent.join(filename))? {
            return Ok(Some(format!("sibling {filename}")));
        }
    }
    Ok(None)
}

fn contained_evidence(path: &Path, filename: &str) -> Result<Option<String>> {
    Ok(regular_file(&path.join(filename))?.then(|| format!("contained {filename}")))
}

fn regular_file(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.file_type().is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error)
            .with_context(|| format!("cannot inspect artifact evidence {}", path.display())),
    }
}

fn cachedir_tag_matches(path: &Path) -> Result<bool> {
    let tag = path.join("CACHEDIR.TAG");
    if !regular_file(&tag)? {
        return Ok(false);
    }
    let mut file = std::fs::File::open(&tag)
        .with_context(|| format!("cannot read artifact marker {}", tag.display()))?;
    let mut prefix = [0u8; CACHEDIR_SIGNATURE.len()];
    match file.read_exact(&mut prefix) {
        Ok(()) => Ok(&prefix == CACHEDIR_SIGNATURE),
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(error) => {
            Err(error).with_context(|| format!("cannot read artifact marker {}", tag.display()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn temp(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("devtrim-artifacts-{name}-{}", std::process::id()));
        crate::ops::remove_test_path(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn context(home: PathBuf) -> Ctx {
        Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: vec![home.clone()],
            active_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home,
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        }
    }

    fn init_old_git_repo(repo: &Path) {
        std::fs::create_dir_all(repo).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .current_dir(repo)
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("git")
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
                .unwrap()
                .success()
        );
    }

    #[test]
    fn corroboration_matrix_is_fail_closed() {
        let root = temp("corroboration");
        std::fs::create_dir_all(root.join("missing-cargo/target")).unwrap();
        std::fs::create_dir_all(root.join("missing-pyvenv/.venv")).unwrap();

        let rust = root.join("rust");
        std::fs::create_dir_all(rust.join("target")).unwrap();
        std::fs::write(rust.join("Cargo.toml"), "[package]").unwrap();

        let python = root.join("python/.venv");
        std::fs::create_dir_all(&python).unwrap();
        std::fs::write(python.join("pyvenv.cfg"), "home = /usr/bin").unwrap();

        let tagged = root.join("tagged");
        std::fs::create_dir_all(&tagged).unwrap();
        std::fs::write(
            tagged.join("CACHEDIR.TAG"),
            [CACHEDIR_SIGNATURE.as_slice(), b"\nextra"].concat(),
        )
        .unwrap();

        let excluded_case = root.join("Build");
        std::fs::create_dir_all(&excluded_case).unwrap();
        std::fs::write(excluded_case.join("CACHEDIR.TAG"), CACHEDIR_SIGNATURE).unwrap();

        let wrong = root.join("wrong-tag");
        std::fs::create_dir_all(&wrong).unwrap();
        std::fs::write(wrong.join("CACHEDIR.TAG"), "Signature: wrong").unwrap();

        let linked = root.join("linked-tag");
        std::fs::create_dir_all(&linked).unwrap();
        let marker = root.join("real-marker");
        std::fs::write(&marker, CACHEDIR_SIGNATURE).unwrap();
        symlink(&marker, linked.join("CACHEDIR.TAG")).unwrap();

        let found = find_artifacts(&root).unwrap();
        let paths = found
            .iter()
            .map(|candidate| candidate.path.as_path())
            .collect::<Vec<_>>();
        assert!(paths.contains(&rust.join("target").as_path()));
        assert!(paths.contains(&python.as_path()));
        assert!(paths.contains(&tagged.as_path()));
        assert!(!paths.contains(&root.join("missing-cargo/target").as_path()));
        assert!(!paths.contains(&root.join("missing-pyvenv/.venv").as_path()));
        assert!(!paths.contains(&wrong.as_path()));
        assert!(!paths.contains(&excluded_case.as_path()));
        assert!(!paths.contains(&linked.as_path()));
        assert_eq!(
            found
                .iter()
                .find(|candidate| candidate.path == tagged)
                .unwrap()
                .evidence
                .label,
            "stale CACHEDIR.TAG cache"
        );
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn walker_prunes_git_node_modules_and_matched_artifacts() {
        let root = temp("walker");
        std::fs::create_dir_all(root.join(".GIT/hidden/target")).unwrap();
        std::fs::write(root.join(".GIT/hidden/Cargo.toml"), "[package]").unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg/target")).unwrap();
        std::fs::write(root.join("node_modules/pkg/Cargo.toml"), "[package]").unwrap();
        let case_variant = root.join("case-app/NODE_MODULES/pkg/target");
        std::fs::create_dir_all(&case_variant).unwrap();
        std::fs::write(
            root.join("case-app/NODE_MODULES/pkg/Cargo.toml"),
            "[package]",
        )
        .unwrap();
        let ordinary = root.join("node-modules/pkg/target");
        std::fs::create_dir_all(&ordinary).unwrap();
        std::fs::write(root.join("node-modules/pkg/Cargo.toml"), "[package]").unwrap();
        std::fs::create_dir_all(root.join(".next/nested/target")).unwrap();
        std::fs::write(root.join(".next/nested/Cargo.toml"), "[package]").unwrap();
        std::fs::create_dir_all(root.join("git/nested/target")).unwrap();
        std::fs::write(root.join("git/nested/Cargo.toml"), "[package]").unwrap();
        std::fs::write(root.join("package.json"), "{}").unwrap();

        let found = find_artifacts(&root).unwrap();

        let paths = found
            .iter()
            .map(|candidate| candidate.path.as_path())
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 3);
        assert!(paths.contains(&root.join(".next").as_path()));
        assert!(paths.contains(&root.join("git/nested/target").as_path()));
        assert!(paths.contains(&ordinary.as_path()));
        assert!(!paths.contains(&case_variant.as_path()));
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn apply_rejects_artifact_under_case_variant_node_modules_ancestor() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!(
                "devtrim-artifact-case-ancestor-{}",
                std::process::id()
            ));
        crate::ops::remove_test_path(&root);
        std::fs::create_dir_all(&root).unwrap();
        let home = root.canonicalize().unwrap();
        let repo = home.join("repo");
        init_old_git_repo(&repo);
        let target = repo.join("packages/NODE_MODULES/pkg/target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(
            target.parent().unwrap().join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\n",
        )
        .unwrap();
        let sentinel = target.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let finding = Finding::new(
            "forged target artifacts under dependency namespace",
            Some(target.clone()),
            4,
            "test",
            9,
            Action::Shred,
        );
        let ctx = context(home.clone());

        let outcome = Artifacts
            .apply_with_process_cwds(&[finding], &ctx, Ok(Vec::new()))
            .unwrap();

        assert_eq!(outcome.summary.items_touched, 0);
        assert!(
            outcome
                .errors
                .iter()
                .any(|error| error.contains("excluded node_modules ancestor")),
            "unexpected outcome: {outcome:?}"
        );
        assert!(sentinel.exists());
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn apply_refuses_removed_corroboration_and_orphans() {
        let home = temp("apply-refusal");
        let repo = home.join("repo");
        let target = repo.join("target");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[package]").unwrap();
        let sentinel = target.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let finding = Finding::new(
            "stale target artifacts",
            Some(target.clone()),
            4,
            "test",
            9,
            Action::Shred,
        );
        std::fs::remove_file(repo.join("Cargo.toml")).unwrap();
        let ctx = context(home.clone());

        let removed = Artifacts
            .apply_with_process_cwds(&[finding], &ctx, Ok(Vec::new()))
            .unwrap();
        assert_eq!(removed.summary.items_touched, 0);
        assert!(removed.errors[0].contains("corroboration changed"));
        assert!(sentinel.exists());

        let orphan = home.join("orphan/target");
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::write(home.join("orphan/Cargo.toml"), "[package]").unwrap();
        let orphan_sentinel = orphan.join("sentinel");
        std::fs::write(&orphan_sentinel, "keep").unwrap();
        let orphan_finding = Finding::new(
            "stale target artifacts",
            Some(orphan),
            4,
            "test",
            9,
            Action::Shred,
        );
        let refused = Artifacts
            .apply_with_process_cwds(&[orphan_finding], &ctx, Ok(Vec::new()))
            .unwrap();
        assert_eq!(refused.summary.items_touched, 0);
        assert!(refused.errors[0].contains("cannot prove Git owner"));
        assert!(orphan_sentinel.exists());
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn active_build_cwd_skips_repo_and_refuses_apply() {
        let home = temp("build-active");
        let repo = home.join("repo");
        let target = repo.join("target");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[package]").unwrap();
        let sentinel = target.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let ctx = context(home.clone());
        let process_cwds = vec![repo.join("src")];

        assert!(
            Artifacts
                .scan_with_process_cwds(&ctx, &process_cwds)
                .unwrap()
                .is_empty()
        );
        assert!(
            ctx.take_diagnostics()
                .iter()
                .any(|message| message.contains("build process is active"))
        );

        let finding = Finding::new(
            "stale target artifacts",
            Some(target),
            4,
            "test",
            9,
            Action::Shred,
        );
        let outcome = Artifacts
            .apply_with_process_cwds(&[finding], &ctx, Ok(process_cwds.clone()))
            .unwrap();
        assert_eq!(outcome.summary.items_touched, 0);
        assert!(outcome.errors[0].contains("build process active"));
        assert!(sentinel.exists());

        let mut json_ctx = context(home.clone());
        json_ctx.json = true;
        assert!(
            Artifacts
                .scan_with_process_cwds(&json_ctx, &process_cwds)
                .unwrap()
                .is_empty()
        );
        assert!(json_ctx.take_diagnostics().is_empty());
        crate::ops::remove_test_path(home);
    }
}
