//! Simulator hygiene via exact per-device `xcrun simctl delete <UDID>` actions.
//! Confirmation flags never add an unpreviewed erase-all operation.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};

use super::project::{UnreadFolder, unread_folders_finding};
use super::{Action, ApplyOutcome, Finding, Op, command_stdout, dir_size, optional_command_stdout};
use crate::process::{BoundedCommand as _, QUERY_TIMEOUT};
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

/// The fields only the report-only disclosure reads. Parsed apart from
/// [`Device`] so an unexpected shape here drops the disclosure, never the
/// unavailable-device authority that shares the same simctl output.
#[derive(serde::Deserialize)]
struct DisclosedDeviceList {
    devices: BTreeMap<String, Vec<DisclosedDevice>>,
}

#[derive(serde::Deserialize)]
struct DisclosedDevice {
    #[serde(rename = "isAvailable")]
    is_available: bool,
    udid: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(rename = "dataPathSize", default)]
    data_path_size: Option<u64>,
    // Xcode 27 reports `lastUsedAt`; Xcode 16 reported only `lastBootedAt`.
    #[serde(rename = "lastUsedAt", default)]
    last_used_at: Option<String>,
    #[serde(rename = "lastBootedAt", default)]
    last_booted_at: Option<String>,
}

fn simctl(args: &[&str]) -> Result<String> {
    let command = format!("`xcrun simctl {}`", args.join(" "));
    command_stdout(
        Command::new("xcrun")
            .arg("simctl")
            .args(args)
            .output_within(QUERY_TIMEOUT),
        &command,
    )
}

/// A simulator UDID is a UUID (8-4-4-4-12 hexadecimal digits). Anything
/// else is refused, including the words `simctl delete` reads as a set of
/// devices (`all`, `unavailable`, `booted`).
fn simulator_device_path(root: &Path, udid: &str) -> Result<std::path::PathBuf> {
    let groups = udid.split('-').map(str::len).collect::<Vec<_>>();
    if groups != [8, 4, 4, 4, 12]
        || !udid
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
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
    let mut findings = Vec::new();
    // A device whose data cannot be measured offers nothing and is named in
    // one error finding (S8); the other devices are still offered. A malformed
    // identifier is not a measurement and still fails the category.
    let mut unread = Vec::new();
    for (udid, is_available) in simulator_states(output)? {
        if is_available {
            continue;
        }
        let path = simulator_device_path(&device_root, &udid)?;
        let size = match dir_size(&path)
            .with_context(|| format!("cannot measure simulator device {}", path.display()))
        {
            Ok(size) => size,
            Err(error) => {
                unread.push(UnreadFolder::new(&path, &error));
                continue;
            }
        };
        findings.push(Finding::command(
            format!("unavailable Apple simulator device {udid}"),
            size,
            format!(
                "references a missing runtime; its device data uses ~{}",
                crate::report::gb(size)
            ),
            escalate(4, size),
            CommandAuthority::DeleteSimulator { udid },
        ));
    }
    if !unread.is_empty() {
        findings.push(unread_folders_finding("simulator devices", &unread));
    }
    findings.extend(available_device_disclosure(output, device_root, ctx));
    Ok(findings)
}

/// Report-only disclosure of the app data held by available simulators.
///
/// Those devices are the bulk of simulator storage on a working machine, yet
/// deleting one destroys the apps and data inside it, which no preview can
/// establish is unwanted — so the category's only authority stays unavailable
/// devices, and this finding just makes the rest visible. The sizes are
/// simctl's own `dataPathSize`; walking the trees here would cost minutes on a
/// large set. When a size is missing or unreadable the disclosure is omitted
/// with a diagnostic rather than showing a total known to be short.
fn available_device_disclosure(output: &str, device_root: PathBuf, ctx: &Ctx) -> Option<Finding> {
    let omitted = |reason: String| {
        ctx.diagnostic(
            "info",
            format!("working simulators are not sized in this preview: {reason}"),
        );
        None
    };
    let devices: DisclosedDeviceList = match serde_json::from_str(output) {
        Ok(devices) => devices,
        Err(error) => return omitted(format!("simctl size fields did not parse: {error}")),
    };
    let mut available = Vec::new();
    for device in devices.devices.values().flatten() {
        if !device.is_available {
            continue;
        }
        let Some(size) = device.data_path_size else {
            return omitted(format!(
                "simctl reported no dataPathSize for {}",
                device.udid
            ));
        };
        available.push((size, device));
    }
    let Some(total) = available
        .iter()
        .try_fold(0u64, |sum, (size, _)| sum.checked_add(*size))
    else {
        return omitted("the reported sizes overflow a byte total".to_string());
    };
    if total == 0 {
        return None;
    }
    available.sort_by(|(left_size, left), (right_size, right)| {
        right_size.cmp(left_size).then(left.udid.cmp(&right.udid))
    });
    let largest = available
        .iter()
        .take(3)
        .map(|(size, device)| {
            let last_used = match (
                stamp_day(device.last_used_at.as_deref()),
                stamp_day(device.last_booted_at.as_deref()),
            ) {
                (Some(day), _) => format!("last used {day}"),
                (None, Some(day)) => format!("last booted {day}"),
                (None, None) => "last use not recorded".to_string(),
            };
            format!(
                "{} ({}) {}, {last_used}",
                device.name.as_deref().unwrap_or("unnamed"),
                device.udid,
                crate::report::gb(*size)
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    let count = match available.len() {
        1 => "1 available device".to_string(),
        count => format!("{count} available devices"),
    };
    Some(Finding::new(
        format!("Simulator device data ({count})"),
        Some(device_root),
        total,
        format!(
            "EXCLUDED: apps and data inside simulators that still work, listed for visibility only; \
             devtrim deletes only devices whose runtime is gone. Largest: {largest}. \
             `xcrun simctl delete <UDID>` removes a device you no longer need, with everything in it"
        ),
        0,
        Action::None,
    ))
}

/// Why `xcrun --version` says this Mac has no developer tools at all, or `None`
/// for any other answer. `/usr/bin/xcrun` is a shim present on every Mac; with
/// no developer directory selected, or one that does not exist, it exits 1 with
/// one of these two errors (observed with Apple's shim and
/// `DEVELOPER_DIR=/nonexistent`, and with the Command Line Tools removed). Only
/// exactly these answers mean there is nothing to manage; any other failure
/// still fails the category.
fn no_developer_tools(output: &Output) -> Option<&'static str> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim_start();
    (output.status.code() == Some(1)
        && (stderr.starts_with("xcrun: error: invalid active developer path")
            || stderr.starts_with("xcrun: error: missing DEVELOPER_DIR path")))
    .then_some("no developer tools are installed or selected (`xcrun` has no developer directory)")
}

/// Why `xcrun simctl` says there is no simulator tool, or `None` for any other
/// answer. The Command Line Tools ship `xcrun` but not `simctl`, which only
/// Xcode provides, and the shim then exits 72 with this error (observed with
/// `DEVELOPER_DIR=/Library/Developer/CommandLineTools`). Any other failure of
/// `simctl` still fails the category.
fn no_simctl(output: &Output) -> Option<&'static str> {
    let stderr = String::from_utf8_lossy(&output.stderr);
    (output.status.code() == Some(72)
        && stderr.trim_start().starts_with(
            "xcrun: error: unable to find utility \"simctl\", not a developer tool or in PATH",
        ))
    .then_some("the selected developer tools have no `simctl`, which only Xcode provides")
}

/// The `YYYY-MM-DD` prefix of a simctl timestamp, if it has one.
fn stamp_day(stamp: Option<&str>) -> Option<&str> {
    stamp.and_then(|stamp| stamp.get(..10))
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
        let version = Command::new("xcrun")
            .arg("--version")
            .output_within(QUERY_TIMEOUT);
        if let Ok(output) = &version
            && let Some(reason) = no_developer_tools(output)
        {
            ctx.diagnostic("info", format!("no simulators to manage: {reason}"));
            return Ok(Vec::new());
        }
        let Some(version) = optional_command_stdout(version, "`xcrun --version`")? else {
            return Ok(Vec::new());
        };
        if version.trim().is_empty() {
            anyhow::bail!("`xcrun --version` returned empty output");
        }
        let list = Command::new("xcrun")
            .args(["simctl", "list", "devices", "--json"])
            .output_within(QUERY_TIMEOUT);
        if let Ok(output) = &list
            && let Some(reason) = no_simctl(output)
        {
            ctx.diagnostic("info", format!("no simulators to manage: {reason}"));
            return Ok(Vec::new());
        }
        let output = command_stdout(list, "`xcrun simctl list devices --json`")?;
        findings_from_simctl(&output, ctx)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let mut outcome = ApplyOutcome::new(self.name());
        // The CLI applies a plan that holds only the report-only disclosure, and
        // measuring before and after walks every simulator's data to change
        // nothing.
        if !findings
            .iter()
            .any(|finding| finding.action.is_actionable())
        {
            return Ok(outcome);
        }
        let before = dir_size(&ctx.home.join("Library/Developer/CoreSimulator/Devices"))?;
        for finding in findings {
            // The available-device disclosure is report-only. Skipping on
            // actionability, never on a missing authority, keeps an actionable
            // forgery refused below.
            if !finding.action.is_actionable() {
                continue;
            }
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
                super::run_command_authority(
                    self.name(),
                    authority,
                    finding.size_bytes,
                    ctx,
                    |_| format!("deleted unavailable simulator device {udid}"),
                )
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
            roots_origin: crate::safety::RootsOrigin::Default,
            active_days: 30,
            retain_days: 30,
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
    fn a_mixed_plan_skips_the_disclosure_and_refuses_only_the_forgery() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-simulators-mixed")
            .tempdir()
            .unwrap();
        std::fs::create_dir_all(home.path().join("Library/Developer/CoreSimulator/Devices"))
            .unwrap();
        let mut ctx = test_ctx();
        ctx.home = home.path().to_path_buf();
        ctx.journal_path = home.path().join("journal.jsonl");
        let disclosure = Finding::new(
            "Simulator device data (1 available devices)",
            Some(home.path().join("Library/Developer/CoreSimulator/Devices")),
            10,
            "EXCLUDED: visibility only",
            0,
            Action::None,
        );
        // Real authority whose action changed after preview: apply refuses it
        // with its own message before anything runs. Apply stops at the first
        // error, so a disclosure mistaken for a command would surface instead.
        let mut forged = Finding::command(
            "unavailable Apple simulator device 60E60E60-0000-4000-8000-000000000001",
            10,
            "forged",
            4,
            CommandAuthority::DeleteSimulator {
                udid: "60E60E60-0000-4000-8000-000000000001".to_string(),
            },
        );
        forged.action = Action::command(
            "xcrun",
            &["simctl", "delete", "07E907E9-0000-4000-8000-000000000002"],
        );

        let outcome = Simulators.apply(&[disclosure, forged], &ctx).unwrap();

        let errors = format!("{:?}", outcome.errors);
        assert_eq!(outcome.errors.len(), 1, "{errors}");
        assert!(
            errors.contains("refusing altered simulator action"),
            "the disclosure was treated as a command: {errors}"
        );
        assert_eq!(outcome.summary.items_touched, 0);
    }

    /// One unavailable device whose data cannot be measured offers nothing
    /// and is named in one error finding; the others are still offered.
    #[test]
    fn an_unmeasurable_device_withholds_only_itself() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-simulators-unread")
            .tempdir()
            .unwrap();
        let mut ctx = test_ctx();
        ctx.home = home.path().to_path_buf();
        let devices = ctx.home.join("Library/Developer/CoreSimulator/Devices");
        let readable = "11111111-1111-1111-1111-111111111111";
        let sealed_udid = "22222222-2222-2222-2222-222222222222";
        std::fs::create_dir_all(devices.join(readable).join("data")).unwrap();
        std::fs::write(devices.join(readable).join("data/file"), "x").unwrap();
        let sealed = crate::ops::Unreadable::new(&devices.join(sealed_udid).join("data"), 0o000);
        let output = format!(
            r#"{{"devices":{{"runtime":[{{"isAvailable":false,"udid":"{readable}"}},{{"isAvailable":false,"udid":"{sealed_udid}"}}]}}}}"#
        );

        let findings = findings_from_simctl(&output, &ctx).unwrap();

        drop(sealed);
        let commands = findings
            .iter()
            .filter(|finding| matches!(finding.action, Action::Command { .. }))
            .map(|finding| finding.label.clone())
            .collect::<Vec<_>>();
        assert_eq!(
            commands,
            vec![format!("unavailable Apple simulator device {readable}")]
        );
        let errors = findings
            .iter()
            .filter_map(Finding::scan_error)
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 1, "{findings:?}");
        assert!(errors[0].contains(sealed_udid), "{errors:?}");
    }

    #[test]
    fn malformed_simulator_device_id_is_an_error() {
        let ctx = test_ctx();
        // `simctl delete` reads these words as every device of a kind, and a
        // name that is not a UUID is not a device identifier at all.
        for udid in [
            "../../escape",
            "--help",
            "all",
            "unavailable",
            "booted",
            "NOT-A-UUID",
        ] {
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
}
