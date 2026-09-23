//! Safety core: context, danger gating, protected paths, and size guards.

use anyhow::{Context, Result, bail};
use colored::Colorize;
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::io::IsTerminal;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use crate::cli::Cli;
use crate::ops::Finding;

pub const DATA_LOSS_NOTICE: &str = "Applying this plan can delete data. devtrim is provided AS IS, without warranties; you assume the risk for the exact targets shown. Keep backups and grant macOS permissions manually only when you understand the request.";

const PROTECTED: &[&str] = &[
    "/",
    "/System",
    "/bin",
    "/sbin",
    "/usr",
    "/etc",
    "/var",
    "/private/etc",
    "/private/var",
    "/Applications",
    "/Library",
    "/boot",
    "/dev",
    "/Volumes",
];

const PROTECTED_USER: &[&str] = &["Library", ".ssh", ".gnupg"];

pub(crate) fn is_git_metadata_name(name: &OsStr) -> bool {
    name.as_encoded_bytes().eq_ignore_ascii_case(b".git")
}

// `Ctx::protect` predates apply-time alias revalidation and is also built
// directly by tests in other modules. Keep its `Vec<PathBuf>` shape while
// tagging values loaded from config as literal/resolved pairs.
const CONFIG_PROTECT_SNAPSHOT_MARKER: &str = "devtrim:protect-snapshot:v1";

pub struct Ctx {
    pub yes: bool,
    pub yolo: bool,
    pub json: bool,
    pub roots: Vec<PathBuf>,
    pub active_days: u32,
    pub home: PathBuf,
    pub protect: Vec<PathBuf>,
    pub journal_path: PathBuf,
    pub interactive: bool,
    pub(crate) diagnostic_output: DiagnosticOutput,
    pub(crate) diagnostics: Mutex<Vec<String>>,
    /// Journal failures observed after a mutation already succeeded; drained
    /// into the apply outcome so automation sees them with a nonzero status.
    pub(crate) journal_errors: Mutex<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiagnosticOutput {
    Stderr,
    Capture,
}

impl Ctx {
    pub fn from_cli(cli: &Cli) -> Result<Self> {
        let home = dirs_home()?;
        let cfg = home.join(".config/devtrim.toml");
        let file_cfg = load_config(&cfg)?;
        let cfg_roots = file_cfg.roots.unwrap_or_default();
        let active_days = file_cfg.active_days.unwrap_or(30).max(1);
        let (protect, protect_warnings) =
            configured_protect(file_cfg.protect.unwrap_or_default(), &home)?;
        let explicit_roots = !cli.roots.is_empty() || !cfg_roots.is_empty();
        let roots = if !cli.roots.is_empty() {
            cli.roots
                .iter()
                .map(|root| PathBuf::from(shellexpand(root, &home)))
                .collect()
        } else if !cfg_roots.is_empty() {
            cfg_roots
                .iter()
                .map(|root| PathBuf::from(shellexpand(root, &home)))
                .collect()
        } else {
            vec![home.join("dev")]
        };
        // A mistyped root would otherwise scan nothing and report a clean
        // machine; the absent default `~/dev` is not a mistake worth a warning.
        let mut root_warnings = Vec::new();
        let roots = roots
            .into_iter()
            .map(|root| {
                if root.exists() {
                    root.canonicalize()
                        .with_context(|| format!("cannot resolve scan root: {}", root.display()))
                } else {
                    if explicit_roots {
                        root_warnings.push(format!(
                            "scan root does not exist and was skipped: {}",
                            root.display()
                        ));
                    }
                    Ok(root)
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let journal_path = journal_path(&home);
        let journal_warnings = match crate::journal::rotate_if_needed(&journal_path) {
            Ok(warnings) => warnings,
            Err(error) => vec![format!("cannot rotate apply journal: {error:#}")],
        };
        let ctx = Self {
            yes: cli.yes,
            yolo: cli.yolo,
            json: cli.json,
            roots,
            active_days,
            protect,
            journal_path,
            interactive: std::io::stdin().is_terminal(),
            home,
            diagnostic_output: if matches!(
                cli.command.as_ref(),
                None | Some(crate::cli::Command::Tui)
            ) {
                DiagnosticOutput::Capture
            } else {
                DiagnosticOutput::Stderr
            },
            diagnostics: Mutex::new(Vec::new()),
            journal_errors: Mutex::new(Vec::new()),
        };
        for warning in root_warnings {
            ctx.diagnostic("warn", warning);
        }
        for warning in protect_warnings {
            ctx.diagnostic("warn", warning);
        }
        for warning in journal_warnings {
            ctx.diagnostic("warn", warning);
        }
        Ok(ctx)
    }

    pub fn diagnostic(&self, level: &str, message: impl Into<String>) {
        let message = message.into();
        match self.diagnostic_output {
            DiagnosticOutput::Capture => self
                .diagnostics
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(format!("{level}: {message}")),
            DiagnosticOutput::Stderr => {
                let label = match level {
                    "info" => level.dimmed(),
                    _ => level.yellow(),
                };
                eprintln!("{} {}", label, crate::report::terminal_safe(&message));
            }
        }
    }

    pub fn take_diagnostics(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .diagnostics
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    pub(crate) fn record_journal_error(&self, message: String) {
        self.journal_errors
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message);
    }

    pub(crate) fn take_journal_errors(&self) -> Vec<String> {
        std::mem::take(
            &mut *self
                .journal_errors
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }
}

fn dirs_home() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| anyhow::anyhow!("$HOME not set"))?;
    home.canonicalize()
        .with_context(|| format!("cannot resolve $HOME: {}", home.display()))
}

fn shellexpand(value: &str, home: &Path) -> String {
    value.strip_prefix("~/").map_or_else(
        || value.to_string(),
        |rest| home.join(rest).display().to_string(),
    )
}

#[derive(serde::Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct FileCfg {
    roots: Option<Vec<String>>,
    active_days: Option<u32>,
    protect: Option<Vec<String>>,
}

fn load_config(path: &Path) -> Result<FileCfg> {
    match std::fs::read_to_string(path) {
        Ok(contents) => parse_config_str(&contents)
            .with_context(|| format!("invalid config: {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(FileCfg::default()),
        Err(error) => Err(error).with_context(|| format!("cannot read config: {}", path.display())),
    }
}

pub(crate) fn parse_config_str(contents: &str) -> Result<FileCfg> {
    toml::from_str(contents).map_err(Into::into)
}

fn configured_protect(entries: Vec<String>, home: &Path) -> Result<(Vec<PathBuf>, Vec<String>)> {
    let mut protect = Vec::with_capacity(entries.len().saturating_mul(2).saturating_add(1));
    let mut warnings = Vec::new();
    if !entries.is_empty() {
        protect.push(PathBuf::from(CONFIG_PROTECT_SNAPSHOT_MARKER));
    }
    for entry in entries {
        let expanded = PathBuf::from(shellexpand(&entry, home));
        if !expanded.is_absolute() {
            bail!("protect entry `{entry}` must expand to an absolute path");
        }
        let cleaned = clean(&expanded);
        let mut resolved_snapshot = cleaned.clone();
        // A protect entry that resolves to nothing usually means a typo, and a
        // typo in a safety valve must be loud, not silent.
        if cleaned.exists() {
            // Scanners report canonical paths, so a symlinked entry must also
            // match its resolved form; keep the literal spelling as well.
            let resolved = cleaned.canonicalize().with_context(|| {
                format!(
                    "cannot resolve protect entry `{entry}`: {}",
                    cleaned.display()
                )
            })?;
            resolved_snapshot = clean(&resolved);
        } else {
            warnings.push(format!(
                "protect entry `{entry}` does not currently resolve to an existing path"
            ));
        }
        protect.push(cleaned);
        protect.push(resolved_snapshot);
    }
    Ok((protect, warnings))
}

/// Journal location without loading cleanup configuration: recovery commands
/// must work even when `devtrim.toml` is malformed.
pub(crate) fn default_journal_path() -> Result<PathBuf> {
    Ok(journal_path(&dirs_home()?))
}

fn journal_path(home: &Path) -> PathBuf {
    let state_home = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".local/state"));
    clean(&state_home.join("devtrim/journal.jsonl"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileIdentity {
    pub(crate) dev: u64,
    pub(crate) ino: u64,
    #[cfg(target_os = "macos")]
    pub(crate) generation: u32,
}

impl FileIdentity {
    pub(crate) fn from_std_metadata(metadata: &std::fs::Metadata) -> Self {
        #[cfg(target_os = "macos")]
        use std::os::macos::fs::MetadataExt as _;
        use std::os::unix::fs::MetadataExt as _;

        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
            #[cfg(target_os = "macos")]
            generation: metadata.st_gen(),
        }
    }

    pub(crate) fn from_rustix_stat(metadata: &rustix::fs::Stat) -> Self {
        Self {
            dev: metadata.st_dev as u64,
            ino: metadata.st_ino,
            #[cfg(target_os = "macos")]
            generation: metadata.st_gen,
        }
    }
}

/// A pathname that passed the deletion boundary's current safety checks.
///
/// Validation resolves the parent before the sink opens its directory handle;
/// target identity is rechecked through that handle before deletion.
#[derive(Debug)]
pub(crate) struct VerifiedTarget(PathBuf);

impl VerifiedTarget {
    pub(crate) fn into_path(self) -> PathBuf {
        self.0
    }
}

pub(crate) fn validate_path_for_deletion(
    path: &Path,
    home: &Path,
    protect: &[PathBuf],
) -> Result<VerifiedTarget> {
    revalidate_configured_protect_aliases(protect)?;
    let literal = abs(path);
    if path_contains_git_metadata_component(&literal) {
        bail!("refusing path inside Git metadata: {}", literal.display());
    }
    if is_protected(&literal, home) {
        bail!("refusing protected path: {}", literal.display());
    }
    if is_config_protected_abs(&literal, protect) {
        bail!("refusing configured protected path: {}", literal.display());
    }
    let parent = literal
        .parent()
        .ok_or_else(|| anyhow::anyhow!("refusing path without parent: {}", literal.display()))?;
    let leaf = literal
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("refusing path without leaf: {}", literal.display()))?;
    let resolved_parent = parent
        .canonicalize()
        .with_context(|| format!("cannot verify parent: {}", parent.display()))?;
    let resolved = clean(&resolved_parent.join(leaf));
    if is_config_protected_abs(&resolved, protect) {
        bail!(
            "refusing configured protected resolved path: {}",
            resolved.display()
        );
    }
    if resolved != literal {
        bail!(
            "refusing path through symlinked ancestor: {} -> {}",
            literal.display(),
            resolved.display()
        );
    }
    if is_protected_abs(&resolved, home) {
        bail!("refusing protected resolved path: {}", resolved.display());
    }
    refuse_git_repository_root(&resolved)?;
    Ok(VerifiedTarget(literal))
}

fn revalidate_configured_protect_aliases(protect: &[PathBuf]) -> Result<()> {
    if protect.first().map(PathBuf::as_path) != Some(Path::new(CONFIG_PROTECT_SNAPSHOT_MARKER)) {
        return Ok(());
    }
    let snapshots = &protect[1..];
    let (pairs, remainder) = snapshots.as_chunks::<2>();
    if !remainder.is_empty() {
        bail!("invalid configured protect snapshot; refusing deletion");
    }
    for pair in pairs {
        let literal = &pair[0];
        let expected = &pair[1];
        let resolved = match literal.canonicalize() {
            Ok(resolved) => resolved,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && literal == expected => {
                continue;
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "cannot re-resolve protect symlink alias: {}",
                        literal.display()
                    )
                });
            }
        };
        let resolved = clean(&resolved);
        if &resolved != expected {
            bail!(
                "protect symlink alias changed: {} resolved to {}, expected {}; refusing deletion",
                literal.display(),
                resolved.display(),
                expected.display()
            );
        }
    }
    Ok(())
}

fn configured_protect_values(protect: &[PathBuf]) -> &[PathBuf] {
    if protect.first().map(PathBuf::as_path) == Some(Path::new(CONFIG_PROTECT_SNAPSHOT_MARKER)) {
        &protect[1..]
    } else {
        protect
    }
}

fn path_contains_git_metadata_component(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(component, std::path::Component::Normal(name) if is_git_metadata_name(name))
    })
}

fn refuse_git_repository_root(path: &Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("cannot inspect deletion target: {}", path.display()));
        }
    };
    if !metadata.is_dir() {
        return Ok(());
    }
    if directory_contains_git_metadata_entry(path)? {
        bail!("refusing Git repository/worktree root: {}", path.display());
    }
    Ok(())
}

fn directory_contains_git_metadata_entry(path: &Path) -> Result<bool> {
    for entry in std::fs::read_dir(path).with_context(|| {
        format!(
            "cannot read directory for Git marker check: {}",
            path.display()
        )
    })? {
        let entry = entry
            .with_context(|| format!("cannot read directory entry under {}", path.display()))?;
        if is_git_metadata_name(&entry.file_name()) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn is_config_protected(path: &Path, protect: &[PathBuf]) -> bool {
    let literal = abs(path);
    if is_config_protected_abs(&literal, protect) {
        return true;
    }
    let Some(parent) = literal.parent() else {
        return false;
    };
    let Some(leaf) = literal.file_name() else {
        return false;
    };
    parent
        .canonicalize()
        .map(|resolved_parent| clean(&resolved_parent.join(leaf)))
        .is_ok_and(|resolved| is_config_protected_abs(&resolved, protect))
}

fn is_config_protected_abs(path: &Path, protect: &[PathBuf]) -> bool {
    // Intersection in either direction is refused: deleting an ancestor of a
    // protected entry would remove the protected descendant with it.
    configured_protect_values(protect).iter().any(|protected| {
        path_is_or_under_protect_entry(path, protected)
            || path_is_or_under_protect_entry(protected, path)
    })
}

// Config strings are typically NFC while macOS directory entries are often NFD,
// so protect matching must be Unicode-normalization-insensitive: raw byte
// comparison silently fails to protect `café`. Deny-only, so a wider match can
// only refuse more, never authorize more.
fn path_is_or_under_protect_entry(path: &Path, entry: &Path) -> bool {
    let mut path_components = path.components();
    for expected in entry.components() {
        let Some(actual) = path_components.next() else {
            return false;
        };
        if !protect_component_matches(actual.as_os_str(), expected.as_os_str()) {
            return false;
        }
    }
    true
}

fn protect_component_matches(actual: &std::ffi::OsStr, expected: &std::ffi::OsStr) -> bool {
    use unicode_normalization::UnicodeNormalization;
    match (actual.to_str(), expected.to_str()) {
        (Some(actual), Some(expected)) => {
            let actual: String = actual.nfc().collect();
            let expected: String = expected.nfc().collect();
            actual.as_bytes().eq_ignore_ascii_case(expected.as_bytes())
        }
        _ => actual
            .as_encoded_bytes()
            .eq_ignore_ascii_case(expected.as_encoded_bytes()),
    }
}

pub fn validate_trash_root(home: &Path) -> Result<PathBuf> {
    let dir = clean(&home.join(".Trash"));
    let metadata = std::fs::symlink_metadata(&dir)
        .with_context(|| format!("cannot inspect Trash: {}", dir.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("refusing unverified Trash directory: {}", dir.display());
    }
    let resolved = dir
        .canonicalize()
        .with_context(|| format!("cannot resolve Trash: {}", dir.display()))?;
    if resolved != dir {
        bail!(
            "refusing Trash through symlinked ancestor: {} -> {}",
            dir.display(),
            resolved.display()
        );
    }
    Ok(dir)
}

pub fn is_protected(path: &Path, home: &Path) -> bool {
    is_protected_abs(&abs(path), home)
}

fn is_protected_abs(path: &Path, home: &Path) -> bool {
    if path_eq_ignore_ascii_case(path, home)
        || path_eq_ignore_ascii_case(path, &home.join(".Trash"))
    {
        return true;
    }
    for protected in PROTECTED {
        let protected = Path::new(protected);
        if path_eq_ignore_ascii_case(path, protected)
            || (protected != Path::new("/") && path_starts_with_ignore_ascii_case(path, protected))
        {
            return true;
        }
    }
    let Some(relative) = path_relative_to_ignore_ascii_case(path, home) else {
        return false;
    };
    let Some(first) = relative.iter().next() else {
        return false;
    };
    if let Some(protected) = PROTECTED_USER.iter().find(|protected| {
        first
            .as_encoded_bytes()
            .eq_ignore_ascii_case(protected.as_bytes())
    }) {
        if *protected == "Library" {
            return !is_managed_library_subpath(&relative);
        }
        return true;
    }
    false
}

fn path_eq_ignore_ascii_case(left: &Path, right: &Path) -> bool {
    path_relative_to_ignore_ascii_case(left, right)
        .is_some_and(|relative| relative.as_os_str().is_empty())
}

fn path_starts_with_ignore_ascii_case(path: &Path, base: &Path) -> bool {
    path_relative_to_ignore_ascii_case(path, base).is_some()
}

fn path_relative_to_ignore_ascii_case(path: &Path, base: &Path) -> Option<PathBuf> {
    let mut path_components = path.components();
    for expected in base.components() {
        let actual = path_components.next()?;
        if !actual
            .as_os_str()
            .as_encoded_bytes()
            .eq_ignore_ascii_case(expected.as_os_str().as_encoded_bytes())
        {
            return None;
        }
    }
    Some(
        path_components
            .map(|component| component.as_os_str())
            .collect(),
    )
}

/// One path a category may delete, and the evidence that says it may.
///
/// `evidence` is a required field, so a new entry cannot be added without one:
/// omission is a compile error rather than something review has to notice. Six
/// entries reached a release justified only by a directory *name* that looked
/// like a cache — `~/.claude/projects`, `~/.claude/jobs`, `Caches/JetBrains`,
/// `Caches/deno`, `Caches/claude-cli-nodejs`, `~/.claude/downloads` — and every
/// one of them turned out to hold something the owner does not regenerate.
///
/// What the field can and cannot do is worth stating plainly: the compiler
/// forces evidence to exist, and [`evidence_is_present`] forces it to say
/// something. Neither can judge whether the cited source actually supports
/// deleting that path. That remains a review requirement, per
/// `AGENTS.md § Conventions`.
pub(crate) struct DeletionEntry {
    pub(crate) label: &'static str,
    /// Path relative to the list's own root — `$HOME` for the category lists,
    /// `~/Library/Caches` for [`MANAGED_LIBRARY_CACHES`].
    pub(crate) relative: &'static str,
    /// What the directory holds, why removal is appropriate, and the source
    /// that establishes it. Where no vendor documentation exists, say so and
    /// name what was observed instead.
    pub(crate) evidence: &'static str,
}

/// Whether every entry carries non-empty, non-whitespace evidence.
///
/// `const` so each list is checked while compiling. It sees ASCII whitespace
/// only: the owning modules' tests cover a non-ASCII blank such as U+00A0,
/// which `str::trim` strips and a `const fn` cannot.
pub(crate) const fn evidence_is_present(entries: &[DeletionEntry]) -> bool {
    let mut index = 0;
    while index < entries.len() {
        if !evidence_is_meaningful(entries[index].evidence) {
            return false;
        }
        index += 1;
    }
    true
}

/// Whether one evidence string says anything at all. Whitespace does not count,
/// so `" "` fails the same way `""` does: the point is to make a missing
/// justification impossible to ship, and a space is a missing justification.
pub(crate) const fn evidence_is_meaningful(evidence: &str) -> bool {
    let bytes = evidence.as_bytes();
    let mut byte = 0;
    while byte < bytes.len() {
        if !bytes[byte].is_ascii_whitespace() {
            return true;
        }
        byte += 1;
    }
    false
}

const _: () = assert!(
    evidence_is_present(MANAGED_LIBRARY_CACHES),
    "every managed Library cache needs evidence for why it may be deleted"
);

/// Exact `~/Library/Caches` subdirectories devtrim manages, each a
/// [`DeletionEntry`] whose `relative` is the directory name.
///
/// `~/Library` is protected wholesale; this is the closed carve-out, and it is
/// the single source of truth for both the protection boundary below and the
/// cache category that acts on it, so the two cannot drift into a state where a
/// path is previewed but refused (or worse, the reverse). Every entry is one
/// directory owned by exactly one developer tool that rebuilds it on demand. A
/// name shared by several producers, or one whose contents a tool cannot
/// re-fetch, does not belong here.
///
/// The pnpm entry is the metadata cache under a cache root, never the
/// content-addressable store (`~/Library/pnpm/store`): every installed
/// `node_modules` hard-links into that store, so removing it would break
/// projects rather than free regenerable bytes.
pub(crate) const MANAGED_LIBRARY_CACHES: &[DeletionEntry] = &[
    DeletionEntry {
        label: "Playwright browser cache",
        relative: "ms-playwright",
        evidence: "Playwright's own docs describe this as the downloaded browser \
                   location, re-created by `npx playwright install`.",
    },
    DeletionEntry {
        label: "VS Code update staging cache",
        relative: "com.microsoft.VSCode.ShipIt",
        evidence: "Squirrel.Mac update staging. Regenerated on the next update \
                   check; removing it mid-update interrupts that update.",
    },
    DeletionEntry {
        label: "VS Code cache",
        relative: "com.microsoft.VSCode",
        evidence: "Electron/Chromium HTTP cache. VS Code keeps user data and \
                   state under `~/Library/Application Support/Code`, not here.",
    },
    DeletionEntry {
        label: "SwiftPM cache",
        relative: "org.swift.swiftpm",
        evidence: "Shared manifest/repository cache and the package-collection \
                   index. Authoritative configuration lives outside Caches, in \
                   `~/Library/org.swift.swiftpm/configuration`.",
    },
    // `Caches/JetBrains` is deliberately absent. On macOS that is the IDE
    // *system directory*, not a cache: each `<Product><Version>` subdirectory
    // holds `LocalHistory`, the per-file change history the IDE keeps for files
    // Git never saw. Nothing regenerates it, and JetBrains stopped clearing it
    // on "Invalidate Caches" for that reason. Carving out only the `caches` and
    // `index` subdirectories would need a per-product depth rule, which this
    // exact-name list cannot express.
    // `Caches/claude-cli-nodejs` is deliberately absent. Despite living under
    // `Caches` it holds per-project `mcp-logs-<server>/` diagnostic logs, which
    // nothing regenerates — deleting them loses MCP debugging history rather
    // than costing a re-fetch. Undocumented by the vendor, so its contents
    // cannot be characterised with confidence either.
    DeletionEntry {
        label: "pip package cache",
        relative: "pip",
        evidence: "pip's caching documentation describes the HTTP and wheel \
                   cache here, cleared by `pip cache purge`.",
    },
    DeletionEntry {
        label: "pnpm metadata cache",
        relative: "pnpm",
        evidence: "pnpm's `cacheDir` (metadata and dlx). Deliberately NOT the \
                   content-addressable store at `~/Library/pnpm/store`, which \
                   every installed `node_modules` hard-links into.",
    },
    // `Caches/gh` is deliberately absent, and it is the reason this field
    // exists. It shipped in 0.9.1 and 0.9.2 on the strength of its name: go-gh
    // resolves the CLI's cache to `$XDG_CACHE_HOME/gh` or `~/.cache/gh` and
    // never to `~/Library/Caches` by default, so the directory devtrim was
    // authorizing belongs to an unidentified owner unless `XDG_CACHE_HOME`
    // happens to point here. gh's real cache is listed in `caches::CACHES`.
    // `Caches/deno` is deliberately absent. On macOS it is `DENO_DIR`, not a
    // module cache alone: `location_data/<hash>/kv.sqlite3` is where every
    // `Deno.openKv()` opened without an explicit path stores its database, and
    // the sibling `local_storage` file backs `localStorage`. Both are documented
    // as persistent across runs and nothing rebuilds them. An exact-name list
    // cannot express a `location_data` exclusion.
    // Go's own `go help cache` says clearing it should not be necessary in
    // typical use. The one part that is not a pure rebuild is the fuzz corpus
    // kept beneath it: those coverage-expanding inputs come back only by
    // fuzzing again.
    DeletionEntry {
        label: "Go build cache",
        relative: "go-build",
        evidence: "`go help cache`: clearing it should not be necessary in \
                   typical use, i.e. it rebuilds. The fuzz corpus kept beneath \
                   it is the one part that returns only by fuzzing again.",
    },
    DeletionEntry {
        label: "TypeScript server cache",
        relative: "typescript",
        evidence: "Automatic type-acquisition cache (`<version>/node_modules/\
                   @types`). Clearing it is the vendor's documented fix for a \
                   corrupt acquisition.",
    },
];

fn is_managed_library_subpath(relative: &Path) -> bool {
    const MANAGED: &[&str] = &[
        "Developer/Toolchains",
        "Developer/Xcode/iOS DeviceSupport",
        "Developer/Xcode/DerivedData",
        "Caches/Homebrew",
    ];
    let mut components = relative.iter();
    if components
        .next()
        .map(|part| part != "Library")
        .unwrap_or(true)
    {
        return false;
    }
    let owned = components.collect::<PathBuf>();
    let owned = owned.to_string_lossy();
    let covered = |managed: &str| owned == managed || owned.starts_with(&format!("{managed}/"));
    MANAGED.iter().copied().any(covered)
        || MANAGED_LIBRARY_CACHES
            .iter()
            .any(|entry| covered(&format!("Caches/{}", entry.relative)))
}

fn abs(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return clean(path);
    }
    std::env::current_dir()
        .map(|cwd| clean(&cwd.join(path)))
        .unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn clean(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                output.pop();
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

pub fn escalate(danger: u8, total_bytes: u64) -> u8 {
    const GB: u64 = 1024 * 1024 * 1024;
    let danger = if total_bytes > 50 * GB {
        danger.max(8)
    } else if total_bytes > 10 * GB {
        danger.max(7)
    } else if total_bytes > GB {
        danger.max(5)
    } else {
        danger
    };
    danger.min(10)
}

pub fn plan_danger(findings: &[Finding]) -> u8 {
    let base = findings
        .iter()
        .filter(|finding| finding.action.is_actionable())
        .map(|finding| finding.danger)
        .max()
        .unwrap_or(1);
    escalate(base, crate::report::actionable_bytes(findings))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationRequirement {
    YesNo { danger: u8 },
    TypedGigabytes { danger: u8, expected: u64 },
}

pub fn confirmation_requirement(danger: u8, findings: &[Finding]) -> ConfirmationRequirement {
    if danger >= 9 {
        ConfirmationRequirement::TypedGigabytes {
            danger,
            expected: crate::report::actionable_bytes(findings) / (1024 * 1024 * 1024),
        }
    } else {
        ConfirmationRequirement::YesNo { danger }
    }
}

pub fn gate(max_danger: u8, ctx: &Ctx, findings: &[Finding]) -> Result<()> {
    if !ctx.interactive && !ctx.yes && !ctx.yolo {
        bail!("non-interactive run: re-run with -y to confirm danger-{max_danger} operations");
    }
    warn_data_loss(ctx);
    if ctx.yolo {
        return Ok(());
    }
    match confirmation_requirement(max_danger, findings) {
        ConfirmationRequirement::TypedGigabytes { danger, expected } => {
            if !ctx.interactive {
                bail!(
                    "danger-{danger} operation requires interactive typed confirmation or --yolo"
                );
            }
            eprintln!(
                "{} about to irreversibly remove ~{expected} GB. Type the number to continue:",
                "CRITICAL".red().bold()
            );
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            if line.trim() != expected.to_string() {
                bail!("confirmation mismatch — aborted");
            }
        }
        ConfirmationRequirement::YesNo { danger } if !ctx.yes => {
            eprintln!(
                "{} danger-{danger}: proceed? [y/N]",
                "confirm".yellow().bold()
            );
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            if !line.trim().eq_ignore_ascii_case("y") {
                bail!("aborted by user");
            }
        }
        ConfirmationRequirement::YesNo { .. } => {}
    }
    Ok(())
}

pub fn warn_data_loss(ctx: &Ctx) {
    if !ctx.json {
        eprintln!("{} {DATA_LOSS_NOTICE}", "DATA-LOSS WARNING:".red().bold(),);
    }
}

/// The acknowledgment is measured over the exact findings a purge would remove,
/// the same basis the preview suggests. Measuring the whole Trash instead makes
/// the acknowledgment unsatisfiable once a large item is excluded from the plan.
pub fn trash_gate(findings: &[Finding], confirm_gb: Option<u64>) -> Result<()> {
    let Some(want) = confirm_gb else {
        bail!("Trash purge requires --confirm=<gb> matching current Trash size");
    };
    let actual_gb = crate::report::actionable_bytes(findings) / (1024 * 1024 * 1024);
    let low = actual_gb.saturating_sub(2);
    let high = actual_gb.saturating_add(2);
    if !(low..=high).contains(&want) {
        bail!(
            "--confirm={want} but Trash holds ~{actual_gb} GB; pass --confirm={actual_gb} to acknowledge"
        );
    }
    Ok(())
}

pub fn dir_size(path: &Path) -> Result<u64> {
    Ok(dir_stats(path)?.0)
}

/// Logical size and the newest regular-file modification time under `path`.
///
/// One traversal serves both, so a size and the staleness judged from it always
/// describe the same tree. Only regular files contribute to either: a
/// directory's own mtime moves when the tree is created and whenever an entry is
/// removed, so a store restored from backup would otherwise look permanently
/// active. An addition is still seen, because the added file carries its own
/// fresh timestamp.
///
/// The timestamp is `None` when any file's modification time could not be read.
/// It is deliberately not an error: `dir_size` — which nine of the ten
/// categories reach for and which needs no timestamp at all — would otherwise
/// start failing on a tree it can measure perfectly well. A caller that judges
/// staleness must treat `None` as a refusal; one that only wants bytes can
/// ignore it.
///
/// A subtree with no regular files reports `UNIX_EPOCH`, which reads as
/// maximally stale — harmless, because it also measures zero bytes, and every
/// caller skips a zero-byte target.
pub(crate) fn dir_stats(path: &Path) -> Result<(u64, Option<std::time::SystemTime>)> {
    let mut bytes = 0u64;
    let mut newest = Some(std::time::SystemTime::UNIX_EPOCH);
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok((0, newest)),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", path.display()));
        }
    }
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .follow_root_links(false)
    {
        let entry = entry.with_context(|| format!("cannot measure {}", path.display()))?;
        if entry.file_type().is_file() {
            let metadata = entry
                .metadata()
                .with_context(|| format!("cannot measure {}", entry.path().display()))?;
            // An unreadable timestamp must not abstain: a file that did not
            // vote would let the subtree read older than it is, and an active
            // session would become deletable. Recording the gap rather than
            // erroring leaves that judgement to the caller that makes it.
            match metadata.modified() {
                Ok(modified) => newest = newest.map(|newest| newest.max(modified)),
                Err(_) => newest = None,
            }

            bytes = bytes
                .checked_add(metadata.len())
                .ok_or_else(|| anyhow::anyhow!("logical size overflow under {}", path.display()))?;
        }
    }
    Ok((bytes, newest))
}

const BUILD_PROCESS_PATTERN: &str = "node|npm|pnpm|yarn|bun|deno|cargo|rustc|go|python|python3|Python|gradle|java|xcodebuild|swift|swiftc|make|ninja|cmake";

/// `pgrep` matching shared by every liveness probe. `-a` keeps devtrim's own
/// ancestors in the list: pgrep omits them by default, and a `make` or
/// `npm run` that invokes devtrim from inside a build is exactly the process
/// whose repository must not lose its dependencies.
const PGREP_MATCH_ARGS: [&str; 2] = ["-a", "-x"];

pub(crate) fn build_process_cwds() -> Result<Vec<PathBuf>> {
    let running_build_pids = || -> Result<BTreeSet<u32>> {
        let pgrep = Command::new("pgrep")
            .args(PGREP_MATCH_ARGS)
            .arg(BUILD_PROCESS_PATTERN)
            .output()
            .context("cannot run build-process pgrep probe")?;
        Ok(parse_pgrep_pids(&pgrep.stdout, pgrep.status.code())?
            .into_iter()
            .collect())
    };
    let pids = running_build_pids()?;
    if pids.is_empty() {
        return Ok(Vec::new());
    }
    let first = lsof_cwds_of(&pids)?;
    resolve_build_process_cwds(&pids, first, running_build_pids, lsof_cwds_of)
}

/// One `lsof` run for the working directories of exactly these processes.
fn lsof_cwds_of(pids: &BTreeSet<u32>) -> Result<LsofCwds> {
    let pid_list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let lsof = Command::new("lsof")
        .args(["-a", "-p", &pid_list, "-d", "cwd", "-F", "n"])
        .output()
        .context("cannot run build-process cwd probe")?;
    parse_lsof_cwds(&lsof.stdout, lsof.status.code())
}

/// Every file a process has mapped for execution — its program and each library
/// it loaded — across every process `lsof` can read.
///
/// A long-lived agent keeps running the release it started from after an
/// upgrade moves `current`, and spawns helpers from that same directory by path,
/// so a release a process is still executing is not obsolete. `lsof -d txt` over
/// the whole system exits 0 when it lists anything; any other status, or a name
/// it escapes ambiguously, refuses. Processes of other users are invisible to
/// it, which is the same limit the build-process probe has.
pub(crate) fn executable_mappings() -> Result<Vec<PathBuf>> {
    // lsof after 4.93.2 omits `f` unless requested; the parser needs it to
    // reject a mapped file whose name is missing.
    let lsof = Command::new("lsof")
        .args(["-w", "-d", "txt", "-F", "fn"])
        .output()
        .context("cannot run executable-mapping probe")?;
    parse_lsof_mappings(&lsof.stdout, lsof.status.code())
}

pub(crate) fn parse_lsof_mappings(output: &[u8], exit_code: Option<i32>) -> Result<Vec<PathBuf>> {
    match exit_code {
        Some(0) => {}
        Some(code) => bail!("lsof executable-mapping probe exited with status {code}"),
        None => bail!("lsof executable-mapping probe terminated without an exit status"),
    }
    // Every mapping is `p` (once per process), then `f`, then its `n`. A
    // mapping whose name is missing or not an absolute path could be any file,
    // including one inside the release being judged, so it fails the probe.
    let mut in_process = false;
    let mut awaiting_name = false;
    let mut paths = Vec::new();
    for line in output.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        match line.first() {
            None => continue,
            Some(b'p') => {
                if awaiting_name {
                    bail!("lsof reported an executable mapping without a name");
                }
                in_process = true;
            }
            Some(b'f') => {
                if !in_process || awaiting_name {
                    bail!("lsof reported an executable mapping without a name or process");
                }
                awaiting_name = true;
            }
            Some(b'n') => {
                if !awaiting_name {
                    bail!("lsof reported a mapped path outside any mapping");
                }
                let name = &line[1..];
                if !name.starts_with(b"/") {
                    bail!("lsof reported a mapped path that is not absolute");
                }
                paths.push(PathBuf::from(OsString::from_vec(decode_lsof_name(name)?)));
                awaiting_name = false;
            }
            Some(_) => bail!("lsof returned an unexpected executable-mapping field"),
        }
    }
    if awaiting_name {
        bail!("lsof reported an executable mapping without a name");
    }
    // devtrim itself maps its own binary, so an empty answer is a failed probe.
    if paths.is_empty() {
        bail!("lsof reported no executable mappings, not even its own");
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

/// Working directories `lsof -F n` reported, the processes it reported them
/// for, and whether it reported every process it was asked about (exit 0).
#[derive(Debug)]
pub(crate) struct LsofCwds {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) reported: BTreeSet<u32>,
    pub(crate) complete: bool,
}

/// Decide whether one `lsof -p <pids> -d cwd` run proves every build process's
/// working directory.
///
/// `lsof` exits 1 when any listed process is missing by the time it looks, and
/// on a busy machine a short-lived `node` or `python` exits between `pgrep` and
/// `lsof` routinely — 2 of 5 consecutive probes on the development machine,
/// each refusing a whole `artifacts` apply. A process that no longer exists
/// runs no build, so an incomplete run is accepted only when every PID `lsof`
/// did not report is also absent from a fresh `pgrep`. `lsof` exits 1 the same
/// way for a process it cannot read, such as another user's; one still running
/// refuses, because its directory stays unknown.
///
/// The fresh `pgrep` can also show a build process that started after the
/// first one — a build moving to its next step. Its directory was never looked
/// up, so it is looked up once, and any gap in that second answer refuses
/// rather than chasing a moving process list.
fn resolve_build_process_cwds(
    requested: &BTreeSet<u32>,
    lsof: LsofCwds,
    running_now: impl FnOnce() -> Result<BTreeSet<u32>>,
    cwds_of: impl FnOnce(&BTreeSet<u32>) -> Result<LsofCwds>,
) -> Result<Vec<PathBuf>> {
    if let Some(unexpected) = lsof.reported.difference(requested).next() {
        bail!("lsof reported process {unexpected}, which was not probed");
    }
    if lsof.complete {
        return Ok(lsof.paths);
    }
    let unreported: BTreeSet<u32> = requested.difference(&lsof.reported).copied().collect();
    if unreported.is_empty() {
        bail!("lsof cwd probe exited with status 1 after reporting every process");
    }
    let running = running_now().context("cannot recheck the processes lsof did not report")?;
    if let Some(pid) = unreported.intersection(&running).next() {
        bail!("lsof could not read the working directory of running build process {pid}");
    }
    let mut paths = lsof.paths;
    let successors: BTreeSet<u32> = running.difference(requested).copied().collect();
    if !successors.is_empty() {
        let later = cwds_of(&successors).context(
            "cannot read the working directories of build processes that started during the probe",
        )?;
        if !later.complete || later.reported != successors {
            bail!("build processes changed again while their working directories were read");
        }
        paths.extend(later.paths);
        paths.sort();
        paths.dedup();
    }
    Ok(paths)
}

/// Processes that write DerivedData: command-line builds, the Swift Build and
/// legacy XCBuild services that Xcode.app builds run through (their parent is
/// the IDE, not `xcodebuild`), and the IDE itself, whose indexer writes there.
const XCODE_BUILD_PATTERN: &str = "xcodebuild|SWBBuildService|XCBBuildService|Xcode";

pub(crate) fn xcode_build_running() -> Result<bool> {
    let output = Command::new("pgrep")
        .args(PGREP_MATCH_ARGS)
        .arg(XCODE_BUILD_PATTERN)
        .output()
        .context("cannot run Xcode build liveness probe")?;
    Ok(!parse_pgrep_pids(&output.stdout, output.status.code())?.is_empty())
}

pub(crate) fn parse_pgrep_pids(output: &[u8], exit_code: Option<i32>) -> Result<Vec<u32>> {
    match exit_code {
        Some(1) => return Ok(Vec::new()),
        Some(0) => {}
        Some(code) => bail!("pgrep liveness probe exited with status {code}"),
        None => bail!("pgrep liveness probe terminated without an exit status"),
    }
    let mut pids = output
        .split(|byte| byte.is_ascii_whitespace())
        .filter(|value| !value.is_empty())
        .map(|value| {
            let value = std::str::from_utf8(value).context("pgrep returned a non-UTF-8 pid")?;
            value
                .parse::<u32>()
                .with_context(|| format!("pgrep returned invalid pid `{value}`"))
        })
        .collect::<Result<Vec<_>>>()?;
    if pids.is_empty() {
        bail!("pgrep reported matches without any pids");
    }
    pids.sort_unstable();
    pids.dedup();
    Ok(pids)
}

/// lsof escapes a name for display, and a build directory whose displayed
/// spelling differs from its real path would never match the repository it
/// runs in — leaving that repository's dependencies deletable mid-build. The
/// unambiguous escapes are decoded; `^X` is refused because lsof renders a
/// control byte and a literal caret identically.
fn decode_lsof_name(name: &[u8]) -> Result<Vec<u8>> {
    let mut decoded = Vec::with_capacity(name.len());
    let mut index = 0;
    while let Some(&byte) = name.get(index) {
        let next = name.get(index + 1).copied();
        match (byte, next) {
            (b'\\', Some(b'x')) => {
                let value = name
                    .get(index + 2..index + 4)
                    .and_then(|hex| std::str::from_utf8(hex).ok())
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                    .ok_or_else(|| anyhow::anyhow!("lsof returned an invalid cwd escape"))?;
                decoded.push(value);
                index += 4;
                continue;
            }
            (b'\\', Some(escape)) => decoded.push(match escape {
                b'\\' => b'\\',
                b'n' => b'\n',
                b't' => b'\t',
                b'r' => b'\r',
                b'b' => 0x08,
                b'f' => 0x0c,
                _ => bail!("lsof returned an unknown cwd escape"),
            }),
            (b'\\', None) => bail!("lsof returned a truncated cwd escape"),
            (b'^', Some(b'@'..=b'_' | b'?')) => {
                bail!("lsof cwd name is ambiguous: `^X` may stand for a control byte")
            }
            (other, _) => {
                decoded.push(other);
                index += 1;
                continue;
            }
        }
        index += 2;
    }
    Ok(decoded)
}

pub(crate) fn parse_lsof_cwds(output: &[u8], exit_code: Option<i32>) -> Result<LsofCwds> {
    let complete = match exit_code {
        Some(0) => true,
        // Exit 1 still carries every process lsof could read; whether the
        // missing ones matter is `resolve_build_process_cwds`'s decision.
        Some(1) => false,
        Some(code) => bail!("lsof cwd probe exited with status {code}"),
        None => bail!("lsof cwd probe terminated without an exit status"),
    };
    let mut paths = Vec::new();
    let mut reported = BTreeSet::new();
    // The process whose fields follow, and whether it has named a cwd yet.
    let mut current: Option<(u32, bool)> = None;
    for line in output.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if let Some(pid) = line.strip_prefix(b"p") {
            if let Some((previous, false)) = current {
                bail!("lsof reported process {previous} without a cwd");
            }
            let pid = std::str::from_utf8(pid)
                .ok()
                .and_then(|pid| pid.parse::<u32>().ok())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "lsof returned invalid pid `{}`",
                        String::from_utf8_lossy(pid)
                    )
                })?;
            if !reported.insert(pid) {
                bail!("lsof reported process {pid} twice");
            }
            current = Some((pid, false));
            continue;
        }
        let Some(path) = line.strip_prefix(b"n") else {
            continue;
        };
        let Some((pid, _)) = current else {
            bail!("lsof returned a cwd before naming its process");
        };
        if path.is_empty() {
            bail!("lsof returned an empty cwd path");
        }
        paths.push(PathBuf::from(OsString::from_vec(decode_lsof_name(path)?)));
        current = Some((pid, true));
    }
    if let Some((pid, false)) = current {
        bail!("lsof reported process {pid} without a cwd");
    }
    if complete && paths.is_empty() {
        bail!("lsof reported success without any cwd paths");
    }
    paths.sort();
    paths.dedup();
    Ok(LsofCwds {
        paths,
        reported,
        complete,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use proptest::test_runner::{Config as ProptestConfig, RngSeed};
    use std::os::unix::{ffi::OsStringExt, fs::PermissionsExt, fs::symlink};

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("devtrim-{name}-{}", std::process::id()));
        crate::ops::remove_test_path(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 64,
            failure_persistence: None,
            rng_seed: RngSeed::Fixed(0xD37_71A),
            ..ProptestConfig::default()
        })]

        #[test]
        fn protected_system_roots_and_descendants(
            index in 0usize..PROTECTED.len(),
            leaf in "[a-z]{1,12}",
        ) {
            let home = Path::new("/Users/example");
            let root = Path::new(PROTECTED[index]);
            prop_assert!(is_protected(root, home));
            if root != Path::new("/") {
                prop_assert!(is_protected(&root.join(leaf), home));
            }
        }

        #[test]
        fn protected_user_roots_and_descendants(
            index in 0usize..PROTECTED_USER.len(),
            leaf in "[a-z]{1,12}",
        ) {
            let home = Path::new("/Users/example");
            let root = home.join(PROTECTED_USER[index]);
            prop_assert!(is_protected(&root, home));
            if PROTECTED_USER[index] != "Library" {
                prop_assert!(is_protected(&root.join(leaf), home));
            }
        }

        #[test]
        fn library_managed_namespaces_are_exact_exceptions(
            index in 0usize..4,
            leaf in "[a-z]{1,12}",
        ) {
            let home = Path::new("/Users/example");
            let managed = [
                "Developer/Toolchains",
                "Developer/Xcode/iOS DeviceSupport",
                "Developer/Xcode/DerivedData",
                "Caches/Homebrew",
            ];
            let root = home.join("Library").join(managed[index]);
            prop_assert!(!is_protected(&root, home));
            prop_assert!(!is_protected(&root.join(leaf), home));
            prop_assert!(is_protected(&home.join("Library/Application Support"), home));
            prop_assert!(is_protected(&home.join("Library/Developer/Xcode/Archives"), home));
        }

        #[test]
        fn cleaned_parent_aliases_to_user_secrets_are_protected(
            index in 0usize..2,
            leaf in "[a-z]{1,12}",
        ) {
            let home = Path::new("/Users/example");
            let secret = [".ssh", ".gnupg"][index];
            let alias = home.join("dev").join("..").join(secret).join(leaf);
            prop_assert!(is_protected(&alias, home));
        }

        #[test]
        fn validation_preserves_arbitrary_non_utf8_leaf_identity(
            raw in proptest::collection::vec(any::<u8>(), 0..24),
        ) {
            let home = std::env::current_dir()
                .unwrap()
                .canonicalize()
                .unwrap()
                .join("target")
                .join(format!("devtrim-nonutf8-{}", std::process::id()));
            std::fs::create_dir_all(&home).unwrap();
            let mut bytes = vec![0xff];
            bytes.extend(raw.into_iter().map(|byte| match byte {
                0 | b'/' => b'_',
                value => value,
            }));
            let target = home.join(std::ffi::OsString::from_vec(bytes));
            let verified = validate_path_for_deletion(&target, &home, &[]).unwrap();
            prop_assert_eq!(verified.into_path(), target);
            crate::ops::remove_test_path(home);
        }
    }

    #[test]
    fn protects_case_variant_aliases() {
        let home = PathBuf::from("/Users/example");
        assert!(is_protected(&home, &home));
        assert!(is_protected(&home.join(".Trash"), &home));
        assert!(is_protected(Path::new("/system"), &home));
        assert!(is_protected(Path::new("/system/tmp"), &home));
        assert!(is_protected(Path::new("/applications"), &home));
        assert!(is_protected(Path::new("/applications/Foo.app"), &home));
        assert!(is_protected(Path::new("/private/var/tmp"), &home));
        assert!(is_protected(Path::new("/volumes/Disk"), &home));
        assert!(!is_protected(Path::new("/systematic/tmp"), &home));
        assert!(is_protected(&home.join(".SSH"), &home));
        assert!(is_protected(Path::new("/users/example/.SSH"), &home));
        assert!(is_protected(Path::new("/users/example/Library"), &home));
        assert!(!is_protected(Path::new("/users/examples/.SSH"), &home));
        assert!(is_protected(&home.join(".GnUpG"), &home));
        assert!(is_protected(&home.join("library"), &home));
        assert!(!is_protected(&home.join("dev/project"), &home));
    }

    #[test]
    fn configured_protect_expands_tilde_and_rejects_relative_entries() {
        let home = Path::new("/Users/example");
        let (protect, warnings) = configured_protect(vec!["~/dev/keep".into()], home).unwrap();
        assert_eq!(
            configured_protect_values(&protect),
            &[home.join("dev/keep"), home.join("dev/keep")]
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("~/dev/keep"));
        assert!(warnings[0].contains("does not currently resolve"));
        let error = configured_protect(vec!["dev/keep".into()], home).unwrap_err();
        assert!(error.to_string().contains("dev/keep"));
        assert!(error.to_string().contains("absolute"));
    }

    #[test]
    fn configured_protect_existing_entry_warns_nothing() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-protect-exists-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(home.join("dev/keep")).unwrap();
        let (_, warnings) = configured_protect(vec!["~/dev/keep".into()], &home).unwrap();
        assert!(warnings.is_empty());
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn deletion_validation_refuses_git_repository_and_worktree_roots() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-git-roots-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        let repository = home.join("dev/repository");
        let worktree = home.join("dev/worktree");
        std::fs::create_dir_all(repository.join(".git")).unwrap();
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(
            worktree.join(".git"),
            "gitdir: ../repository/.git/worktrees/test\n",
        )
        .unwrap();
        let home = home.canonicalize().unwrap();

        for target in [home.join("dev/repository"), home.join("dev/worktree")] {
            let error = validate_path_for_deletion(&target, &home, &[]).unwrap_err();
            assert!(error.to_string().contains("Git repository/worktree root"));
            assert!(target.exists());
        }
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn deletion_validation_refuses_git_metadata_case_variants() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-git-case-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        let repository = home.join("dev/repository");
        let metadata_target = home.join("dev/repository/.GIT/objects/target");
        let normal_target = home.join("dev/repository/git/objects/target");
        std::fs::create_dir_all(&metadata_target).unwrap();
        std::fs::create_dir_all(&normal_target).unwrap();
        let home = home.canonicalize().unwrap();

        for variant in [".GIT", ".Git", ".gIt"] {
            let target = home
                .join("dev/repository")
                .join(variant)
                .join("objects/target");
            let error = validate_path_for_deletion(&target, &home, &[]).unwrap_err();
            assert!(error.to_string().contains("inside Git metadata"));
        }
        assert!(validate_path_for_deletion(&normal_target, &home, &[]).is_ok());
        let repository_error = validate_path_for_deletion(&repository, &home, &[]).unwrap_err();
        assert!(
            repository_error
                .to_string()
                .contains("Git repository/worktree root")
        );
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn deletion_validation_fails_closed_when_protect_alias_drifts() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-protect-drift-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(home.join("dev/protected-original")).unwrap();
        std::fs::create_dir_all(home.join("dev/protected-replacement")).unwrap();
        std::fs::create_dir_all(home.join("dev/deletable")).unwrap();
        symlink(home.join("dev/protected-original"), home.join("keep")).unwrap();
        let home = home.canonicalize().unwrap();
        let (protect, warnings) = configured_protect(vec!["~/keep".into()], &home).unwrap();
        assert!(warnings.is_empty());

        std::fs::remove_file(home.join("keep")).unwrap();
        symlink(home.join("dev/protected-replacement"), home.join("keep")).unwrap();
        let retargeted =
            validate_path_for_deletion(&home.join("dev/deletable"), &home, &protect).unwrap_err();
        assert!(
            retargeted
                .to_string()
                .contains("protect symlink alias changed")
        );

        std::fs::remove_file(home.join("keep")).unwrap();
        symlink(home.join("dev/missing"), home.join("keep")).unwrap();
        let broken =
            validate_path_for_deletion(&home.join("dev/deletable"), &home, &protect).unwrap_err();
        assert!(
            broken
                .to_string()
                .contains("cannot re-resolve protect symlink alias")
        );

        let (unresolved_protect, warnings) =
            configured_protect(vec!["~/future-keep".into()], &home).unwrap();
        assert_eq!(warnings.len(), 1);
        symlink(
            home.join("dev/protected-replacement"),
            home.join("future-keep"),
        )
        .unwrap();
        let appeared_alias =
            validate_path_for_deletion(&home.join("dev/deletable"), &home, &unresolved_protect)
                .unwrap_err();
        assert!(
            appeared_alias
                .to_string()
                .contains("protect symlink alias changed")
        );
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn configured_protect_refuses_ancestors_and_matches_symlinked_entries() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-protect-intersect-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(home.join("dev/repo/target/important")).unwrap();
        std::fs::create_dir_all(home.join("dev/repo/other")).unwrap();
        let home = home.canonicalize().unwrap();
        let (protect, _) =
            configured_protect(vec!["~/dev/repo/target/important".into()], &home).unwrap();

        // Deleting an ancestor would delete the protected descendant with it.
        assert!(is_config_protected(&home.join("dev/repo/target"), &protect));
        assert!(
            validate_path_for_deletion(&home.join("dev/repo/target"), &home, &protect).is_err()
        );
        assert!(validate_path_for_deletion(&home.join("dev/repo"), &home, &protect).is_err());
        assert!(!is_config_protected(&home.join("dev/repo/other"), &protect));

        // A symlinked entry must protect the canonical location scanners report.
        std::fs::create_dir_all(home.join("dev/project/node_modules")).unwrap();
        symlink(home.join("dev/project"), home.join("keep")).unwrap();
        let (linked, warnings) = configured_protect(vec!["~/keep".into()], &home).unwrap();
        assert!(warnings.is_empty());
        assert!(is_config_protected(
            &home.join("dev/project/node_modules"),
            &linked
        ));
        assert!(
            validate_path_for_deletion(&home.join("dev/project/node_modules"), &home, &linked)
                .is_err()
        );
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn configured_protect_matches_across_unicode_normalization_forms() {
        // NFC in the config, NFD on disk (the common macOS mismatch) and the
        // reverse must both protect; raw byte comparison protects neither.
        let nfc = "caf\u{e9}";
        let nfd = "cafe\u{301}";
        assert!(protect_component_matches(
            std::ffi::OsStr::new(nfd),
            std::ffi::OsStr::new(nfc)
        ));
        assert!(protect_component_matches(
            std::ffi::OsStr::new(nfc),
            std::ffi::OsStr::new(nfd)
        ));
        // ASCII letters stay case-insensitive; non-ASCII case folding is out of
        // scope, matching the ASCII-only folding of the protected-path denylist.
        assert!(protect_component_matches(
            std::ffi::OsStr::new("CAFe\u{301}"),
            std::ffi::OsStr::new(nfc)
        ));
        assert!(!protect_component_matches(
            std::ffi::OsStr::new("cafes"),
            std::ffi::OsStr::new(nfc)
        ));

        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-protect-nfc-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        let on_disk = home.join("dev").join(nfd).join("node_modules");
        std::fs::create_dir_all(&on_disk).unwrap();
        let home = home.canonicalize().unwrap();
        let on_disk = home.join("dev").join(nfd).join("node_modules");
        let (protect, _) = configured_protect(vec![format!("~/dev/{nfc}")], &home).unwrap();

        assert!(is_config_protected(&on_disk, &protect));
        let error = validate_path_for_deletion(&on_disk, &home, &protect).unwrap_err();
        assert!(error.to_string().contains("protected"));
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn configured_protect_refuses_literal_case_variant_and_children() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-configured-protect-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(&home).unwrap();
        let home = home.canonicalize().unwrap();
        let target = home.join("dev/Protected");
        std::fs::create_dir_all(target.join("child")).unwrap();
        let (protect, _) = configured_protect(vec!["~/DEV/protected".into()], &home).unwrap();

        assert!(validate_path_for_deletion(&target, &home, &protect).is_err());
        assert!(validate_path_for_deletion(&target.join("child"), &home, &protect).is_err());
        assert!(is_config_protected(&target, &protect));
        assert!(is_config_protected(&target.join("child"), &protect));

        let resolved = home.join("resolved-protected");
        std::fs::create_dir_all(resolved.join("child")).unwrap();
        symlink(&resolved, home.join("alias")).unwrap();
        let error = validate_path_for_deletion(
            &home.join("alias/child"),
            &home,
            std::slice::from_ref(&resolved),
        )
        .unwrap_err();
        assert!(error.to_string().contains("protected resolved path"));
        crate::ops::remove_test_path(home);
    }

    /// The const assertion above rejects empty and ASCII-whitespace evidence
    /// while compiling, so those cases never reach a test — the crate does not
    /// build at all. What is left is the gap that assertion cannot see:
    /// `is_ascii_whitespace` accepts non-ASCII blanks such as U+00A0, which
    /// `str::trim` strips. That is the only case this loop can catch, and
    /// `scripts/tests/planted-violations.py` (`evidence/library-caches`) plants
    /// exactly it. Asserting facts about the helper here instead would prove
    /// nothing about this loop.
    #[test]
    fn every_managed_library_cache_carries_evidence() {
        for entry in MANAGED_LIBRARY_CACHES {
            assert!(
                !entry.evidence.trim().is_empty(),
                "PV evidence/library-caches: missing deletion evidence: Library/Caches/{}",
                entry.relative
            );
        }
    }

    #[test]
    fn managed_library_caches_are_exact_exceptions() {
        let home = Path::new("/Users/example");
        assert!(!MANAGED_LIBRARY_CACHES.is_empty());
        // Each entry must be exactly one normal component. The list is read by
        // two places that treat it differently: `caches::library_caches` joins
        // it raw, while this boundary only ever sees cleaned paths. An entry
        // containing `..` would therefore be previewed under one spelling and
        // matched under another, letting a target outside the carve-out reach
        // the deletion sink. Nothing but this assertion prevents that.
        for entry in MANAGED_LIBRARY_CACHES {
            let name = entry.relative;
            let mut components = Path::new(name).components();
            assert!(
                matches!(components.next(), Some(std::path::Component::Normal(_))),
                "{name} must be one normal path component"
            );
            assert!(components.next().is_none(), "{name} must be a single name");
        }
        for entry in MANAGED_LIBRARY_CACHES {
            let name = entry.relative;
            let root = home.join("Library/Caches").join(name);
            assert!(!is_protected(&root, home), "{name}");
            assert!(!is_protected(&root.join("nested/file"), home), "{name}");
        }
        for still_protected in [
            "Library",
            "Library/Caches",
            "Library/Caches/com.apple.Safari",
            "Library/Caches/CloudKit",
            "Library/Application Support",
            "Library/Mail",
            // The JetBrains system directory holds non-regenerable Local History.
            "Library/Caches/JetBrains",
            "Library/Caches/JetBrains/IntelliJIdea2026.2/LocalHistory",
            // DENO_DIR holds default-path Deno KV databases and localStorage.
            "Library/Caches/deno",
            "Library/Caches/deno/location_data",
            // A listed name is not a prefix licence for its neighbours.
            "Library/Caches/ms-playwright-extra",
            "Library/Caches/pip-secrets",
        ] {
            assert!(
                is_protected(&home.join(still_protected), home),
                "{still_protected}"
            );
        }
    }

    #[test]
    fn rejects_symlinked_ancestor() {
        let home = std::env::current_dir()
            .unwrap()
            .canonicalize()
            .unwrap()
            .join("target")
            .join(format!("devtrim-ancestor-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        let safe = home.join("dev");
        let protected = home.join("Library");
        std::fs::create_dir_all(protected.join("node_modules")).unwrap();
        std::fs::create_dir_all(&safe).unwrap();
        symlink(&protected, safe.join("linked")).unwrap();
        let target = safe.join("linked/node_modules");
        let error = validate_path_for_deletion(&target, &home, &[]).unwrap_err();
        assert!(error.to_string().contains("symlinked ancestor"));
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn refuses_symlinked_trash() {
        let home = temp("trash-link");
        let target = home.join("elsewhere");
        std::fs::create_dir_all(&target).unwrap();
        symlink(&target, home.join(".Trash")).unwrap();
        assert!(validate_trash_root(&home).is_err());
        crate::ops::remove_test_path(home.join(".Trash"));
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn trash_acknowledgment_measures_the_plan_not_the_whole_trash() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let small = Finding::new(
            "Trash item: small",
            Some(PathBuf::from("/Users/example/.Trash/small")),
            1024,
            "permanent purge",
            9,
            crate::ops::Action::Shred,
        );
        let large = Finding::new(
            "Trash item: large",
            Some(PathBuf::from("/Users/example/.Trash/large")),
            10 * GIB,
            "permanent purge",
            9,
            crate::ops::Action::Shred,
        );
        // A 10 GiB item excluded from the plan must not demand `--confirm=10`
        // for the 1 KiB that will actually be purged…
        assert!(trash_gate(std::slice::from_ref(&small), Some(0)).is_ok());
        assert!(trash_gate(std::slice::from_ref(&small), Some(10)).is_err());
        // …and a plan that does include it still requires acknowledging it.
        let both = [small, large];
        assert!(trash_gate(&both, Some(0)).is_err());
        assert!(trash_gate(&both, Some(10)).is_ok());
        assert!(trash_gate(&both, None).is_err());
    }

    #[test]
    fn aggregate_size_escalates() {
        assert_eq!(escalate(3, 11 * 1024 * 1024 * 1024), 7);
        assert_eq!(escalate(3, 51 * 1024 * 1024 * 1024), 8);
    }

    #[test]
    fn yes_does_not_bypass_critical_typed_confirmation() {
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: true,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: PathBuf::from("/tmp/devtrim-test-journal.jsonl"),
            home: PathBuf::from("/Users/example"),
            interactive: false,
            diagnostic_output: DiagnosticOutput::Stderr,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };

        let error = gate(9, &ctx, &[]).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("requires interactive typed confirmation")
        );
    }

    #[test]
    fn directory_size_fails_closed_on_unreadable_content() {
        let root = temp("unreadable-size");
        let unreadable = root.join("unreadable");
        std::fs::create_dir_all(&unreadable).unwrap();
        std::fs::write(unreadable.join("hidden"), "not measured").unwrap();

        let original = std::fs::metadata(&unreadable).unwrap().permissions();
        let mut denied = original.clone();
        denied.set_mode(0o000);
        std::fs::set_permissions(&unreadable, denied).unwrap();
        let measured = dir_size(&root);
        std::fs::set_permissions(&unreadable, original).unwrap();

        assert!(measured.is_err());
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn directory_size_does_not_follow_a_root_symlink() {
        let root = temp("symlink-size");
        let target = root.join("target");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("payload"), "not part of the link").unwrap();
        let link = root.join("link");
        symlink(&target, &link).unwrap();

        assert_eq!(dir_size(&link).unwrap(), 0);
        crate::ops::remove_test_path(root);
    }

    fn cwds_complete_was(output: &[u8], exit: i32) -> bool {
        parse_lsof_cwds(output, Some(exit)).unwrap().complete
    }

    #[test]
    fn parses_build_process_probe_outputs() {
        assert!(
            BUILD_PROCESS_PATTERN
                .split('|')
                .any(|name| name == "Python")
        );
        assert_eq!(
            parse_pgrep_pids(b"12\n34\n12\n", Some(0)).unwrap(),
            vec![12, 34]
        );
        assert!(parse_pgrep_pids(b"", Some(1)).unwrap().is_empty());
        assert!(parse_pgrep_pids(b"not-a-pid\n", Some(0)).is_err());
        assert!(parse_pgrep_pids(b"", Some(2)).is_err());

        let cwds = parse_lsof_cwds(b"p12\nfcwd\nn/tmp/a\np34\nfcwd\nn/tmp/b\n", Some(0)).unwrap();
        assert_eq!(
            cwds.paths,
            vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]
        );
        assert_eq!(cwds.reported.into_iter().collect::<Vec<_>>(), vec![12, 34]);
        assert!(cwds_complete_was(b"p12\nfcwd\nn/tmp/a\n", 0));
        let gone = parse_lsof_cwds(b"", Some(1)).unwrap();
        assert!(gone.paths.is_empty() && gone.reported.is_empty() && !gone.complete);
        assert!(parse_lsof_cwds(b"", Some(0)).is_err());
        assert!(parse_lsof_cwds(b"p1\nn/tmp\n", Some(3)).is_err());
        assert!(parse_lsof_cwds(b"p1\nn/tmp\n", None).is_err());
        for malformed in [
            &b"n/tmp/orphan\n"[..],
            b"p12\nfcwd\n",
            b"p12\np34\nn/tmp/b\n",
            b"p12\nn/tmp/a\np12\nn/tmp/b\n",
            b"px\nn/tmp/a\n",
            b"p\nn/tmp/a\n",
        ] {
            assert!(
                parse_lsof_cwds(malformed, Some(0)).is_err(),
                "accepted {}",
                String::from_utf8_lossy(malformed)
            );
        }
    }

    fn pid_set(pids: &[u32]) -> BTreeSet<u32> {
        pids.iter().copied().collect()
    }

    #[test]
    fn lsof_exit_one_passes_only_when_every_unreported_process_is_gone() {
        let lsof = || parse_lsof_cwds(b"p12\nfcwd\nn/work/live\n", Some(1)).unwrap();
        let unused =
            |_: &BTreeSet<u32>| -> Result<LsofCwds> { bail!("no successor lookup expected") };

        // PID 34 exited between pgrep and lsof, and a fresh pgrep agrees.
        let cwds =
            resolve_build_process_cwds(&pid_set(&[12, 34]), lsof(), || Ok(pid_set(&[12])), unused)
                .unwrap();
        assert_eq!(cwds, vec![PathBuf::from("/work/live")]);

        // PID 34 is still running and lsof could not read it.
        match resolve_build_process_cwds(
            &pid_set(&[12, 34]),
            lsof(),
            || Ok(pid_set(&[12, 34])),
            unused,
        ) {
            Ok(cwds) => panic!(
                "PV liveness/lsof-unreported-running: accepted {cwds:?} with process 34 unread"
            ),
            Err(error) => assert!(
                error.to_string().contains("running build process 34"),
                "PV liveness/lsof-unreported-running: {error:#}"
            ),
        }

        let error = resolve_build_process_cwds(
            &pid_set(&[12, 34]),
            lsof(),
            || Err(anyhow::anyhow!("pgrep failed")),
            unused,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("pgrep failed"), "{error:#}");

        let error =
            resolve_build_process_cwds(&pid_set(&[12]), lsof(), || Ok(pid_set(&[12])), unused)
                .unwrap_err();
        assert!(error.to_string().contains("after reporting every process"));

        let complete = parse_lsof_cwds(b"p12\nfcwd\nn/work/live\n", Some(0)).unwrap();
        let error =
            resolve_build_process_cwds(&pid_set(&[34]), complete, || Ok(BTreeSet::new()), unused)
                .unwrap_err();
        assert!(error.to_string().contains("was not probed"));

        let complete = parse_lsof_cwds(b"p12\nfcwd\nn/work/live\n", Some(0)).unwrap();
        let cwds =
            resolve_build_process_cwds(&pid_set(&[12]), complete, || Ok(BTreeSet::new()), unused)
                .unwrap();
        assert_eq!(cwds, vec![PathBuf::from("/work/live")]);
    }

    #[test]
    fn a_build_process_that_started_during_the_probe_is_looked_up_once() {
        let lsof = || parse_lsof_cwds(b"p12\nfcwd\nn/work/live\n", Some(1)).unwrap();

        // PID 34 exited; PID 56 (the build's next step) appeared in the recheck.
        let looked_up = std::cell::RefCell::new(BTreeSet::new());
        let cwds = resolve_build_process_cwds(
            &pid_set(&[12, 34]),
            lsof(),
            || Ok(pid_set(&[12, 56])),
            |pids| {
                looked_up.replace(pids.clone());
                Ok(parse_lsof_cwds(b"p56\nfcwd\nn/work/next-step\n", Some(0)).unwrap())
            },
        )
        .unwrap();
        assert_eq!(*looked_up.borrow(), pid_set(&[56]));
        assert_eq!(
            cwds,
            vec![
                PathBuf::from("/work/live"),
                PathBuf::from("/work/next-step")
            ]
        );

        // The successor's directory could not be read, or it too vanished.
        for (output, exit) in [(&b""[..], 1), (b"p56\nfcwd\nn/work/next-step\n", 1)] {
            match resolve_build_process_cwds(
                &pid_set(&[12, 34]),
                lsof(),
                || Ok(pid_set(&[12, 56])),
                |_| parse_lsof_cwds(output, Some(exit)),
            ) {
                Ok(cwds) => panic!(
                    "PV liveness/lsof-successor: accepted {cwds:?} without the successor's directory"
                ),
                Err(error) => assert!(
                    error.to_string().contains("changed again"),
                    "PV liveness/lsof-successor: {error:#}"
                ),
            }
        }

        let error = resolve_build_process_cwds(
            &pid_set(&[12, 34]),
            lsof(),
            || Ok(pid_set(&[12, 56])),
            |_| Err(anyhow::anyhow!("lsof failed")),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("lsof failed"), "{error:#}");
    }

    #[test]
    fn real_lsof_race_with_an_exited_process_is_resolved_not_refused() {
        // Reproduce the race deterministically: probe this test process together
        // with one that has already exited, exactly as `lsof -p` sees it when a
        // build tool exits between `pgrep` and `lsof`.
        let mut child = Command::new("/usr/bin/true").spawn().unwrap();
        let exited = child.id();
        child.wait().unwrap();
        let this = std::process::id();
        let requested = pid_set(&[this, exited]);
        // The production probe itself, so its flags cannot drift from this test.
        let parsed = lsof_cwds_of(&requested).unwrap();
        assert!(!parsed.complete, "control: lsof must exit 1 here");
        let cwd = std::env::current_dir().unwrap().canonicalize().unwrap();
        let no_successor = |_: &BTreeSet<u32>| -> Result<LsofCwds> { bail!("unexpected lookup") };

        let cwds =
            resolve_build_process_cwds(&requested, parsed, || Ok(pid_set(&[this])), no_successor)
                .unwrap();
        assert!(cwds.contains(&cwd), "{cwds:?} lacks {}", cwd.display());

        // Control: had the exited PID still been running, the same output refuses.
        let parsed = lsof_cwds_of(&requested).unwrap();
        let error =
            resolve_build_process_cwds(&requested, parsed, || Ok(requested.clone()), no_successor)
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("running build process {exited}")),
            "{error:#}"
        );
    }

    #[test]
    fn liveness_pgrep_matches_devtrims_own_ancestors() {
        // `/usr/bin/time` under a unique name runs pgrep as its child, waits,
        // and exits with pgrep's status, so it is an ancestor of the probe — the
        // position of a `make` that runs devtrim — with no shell involved.
        let directory = temp("pgrep-ancestor");
        std::fs::create_dir_all(&directory).unwrap();
        let name = format!("devtrimanc{}", std::process::id());
        let ancestor = directory.join(&name);
        symlink("/usr/bin/time", &ancestor).unwrap();
        let pgrep_status = |match_args: &[&str]| {
            Command::new(&ancestor)
                .arg("pgrep")
                .args(match_args)
                .arg(&name)
                .output()
                .unwrap()
                .status
                .code()
        };
        let with = pgrep_status(&PGREP_MATCH_ARGS);
        let without = pgrep_status(&["-x"]);
        crate::ops::remove_test_path(&directory);

        assert_eq!(with, Some(0), "ancestor not matched");
        // Control: pgrep's default excludes the same ancestor.
        assert_eq!(without, Some(1), "control failed");
    }

    #[test]
    fn executable_mappings_refuse_any_mapping_they_cannot_name() {
        let parsed = parse_lsof_mappings(
            b"p1\nftxt\nn/a\nftxt\nn/b\np2\nftxt\nn/c\nftxt\nn/a\n",
            Some(0),
        )
        .unwrap();
        assert_eq!(
            parsed,
            ["/a", "/b", "/c"].map(PathBuf::from).to_vec(),
            "control: complete output parses"
        );
        for incomplete in [
            // A second process whose mapping lsof could not name.
            &b"p1\nftxt\nn/usr/lib/dyld\np2\nftxt\n"[..],
            b"p1\nftxt\nftxt\nn/a\n",
            b"p1\nftxt\np2\nftxt\nn/a\n",
            b"ftxt\nn/a\n",
            b"p1\nn/a\n",
            b"p1\nftxt\nnrelative\n",
            b"p1\nftxt\nn\n",
            b"p1\nx1\nftxt\nn/a\n",
            b"",
        ] {
            assert!(
                parse_lsof_mappings(incomplete, Some(0)).is_err(),
                "PV liveness/lsof-mapping-names: accepted {}",
                String::from_utf8_lossy(incomplete)
            );
        }
        assert!(parse_lsof_mappings(b"p1\nftxt\nn/a\n", Some(1)).is_err());
        assert!(parse_lsof_mappings(b"p1\nftxt\nn/a\n", None).is_err());
    }

    #[test]
    fn lsof_cwd_names_are_decoded_or_refused_never_taken_literally() {
        // Observed on macOS: lsof renders a newline as `\n`, a backslash as
        // `\\`, a byte outside printable ASCII as `\xHH`, and a control byte as
        // `^X` — the last indistinguishable from a literal caret in the name.
        for ambiguous in [
            &b"p1\nn/work/caret^Ay\n"[..],
            b"p1\nn/work/unknown\\q\n",
            b"p1\nn/work/short\\x4\n",
            b"p1\nn/work/trailing\\\n",
        ] {
            assert!(
                parse_lsof_cwds(ambiguous, Some(0)).is_err(),
                "PV liveness/lsof-escape: {} was taken literally",
                String::from_utf8_lossy(ambiguous)
            );
        }
        let cwds = parse_lsof_cwds(
            b"p1\nn/work/a\\nb\np2\nn/work/c\\\\x41d\np3\nn/work/uni\\xe2\\x80\\xaeq\np4\nn/work/v1^2\n",
            Some(0),
        )
        .unwrap();
        assert_eq!(
            cwds.paths,
            vec![
                PathBuf::from("/work/a\nb"),
                PathBuf::from("/work/c\\x41d"),
                PathBuf::from("/work/uni\u{202e}q"),
                PathBuf::from("/work/v1^2"),
            ]
        );
    }

    #[test]
    fn size_lookup_errors_are_not_reported_as_empty_paths() {
        let root = temp("size-lookup-error");
        let non_directory = root.join("file");
        std::fs::write(&non_directory, "data").unwrap();

        let error = dir_size(&non_directory.join("child")).unwrap_err();

        assert!(error.to_string().contains("cannot inspect"));
        crate::ops::remove_test_path(root);
    }
}
