//! Simulator hygiene via exact per-device `xcrun simctl delete <UDID>` actions.
//! Confirmation flags never add an unpreviewed erase-all operation.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use super::{Action, ApplyOutcome, Finding, Op, command_stdout, dir_size, optional_command_stdout};
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
    let mut findings: Vec<Finding> = simulator_states(output)?
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
        .collect::<Result<_>>()?;
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
    let total = available
        .iter()
        .fold(0u64, |sum, (size, _)| sum.saturating_add(*size));
    if total == 0 {
        return None;
    }
    available.sort_by(|(a_size, a), (b_size, b)| b_size.cmp(a_size).then(a.udid.cmp(&b.udid)));
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
    fn available_simulator_data_is_disclosed_but_never_actionable() {
        let mut ctx = test_ctx();
        ctx.home = PathBuf::from("/Users/example");
        let output = r#"{"devices":{
            "com.apple.CoreSimulator.SimRuntime.iOS-27-0":[
              {"udid":"AAAA-1","isAvailable":true,"name":"iPhone 17","state":"Booted","dataPathSize":14000000000,"lastUsedAt":"2026-09-15T07:48:32Z"},
              {"udid":"BBBB-2","isAvailable":true,"name":"iPad Pro","state":"Shutdown","dataPathSize":11000000000,"lastBootedAt":"2026-07-04T09:00:00Z"},
              {"udid":"CCCC-3","isAvailable":true,"name":"Apple Watch","state":"Shutdown","dataPathSize":8000},
              {"udid":"DDDD-4","isAvailable":true,"name":"iPhone Air","state":"Shutdown","dataPathSize":4500000000,"lastBootedAt":null}
            ]}}"#;

        let findings = findings_from_simctl(output, &ctx).unwrap();

        assert_eq!(findings.len(), 1, "{findings:?}");
        let finding = &findings[0];
        assert_eq!(finding.action, Action::None);
        assert!(!finding.action.is_actionable());
        assert_eq!(finding.danger, 0);
        assert_eq!(finding.size_bytes, 29_500_008_000);
        assert_eq!(
            finding.target(),
            Some(Path::new(
                "/Users/example/Library/Developer/CoreSimulator/Devices"
            ))
        );
        assert_eq!(finding.label, "Simulator device data (4 available devices)");
        let note = &finding.note;
        assert!(note.starts_with("EXCLUDED:"), "{note}");
        let first = note.find("iPhone 17 (AAAA-1)").expect(note);
        let second = note.find("iPad Pro (BBBB-2)").expect(note);
        let third = note.find("iPhone Air (DDDD-4)").expect(note);
        assert!(first < second && second < third, "{note}");
        assert!(
            !note.contains("Apple Watch"),
            "only the three largest: {note}"
        );
        assert!(
            note.contains("(AAAA-1) 13.0 GB, last used 2026-09-15"),
            "{note}"
        );
        // Xcode 16 reported only `lastBootedAt`.
        assert!(
            note.contains("(BBBB-2) 10.2 GB, last booted 2026-07-04"),
            "{note}"
        );
        assert!(
            note.contains("(DDDD-4) 4.2 GB, last use not recorded"),
            "{note}"
        );
        assert!(note.contains("xcrun simctl delete <UDID>"), "{note}");
    }

    #[test]
    fn apply_skips_the_report_only_disclosure_instead_of_refusing_it() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-simulators-disclosure")
            .tempdir()
            .unwrap();
        let mut ctx = test_ctx();
        ctx.home = home.path().to_path_buf();
        ctx.journal_path = home.path().join("journal.jsonl");
        // An unreadable device makes any walk of the simulator tree fail. A
        // disclosure-only plan must not walk it at all: on a working machine that
        // tree is the 100+ GB this finding exists to describe.
        let locked = home
            .path()
            .join("Library/Developer/CoreSimulator/Devices/LOCKED/data");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("payload"), "x").unwrap();
        let locked = locked.parent().unwrap();
        let original = std::fs::metadata(locked).unwrap().permissions();
        std::fs::set_permissions(locked, std::os::unix::fs::PermissionsExt::from_mode(0o000))
            .unwrap();
        assert!(
            dir_size(&home.path().join("Library/Developer/CoreSimulator/Devices")).is_err(),
            "control: the locked device must make a tree walk fail"
        );
        let disclosure = Finding::new(
            "Simulator device data (1 available devices)",
            Some(home.path().join("Library/Developer/CoreSimulator/Devices")),
            10,
            "EXCLUDED: visibility only",
            0,
            Action::None,
        );

        let outcome = Simulators.apply(&[disclosure], &ctx);
        std::fs::set_permissions(locked, original).unwrap();

        let outcome = outcome.unwrap();
        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
        assert_eq!(outcome.summary.items_touched, 0);
    }

    #[test]
    fn a_changed_size_field_drops_only_the_disclosure() {
        let mut ctx = test_ctx();
        ctx.diagnostic_output = crate::safety::DiagnosticOutput::Capture;
        let output = r#"{"devices":{"runtime":[
            {"udid":"AAAA-1","isAvailable":true,"name":"iPhone","dataPathSize":"12 GB"},
            {"udid":"GONE-2","isAvailable":false,"name":"Old iPhone","dataPathSize":1.5}
        ]}}"#;

        let findings = findings_from_simctl(output, &ctx).unwrap();

        assert_eq!(findings.len(), 1, "{findings:?}");
        assert_eq!(
            findings[0].action,
            Action::command("xcrun", &["simctl", "delete", "GONE-2"])
        );
        assert!(
            ctx.take_diagnostics()
                .iter()
                .any(|message| message.contains("size fields did not parse"))
        );
    }

    #[test]
    fn simulator_disclosure_is_omitted_when_any_size_is_unreported() {
        let mut ctx = test_ctx();
        ctx.diagnostic_output = crate::safety::DiagnosticOutput::Capture;
        let output = r#"{"devices":{"runtime":[
            {"udid":"AAAA-1","isAvailable":true,"name":"iPhone","dataPathSize":10},
            {"udid":"BBBB-2","isAvailable":true,"name":"iPad"}
        ]}}"#;

        assert!(findings_from_simctl(output, &ctx).unwrap().is_empty());
        assert!(
            ctx.take_diagnostics()
                .iter()
                .any(|message| message.contains("no dataPathSize for BBBB-2")),
            "an omitted disclosure must say why"
        );

        // Positive control: the same list with every size reported is disclosed.
        let reported = output.replace(r#""name":"iPad"}"#, r#""name":"iPad","dataPathSize":5}"#);
        let findings = findings_from_simctl(&reported, &ctx).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].size_bytes, 15);
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
