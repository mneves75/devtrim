//! Simulator hygiene via exact per-device `xcrun simctl delete <UDID>` actions.
//! Confirmation flags never add an unpreviewed erase-all operation.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Component, Path};
use std::process::Command;

use super::{ApplyOutcome, Finding, Op, command_stdout, dir_size, optional_command_stdout};
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

fn simulator_device_path(root: &Path, udid: &str) -> Result<std::path::PathBuf> {
    let mut bytes = udid.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        anyhow::bail!("invalid simulator device identifier `{udid}`");
    }
    let mut components = Path::new(udid).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(component)), None) => Ok(root.join(component)),
        _ => anyhow::bail!("invalid simulator device identifier `{udid}`"),
    }
}

fn simulator_states(output: &str) -> Result<BTreeMap<String, bool>> {
    let devices: DeviceList =
        serde_json::from_str(output).context("simctl returned invalid device JSON")?;
    let mut states = BTreeMap::new();
    for device in devices.devices.values().flatten() {
        simulator_device_path(Path::new("/"), &device.udid)?;
        if states
            .insert(device.udid.clone(), device.is_available)
            .is_some()
        {
            anyhow::bail!(
                "simctl returned duplicate device identifier `{}`",
                device.udid
            );
        }
    }
    Ok(states)
}

fn findings_from_simctl(output: &str, ctx: &Ctx) -> Result<Vec<Finding>> {
    let device_root = ctx.home.join("Library/Developer/CoreSimulator/Devices");
    simulator_states(output)?
        .into_iter()
        .filter(|(_, is_available)| !is_available)
        .map(|(udid, _)| {
            let path = simulator_device_path(&device_root, &udid)?;
            let size = dir_size(&path)
                .with_context(|| format!("cannot measure simulator device {}", path.display()))?;
            Ok(Finding::command(
                format!("unavailable Apple simulator device {udid}"),
                size,
                format!(
                    "references a missing runtime; its device data uses ~{}",
                    crate::report::gb(size)
                ),
                escalate(4, size),
                CommandAuthority::DeleteSimulator { udid },
            ))
        })
        .collect()
}

impl Op for Simulators {
    fn name(&self) -> &'static str {
        "simulators"
    }

    fn scan(
        &self,
        ctx: &Ctx,
        _observations: &super::project::ScanObservations,
    ) -> Result<Vec<Finding>> {
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
                let Some(udid) = authority.simulator_udid() else {
                    anyhow::bail!("refusing unexpected simulator action");
                };
                if finding.action != authority.action() {
                    anyhow::bail!("refusing altered simulator action");
                }
                let current = simulator_states(&simctl(&["list", "devices", "--json"])?)?;
                match current.get(udid) {
                    Some(false) => {}
                    Some(true) => anyhow::bail!(
                        "simulator device became available after preview; refusing `{udid}`"
                    ),
                    None => {
                        anyhow::bail!("simulator device vanished after preview; refusing `{udid}`")
                    }
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
                        anyhow::bail!("`xcrun simctl delete {udid}` failed");
                    }
                    Ok(format!("deleted unavailable simulator device {udid}"))
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
    fn rejects_forged_actions_without_authority() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-simulators-forged")
            .tempdir()
            .unwrap();
        let sentinel = home.path().join("sentinel");
        std::fs::write(&sentinel, "keep").unwrap();
        let mut ctx = test_ctx();
        ctx.home = home.path().to_path_buf();
        ctx.journal_path = home.path().join("journal.jsonl");
        let pathless_command = Action::command("xcrun", &["simctl", "delete", "unavailable"]);
        for finding in [
            // Simulator erase all is never authorized: keep the forged payload explicit.
            Finding::new(
                "forged simulator action",
                Some(sentinel.clone()),
                1,
                "forged",
                6,
                Action::command("xcrun", &["simctl", "erase", "all"]),
            ),
            Finding::new(
                "forged simulator action",
                Some(sentinel.clone()),
                1,
                "forged",
                6,
                Action::command("xcrun", &["simctl", "delete", "unavailable"]),
            ),
            Finding::new("pathless forgery", None, 1, "forged", 6, pathless_command),
        ] {
            let outcome = Simulators.apply(&[finding], &ctx).unwrap();
            assert_eq!(outcome.summary.items_touched, 0);
            assert_eq!(outcome.errors.len(), 1);
            assert!(
                outcome.errors[0].contains("refusing unexpected simulator action"),
                "{:?}",
                outcome.errors
            );
            assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "keep");
        }
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
        assert_eq!(findings[0].danger, 5);
        assert_eq!(
            findings[0].action,
            Action::command("xcrun", &["simctl", "delete", "DEVICE-1"])
        );
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn malformed_simulator_device_id_is_an_error() {
        let ctx = test_ctx();
        for udid in ["../../escape", "--help"] {
            let output =
                format!(r#"{{"devices":{{"runtime":[{{"isAvailable":false,"udid":"{udid}"}}]}}}}"#);
            let error = findings_from_simctl(&output, &ctx).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("invalid simulator device identifier")
            );
        }
    }

    #[test]
    fn preview_never_authorizes_a_broad_simulator_delete() {
        let ctx = test_ctx();
        let output = r#"{"devices":{"runtime":[{"isAvailable":false,"udid":"DEVICE-A"},{"isAvailable":false,"udid":"DEVICE-B"}]}}"#;

        let findings = findings_from_simctl(output, &ctx).unwrap();

        assert_eq!(findings.len(), 2);
        assert_eq!(
            findings[0].action,
            Action::command("xcrun", &["simctl", "delete", "DEVICE-A"])
        );
        assert_eq!(
            findings[1].action,
            Action::command("xcrun", &["simctl", "delete", "DEVICE-B"])
        );
    }
}
