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
use std::path::Path;

pub use crate::report::{Action, Finding, Summary};
use crate::safety::Ctx;

pub use icloud::icloud_status;

pub trait Op {
    fn name(&self) -> &'static str;
    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>>;
    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<Summary>;
}

pub struct ScanResult {
    pub findings: Vec<Finding>,
    pub errors: Vec<String>,
}

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
    all().iter().map(|operation| operation.name()).collect()
}

pub fn by_name(name: &str) -> Option<Box<dyn Op>> {
    all().into_iter().find(|operation| operation.name() == name)
}

pub fn scan_all(ctx: &Ctx) -> ScanResult {
    let mut findings = Vec::new();
    let mut errors = Vec::new();
    for operation in all() {
        match operation.scan(ctx) {
            Ok(mut operation_findings) => findings.append(&mut operation_findings),
            Err(error) => errors.push(format!("{}: {error:#}", operation.name())),
        }
    }
    ScanResult { findings, errors }
}

pub fn dir_size(path: &Path) -> u64 {
    crate::safety::dir_size(path).unwrap_or(0)
}

pub fn remove_path(path: &Path, ctx: &Ctx) -> Result<()> {
    remove_path_with_mode(path, ctx, ctx.shred)
}

fn remove_path_with_mode(path: &Path, ctx: &Ctx, permanent: bool) -> Result<()> {
    let path = crate::safety::validate_path_for_deletion(path, &ctx.home)?;
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() {
        if permanent {
            std::fs::remove_file(path)?;
        } else {
            trash::delete(path)?;
        }
        return Ok(());
    }
    if permanent && metadata.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else if permanent {
        std::fs::remove_file(path)?;
    } else {
        trash::delete(path)?;
    }
    Ok(())
}

pub fn purge_trash(ctx: &Ctx) -> Result<usize> {
    let directory = crate::safety::validate_trash_root(&ctx.home)?;
    let mut count = 0usize;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        remove_path_with_mode(&entry.path(), ctx, true)?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    fn context(home: PathBuf) -> Ctx {
        Ctx {
            yes: true,
            yolo: false,
            shred: true,
            json: false,
            roots: vec![],
            active_days: 30,
            home,
            interactive: false,
        }
    }

    #[test]
    fn shared_sink_deletes_a_verified_normal_path() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-normal-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(home.join("dev/node_modules")).unwrap();
        let home = home.canonicalize().unwrap();
        let target = home.join("dev/node_modules");
        remove_path(&target, &context(home.clone())).unwrap();
        assert!(!target.exists());
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn shared_sink_rejects_symlinked_ancestor() {
        let home = std::env::temp_dir().join(format!("devtrim-sink-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(home.join("dev")).unwrap();
        std::fs::create_dir_all(home.join("Library/node_modules")).unwrap();
        symlink(home.join("Library"), home.join("dev/link")).unwrap();
        let target = home.join("dev/link/node_modules");
        assert!(remove_path(&target, &context(home.clone())).is_err());
        assert!(home.join("Library/node_modules").exists());
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn purge_trash_refuses_symlinked_root_and_preserves_sentinel() {
        let home = std::env::temp_dir().join(format!("devtrim-purge-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        let outside = home.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let sentinel = outside.join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        symlink(&outside, home.join(".Trash")).unwrap();

        assert!(purge_trash(&context(home.clone())).is_err());
        assert_eq!(std::fs::read_to_string(sentinel).unwrap(), "keep");

        std::fs::remove_file(home.join(".Trash")).ok();
        std::fs::remove_dir_all(home).ok();
    }
}
