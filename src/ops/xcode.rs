//! Xcode support files. Archives are deliberately exempt release artifacts.

use anyhow::Result;
use std::path::Path;

use super::{Action, Finding, Op, Summary, dir_size, remove_path};
use crate::safety::{Ctx, escalate};

pub struct Xcode;

const TARGETS: &[(&str, &str, &str)] = &[
    (
        "iOS DeviceSupport",
        "Developer/Xcode/iOS DeviceSupport",
        "symbol cache; rebuilt on next device connect/debug",
    ),
    (
        "DerivedData",
        "Developer/Xcode/DerivedData",
        "build output; rebuilt on next build",
    ),
];

impl Op for Xcode {
    fn name(&self) -> &'static str {
        "xcode"
    }

    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        for (label, relative, note) in TARGETS {
            let directory = ctx.home.join("Library").join(relative);
            let entries = match std::fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            for entry in entries {
                let path = entry?.path();
                let size = dir_size(&path);
                findings.push(Finding {
                    label: format!(
                        "{label}: {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    path: Some(path.display().to_string()),
                    size_bytes: size,
                    note: (*note).into(),
                    danger: escalate(4, size),
                    action: Action::Trash,
                });
            }
        }
        let archives = ctx.home.join("Library/Developer/Xcode/Archives");
        let archive_size = dir_size(&archives);
        if archive_size > 0 {
            findings.push(Finding {
                label: "Xcode Archives".into(),
                path: Some(archives.display().to_string()),
                size_bytes: archive_size,
                note: "EXCLUDED: release artifacts; listed for visibility only".into(),
                danger: 0,
                action: Action::None,
            });
        }
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<Summary> {
        let mut notes = Vec::new();
        let mut touched = 0usize;
        let mut bytes = 0u64;
        for finding in findings {
            if !matches!(finding.action, Action::Trash | Action::Shred) {
                if finding.action == Action::None {
                    notes.push("skipped Xcode Archives by design".into());
                }
                continue;
            }
            let path = finding
                .path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Xcode finding missing path"))?;
            remove_path(Path::new(path), ctx)?;
            touched += 1;
            bytes += finding.size_bytes;
            notes.push(format!(
                "{} {path}",
                if ctx.shred {
                    "permanently deleted"
                } else {
                    "trashed"
                }
            ));
        }
        Ok(Summary {
            op: self.name().into(),
            items_touched: touched,
            bytes_freed_estimate: bytes,
            notes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archives_are_never_applied_or_counted() {
        let home = std::env::temp_dir().join(format!("devtrim-xcode-{}", std::process::id()));
        std::fs::remove_dir_all(&home).ok();
        let archive = home.join("Library/Developer/Xcode/Archives/release.xcarchive");
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::write(archive.join("sentinel"), "keep").unwrap();
        let finding = Finding {
            label: "Xcode Archives".into(),
            path: Some(archive.display().to_string()),
            size_bytes: 4,
            note: "excluded".into(),
            danger: 0,
            action: Action::None,
        };
        let ctx = Ctx {
            yes: true,
            yolo: false,
            shred: true,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            home: home.clone(),
            interactive: false,
        };

        let summary = Xcode.apply(&[finding], &ctx).unwrap();
        assert_eq!(summary.items_touched, 0);
        assert_eq!(summary.bytes_freed_estimate, 0);
        assert!(archive.join("sentinel").exists());

        std::fs::remove_dir_all(home).ok();
    }
}
