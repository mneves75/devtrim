//! Regenerable download caches. Filesystem targets remain Trash-first.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::project::{UnreadFolder, has_git_marker, unread_folders_finding};
use super::{
    Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, apply_uv_cache_finding, dir_size,
    removal_note,
};
use crate::process::{BoundedCommand as _, PROBE_TIMEOUT, QUERY_TIMEOUT};
use crate::report::TargetAuthority;
use crate::safety::{Ctx, DeletionEntry, escalate};

pub struct Caches;

const UV_CACHE: &str = ".cache/uv";

/// Exact paths relative to `$HOME`, each owned by one tool that re-creates it.
///
/// These are the tools that keep their cache outside `~/Library/Caches` on
/// macOS. A tool that uses the platform location is listed once, in
/// [`crate::safety::MANAGED_LIBRARY_CACHES`], rather than twice under two
/// spellings — two findings sharing one label would read as a duplicate rather
/// than as two places.
const CACHES: &[DeletionEntry] = &[
    DeletionEntry {
        label: "huggingface model cache",
        relative: ".cache/huggingface/hub",
        evidence: "Model snapshots re-downloaded on next use. Scoped to `hub` \
                   so the sibling tokens and settings are never authority.",
    },
    DeletionEntry {
        label: "uv package cache",
        relative: UV_CACHE,
        evidence: "uv documents `$HOME/.cache/uv` as its Unix cache and \
                   `uv cache clean` as removing every entry; uv refills it on \
                   the next resolve. Apply takes uv's own cache lock first, \
                   as `uv cache clean` does. uv writes an empty `.git` into \
                   its `sdists-v<N>` bucket (uv 0.9.24 \
                   `crates/uv-cache/src/lib.rs:439-449`), which the sink \
                   tolerates in exactly that shape.",
    },
    DeletionEntry {
        label: "Corepack package-manager cache",
        relative: ".cache/node/corepack",
        evidence: "Corepack's README (nodejs/corepack d4dcb1f, v0.36.0): \
                   `COREPACK_HOME` is where Corepack installs the package \
                   managers it downloads and defaults to \
                   `$HOME/.cache/node/corepack` outside Windows; `corepack \
                   cache clean` clears it. Only this child is listed: Node \
                   itself documents nothing under `~/.cache/node` (its compile \
                   cache defaults to the system temporary directory), so the \
                   parent has no owner to vouch for it.",
    },
    DeletionEntry {
        label: "bun package cache",
        relative: ".bun/install/cache",
        evidence: "Bun's documented global install cache; `bun pm cache rm` is \
                   the vendor equivalent. Existing `node_modules` are untouched.",
    },
    DeletionEntry {
        label: "GitHub CLI cache",
        relative: ".cache/gh",
        evidence: "go-gh resolves the CLI cache to `$XDG_CACHE_HOME/gh`, then \
                   `~/.cache/gh`; never `~/Library/Caches` on macOS. Verified \
                   present here with `XDG_CACHE_HOME` unset. Holds API \
                   responses and the sigstore trust cache, both re-fetched. \
                   Credentials live in `~/.config/gh/hosts.yml` or the keychain, \
                   never here — though cached private API bodies can.",
    },
    DeletionEntry {
        label: "cargo registry download cache",
        relative: ".cargo/registry/cache",
        evidence: "Cargo's own guide states any part of this cache may be \
                   removed and Cargo restores sources by re-downloading.",
    },
    DeletionEntry {
        label: "cargo registry sources",
        relative: ".cargo/registry/src",
        evidence: "Unpacked form of the `.crate` archives above; Cargo \
                   re-extracts or re-downloads it, which is why Cargo's own CI \
                   guidance excludes it from caching.",
    },
];

const _: () = assert!(
    crate::safety::evidence_is_present(CACHES),
    "every built-in cache needs evidence for why it may be deleted"
);

/// The two children of Gradle's user home that are caches, relative to that
/// home. Never the home itself, which holds `gradle.properties` and `init.d`.
const GRADLE: &[DeletionEntry] = &[
    DeletionEntry {
        label: "Gradle caches",
        relative: "caches",
        evidence: "Gradle 9.8.1 \"Gradle User Home and Cache Cleanup\" \
                   (docs.gradle.org/current/userguide/directory_layout.html): \
                   version-specific caches \"are recreated on first use of that \
                   Gradle version\", and Gradle itself deletes them after 30 \
                   days unused and shared-cache files after 7 or 30; a \
                   dependency missing from the cache is downloaded again \
                   (dependency_caching.html). Source-dependency checkouts \
                   (`vcs-1`) are build-tree scoped, never here (gradle/gradle \
                   4c8b4c5 `VersionControlRepositoryCacheFactory`). Refused \
                   while any Gradle process runs.",
    },
    DeletionEntry {
        label: "Gradle wrapper distributions",
        relative: "wrapper/dists",
        evidence: "Same page: \"Distributions downloaded by the Gradle \
                   Wrapper\", which Gradle's own cleanup deletes once unused; \
                   the Wrapper downloads a missing distribution again before it \
                   runs (gradle_wrapper.html, 9.8.1). Refused while any Gradle \
                   process runs, since one may be executing from it.",
    },
];

const _: () = assert!(
    crate::safety::evidence_is_present(GRADLE),
    "every Gradle cache needs evidence for why it may be deleted"
);

/// The `~/Library/Caches` half of the same list, derived from the closed
/// carve-out in the protection boundary so a cache can never be previewed
/// without also being deletable, or protected without also being unlisted.
fn library_caches(home: &Path) -> impl Iterator<Item = (&'static str, PathBuf)> {
    crate::safety::MANAGED_LIBRARY_CACHES.iter().map(|entry| {
        (
            entry.label,
            home.join("Library/Caches").join(entry.relative),
        )
    })
}

impl Op for Caches {
    fn name(&self) -> &'static str {
        "caches"
    }

    fn scan(
        &self,
        ctx: &Ctx,
        _observations: &super::project::ScanObservations,
    ) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        // A cache whose size cannot be measured offers nothing and is named in
        // one error finding (S8): it must not stop the caches listed after it.
        let mut unread = Vec::new();
        for entry in CACHES {
            let path = ctx.home.join(entry.relative);
            let size = measured(&path, &mut unread);
            if size > 0 {
                findings.push(cache_finding(entry.label, path, size, 3));
            }
        }
        for (label, path) in library_caches(&ctx.home) {
            let size = measured(&path, &mut unread);
            if size > 0 {
                findings.push(cache_finding(label, path, size, 3));
            }
        }
        if let Some(path) = owner_cache_path("npm", &["config", "get", "cache"], ctx)? {
            let size = measured(&path, &mut unread);
            if size > 0 {
                // `npm config get cache` names npm's whole cache folder, not
                // only the content store `npm cache clean` empties.
                let mut finding = cache_finding("npm cache folder", path, size, 2)
                    .with_authority(TargetAuthority::NpmCache);
                finding.note =
                    "npm's whole cache folder: downloaded packages (`_cacache`) and npx \
                                installs (`_npx`) are fetched again on next use; its logs \
                                (`_logs`) do not come back"
                        .to_string();
                findings.push(finding);
            }
        }
        if let Some(path) = owner_cache_path("brew", &["--cache"], ctx)? {
            match brew_cache_findings(&path, &ctx.home, &mut unread) {
                Ok(brew) => findings.extend(brew),
                Err(error) => unread.push(UnreadFolder::new(&path, &error)),
            }
        }
        findings.extend(gradle_findings(ctx, &mut unread));
        match go_module_cache(ctx, true) {
            Ok(Some(path)) => {
                let size = measured(&path, &mut unread);
                if size > 0 {
                    let mut finding = cache_finding("Go module cache", path, size, 3);
                    finding.note = "Go downloads modules again on next build; moved to the Trash, \
                                    `devtrim trash-empty` frees it; Go keeps the cache read-only, \
                                    so permanent removal in place is refused (`go clean -modcache` \
                                    is Go's own way)"
                        .to_string();
                    findings.push(finding);
                }
            }
            Ok(None) => {}
            Err(error) => findings.push(not_judged("Go module cache", &error)),
        }
        if !unread.is_empty() {
            findings.push(unread_folders_finding("caches", &unread));
        }
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let mut outcome = ApplyOutcome::new(self.name());
        for finding in findings {
            if !matches!(finding.action, Action::Trash | Action::Shred) {
                continue;
            }
            let result = (|| -> Result<()> {
                match authorize_cache_finding(finding, ctx)? {
                    CacheKind::Plain => {}
                    CacheKind::Gradle => {
                        // Probed again at each finding: a build can start while
                        // earlier caches are removed.
                        let running = gradle_processes()?;
                        if !running.is_empty() {
                            anyhow::bail!(
                                "refusing {} while Gradle is running ({} process(es)); `gradle --stop` ends its daemons",
                                finding.label,
                                running.len()
                            );
                        }
                    }
                    CacheKind::GoModules => {
                        if let Some(target) = finding.target() {
                            go_removal_preflight(target)?;
                        }
                    }
                }
                let uv_cache = ctx.home.join(UV_CACHE);
                if finding.target() == Some(uv_cache.as_path()) {
                    // Held until the cache has moved, then released on drop;
                    // holding it is also what lets the sink tolerate uv's
                    // source-distribution marker.
                    let lock = crate::safety::lock_uv_cache(&uv_cache)?;
                    return apply_uv_cache_finding(self.name(), finding, ctx, &lock);
                }
                apply_filesystem_finding(self.name(), finding, ctx)
            })()
            .with_context(|| format!("failed to remove {}", finding.label));
            // One refused cache must not abandon the rest of the previewed plan.
            // The list spans unrelated tools, and one entry can be refused for a
            // reason of its own — uv busy with its cache, a nested repository —
            // which would otherwise block every cache listed after it. Each
            // failure is still recorded, so the run reports nonzero.
            if let Err(error) = result {
                outcome.fail(error);
                continue;
            }
            outcome.record(finding, removal_note(finding, &finding.label));
        }
        Ok(outcome)
    }
}

/// Homebrew's cache, whole, or as its direct children when it holds a Git
/// clone. Homebrew's Git download strategy keeps one repository per
/// Git-sourced formula there (`<name>--git`), which the sink never removes, so
/// a cache holding one would always be refused whole. Each clone, judged by
/// its Git marker rather than its name, stays; every other non-empty child is
/// offered. A top-level link into `downloads` is left in place: Homebrew's own
/// `brew cleanup` removes links it finds dangling.
fn brew_cache_findings(
    cache: &Path,
    home: &Path,
    unread: &mut Vec<UnreadFolder>,
) -> Result<Vec<Finding>> {
    const LABEL: &str = "homebrew downloads cache";
    let entries = match std::fs::read_dir(cache) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("cannot read homebrew cache {}", cache.display()));
        }
    };
    let mut children = Vec::new();
    let mut clones = 0usize;
    for entry in entries {
        let entry =
            entry.with_context(|| format!("cannot read homebrew cache {}", cache.display()))?;
        let file_type = entry
            .file_type()
            .with_context(|| format!("cannot inspect {}", entry.path().display()))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() && has_git_marker(&entry.path())? {
            clones = clones.saturating_add(1);
            continue;
        }
        children.push(entry.path());
    }
    // Only the standard cache is split, so apply can hold each entry to being
    // its direct child; a relocated `HOMEBREW_CACHE` stays one finding, which
    // the sink refuses whole while it holds a clone.
    if clones == 0 || !is_standard_brew_cache(cache, home) {
        let size = measured(cache, unread);
        return Ok((size > 0)
            .then(|| {
                cache_finding(LABEL, cache.to_path_buf(), size, 1)
                    .with_authority(TargetAuthority::BrewCache)
            })
            .into_iter()
            .collect());
    }
    children.sort();
    let mut findings = Vec::new();
    for child in children {
        let size = measured(&child, unread);
        if size == 0 {
            continue;
        }
        let name = child
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let note = format!(
            "{}; the {clones} Git clone(s) Homebrew keeps for Git-sourced formulae, such as `<name>--git`, stay",
            cache_note(&child)
        );
        findings.push(
            Finding::new(
                format!("{LABEL}: {name}"),
                Some(child),
                size,
                note,
                escalate(1, size),
                Action::Trash,
            )
            .with_authority(TargetAuthority::BrewCacheEntry),
        );
    }
    Ok(findings)
}

/// A cache's size, or 0 with the cache recorded in `unread` when it cannot be
/// measured: an unreadable folder, or a size that overflows. Only a cache that
/// does not exist (`dir_size` yields 0) is an ordinary empty one.
fn measured(path: &Path, unread: &mut Vec<UnreadFolder>) -> u64 {
    dir_size(path).unwrap_or_else(|error| {
        unread.push(UnreadFolder::new(path, &error));
        0
    })
}

fn cache_finding(label: &str, path: PathBuf, size: u64, danger: u8) -> Finding {
    let note = cache_note(&path);
    Finding::new(
        label,
        Some(path),
        size,
        note,
        escalate(danger, size),
        Action::Trash,
    )
}

/// What removing one cache costs. Not every entry is a download: an editor
/// index or a compiler cache is rebuilt locally, so the default note does not
/// say "re-downloads". A few owners rebuild only when a command is run, and the
/// note names that command rather than promising a rebuild that never comes.
fn cache_note(path: &Path) -> String {
    match crate::safety::REBUILT_BY_COMMAND
        .iter()
        .find(|(relative, _)| path.ends_with(Path::new("Library/Caches").join(relative)))
    {
        Some((_, command)) => format!(
            "not rebuilt by its next use: `{command}` downloads it again; a large cache costs bandwidth"
        ),
        None => {
            "regenerated automatically on next use; a large cache costs bandwidth or rebuild time"
                .to_string()
        }
    }
}

/// Which cache an authorized finding is, for the checks its owner needs before
/// the sink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CacheKind {
    Plain,
    Gradle,
    GoModules,
}

fn authorize_cache_finding(finding: &Finding, ctx: &Ctx) -> Result<CacheKind> {
    let home = &ctx.home;
    let target = finding
        .target()
        .ok_or_else(|| anyhow::anyhow!("cache finding missing internal target"))?;
    let kind = match finding.authority() {
        TargetAuthority::Standard if is_builtin_cache_root(target, home) => Some(CacheKind::Plain),
        // Both owner roots are recomputed now, never taken from the preview.
        TargetAuthority::Standard if is_gradle_cache(target, home)? => Some(CacheKind::Gradle),
        // Only a folder holding Go's download cache sends devtrim to ask `go`.
        TargetAuthority::Standard if holds_go_download_cache(target)? => {
            go_module_cache(ctx, false)?
                .is_some_and(|cache| cache == target)
                .then_some(CacheKind::GoModules)
        }
        TargetAuthority::Standard => None,
        TargetAuthority::NpmCache => {
            is_eligible_owner_cache("npm", target, home).then_some(CacheKind::Plain)
        }
        TargetAuthority::BrewCache => {
            is_eligible_owner_cache("brew", target, home).then_some(CacheKind::Plain)
        }
        TargetAuthority::BrewCacheEntry => {
            is_brew_cache_entry(target, home)?.then_some(CacheKind::Plain)
        }
    };
    kind.ok_or_else(|| {
        anyhow::anyhow!(
            "cache target is outside its authorized namespace: {}",
            target.display()
        )
    })
}

/// A finding for a cache whose owner could not be asked: it offers nothing,
/// and the run reports failure, while the other caches are still offered.
fn not_judged(subject: &str, error: &anyhow::Error) -> Finding {
    let message = format!("cannot judge the {subject}: {error:#}");
    Finding::new(
        format!("{subject} not judged"),
        None,
        0,
        &message,
        5,
        Action::Info,
    )
    .with_scan_error(message)
}

/// Gradle's user home: `GRADLE_USER_HOME`, by default `~/.gradle` (Gradle 9.8.1
/// "Gradle User Home"). `None` when the variable is relative, which would be
/// read against whatever folder devtrim started in, or names a folder that is
/// not inside the home folder. The home itself is never a target: it also
/// holds `gradle.properties` and `init.d`.
fn gradle_user_home(home: &Path) -> Result<Option<PathBuf>> {
    let Some(configured) = std::env::var_os("GRADLE_USER_HOME") else {
        return Ok(Some(home.join(".gradle")));
    };
    let configured = PathBuf::from(configured);
    if !configured.is_absolute() {
        return Ok(None);
    }
    let resolved = resolved_owner_path("gradle", configured)?;
    Ok(normalized_absolute(&resolved).filter(|path| path.starts_with(home) && path != home))
}

fn is_gradle_cache(target: &Path, home: &Path) -> Result<bool> {
    let Some(gradle) = gradle_user_home(home)? else {
        return Ok(false);
    };
    if gradle_home_in_repository(&gradle, home)? {
        return Ok(false);
    }
    for entry in GRADLE {
        if target == gradle.join(entry.relative) {
            return holds_gradle_layout(entry.relative, target);
        }
    }
    Ok(false)
}

/// A Gradle user home inside a Git repository holds the repository's data,
/// not Gradle's: `GRADLE_USER_HOME` alone never makes a folder a cache.
fn gradle_home_in_repository(gradle: &Path, home: &Path) -> Result<bool> {
    Ok(super::project::has_git_marker(gradle)?
        || super::project::owning_repo(gradle, home)?.is_some())
}

/// Whether one of Gradle's entries holds what Gradle writes there, per the
/// Gradle 9.8.1 user-home layout (docs.gradle.org "Gradle User Home"):
/// `caches/modules-2`, the shared dependency cache, and wrapper distributions
/// named `gradle-<version>-bin` or `gradle-<version>-all` under
/// `wrapper/dists`. A folder that only shares the name holds neither.
fn holds_gradle_layout(relative: &str, path: &Path) -> Result<bool> {
    let real_directory = |candidate: &Path| match std::fs::symlink_metadata(candidate) {
        Ok(metadata) => Ok(metadata.is_dir()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("cannot inspect {}", candidate.display())),
    };
    match relative {
        "caches" => real_directory(&path.join("modules-2")),
        "wrapper/dists" => {
            for entry in std::fs::read_dir(path)
                .with_context(|| format!("cannot read {}", path.display()))?
            {
                let entry = entry.with_context(|| format!("cannot read {}", path.display()))?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let distribution = name.starts_with("gradle-")
                    && (name.ends_with("-bin") || name.ends_with("-all"));
                if distribution && real_directory(&entry.path())? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => Ok(false),
    }
}

/// Gradle's own JVM entry points, matched against the whole command line since
/// every one runs as `java`: the daemon `DefaultDaemonStarter` launches
/// (`org.gradle.launcher.daemon.bootstrap.GradleDaemon`), a build run without
/// one (`org.gradle.launcher.GradleMain`), and the Wrapper
/// (`org.gradle.wrapper.GradleWrapperMain`), per gradle/gradle 4c8b4c5.
const GRADLE_PROCESS_PATTERN: &str = "org\\.gradle\\.(launcher\\.daemon\\.bootstrap\\.GradleDaemon|launcher\\.GradleMain|wrapper\\.GradleWrapperMain)";

/// The Gradle processes running now. Gradle locks its caches only among its
/// own processes and cleans them itself when its daemon stops (Gradle 9.8.1
/// "Dependency caching", "Gradle User Home and Cache Cleanup"), so devtrim,
/// which takes no part in that locking, refuses them while any Gradle process
/// runs. A probe that fails refuses as well. `-a` keeps a Gradle build that
/// started devtrim visible to the probe.
fn gradle_processes() -> Result<Vec<u32>> {
    let output = std::process::Command::new("pgrep")
        .args(["-a", "-f", GRADLE_PROCESS_PATTERN])
        .output_within(PROBE_TIMEOUT)
        .context("cannot run the Gradle process probe")?;
    crate::safety::parse_pgrep_pids(&output.stdout, output.status.code())
        .context("the Gradle process probe failed")
}

/// Gradle's caches and Wrapper distributions, unless a Gradle process runs.
fn gradle_findings(ctx: &Ctx, unread: &mut Vec<UnreadFolder>) -> Vec<Finding> {
    let gradle = match gradle_user_home(&ctx.home) {
        Ok(Some(gradle)) => gradle,
        Ok(None) => {
            ctx.diagnostic(
                "warn",
                "GRADLE_USER_HOME is not a folder inside the home folder; skipping Gradle's caches",
            );
            return Vec::new();
        }
        Err(error) => return vec![not_judged("Gradle caches", &error)],
    };
    match gradle_home_in_repository(&gradle, &ctx.home) {
        Ok(false) => {}
        Ok(true) => {
            ctx.diagnostic(
                "warn",
                format!(
                    "the Gradle user home {} is inside a Git repository; skipping Gradle's caches",
                    gradle.display()
                ),
            );
            return Vec::new();
        }
        Err(error) => return vec![not_judged("Gradle caches", &error)],
    }
    let mut present = Vec::new();
    for entry in GRADLE {
        let path = gradle.join(entry.relative);
        match std::fs::symlink_metadata(&path) {
            Ok(_) => match holds_gradle_layout(entry.relative, &path) {
                Ok(true) => present.push((entry.label, path)),
                Ok(false) => {}
                Err(error) => unread.push(UnreadFolder::new(&path, &error)),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => unread.push(UnreadFolder::new(&path, &error.into())),
        }
    }
    if present.is_empty() {
        return Vec::new();
    }
    match gradle_processes() {
        Ok(running) if running.is_empty() => {}
        Ok(running) => {
            ctx.diagnostic(
                "info",
                format!(
                    "Gradle is running ({} process(es)); skipping its caches until `gradle --stop` ends its daemons",
                    running.len()
                ),
            );
            return Vec::new();
        }
        Err(error) => return vec![not_judged("Gradle caches", &error)],
    }
    present
        .into_iter()
        .filter_map(|(label, path)| {
            let size = measured(&path, unread);
            (size > 0).then(|| cache_finding(label, path, size, 3))
        })
        .collect()
}

/// The Go module cache `go env GOMODCACHE` reports (go.dev/ref/mod "Module
/// cache": by default `$GOPATH/pkg/mod`; `go clean -modcache` is Go's own
/// removal, and modules missing from it are downloaded again). It is judged
/// like the other owner-reported roots: resolved first, and trusted only inside
/// the home folder and only when it holds Go's download cache
/// (`cache/download`), the layout that reference documents, so a `go` answering
/// with any other folder offers nothing. `GOTOOLCHAIN=local` keeps the query on
/// the installed toolchain: a `go.mod` asking for a newer Go would otherwise
/// make `go` download one (go.dev/doc/toolchain). `go` absent, or no cache yet,
/// is nothing to offer; `go` failing is an error for this cache alone.
fn go_module_cache(ctx: &Ctx, warn: bool) -> Result<Option<PathBuf>> {
    let Some(path) = command_path(
        "go",
        &["env", "GOMODCACHE"],
        &ctx.home,
        &[("GOTOOLCHAIN", "local")],
    )?
    else {
        return Ok(None);
    };
    let path = resolved_owner_path("go", path)?;
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", path.display()));
        }
        Ok(_) => {}
    }
    let corroborated = holds_go_download_cache(&path)?;
    let inside_home = normalized_absolute(&path)
        .is_some_and(|cache| cache.starts_with(&ctx.home) && cache != ctx.home);
    if !(corroborated && inside_home) {
        if warn {
            ctx.diagnostic(
                "warn",
                format!(
                    "`go` reports module cache {} outside the home folder or without Go's `cache/download`; skipping",
                    path.display()
                ),
            );
        }
        return Ok(None);
    }
    Ok(Some(path))
}

/// Whether `path` holds Go's download cache, `cache/download`, as a real folder.
fn holds_go_download_cache(path: &Path) -> Result<bool> {
    let download = path.join("cache/download");
    match std::fs::symlink_metadata(&download) {
        Ok(metadata) => Ok(metadata.is_dir()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("cannot inspect {}", download.display())),
    }
}

/// Go makes most module-cache folders read-only, so `rm -r` "fails, since files
/// can't be removed without first making their parent directories writable"
/// (go.dev/ref/mod "go clean -modcache"). A move to the Trash renames the cache
/// folder alone, which needs only that folder to be writable. A permanent
/// removal deletes every file; the sink's preflight refuses it whole when any
/// folder in the tree is read-only, before anything is removed.
fn go_removal_preflight(target: &Path) -> Result<()> {
    let writable = |path: &Path| match rustix::fs::access(path, rustix::fs::Access::WRITE_OK) {
        Ok(()) => Ok(true),
        Err(rustix::io::Errno::ACCESS) => Ok(false),
        Err(error) => Err(std::io::Error::from(error))
            .with_context(|| format!("cannot check whether {} is writable", path.display())),
    };
    if !writable(target)? {
        anyhow::bail!(
            "refusing the Go module cache: {} is read-only, so it cannot be moved; `go clean -modcache` removes it",
            target.display()
        );
    }
    Ok(())
}

/// Whether `cache` is Homebrew's standard cache folder, the only one split.
fn is_standard_brew_cache(cache: &Path, home: &Path) -> bool {
    normalized_absolute(cache)
        .zip(normalized_absolute(&home.join("Library/Caches/Homebrew")))
        .is_some_and(|(cache, standard)| cache == standard)
}

/// The shape `brew_cache_findings` offers a child in: directly inside the
/// standard Homebrew cache, neither a link nor a Git repository.
fn is_brew_cache_entry(target: &Path, home: &Path) -> Result<bool> {
    let Some(parent) = target.parent() else {
        return Ok(false);
    };
    if !is_standard_brew_cache(parent, home) {
        return Ok(false);
    }
    let metadata = std::fs::symlink_metadata(target)
        .with_context(|| format!("cannot inspect {}", target.display()))?;
    if metadata.file_type().is_symlink() {
        return Ok(false);
    }
    Ok(!(metadata.is_dir() && has_git_marker(target)?))
}

fn is_builtin_cache_root(path: &Path, home: &Path) -> bool {
    CACHES.iter().any(|entry| path == home.join(entry.relative))
        || library_caches(home).any(|(_, candidate)| path == candidate)
}
/// Owner-reported paths are trusted only inside the owner's exact cache namespace.
fn is_eligible_owner_cache(program: &str, path: &Path, home: &Path) -> bool {
    let (Some(path), Some(home)) = (normalized_absolute(path), normalized_absolute(home)) else {
        return false;
    };
    match program {
        "npm" => path.starts_with(home.join(".npm")) || path.starts_with(home.join(".cache/npm")),
        "brew" => path.starts_with(home.join("Library/Caches/Homebrew")),
        _ => false,
    }
}

fn normalized_absolute(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => return None,
            std::path::Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    Some(normalized)
}

/// The home folder is resolved to its real path; an owner answers in the
/// spelling `$HOME` gave it, which may run through a link, so its answer is
/// resolved the same way before it is judged. A cache that does not exist yet
/// holds nothing to offer either way.
fn resolved_owner_path(program: &str, path: PathBuf) -> Result<PathBuf> {
    match path.canonicalize() {
        Ok(real) => Ok(real),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(path),
        Err(error) => {
            Err(error).with_context(|| format!("cannot resolve the cache root `{program}` reports"))
        }
    }
}

fn owner_cache_path(program: &str, args: &[&str], ctx: &Ctx) -> Result<Option<PathBuf>> {
    let Some(path) = command_path(program, args, &ctx.home, &[])? else {
        return Ok(None);
    };
    let path = resolved_owner_path(program, path)?;
    if !is_eligible_owner_cache(program, &path, &ctx.home) {
        ctx.diagnostic(
            "warn",
            format!(
                "`{program}` reports cache root {} outside a home cache location; skipping",
                path.display()
            ),
        );
        return Ok(None);
    }
    Ok(Some(path))
}

/// Runs from `$HOME` so a project-level `.npmrc` (or `go.mod`) in whatever
/// directory devtrim was started from cannot redirect the reported cache root.
fn command_path(
    program: &str,
    args: &[&str],
    home: &Path,
    envs: &[(&str, &str)],
) -> Result<Option<PathBuf>> {
    let command = format!("`{program} {}`", args.join(" "));
    let output = std::process::Command::new(program)
        .args(args)
        .envs(envs.iter().copied())
        .current_dir(home)
        .output_within(QUERY_TIMEOUT);
    let Some(value) = super::optional_command_stdout(output, &command)? else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(PathBuf::from(value)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_cache_roots_are_program_specific_and_normalized() {
        let home = Path::new("/Users/example");
        for path in [
            home.join(".npm"),
            home.join(".npm/_cacache"),
            home.join(".cache/npm"),
            home.join(".cache/npm/content"),
        ] {
            assert!(is_eligible_owner_cache("npm", &path, home));
        }
        for path in [
            home.join("Library/Caches/Homebrew"),
            home.join("Library/Caches/Homebrew/downloads"),
        ] {
            assert!(is_eligible_owner_cache("brew", &path, home));
        }
        for path in [
            home.to_path_buf(),
            home.join(".cache"),
            home.join(".cache/uv"),
            home.join(".aws"),
            home.join(".config"),
            home.join(".local"),
            home.join("Documents"),
            home.join("Library/Caches"),
            home.join(".npm/../.ssh"),
            home.join(".cache/npm/../../.ssh"),
            PathBuf::from("/tmp/cache"),
            PathBuf::from(".npm"),
        ] {
            assert!(!is_eligible_owner_cache("npm", &path, home));
            assert!(!is_eligible_owner_cache("brew", &path, home));
        }
        assert!(!is_eligible_owner_cache("brew", &home.join(".npm"), home));
        assert!(!is_eligible_owner_cache(
            "npm",
            &home.join("Library/Caches/Homebrew"),
            home,
        ));
    }
    #[test]
    fn apply_reasserts_owner_namespace_and_preserves_sentinel() {
        let fixture = crate::ops::TestFixture::new("devtrim-cache-auth");
        let home = fixture.path().to_path_buf();
        std::fs::create_dir_all(home.join(".aws")).unwrap();
        let home = home.canonicalize().unwrap();
        let sentinel = home.join(".aws/sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let finding = Finding::new(
            "forged npm cache",
            Some(home.join(".aws")),
            4,
            "test",
            9,
            Action::Shred,
        )
        .with_authority(TargetAuthority::NpmCache);
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            roots_origin: crate::safety::RootsOrigin::Default,
            active_days: 30,
            retain_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home: home.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Stderr,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };

        let outcome = Caches.apply(&[finding], &ctx).unwrap();
        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
        assert!(sentinel.exists());
        crate::ops::remove_test_path(home);
    }

    /// Homebrew keeps a Git clone per Git-sourced formula inside its cache
    /// (`<name>--git`). The sink never removes a repository, so offering the
    /// cache whole was always refused (observed: 1.1 GB stranded by
    /// `omlx--git`). With a clone inside, the cache is offered as its other
    /// direct children; the clones stay, and apply refuses an entry that is a
    /// clone or a link. Without one, the cache is still one finding.
    #[test]
    fn a_homebrew_cache_holding_a_git_clone_is_offered_around_it() {
        let fixture = crate::ops::TestFixture::new("devtrim-brew-split");
        std::fs::create_dir_all(fixture.path()).unwrap();
        let home = fixture.path().canonicalize().unwrap();
        let cache = home.join("Library/Caches/Homebrew");
        std::fs::create_dir_all(cache.join("downloads")).unwrap();
        std::fs::write(
            cache.join("downloads/abc--tool--1.0.bottle.tar.gz"),
            vec![b'x'; 64],
        )
        .unwrap();
        std::fs::create_dir_all(cache.join("api")).unwrap();
        std::fs::write(cache.join("api/formula.jws.json"), "{}").unwrap();
        std::fs::create_dir_all(cache.join("Cask")).unwrap();
        let clone = cache.join("tool--git");
        std::fs::create_dir_all(clone.join(".git")).unwrap();
        std::fs::write(clone.join("README"), "source").unwrap();
        std::os::unix::fs::symlink(
            cache.join("downloads/abc--tool--1.0.bottle.tar.gz"),
            cache.join("tool--1.0"),
        )
        .unwrap();

        let findings = brew_cache_findings(&cache, &home, &mut Vec::new()).unwrap();
        let mut offered = findings
            .iter()
            .filter_map(Finding::target)
            .collect::<Vec<_>>();
        offered.sort();
        assert_eq!(
            offered,
            vec![
                cache.join("api").as_path(),
                cache.join("downloads").as_path()
            ],
            "PV caches/brew-clone-scan: the clone or an empty entry was offered"
        );
        assert!(
            findings
                .iter()
                .all(|finding| finding.authority() == TargetAuthority::BrewCacheEntry)
        );

        let plain = home.join("plain/Library/Caches/Homebrew");
        std::fs::create_dir_all(plain.join("downloads")).unwrap();
        std::fs::write(plain.join("downloads/a.tar.gz"), "x").unwrap();
        let whole = brew_cache_findings(&plain, &home.join("plain"), &mut Vec::new()).unwrap();
        assert_eq!(
            whole.iter().filter_map(Finding::target).collect::<Vec<_>>(),
            vec![plain.as_path()]
        );
        assert_eq!(whole[0].authority(), TargetAuthority::BrewCache);

        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            roots_origin: crate::safety::RootsOrigin::Default,
            active_days: 30,
            retain_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home: home.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };
        let entry = |path: PathBuf| {
            Finding::new(
                "forged homebrew entry",
                Some(path),
                4,
                "test",
                5,
                Action::Shred,
            )
            .with_authority(TargetAuthority::BrewCacheEntry)
        };
        std::fs::create_dir_all(clone.join("src")).unwrap();
        std::fs::write(clone.join("src/lib.rs"), "source").unwrap();
        for (forged, marker) in [
            (cache.join("tool--1.0"), "PV caches/brew-entry-apply"),
            (clone.clone(), "PV caches/brew-entry-repository"),
            (clone.join("src"), "PV caches/brew-entry-direct-child"),
        ] {
            let refused = Caches.apply(&[entry(forged.clone())], &ctx).unwrap();
            assert!(
                refused.summary.items_touched == 0
                    && refused.errors.len() == 1
                    && refused.errors[0].contains("outside its authorized namespace")
                    && std::fs::symlink_metadata(&forged).is_ok()
                    && clone.join("README").exists(),
                "{marker}: {} was not refused by the entry authority: {refused:?}",
                forged.display()
            );
        }
        let removed = Caches.apply(&[entry(cache.join("api"))], &ctx).unwrap();
        assert!(removed.errors.is_empty(), "{removed:?}");
        assert!(!cache.join("api").exists() && clone.join("README").exists());
    }

    /// The const assertion above rejects empty and ASCII-whitespace evidence
    /// while compiling, so those cases never reach a test — the crate does not
    /// build at all. What is left for a test is the gap that assertion cannot
    /// see: `is_ascii_whitespace` accepts non-ASCII blanks such as U+00A0, which
    /// `str::trim` strips. That is the only case this loop can catch, and
    /// `scripts/tests/planted-violations.py` (`evidence/built-in-caches`) plants exactly
    /// it.
    #[test]
    fn every_built_in_cache_carries_evidence() {
        for entry in CACHES {
            assert!(
                !entry.evidence.trim().is_empty(),
                "PV evidence/built-in-caches: missing deletion evidence: {}",
                entry.relative
            );
        }
    }

    /// The command table can never name a cache the carve-out does not list.
    #[test]
    fn every_command_rebuilt_cache_is_a_managed_library_cache() {
        for (relative, _) in crate::safety::REBUILT_BY_COMMAND {
            assert!(
                crate::safety::MANAGED_LIBRARY_CACHES
                    .iter()
                    .any(|entry| entry.relative == *relative),
                "{relative}"
            );
        }
    }

    /// Every running uv holds a shared lock on `<cache>/.lock`, and `uv cache
    /// clean` waits for it. Removal must honour the same lock rather than move
    /// the cache out from under a `uv sync` another session is running.
    #[test]
    fn uv_cache_is_refused_while_a_uv_process_holds_its_lock() {
        let fixture = crate::ops::TestFixture::new("devtrim-uv-lock");
        let home = fixture.path().to_path_buf();
        std::fs::create_dir_all(home.join(".cache/uv/sdists-v9")).unwrap();
        let home = home.canonicalize().unwrap();
        let uv = home.join(".cache/uv");
        std::fs::write(
            uv.join("CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n",
        )
        .unwrap();
        std::fs::write(uv.join("sdists-v9/.git"), "").unwrap();
        std::fs::write(uv.join("entry"), "cached").unwrap();
        let running_uv = std::fs::File::create(uv.join(".lock")).unwrap();
        rustix::fs::flock(&running_uv, rustix::fs::FlockOperation::LockShared).unwrap();
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            roots_origin: crate::safety::RootsOrigin::Default,
            active_days: 30,
            retain_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home: home.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };
        let plan = || {
            let mut findings = vec![cache_finding("uv package cache", uv.clone(), 6, 3)];
            crate::report::effective_actions(&mut findings, true);
            findings
        };

        let refused = Caches.apply(&plan(), &ctx).unwrap();

        assert_eq!(
            refused.summary.items_touched, 0,
            "PV caches/uv-lock: the uv cache was removed while a uv process held its lock"
        );
        assert!(
            refused
                .errors
                .iter()
                .any(|error| error.contains("uv is using its cache")),
            "{:?}",
            refused.errors
        );
        assert!(uv.join("entry").exists());

        // Unlock rather than just close: a child another test forks in parallel
        // shares this descriptor until it execs, and a `flock` lives as long as
        // any descriptor for it does.
        rustix::fs::flock(&running_uv, rustix::fs::FlockOperation::Unlock).unwrap();
        drop(running_uv);
        let removed = Caches.apply(&plan(), &ctx).unwrap();

        assert!(removed.errors.is_empty(), "{:?}", removed.errors);
        assert!(!uv.exists(), "with uv idle the cache must be removed");
        crate::ops::remove_test_path(home);
    }

    /// The lock is taken before the sink validates the target, so it must not
    /// follow a symlinked ancestor and create a file in the directory behind it.
    #[test]
    fn uv_lock_is_never_created_through_a_symlinked_ancestor() {
        let fixture = crate::ops::TestFixture::new("devtrim-uv-lock-link");
        let home = fixture.path().to_path_buf();
        std::fs::create_dir_all(home.join("elsewhere/uv")).unwrap();
        let home = home.canonicalize().unwrap();
        std::fs::write(home.join("elsewhere/uv/entry"), "cached").unwrap();
        std::os::unix::fs::symlink(home.join("elsewhere"), home.join(".cache")).unwrap();
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            roots_origin: crate::safety::RootsOrigin::Default,
            active_days: 30,
            retain_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home: home.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };
        let mut plan = vec![cache_finding(
            "uv package cache",
            home.join(".cache/uv"),
            6,
            3,
        )];
        crate::report::effective_actions(&mut plan, true);

        let outcome = Caches.apply(&plan, &ctx).unwrap();

        assert_eq!(outcome.summary.items_touched, 0);
        assert!(
            outcome
                .errors
                .iter()
                .any(|error| error.contains("symlinked ancestor")),
            "{:?}",
            outcome.errors
        );
        assert!(
            !home.join("elsewhere/uv/.lock").exists(),
            "PV caches/ancestor-lock: the lock was created through a symlinked ancestor"
        );
        assert!(home.join("elsewhere/uv/entry").exists());
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn standard_authority_rejects_forged_cache_subpath() {
        let fixture = crate::ops::TestFixture::new("devtrim-cache-standard");
        let home = fixture.path().to_path_buf();
        let forged = home.join(".cache/uv/nested");
        std::fs::create_dir_all(&forged).unwrap();
        let home = home.canonicalize().unwrap();
        let forged = home.join(".cache/uv/nested");
        let sentinel = forged.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let finding = Finding::new(
            "forged built-in cache",
            Some(forged),
            4,
            "test",
            9,
            Action::Shred,
        );
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            roots_origin: crate::safety::RootsOrigin::Default,
            active_days: 30,
            retain_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home: home.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Stderr,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };

        let outcome = Caches.apply(&[finding], &ctx).unwrap();

        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
        assert!(sentinel.exists());
        crate::ops::remove_test_path(home);
    }
}
