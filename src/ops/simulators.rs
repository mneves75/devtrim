//! Simulator hygiene via `xcrun simctl delete unavailable`.
//! Confirmation flags never add an unpreviewed erase-all operation.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::process::Command;

use super::{Action, Finding, Op, Summary, dir_size};
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
                    eprintln!("warn simulators: {error:#}");
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
        let storage = dir_size(&ctx.home.join("Library/Developer/CoreSimulator/Devices"));
        Ok(vec![Finding {
            label: "unavailable Apple simulator devices".into(),
            path: None,
            size_bytes: 0,
            note: format!(
                "{unavailable} device(s) reference missing runtimes; total simulator storage is ~{}",
                crate::report::gb(storage)
            ),
            danger: 4,
            action: Action::command("xcrun", &["simctl", "delete", "unavailable"]),
        }])
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<Summary> {
        let before = dir_size(&ctx.home.join("Library/Developer/CoreSimulator/Devices"));
        let mut touched = 0usize;
        let mut notes = Vec::new();
        for finding in findings {
            let Action::Command { program, args } = &finding.action else {
                continue;
            };
            if program != "xcrun" || args != &["simctl", "delete", "unavailable"] {
                anyhow::bail!("refusing unexpected simulator action");
            }
            let output = Command::new(program).args(args).output()?;
            if !output.status.success() {
                anyhow::bail!("`xcrun simctl delete unavailable` failed");
            }
            touched += 1;
            notes.push("deleted unavailable simulator devices".into());
        }
        let after = dir_size(&ctx.home.join("Library/Developer/CoreSimulator/Devices"));
        Ok(Summary {
            op: self.name().into(),
            items_touched: touched,
            bytes_freed_estimate: before.saturating_sub(after),
            notes,
        })
    }
}
