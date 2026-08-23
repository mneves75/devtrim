//! Docker/OrbStack pruning: unused images + build cache. Volumes are never pruned.

use anyhow::{Context, Result};
use std::process::Command;

use super::{Action, Finding, Op, Summary};
use crate::safety::{Ctx, escalate};

pub struct Docker;

fn docker(args: &[&str]) -> Result<String> {
    let output = Command::new("docker").args(args).output()?;
    if !output.status.success() {
        anyhow::bail!("`docker {}` failed", args.join(" "));
    }
    String::from_utf8(output.stdout).context("docker returned non-UTF-8 output")
}

impl Op for Docker {
    fn name(&self) -> &'static str {
        "docker"
    }

    fn scan(&self, _ctx: &Ctx) -> Result<Vec<Finding>> {
        let version = match Command::new("docker").arg("version").output() {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        if !version.status.success() {
            return Ok(Vec::new());
        }
        let output = docker(&[
            "system",
            "df",
            "--format",
            "{{.Type}}\t{{.Size}}\t{{.Reclaimable}}",
        ])?;
        let mut findings = Vec::new();
        for line in output.lines() {
            let mut columns = line.split('\t');
            let (Some(kind), Some(total), Some(reclaimable)) =
                (columns.next(), columns.next(), columns.next())
            else {
                continue;
            };
            let (label, args) = match kind {
                "Images" => ("Docker Images reclaimable", ["image", "prune", "-a", "-f"]),
                "Build Cache" => (
                    "Docker Build Cache reclaimable",
                    ["builder", "prune", "-a", "-f"],
                ),
                _ => continue,
            };
            let bytes = parse_size(reclaimable);
            if bytes == 0 {
                continue;
            }
            findings.push(Finding {
                label: label.into(),
                path: None,
                size_bytes: bytes,
                note: format!(
                    "{total} total; removes every image not referenced by a container, including untagged local builds; volumes are never touched"
                ),
                danger: escalate(6, bytes),
                action: Action::command("docker", &args),
            });
        }
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], _ctx: &Ctx) -> Result<Summary> {
        let mut notes = Vec::new();
        let mut touched = 0usize;
        let mut bytes = 0u64;
        for finding in findings {
            let Action::Command { program, args } = &finding.action else {
                continue;
            };
            let expected = args == &["image", "prune", "-a", "-f"]
                || args == &["builder", "prune", "-a", "-f"];
            if program != "docker" || !expected {
                anyhow::bail!("refusing unexpected Docker action");
            }
            let output = Command::new(program).args(args).output()?;
            if !output.status.success() {
                anyhow::bail!("`{program} {}` failed", args.join(" "));
            }
            touched += 1;
            bytes += finding.size_bytes;
            notes.push(format!("`{program} {}` completed", args.join(" ")));
        }
        notes.push("OrbStack compacts its disk lazily; restart it to trigger TRIM".into());
        Ok(Summary {
            op: self.name().into(),
            items_touched: touched,
            bytes_freed_estimate: bytes,
            notes,
        })
    }
}

pub(crate) fn parse_size(value: &str) -> u64 {
    let value = value.split('(').next().unwrap_or("").trim();
    let split = value
        .find(|character: char| character.is_alphabetic())
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: f64 = number.trim().parse().unwrap_or(0.0);
    let multiplier = match unit.trim().to_lowercase().as_str() {
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
    (number * multiplier).round() as u64
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
