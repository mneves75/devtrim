//! Op registry: every category is scan-then-apply with a danger score.

pub mod caches;
pub mod docker;
pub mod icloud;
pub mod leftovers;
pub mod node_modules;
pub mod simulators;
pub mod toolchains;
pub mod xcode;

use anyhow::Result;
use colored::Colorize;
use std::path::Path;

pub use crate::report::{Finding, Summary};
use crate::safety::Ctx;

pub use icloud::icloud_status;

pub trait Op {
    fn name(&self) -> &'static str;
    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>>;
    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<Summary>;
}

/// All ops in stable display order.
pub fn all() -> Vec<Box<dyn Op>> {
    vec![
        Box::new(caches::Caches),
        Box::new(node_modules::NodeModules),
        Box::new(simulators::Simulators),
        Box::new(xcode::Xcode),
        Box::new(docker::Docker),
        Box::new(toolchains::Toolchains),
        Box::new(leftovers::Leftovers),
    ]
}

pub fn names() -> Vec<&'static str> {
    all().iter().map(|o| o.name()).collect()
}

pub fn by_name(name: &str) -> Option<Box<dyn Op>> {
    all().into_iter().find(|o| o.name() == name)
}

/// Scan every op; read-only. Used by `devtrim scan`.
pub fn scan_all(ctx: &Ctx) -> Result<Vec<Finding>> {
    let mut out = Vec::new();
    for op in all() {
        match op.scan(ctx) {
            Ok(mut f) => out.append(&mut f),
            Err(e) => {
                if !ctx.json {
                    eprintln!("{} {}: {e}", "warn".yellow(), op.name());
                }
            }
        }
    }
    Ok(out)
}

// ---------- shared helpers ----------

pub fn dir_size(p: &Path) -> u64 {
    crate::safety::dir_size(p).unwrap_or(0)
}

/// Move to Trash (default) or permanently delete (--shred).
/// Protected paths are refused unconditionally.
pub fn remove_path(path: &Path, ctx: &Ctx) -> Result<()> {
    if crate::safety::is_protected(path) {
        anyhow::bail!("refusing protected path: {}", path.display());
    }
    if ctx.shred {
        if path.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
    } else {
        trash::delete(path)?;
    }
    Ok(())
}
