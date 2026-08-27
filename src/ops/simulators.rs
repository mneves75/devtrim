//! Simulator hygiene via `xcrun simctl delete unavailable`.
//! Confirmation flags never add an unpreviewed erase-all operation.

use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Component, Path};
use std::process::{Command, Output};

use super::{ApplyOutcome, Finding, Op, dir_size};
use crate::report::CommandAuthority;
use crate::safety::{Ctx, escalate};

pub struct Simulators;

#[derive(serde::Deserialize)]
struct DeviceList {
    devices: BTreeMap<String, Vec<Device>>,
}

#[derive(serde::Deserialize)]
struct Device {
    #[serde(rename = "isAvailable")]
    is_available: bool,
    udid: String,
}

fn simctl(args: &[&str]) -> Result<String> {
    let command = format!("`xcrun simctl {}`", args.join(" "));
    command_stdout(
        Command::new("xcrun").arg("simctl").args(args).output(),
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

fn simulator_device_path(root: &Path, udid: &str) -> Result<std::path::PathBuf> {
    if udid.is_empty()
        || !udid
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        anyhow::bail!("invalid simulator device identifier `{udid}`");
    }
    let mut components = Path::new(udid).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(component)), None) => Ok(root.join(component)),
        _ => anyhow::bail!("invalid simulator device identifier `{udid}`"),
    }
}

fn findings_from_simctl(output: &str, ctx: &Ctx) -> Result<Vec<Finding>> {
    let devices: DeviceList =
        serde_json::from_str(output).context("simctl returned invalid device JSON")?;
    let device_root = ctx.home.join("Library/Developer/CoreSimulator/Devices");
    let mut unavailable_paths = BTreeSet::new();
    for device in devices
        .devices
        .values()
        .flatten()
        .filter(|device| !device.is_available)
    {
        let path = simulator_device_path(&device_root, &device.udid)?;
        if !unavailable_paths.insert(path) {
            anyhow::bail!(
                "simctl returned duplicate device identifier `{}`",
                device.udid
            );
        }
    }
    if unavailable_paths.is_empty() {
        return Ok(Vec::new());
    }
    let storage = unavailable_paths.iter().try_fold(0u64, |total, path| {
        let size = dir_size(path)
            .with_context(|| format!("cannot measure simulator device {}", path.display()))?;
        total
            .checked_add(size)
            .ok_or_else(|| anyhow::anyhow!("simulator device size overflow"))
    })?;
    let unavailable = unavailable_paths.len();
    Ok(vec![Finding::command(
        "unavailable Apple simulator devices",
        storage,
        format!(
            "{unavailable} device(s) reference missing runtimes; their device data uses ~{}",
            crate::report::gb(storage)
        ),
        escalate(4, storage),
        CommandAuthority::DeleteUnavailableSimulators,
    )])
}

impl Op for Simulators {
    fn name(&self) -> &'static str {
        "simulators"
    }

    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>> {
        let Some(version) = optional_command_stdout(
            Command::new("xcrun").arg("--version").output(),
            "`xcrun --version`",
        )?
        else {
            return Ok(Vec::new());
        };
        if version.trim().is_empty() {
            anyhow::bail!("`xcrun --version` returned empty output");
        }
        let output = simctl(&["list", "devices", "--json"])?;
        findings_from_simctl(&output, ctx)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let before = dir_size(&ctx.home.join("Library/Developer/CoreSimulator/Devices"))?;
        let mut outcome = ApplyOutcome::new(self.name());
        for finding in findings {
            let result = (|| -> Result<String> {
                let Some(authority) = finding.command_authority() else {
                    anyhow::bail!("refusing unexpected simulator action");
                };
                if authority != CommandAuthority::DeleteUnavailableSimulators {
                    anyhow::bail!("refusing unexpected simulator action");
                }
                if finding.action != authority.action() {
                    anyhow::bail!("refusing altered simulator action");
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
                        anyhow::bail!("`xcrun simctl delete unavailable` failed");
                    }
                    Ok("deleted unavailable simulator devices".into())
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
        match dir_size(&ctx.home.join("Library/Developer/CoreSimulator/Devices")) {
            Ok(after) => outcome.summary.bytes_freed_estimate = before.saturating_sub(after),
            Err(error) => {
                outcome.fail(error.context("cannot measure simulator storage after apply"))
            }
        }
        Ok(outcome)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::Action;
    use std::fs::File;
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
            journal_path: PathBuf::from("/tmp/devtrim-simulators-test-journal.jsonl"),
            home: PathBuf::from("/tmp"),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Stderr,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        }
    }

    #[test]
    fn rejects_forged_erase_all_action() {
        let finding = Finding::new(
            "forged",
            None,
            0,
            "test",
            1,
            Action::command("xcrun", &["simctl", "erase", "all"]),
        );
        let ctx = test_ctx();
        let outcome = Simulators.apply(&[finding], &ctx).unwrap();
        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
    }

    #[test]
    fn optional_probe_only_treats_not_found_as_absent() {
        let missing = optional_command_stdout(
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "missing")),
            "`xcrun --version`",
        )
        .unwrap();
        assert_eq!(missing, None);

        let nonzero = optional_command_stdout(
            Ok(Output {
                status: ExitStatus::from_raw(1 << 8),
                stdout: Vec::new(),
                stderr: b"toolchain unavailable".to_vec(),
            }),
            "`xcrun --version`",
        )
        .unwrap_err();
        assert!(nonzero.to_string().contains("toolchain unavailable"));

        let invalid = optional_command_stdout(
            Ok(Output {
                status: ExitStatus::from_raw(0),
                stdout: vec![0xff],
                stderr: Vec::new(),
            }),
            "`xcrun --version`",
        )
        .unwrap_err();
        assert!(invalid.to_string().contains("non-UTF-8"));
    }

    #[test]
    fn unavailable_device_finding_uses_measured_size_and_escalated_danger() {
        let root =
            std::env::temp_dir().join(format!("devtrim-simulators-size-{}", std::process::id()));
        crate::ops::remove_test_path(&root);
        let home = root.join("home");
        let data = home.join("Library/Developer/CoreSimulator/Devices/DEVICE-1/data");
        std::fs::create_dir_all(&data).unwrap();
        let payload = File::create(data.join("payload")).unwrap();
        let measured = 2 * 1024 * 1024 * 1024;
        payload.set_len(measured).unwrap();
        let mut ctx = test_ctx();
        ctx.home = home;
        let output = r#"{"devices":{"runtime":[{"isAvailable":false,"udid":"DEVICE-1"}]}}"#;

        let findings = findings_from_simctl(output, &ctx).unwrap();

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].size_bytes, measured);
        assert_eq!(findings[0].danger, crate::safety::escalate(4, measured));
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn malformed_simulator_device_id_is_an_error() {
        let ctx = test_ctx();
        let output = r#"{"devices":{"runtime":[{"isAvailable":false,"udid":"../../escape"}]}}"#;

        let error = findings_from_simctl(output, &ctx).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("invalid simulator device identifier")
        );
    }

    #[test]
    fn rejects_forged_valid_looking_command_without_authority() {
        let finding = Finding::new(
            "forged",
            None,
            0,
            "test",
            1,
            Action::command("xcrun", &["simctl", "delete", "unavailable"]),
        );
        let ctx = test_ctx();
        let outcome = Simulators.apply(&[finding], &ctx).unwrap();
        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
    }
}
