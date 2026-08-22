//! Xcode support files. Archives are deliberately exempt (release artifacts).

use anyhow::Result;

use super::{Finding, Op, Summary, dir_size, remove_path};
use crate::safety::{Ctx, escalate};

pub struct Xcode;

const TARGETS: &[(&str, &str, &str)] = &[
    (
        "iOS DeviceSupport (symbol caches)",
        "Developer/Xcode/iOS DeviceSupport",
        "rebuilt on next device connect/debug",
    ),
    ("DerivedData", "Developer/Xcode/DerivedData", "rebuilt on next build"),
];

impl Op for Xcode {
    fn name(&self) -> &'static str {
        "xcode"
    }
    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>> {
        let mut out = Vec::new();
        for (label, rel, note) in TARGETS {
            let p = ctx.home.join("Library").join(rel);
            let size = dir_size(&p);
            if size == 0 {
                continue;
            }
            out.push(Finding {
                label: label.to_string(),
                path: Some(p.display().to_string()),
                size_bytes: size,
                note: note.to_string(),
                danger: escalate(4, size),
                action: "trash".into(),
            });
        }
        let archives = ctx.home.join("Library/Developer/Xcode/Archives");
        let a_size = dir_size(&archives);
        if a_size > 0 {
            out.push(Finding {
                label: "Xcode Archives".into(),
                path: Some(archives.display().to_string()),
                size_bytes: a_size,
                note: "EXCLUDED: release artifacts — listed for visibility only".into(),
                danger: 0,
                action: "none".into(),
            });
        }
        Ok(out)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<Summary> {
        let mut notes = Vec::new();
        let mut touched = 0usize;
        let mut bytes = 0u64;
        for f in findings {
            if f.action == "none" {
                notes.push("skipped Archives by design".into());
                continue;
            }
            if let Some(path) = &f.path {
                // Trash children, keep the parent dir so Xcode finds it again.
                if let Ok(entries) = std::fs::read_dir(path) {
                    for e in entries.flatten() {
                        remove_path(&e.path(), ctx)?;
                    }
                }
                bytes += f.size_bytes;
                touched += 1;
                notes.push(format!("cleared {}", f.label));
            }
        }
        Ok(Summary {
            op: self.name().into(),
            items_touched: touched,
            bytes_freed_estimate: bytes,
            notes,
        })
    }
}

