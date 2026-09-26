//! Findings model + human/JSON rendering.

use colored::Colorize;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::safety::FileIdentity;

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Action {
    Trash,
    Shred,
    Command { program: String, args: Vec<String> },
    Info,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CommandAuthority {
    DockerImagePrune { host: String },
    DockerBuilderPrune { host: String },
    DeleteSimulator { udid: String },
    Maintenance(MaintenanceTask),
}

/// Closed set of maintenance tasks. Every one is a fixed program with fixed
/// arguments and no caller-supplied data at all, which is why none of them can
/// carry an unvalidated value into a process.
///
/// A DNS entry is deliberately absent. On modern macOS the resolver cache lives
/// in `mDNSResponder`, and `dscacheutil -flushcache` only clears the
/// directory-service cache — it would report success while leaving the cache it
/// advertised intact. Doing it properly needs to signal a privileged service,
/// which this non-root catalog cannot do, so the task is omitted rather than
/// offered as something it is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MaintenanceTask {
    QuickLookCache,
    FontCaches,
    LaunchServices,
}

impl MaintenanceTask {
    pub(crate) const ALL: &'static [Self] =
        &[Self::QuickLookCache, Self::FontCaches, Self::LaunchServices];

    /// Stable CLI name, so a task can be selected without depending on its
    /// human label.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::QuickLookCache => "quicklook",
            Self::FontCaches => "fonts",
            Self::LaunchServices => "launch-services",
        }
    }

    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|task| task.name() == name)
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::QuickLookCache => "QuickLook thumbnail cache",
            Self::FontCaches => "user font caches",
            Self::LaunchServices => "Launch Services database",
        }
    }

    pub(crate) fn note(self) -> &'static str {
        match self {
            Self::QuickLookCache => {
                "regenerates on demand; frees thumbnail storage of an unknown size"
            }
            Self::FontCaches => {
                "regenerates on demand; running apps may need a restart to pick up fonts"
            }
            Self::LaunchServices => {
                "rebuilds the Open With database; takes a while and resets custom app associations"
            }
        }
    }

    pub(crate) fn danger(self) -> u8 {
        match self {
            Self::QuickLookCache => 2,
            Self::FontCaches => 3,
            Self::LaunchServices => 4,
        }
    }
}

impl CommandAuthority {
    pub(crate) fn parts(&self) -> (&'static str, Vec<String>) {
        match self {
            Self::DockerImagePrune { host } => (
                "docker",
                ["--host", host, "image", "prune", "-a", "-f"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
            ),
            Self::DockerBuilderPrune { host } => (
                "docker",
                ["--host", host, "builder", "prune", "-a", "-f"]
                    .into_iter()
                    .map(String::from)
                    .collect(),
            ),
            Self::DeleteSimulator { udid } => (
                "xcrun",
                ["simctl", "delete", udid]
                    .into_iter()
                    .map(String::from)
                    .collect(),
            ),
            Self::Maintenance(task) => {
                let (program, args): (&'static str, &[&str]) = match task {
                    MaintenanceTask::QuickLookCache => ("qlmanage", &["-r", "cache"]),
                    MaintenanceTask::FontCaches => ("atsutil", &["databases", "-removeUser"]),
                    MaintenanceTask::LaunchServices => (
                        "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
                        &["-kill", "-r", "-domain", "local", "-domain", "user"],
                    ),
                };
                (program, args.iter().map(|arg| (*arg).to_string()).collect())
            }
        }
    }

    pub(crate) fn action(&self) -> Action {
        let (program, args) = self.parts();
        Action::Command {
            program: program.into(),
            args,
        }
    }

    pub(crate) fn docker_host(&self) -> Option<&str> {
        match self {
            Self::DockerImagePrune { host } | Self::DockerBuilderPrune { host } => Some(host),
            Self::DeleteSimulator { .. } | Self::Maintenance(_) => None,
        }
    }

    pub(crate) fn simulator_udid(&self) -> Option<&str> {
        match self {
            Self::DeleteSimulator { udid } => Some(udid),
            Self::DockerImagePrune { .. }
            | Self::DockerBuilderPrune { .. }
            | Self::Maintenance(_) => None,
        }
    }

    pub(crate) fn maintenance_task(&self) -> Option<MaintenanceTask> {
        match self {
            Self::Maintenance(task) => Some(*task),
            Self::DockerImagePrune { .. }
            | Self::DockerBuilderPrune { .. }
            | Self::DeleteSimulator { .. } => None,
        }
    }
}

impl Action {
    #[cfg(test)]
    pub fn command(program: &str, args: &[&str]) -> Self {
        Self::Command {
            program: program.into(),
            args: args.iter().map(|arg| (*arg).into()).collect(),
        }
    }

    pub fn is_actionable(&self) -> bool {
        matches!(self, Self::Trash | Self::Shred | Self::Command { .. })
    }

    fn display(&self) -> String {
        match self {
            Self::Trash => "move to Trash".into(),
            Self::Shred => "permanently delete".into(),
            Self::Command { program, args } => format!("run `{program} {}`", args.join(" ")),
            Self::Info => "information only".into(),
            Self::None => "excluded".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetAuthority {
    Standard,
    NpmCache,
    BrewCache,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct Finding {
    pub label: String,
    pub path: Option<String>,
    /// Estimated logical bytes by default; APFS clones and sparse files can
    /// differ on disk. A finding whose subject is inherently sparse may report
    /// allocated bytes instead, and must disclose that basis in its `note`.
    pub size_bytes: u64,
    pub note: String,
    /// 1-10
    pub danger: u8,
    pub action: Action,
    /// Repository that owns the target, for grouping a plan by project.
    /// Display only: never parsed back into deletion authority.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip)]
    target: Option<PathBuf>,
    #[serde(skip)]
    identity: Option<FileIdentity>,
    #[serde(skip)]
    authority: TargetAuthority,
    #[serde(skip)]
    command_authority: Option<CommandAuthority>,
    #[serde(skip)]
    scan_error: Option<String>,
}

impl Finding {
    pub fn new(
        label: impl Into<String>,
        path: Option<PathBuf>,
        size_bytes: u64,
        note: impl Into<String>,
        danger: u8,
        action: Action,
    ) -> Self {
        let display_path = path.as_ref().map(|value| value.display().to_string());
        let identity = path.as_ref().and_then(|target| {
            std::fs::symlink_metadata(target)
                .ok()
                .map(|metadata| FileIdentity::from_std_metadata(&metadata))
        });
        Self {
            label: label.into(),
            path: display_path,
            size_bytes,
            note: note.into(),
            danger,
            action,
            project: None,
            target: path,
            identity,
            authority: TargetAuthority::Standard,
            command_authority: None,
            scan_error: None,
        }
    }

    pub(crate) fn with_scan_error(mut self, error: String) -> Self {
        self.scan_error = Some(error);
        self
    }

    pub(crate) fn scan_error(&self) -> Option<&str> {
        self.scan_error.as_deref()
    }

    pub(crate) fn command(
        label: impl Into<String>,
        size_bytes: u64,
        note: impl Into<String>,
        danger: u8,
        authority: CommandAuthority,
    ) -> Self {
        let mut finding = Self::new(label, None, size_bytes, note, danger, authority.action());
        finding.command_authority = Some(authority);
        finding
    }

    pub(crate) fn target(&self) -> Option<&Path> {
        self.target.as_deref()
    }

    pub(crate) fn identity(&self) -> Option<FileIdentity> {
        self.identity
    }

    pub(crate) fn with_project(mut self, project: &Path) -> Self {
        self.project = Some(project.display().to_string());
        self
    }

    pub(crate) fn with_authority(mut self, authority: TargetAuthority) -> Self {
        self.authority = authority;
        self
    }

    pub(crate) fn authority(&self) -> TargetAuthority {
        self.authority
    }

    pub(crate) fn command_authority(&self) -> Option<&CommandAuthority> {
        self.command_authority.as_ref()
    }
}

pub fn terminal_safe(value: &str) -> String {
    value.chars().flat_map(char::escape_debug).collect()
}

/// Escapes what could drive or reorder the terminal while keeping line breaks,
/// quotes and backslashes literal, for multi-line text such as a parser error
/// that interpolates an argument from argv.
pub fn terminal_safe_text(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| {
            let keep = matches!(character, '\n' | '\'' | '"' | '\\')
                || character.escape_debug().nth(1).is_none();
            let escaped: Vec<char> = if keep {
                vec![character]
            } else {
                character.escape_debug().collect()
            };
            escaped
        })
        .collect()
}

#[derive(Debug, serde::Serialize)]
pub struct Summary {
    pub op: String,
    pub items_touched: usize,
    pub bytes_freed_estimate: u64,
    /// The part of `bytes_freed_estimate` that was moved to Trash. It stays on
    /// the same volume until the Trash is emptied, so it is not free space yet.
    pub bytes_trashed_estimate: u64,
    pub notes: Vec<String>,
}

/// What an apply did, in words that never call Trash moves freed space:
/// `op: N item(s), ~X reclaimed estimate` for permanent work, and the Trash
/// part named as such with the command that actually frees it.
pub fn summary_headline(summary: &Summary) -> String {
    format!("{}: {}", summary.op, summary_counts(summary))
}

pub fn summary_counts(summary: &Summary) -> String {
    let trashed = summary
        .bytes_trashed_estimate
        .min(summary.bytes_freed_estimate);
    let reclaimed = summary.bytes_freed_estimate - trashed;
    let items = format!("{} item(s)", summary.items_touched);
    let freed_later = "freed once the Trash is emptied (devtrim trash-empty)";
    match (reclaimed, trashed) {
        (_, 0) => format!("{items}, ~{} reclaimed estimate", gb(reclaimed)),
        (0, _) => format!(
            "{items}, ~{} moved to Trash; it is {freed_later}",
            gb(trashed)
        ),
        _ => format!(
            "{items}, ~{} reclaimed estimate and ~{} moved to Trash; the Trash part is {freed_later}",
            gb(reclaimed),
            gb(trashed)
        ),
    }
}

#[derive(serde::Serialize)]
struct Response<'a> {
    operation: &'a str,
    applied: bool,
    findings: &'a [Finding],
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<&'a Summary>,
    errors: &'a [String],
}

pub fn gb(bytes: u64) -> String {
    let size = bytes as f64;
    if size >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", size / 1024.0 / 1024.0 / 1024.0)
    } else if size >= 1024.0 * 1024.0 {
        format!("{:.0} MB", size / 1024.0 / 1024.0)
    } else if size >= 1024.0 {
        format!("{:.0} KB", size / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn danger_tag(danger: u8) -> colored::ColoredString {
    match danger {
        0..=2 => format!("danger:{danger}").green(),
        3..=5 => format!("danger:{danger}").yellow(),
        6..=8 => format!("danger:{danger}").truecolor(255, 140, 0),
        _ => format!("danger:{danger}").red().bold(),
    }
}

pub fn effective_actions(findings: &mut [Finding], shred: bool) {
    if !shred {
        return;
    }
    for finding in findings {
        if finding.action == Action::Trash {
            finding.action = Action::Shred;
            finding.danger = finding.danger.max(9);
        }
    }
}

pub fn actionable_bytes(findings: &[Finding]) -> u64 {
    findings
        .iter()
        .filter(|finding| finding.action.is_actionable())
        .fold(0, |total, finding| total.saturating_add(finding.size_bytes))
}

pub(crate) fn human_action_display(action: &Action) -> String {
    terminal_safe(&action.display())
}

pub fn print_human(findings: &[Finding]) -> std::io::Result<()> {
    write_stdout(human_text(findings).as_bytes())
}

/// Consecutive findings that share a project, with their combined size.
fn project_runs(findings: &[Finding]) -> Vec<(Option<&str>, std::ops::Range<usize>, u64)> {
    let mut runs: Vec<(Option<&str>, std::ops::Range<usize>, u64)> = Vec::new();
    for (index, finding) in findings.iter().enumerate() {
        let project = finding.project.as_deref();
        match runs.last_mut() {
            Some((current, range, total)) if *current == project => {
                range.end = index + 1;
                *total = total.saturating_add(finding.size_bytes);
            }
            _ => runs.push((project, index..index + 1, finding.size_bytes)),
        }
    }
    runs
}

/// A category listing this long or shorter is shown whole in a scan.
const SCAN_FULL_LISTING: usize = 8;
/// How many of a longer category's largest findings a scan lists.
const SCAN_LARGEST: usize = 5;

/// One category's contiguous slice of a scan's findings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanSection {
    pub category: &'static str,
    pub range: std::ops::Range<usize>,
}

/// The scan report: one line per category with its size and the command that
/// acts on it, largest first, then each category's largest findings.
pub fn print_scan_human(
    findings: &[Finding],
    sections: &[ScanSection],
    all: bool,
) -> std::io::Result<()> {
    write_stdout(scan_text(findings, sections, all).as_bytes())
}

fn scan_text(findings: &[Finding], sections: &[ScanSection], all: bool) -> String {
    let mut ordered: Vec<(&str, &[Finding])> = sections
        .iter()
        .filter_map(|section| {
            findings
                .get(section.range.clone())
                .map(|part| (section.category, part))
        })
        .filter(|(_, part)| !part.is_empty())
        .collect();
    ordered.sort_by_key(|(_, part)| std::cmp::Reverse(actionable_bytes(part)));

    let mut output = String::new();
    if !ordered.is_empty() {
        output.push_str(&format!("{}\n", "Categories".bold()));
    }
    for (name, part) in &ordered {
        let next = if part.iter().any(|finding| finding.action.is_actionable()) {
            format!("devtrim clean {name} --apply").cyan()
        } else {
            "report only".dimmed()
        };
        output.push_str(&format!(
            "  {name:<13} {:>9}  {:>5} finding(s)   {next}\n",
            gb(actionable_bytes(part)),
            part.len()
        ));
    }
    for (name, part) in &ordered {
        output.push_str(&format!("\n{}\n", name.bold()));
        if all || part.len() <= SCAN_FULL_LISTING {
            output.push_str(&findings_text(part));
            continue;
        }
        let mut largest: Vec<Finding> = part.to_vec();
        largest.sort_by_key(|finding| std::cmp::Reverse(finding.size_bytes));
        let rest = largest.split_off(SCAN_LARGEST);
        output.push_str(&findings_text(&largest));
        output.push_str(&format!(
            "           … {} more ({}); `devtrim clean {name}` lists them, `devtrim scan --all` lists everything\n",
            rest.len(),
            gb(rest
                .iter()
                .fold(0, |total: u64, finding| total.saturating_add(finding.size_bytes)))
        ));
    }
    output.push_str(&format!(
        "\n{} actionable across {} finding(s)\n",
        gb(actionable_bytes(findings)).bold(),
        findings.len()
    ));
    if !ordered.is_empty() {
        output.push_str(&format!(
            "Pick items one by one in the interactive view: {}. Project build output together: {}.\n",
            "devtrim".cyan(),
            "devtrim purge".cyan()
        ));
    }
    output
}

fn human_text(findings: &[Finding]) -> String {
    let mut output = findings_text(findings);
    output.push_str(&format!(
        "\n{} actionable across {} finding(s)\n",
        gb(actionable_bytes(findings)).bold(),
        findings.len()
    ));
    output
}

/// One entry per finding, with a header wherever a new project begins.
fn findings_text(findings: &[Finding]) -> String {
    let mut output = String::new();
    let mut headers = project_runs(findings)
        .into_iter()
        .filter_map(|(project, range, bytes)| {
            project.map(|project| (range.start, project, range.len(), bytes))
        })
        .peekable();
    for (index, finding) in findings.iter().enumerate() {
        if let Some((_, project, count, bytes)) = headers.next_if(|(start, ..)| *start == index) {
            output.push_str(&format!(
                "\n{} · {} in {count} item(s)\n",
                terminal_safe(project).bold(),
                gb(bytes)
            ));
        }
        let path = finding.path.as_deref().unwrap_or("-");
        output.push_str(&format!(
            "{:>9}  {}  {}  {}\n           └─ {}; action: {}\n",
            gb(finding.size_bytes),
            danger_tag(finding.danger),
            terminal_safe(&finding.label).bold(),
            terminal_safe(path).dimmed(),
            terminal_safe(&finding.note),
            human_action_display(&finding.action)
        ));
    }
    output
}

pub fn print_summary(summary: &Summary) -> std::io::Result<()> {
    let mut output = String::new();
    for note in &summary.notes {
        output.push_str(&format!("  {}\n", terminal_safe(note)));
    }
    output.push_str(&format!(
        "\n{} {}\n",
        "✓".green().bold(),
        terminal_safe_text(&summary_headline(summary))
    ));
    write_stdout(output.as_bytes())
}

/// One human-facing line to stdout, tolerant of a closed downstream pipe.
pub fn print_line(line: &str) -> std::io::Result<()> {
    write_stdout(format!("{line}\n").as_bytes())
}

pub fn print_json(
    operation: &str,
    applied: bool,
    findings: &[Finding],
    summary: Option<&Summary>,
    errors: &[String],
) -> std::io::Result<()> {
    let response = Response {
        operation,
        applied,
        findings,
        summary,
        errors,
    };
    let mut output = serde_json::to_string_pretty(&response)
        .unwrap_or_else(|_| r#"{"operation":"unknown","applied":false,"findings":[],"errors":["serialization failed"]}"#.into())
        .into_bytes();
    output.push(b'\n');
    write_stdout(&output)
}

pub fn print_error_json(operation: &str, message: &str) -> std::io::Result<()> {
    print_json(operation, false, &[], None, &[message.to_string()])
}

pub fn write_stdout(output: &[u8]) -> std::io::Result<()> {
    let mut stdout = std::io::stdout().lock();
    match stdout.write_all(output).and_then(|()| stdout.flush()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actionable_bytes_saturates_instead_of_wrapping() {
        let findings = [
            Finding::new("first", None, u64::MAX, "test", 1, Action::Info),
            Finding::new("second", None, 1, "test", 1, Action::Trash),
            Finding::new("third", None, u64::MAX, "test", 1, Action::Trash),
        ];

        assert_eq!(actionable_bytes(&findings), u64::MAX);
    }

    #[test]
    fn finding_preserves_json_text_and_escapes_only_for_terminals() {
        let finding = Finding::new(
            "cache\u{1b}[2Jé",
            Some(PathBuf::from("/tmp/line\nnext\u{202e}")),
            0,
            "note\rhidden",
            1,
            Action::Info,
        );

        let serialized = serde_json::to_value(&finding).unwrap();
        assert_eq!(serialized["label"], "cache\u{1b}[2Jé");
        assert_eq!(serialized["path"], "/tmp/line\nnext\u{202e}");
        assert!(serialized.get("identity").is_none());
        assert_eq!(terminal_safe(&finding.label), "cache\\u{1b}[2Jé");
    }

    /// Trash keeps every byte on the same volume until it is emptied, so a
    /// summary that called those bytes "reclaimed" read as freed space while
    /// free space had not moved (observed: 25 GB "reclaimed", `df` unchanged).
    #[test]
    fn summary_headline_never_calls_trashed_bytes_reclaimed() {
        let gib = 1024 * 1024 * 1024;
        let summary = |freed, trashed| Summary {
            op: "caches".into(),
            items_touched: 2,
            bytes_freed_estimate: freed,
            bytes_trashed_estimate: trashed,
            notes: Vec::new(),
        };

        let trashed = summary_headline(&summary(5 * gib, 5 * gib));
        assert!(trashed.contains("~5.0 GB moved to Trash"), "{trashed}");
        assert!(trashed.contains("devtrim trash-empty"), "{trashed}");
        assert!(!trashed.contains("reclaimed"), "{trashed}");

        let permanent = summary_headline(&summary(5 * gib, 0));
        assert!(permanent.contains("~5.0 GB reclaimed"), "{permanent}");
        assert!(!permanent.contains("Trash"), "{permanent}");

        let mixed = summary_headline(&summary(5 * gib, 2 * gib));
        assert!(mixed.contains("~3.0 GB reclaimed"), "{mixed}");
        assert!(mixed.contains("~2.0 GB moved to Trash"), "{mixed}");
    }

    /// A plan spanning many repositories reads as one flat list otherwise; each
    /// project gets one header with its own total, in plan order.
    #[test]
    fn human_plan_groups_consecutive_findings_by_project() {
        let finding = |path: &str, size, project: &str| {
            let mut finding = Finding::new(
                "stale node_modules",
                Some(PathBuf::from(path)),
                size,
                "test",
                5,
                Action::Trash,
            );
            finding.project = Some(project.into());
            finding
        };
        let plan = [
            finding("/dev/alpha/target", 3000, "/dev/alpha"),
            finding("/dev/alpha/node_modules", 1000, "/dev/alpha"),
            finding("/dev/beta/node_modules", 2000, "/dev/beta"),
        ];

        let text = human_text(&plan)
            .split('\u{1b}')
            .enumerate()
            .map(|(index, part)| {
                if index == 0 {
                    part
                } else {
                    part.split_once('m').map_or("", |(_, rest)| rest)
                }
            })
            .collect::<String>();

        assert_eq!(text.matches("/dev/alpha ·").count(), 1, "{text}");
        assert_eq!(text.matches("/dev/beta ·").count(), 1, "{text}");
        assert!(text.contains("/dev/alpha · 4 KB in 2 item(s)"), "{text}");
        assert!(
            text.find("/dev/alpha ·") < text.find("/dev/beta ·"),
            "project headers must follow plan order: {text}"
        );
    }

    #[test]
    fn human_command_action_escapes_controls_without_changing_json() {
        let raw = "unix:///tmp/\u{1b}]8;;https://example.com\u{7}\nnext\u{202e}";
        let action = Action::command("docker", &["--host", raw, "image", "prune"]);

        let serialized = serde_json::to_value(&action).unwrap();
        assert_eq!(serialized["args"][1], raw);

        let rendered = human_action_display(&action);
        assert!(!rendered.as_bytes().contains(&0x1b));
        assert!(!rendered.contains(raw));
        assert!(rendered.contains("\\u{1b}"));
        assert!(rendered.contains("\\u{7}"));
        assert!(rendered.contains("\\n"));
        assert!(rendered.contains("\\u{202e}"));
    }
}
