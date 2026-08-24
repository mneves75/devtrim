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
use crate::safety::{Ctx, VerifiedTarget};

pub use icloud::icloud_status;

pub trait Op {
    fn name(&self) -> &'static str;
    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>>;
    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome>;
}

#[derive(Debug)]
pub struct ApplyOutcome {
    pub summary: Summary,
    pub errors: Vec<String>,
}

impl ApplyOutcome {
    pub fn new(operation: &str) -> Self {
        Self {
            summary: Summary {
                op: operation.into(),
                items_touched: 0,
                bytes_freed_estimate: 0,
                notes: Vec::new(),
            },
            errors: Vec::new(),
        }
    }

    pub fn record(&mut self, finding: &Finding, note: String) {
        self.summary.items_touched += 1;
        self.summary.bytes_freed_estimate += finding.size_bytes;
        self.summary.notes.push(note);
    }

    pub fn fail(&mut self, error: anyhow::Error) {
        self.errors.push(format!("{error:#}"));
    }
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

pub fn dir_size(path: &Path) -> Result<u64> {
    crate::safety::dir_size(path)
}

pub fn apply_filesystem_finding(finding: &Finding, ctx: &Ctx) -> Result<()> {
    let permanent = match finding.action {
        Action::Trash => false,
        Action::Shred => true,
        _ => anyhow::bail!("refusing non-filesystem action at deletion sink"),
    };
    let target = finding
        .target()
        .ok_or_else(|| anyhow::anyhow!("filesystem finding missing internal target"))?;
    let verified = crate::safety::validate_path_for_deletion(target, &ctx.home)?;
    remove_path(verified, permanent)
}

fn remove_path(target: VerifiedTarget, permanent: bool) -> Result<()> {
    let path = target.into_path();
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() {
        if permanent {
            std::fs::remove_file(&path)?;
        } else {
            trash::delete(&path)?;
        }
        return Ok(());
    }
    if permanent && metadata.is_dir() {
        std::fs::remove_dir_all(&path)?;
    } else if permanent {
        std::fs::remove_file(&path)?;
    } else {
        trash::delete(&path)?;
    }
    Ok(())
}

pub fn purge_trash(ctx: &Ctx) -> Result<usize> {
    let directory = crate::safety::validate_trash_root(&ctx.home)?;
    let mut count = 0usize;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let target = crate::safety::validate_path_for_deletion(&entry.path(), &ctx.home)?;
        remove_path(target, true)?;
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
pub(crate) fn remove_test_path(path: impl AsRef<Path>) {
    let path = path.as_ref();
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_dir() {
        std::fs::remove_dir_all(path).ok();
    } else {
        std::fs::remove_file(path).ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    use std::path::PathBuf;

    fn context(home: PathBuf) -> Ctx {
        Ctx {
            yes: true,
            yolo: false,
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
        let finding = Finding::new(
            "node_modules",
            Some(target.clone()),
            0,
            "test",
            9,
            Action::Shred,
        );
        apply_filesystem_finding(&finding, &context(home.clone())).unwrap();
        assert!(!target.exists());
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn shared_sink_uses_exact_non_utf8_target_identity() {
        let home = std::env::current_dir()
            .unwrap()
            .canonicalize()
            .unwrap()
            .join("target")
            .join(format!("devtrim-exact-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(&home).unwrap();
        let target = home.join(std::ffi::OsString::from_vec(vec![b'c', 0xff]));
        assert!(target.to_str().is_none());
        let finding = Finding::new(
            "non-UTF-8 cache",
            Some(target.clone()),
            0,
            "test",
            9,
            Action::Shred,
        );
        assert_eq!(finding.target(), Some(target.as_path()));
        match std::fs::create_dir_all(&target) {
            Ok(()) => {}
            Err(_) => {
                std::fs::remove_dir_all(home).ok();
                return;
            }
        }
        apply_filesystem_finding(&finding, &context(home.clone())).unwrap();
        assert!(!target.exists());
        std::fs::remove_dir_all(home).ok();
    }

    #[test]
    fn shared_sink_rejects_missing_targets_and_non_filesystem_actions() {
        let home = std::env::current_dir()
            .unwrap()
            .canonicalize()
            .unwrap()
            .join("target")
            .join(format!("devtrim-reject-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(&home).unwrap();
        let target = home.join("sentinel");
        std::fs::write(&target, "keep").unwrap();
        let missing = Finding::new("missing", None, 0, "test", 9, Action::Shred);
        assert!(apply_filesystem_finding(&missing, &context(home.clone())).is_err());

        let command = Finding::new(
            "wrong action",
            Some(target.clone()),
            0,
            "test",
            1,
            Action::Info,
        );
        assert!(apply_filesystem_finding(&command, &context(home.clone())).is_err());
        assert!(target.exists());
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
        let finding = Finding::new("node_modules", Some(target), 0, "test", 9, Action::Shred);
        assert!(apply_filesystem_finding(&finding, &context(home.clone())).is_err());
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
