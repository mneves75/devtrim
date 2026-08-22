//! Safety core: context, danger gating, protected paths, size guards.
//!
//! Danger scale (mirrors ai-shell's scorer philosophy):
//!   1-2 read-only · 3-4 regenerable caches · 5-6 rebuildable state
//!   7-8 user-visible state · 9-10 irreversible bulk deletion

use anyhow::{Result, bail};
use colored::Colorize;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use crate::cli::Cli;
use crate::ops::Finding;

/// Hard denylist: never touched, no flag overrides these (except inside
/// explicit user-provided subpaths that are themselves allowlisted ops).
const PROTECTED: &[&str] = &[
    "/", "/System", "/usr", "/etc", "/private/etc", "/private/var", "/Applications", "/Library",
    "/boot", "/dev", "/Volumes",
];

/// User-level paths we must never wholesale delete (op subpaths are fine).
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
        let mut roots = vec![home.join("dev")];
        for r in &cli.roots {
            roots.push(PathBuf::from(shellexpand(r)));
        }
        // Config override if present (roots + active_days).
        let cfg = home.join(".config/devtrim.toml");
        let (cfg_roots, active_days) = load_config(&cfg);
        if !cli.roots.is_empty() {
            roots = cli.roots.iter().map(|r| PathBuf::from(shellexpand(r))).collect();
        } else if !cfg_roots.is_empty() {
            roots = cfg_roots.into_iter().map(PathBuf::from).collect();
        }
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
    std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| anyhow::anyhow!("$HOME not set"))
}

fn shellexpand(s: &str) -> String {
    s.strip_prefix("~/").map_or_else(
        || s.to_string(),
        |rest| format!("{}/{}", std::env::var("HOME").unwrap_or_default(), rest),
    )
}

#[derive(serde::Deserialize, Default)]
struct FileCfg {
    roots: Option<Vec<String>>,
    active_days: Option<u32>,
}

fn load_config(path: &Path) -> (Vec<String>, u32) {
    match std::fs::read_to_string(path) {
        Ok(s) => match toml::from_str::<FileCfg>(&s) {
            Ok(c) => (c.roots.unwrap_or_default(), c.active_days.unwrap_or(30)),
            Err(_) => (Vec::new(), 30),
        },
        Err(_) => (Vec::new(), 30),
    }
}

pub fn is_protected(path: &Path) -> bool {
    let path = abs(path);
    for p in PROTECTED {
        if path == Path::new(p) || path.starts_with(p) && p != &"/" || path == Path::new("/") {
            if path == Path::new("/private/var/folders") {
                return false; // temp dirs are fair game
            }
            return true;
        }
    }
    if let Some(rel) = path.strip_prefix(dirs_home().unwrap_or_default()).ok() {
        if let Some(first) = rel.iter().next() {
            let first = first.to_string_lossy();
            if PROTECTED_USER.contains(&first.as_ref()) {
                // Library itself protected; known op subpaths under it are not.
                if first == "Library" {
                    return !is_managed_library_subpath(&rel);
                }
                return true;
            }
        }
    }
    false
}

fn is_managed_library_subpath(rel: &Path) -> bool {
    const MANAGED: &[&str] = &[
        "Developer/Toolchains",
        "Developer/Xcode/iOS DeviceSupport",
        "Developer/Xcode/DerivedData",
        "Developer/CoreSimulator/Devices",
        "Caches/Homebrew",
        "Caches/com.apple.AMPArtworkAgent",
    ];
    let mut it = rel.iter();
    if it.next().map(|c| c != "Library").unwrap_or(true) {
        return false;
    }
    let owned = it.collect::<std::path::PathBuf>();
    let s = owned.to_string_lossy().into_owned();
    MANAGED.iter().any(|m| s == *m || s.starts_with(&format!("{m}/")))
}

fn abs(p: &Path) -> PathBuf {
    if p.is_absolute() {
        return clean(p);
    }
    match std::env::current_dir() {
        Ok(cwd) => clean(&cwd.join(p)),
        Err(_) => p.to_path_buf(),
    }
}

fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Escalate danger by size: big deletions deserve bigger numbers.
pub fn escalate(danger: u8, total_bytes: u64) -> u8 {
    const GB: u64 = 1024 * 1024 * 1024;
    let d = if total_bytes > 50 * GB {
        danger.max(8)
    } else if total_bytes > 10 * GB {
        danger.max(7)
    } else if total_bytes > 1 * GB {
        danger.max(5)
    } else {
        danger
    };
    d.min(10)
}

/// Gate before applying. Refuses non-interactive runs without explicit flags.
pub fn gate(max_danger: u8, ctx: &Ctx, findings: &[Finding]) -> Result<()> {
    if max_danger <= 2 {
        return Ok(());
    }
    if !ctx.interactive && !ctx.yes && !ctx.yolo {
        bail!("non-interactive run: re-run with -y to confirm danger-{max_danger} operations");
    }
    if ctx.yolo {
        return Ok(());
    }
    if max_danger >= 9 {
        let gb = findings.iter().map(|f| f.size_bytes).sum::<u64>() / (1024 * 1024 * 1024);
        if !ctx.interactive {
            bail!("danger-{max_danger} operation requires interactive typed confirmation or --yolo");
        }
        println!(
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
    if max_danger >= 6 && !ctx.yes {
        println!(
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

/// Trash purge has its own typed gate regardless of flags (except --yolo).
pub fn trash_gate(confirm_gb: Option<u64>) -> Result<()> {
    let Some(want) = confirm_gb else {
        bail!("Trash purge requires --confirm=<gb> matching current Trash size");
    };
    let trash_dir = dirs_home()?.join(".Trash");
    let actual_gb = dir_size(&trash_dir)? / (1024 * 1024 * 1024);
    let lo = actual_gb.saturating_sub(2);
    let hi = actual_gb + 2;
    if !(lo..=hi).contains(&want) {
        bail!(
            "--confirm={want} but Trash holds ~{actual_gb} GB; pass --confirm={actual_gb} to acknowledge"
        );
    }
    Ok(())
}

pub fn dir_size(p: &Path) -> Result<u64> {
    let mut n = 0u64;
    if !p.exists() {
        return Ok(0);
    }
    for e in walkdir::WalkDir::new(p).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        if e.file_type().is_file() {
            n += e.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    Ok(n)
}

/// Permanently remove everything inside ~/.Trash (macOS has no bulk-purge API).
pub fn purge_trash(home: &Path) -> Result<usize> {
    let dir = home.join(".Trash");
    let mut n = 0usize;
    for e in std::fs::read_dir(&dir)?.flatten() {
        let p = e.path();
        if p.is_dir() {
            std::fs::remove_dir_all(&p)?;
        } else {
            std::fs::remove_file(&p)?;
        }
        n += 1;
    }
    Ok(n)
}
