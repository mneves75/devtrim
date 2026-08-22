//! Simulator hygiene via `xcrun simctl`: delete devices whose runtime is
//! gone (safe), optionally erase all remaining device contents (dangerous).

use anyhow::Result;
use std::process::Command;

use super::{Finding, Op, Summary, dir_size};
use crate::safety::Ctx;

pub struct Simulators;

fn simctl(args: &[&str]) -> Result<String> {
    let o = Command::new("xcrun")
        .arg("simctl")
        .args(args)
        .output()?;
    Ok(String::from_utf8_lossy(&o.stdout).to_string())
}

impl Op for Simulators {
    fn name(&self) -> &'static str {
        "simulators"
    }
    fn scan(&self, ctx: &Ctx) -> Result<Vec<Finding>> {
        if !Command::new("xcrun").arg("--version").output().map(|o| o.status.success()).unwrap_or(false) {
            return Ok(Vec::new());
        }
        let devices_dir = ctx.home.join("Library/Developer/CoreSimulator/Devices");
        let size = dir_size(&devices_dir);
        // Count unavailable devices (runtime deleted but device remains).
        let list = simctl(&["list", "devices"]).unwrap_or_default();
        let unavailable = list.lines().filter(|l| l.contains("(Unavailable)")).count();
        let total = list.lines().filter(|l| l.contains('(') && l.contains(')')).count();

        if size == 0 && unavailable == 0 {
            return Ok(Vec::new());
        }
        let mut notes = Vec::new();
        let danger;
        if unavailable > 0 {
            notes.push(format!("{unavailable} device(s) reference missing runtimes → deletable"));
        }
        if total > 12 {
            notes.push(format!("{total} devices installed — consider erasing contents"));
        }
        danger = if unavailable > 0 { 4 } else { 6 };
        Ok(vec![Finding {
            label: "iOS/watchOS simulator storage".into(),
            path: Some(devices_dir.display().to_string()),
            size_bytes: size,
            note: {
                let mut n = notes.join("; ");
                if n.is_empty() {
                    n = "delete-unavailable is safe; erase-all wipes app data".into();
                }
                n
            },
            danger,
            action: "command:xcrun simctl delete unavailable".into(),
        }])
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<Summary> {
        let mut notes = Vec::new();
        let before = findings.first().map(|f| f.size_bytes).unwrap_or(0);

        let del = Command::new("xcrun").arg("simctl").args(["delete", "unavailable"]).output()?;
        notes.push(format!(
            "simctl delete unavailable: {}",
            if del.status.success() { "ok" } else { "failed" }
        ));
        if ctx.yolo {
            // erase-all only under yolo: it wipes user-visible simulator state.
            let erase = Command::new("xcrun").arg("simctl").args(["erase", "all"]).output()?;
            notes.push(format!(
                "simctl erase all (--yolo): {}",
                if erase.status.success() { "ok" } else { "failed" }
            ));
        } else {
            notes.push("erase-all skipped (needs --yolo): it wipes simulator contents".into());
        }
        let after = dir_size(&ctx.home.join("Library/Developer/CoreSimulator/Devices"));
        Ok(Summary {
            op: self.name().into(),
            items_touched: 1,
            bytes_freed_estimate: before.saturating_sub(after),
            notes,
        })
    }
}
