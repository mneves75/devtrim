//! Docker/OrbStack pruning: unused images + build cache. Volumes are never pruned.

use anyhow::{Context, Result};
use std::io;
use std::path::{Component, Path};
use std::process::{Command, Output};

use super::{Action, ApplyOutcome, Finding, Op};
use crate::report::CommandAuthority;
use crate::safety::{Ctx, escalate};

pub struct Docker;

/// Host-side virtual disk images for the supported local Docker runtimes,
/// relative to the user's home directory.
///
/// `docker system df` reports space *inside* the guest filesystem. The host
/// pays for these files instead, and pruning inside the VM never shrinks them:
/// the runtime compacts its own image on its own schedule, in practice after
/// the VM stops. Reporting only the guest number understates the host cost and
/// hides the fact that reclaiming it needs a separate step, so devtrim
/// discloses the image itself.
const VM_DISK_IMAGES: &[(&str, &str)] = &[
    (
        "OrbStack",
        "Library/Group Containers/HUAQ24HBR6.dev.orbstack/data/data.img.raw",
    ),
    (
        "Docker Desktop",
        "Library/Containers/com.docker.docker/Data/vms/0/data/Docker.raw",
    ),
    (
        "Docker Desktop",
        "Library/Containers/com.docker.docker/Data/vms/0/Docker.raw",
    ),
];

#[derive(serde::Deserialize)]
struct DockerContext {
    #[serde(rename = "Endpoints")]
    endpoints: DockerEndpoints,
}

#[derive(serde::Deserialize)]
struct DockerEndpoints {
    docker: DockerEndpoint,
}

#[derive(serde::Deserialize)]
struct DockerEndpoint {
    #[serde(rename = "Host")]
    host: String,
}

fn docker(host: &str, args: &[&str]) -> Result<String> {
    let command = format!("`docker --host {host} {}`", args.join(" "));
    command_stdout(
        Command::new("docker")
            .arg("--host")
            .arg(host)
            .args(args)
            .output(),
        &command,
    )
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

fn docker_host() -> Result<Option<String>> {
    let Some(output) = optional_command_stdout(
        Command::new("docker").args(["context", "inspect"]).output(),
        "`docker context inspect`",
    )?
    else {
        return Ok(None);
    };
    parse_docker_host(&output).map(Some)
}

fn parse_docker_host(output: &str) -> Result<String> {
    let contexts: Vec<DockerContext> =
        serde_json::from_str(output).context("docker context inspect returned invalid JSON")?;
    let [context] = contexts.as_slice() else {
        anyhow::bail!("docker context inspect must return exactly one active context");
    };
    let host = context.endpoints.docker.host.trim();
    if !is_local_docker_host(host) {
        anyhow::bail!("refusing non-local Docker endpoint `{host}`");
    }
    Ok(host.to_string())
}

fn is_local_docker_host(host: &str) -> bool {
    let Some(socket) = host.strip_prefix("unix://") else {
        return false;
    };
    let path = Path::new(socket);
    path.is_absolute()
        && path.file_name().is_some()
        && path
            .components()
            .all(|component| !matches!(component, Component::CurDir | Component::ParentDir))
}

/// Blocks actually allocated on the host, not the logical length.
///
/// A VM disk image is sparse, so the two differ by orders of magnitude: a real
/// OrbStack image measured 926 GB logical against 35 GB allocated. The crate's
/// `dir_size` convention is documented as logical bytes, which is right for
/// ordinary directories and wrong by ~26x here, so this measurement is separate
/// and deliberate. Returns `None` when the path is absent or is not a regular
/// file; a stat failure for a path that does exist fails closed.
fn allocated_bytes(path: &Path) -> Result<Option<u64>> {
    use std::os::unix::fs::MetadataExt;

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", path.display()));
        }
    };
    if !metadata.file_type().is_file() {
        return Ok(None);
    }
    Ok(Some(metadata.blocks().saturating_mul(512)))
}

/// Report-only disclosure of the host cost of each present runtime disk image.
///
/// These findings are never actionable: compacting or deleting a live VM image
/// is the runtime's job, and doing it from here would destroy every container
/// and volume on the machine.
fn vm_disk_findings(home: &Path) -> Result<Vec<Finding>> {
    let mut findings = Vec::new();
    for (runtime, relative) in VM_DISK_IMAGES {
        let path = home.join(relative);
        let Some(allocated) = allocated_bytes(&path)? else {
            continue;
        };
        if allocated == 0 {
            continue;
        }
        let logical = std::fs::symlink_metadata(&path)
            .with_context(|| format!("cannot inspect {}", path.display()))?
            .len();
        findings.push(Finding::new(
            format!("{runtime} VM disk image"),
            Some(path),
            allocated,
            format!(
                "EXCLUDED: host-side virtual disk for the local Docker runtime, listed for visibility only. \
                 Shown size is allocated blocks; the file is sparse and reports {logical} logical bytes. \
                 Pruning images or build cache frees space inside the VM but does not shrink this file: \
                 {runtime} compacts it on its own schedule, in practice after the VM stops"
            ),
            0,
            Action::None,
        ));
    }
    Ok(findings)
}

fn parse_system_df(output: &str, host: &str) -> Result<Vec<Finding>> {
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
                CommandAuthority::DockerImagePrune {
                    host: host.to_string(),
                },
            ),
            "Build Cache" => (
                "Docker Build Cache reclaimable",
                CommandAuthority::DockerBuilderPrune {
                    host: host.to_string(),
                },
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
                "{total} total on local endpoint {host}; removes every image not referenced by a container, including untagged local builds; volumes are never touched"
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

    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>> {
        // The host image is disclosed even when the daemon is unreachable. A
        // stopped VM is precisely when its disk image is invisible to `docker`
        // and still occupying the host, so making this disclosure depend on a
        // live daemon would hide the cost in the one state that matters most.
        let mut findings = vm_disk_findings(&ctx.home)?;
        // A refused endpoint or a malformed response stays a hard error: those
        // are the fail-closed boundaries that must never degrade to a warning.
        // Only a daemon that is simply not running is treated as normal, and
        // that is exactly the state in which the host image matters most.
        let Some(host) = docker_host()? else {
            return Ok(findings);
        };
        match docker(&host, &["version"]) {
            Ok(version) if version.trim().is_empty() => {
                anyhow::bail!("`docker version` returned empty output")
            }
            Ok(_) => {
                let output = docker(
                    &host,
                    &[
                        "system",
                        "df",
                        "--format",
                        "{{.Type}}\t{{.Size}}\t{{.Reclaimable}}",
                    ],
                )?;
                findings.extend(parse_system_df(&output, &host)?);
            }
            Err(error) => ctx.diagnostic(
                "warn",
                format!("Docker daemon unreachable, no image or build-cache actions: {error:#}"),
            ),
        }
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let mut outcome = ApplyOutcome::new(self.name());
        for finding in findings {
            // The host VM disk image is a report-only disclosure, not an action.
            // The skip is on ACTIONABILITY, never on whether an authority is
            // present: an actionable finding that carries no authority is a
            // forgery and must still be refused below.
            if !finding.action.is_actionable() {
                continue;
            }
            let result = (|| -> Result<String> {
                let Some(authority) = finding.command_authority() else {
                    anyhow::bail!("refusing unexpected Docker action");
                };
                if !matches!(
                    authority,
                    CommandAuthority::DockerImagePrune { .. }
                        | CommandAuthority::DockerBuilderPrune { .. }
                ) {
                    anyhow::bail!("refusing unexpected Docker action");
                }
                let host = authority
                    .docker_host()
                    .ok_or_else(|| anyhow::anyhow!("Docker action missing endpoint authority"))?;
                if !is_local_docker_host(host) {
                    anyhow::bail!("refusing non-local Docker endpoint `{host}`");
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
                        &args,
                        finding.size_bytes,
                    ),
                )?;
                let result = (|| -> Result<String> {
                    let output = Command::new(program).args(&args).output()?;
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

    /// Positive control for the measurement basis itself: a purely logical
    /// measurement passes every other assertion here, so without this the fix
    /// could regress to `len()` silently.
    #[test]
    fn allocated_bytes_measures_allocation_not_logical_length() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-docker-sparse")
            .tempdir()
            .unwrap();
        let path = root.path().join("data.img.raw");
        let file = std::fs::File::create(&path).unwrap();
        // A hole, not a write: logical length grows to 1 GiB while almost no
        // blocks are allocated. This is the exact shape of a VM disk image.
        file.set_len(1024 * 1024 * 1024).unwrap();
        file.sync_all().unwrap();

        let logical = std::fs::symlink_metadata(&path).unwrap().len();
        let allocated = allocated_bytes(&path).unwrap().unwrap();

        assert_eq!(logical, 1024 * 1024 * 1024);
        assert!(
            allocated < logical / 100,
            "allocated {allocated} should be far below logical {logical}"
        );
    }

    #[test]
    fn allocated_bytes_ignores_missing_paths_and_directories() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-docker-absent")
            .tempdir()
            .unwrap();
        assert_eq!(
            allocated_bytes(&root.path().join("missing.raw")).unwrap(),
            None
        );
        assert_eq!(allocated_bytes(root.path()).unwrap(), None);
    }

    #[test]
    fn vm_disk_image_is_disclosed_but_never_actionable() {
        use std::io::Write;

        let home = tempfile::Builder::new()
            .prefix("devtrim-docker-home")
            .tempdir()
            .unwrap();
        let image = home.path().join(VM_DISK_IMAGES[0].1);
        std::fs::create_dir_all(image.parent().unwrap()).unwrap();
        let mut file = std::fs::File::create(&image).unwrap();
        file.write_all(b"x").unwrap();
        file.set_len(64 * 1024 * 1024).unwrap();
        file.sync_all().unwrap();

        let findings = vm_disk_findings(home.path()).unwrap();
        assert_eq!(findings.len(), 1);
        let finding = &findings[0];
        assert_eq!(finding.action, Action::None);
        assert!(!finding.action.is_actionable());
        assert_eq!(finding.danger, 0);
        assert!(
            finding.note.contains("does not shrink this file"),
            "note must disclose that pruning does not reclaim the host image: {}",
            finding.note
        );
    }

    /// Regression: the disclosure is pushed FIRST by `scan`, and an apply loop
    /// that refused it would abort before any prune ran — turning
    /// `clean docker --apply` into a no-op that reports failure on every
    /// machine that actually has a VM image.
    #[test]
    fn apply_skips_the_report_only_vm_disk_finding_instead_of_refusing_it() {
        use std::io::Write;

        let home = tempfile::Builder::new()
            .prefix("devtrim-docker-apply")
            .tempdir()
            .unwrap();
        let image = home.path().join(VM_DISK_IMAGES[0].1);
        std::fs::create_dir_all(image.parent().unwrap()).unwrap();
        let mut file = std::fs::File::create(&image).unwrap();
        file.write_all(b"x").unwrap();
        file.sync_all().unwrap();

        let findings = vm_disk_findings(home.path()).unwrap();
        assert_eq!(findings.len(), 1);

        let mut ctx = test_ctx();
        ctx.home = home.path().to_path_buf();
        let outcome = Docker.apply(&findings, &ctx).unwrap();
        assert!(
            outcome.errors.is_empty(),
            "a report-only finding must not fail the apply: {:?}",
            outcome.errors
        );
        assert_eq!(outcome.summary.items_touched, 0);
    }

    /// The skip above must not become a hole: an actionable finding that
    /// carries no command authority is a forgery and stays refused.
    #[test]
    fn apply_still_refuses_an_actionable_finding_without_authority() {
        let forged = Finding::new("forged docker action", None, 1, "forged", 6, Action::Trash);
        let outcome = Docker.apply(&[forged], &test_ctx()).unwrap();
        assert!(
            !outcome.errors.is_empty(),
            "an actionable finding with no authority must be refused"
        );
    }

    #[test]
    fn absent_vm_disk_image_yields_no_finding() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-docker-empty-home")
            .tempdir()
            .unwrap();
        assert!(vm_disk_findings(home.path()).unwrap().is_empty());
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
        let host = "unix:///var/run/docker.sock";
        assert!(parse_system_df("", host).is_err());
        assert!(parse_system_df("Images\t1GB", host).is_err());
        assert!(parse_system_df("Images\t1GB\t500MB\textra", host).is_err());
    }

    #[test]
    fn docker_context_must_resolve_to_an_absolute_local_socket() {
        let local = r#"[{"Endpoints":{"docker":{"Host":"unix:///Users/example/.docker/run/docker.sock"}}}]"#;
        assert_eq!(
            parse_docker_host(local).unwrap(),
            "unix:///Users/example/.docker/run/docker.sock"
        );

        for host in [
            "ssh://prod.example",
            "tcp://127.0.0.1:2375",
            "unix://relative.sock",
            "unix:///tmp/../remote.sock",
        ] {
            let document = format!(r#"[{{"Endpoints":{{"docker":{{"Host":"{host}"}}}}}}]"#);
            assert!(parse_docker_host(&document).is_err(), "accepted {host}");
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
