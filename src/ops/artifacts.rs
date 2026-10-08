//! Regenerable project artifacts in conclusively stale Git repositories.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};

use super::project::{
    ScanObservations, TrackedIndex, UnreadFolder, activity_window, has_git_marker,
    is_directory_if_present, iso_days_ago, listed_repositories, normalized_roots,
    orphaned_worktree, owning_repo, repo_has_active_build, repo_last_activity, tracks_files_under,
    unborn_branch, unjudged_finding, unread_folders_finding,
};
use super::{
    Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, dir_size,
    has_node_modules_ancestor, is_node_modules_name, removal_note,
};
use crate::safety::{Ctx, build_process_cwds, escalate, is_git_metadata_name};

/// The fixed first line of a cache directory tag (<https://bford.info/cachedir/>).
pub(crate) const CACHEDIR_SIGNATURE: &[u8; 43] = b"Signature: 8a477f597d28d172789f06886806bc55";
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

    fn scans_roots(&self) -> bool {
        true
    }

    fn scan(&self, ctx: &Ctx, observations: &ScanObservations) -> Result<Vec<Finding>> {
        observations.process_cwds()?;
        let cutoff = iso_days_ago(ctx.active_days);
        let mut groups: BTreeMap<PathBuf, Vec<ArtifactCandidate>> = BTreeMap::new();
        let mut findings = Vec::new();
        let mut unread = Vec::new();
        for root in normalized_roots(&ctx.roots) {
            if !is_directory_if_present(root)? {
                continue;
            }
            for candidate in find_artifacts(root, &mut unread)? {
                match owning_repo(&candidate.path, &ctx.home) {
                    Ok(Some(owner)) => groups.entry(owner).or_default().push(candidate),
                    Ok(None) => {}
                    Err(error) => {
                        findings.push(unjudged_finding("artifacts", &candidate.path, &error));
                    }
                }
            }
        }
        if !unread.is_empty() {
            findings.push(unread_folders_finding("artifacts", &unread));
        }

        let mut active = 0usize;
        let mut build_active = 0usize;
        let mut busy = Vec::new();
        let mut tracked = 0usize;
        let mut keypairs = 0usize;
        let mut repositories = 0usize;
        let mut states = 0usize;
        let mut orphaned = 0usize;
        let mut orphans = Vec::new();
        let mut unborn = Vec::new();
        for (owner, candidates) in groups {
            // A repository whose checks fail offers nothing — not even a
            // finding judged before the failure — and reports the error; every
            // other repository is judged as usual.
            let mut judged = Vec::new();
            // Counted only once the whole repository has been judged, so a
            // repository that then fails is reported as an error, not a skip.
            let (mut tracked_here, mut keypairs_here, mut repositories_here, mut states_here) =
                (0usize, 0, 0, 0);
            let result = (|| -> Result<()> {
                if orphaned_worktree(&owner)? {
                    orphaned = orphaned.saturating_add(candidates.len());
                    orphans.push(owner.clone());
                    return Ok(());
                }
                if repo_has_active_build(&owner, observations.process_cwds()?) {
                    build_active = build_active.saturating_add(candidates.len());
                    busy.push(owner.clone());
                    return Ok(());
                }
                let last_activity = match observations.last_activity(&owner) {
                    Ok(last_activity) => last_activity,
                    Err(_) if unborn_branch(&owner) => {
                        unborn.push(owner.clone());
                        return Ok(());
                    }
                    Err(error) => return Err(error),
                };
                if last_activity.as_str() > cutoff.as_str() {
                    active = active.saturating_add(candidates.len());
                    return Ok(());
                }
                let mut index = TrackedIndex::list(&owner)?;
                for candidate in &candidates {
                    if index.holds_tracked_files(&candidate.path)? {
                        tracked_here += 1;
                        continue;
                    }
                    match authored_entry_under(&candidate.path)? {
                        Some(Authored::ProgramKeypair(_)) => {
                            keypairs_here += 1;
                            continue;
                        }
                        Some(Authored::Repository(_)) => {
                            repositories_here += 1;
                            continue;
                        }
                        Some(Authored::TerraformState(_)) => {
                            states_here += 1;
                            continue;
                        }
                        _ => {}
                    }
                    let size = dir_size(&candidate.path)?;
                    judged.push(
                        Finding::new(
                            candidate.evidence.label.clone(),
                            Some(candidate.path.clone()),
                            size,
                            format!(
                                "repo last active {last_activity} UTC; corroboration: {}",
                                candidate.evidence.corroboration
                            ),
                            escalate(5, size),
                            Action::Trash,
                        )
                        .with_project(&owner),
                    );
                }
                Ok(())
            })();
            match result {
                Ok(()) => {
                    findings.append(&mut judged);
                    tracked = tracked.saturating_add(tracked_here);
                    keypairs = keypairs.saturating_add(keypairs_here);
                    repositories = repositories.saturating_add(repositories_here);
                    states = states.saturating_add(states_here);
                }
                Err(error) => findings.push(unjudged_finding("artifacts", &owner, &error)),
            }
        }
        if !unborn.is_empty() && !ctx.json {
            ctx.diagnostic(
                "info",
                format!(
                    "skipping artifact directories in {} repositories with no commits yet, so their activity cannot be read: {}",
                    unborn.len(),
                    listed_repositories(&unborn)
                ),
            );
        }
        if orphaned > 0 && !ctx.json {
            ctx.diagnostic(
                "info",
                format!(
                    "skipping {orphaned} artifact directories in a worktree whose repository is gone, so its activity cannot be read: {}; delete such a worktree yourself once you no longer need it",
                    listed_repositories(&orphans)
                ),
            );
        }
        if active > 0 && !ctx.json {
            ctx.diagnostic(
                "info",
                format!(
                    "skipping {active} artifact directories in repos active in {}",
                    activity_window(ctx.active_days)
                ),
            );
        }
        if build_active > 0 && !ctx.json {
            ctx.diagnostic(
                "info",
                format!(
                    "skipping {build_active} artifact directories because a build process is active in {}",
                    listed_repositories(&busy)
                ),
            );
        }
        if tracked > 0 && !ctx.json {
            ctx.diagnostic(
                "info",
                format!("skipping {tracked} artifact directories holding Git-tracked files"),
            );
        }
        if keypairs > 0 && !ctx.json {
            ctx.diagnostic(
                "info",
                format!(
                    "skipping {keypairs} artifact directories holding a program keypair (*-keypair.json)"
                ),
            );
        }
        if states > 0 && !ctx.json {
            ctx.diagnostic(
                "info",
                format!(
                    "skipping {states} artifact directories holding Terraform state (*.tfstate), which no rebuild restores"
                ),
            );
        }
        if repositories > 0 && !ctx.json {
            ctx.diagnostic(
                "info",
                format!("skipping {repositories} artifact directories holding a Git repository"),
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
        self.scan(
            ctx,
            &ScanObservations::with_process_cwds(process_cwds.to_vec()),
        )
    }

    pub(super) fn apply_with_process_cwds(
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
        // Everything a finding must still satisfy, judged in the preflight
        // for the whole plan and again just before its own removal.
        let recheck = |path: &Path| -> Result<()> {
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
            let owner = owning_repo(path, &ctx.home)?
                .ok_or_else(|| anyhow::anyhow!("cannot prove Git owner for {}", path.display()))?;
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
            let last_activity = repo_last_activity(&owner)?;
            if last_activity > cutoff {
                anyhow::bail!(
                    "repo became active after preview; refusing {}",
                    path.display()
                );
            }
            if tracks_files_under(&owner, path)? {
                anyhow::bail!(
                    "refusing {}: its repository tracks files under it",
                    path.display()
                );
            }
            match authored_entry_under(path)? {
                Some(Authored::ProgramKeypair(keypair)) => anyhow::bail!(
                    "refusing {}: it holds the program keypair {}",
                    path.display(),
                    keypair.display()
                ),
                Some(Authored::Repository(marker)) => anyhow::bail!(
                    "refusing {}: it holds the Git repository marked by {}",
                    path.display(),
                    marker.display()
                ),
                Some(Authored::TerraformState(state)) => anyhow::bail!(
                    "refusing {}: it holds the Terraform state {}",
                    path.display(),
                    state.display()
                ),
                _ => {}
            }
            Ok(())
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
            let result = recheck(path);
            if let Err(error) = result {
                outcome.fail(error);
                return Ok(outcome);
            }
            ready.push((finding, path));
        }

        // Every finding passed its preflight; one the sink still refuses costs
        // only itself. Each failure is recorded, so the run reports nonzero.
        // The preflight judged every finding before any was removed, but
        // removing the earlier ones takes time: each is judged again just
        // before its own removal, and a change in between refuses only it.
        for (finding, path) in ready {
            match recheck(path).and_then(|()| apply_filesystem_finding(self.name(), finding, ctx)) {
                Ok(()) => outcome.record(finding, removal_note(finding, path.display())),
                Err(error) => outcome.fail(error),
            }
        }
        Ok(outcome)
    }
}

/// Something inside a build-output tree that no rebuild restores.
enum Authored {
    /// A Solana program keypair.
    ProgramKeypair(PathBuf),
    /// The marker of a Git repository or worktree below the tree's root.
    Repository(PathBuf),
    /// Terraform state or its backup, the only record of what it manages.
    TerraformState(PathBuf),
}

/// The first entry under `path` that no rebuild restores, as Mole V1.56.0
/// looks for it before a purge (`lib/clean/project.sh`: `find` for `.git` or
/// `*-keypair.json`).
///
/// `cargo build-sbf`, which `anchor build` runs, writes `<program>-keypair.json`
/// into its output directory — `target/deploy` unless `--sbf-out-dir` or
/// `SBF_OUT_PATH` names another — and generates one only when none exists
/// (cargo-build-sbf v4.4.0 `src/post_processing.rs:167-171,188,201`). Its
/// public key is the program's address, so a rebuild after removal mints a
/// different address. Git ignores `target`, so the key is untracked and only
/// its name identifies it. A repository nested anywhere below the root — a
/// SwiftPM `.build` keeps its dependencies as Git clones in `checkouts`, a
/// `.venv` holds an editable install's — is someone's history, which the sink
/// would refuse at apply anyway. A keypair's name matches as the volume folds
/// it, a Git marker's in any ASCII case, and the walk follows no link.
fn authored_entry_under(path: &Path) -> Result<Option<Authored>> {
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .follow_root_links(false)
    {
        let entry = entry.with_context(|| {
            format!(
                "cannot search {} for a program keypair or a Git repository",
                path.display()
            )
        })?;
        if is_program_keypair_name(entry.file_name()) {
            return Ok(Some(Authored::ProgramKeypair(entry.into_path())));
        }
        if is_terraform_state_name(entry.file_name()) {
            return Ok(Some(Authored::TerraformState(entry.into_path())));
        }
        if entry.depth() > 0 && is_git_metadata_name(entry.file_name()) {
            return Ok(Some(Authored::Repository(entry.into_path())));
        }
    }
    Ok(None)
}

/// Whether `name` is Terraform state: `*.tfstate`, or the `*.tfstate.backup`
/// Terraform keeps beside it, in any ASCII case. Terraform's local backend
/// writes `terraform.tfstate` into its working directory, which under
/// Terragrunt is inside `.terragrunt-cache` (Terraform "Backend Type: local";
/// Terragrunt "Terragrunt cache"), and nothing regenerates it.
fn is_terraform_state_name(name: &OsStr) -> bool {
    let name = name.as_encoded_bytes();
    [&b".tfstate"[..], b".tfstate.backup"].iter().any(|suffix| {
        name.len() >= suffix.len() && name[name.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
    })
}

/// Whether `name` ends in `-keypair.json` as the volume compares names: in any
/// case, with the two non-ASCII letters APFS folds onto ASCII ones folded too —
/// the Kelvin sign onto `k` and the long s onto `s` (Unicode `CaseFolding.txt`)
/// — so a key under such a spelling, which still opens by its usual name, is
/// still recognized. A name that is not UTF-8 compares its bytes.
fn is_program_keypair_name(name: &OsStr) -> bool {
    const SUFFIX: &str = "-keypair.json";
    match name.to_str() {
        Some(name) => name
            .chars()
            .map(|character| match character {
                '\u{212a}' => 'k',
                '\u{17f}' => 's',
                other => other.to_ascii_lowercase(),
            })
            .collect::<String>()
            .ends_with(SUFFIX),
        None => {
            let name = name.as_encoded_bytes();
            name.len() >= SUFFIX.len()
                && name[name.len() - SUFFIX.len()..].eq_ignore_ascii_case(SUFFIX.as_bytes())
        }
    }
}

/// Corroborated build output under `root`. A folder the walk cannot read is
/// recorded in `unread` and skipped with everything below it, as a scan root
/// that cannot be read is; only the root itself failing fails the walk.
fn find_artifacts(root: &Path, unread: &mut Vec<UnreadFolder>) -> Result<Vec<ArtifactCandidate>> {
    let mut found = Vec::new();
    let mut entries = walkdir::WalkDir::new(root).follow_links(false).into_iter();
    while let Some(result) = entries.next() {
        let entry = match result {
            Ok(entry) => entry,
            Err(error) if error.depth() > 0 => {
                unread.push(UnreadFolder::from_walk(&error));
                continue;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("cannot scan artifacts under {}", root.display()));
            }
        };
        if !entry.file_type().is_dir() {
            continue;
        }
        if is_git_metadata_name(entry.file_name()) || is_node_modules_name(entry.file_name()) {
            entries.skip_current_dir();
            continue;
        }
        let evidence = match artifact_evidence(entry.path()).and_then(|evidence| {
            Ok(match evidence {
                Some(evidence) => Some((evidence, has_git_marker(entry.path())?)),
                None => None,
            })
        }) {
            Ok(evidence) => evidence,
            Err(error) if entry.depth() > 0 => {
                unread.push(UnreadFolder::new(entry.path(), &error));
                entries.skip_current_dir();
                continue;
            }
            Err(error) => return Err(error),
        };
        if let Some((evidence, repository)) = evidence {
            entries.skip_current_dir();
            if !repository {
                found.push(ArtifactCandidate {
                    path: entry.path().to_path_buf(),
                    evidence,
                });
            }
        }
    }
    found.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(found)
}

/// Whether `path` is build output this category would offer, by the same
/// name, corroboration and `CACHEDIR.TAG` evidence, whatever its staleness.
pub(super) fn is_build_output(path: &Path) -> Result<bool> {
    Ok(artifact_evidence(path)?.is_some())
}

/// The build output of `owner` that holds `path`: a directory strictly
/// between the repository and `path` that this category would offer.
/// Everything below it belongs to that output, so whether it goes is decided
/// by the output's own finding and gates, never by a finding nested inside
/// it. Ancestors above the repository are the repository's surroundings, not
/// its output.
pub(super) fn build_output_between(owner: &Path, path: &Path) -> Result<Option<PathBuf>> {
    for ancestor in path
        .ancestors()
        .skip(1)
        .take_while(|ancestor| *ancestor != owner && ancestor.starts_with(owner))
    {
        if is_build_output(ancestor)? {
            return Ok(Some(ancestor.to_path_buf()));
        }
    }
    Ok(None)
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
        // The Android Gradle Plugin's default "external native build output
        // directory", `<project_dir>/<module>/.cxx/`, which "also includes
        // other build system files that should persist when performing clean
        // builds, such as Ninja build files"; the plugin creates it and the
        // next native build regenerates it (AGP 9.4 DSL reference,
        // `Cmake.buildStagingDirectory`).
        Some(".cxx") => sibling_evidence(path, &["build.gradle", "build.gradle.kts"]),
        // "You can safely delete this folder any time, and Terragrunt will
        // recreate it as necessary" (Terragrunt reference, "Terragrunt cache",
        // docs.terragrunt.com/reference/terragrunt-cache); state lives in the
        // backend, not here.
        Some(".terragrunt-cache") => sibling_evidence(path, &["terragrunt.hcl"]),
        // Nuxt's production build output, "re-created each time you run
        // `nuxt build`", which Nuxt says to keep out of Git (Nuxt 4.x docs,
        // "Directory Structure: .output").
        Some(".output") => sibling_evidence(
            path,
            &[
                "nuxt.config.ts",
                "nuxt.config.js",
                "nuxt.config.mjs",
                "nuxt.config.mts",
                "nuxt.config.cjs",
                "nuxt.config.cts",
            ],
        ),
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

/// Opened once, without following a link and without blocking, as the uv tag
/// is read (`ops::has_cachedir_tag`): a symlink, FIFO or directory at the name
/// is not a tag, and nothing can replace the file between a check and the
/// read. Every directory the project walks visit is probed, so this is also
/// the cheaper path. A failure other than absence still fails the scan.
fn cachedir_tag_matches(path: &Path) -> Result<bool> {
    use rustix::fs::{Mode, OFlags};
    use rustix::io::Errno;

    let tag = path.join("CACHEDIR.TAG");
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    let descriptor = match rustix::fs::open(&tag, flags, Mode::empty()) {
        Ok(descriptor) => descriptor,
        Err(Errno::NOENT | Errno::LOOP) => return Ok(false),
        Err(error) => {
            return Err(std::io::Error::from(error))
                .with_context(|| format!("cannot inspect artifact evidence {}", tag.display()));
        }
    };
    let mut file = std::fs::File::from(descriptor);
    let is_file = file
        .metadata()
        .with_context(|| format!("cannot inspect artifact evidence {}", tag.display()))?
        .file_type()
        .is_file();
    if !is_file {
        return Ok(false);
    }
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
    use crate::ops::project::{commit_old_git_fixture, init_old_git_repo};
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
            roots_origin: crate::safety::RootsOrigin::Default,
            active_days: 30,
            retain_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home,
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        }
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

        let mut unread = Vec::new();
        let found = find_artifacts(&root, &mut unread).unwrap();
        assert!(unread.is_empty());
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

        let mut unread = Vec::new();
        let found = find_artifacts(&root, &mut unread).unwrap();
        assert!(unread.is_empty());

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
        let fixture = crate::ops::TestFixture::new("devtrim-artifact-case-ancestor");
        let root = fixture.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        let home = root.canonicalize().unwrap();
        let repo = home.join("repo");
        init_old_git_repo(&repo).unwrap();
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

    /// A fixture root under this checkout's `target`, where the sink may delete.
    fn deletable_root(name: &str) -> (crate::ops::TestFixture, PathBuf, PathBuf) {
        let fixture = crate::ops::TestFixture::new(&format!("devtrim-artifacts-{name}"));
        let root = fixture.path().to_path_buf();
        std::fs::create_dir_all(&root).unwrap();
        let home = root.canonicalize().unwrap();
        (fixture, root, home)
    }

    /// Only a regular file at the name is a tag. A FIFO there must not block
    /// the walk (no writer ever opens it) and a link must not lend its target's
    /// signature; the regular tag beside them is the positive control.
    #[test]
    fn only_a_regular_cachedir_tag_marks_build_output() {
        let base = temp("cachedir-shapes");
        let tagged = base.join("tagged");
        let linked = base.join("linked");
        let fifo = base.join("fifo");
        for dir in [&tagged, &linked, &fifo] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(tagged.join("CACHEDIR.TAG"), CACHEDIR_SIGNATURE).unwrap();
        symlink(tagged.join("CACHEDIR.TAG"), linked.join("CACHEDIR.TAG")).unwrap();
        let status = std::process::Command::new("/usr/bin/mkfifo")
            .arg(fifo.join("CACHEDIR.TAG"))
            .status()
            .unwrap();
        assert!(status.success());

        assert!(is_build_output(&tagged).unwrap());
        assert!(!is_build_output(&linked).unwrap());
        assert!(!is_build_output(&fifo).unwrap());
        crate::ops::remove_test_path(base);
    }

    fn offered(findings: &[Finding]) -> Vec<&Path> {
        findings.iter().filter_map(Finding::target).collect()
    }

    #[test]
    fn a_tree_its_repository_tracks_is_never_offered_or_removed() {
        let (_fixture, root, home) = deletable_root("tracked");
        let repo = home.join("app");
        init_old_git_repo(&repo).unwrap();
        // CocoaPods recommends committing `Pods`.
        let pods = repo.join("Pods");
        std::fs::create_dir_all(pods.join("Alamofire")).unwrap();
        std::fs::write(repo.join("Podfile"), "platform :ios, '17.0'\n").unwrap();
        let vendored = pods.join("Alamofire/Session.swift");
        std::fs::write(&vendored, "// vendored\n").unwrap();
        commit_old_git_fixture(&repo, &["Podfile", "Pods"]).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[package]").unwrap();
        std::fs::create_dir_all(repo.join("target")).unwrap();
        std::fs::write(repo.join("target/out"), "x").unwrap();
        let ctx = context(home.clone());

        let findings = Artifacts.scan_with_process_cwds(&ctx, &[]).unwrap();
        let paths = offered(&findings);
        assert!(
            !paths.contains(&pods.as_path()),
            "PV project/tracked-files: a committed Pods was offered: {paths:?}"
        );
        assert!(
            paths.contains(&repo.join("target").as_path()),
            "positive control: the untracked target was not offered: {paths:?}"
        );

        let forged = Finding::new(
            "stale Pods artifacts",
            Some(pods.clone()),
            12,
            "test",
            9,
            Action::Shred,
        );
        let outcome = Artifacts
            .apply_with_process_cwds(&[forged], &ctx, Ok(Vec::new()))
            .unwrap();
        assert!(
            outcome.summary.items_touched == 0
                && vendored.exists()
                && outcome
                    .errors
                    .iter()
                    .any(|error| error.contains("tracks files under it")),
            "PV artifacts/tracked-apply: {outcome:?}"
        );
        crate::ops::remove_test_path(root);
    }

    /// On a case-insensitive volume a directory renamed only in case goes
    /// unnoticed by Git, which keeps the old spelling in its index. The files
    /// under it are still tracked, so the tree is still never offered.
    #[test]
    fn a_tracked_tree_renamed_only_in_case_is_never_offered() {
        let (_fixture, root, home) = deletable_root("tracked-case");
        let repo = home.join("app");
        init_old_git_repo(&repo).unwrap();
        std::fs::create_dir_all(repo.join("client/Pods/Alamofire")).unwrap();
        std::fs::write(repo.join("client/Podfile"), "platform :ios, '17.0'\n").unwrap();
        std::fs::write(
            repo.join("client/Pods/Alamofire/Session.swift"),
            "// vendored\n",
        )
        .unwrap();
        commit_old_git_fixture(&repo, &["client"]).unwrap();
        std::fs::rename(repo.join("client"), repo.join("renaming")).unwrap();
        std::fs::rename(repo.join("renaming"), repo.join("Client")).unwrap();
        let pods = repo.join("Client/Pods");
        let ctx = context(home.clone());

        let findings = Artifacts.scan_with_process_cwds(&ctx, &[]).unwrap();

        let paths = offered(&findings);
        assert!(pods.is_dir(), "fixture: the renamed tree is there");
        assert!(
            !paths.contains(&pods.as_path()),
            "PV project/tracked-case-rename: a tracked tree renamed only in case was offered: {paths:?}"
        );
        crate::ops::remove_test_path(root);
    }

    /// Git folds only ASCII case, while the volume folds Unicode case too, so a
    /// tracked directory renamed only in the case of a letter like `Ä` reads as
    /// untracked to Git. The filesystem decides instead; an untracked sibling
    /// with a non-ASCII name is the control that nothing else is refused.
    #[test]
    fn a_tracked_tree_renamed_only_in_unicode_case_is_never_offered() {
        let (_fixture, root, home) = deletable_root("tracked-unicode-case");
        let repo = home.join("app");
        init_old_git_repo(&repo).unwrap();
        std::fs::create_dir_all(repo.join("\u{c4}pp/Pods/Alamofire")).unwrap();
        std::fs::write(repo.join("\u{c4}pp/Podfile"), "platform :ios, '17.0'\n").unwrap();
        std::fs::write(
            repo.join("\u{c4}pp/Pods/Alamofire/Session.swift"),
            "// vendored\n",
        )
        .unwrap();
        commit_old_git_fixture(&repo, &["\u{c4}pp"]).unwrap();
        std::fs::rename(repo.join("\u{c4}pp"), repo.join("renaming")).unwrap();
        std::fs::rename(repo.join("renaming"), repo.join("\u{e4}pp")).unwrap();
        std::fs::create_dir_all(repo.join("\u{3a9}mega/target")).unwrap();
        std::fs::write(repo.join("\u{3a9}mega/Cargo.toml"), "[package]").unwrap();
        std::fs::write(repo.join("\u{3a9}mega/target/out"), "x").unwrap();
        let pods = repo.join("\u{e4}pp/Pods");
        let ctx = context(home.clone());

        let findings = Artifacts.scan_with_process_cwds(&ctx, &[]).unwrap();

        let paths = offered(&findings);
        assert!(pods.is_dir(), "fixture: the renamed tree is there");
        assert!(
            !paths.contains(&pods.as_path()),
            "PV project/tracked-unicode-case: a tracked tree renamed only in case was offered: {paths:?}"
        );
        assert!(
            paths.contains(&repo.join("\u{3a9}mega/target").as_path()),
            "positive control: the untracked non-ASCII target was not offered: {paths:?}"
        );
        crate::ops::remove_test_path(root);
    }

    /// The volume folds Unicode case fully: `STRASSE` names a directory
    /// created as `Stra\u{df}e`. An all-ASCII spelling can therefore alias a
    /// tracked non-ASCII one, and only the volume can say they are the same.
    #[test]
    fn a_tracked_tree_renamed_to_an_ascii_alias_is_never_offered() {
        let (_fixture, root, home) = deletable_root("tracked-ascii-alias");
        let repo = home.join("app");
        init_old_git_repo(&repo).unwrap();
        std::fs::create_dir_all(repo.join("Stra\u{df}e/Pods/Alamofire")).unwrap();
        std::fs::write(repo.join("Stra\u{df}e/Podfile"), "platform :ios, '17.0'\n").unwrap();
        std::fs::write(
            repo.join("Stra\u{df}e/Pods/Alamofire/Session.swift"),
            "// vendored\n",
        )
        .unwrap();
        commit_old_git_fixture(&repo, &["Stra\u{df}e"]).unwrap();
        std::fs::rename(repo.join("Stra\u{df}e"), repo.join("renaming")).unwrap();
        std::fs::rename(repo.join("renaming"), repo.join("STRASSE")).unwrap();
        let pods = repo.join("STRASSE/Pods");
        let ctx = context(home.clone());

        let findings = Artifacts.scan_with_process_cwds(&ctx, &[]).unwrap();

        let paths = offered(&findings);
        assert!(pods.is_dir(), "fixture: the renamed tree is there");
        assert!(
            !paths.contains(&pods.as_path()),
            "PV project/tracked-ascii-alias: a tracked tree renamed to an ASCII alias was offered: {paths:?}"
        );
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn a_tree_holding_a_program_keypair_is_never_offered_or_removed() {
        let (_fixture, root, home) = deletable_root("keypair");
        let program = home.join("program");
        init_old_git_repo(&program).unwrap();
        std::fs::write(program.join("Cargo.toml"), "[package]").unwrap();
        let target = program.join("target");
        std::fs::create_dir_all(target.join("deploy")).unwrap();
        let keypair = target.join("deploy/program-keypair.json");
        std::fs::write(&keypair, "[0]").unwrap();
        let other = home.join("other");
        init_old_git_repo(&other).unwrap();
        std::fs::write(other.join("Cargo.toml"), "[package]").unwrap();
        std::fs::create_dir_all(other.join("target")).unwrap();
        std::fs::write(other.join("target/out"), "x").unwrap();
        let ctx = context(home.clone());

        let findings = Artifacts.scan_with_process_cwds(&ctx, &[]).unwrap();
        let paths = offered(&findings);
        assert!(
            !paths.contains(&target.as_path()),
            "PV artifacts/keypair-scan: a target holding a program keypair was offered: {paths:?}"
        );
        assert!(
            paths.contains(&other.join("target").as_path()),
            "positive control: the target without a keypair was not offered: {paths:?}"
        );

        let forged = Finding::new(
            "stale target artifacts",
            Some(target.clone()),
            3,
            "test",
            9,
            Action::Shred,
        );
        let outcome = Artifacts
            .apply_with_process_cwds(&[forged], &ctx, Ok(Vec::new()))
            .unwrap();
        assert!(
            outcome.summary.items_touched == 0
                && keypair.exists()
                && outcome
                    .errors
                    .iter()
                    .any(|error| error.contains("holds the program keypair")),
            "PV artifacts/keypair-apply: {outcome:?}"
        );
        crate::ops::remove_test_path(root);
    }

    /// Mole refuses a purge target holding a `.git` anywhere inside, not only
    /// at its top: a SwiftPM `.build` keeps its dependencies as Git clones in
    /// `checkouts`, and a `.venv` holds an editable install's repository. The
    /// sink would refuse either at apply, so the scan must not offer them.
    #[test]
    fn a_tree_holding_a_git_repository_is_never_offered_or_removed() {
        let (_fixture, root, home) = deletable_root("nested-repository");
        let package = home.join("package");
        init_old_git_repo(&package).unwrap();
        std::fs::write(
            package.join("Package.swift"),
            "// swift-tools-version:5.9\n",
        )
        .unwrap();
        let build = package.join(".build");
        let clone = build.join("checkouts/swift-argument-parser/.git");
        std::fs::create_dir_all(&clone).unwrap();
        std::fs::write(clone.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        let other = home.join("other");
        init_old_git_repo(&other).unwrap();
        std::fs::write(other.join("Cargo.toml"), "[package]").unwrap();
        std::fs::create_dir_all(other.join("target")).unwrap();
        std::fs::write(other.join("target/out"), "x").unwrap();
        let ctx = context(home.clone());

        let findings = Artifacts.scan_with_process_cwds(&ctx, &[]).unwrap();
        let paths = offered(&findings);
        assert!(
            !paths.contains(&build.as_path()),
            "PV artifacts/nested-repository-scan: a tree holding a repository was offered: {paths:?}"
        );
        assert!(
            paths.contains(&other.join("target").as_path()),
            "positive control: the target without a repository was not offered: {paths:?}"
        );

        let forged = Finding::new(
            "stale .build artifacts",
            Some(build.clone()),
            3,
            "test",
            9,
            Action::Shred,
        );
        let outcome = Artifacts
            .apply_with_process_cwds(&[forged], &ctx, Ok(Vec::new()))
            .unwrap();
        assert!(
            outcome.summary.items_touched == 0
                && clone.join("HEAD").exists()
                && outcome
                    .errors
                    .iter()
                    .any(|error| error.contains("holds the Git repository")),
            "PV artifacts/nested-repository-apply: {outcome:?}"
        );
        crate::ops::remove_test_path(root);
    }

    /// A finding the sink refuses costs only itself: here a configured
    /// `protect` entry, which only the sink consults, and the plan continues.
    #[test]
    fn a_refused_artifact_does_not_block_the_rest_of_the_plan() {
        let (_fixture, root, home) = deletable_root("artifacts-continue");
        for name in ["first", "second"] {
            let repo = home.join(name);
            init_old_git_repo(&repo).unwrap();
            std::fs::write(repo.join("Cargo.toml"), "[package]").unwrap();
            std::fs::create_dir_all(repo.join("target")).unwrap();
            std::fs::write(repo.join("target/out"), "x").unwrap();
        }
        let mut ctx = context(home.clone());
        ctx.protect = vec![home.join("first/target")];
        let mut findings = Artifacts.scan_with_process_cwds(&ctx, &[]).unwrap();
        crate::report::effective_actions(&mut findings, true);

        let outcome = Artifacts
            .apply_with_process_cwds(&findings, &ctx, Ok(Vec::new()))
            .unwrap();

        assert!(home.join("first/target/out").exists());
        assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
        assert!(
            !home.join("second/target").exists(),
            "PV artifacts/continue-past-refusal: the refusal stranded the next finding: {outcome:?}"
        );
        crate::ops::remove_test_path(root);
    }

    fn stale_target(repo: &Path) -> PathBuf {
        init_old_git_repo(repo).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[package]").unwrap();
        let target = repo.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("out"), "x").unwrap();
        target
    }

    /// Terraform's local backend writes `terraform.tfstate` into the working
    /// directory Terragrunt runs it in, `.terragrunt-cache/<hash>/<hash>/`, and
    /// that file is the only record of the infrastructure it manages: no
    /// rebuild restores it. A build-output tree holding a state file or its
    /// backup is never offered, whatever its name, and apply refuses it.
    #[test]
    fn a_tree_holding_terraform_state_is_never_offered_or_removed() {
        let (_fixture, root, home) = deletable_root("terraform-state");
        let repo = home.join("infra");
        init_old_git_repo(&repo).unwrap();
        let unit = repo.join("live/app");
        std::fs::create_dir_all(unit.join(".terragrunt-cache/a1/b2")).unwrap();
        std::fs::write(unit.join("terragrunt.hcl"), "").unwrap();
        let state = unit.join(".terragrunt-cache/a1/b2/terraform.tfstate");
        std::fs::write(&state, "{\"version\": 4}").unwrap();
        let other_unit = repo.join("live/db");
        std::fs::create_dir_all(other_unit.join(".terragrunt-cache/c3/modules")).unwrap();
        std::fs::write(other_unit.join("terragrunt.hcl"), "").unwrap();
        std::fs::write(other_unit.join(".terragrunt-cache/c3/modules/main.tf"), "").unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[package]").unwrap();
        std::fs::create_dir_all(repo.join("target/debug")).unwrap();
        std::fs::write(repo.join("target/debug/Terraform.TFSTATE.backup"), "{}").unwrap();
        let ctx = context(home.clone());

        let findings = Artifacts.scan_with_process_cwds(&ctx, &[]).unwrap();

        assert_eq!(
            findings
                .iter()
                .filter_map(Finding::target)
                .collect::<Vec<_>>(),
            vec![other_unit.join(".terragrunt-cache").as_path()],
            "PV artifacts/terraform-state-scan: a tree holding Terraform state was offered"
        );
        assert!(
            ctx.take_diagnostics()
                .iter()
                .any(|message| message.contains("2 artifact directories holding Terraform state")),
            "the kept state must be named"
        );

        let forged = Finding::new(
            "stale .terragrunt-cache artifacts",
            Some(unit.join(".terragrunt-cache")),
            1,
            "test",
            5,
            Action::Shred,
        );
        let outcome = Artifacts
            .apply_with_process_cwds(&[forged], &ctx, Ok(Vec::new()))
            .unwrap();
        assert!(
            outcome.summary.items_touched == 0
                && state.exists()
                && outcome
                    .errors
                    .iter()
                    .any(|error| error.contains("Terraform state")),
            "PV artifacts/terraform-state-apply: {outcome:?}"
        );
        crate::ops::remove_test_path(root);
    }

    /// Mole's three further names, each only beside the file of the tool that
    /// writes it: Android's native build staging folder `.cxx` beside the
    /// module's `build.gradle(.kts)`, Terragrunt's `.terragrunt-cache` beside a
    /// `terragrunt.hcl`, and Nuxt's `.output` beside a `nuxt.config.*`.
    /// Without that file the same name is someone else's folder.
    #[test]
    fn newer_names_are_offered_only_beside_their_owners_file() {
        let root = temp("newer-names");
        for (folder, owner_file) in [
            ("android/app/.cxx", Some("android/app/build.gradle")),
            ("android/kts/.cxx", Some("android/kts/build.gradle.kts")),
            (
                "infra/unit/.terragrunt-cache",
                Some("infra/unit/terragrunt.hcl"),
            ),
            ("web/.output", Some("web/nuxt.config.ts")),
            ("web-mjs/.output", Some("web-mjs/nuxt.config.mjs")),
            ("native/.cxx", None),
            ("ops/.terragrunt-cache", None),
            ("site/.output", None),
        ] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
            if let Some(owner_file) = owner_file {
                std::fs::write(root.join(owner_file), "").unwrap();
            }
            let evidence = artifact_evidence(&root.join(folder)).unwrap();
            assert_eq!(
                evidence.is_some(),
                owner_file.is_some(),
                "PV artifacts/newer-names: {folder} beside {owner_file:?}"
            );
        }
        // A folder named like the owner file is not one.
        std::fs::create_dir_all(root.join("fake/nuxt.config.ts")).unwrap();
        std::fs::create_dir_all(root.join("fake/.output")).unwrap();
        assert!(
            artifact_evidence(&root.join("fake/.output"))
                .unwrap()
                .is_none()
        );
        crate::ops::remove_test_path(root);
    }

    /// A tree the scan cannot read through is not known to be free of keys or
    /// repositories, so it is never offered: its repository reports the error
    /// and offers nothing, while every other repository is judged as usual.
    #[test]
    fn an_unreadable_tree_refuses_its_repository_rather_than_offers() {
        let (_fixture, root, home) = deletable_root("unreadable-tree");
        let healthy = stale_target(&home.join("healthy"));
        // Judged in path order: `a/target` is measured, then `b/target`
        // fails, so the repository's already-built finding must be withheld.
        let repo = home.join("app");
        init_old_git_repo(&repo).unwrap();
        for package in ["a", "b"] {
            std::fs::create_dir_all(repo.join(package).join("target")).unwrap();
            std::fs::write(repo.join(package).join("Cargo.toml"), "[package]").unwrap();
        }
        std::fs::write(repo.join("a/target/out"), "x").unwrap();
        let locked = repo.join("b/target/locked");
        let _locked = crate::ops::Unreadable::new(&locked, 0o000);
        let ctx = context(home.clone());

        let findings = Artifacts.scan_with_process_cwds(&ctx, &[]).unwrap();

        assert_eq!(
            offered(&findings),
            vec![healthy.as_path()],
            "PV artifacts/repository-contained: an unreadable tree was offered"
        );
        let errors = findings
            .iter()
            .filter_map(Finding::scan_error)
            .collect::<Vec<_>>();
        assert!(
            errors.len() == 1
                && errors[0].contains("cannot search")
                && errors[0].contains(&repo.display().to_string()),
            "{errors:?}"
        );
        drop(_locked);
        crate::ops::remove_test_path(root);
    }

    /// The failure found on the owner's Mac: a root-owned `.fseventsd`
    /// (`0o700`) left in a project's scratch folder by a mounted disk image.
    /// Probing it for `CACHEDIR.TAG` was refused, and that emptied `artifacts`,
    /// `node-modules` and `purge` alike. The folder is now reported as an
    /// error, nothing in it is offered, and everything else is still judged.
    #[test]
    fn an_unreadable_folder_is_reported_and_blocks_only_itself() {
        let (_fixture, root, home) = deletable_root("unreadable-walk");
        let target = stale_target(&home.join("app"));
        let locked = home.join("app/.scratch/recovery-volume/.fseventsd");
        let unlisted = home.join("other/unlisted");
        let _locked = crate::ops::Unreadable::new(&locked, 0o000);
        let _unlisted = crate::ops::Unreadable::new(&unlisted, 0o100);
        let ctx = context(home.clone());

        let findings = Artifacts.scan_with_process_cwds(&ctx, &[]).unwrap();

        assert_eq!(offered(&findings), vec![target.as_path()]);
        let errors = findings
            .iter()
            .filter_map(Finding::scan_error)
            .collect::<Vec<_>>();
        assert!(
            errors.len() == 1
                && errors[0].contains("could not be read")
                && errors[0].contains(&locked.display().to_string())
                && errors[0].contains(&unlisted.display().to_string()),
            "PV artifacts/unread-folders-reported: {errors:?}"
        );
        drop((_locked, _unlisted));
        crate::ops::remove_test_path(root);
    }

    /// A repository created with `git init` and not yet committed to is left
    /// out and named; it never empties the category.
    #[test]
    fn a_repository_without_commits_is_skipped_and_named() {
        let (_fixture, root, home) = deletable_root("unborn-artifacts");
        let target = stale_target(&home.join("stale"));
        let fresh = home.join("fresh");
        std::fs::create_dir_all(fresh.join("target")).unwrap();
        std::fs::write(fresh.join("Cargo.toml"), "[package]").unwrap();
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(&fresh)
            .status()
            .unwrap();
        assert!(status.success());
        let ctx = context(home.clone());

        let findings = Artifacts.scan_with_process_cwds(&ctx, &[]).unwrap();

        assert_eq!(offered(&findings), vec![target.as_path()]);
        assert!(
            ctx.take_diagnostics()
                .iter()
                .any(|message| message.contains("no commits yet")
                    && message.contains(&fresh.display().to_string())),
            "the repository without commits must be named"
        );
        crate::ops::remove_test_path(root);
    }

    /// The name is matched as the volume matches it: in any case, including the
    /// two non-ASCII letters APFS folds onto ASCII ones, the Kelvin sign onto
    /// `k` and the long s onto `s`, so a key under such a spelling, which still
    /// opens by its usual name, is still recognized.
    #[test]
    fn program_keypair_names_match_the_solana_suffix_as_the_volume_does() {
        for name in [
            "program-keypair.json",
            "Program-KEYPAIR.Json",
            "-keypair.json",
            "program-\u{212a}eypair.json",
            "program-keypair.j\u{17f}on",
        ] {
            assert!(
                is_program_keypair_name(OsStr::new(name)),
                "PV artifacts/keypair-alias: {name}"
            );
        }
        for name in [
            "keypair.json",
            "program-keypair.json.bak",
            "program_keypair.json",
            "program-keypair",
        ] {
            assert!(!is_program_keypair_name(OsStr::new(name)), "{name}");
        }
    }
}
