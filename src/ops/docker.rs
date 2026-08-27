//! Docker/OrbStack pruning: unused images + build cache. Volumes are never pruned.

use anyhow::{Context, Result};
use std::io;
use std::process::{Command, Output};

use super::{ApplyOutcome, Finding, Op};
use crate::report::CommandAuthority;
use crate::safety::{Ctx, escalate};

pub struct Docker;

fn docker(args: &[&str]) -> Result<String> {
    let command = format!("`docker {}`", args.join(" "));
    command_stdout(Command::new("docker").args(args).output(), &command)
}

fn command_stdout(output: io::Result<Output>, command: &str) -> Result<String> {
    let output = output.with_context(|| format!("cannot run {command}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        if detail.is_empty() {
            anyhow::bail!("{command} failed with {}", output.status);
        }
        anyhow::bail!("{command} failed with {}: {detail}", output.status);
    }
    String::from_utf8(output.stdout).with_context(|| format!("{command} returned non-UTF-8 output"))
}

fn optional_command_stdout(output: io::Result<Output>, command: &str) -> Result<Option<String>> {
    match output {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        output => command_stdout(output, command).map(Some),
    }
}

fn parse_system_df(output: &str) -> Result<Vec<Finding>> {
    if output.trim().is_empty() {
        anyhow::bail!("docker system df returned empty output");
    }
    let mut findings = Vec::new();
    for (index, line) in output.lines().enumerate() {
        let mut columns = line.split('\t');
        let (Some(kind), Some(total), Some(reclaimable), None) = (
            columns.next(),
            columns.next(),
            columns.next(),
            columns.next(),
        ) else {
            anyhow::bail!("invalid Docker system df row {}", index.saturating_add(1));
        };
        if kind.is_empty() || total.is_empty() || reclaimable.is_empty() {
            anyhow::bail!("invalid Docker system df row {}", index.saturating_add(1));
        }
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

impl Op for Docker {
    fn name(&self) -> &'static str {
        "docker"
    }

    fn scan(&self, _ctx: &Ctx) -> Result<Vec<Finding>> {
        let Some(version) = optional_command_stdout(
            Command::new("docker").arg("version").output(),
            "`docker version`",
        )?
        else {
            return Ok(Vec::new());
        };
        if version.trim().is_empty() {
            anyhow::bail!("`docker version` returned empty output");
        }
        let output = docker(&[
            "system",
            "df",
            "--format",
            "{{.Type}}\t{{.Size}}\t{{.Reclaimable}}",
        ])?;
        parse_system_df(&output)
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
                let attempt = crate::journal::begin(
                    ctx,
                    crate::journal::JournalRecord::command_attempt(
                        self.name(),
                        program,
                        args,
                        finding.size_bytes,
                    ),
                )?;
                let result = (|| -> Result<String> {
                    let output = Command::new(program).args(args).output()?;
                    if !output.status.success() {
                        anyhow::bail!("`{program} {}` failed", args.join(" "));
                    }
                    Ok(format!("`{program} {}` completed", args.join(" ")))
                })();
                attempt.finish(ctx, result)
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
    use std::os::unix::process::ExitStatusExt;
    use std::path::PathBuf;
    use std::process::{ExitStatus, Output};

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
    fn optional_probe_only_treats_not_found_as_absent() {
        let missing = optional_command_stdout(
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing")),
            "`docker version`",
        )
        .unwrap();
        assert_eq!(missing, None);

        let nonzero = optional_command_stdout(
            Ok(Output {
                status: ExitStatus::from_raw(1 << 8),
                stdout: Vec::new(),
                stderr: b"daemon unavailable".to_vec(),
            }),
            "`docker version`",
        )
        .unwrap_err();
        assert!(nonzero.to_string().contains("daemon unavailable"));

        let invalid = optional_command_stdout(
            Ok(Output {
                status: ExitStatus::from_raw(0),
                stdout: vec![0xff],
                stderr: Vec::new(),
            }),
            "`docker version`",
        )
        .unwrap_err();
        assert!(invalid.to_string().contains("non-UTF-8"));
    }

    #[test]
    fn rejects_malformed_docker_system_df_output() {
        assert!(parse_system_df("").is_err());
        assert!(parse_system_df("Images\t1GB").is_err());
        assert!(parse_system_df("Images\t1GB\t500MB\textra").is_err());
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
