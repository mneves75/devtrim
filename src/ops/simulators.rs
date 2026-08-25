//! Simulator hygiene via `xcrun simctl delete unavailable`.
//! Confirmation flags never add an unpreviewed erase-all operation.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::process::Command;

use super::{ApplyOutcome, Finding, Op, dir_size};
use crate::report::CommandAuthority;
use crate::safety::Ctx;

pub struct Simulators;

#[derive(serde::Deserialize)]
struct DeviceList {
    devices: BTreeMap<String, Vec<Device>>,
}

#[derive(serde::Deserialize)]
struct Device {
    #[serde(rename = "isAvailable")]
    is_available: bool,
}

fn simctl(args: &[&str]) -> Result<String> {
    let output = Command::new("xcrun").arg("simctl").args(args).output()?;
    if !output.status.success() {
        anyhow::bail!("`xcrun simctl {}` failed", args.join(" "));
    }
    String::from_utf8(output.stdout).context("simctl returned non-UTF-8 output")
}

impl Op for Simulators {
    fn name(&self) -> &'static str {
        "simulators"
    }

    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>> {
        let available = match Command::new("xcrun").arg("--version").output() {
            Ok(output) => output.status.success(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.into()),
        };
        if !available {
            return Ok(Vec::new());
        }
        // A Command Line Tools-only host has `xcrun` but no working `simctl`. Unknown
        // device state is not deletion authority, so report nothing instead of failing.
        let output = match simctl(&["list", "devices", "--json"]) {
            Ok(output) => output,
            Err(error) => {
                if !ctx.json {
                    ctx.diagnostic("warn", format!("simulators: {error:#}"));
                }
                return Ok(Vec::new());
            }
        };
        let devices: DeviceList =
            serde_json::from_str(&output).context("simctl returned invalid device JSON")?;
        let unavailable = devices
            .devices
            .values()
            .flatten()
            .filter(|device| !device.is_available)
            .count();
        if unavailable == 0 {
            return Ok(Vec::new());
        }
        let storage = dir_size(&ctx.home.join("Library/Developer/CoreSimulator/Devices"))?;
        Ok(vec![Finding::command(
            "unavailable Apple simulator devices",
            0,
            format!(
                "{unavailable} device(s) reference missing runtimes; total simulator storage is ~{}",
                crate::report::gb(storage)
            ),
            4,
            CommandAuthority::DeleteUnavailableSimulators,
        )])
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
                let output = Command::new(program).args(args).output()?;
                if !output.status.success() {
                    anyhow::bail!("`xcrun simctl delete unavailable` failed");
                }
                Ok("deleted unavailable simulator devices".into())
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
    use std::path::PathBuf;

    fn test_ctx() -> Ctx {
        Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            home: PathBuf::from("/tmp"),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Stderr,
            diagnostics: Default::default(),
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
