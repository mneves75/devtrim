//! Regenerable download caches. Filesystem targets remain Trash-first.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use super::{Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, dir_size, removal_note};
use crate::report::TargetAuthority;
use crate::safety::{Ctx, DeletionEntry, escalate};

pub struct Caches;

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
        relative: ".cache/uv",
        evidence: "uv's package cache, refilled on the next resolve. A source \
                   distribution here can carry its own `.git`, which the \
                   repository-root refusal then blocks — expected, not a bug.",
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
                apply_filesystem_finding(self.name(), finding, ctx)
            })()
            .with_context(|| format!("failed to remove {}", finding.label));
            // One refused cache must not abandon the rest of the previewed plan.
            // The list spans unrelated tools, and a single entry can be
            // permanently unremovable — a `uv` source distribution checked out
            // with its own `.git` trips the repository-root refusal on every
            // run — which would otherwise block every cache listed after it.
            // Each failure is still recorded, so the run reports nonzero.
            if let Err(error) = result {
                outcome.fail(error);
                continue;
            }
            outcome.record(finding, removal_note(finding, &finding.label));
        }
        Ok(outcome)
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
    let Some(path) = command_path(program, args)? else {
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

fn command_path(program: &str, args: &[&str]) -> Result<Option<PathBuf>> {
    let output = match std::process::Command::new(program).args(args).output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !output.status.success() {
        anyhow::bail!("`{program} {}` failed", args.join(" "));
    }
    let value = String::from_utf8(output.stdout).context("command returned non-UTF-8 path")?;
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

    /// The managed `~/Library/Caches` list is authority for exactly its own
    /// entries. Paired assertions: every listed root is accepted (so the list is
    /// live, not dead code) and a neighbour sharing a prefix is not.
    #[test]
    fn every_built_in_cache_carries_evidence() {
        for entry in CACHES {
            assert!(
                !entry.evidence.trim().is_empty(),
                "missing deletion evidence: {}",
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

    /// One unremovable cache must not abandon the rest of the previewed plan.
    /// Observed for real: a `uv` source distribution checked out with its own
    /// `.git` trips the repository-root refusal on every run, and while apply
    /// stopped at the first failure it blocked every cache listed after it.
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
        let findings = vec![
            cache_finding("uv package cache", home.join(".cache/uv"), 4, 3),
            cache_finding("node core cache", home.join(".cache/node"), 4, 3),
        ];

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
