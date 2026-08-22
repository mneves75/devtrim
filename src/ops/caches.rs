//! Regenerable package-manager caches. Low risk: everything re-downloads.

use anyhow::Result;
use std::path::PathBuf;

use super::{Finding, Op, Summary, dir_size, remove_path};
use crate::safety::{Ctx, escalate};

pub struct Caches;

const CACHES: &[(&str, &str)] = &[
    ("huggingface model cache", ".cache/huggingface"),
    ("codex runtimes cache", ".cache/codex-runtimes"),
    ("uv package cache", ".cache/uv"),
    ("node core cache", ".cache/node"),
];

impl Op for Caches {
    fn name(&self) -> &'static str {
        "caches"
    }
    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>> {
        let mut out = Vec::new();
        for (label, rel) in CACHES {
            let p = ctx.home.join(rel);
            let size = dir_size(&p);
            if size == 0 {
                continue;
            }
            out.push(Finding {
                label: label.to_string(),
                path: Some(p.display().to_string()),
                size_bytes: size,
                note: "re-downloads automatically on next use".into(),
                danger: escalate(3, size),
                action: "trash".into(),
            });
        }
        // npm cache
        if let Ok(outp) = std::process::Command::new("npm")
            .args(["config", "get", "cache"])
            .output()
        {
            let dir = PathBuf::from(String::from_utf8_lossy(&outp.stdout).trim());
            let size = dir_size(&dir);
            if size > 0 {
                out.push(Finding {
                    label: "npm download cache".into(),
                    path: Some(dir.display().to_string()),
                    size_bytes: size,
                    note: "`npm cache clean --force` equivalent; safe".into(),
                    danger: escalate(2, size),
                    action: format!("command:npm cache clean --force ({})", dir.display()),
                });
            }
        }
        // brew cache
        if let Ok(outp) = std::process::Command::new("brew").arg("--cache").output() {
            let dir = PathBuf::from(String::from_utf8_lossy(&outp.stdout).trim());
            let size = dir_size(&dir);
            if size > 0 {
                out.push(Finding {
                    label: "homebrew downloads cache".into(),
                    path: Some(dir.display().to_string()),
                    size_bytes: size,
                    note: "re-downloaded on next install; safe".into(),
                    danger: escalate(1, size),
                    action: "trash".into(),
                });
            }
        }
        Ok(out)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<Summary> {
        let mut notes = Vec::new();
        let mut touched = 0usize;
        let mut bytes = 0u64;
        for f in findings {
            bytes += f.size_bytes;
            if f.action.starts_with("command:") {
                let cmd_part = f.action.trim_start_matches("command:");
                let (cmd, rest) = cmd_part.split_once(' ').unwrap_or((cmd_part, ""));
                let args: Vec<&str> = rest
                    .split_whitespace()
                    .filter(|a| !a.starts_with('('))
                    .collect();
                match std::process::Command::new(cmd).args(&args).output() {
                    Ok(o) if o.status.success() => {
                        touched += 1;
                        notes.push(format!("ran `{cmd} {}`", args.join(" ")));
                    }
                    _ => notes.push(format!("FAILED: {}", f.action)),
                }
            } else if let Some(path) = &f.path {
                remove_path(std::path::Path::new(path), ctx)?;
                touched += 1;
                notes.push(format!("trashed {}", f.label));
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
