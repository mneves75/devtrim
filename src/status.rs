//! Read-only machine vitals.
//!
//! Every value comes from a fixed-argv system tool, parsed by a pure function
//! that fails closed. A metric that cannot be read is reported as unavailable
//! with its reason, never as a fabricated zero — a dashboard that invents a
//! number is worse than one that admits a gap, because the invented number is
//! indistinguishable from a measurement.
//!
//! The health score names the inputs it was missing for the same reason: a
//! score computed over half the signals must not present itself as a verdict
//! over all of them.

use std::io::IsTerminal;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::time::Duration;

use anyhow::{Context, Result};
use colored::Colorize;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};

use crate::report;
use crate::safety::Ctx;
use crate::theme::{Theme, Token};

/// Closed set of system tools this module may execute.
///
/// `CODING_STANDARDS.md` S12 makes `Command::new` reached through a variable a
/// hard violation unless the value is a `&'static str` from a closed enum, and
/// the `no-shell-invocation` rule cannot see that shape — it only inspects the
/// literal at the call site. This mirrors `CommandAuthority::parts()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SystemTool {
    BootTime,
    LoadAverage,
    LogicalCpuCount,
    PhysicalMemory,
    VmStat,
    DataVolume,
    RootFilesystem,
    Battery,
    Thermal,
    NetworkInterfaces,
    Processes,
}

impl SystemTool {
    fn parts(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Self::BootTime => ("sysctl", &["-n", "kern.boottime"]),
            Self::LoadAverage => ("sysctl", &["-n", "vm.loadavg"]),
            Self::LogicalCpuCount => ("sysctl", &["-n", "hw.logicalcpu"]),
            Self::PhysicalMemory => ("sysctl", &["-n", "hw.memsize"]),
            Self::VmStat => ("vm_stat", &[]),
            Self::DataVolume => ("df", &["-k", "/System/Volumes/Data"]),
            Self::RootFilesystem => ("df", &["-k", "/"]),
            Self::Battery => ("pmset", &["-g", "batt"]),
            Self::Thermal => ("pmset", &["-g", "therm"]),
            Self::NetworkInterfaces => ("netstat", &["-ib"]),
            Self::Processes => ("ps", &["-Aco", "pid,pcpu,rss,comm", "-r"]),
        }
    }

    /// The metric name used when this tool's failure is reported.
    fn metric(self) -> &'static str {
        match self {
            Self::BootTime => "uptime",
            Self::LoadAverage => "load",
            Self::LogicalCpuCount => "cpu",
            Self::PhysicalMemory | Self::VmStat => "memory",
            Self::DataVolume | Self::RootFilesystem => "disk",
            Self::Battery => "battery",
            Self::Thermal => "thermal",
            Self::NetworkInterfaces => "network",
            Self::Processes => "processes",
        }
    }
}

/// Runs one of the closed set above. No shell, ever.
///
/// The parsers read the C locale's spelling: under `pt_BR` `sysctl` prints a
/// load average as `65,41` and `ps` a CPU share as `136,6`, which would turn
/// two metrics unavailable and, with them, raise the health score.
fn capture(tool: SystemTool) -> Result<String> {
    let (program, args) = tool.parts();
    let label = format!("`{program} {}`", args.join(" "));
    crate::ops::command_stdout(
        Command::new(program).args(args).env("LC_ALL", "C").output(),
        &label,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Memory {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub active_bytes: u64,
    /// File-backed pages macOS can reclaim under pressure. Deliberately NOT
    /// counted as used: doing so reports a healthy machine at 96% and then
    /// deducts from its health score for it.
    pub inactive_bytes: u64,
    pub wired_bytes: u64,
    /// Physical footprint of compressed memory. Genuinely occupied, so it
    /// counts as used. Zero on a system with no compressor.
    pub compressed_bytes: u64,
    /// `active + wired + compressed` — what is actually resident and
    /// unreclaimable. The basis is stated because every other definition of
    /// "used memory" on macOS produces a materially different number.
    pub used_bytes: u64,
}

impl Memory {
    fn used_percent(self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        (self.used_bytes as f64) * 100.0 / (self.total_bytes as f64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Disk {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub available_bytes: u64,
}

impl Disk {
    fn used_percent(self) -> f64 {
        let denominator = self.used_bytes.saturating_add(self.available_bytes);
        if denominator == 0 {
            return 0.0;
        }
        (self.used_bytes as f64) * 100.0 / (denominator as f64)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Battery {
    pub percent: u8,
    pub state: String,
    pub on_ac_power: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Thermal {
    /// `None` when the system has recorded no limit, which is the nominal state.
    pub cpu_speed_limit_percent: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct Network {
    /// Cumulative since boot, not a rate. A rate needs two samples and this
    /// command takes one, so calling it throughput would be a lie.
    pub received_bytes: u64,
    pub sent_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Process {
    pub pid: u32,
    pub cpu_percent: f64,
    pub resident_bytes: u64,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Health {
    pub score: u8,
    /// Signals that could not be read. A score computed without these is not a
    /// verdict over them, and saying so is the whole point of the field.
    pub missing_inputs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct StatusReport {
    pub uptime_seconds: Option<u64>,
    pub load_average: Option<[f64; 3]>,
    pub cpu_count: Option<u32>,
    pub memory: Option<Memory>,
    pub disk: Option<Disk>,
    pub battery: Option<Battery>,
    pub thermal: Option<Thermal>,
    pub network: Option<Network>,
    pub top_processes: Vec<Process>,
    pub health: Health,
    /// One entry per metric that could not be read, with the reason.
    pub unavailable: Vec<String>,
}

/// `{ 9.88 10.03 7.71 }`
pub(crate) fn parse_loadavg(output: &str) -> Result<[f64; 3]> {
    let trimmed = output.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .ok_or_else(|| anyhow::anyhow!("unexpected vm.loadavg shape: {trimmed}"))?;
    let mut values = [0.0f64; 3];
    let mut fields = inner.split_whitespace();
    for slot in &mut values {
        let field = fields
            .next()
            .ok_or_else(|| anyhow::anyhow!("vm.loadavg is missing a value: {trimmed}"))?;
        *slot = field
            .parse::<f64>()
            .with_context(|| format!("invalid load average `{field}`"))?;
        if !slot.is_finite() || *slot < 0.0 {
            anyhow::bail!("invalid load average `{field}`");
        }
    }
    if fields.next().is_some() {
        anyhow::bail!("vm.loadavg carried more values than expected: {trimmed}");
    }
    Ok(values)
}

/// `{ sec = 1788197900, usec = 237762 } Mon Aug 31 14:38:20 2026`
pub(crate) fn parse_boottime_seconds(output: &str, now_unix: u64) -> Result<u64> {
    let trimmed = output.trim();
    // Fields are matched by exact key, never by substring: `usec = ` ends with
    // `sec = `, so a substring search silently reads the microseconds field as
    // the boot time on any line where `sec` itself is absent.
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|value| value.split('}').next())
        .ok_or_else(|| anyhow::anyhow!("unexpected kern.boottime shape: {trimmed}"))?;
    let mut seconds = None;
    for field in inner.split(',') {
        let mut halves = field.splitn(2, '=');
        let (Some(key), Some(value)) = (halves.next(), halves.next()) else {
            continue;
        };
        if key.trim() != "sec" {
            continue;
        }
        let value = value.trim();
        seconds = Some(
            value
                .parse::<u64>()
                .with_context(|| format!("invalid boot time `{value}`"))?,
        );
    }
    let boot = seconds.ok_or_else(|| anyhow::anyhow!("kern.boottime carried no seconds value"))?;
    // A boot time in the future means the clock disagrees with itself; refuse
    // rather than report a nonsense uptime.
    now_unix
        .checked_sub(boot)
        .ok_or_else(|| anyhow::anyhow!("boot time is in the future"))
}

/// `vm_stat`, whose page size is declared in its own header. Assuming 4096 would
/// misreport every figure by 4x on Apple silicon, where the page is 16384.
pub(crate) fn parse_vm_stat(output: &str, total_bytes: u64) -> Result<Memory> {
    let header = output
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("vm_stat produced no output"))?;
    let marker = "page size of ";
    let page_start = header
        .find(marker)
        .ok_or_else(|| anyhow::anyhow!("vm_stat did not declare its page size"))?
        .saturating_add(marker.len());
    let page_digits = header
        .get(page_start..)
        .unwrap_or_default()
        .split(|character: char| !character.is_ascii_digit())
        .next()
        .unwrap_or_default();
    let page_size = page_digits
        .parse::<u64>()
        .with_context(|| format!("invalid vm_stat page size `{page_digits}`"))?;
    if page_size == 0 {
        anyhow::bail!("vm_stat declared a zero page size");
    }

    let pages = |name: &str| -> Result<u64> {
        let line = output
            .lines()
            .find(|line| line.starts_with(name))
            .ok_or_else(|| anyhow::anyhow!("vm_stat is missing `{name}`"))?;
        let value = line
            .rsplit(':')
            .next()
            .unwrap_or_default()
            .trim()
            .trim_end_matches('.');
        value
            .parse::<u64>()
            .with_context(|| format!("invalid vm_stat value for `{name}`: {value}"))
    };

    // A measured quantity is exact or refused (`CODING_STANDARDS.md` S8).
    let overflow = || anyhow::anyhow!("vm_stat page counts overflow");
    let bytes = |count: u64| count.checked_mul(page_size).ok_or_else(overflow);
    let free = bytes(
        pages("Pages free")?
            .checked_add(pages("Pages speculative")?)
            .ok_or_else(overflow)?,
    )?;
    let active = bytes(pages("Pages active")?)?;
    let inactive = bytes(pages("Pages inactive")?)?;
    let wired = bytes(pages("Pages wired down")?)?;
    // Absent only on a system with no memory compressor, where zero is the
    // correct value rather than a stand-in for an unread one.
    let compressed = output
        .lines()
        .find(|line| line.starts_with("Pages occupied by compressor"))
        .map_or(Ok(0), |_| pages("Pages occupied by compressor"))
        .and_then(bytes)?;
    let used = active
        .checked_add(wired)
        .and_then(|sum| sum.checked_add(compressed))
        .ok_or_else(overflow)?;
    Ok(Memory {
        total_bytes,
        free_bytes: free,
        active_bytes: active,
        inactive_bytes: inactive,
        wired_bytes: wired,
        compressed_bytes: compressed,
        used_bytes: used.min(total_bytes),
    })
}

/// `df -k <path>`; the second line carries 1024-byte blocks.
pub(crate) fn parse_df(output: &str) -> Result<Disk> {
    let line = output
        .lines()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("df produced no data row"))?;
    let fields = line.split_whitespace().collect::<Vec<_>>();
    let (Some(used), Some(available)) = (fields.get(2), fields.get(3)) else {
        anyhow::bail!("unexpected df row: {line}");
    };
    let kib = |value: &str| -> Result<u64> {
        value
            .parse::<u64>()
            .with_context(|| format!("invalid df value `{value}`"))?
            .checked_mul(1024)
            .ok_or_else(|| anyhow::anyhow!("df value overflows: {value}"))
    };
    let used_bytes = kib(used)?;
    let available_bytes = kib(available)?;
    Ok(Disk {
        total_bytes: used_bytes.saturating_add(available_bytes),
        used_bytes,
        available_bytes,
    })
}

/// `pmset -g batt`
pub(crate) fn parse_battery(output: &str) -> Result<Option<Battery>> {
    let on_ac_power = output.contains("'AC Power'");
    let Some(line) = output.lines().find(|line| line.contains('%')) else {
        // A desktop with no battery is a valid machine, not a failure.
        return Ok(None);
    };
    let percent_field = line
        .split_whitespace()
        .find(|field| field.trim_end_matches(&[';', ','][..]).ends_with('%'))
        .ok_or_else(|| anyhow::anyhow!("unexpected pmset battery row: {line}"))?;
    let digits = percent_field.trim_end_matches(&[';', ',', '%'][..]);
    let percent = digits
        .parse::<u8>()
        .with_context(|| format!("invalid battery percentage `{digits}`"))?;
    if percent > 100 {
        anyhow::bail!("battery percentage out of range: {percent}");
    }
    let state = line
        .split(';')
        .nth(1)
        .map_or_else(|| "unknown".to_string(), |value| value.trim().to_string());
    Ok(Some(Battery {
        percent,
        state,
        on_ac_power,
    }))
}

/// `pmset -g therm`
pub(crate) fn parse_thermal(output: &str) -> Result<Thermal> {
    let Some(line) = output.lines().find(|line| line.contains("CPU_Speed_Limit")) else {
        // Nominal is a specific answer, not merely the absence of a key. Empty,
        // truncated, or reshaped output would otherwise be reported as "no
        // limit recorded", and `health` would then count thermal as read and
        // score the machine higher than the evidence supports.
        // The note has to be the CPU-specific one. `pmset` prints three
        // independent lines — thermal warning, performance warning, CPU power
        // status — and only the last speaks for `CPU_Speed_Limit`. Accepting
        // the generic phrase would let truncated output carrying just the
        // thermal line stand in for a CPU reading that was never made.
        if output
            .lines()
            .any(|line| line.contains("No CPU power status has been recorded"))
        {
            return Ok(Thermal {
                cpu_speed_limit_percent: None,
            });
        }
        anyhow::bail!(
            "unrecognized `pmset -g therm` output: no CPU_Speed_Limit and no CPU power-status note"
        );
    };
    let value = line.rsplit('=').next().unwrap_or_default().trim();
    let percent = value
        .parse::<u8>()
        .with_context(|| format!("invalid CPU_Speed_Limit `{value}`"))?;
    if percent > 100 {
        anyhow::bail!("CPU_Speed_Limit out of range: {percent}");
    }
    Ok(Thermal {
        cpu_speed_limit_percent: Some(percent),
    })
}

/// `netstat -ib`.
///
/// Each interface appears several times: once for its `<Link#N>` row and once
/// per configured address. Summing every row would count `lo0` three times, so
/// only the link rows are totalled.
pub(crate) fn parse_netstat(output: &str) -> Result<Network> {
    let mut received = 0u64;
    let mut sent = 0u64;
    let mut rows = 0usize;
    for line in output.lines().skip(1) {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        if !fields
            .get(2)
            .is_some_and(|network| network.starts_with("<Link#"))
        {
            continue;
        }
        // Columns are counted from the END. `Address` is empty on a link row
        // for an interface with no hardware address (`lo0`, `utun*`) and
        // populated for one with a MAC, so a link row is 10 or 11 fields wide
        // depending on the interface. Indexing from the left reads `Ipkts` as
        // `Ibytes` on exactly half of them; the trailing seven columns are
        // always `Ipkts Ierrs Ibytes Opkts Oerrs Obytes Coll`. Any other width
        // is not the documented shape, and a sum refuses rather than skips it.
        let (10 | 11) = fields.len() else {
            anyhow::bail!("unexpected netstat link row: {line}");
        };
        let (ibytes, obytes) = (fields[fields.len() - 5], fields[fields.len() - 2]);
        // An aggregate is exact or it is refused. Skipping an unparseable link
        // row would report a confidently wrong total, which is the failure this
        // module exists to avoid; a top-N display list may skip a row, a sum
        // may not (`CODING_STANDARDS.md` S8).
        let ibytes = ibytes
            .parse::<u64>()
            .with_context(|| format!("invalid netstat Ibytes `{ibytes}`"))?;
        let obytes = obytes
            .parse::<u64>()
            .with_context(|| format!("invalid netstat Obytes `{obytes}`"))?;
        received = received
            .checked_add(ibytes)
            .ok_or_else(|| anyhow::anyhow!("netstat received bytes overflow"))?;
        sent = sent
            .checked_add(obytes)
            .ok_or_else(|| anyhow::anyhow!("netstat sent bytes overflow"))?;
        rows = rows.saturating_add(1);
    }
    if rows == 0 {
        anyhow::bail!("netstat reported no interface link rows");
    }
    Ok(Network {
        received_bytes: received,
        sent_bytes: sent,
    })
}

/// `ps -Aco pid,pcpu,rss,comm -r`, already sorted by CPU.
///
/// Unlike the aggregate parsers, this one skips a row it cannot read: the
/// result is a top-N display list, not a sum, so a dropped row shortens the
/// list rather than corrupting a number. It still fails closed when nothing
/// parses at all.
pub(crate) fn parse_processes(output: &str, limit: usize) -> Result<Vec<Process>> {
    let mut processes = Vec::new();
    for line in output.lines().skip(1) {
        if processes.len() >= limit {
            break;
        }
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(cpu), Some(rss)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let command = fields.collect::<Vec<_>>().join(" ");
        if command.is_empty() {
            continue;
        }
        let (Ok(pid), Ok(cpu), Ok(rss)) =
            (pid.parse::<u32>(), cpu.parse::<f64>(), rss.parse::<u64>())
        else {
            continue;
        };
        if !cpu.is_finite() || cpu < 0.0 {
            continue;
        }
        processes.push(Process {
            pid,
            cpu_percent: cpu,
            resident_bytes: rss.saturating_mul(1024),
            command,
        });
    }
    if processes.is_empty() {
        anyhow::bail!("ps reported no processes");
    }
    Ok(processes)
}

/// Deducts from a perfect score for each pressure signal that is present, and
/// records every signal that could not be read.
pub(crate) fn health(report: &StatusReport) -> Health {
    let mut score = 100i32;
    let mut missing = Vec::new();

    match report.disk {
        Some(disk) => {
            let used = disk.used_percent();
            if used >= 95.0 {
                score -= 40;
            } else if used >= 90.0 {
                score -= 25;
            } else if used >= 80.0 {
                score -= 10;
            }
        }
        None => missing.push("disk".to_string()),
    }

    match report.memory {
        Some(memory) if memory.total_bytes > 0 => {
            let used = memory.used_percent();
            if used >= 95.0 {
                score -= 20;
            } else if used >= 85.0 {
                score -= 10;
            }
        }
        Some(_) | None => missing.push("memory".to_string()),
    }

    match (report.load_average, report.cpu_count) {
        (Some(load), Some(cpus)) if cpus > 0 => {
            let per_core = load[0] / f64::from(cpus);
            if per_core >= 2.0 {
                score -= 20;
            } else if per_core >= 1.0 {
                score -= 10;
            }
        }
        _ => missing.push("load".to_string()),
    }

    match &report.thermal {
        Some(thermal) => {
            if let Some(limit) = thermal.cpu_speed_limit_percent
                && limit < 100
            {
                score -= 20;
            }
        }
        None => missing.push("thermal".to_string()),
    }

    Health {
        score: score.clamp(0, 100) as u8,
        missing_inputs: missing,
    }
}

fn now_unix() -> Result<u64> {
    // Health inputs must fail closed on clock errors, so `project::unix_secs`,
    // which falls back to 0, is unsuitable.
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .context("system clock is before the Unix epoch")
}

fn disk_tool(data_layout: std::io::Result<bool>) -> Result<SystemTool> {
    match data_layout {
        Ok(true) => Ok(SystemTool::DataVolume),
        Ok(false) => anyhow::bail!("/System/Volumes/Data is not a directory"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(SystemTool::RootFilesystem)
        }
        Err(error) => Err(error).context("cannot inspect /System/Volumes/Data"),
    }
}

fn collect() -> StatusReport {
    let mut unavailable = Vec::new();

    /// Reads one metric, recording the reason under the tool's own metric name
    /// on failure. Written once: ten hand-copied match arms are ten chances to
    /// record a failure against the wrong metric.
    fn read<T>(
        unavailable: &mut Vec<String>,
        tool: SystemTool,
        parse: impl FnOnce(String) -> Result<T>,
    ) -> Option<T> {
        match capture(tool).and_then(parse) {
            Ok(value) => Some(value),
            Err(error) => {
                unavailable.push(format!("{}: {error:#}", tool.metric()));
                None
            }
        }
    }

    let uptime_seconds = read(&mut unavailable, SystemTool::BootTime, |output| {
        parse_boottime_seconds(&output, now_unix()?)
    });
    let load_average = read(&mut unavailable, SystemTool::LoadAverage, |output| {
        parse_loadavg(&output)
    });
    let cpu_count = read(&mut unavailable, SystemTool::LogicalCpuCount, |output| {
        let trimmed = output.trim().to_string();
        trimmed
            .parse::<u32>()
            .with_context(|| format!("invalid hw.logicalcpu `{trimmed}`"))
    });
    let total_memory = read(&mut unavailable, SystemTool::PhysicalMemory, |output| {
        let trimmed = output.trim().to_string();
        trimmed
            .parse::<u64>()
            .with_context(|| format!("invalid hw.memsize `{trimmed}`"))
    });
    let memory = total_memory.and_then(|total| {
        read(&mut unavailable, SystemTool::VmStat, |output| {
            parse_vm_stat(&output, total)
        })
    });
    // On a modern macOS the root volume is the SEALED system snapshot: `df /`
    // reports it as barely used while the writable Data volume holds everything
    // the user owns. Measured on a real machine, `/` read 12 GB used (17%) with
    // `/System/Volumes/Data` at 840 GB (94%) — a disk-hygiene tool that reports
    // the first number is reassuring exactly when it should not be.
    //
    // `statfs`, which is what `df` uses, is the only thing that separates the
    // two: `st_dev` is identical across `/`, `/System/Volumes/Data` and
    // `/Users`, so no metadata comparison can find this boundary. Metadata
    // establishes only the layout: root is valid only when Data is absent.
    // A present Data volume whose metric fails must remain unavailable.
    let disk = match disk_tool(
        std::fs::metadata("/System/Volumes/Data").map(|metadata| metadata.is_dir()),
    ) {
        Ok(tool) => read(&mut unavailable, tool, |output| parse_df(&output)),
        Err(error) => {
            unavailable.push(format!("disk: {error:#}"));
            None
        }
    };
    let battery = read(&mut unavailable, SystemTool::Battery, |output| {
        parse_battery(&output)
    })
    .flatten();
    let thermal = read(&mut unavailable, SystemTool::Thermal, |output| {
        parse_thermal(&output)
    });
    let network = read(&mut unavailable, SystemTool::NetworkInterfaces, |output| {
        parse_netstat(&output)
    });
    let top_processes = read(&mut unavailable, SystemTool::Processes, |output| {
        parse_processes(&output, 8)
    })
    .unwrap_or_default();

    let mut report = StatusReport {
        uptime_seconds,
        load_average,
        cpu_count,
        memory,
        disk,
        battery,
        thermal,
        network,
        top_processes,
        health: Health {
            score: 0,
            missing_inputs: Vec::new(),
        },
        unavailable,
    };
    report.health = health(&report);
    report
}

/// Names the band a score falls in, so severity survives with colour stripped.
pub(crate) fn health_band(score: u8) -> &'static str {
    match score {
        90..=100 => "healthy",
        70..=89 => "degraded",
        _ => "critical",
    }
}

/// Every signal [`health`] scores; a score with all of them missing was
/// computed from nothing.
const HEALTH_INPUTS: usize = 4;

/// The one-line verdict both surfaces print. A score never claims a band
/// over signals it could not read: it names them, and with none read there is
/// no verdict at all, since the perfect score it starts from is not one.
pub(crate) fn health_headline(health: &Health) -> String {
    if health.missing_inputs.len() >= HEALTH_INPUTS {
        return "health unknown (no signal could be read)".into();
    }
    let band = health_band(health.score);
    if health.missing_inputs.is_empty() {
        format!("health {}/100 ({band})", health.score)
    } else {
        format!(
            "health {}/100 ({band}, without {})",
            health.score,
            health.missing_inputs.join(", ")
        )
    }
}

fn format_uptime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h {minutes}m")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else {
        format!("{minutes}m")
    }
}

fn human_text(report: &StatusReport) -> String {
    use std::fmt::Write as _;

    let mut output = String::new();
    let heading = |output: &mut String, text: &str| {
        let _ = writeln!(output, "{}", text.bold());
    };

    heading(&mut output, "machine");
    // Only what was read: the reasons for the rest follow the headline.
    for (label, metric) in metric_rows(report) {
        let Metric::Read(lines) = metric else {
            continue;
        };
        for (index, line) in lines.iter().enumerate() {
            let label = if index == 0 { label } else { "" };
            let _ = writeln!(output, "  {label:<16}{}", report::terminal_safe(line));
        }
    }

    if !report.top_processes.is_empty() {
        let _ = writeln!(output);
        heading(&mut output, "busiest processes");
        for process in &report.top_processes {
            let _ = writeln!(
                output,
                "  {:>7}  {:>6.1}%  {:>9}  {}",
                process.pid,
                process.cpu_percent,
                report::gb(process.resident_bytes),
                report::terminal_safe(&process.command)
            );
        }
    }

    let _ = writeln!(output);
    // The band is named in the TEXT, not carried by the colour. `colored`
    // suppresses every style under `NO_COLOR`, bold included, so a colour-only
    // ladder would collapse all three bands into identical plain text — the
    // exact failure the theme module exists to prevent, on the one surface
    // that cannot use it.
    let headline = health_headline(&report.health);
    let headline = match report.health.score {
        _ if report.health.missing_inputs.len() >= HEALTH_INPUTS => headline.yellow(),
        90..=100 => headline.green(),
        70..=89 => headline.yellow(),
        _ => headline.red(),
    };
    let _ = writeln!(output, "{}", headline.bold());
    for entry in &report.unavailable {
        let _ = writeln!(output, "  unavailable: {}", report::terminal_safe(entry));
    }
    output
}

/// How often the live dashboard re-reads the machine. Collection spawns ten
/// short-lived processes, so a faster cadence would spend more time measuring
/// than displaying.
const WATCH_INTERVAL: Duration = Duration::from_secs(2);

/// Input poll cadence for the live dashboard, independent of the refresh above
/// so a keypress is never waiting on a sample.
const WATCH_POLL: Duration = Duration::from_millis(80);

/// Live dashboard. Sampling runs on a worker thread and the interface redraws
/// when a report arrives, so a slow probe delays the numbers rather than the
/// keyboard.
fn run_watch(ctx: &Ctx) -> Result<std::process::ExitCode> {
    // A live dashboard has no single-document form, and silently ignoring
    // `--json` would make it the no-op the capability contract forbids.
    if ctx.json {
        anyhow::bail!("status --watch has no JSON form; use `status --json` for one document");
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!("status --watch requires an interactive terminal; use --json for automation");
    }
    // Before the terminal changes, so no signal can leave it in raw mode.
    let interrupts =
        crate::process::forward_interrupts().context("cannot install the interrupt handler")?;
    let stop = Arc::new(AtomicBool::new(false));
    let (sender, receiver) = channel();
    let worker_stop = Arc::clone(&stop);
    let worker = std::thread::spawn(move || {
        while !worker_stop.load(Ordering::Relaxed) {
            if sender.send(collect()).is_err() {
                return;
            }
            // Wake often enough that quitting is immediate rather than waiting
            // out a full refresh interval.
            let deadline = std::time::Instant::now() + WATCH_INTERVAL;
            while std::time::Instant::now() < deadline {
                if worker_stop.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(WATCH_POLL);
            }
        }
    });

    let mut terminal = ratatui::try_init().context("cannot initialize status terminal")?;
    let theme = Theme::from_env();
    let mut latest: Option<StatusReport> = None;
    let outcome = (|| -> Result<std::process::ExitCode> {
        let mut redraw = true;
        let mut sampled_at: Option<std::time::Instant> = None;
        let mut shown_age = None;
        loop {
            if let Some(signal) = interrupts.received() {
                crate::tui::restore_and_exit(signal);
            }
            while let Ok(report) = receiver.try_recv() {
                latest = Some(report);
                sampled_at = Some(std::time::Instant::now());
                redraw = true;
            }
            // The footer counts whole seconds, so it is redrawn as each passes.
            let age = sampled_at.map(|at| at.elapsed());
            if age.map(|age| age.as_secs()) != shown_age {
                redraw = true;
            }
            if redraw {
                terminal.draw(|frame| render_watch(frame, latest.as_ref(), theme, age))?;
                shown_age = age.map(|age| age.as_secs());
                redraw = false;
            }
            if !event::poll(WATCH_POLL).context("cannot poll terminal input")? {
                continue;
            }
            match event::read().context("cannot read terminal input")? {
                Event::Resize(_, _) => redraw = true,
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    let quit = matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                        || crate::tui::is_interrupt(key);
                    if quit {
                        let failed = latest
                            .as_ref()
                            .is_some_and(|report| !report.unavailable.is_empty());
                        return Ok(if failed {
                            std::process::ExitCode::from(1)
                        } else {
                            std::process::ExitCode::SUCCESS
                        });
                    }
                }
                _ => {}
            }
        }
    })();
    let restore = ratatui::try_restore().context("cannot restore terminal after status exit");
    stop.store(true, Ordering::Relaxed);
    // Deliberately NOT joined. The flag is only observed between samples, so a
    // worker inside `collect()` would make `q` wait out the whole sample — and
    // a system command that hangs would keep the process alive forever. The
    // worker holds nothing but a channel whose receiver is about to drop, so
    // letting it finish and exit on its own is the only shutdown that cannot
    // block the interface it exists to keep responsive.
    drop(worker);
    for entry in latest.iter().flat_map(|report| report.unavailable.iter()) {
        ctx.diagnostic("warn", entry.clone());
    }
    match (outcome, restore) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(code), Ok(())) => Ok(code),
    }
}

/// The token a health headline is drawn in: its band's, and Warning when no
/// signal was read, as the CLI colours it.
fn headline_token(health: &Health) -> Token {
    if health.missing_inputs.len() >= HEALTH_INPUTS {
        return Token::Warning;
    }
    match health.score {
        90..=100 => Token::Success,
        70..=89 => Token::Warning,
        _ => Token::Critical,
    }
}

/// `age` is how long ago `report` was sampled.
fn render_watch(
    frame: &mut Frame,
    report: Option<&StatusReport>,
    theme: Theme,
    age: Option<Duration>,
) {
    let area = frame.area();
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .areas(area);

    crate::tui::render_header(frame, header, theme, "status");

    let width = usize::from(body.width.saturating_sub(2));
    let height = usize::from(body.height.saturating_sub(2));
    let mut lines = Vec::new();
    match report {
        None => lines.push(Line::raw("sampling…")),
        Some(report) => {
            lines.push(Line::styled(
                health_headline(&report.health),
                theme.bold(headline_token(&report.health)),
            ));
            // A value too wide for its row is wrapped under itself rather than
            // cut: these are the numbers the view exists to show.
            let metric_lines = |details: bool| {
                const LABEL: usize = 14;
                let mut lines = Vec::new();
                for (label, metric) in metric_rows(report) {
                    let rows = match metric {
                        Metric::Read(values) => {
                            let shown = if details { values.len() } else { 1 };
                            values
                                .iter()
                                .take(shown)
                                .flat_map(|value| {
                                    let value = report::terminal_safe(value);
                                    if Span::raw(value.as_str()).width() + LABEL <= width {
                                        vec![value]
                                    } else {
                                        crate::tui::wrap_words(&value, width.saturating_sub(LABEL))
                                    }
                                })
                                .map(Span::raw)
                                .collect()
                        }
                        Metric::Absent => vec![Span::styled("none", theme.style(Token::Muted))],
                        Metric::Unavailable => {
                            vec![Span::styled("unavailable", theme.style(Token::Warning))]
                        }
                    };
                    for (index, value) in rows.into_iter().enumerate() {
                        let label = if index == 0 { label } else { "" };
                        lines.push(Line::from(vec![
                            Span::styled(format!("  {label:<12}"), theme.style(Token::Muted)),
                            value,
                        ]));
                    }
                }
                lines
            };
            // Each reason is why a row above reads "unavailable", so they come
            // before the processes, which take only the rows left. A metric's
            // detail lines (the memory breakdown) give way before a reason.
            let mut reasons = Vec::new();
            for entry in &report.unavailable {
                let reason = format!("unavailable: {}", report::terminal_safe(entry));
                for (index, row) in crate::tui::wrap_words(&reason, width.saturating_sub(4))
                    .into_iter()
                    .enumerate()
                {
                    let indent = if index == 0 { "  " } else { "    " };
                    reasons.push(Line::styled(
                        format!("{indent}{row}"),
                        theme.style(Token::Warning),
                    ));
                }
            }
            let mut metrics = metric_lines(true);
            if lines.len() + metrics.len() + reasons.len() > height {
                metrics = metric_lines(false);
            }
            lines.extend(metrics);
            lines.extend(reasons);
            let room = height.saturating_sub(lines.len());
            if !report.top_processes.is_empty() && room >= 3 {
                lines.push(Line::raw(""));
                lines.push(Line::styled(
                    "  busiest processes",
                    theme.style(Token::AccentSecondary),
                ));
                for process in report.top_processes.iter().take(room - 2) {
                    let row = format!(
                        "  {:>7}  {:>6.1}%  {:>9}  {}",
                        process.pid,
                        process.cpu_percent,
                        report::gb(process.resident_bytes),
                        report::terminal_safe(&process.command)
                    );
                    lines.push(Line::raw(crate::tui::truncate_right(&row, width)));
                }
            }
        }
    }
    if lines.len() > height && height > 0 {
        let hidden = lines.len() - (height - 1);
        lines.truncate(height - 1);
        lines.push(Line::styled(
            format!("  … {hidden} more line(s); a taller window shows them"),
            theme.style(Token::Muted),
        ));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(Block::bordered().title(" Machine ")),
        body,
    );

    let status = age.map_or_else(
        || "Waiting for the first sample. Read-only.".to_string(),
        |age| format!("Sampled {} s ago. Read-only.", age.as_secs()),
    );
    crate::tui::render_footer_rows(
        frame,
        footer,
        theme,
        &[("q", "quit")],
        (&status, Token::Muted),
    );
}

/// One metric as both surfaces state it.
enum Metric {
    /// Read: the lines that state it, the first beside its label.
    Read(Vec<String>),
    /// Read and found not to exist: a desktop has no battery.
    Absent,
    /// Its probe failed; the reason is in [`StatusReport::unavailable`].
    Unavailable,
}

/// Every metric in a fixed order, worded once. `status` prints the ones it
/// read; `--watch` keeps a row for each so none moves between refreshes.
fn metric_rows(report: &StatusReport) -> Vec<(&'static str, Metric)> {
    let read = |line: String| Metric::Read(vec![line]);
    let battery = match &report.battery {
        Some(battery) => read(format!(
            "{}%, {}{}",
            battery.percent,
            battery.state,
            if battery.on_ac_power { ", on AC" } else { "" }
        )),
        // A failed probe and a machine without a battery both leave the field
        // empty. Only the first is a gap, and calling it "none" would assert a
        // hardware fact the probe never established.
        None if report
            .unavailable
            .iter()
            .any(|entry| entry.starts_with("battery:")) =>
        {
            Metric::Unavailable
        }
        None => Metric::Absent,
    };
    vec![
        (
            "uptime",
            report
                .uptime_seconds
                .map_or(Metric::Unavailable, |seconds| read(format_uptime(seconds))),
        ),
        (
            "load",
            match (report.load_average, report.cpu_count) {
                (Some(load), Some(cpus)) => read(format!(
                    "{:.2} {:.2} {:.2}  over {cpus} logical CPUs",
                    load[0], load[1], load[2]
                )),
                _ => Metric::Unavailable,
            },
        ),
        (
            "memory",
            report.memory.map_or(Metric::Unavailable, |memory| {
                Metric::Read(vec![
                    format!(
                        "{} of {} used ({:.0}%) = active + wired + compressed",
                        report::gb(memory.used_bytes),
                        report::gb(memory.total_bytes),
                        memory.used_percent()
                    ),
                    format!(
                        "{} wired, {} compressed, {} inactive (reclaimable), {} free",
                        report::gb(memory.wired_bytes),
                        report::gb(memory.compressed_bytes),
                        report::gb(memory.inactive_bytes),
                        report::gb(memory.free_bytes)
                    ),
                ])
            }),
        ),
        (
            "disk",
            report.disk.map_or(Metric::Unavailable, |disk| {
                read(format!(
                    "{} used, {} available ({:.0}% of used + available)",
                    report::gb(disk.used_bytes),
                    report::gb(disk.available_bytes),
                    disk.used_percent()
                ))
            }),
        ),
        ("battery", battery),
        (
            "thermal",
            report
                .thermal
                .as_ref()
                .map_or(Metric::Unavailable, |thermal| {
                    read(thermal.cpu_speed_limit_percent.map_or_else(
                        || "no limit recorded".to_string(),
                        |limit| format!("CPU limited to {limit}%"),
                    ))
                }),
        ),
        (
            "network",
            report.network.map_or(Metric::Unavailable, |network| {
                read(format!(
                    "{} in, {} out (cumulative since boot)",
                    report::gb(network.received_bytes),
                    report::gb(network.sent_bytes)
                ))
            }),
        ),
    ]
}

pub fn run(ctx: &Ctx, watch: bool) -> Result<std::process::ExitCode> {
    if watch {
        return run_watch(ctx);
    }
    let report = collect();
    if ctx.json {
        let document =
            serde_json::to_string_pretty(&report).context("cannot serialize status report")?;
        report::write_stdout(document.as_bytes())?;
        report::write_stdout(b"\n")?;
    } else {
        report::write_stdout(human_text(&report).as_bytes())?;
    }
    // Partial visibility is disclosed in the document AND in the exit status,
    // matching scan and largest.
    Ok(if report.unavailable.is_empty() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::from(1)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ColorSupport;

    const VM_STAT: &str = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\n\
Pages free:                                   145575.\n\
Pages active:                                1522090.\n\
Pages inactive:                              1505121.\n\
Pages speculative:                             18528.\n\
Pages throttled:                                   0.\n\
Pages wired down:                             423049.\n\
Pages occupied by compressor:                 622372.\n";

    const NETSTAT: &str = "Name       Mtu   Network       Address            Ipkts Ierrs     Ibytes    Opkts Oerrs     Obytes  Coll\n\
lo0        16384 <Link#1>                       3727486     0 5479580870  3727486     0 5479580870     0\n\
lo0        16384 127           localhost        3727486     - 5479580870  3727486     - 5479580870     -\n\
lo0        16384 localhost   ::1                3727486     - 5479580870  3727486     - 5479580870     -\n\
en0        1500  <Link#12>   a4:83:e7:11:22:33     50000     0    1000000    40000     0     900000     0\n";

    #[test]
    fn a_score_never_claims_a_band_over_signals_it_could_not_read() {
        let missing = |names: &[&str]| Health {
            score: 100,
            missing_inputs: names.iter().map(ToString::to_string).collect(),
        };

        assert_eq!(health_headline(&missing(&[])), "health 100/100 (healthy)");
        assert_eq!(
            health_headline(&missing(&["disk"])),
            "health 100/100 (healthy, without disk)",
            "PV status/health-missing"
        );
        let nothing = health_headline(&missing(&["disk", "memory", "load", "thermal"]));
        assert!(
            !nothing.contains("healthy"),
            "PV status/health-missing: {nothing}"
        );
        assert!(nothing.contains("unknown"), "{nothing}");
    }

    #[test]
    fn disk_layout_selection_fails_closed_except_for_absence() {
        assert_eq!(disk_tool(Ok(true)).unwrap(), SystemTool::DataVolume);
        assert_eq!(
            disk_tool(Err(std::io::ErrorKind::NotFound.into())).unwrap(),
            SystemTool::RootFilesystem
        );
        assert!(
            disk_tool(Err(std::io::ErrorKind::PermissionDenied.into())).is_err(),
            "PV status/disk-layout"
        );
        assert!(disk_tool(Ok(false)).is_err(), "PV status/disk-layout");
    }

    #[test]
    fn load_average_fails_closed_on_malformed_input() {
        for value in [
            "",
            "9.88 10.03 7.71",
            "{ 9.88 10.03 }",
            "{ a b c }",
            "{ -1 0 0 }",
            "{ 1 2 3 4 }",
        ] {
            assert!(parse_loadavg(value).is_err(), "accepted {value:?}");
        }
    }

    #[test]
    fn boot_time_fails_closed_when_it_is_in_the_future_or_malformed() {
        let output = "{ sec = 1788197900, usec = 237762 }\n";
        assert!(parse_boottime_seconds(output, 1788197899).is_err());
        assert!(parse_boottime_seconds("{ usec = 1 }", 10).is_err());
        assert!(parse_boottime_seconds("", 10).is_err());
    }

    /// The page size is read from vm_stat's own header. Apple silicon reports
    /// 16384, so a hardcoded 4096 would understate every figure by 4x — this
    /// asserts the declared size is actually used.
    #[test]
    fn vm_stat_uses_the_declared_page_size() {
        let memory = parse_vm_stat(VM_STAT, 68_719_476_736).unwrap();
        assert_eq!(memory.free_bytes, (145_575 + 18_528) * 16_384);
        assert_eq!(memory.active_bytes, 1_522_090 * 16_384);
        assert_eq!(memory.wired_bytes, 423_049 * 16_384);
        assert_eq!(memory.compressed_bytes, 622_372 * 16_384);
        // Used is active + wired + compressed, NOT total - free: inactive is
        // file-backed and reclaimable, and counting it reports a healthy
        // machine at 96% used.
        assert_eq!(memory.used_bytes, (1_522_090 + 423_049 + 622_372) * 16_384);
        assert!(
            memory.used_bytes < 68_719_476_736 - memory.free_bytes,
            "inactive memory must not be counted as used"
        );

        let four_k = VM_STAT.replace("page size of 16384", "page size of 4096");
        let smaller = parse_vm_stat(&four_k, 68_719_476_736).unwrap();
        assert_eq!(
            smaller.active_bytes * 4,
            memory.active_bytes,
            "the header's page size must drive the result"
        );
    }

    #[test]
    fn vm_stat_fails_closed_on_malformed_input() {
        assert!(parse_vm_stat("", 1).is_err());
        assert!(parse_vm_stat("Mach Virtual Memory Statistics:\nPages free: 1.\n", 1).is_err());
        assert!(
            parse_vm_stat(
                "Mach Virtual Memory Statistics: (page size of 0 bytes)\nPages free: 1.\n",
                1
            )
            .is_err()
        );
        // Present header, missing a required counter.
        assert!(
            parse_vm_stat(
                "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free: 1.\n",
                1
            )
            .is_err()
        );
    }

    #[test]
    fn df_fails_closed_on_malformed_input() {
        assert!(parse_df("").is_err());
        assert!(parse_df("Filesystem 1024-blocks\n").is_err());
        assert!(parse_df("Filesystem\n/dev/disk a b\n").is_err());
    }

    #[test]
    fn battery_fails_closed_on_an_impossible_percentage() {
        let output = " -InternalBattery-0\t250%; charged;\n";
        assert!(parse_battery(output).is_err());
    }

    #[test]
    fn thermal_reports_no_recorded_limit_as_nominal() {
        let output = "Note: No thermal warning level has been recorded\n\
Note: No performance warning level has been recorded\n\
Note: No CPU power status has been recorded\n";
        assert_eq!(parse_thermal(output).unwrap().cpu_speed_limit_percent, None);

        // Only the CPU power-status note speaks for `CPU_Speed_Limit`. Output
        // truncated to the thermal-warning line alone establishes nothing about
        // the CPU and must not be scored as a nominal reading.
        assert!(
            parse_thermal("Note: No thermal warning level has been recorded\n").is_err(),
            "the generic note must not stand in for the CPU one"
        );

        let limited = "CPU_Speed_Limit \t= 70\n";
        assert_eq!(
            parse_thermal(limited).unwrap().cpu_speed_limit_percent,
            Some(70)
        );
        assert!(parse_thermal("CPU_Speed_Limit = 400\n").is_err());

        // Nominal must be an answer, not an absence: empty or reshaped output
        // would otherwise be scored as a healthy thermal reading.
        assert!(parse_thermal("").is_err());
        assert!(parse_thermal("some future pmset format\n").is_err());
    }

    /// Each interface appears once per configured address; only the link rows
    /// may be summed or `lo0` is counted three times.
    ///
    /// The fixture deliberately mixes a link row with no hardware address
    /// (`lo0`, 10 fields) and one with a MAC (`en0`, 11 fields), because that
    /// width difference is what makes left-indexed columns read `Ipkts` as
    /// `Ibytes`. Without both shapes present this test passes over the bug.
    #[test]
    fn netstat_counts_each_interface_once_across_both_link_row_widths() {
        assert_eq!(
            NETSTAT
                .lines()
                .filter(|line| line.contains("<Link#"))
                .map(|line| line.split_whitespace().count())
                .collect::<Vec<_>>(),
            vec![10, 11],
            "the fixture must exercise both link-row widths"
        );
        let network = parse_netstat(NETSTAT).unwrap();
        assert_eq!(network.received_bytes, 5_479_580_870 + 1_000_000);
        assert_eq!(network.sent_bytes, 5_479_580_870 + 900_000);
    }

    #[test]
    fn netstat_fails_closed_without_link_rows() {
        assert!(parse_netstat("Name Mtu Network Address\n").is_err());
        assert!(parse_netstat("").is_err());
    }

    #[test]
    fn netstat_refuses_a_link_row_of_undocumented_width_instead_of_skipping_it() {
        let truncated = format!("{NETSTAT}lo1   16384 <Link#9>   12 0 3400\n");
        let error = parse_netstat(&truncated).unwrap_err();
        assert!(
            error.to_string().contains("unexpected netstat link row"),
            "a sum dropped a row it could not read: {error:#}"
        );
    }

    #[test]
    fn processes_fail_closed_when_none_parse() {
        assert!(parse_processes("PID %CPU RSS COMM\n", 5).is_err());
        assert!(parse_processes("", 5).is_err());
    }

    const GIB: u64 = 1 << 30;

    /// A laptop read in full, with `processes` busy processes.
    fn sample(processes: usize) -> StatusReport {
        let mut report = StatusReport {
            uptime_seconds: Some(93_784),
            load_average: Some([2.5, 2.0, 1.5]),
            cpu_count: Some(10),
            memory: Some(Memory {
                total_bytes: 32 * GIB,
                free_bytes: 2 * GIB,
                active_bytes: 10 * GIB,
                inactive_bytes: 8 * GIB,
                wired_bytes: 4 * GIB,
                compressed_bytes: 2 * GIB,
                used_bytes: 16 * GIB,
            }),
            disk: Some(Disk {
                total_bytes: 994 * GIB,
                used_bytes: 500 * GIB,
                available_bytes: 400 * GIB,
            }),
            battery: Some(Battery {
                percent: 80,
                state: "charging".into(),
                on_ac_power: true,
            }),
            thermal: Some(Thermal {
                cpu_speed_limit_percent: None,
            }),
            network: Some(Network {
                received_bytes: 3 * GIB,
                sent_bytes: GIB,
            }),
            top_processes: (0..processes)
                .map(|index| Process {
                    pid: 100 + index as u32,
                    cpu_percent: 50.0 - index as f64,
                    resident_bytes: GIB,
                    command: format!("process-{index}"),
                })
                .collect(),
            health: Health {
                score: 0,
                missing_inputs: Vec::new(),
            },
            unavailable: Vec::new(),
        };
        report.health = health(&report);
        report
    }

    fn draw(
        report: Option<&StatusReport>,
        theme: Theme,
        age: Option<Duration>,
        width: u16,
        height: u16,
    ) -> ratatui::buffer::Buffer {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render_watch(frame, report, theme, age))
            .unwrap();
        terminal.backend().buffer().clone()
    }

    fn rows(buffer: &ratatui::buffer::Buffer) -> Vec<String> {
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    /// The cell where `text` starts, searching row by row.
    fn find(buffer: &ratatui::buffer::Buffer, text: &str) -> Option<(u16, u16)> {
        rows(buffer).iter().enumerate().find_map(|(y, row)| {
            row.find(text).map(|byte| {
                let x = row[..byte].chars().count();
                (x as u16, y as u16)
            })
        })
    }

    #[test]
    fn unread_metrics_are_listed_before_processes_take_what_is_left() {
        let mut report = sample(10);
        report.battery = None;
        report.thermal = None;
        report.network = None;
        report.unavailable = vec![
            "battery: pmset exited with status 1".into(),
            "thermal: pmset exited with status 1".into(),
            "network: netstat exited with status 1".into(),
        ];
        report.health = health(&report);
        let buffer = draw(
            Some(&report),
            Theme::new(ColorSupport::None),
            Some(Duration::from_secs(1)),
            64,
            18,
        );
        let screen = rows(&buffer).join("\n");
        for entry in &report.unavailable {
            assert!(
                screen.contains(entry.as_str()),
                "the reason {entry:?} fell off the screen:\n{screen}"
            );
        }
        assert!(
            !screen.contains("more line(s)"),
            "nothing is hidden when only detail gave way:\n{screen}"
        );

        // Taller, the processes take exactly the rows left, under the reasons.
        let buffer = draw(
            Some(&report),
            Theme::new(ColorSupport::None),
            Some(Duration::from_secs(1)),
            64,
            24,
        );
        let screen = rows(&buffer).join("\n");
        let shown = (0..10)
            .filter(|index| screen.contains(&format!("process-{index}")))
            .count();
        assert!(
            (1..10).contains(&shown),
            "{shown} of 10 processes shown; they must fill the rows left, not more:\n{screen}"
        );
        let last_body_row = &rows(&buffer)[24 - 4];
        assert!(
            last_body_row.contains(&format!("process-{}", shown - 1)),
            "the last process sits on the last row of the box:\n{screen}"
        );
        let reasons = find(&buffer, "unavailable: network").unwrap().1;
        let processes = find(&buffer, "busiest processes").unwrap().1;
        assert!(
            reasons < processes,
            "reasons come before processes:\n{screen}"
        );
    }

    #[test]
    fn a_box_too_short_says_how_many_lines_it_hides() {
        let mut report = sample(8);
        report.unavailable = (0..12)
            .map(|index| format!("probe-{index}: exited with status 1"))
            .collect();
        let buffer = draw(
            Some(&report),
            Theme::new(ColorSupport::None),
            Some(Duration::ZERO),
            64,
            18,
        );
        let screen = rows(&buffer);
        assert!(
            screen[18 - 4].contains("more line(s); a taller window shows them"),
            "the last row of the box names what it hides:\n{}",
            screen.join("\n")
        );
    }

    #[test]
    fn the_headline_carries_the_colour_of_its_band() {
        let theme = Theme::new(ColorSupport::Named);
        let mut unknown = sample(0);
        unknown.health = Health {
            score: 100,
            missing_inputs: ["disk", "memory", "load", "thermal"]
                .map(String::from)
                .to_vec(),
        };
        for (score, token) in [
            (95, Token::Success),
            (75, Token::Warning),
            (40, Token::Critical),
        ] {
            let mut report = sample(0);
            report.health.score = score;
            let buffer = draw(Some(&report), theme, Some(Duration::ZERO), 100, 30);
            let (x, y) = find(&buffer, "health ").expect("headline drawn");
            assert_eq!(
                buffer[(x, y)].style().fg,
                theme.bold(token).fg,
                "score {score}"
            );
            assert!(
                buffer[(x, y)]
                    .style()
                    .add_modifier
                    .contains(ratatui::style::Modifier::BOLD),
                "score {score}"
            );
        }
        let buffer = draw(Some(&unknown), theme, Some(Duration::ZERO), 100, 30);
        let (x, y) = find(&buffer, "health unknown").unwrap();
        assert_eq!(buffer[(x, y)].style().fg, theme.bold(Token::Warning).fg);
    }

    #[test]
    fn the_footer_says_how_old_the_sample_is() {
        let theme = Theme::new(ColorSupport::None);
        let report = sample(0);
        let screen = rows(&draw(
            Some(&report),
            theme,
            Some(Duration::from_millis(3_400)),
            64,
            18,
        ))
        .join("\n");
        assert!(screen.contains("Sampled 3 s ago."), "{screen}");
        assert!(
            screen
                .lines()
                .nth(16)
                .is_some_and(|row| row.contains("q quit")),
            "{screen}"
        );
        assert!(!screen.contains("Refreshing every"), "{screen}");
        let waiting = rows(&draw(None, theme, None, 64, 18)).join("\n");
        assert!(waiting.contains("sampling…"), "{waiting}");
        assert!(!waiting.contains("Sampled"), "{waiting}");
    }

    #[test]
    fn an_unread_metric_reads_as_a_warning_not_a_failure() {
        let theme = Theme::new(ColorSupport::Named);
        let mut report = sample(0);
        report.thermal = None;
        report.unavailable = vec!["thermal: pmset exited with status 1".into()];
        report.health = health(&report);
        let buffer = draw(Some(&report), theme, Some(Duration::ZERO), 100, 30);
        for text in ["unavailable", "unavailable: thermal"] {
            let (x, y) = find(&buffer, text).unwrap();
            assert_eq!(
                buffer[(x, y)].style().fg,
                theme.style(Token::Warning).fg,
                "{text:?}"
            );
        }
    }

    #[test]
    fn the_report_and_the_watch_state_each_metric_in_the_same_words() {
        let report = sample(0);
        // `colored` styles only the headings and the headline, never a metric
        // line, so no escape sequence can sit inside the text compared here.
        let cli = human_text(&report);
        let screen = rows(&draw(
            Some(&report),
            Theme::new(ColorSupport::None),
            Some(Duration::ZERO),
            120,
            40,
        ))
        .join("\n");
        let mut compared = 0;
        for (label, metric) in metric_rows(&report) {
            let Metric::Read(lines) = metric else {
                continue;
            };
            assert!(
                cli.contains(&format!("  {label:<16}{}", lines[0])),
                "the report does not say {label} as {:?}:\n{cli}",
                lines[0]
            );
            for line in &lines {
                assert!(
                    screen.contains(line.as_str()),
                    "the watch does not say {label} as {line:?}:\n{screen}"
                );
            }
            compared += 1;
        }
        assert_eq!(compared, 7, "every metric of a full sample is compared");
        // The watch once wrote "X of TOTAL (P%)" with P a share of used +
        // available, not of TOTAL.
        assert!(
            screen.contains("500.0 GB used, 400.0 GB available"),
            "{screen}"
        );
    }
}
