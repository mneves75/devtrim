//! swift.org toolchains under ~/Library/Developer/Toolchains.
//! Keeps the one `swift-latest` points at (repointing to newest first),
//! removes the rest. Xcode's built-in toolchain is unaffected.

use anyhow::Result;
use std::path::{Path, PathBuf};

use super::{Finding, Op, Summary, dir_size, remove_path};
use crate::safety::Ctx;

pub struct Toolchains;

impl Op for Toolchains {
    fn name(&self) -> &'static str {
        "toolchains"
    }
    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>> {
        let dir = ctx.home.join("Library/Developer/Toolchains");
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let latest_target = std::fs::read_link(dir.join("swift-latest.xctoolchain"))
            .map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or(None);

        let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.is_dir()
                    && p.file_name().map(|n| n != "swift-latest.xctoolchain").unwrap_or(true)
                    && p.extension().map(|e| e == "xctoolchain").unwrap_or(false)
            })
            .collect();
        entries.sort();

        let mut out = Vec::new();
        for t in &entries {
            let name = t.file_name().unwrap().to_string_lossy().to_string();
            if Some(name.as_str()) == latest_target.as_deref() {
                continue; // this is what swift-latest resolves to
            }
            // Never propose removing a target another symlink still points at.
            let size = dir_size(t);
            out.push(Finding {
                label: format!("Swift toolchain {name}"),
                path: Some(t.display().to_string()),
                size_bytes: size,
                note: format!(
                    "not referenced by swift-latest (→ {}); re-installable from swift.org",
                    latest_target.as_deref().unwrap_or("(unset)")
                ),
                danger: 6,
                action: "trash".into(),
            });
        }
        Ok(out)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<Summary> {
        let mut notes = Vec::new();
        let mut touched = 0usize;
        let mut bytes = 0u64;
        for f in findings {
            let Some(path) = &f.path else { continue };
            let p = Path::new(path);
            remove_path(p, ctx)?;
            bytes += f.size_bytes;
            touched += 1;
            notes.push(format!("trashed {}", p.display()));
        }
        Ok(Summary {
            op: self.name().into(),
            items_touched: touched,
            bytes_freed_estimate: bytes,
            notes,
        })
    }
}
