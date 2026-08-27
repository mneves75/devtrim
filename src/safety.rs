//! Safety core: context, danger gating, protected paths, and size guards.

use anyhow::{Context, Result, bail};
use colored::Colorize;
use std::cell::RefCell;
use std::ffi::OsString;
use std::io::IsTerminal;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    pub(crate) diagnostics: RefCell<Vec<String>>,
    /// Journal failures observed after a mutation already succeeded; drained
    /// into the apply outcome so automation sees them with a nonzero status.
    pub(crate) journal_errors: RefCell<Vec<String>>,
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
        let roots = roots
            .into_iter()
            .map(|root| {
                if root.exists() {
                    root.canonicalize()
                        .with_context(|| format!("cannot resolve scan root: {}", root.display()))
                } else {
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
            diagnostics: RefCell::new(Vec::new()),
            journal_errors: RefCell::new(Vec::new()),
        };
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
                .borrow_mut()
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
        std::mem::take(&mut *self.diagnostics.borrow_mut())
    }

    pub(crate) fn record_journal_error(&self, message: String) {
        self.journal_errors.borrow_mut().push(message);
    }

    pub(crate) fn take_journal_errors(&self) -> Vec<String> {
        std::mem::take(&mut *self.journal_errors.borrow_mut())
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
    let mut protect = Vec::with_capacity(entries.len());
    let mut warnings = Vec::new();
    for entry in entries {
        let expanded = PathBuf::from(shellexpand(&entry, home));
        if !expanded.is_absolute() {
            bail!("protect entry `{entry}` must expand to an absolute path");
        }
        let cleaned = clean(&expanded);
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
            if resolved != cleaned {
                protect.push(resolved);
            }
        } else {
            warnings.push(format!(
                "protect entry `{entry}` does not currently resolve to an existing path"
            ));
        }
        protect.push(cleaned);
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
    let literal = abs(path);
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
    Ok(VerifiedTarget(literal))
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
    protect.iter().any(|protected| {
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
    MANAGED
        .iter()
        .any(|managed| owned == *managed || owned.starts_with(&format!("{managed}/")))
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

pub fn trash_gate(home: &Path, confirm_gb: Option<u64>) -> Result<()> {
    let Some(want) = confirm_gb else {
        bail!("Trash purge requires --confirm=<gb> matching current Trash size");
    };
    let actual_gb = dir_size(&home.join(".Trash"))? / (1024 * 1024 * 1024);
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
    let mut bytes = 0u64;
    if !path.exists() {
        return Ok(0);
    }
    for entry in walkdir::WalkDir::new(path)
        .follow_links(false)
        .follow_root_links(false)
    {
        let entry = entry.with_context(|| format!("cannot measure {}", path.display()))?;
        if entry.file_type().is_file() {
            let len = entry
                .metadata()
                .with_context(|| format!("cannot measure {}", entry.path().display()))?
                .len();
            bytes = bytes
                .checked_add(len)
                .ok_or_else(|| anyhow::anyhow!("logical size overflow under {}", path.display()))?;
        }
    }
    Ok(bytes)
}

const BUILD_PROCESS_PATTERN: &str = "node|npm|pnpm|yarn|bun|deno|cargo|rustc|go|python|python3|Python|gradle|java|xcodebuild|swift|swiftc|make|ninja|cmake";

pub(crate) fn build_process_cwds() -> Result<Vec<PathBuf>> {
    let pgrep = Command::new("pgrep")
        .args(["-x", BUILD_PROCESS_PATTERN])
        .output()
        .context("cannot run build-process pgrep probe")?;
    let pids = parse_pgrep_pids(&pgrep.stdout, pgrep.status.code())?;
    if pids.is_empty() {
        return Ok(Vec::new());
    }
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

pub(crate) fn xcodebuild_running() -> Result<bool> {
    let output = Command::new("pgrep")
        .args(["-x", "xcodebuild"])
        .output()
        .context("cannot run xcodebuild liveness probe")?;
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

pub(crate) fn parse_lsof_cwds(output: &[u8], exit_code: Option<i32>) -> Result<Vec<PathBuf>> {
    match exit_code {
        Some(0 | 1) => {}
        Some(code) => bail!("lsof cwd probe exited with status {code}"),
        None => bail!("lsof cwd probe terminated without an exit status"),
    }
    let mut paths = Vec::new();
    for line in output.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(path) = line.strip_prefix(b"n") else {
            continue;
        };
        if path.is_empty() {
            bail!("lsof returned an empty cwd path");
        }
        paths.push(PathBuf::from(OsString::from_vec(path.to_vec())));
    }
    if exit_code == Some(0) && paths.is_empty() {
        bail!("lsof reported success without any cwd paths");
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
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
    fn protects_home_and_user_secrets() {
        let home = PathBuf::from("/Users/example");
        assert!(is_protected(&home, &home));
        assert!(is_protected(&home.join(".ssh/key"), &home));
        assert!(!is_protected(&home.join("dev/project"), &home));
    }

    #[test]
    fn protects_case_variant_aliases() {
        let home = PathBuf::from("/Users/example");
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
    }

    #[test]
    fn protected_path_comparison_uses_normalized_components() {
        assert!(path_eq_ignore_ascii_case(
            Path::new("/System/"),
            Path::new("/system")
        ));
    }

    #[test]
    fn configured_protect_expands_tilde_and_rejects_relative_entries() {
        let home = Path::new("/Users/example");
        let (protect, warnings) = configured_protect(vec!["~/dev/keep".into()], home).unwrap();
        assert_eq!(protect, vec![home.join("dev/keep")]);
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
        assert_eq!(cwds, vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]);
        assert!(parse_lsof_cwds(b"", Some(1)).unwrap().is_empty());
        assert!(parse_lsof_cwds(b"", Some(0)).is_err());
        assert!(parse_lsof_cwds(b"n/tmp\n", Some(3)).is_err());
    }
}
