//! Safety core: context, danger gating, protected paths, and size guards.

use anyhow::{Context, Result, bail};
use colored::Colorize;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::cli::Cli;
use crate::ops::Finding;

const PROTECTED: &[&str] = &[
    "/",
    "/System",
    "/usr",
    "/etc",
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
    pub shred: bool,
    pub json: bool,
    pub roots: Vec<PathBuf>,
    pub active_days: u32,
    pub home: PathBuf,
    pub interactive: bool,
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
            shred: cli.shred,
            json: cli.json,
            roots,
            active_days,
            interactive: std::io::stdin().is_terminal(),
            home,
        })
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

pub fn validate_path_for_deletion(path: &Path, home: &Path) -> Result<PathBuf> {
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
    Ok(literal)
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
    if path == home || path == home.join(".Trash") {
        return true;
    }
    for protected in PROTECTED {
        let protected = Path::new(protected);
        if path == protected || (protected != Path::new("/") && path.starts_with(protected)) {
            return true;
        }
    }
    let Ok(relative) = path.strip_prefix(home) else {
        return false;
    };
    let Some(first) = relative.iter().next() else {
        return false;
    };
    let first = first.to_string_lossy();
    if PROTECTED_USER.contains(&first.as_ref()) {
        if first == "Library" {
            return !is_managed_library_subpath(relative);
        }
        return true;
    }
    false
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

pub fn gate(max_danger: u8, ctx: &Ctx, findings: &[Finding]) -> Result<()> {
    if !ctx.interactive && !ctx.yes && !ctx.yolo {
        bail!("non-interactive run: re-run with -y to confirm danger-{max_danger} operations");
    }
    if ctx.yolo {
        return Ok(());
    }
    if max_danger >= 9 {
        if !ctx.interactive {
            bail!(
                "danger-{max_danger} operation requires interactive typed confirmation or --yolo"
            );
        }
        let gb = crate::report::actionable_bytes(findings) / (1024 * 1024 * 1024);
        eprintln!(
            "{} about to irreversibly remove ~{gb} GB. Type the number to continue:",
            "CRITICAL".red().bold()
        );
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if line.trim() != gb.to_string() {
            bail!("confirmation mismatch — aborted");
        }
        return Ok(());
    }
    if max_danger >= 3 && !ctx.yes {
        eprintln!(
            "{} danger-{max_danger}: proceed? [y/N]",
            "confirm".yellow().bold()
        );
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !line.trim().eq_ignore_ascii_case("y") {
            bail!("aborted by user");
        }
    }
    Ok(())
}

pub fn trash_gate(home: &Path, confirm_gb: Option<u64>) -> Result<()> {
    let Some(want) = confirm_gb else {
        bail!("Trash purge requires --confirm=<gb> matching current Trash size");
    };
    let actual_gb = dir_size(&home.join(".Trash"))? / (1024 * 1024 * 1024);
    let low = actual_gb.saturating_sub(2);
    let high = actual_gb + 2;
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
        .into_iter()
        .filter_map(Result::ok)
    {
        if entry.file_type().is_file() {
            bytes += entry.metadata().map(|metadata| metadata.len()).unwrap_or(0);
        }
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn temp(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("devtrim-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn protects_home_and_user_secrets() {
        let home = temp("protected");
        assert!(is_protected(&home, &home));
        assert!(is_protected(&home.join(".ssh/key"), &home));
        assert!(!is_protected(&home.join("dev/project"), &home));
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn rejects_symlinked_ancestor() {
        let home = temp("ancestor");
        let safe = home.join("dev");
        let protected = home.join("Library");
        std::fs::create_dir_all(protected.join("node_modules")).unwrap();
        std::fs::create_dir_all(&safe).unwrap();
        symlink(&protected, safe.join("linked")).unwrap();
        let target = safe.join("linked/node_modules");
        let error = validate_path_for_deletion(&target, &home).unwrap_err();
        assert!(error.to_string().contains("symlinked ancestor"));
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn refuses_symlinked_trash() {
        let home = temp("trash-link");
        let target = home.join("elsewhere");
        std::fs::create_dir_all(&target).unwrap();
        symlink(&target, home.join(".Trash")).unwrap();
        assert!(validate_trash_root(&home).is_err());
        std::fs::remove_file(home.join(".Trash")).ok();
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn aggregate_size_escalates() {
        assert_eq!(escalate(3, 11 * 1024 * 1024 * 1024), 7);
        assert_eq!(escalate(3, 51 * 1024 * 1024 * 1024), 8);
    }
}
