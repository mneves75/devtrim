//! Safety core: context, danger gating, protected paths, and size guards.

use anyhow::{Context, Result, bail};
use colored::Colorize;
use std::cell::RefCell;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

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
    pub interactive: bool,
    pub(crate) diagnostic_output: DiagnosticOutput,
    pub(crate) diagnostics: RefCell<Vec<String>>,
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
        let (cfg_roots, active_days) = load_config(&cfg)?;
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
        Ok(Self {
            yes: cli.yes,
            yolo: cli.yolo,
            json: cli.json,
            roots,
            active_days,
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
        })
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
struct FileCfg {
    roots: Option<Vec<String>>,
    active_days: Option<u32>,
}

fn load_config(path: &Path) -> Result<(Vec<String>, u32)> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            let config: FileCfg = toml::from_str(&contents)
                .with_context(|| format!("invalid config: {}", path.display()))?;
            Ok((
                config.roots.unwrap_or_default(),
                // active_days = 0 would mark every repo stale and disable the guard.
                config.active_days.unwrap_or(30).max(1),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok((Vec::new(), 30)),
        Err(error) => Err(error).with_context(|| format!("cannot read config: {}", path.display())),
    }
}

/// A pathname that passed the deletion boundary's current safety checks.
///
/// Validation has a documented pathname TOCTOU limitation: it does not hold an
/// open descriptor across deletion, so ambiguous identity must still fail closed.
#[derive(Debug)]
pub(crate) struct VerifiedTarget(PathBuf);

impl VerifiedTarget {
    pub(crate) fn into_path(self) -> PathBuf {
        self.0
    }
}

pub(crate) fn validate_path_for_deletion(path: &Path, home: &Path) -> Result<VerifiedTarget> {
    let literal = abs(path);
    if is_protected(&literal, home) {
        bail!("refusing protected path: {}", literal.display());
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

fn clean(path: &Path) -> PathBuf {
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
            let verified = validate_path_for_deletion(&target, &home).unwrap();
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
        let error = validate_path_for_deletion(&target, &home).unwrap_err();
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
            home: PathBuf::from("/Users/example"),
            interactive: false,
            diagnostic_output: DiagnosticOutput::Stderr,
            diagnostics: Default::default(),
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
}
