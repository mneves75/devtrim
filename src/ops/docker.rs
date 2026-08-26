//! Docker/OrbStack pruning: unused images + build cache. Volumes are never pruned.

use anyhow::{Context, Result};
use std::process::Command;

use super::{ApplyOutcome, Finding, Op};
use crate::report::CommandAuthority;
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
            let (label, authority) = match kind {
                "Images" => (
                    "Docker Images reclaimable",
                    CommandAuthority::DockerImagePrune,
                ),
                "Build Cache" => (
                    "Docker Build Cache reclaimable",
                    CommandAuthority::DockerBuilderPrune,
                ),
                _ => continue,
            };
            let bytes = parse_size(reclaimable)
                .with_context(|| format!("invalid Docker reclaimable size: {reclaimable}"))?;
            if bytes == 0 {
                continue;
            }
            findings.push(Finding::command(
                label,
                bytes,
                format!(
                    "{total} total; removes every image not referenced by a container, including untagged local builds; volumes are never touched"
                ),
                escalate(6, bytes),
                authority,
            ));
        }
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let mut outcome = ApplyOutcome::new(self.name());
        for finding in findings {
            let result = (|| -> Result<String> {
                let Some(authority) = finding.command_authority() else {
                    anyhow::bail!("refusing unexpected Docker action");
                };
                if !matches!(
                    authority,
                    CommandAuthority::DockerImagePrune | CommandAuthority::DockerBuilderPrune
                ) {
                    anyhow::bail!("refusing unexpected Docker action");
                }
                if finding.action != authority.action() {
                    anyhow::bail!("refusing altered Docker action");
                }
                let (program, args) = authority.parts();
                let attempt = crate::journal::JournalRecord::command_attempt(
                    self.name(),
                    program,
                    args,
                    finding.size_bytes,
                );
                crate::journal::append(ctx, &attempt)?;
                let result = (|| -> Result<String> {
                    let output = Command::new(program).args(args).output()?;
                    if !output.status.success() {
                        anyhow::bail!("`{program} {}` failed", args.join(" "));
                    }
                    Ok(format!("`{program} {}` completed", args.join(" ")))
                })();
                crate::journal::finish(ctx, &attempt, result)
            })();
            match result {
                Ok(note) => outcome.record(finding, note),
                Err(error) => {
                    outcome.fail(error);
                    break;
                }
            }
        }
        if outcome.summary.items_touched > 0 {
            outcome
                .summary
                .notes
                .push("OrbStack compacts its disk lazily; restart it to trigger TRIM".into());
        }
        Ok(outcome)
    }
}

pub(crate) fn parse_size(value: &str) -> Result<u64> {
    let value = value.split('(').next().unwrap_or("").trim();
    let split = value
        .find(|character: char| character.is_alphabetic())
        .unwrap_or(value.len());
    let (number, unit) = value.split_at(split);
    let number: f64 = number.trim().parse().context("invalid numeric size")?;
    let multiplier = match unit.trim().to_lowercase().as_str() {
        "b" => 1.0,
        "kb" => 1e3,
        "mb" => 1e6,
        "gb" => 1e9,
        "tb" => 1e12,
        "pb" => 1e15,
        "eb" => 1e18,
        unit => anyhow::bail!("unsupported size unit `{unit}`"),
    };
    let bytes = (number * multiplier).round();
    // `i64::MAX as f64` rounds up to 2^63, so equality is already ambiguous.
    if !bytes.is_finite() || bytes < 0.0 || bytes >= i64::MAX as f64 {
        anyhow::bail!("size is outside Docker's signed 64-bit byte range");
    }
    Ok(bytes as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::report::Action;
    use std::path::PathBuf;

    fn test_ctx() -> Ctx {
        Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: PathBuf::from("/tmp/devtrim-docker-test-journal.jsonl"),
            home: PathBuf::from("/tmp"),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Stderr,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        }
    }

    #[test]
    fn parses_docker_sizes() {
        assert_eq!(parse_size("8.376GB (59%)").unwrap(), 8_376_000_000);
        assert_eq!(parse_size("729.1kB").unwrap(), 729_100);
        assert_eq!(parse_size("5.051GB").unwrap(), 5_051_000_000);
        assert_eq!(parse_size("2.22PB").unwrap(), 2_220_000_000_000_000);
        assert_eq!(parse_size("9EB").unwrap(), 9_000_000_000_000_000_000);
        assert_eq!(parse_size("0B").unwrap(), 0);
    }

    #[test]
    fn rejects_ambiguous_or_out_of_range_docker_sizes() {
        for value in [
            "5",
            "5XB",
            "-1GB",
            "NaNGB",
            "10EB",
            "9223372036854775807B",
            "9223372036854775808B",
        ] {
            assert!(parse_size(value).is_err(), "accepted {value}");
        }
    }
    #[test]
    fn rejects_forged_volume_prune_action() {
        let finding = Finding::new(
            "forged",
            None,
            0,
            "test",
            1,
            Action::command("docker", &["volume", "prune", "-f"]),
        );
        let ctx = test_ctx();
        let outcome = Docker.apply(&[finding], &ctx).unwrap();
        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
    }

    #[test]
    fn rejects_forged_valid_looking_command_without_authority() {
        let finding = Finding::new(
            "forged",
            None,
            0,
            "test",
            1,
            Action::command("docker", &["image", "prune", "-a", "-f"]),
        );
        let ctx = test_ctx();
        let outcome = Docker.apply(&[finding], &ctx).unwrap();
        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
    }
}
