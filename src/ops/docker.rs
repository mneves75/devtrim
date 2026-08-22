//! Docker/OrbStack pruning: unused images + build cache. Volumes are NEVER
//! pruned automatically — a live database volume is user data.

use anyhow::Result;
use std::process::Command;

use super::{Finding, Op, Summary};
use crate::safety::{Ctx, escalate};

pub struct Docker;

fn docker(args: &[&str]) -> Result<String> {
    let o = Command::new("docker").args(args).output()?;
    Ok(String::from_utf8_lossy(&o.stdout).to_string())
}

impl Op for Docker {
    fn name(&self) -> &'static str {
        "docker"
    }
    fn scan(&self, _ctx: &Ctx) -> Result<Vec<Finding>> {
        if Command::new("docker").arg("version").output().map(|o| !o.status.success()).unwrap_or(true) {
            return Ok(Vec::new());
        }
        let df = docker(&["system", "df", "--format", "{{.Type}}\t{{.Size}}\t{{.Reclaimable}}"])?;
        let mut out = Vec::new();
        for line in df.lines() {
            let mut it = line.split('\t');
            let (Some(kind), Some(size), Some(recl)) = (it.next(), it.next(), it.next()) else {
                continue;
            };
            match kind {
                "Images" | "Build Cache" => {
                    let bytes = parse_size(recl);
                    if bytes == 0 {
                        continue;
                    }
                    out.push(Finding {
                        label: format!("Docker {kind} reclaimable"),
                        path: None,
                        size_bytes: bytes,
                        note: format!(
                            "prunes unused {} ({total} total); volumes are never touched",
                            if kind == "Images" { "images" } else { "build cache" },
                            total = size
                        ),
                        danger: escalate(6, bytes),
                        action: if kind == "Images" {
                            "command:docker image prune -a -f".into()
                        } else {
                            "command:docker builder prune -a -f".into()
                        },
                    });
                }
                _ => {}
            }
        }
        Ok(out)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<Summary> {
        let mut notes = Vec::new();
        let mut touched = 0usize;
        let mut bytes = 0u64;
        for f in findings {
            let cmd = f.action.trim_start_matches("command:");
            let args: Vec<&str> = cmd.split_whitespace().collect();
            let r = Command::new(args[0]).args(&args[1..]).output()?;
            bytes += f.size_bytes;
            touched += 1;
            notes.push(format!(
                "`{cmd}` → {}",
                if r.status.success() { "ok" } else { "FAILED" }
            ));
        }
        notes.push(
            "note: OrbStack compacts its disk lazily; restart OrbStack to trigger TRIM".into(),
        );
        let _ = ctx;
        Ok(Summary {
            op: self.name().into(),
            items_touched: touched,
            bytes_freed_estimate: bytes,
            notes,
        })
    }
}

/// Parse Docker size strings like "8.376GB (59%)" or "729.1kB" into bytes.
pub(crate) fn parse_size(s: &str) -> u64 {
    let s = s.split('(').next().unwrap_or("").trim();
    let (num, unit) = s.split_at(s.find(|c: char| c.is_alphabetic()).unwrap_or(s.len()));
    let v: f64 = num.trim().parse().unwrap_or(0.0);
    let mult = match unit.trim().to_lowercase().as_str() {
        "b" | "" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        _ => 1.0,
    };
    (v * mult).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_docker_sizes() {
        assert_eq!(parse_size("8.376GB (59%)"), 8_376_000_000);
        assert_eq!(parse_size("729.1kB"), 729_100);
        assert_eq!(parse_size("5.051GB"), 5_051_000_000);
        assert_eq!(parse_size("0B"), 0);
    }
}
