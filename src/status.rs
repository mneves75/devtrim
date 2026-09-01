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

use std::process::{Command, Output};

use anyhow::{Context, Result};
use colored::Colorize;

use crate::report;
use crate::safety::Ctx;

/// Runs a fixed program with fixed arguments. No shell, ever.
fn capture(program: &str, args: &[&str]) -> Result<String> {
    let label = format!("`{program} {}`", args.join(" "));
    let output: Output = Command::new(program)
        .args(args)
        .output()
        .with_context(|| format!("cannot run {label}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        if detail.is_empty() {
            anyhow::bail!("{label} failed with {}", output.status);
        }
        anyhow::bail!("{label} failed with {}: {detail}", output.status);
    }
    String::from_utf8(output.stdout).with_context(|| format!("{label} returned non-UTF-8 output"))
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

    let bytes = |count: u64| count.saturating_mul(page_size);
    let free = bytes(pages("Pages free")?.saturating_add(pages("Pages speculative")?));
    let active = bytes(pages("Pages active")?);
    let inactive = bytes(pages("Pages inactive")?);
    let wired = bytes(pages("Pages wired down")?);
    // Absent only on a system with no memory compressor, where zero is the
    // correct value rather than a stand-in for an unread one.
    let compressed = output
        .lines()
        .find(|line| line.starts_with("Pages occupied by compressor"))
        .map_or(Ok(0), |_| pages("Pages occupied by compressor"))
        .map(bytes)?;
    Ok(Memory {
        total_bytes,
        free_bytes: free,
        active_bytes: active,
        inactive_bytes: inactive,
        wired_bytes: wired,
        compressed_bytes: compressed,
        used_bytes: active
            .saturating_add(wired)
            .saturating_add(compressed)
            .min(total_bytes),
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
        // No recorded limit is the nominal state, not missing data.
        return Ok(Thermal {
            cpu_speed_limit_percent: None,
        });
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
        // always `Ipkts Ierrs Ibytes Opkts Oerrs Obytes Coll`.
        if fields.len() < 9 {
            continue;
        }
        let (Some(ibytes), Some(obytes)) = (
            fields.get(fields.len().saturating_sub(5)),
            fields.get(fields.len().saturating_sub(2)),
        ) else {
            continue;
        };
        let (Ok(ibytes), Ok(obytes)) = (ibytes.parse::<u64>(), obytes.parse::<u64>()) else {
            continue;
        };
        received = received.saturating_add(ibytes);
        sent = sent.saturating_add(obytes);
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
            let used = (memory.used_bytes as f64) * 100.0 / (memory.total_bytes as f64);
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
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .context("system clock is before the Unix epoch")
}

fn collect() -> StatusReport {
    let mut unavailable = Vec::new();
    let mut record = |metric: &str, error: &anyhow::Error| {
        unavailable.push(format!("{metric}: {error:#}"));
    };

    let uptime_seconds = match capture("sysctl", &["-n", "kern.boottime"])
        .and_then(|output| parse_boottime_seconds(&output, now_unix()?))
    {
        Ok(value) => Some(value),
        Err(error) => {
            record("uptime", &error);
            None
        }
    };

    let load_average =
        match capture("sysctl", &["-n", "vm.loadavg"]).and_then(|output| parse_loadavg(&output)) {
            Ok(value) => Some(value),
            Err(error) => {
                record("load", &error);
                None
            }
        };

    let cpu_count = match capture("sysctl", &["-n", "hw.logicalcpu"]).and_then(|output| {
        let trimmed = output.trim().to_string();
        trimmed
            .parse::<u32>()
            .with_context(|| format!("invalid hw.logicalcpu `{trimmed}`"))
    }) {
        Ok(value) => Some(value),
        Err(error) => {
            record("cpu", &error);
            None
        }
    };

    let memory = match capture("sysctl", &["-n", "hw.memsize"])
        .and_then(|output| {
            let trimmed = output.trim().to_string();
            trimmed
                .parse::<u64>()
                .with_context(|| format!("invalid hw.memsize `{trimmed}`"))
        })
        .and_then(|total| parse_vm_stat(&capture("vm_stat", &[])?, total))
    {
        Ok(value) => Some(value),
        Err(error) => {
            record("memory", &error);
            None
        }
    };

    let disk = match capture("df", &["-k", "/"]).and_then(|output| parse_df(&output)) {
        Ok(value) => Some(value),
        Err(error) => {
            record("disk", &error);
            None
        }
    };

    let battery = match capture("pmset", &["-g", "batt"]).and_then(|output| parse_battery(&output))
    {
        Ok(value) => value,
        Err(error) => {
            record("battery", &error);
            None
        }
    };

    let thermal = match capture("pmset", &["-g", "therm"]).and_then(|output| parse_thermal(&output))
    {
        Ok(value) => Some(value),
        Err(error) => {
            record("thermal", &error);
            None
        }
    };

    let network = match capture("netstat", &["-ib"]).and_then(|output| parse_netstat(&output)) {
        Ok(value) => Some(value),
        Err(error) => {
            record("network", &error);
            None
        }
    };

    let top_processes = match capture("ps", &["-Aco", "pid,pcpu,rss,comm", "-r"])
        .and_then(|output| parse_processes(&output, 8))
    {
        Ok(value) => value,
        Err(error) => {
            record("processes", &error);
            Vec::new()
        }
    };

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

fn print_human(report: &StatusReport) -> Result<()> {
    use std::fmt::Write as _;

    let mut out = String::new();
    // `colored` disables itself when NO_COLOR is set, which is the same
    // baseline the TUI theme enforces through its own tokens.
    let heading = |out: &mut String, text: &str| {
        let _ = writeln!(out, "{}", text.bold());
    };

    heading(&mut out, "machine");
    if let Some(uptime) = report.uptime_seconds {
        let _ = writeln!(out, "  uptime          {}", format_uptime(uptime));
    }
    if let (Some(load), Some(cpus)) = (report.load_average, report.cpu_count) {
        let _ = writeln!(
            out,
            "  load            {:.2} {:.2} {:.2}  over {cpus} logical CPUs",
            load[0], load[1], load[2]
        );
    }
    if let Some(memory) = report.memory {
        let percent = if memory.total_bytes == 0 {
            0.0
        } else {
            (memory.used_bytes as f64) * 100.0 / (memory.total_bytes as f64)
        };
        let _ = writeln!(
            out,
            "  memory          {} of {} used ({percent:.0}%) = active + wired + compressed",
            report::gb(memory.used_bytes),
            report::gb(memory.total_bytes)
        );
        let _ = writeln!(
            out,
            "                  {} wired, {} compressed, {} inactive (reclaimable), {} free",
            report::gb(memory.wired_bytes),
            report::gb(memory.compressed_bytes),
            report::gb(memory.inactive_bytes),
            report::gb(memory.free_bytes)
        );
    }
    if let Some(disk) = report.disk {
        let _ = writeln!(
            out,
            "  disk /          {} of {} used ({:.0}%), {} available",
            report::gb(disk.used_bytes),
            report::gb(disk.total_bytes),
            disk.used_percent(),
            report::gb(disk.available_bytes)
        );
    }
    if let Some(battery) = &report.battery {
        let _ = writeln!(
            out,
            "  battery         {}%, {}{}",
            battery.percent,
            report::terminal_safe(&battery.state),
            if battery.on_ac_power { ", on AC" } else { "" }
        );
    }
    if let Some(thermal) = &report.thermal {
        let _ = writeln!(
            out,
            "  thermal         {}",
            thermal.cpu_speed_limit_percent.map_or_else(
                || "no limit recorded".to_string(),
                |limit| format!("CPU limited to {limit}%")
            )
        );
    }
    if let Some(network) = report.network {
        let _ = writeln!(
            out,
            "  network         {} in, {} out (cumulative since boot)",
            report::gb(network.received_bytes),
            report::gb(network.sent_bytes)
        );
    }

    if !report.top_processes.is_empty() {
        let _ = writeln!(out);
        heading(&mut out, "busiest processes");
        for process in &report.top_processes {
            let _ = writeln!(
                out,
                "  {:>7}  {:>6.1}%  {:>9}  {}",
                process.pid,
                process.cpu_percent,
                report::gb(process.resident_bytes),
                report::terminal_safe(&process.command)
            );
        }
    }

    let _ = writeln!(out);
    let headline = format!("health {}/100", report.health.score);
    let headline = match report.health.score {
        90..=100 => headline.green(),
        70..=89 => headline.yellow(),
        _ => headline.red(),
    };
    let _ = writeln!(out, "{}", headline.bold());
    if !report.health.missing_inputs.is_empty() {
        let _ = writeln!(
            out,
            "  scored without: {}",
            report.health.missing_inputs.join(", ")
        );
    }
    for entry in &report.unavailable {
        let _ = writeln!(out, "  unavailable: {}", report::terminal_safe(entry));
    }
    report::write_stdout(out.as_bytes())?;
    Ok(())
}

pub fn run(ctx: &Ctx) -> Result<std::process::ExitCode> {
    let report = collect();
    if ctx.json {
        let document =
            serde_json::to_string_pretty(&report).context("cannot serialize status report")?;
        report::write_stdout(document.as_bytes())?;
        report::write_stdout(b"\n")?;
    } else {
        print_human(&report)?;
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
    fn parses_load_average() {
        assert_eq!(
            parse_loadavg("{ 9.88 10.03 7.71 }\n").unwrap(),
            [9.88, 10.03, 7.71]
        );
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
    fn parses_boot_time_into_uptime() {
        let output = "{ sec = 1788197900, usec = 237762 } Mon Aug 31 14:38:20 2026\n";
        assert_eq!(
            parse_boottime_seconds(output, 1788197900 + 3600).unwrap(),
            3600
        );
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
    fn parses_df_rows() {
        let output = "Filesystem     1024-blocks      Used Available Capacity iused     ifree %iused  Mounted on\n\
/dev/disk3s1s1   971298980  12353644  76941040    14%  482570 769410400    0%   /\n";
        let disk = parse_df(output).unwrap();
        assert_eq!(disk.used_bytes, 12_353_644 * 1024);
        assert_eq!(disk.available_bytes, 76_941_040 * 1024);
        assert_eq!(disk.total_bytes, disk.used_bytes + disk.available_bytes);
    }

    #[test]
    fn df_fails_closed_on_malformed_input() {
        assert!(parse_df("").is_err());
        assert!(parse_df("Filesystem 1024-blocks\n").is_err());
        assert!(parse_df("Filesystem\n/dev/disk a b\n").is_err());
    }

    #[test]
    fn parses_battery_and_tolerates_a_machine_without_one() {
        let output = "Now drawing from 'AC Power'\n -InternalBattery-0 (id=36896867)\t100%; charged; 0:00 remaining present: true\n";
        let battery = parse_battery(output).unwrap().unwrap();
        assert_eq!(battery.percent, 100);
        assert!(battery.on_ac_power);
        assert_eq!(battery.state, "charged");

        assert_eq!(
            parse_battery("Now drawing from 'AC Power'\n").unwrap(),
            None
        );
    }

    #[test]
    fn battery_fails_closed_on_an_impossible_percentage() {
        let output = " -InternalBattery-0\t250%; charged;\n";
        assert!(parse_battery(output).is_err());
    }

    #[test]
    fn thermal_reports_no_recorded_limit_as_nominal() {
        let output = "Note: No thermal warning level has been recorded\n";
        assert_eq!(parse_thermal(output).unwrap().cpu_speed_limit_percent, None);

        let limited = "CPU_Speed_Limit \t= 70\n";
        assert_eq!(
            parse_thermal(limited).unwrap().cpu_speed_limit_percent,
            Some(70)
        );
        assert!(parse_thermal("CPU_Speed_Limit = 400\n").is_err());
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
    fn parses_processes_and_respects_the_limit() {
        let output = "  PID  %CPU    RSS COMM\n\
  621  85.8 261728 WindowServer\n\
40635  34.6 9776560 OrbStack Helper\n\
 6255  23.8 548352 stable\n";
        let processes = parse_processes(output, 2).unwrap();
        assert_eq!(processes.len(), 2);
        assert_eq!(processes[0].pid, 621);
        assert_eq!(processes[0].resident_bytes, 261_728 * 1024);
        assert_eq!(processes[1].command, "OrbStack Helper");
    }

    #[test]
    fn processes_fail_closed_when_none_parse() {
        assert!(parse_processes("PID %CPU RSS COMM\n", 5).is_err());
        assert!(parse_processes("", 5).is_err());
    }

    fn empty_report() -> StatusReport {
        StatusReport {
            uptime_seconds: None,
            load_average: None,
            cpu_count: None,
            memory: None,
            disk: None,
            battery: None,
            thermal: None,
            network: None,
            top_processes: Vec::new(),
            health: Health {
                score: 0,
                missing_inputs: Vec::new(),
            },
            unavailable: Vec::new(),
        }
    }

    /// A score computed over nothing must say so rather than present a perfect
    /// machine. This is the assertion that stops the dashboard from lying.
    #[test]
    fn health_names_every_missing_input_instead_of_scoring_over_gaps() {
        let health = health(&empty_report());
        assert_eq!(health.score, 100);
        let mut missing = health.missing_inputs;
        missing.sort();
        assert_eq!(missing, vec!["disk", "load", "memory", "thermal"]);
    }

    #[test]
    fn health_deducts_for_real_pressure() {
        let mut report = empty_report();
        report.disk = Some(Disk {
            total_bytes: 100,
            used_bytes: 96,
            available_bytes: 4,
        });
        report.memory = Some(Memory {
            total_bytes: 100,
            free_bytes: 2,
            active_bytes: 50,
            inactive_bytes: 0,
            wired_bytes: 28,
            compressed_bytes: 20,
            used_bytes: 98,
        });
        report.load_average = Some([20.0, 10.0, 5.0]);
        report.cpu_count = Some(8);
        report.thermal = Some(Thermal {
            cpu_speed_limit_percent: Some(50),
        });
        let health = health(&report);
        assert!(health.missing_inputs.is_empty());
        assert_eq!(
            health.score, 0,
            "every pressure signal firing floors the score"
        );
    }

    #[test]
    fn uptime_formatting_covers_each_magnitude() {
        assert_eq!(format_uptime(90), "1m");
        assert_eq!(format_uptime(3_700), "1h 1m");
        assert_eq!(format_uptime(90_000), "1d 1h 0m");
    }
}
