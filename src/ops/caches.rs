//! Regenerable download caches. Filesystem targets remain Trash-first.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::{Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, dir_size, removal_note};
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
        label: "node core cache",
        relative: ".cache/node",
        evidence: "Node's XDG cache. Corepack keeps its downloaded \
                   package-manager versions in `.cache/node/corepack`, so this \
                   one entry covers both and neither is listed twice.",
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
        for entry in CACHES {
            let path = ctx.home.join(entry.relative);
            let size = dir_size(&path)?;
            if size > 0 {
                findings.push(cache_finding(entry.label, path, size, 3));
            }
        }
        for (label, path) in library_caches(&ctx.home) {
            let size = dir_size(&path)?;
            if size > 0 {
                findings.push(cache_finding(label, path, size, 3));
            }
        }
        if let Some(path) = owner_cache_path("npm", &["config", "get", "cache"], ctx)? {
            let size = dir_size(&path)?;
            if size > 0 {
                findings.push(
                    cache_finding("npm download cache", path, size, 2)
                        .with_authority(TargetAuthority::NpmCache),
                );
            }
        }
        if let Some(path) = owner_cache_path("brew", &["--cache"], ctx)? {
            let size = dir_size(&path)?;
            if size > 0 {
                findings.push(
                    cache_finding("homebrew downloads cache", path, size, 1)
                        .with_authority(TargetAuthority::BrewCache),
                );
            }
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
                authorize_cache_finding(finding, &ctx.home)?;
                // Held until the cache has moved, then released on drop.
                let _uv_lock = finding
                    .target()
                    .filter(|target| *target == ctx.home.join(UV_CACHE))
                    .map(lock_uv_cache)
                    .transpose()?;
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

/// Every uv process holds a shared `flock` on `<cache>/.lock` while it uses
/// the cache, and `uv cache clean` takes it exclusively (uv 0.9.24
/// `crates/uv-cache/src/lib.rs:205-263,460`; `crates/uv-fs/src/locked_file.rs`
/// locks through std `File::lock`, which is `flock(2)` on macOS). Taking the
/// same lock without waiting refuses removal while uv runs, and holding it
/// until the move completes keeps a new uv process from starting in the tree.
/// Like uv, this creates the lock file when it is missing.
fn lock_uv_cache(root: &Path) -> Result<std::fs::File> {
    use rustix::fs::{FlockOperation, Mode, OFlags};

    // The lock is taken before the sink validates the target, so it must not
    // reach through a symlink and create a file in the directory behind it.
    let resolved = root
        .canonicalize()
        .with_context(|| format!("cannot resolve uv cache {}", root.display()))?;
    if resolved != root {
        anyhow::bail!(
            "refusing uv cache through a symlink or symlinked ancestor: {} resolves to {}",
            root.display(),
            resolved.display()
        );
    }
    let lock = root.join(".lock");
    let directory = rustix::fs::open(
        root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .with_context(|| format!("cannot open uv cache {}", root.display()))?;
    let fd = rustix::fs::openat(
        &directory,
        ".lock",
        OFlags::RDONLY | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o644),
    )
    .with_context(|| format!("cannot open uv's cache lock {}", lock.display()))?;
    let file = std::fs::File::from(fd);
    if !file
        .metadata()
        .with_context(|| format!("cannot inspect uv's cache lock {}", lock.display()))?
        .file_type()
        .is_file()
    {
        anyhow::bail!("uv's cache lock is not a regular file: {}", lock.display());
    }
    match rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(file),
        Err(rustix::io::Errno::WOULDBLOCK) => anyhow::bail!(
            "uv is using its cache (a uv process holds {}); retry after it exits",
            lock.display()
        ),
        Err(error) => Err(error).with_context(|| format!("cannot lock {}", lock.display())),
    }
}

fn cache_finding(label: &str, path: PathBuf, size: u64, danger: u8) -> Finding {
    Finding::new(
        label,
        Some(path),
        size,
        // Not every entry is a download: an editor index or a compiler cache is
        // rebuilt locally, and saying "re-downloads" would misdescribe the cost
        // of removing one.
        "regenerated automatically on next use; a large cache costs bandwidth or rebuild time",
        escalate(danger, size),
        Action::Trash,
    )
}

fn authorize_cache_finding(finding: &Finding, home: &Path) -> Result<()> {
    let target = finding
        .target()
        .ok_or_else(|| anyhow::anyhow!("cache finding missing internal target"))?;
    let authorized = match finding.authority() {
        TargetAuthority::Standard => is_builtin_cache_root(target, home),
        TargetAuthority::NpmCache => is_eligible_owner_cache("npm", target, home),
        TargetAuthority::BrewCache => is_eligible_owner_cache("brew", target, home),
    };
    if !authorized {
        anyhow::bail!(
            "cache target is outside its authorized namespace: {}",
            target.display()
        );
    }
    Ok(())
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

fn owner_cache_path(program: &str, args: &[&str], ctx: &Ctx) -> Result<Option<PathBuf>> {
    let Some(path) = command_path(program, args, &ctx.home)? else {
        return Ok(None);
    };
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

/// Runs from `$HOME` so a project-level `.npmrc` in whatever directory devtrim
/// was started from cannot redirect the reported cache root.
fn command_path(program: &str, args: &[&str], home: &Path) -> Result<Option<PathBuf>> {
    let command = format!("`{program} {}`", args.join(" "));
    let output = std::process::Command::new(program)
        .args(args)
        .current_dir(home)
        .output();
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
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-cache-auth-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
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
            active_days: 30,
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

    #[test]
    fn library_cache_authority_matches_the_protection_carve_out() {
        let home = Path::new("/Users/example");
        for (_, path) in library_caches(home) {
            assert!(is_builtin_cache_root(&path, home), "{}", path.display());
            assert!(!crate::safety::is_protected(&path, home));
        }
        for rejected in [
            home.join("Library/Caches"),
            home.join("Library/Caches/ms-playwright-extra"),
            home.join("Library/Caches/ms-playwright/browsers"),
            home.join("Library/Application Support"),
        ] {
            assert!(
                !is_builtin_cache_root(&rejected, home),
                "{}",
                rejected.display()
            );
        }
    }

    /// One refused cache must not abandon the rest of the previewed plan. Here a
    /// real worktree gitfile at the cache root trips the repository refusal;
    /// while apply stopped at the first failure, a refusal like this blocked
    /// every cache listed after it.
    #[test]
    fn a_refused_cache_does_not_block_the_rest_of_the_plan() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-cache-continue-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(home.join(".cache/uv")).unwrap();
        std::fs::create_dir_all(home.join(".cache/node")).unwrap();
        let home = home.canonicalize().unwrap();
        // A Git worktree marker makes this cache permanently unremovable.
        std::fs::write(home.join(".cache/uv/.git"), "gitdir: elsewhere\n").unwrap();
        std::fs::write(home.join(".cache/node/entry"), "regenerable").unwrap();
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home: home.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };
        let mut findings = vec![
            cache_finding("uv package cache", home.join(".cache/uv"), 4, 3),
            cache_finding("node core cache", home.join(".cache/node"), 4, 3),
        ];
        // Permanent, not Trash: a Trash move goes to the real user's Trash
        // whatever this test's home is, and left a directory there every run.
        crate::report::effective_actions(&mut findings, true);

        let outcome = Caches.apply(&findings, &ctx).unwrap();

        assert_eq!(
            outcome.errors.len(),
            1,
            "the refusal must still be reported"
        );
        assert_eq!(
            outcome.summary.items_touched, 1,
            "the cache listed after the refused one must still be removed"
        );
        assert!(home.join(".cache/uv/.git").exists());
        assert!(!home.join(".cache/node").exists());
        crate::ops::remove_test_path(home);
    }

    /// Every running uv holds a shared lock on `<cache>/.lock`, and `uv cache
    /// clean` waits for it. Removal must honour the same lock rather than move
    /// the cache out from under a `uv sync` another session is running.
    #[test]
    fn uv_cache_is_refused_while_a_uv_process_holds_its_lock() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-uv-lock-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
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
            active_days: 30,
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
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-uv-lock-link-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(home.join("elsewhere/uv")).unwrap();
        let home = home.canonicalize().unwrap();
        std::fs::write(home.join("elsewhere/uv/entry"), "cached").unwrap();
        std::os::unix::fs::symlink(home.join("elsewhere"), home.join(".cache")).unwrap();
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
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
    fn huggingface_authority_accepts_only_model_cache() {
        let home = Path::new("/Users/cache-test");
        for (relative, accepted) in [
            (".cache/huggingface/hub", true),
            (".cache/huggingface", false),
            (".cache/huggingface/token", false),
            (".cache/huggingface/stored_tokens", false),
        ] {
            let finding = cache_finding("huggingface", home.join(relative), 1, 3);
            assert_eq!(authorize_cache_finding(&finding, home).is_ok(), accepted);
        }
    }

    #[test]
    fn standard_authority_rejects_forged_cache_subpath() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-cache-standard-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
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
            active_days: 30,
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
