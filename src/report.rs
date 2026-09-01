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
            Self::DeleteSimulator { .. } => None,
        }
    }

    pub(crate) fn simulator_udid(&self) -> Option<&str> {
        match self {
            Self::DeleteSimulator { udid } => Some(udid),
            Self::DockerImagePrune { .. } | Self::DockerBuilderPrune { .. } => None,
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
    #[serde(skip)]
    target: Option<PathBuf>,
    #[serde(skip)]
    identity: Option<FileIdentity>,
    #[serde(skip)]
    authority: TargetAuthority,
    #[serde(skip)]
    command_authority: Option<CommandAuthority>,
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
            target: path,
            identity,
            authority: TargetAuthority::Standard,
            command_authority: None,
        }
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

#[derive(Debug, serde::Serialize)]
pub struct Summary {
    pub op: String,
    pub items_touched: usize,
    pub bytes_freed_estimate: u64,
    pub notes: Vec<String>,
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
    let b = bytes as f64;
    if b >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} GB", b / 1024.0 / 1024.0 / 1024.0)
    } else if b >= 1024.0 * 1024.0 {
        format!("{:.0} MB", b / 1024.0 / 1024.0)
    } else if b >= 1024.0 {
        format!("{:.0} KB", b / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn danger_tag(d: u8) -> colored::ColoredString {
    match d {
        0..=2 => format!("danger:{d}").green(),
        3..=5 => format!("danger:{d}").yellow(),
        6..=8 => format!("danger:{d}").truecolor(255, 140, 0),
        _ => format!("danger:{d}").red().bold(),
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

fn human_action_display(action: &Action) -> String {
    terminal_safe(&action.display())
}

pub fn print_human(findings: &[Finding]) -> std::io::Result<()> {
    let total = actionable_bytes(findings);
    let mut output = String::new();
    for finding in findings {
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
    output.push_str(&format!(
        "\n{} actionable across {} finding(s)\n",
        gb(total).bold(),
        findings.len()
    ));
    write_stdout(output.as_bytes())
}

pub fn print_summary(summary: &Summary) -> std::io::Result<()> {
    let mut output = String::new();
    for note in &summary.notes {
        output.push_str(&format!("  {}\n", terminal_safe(note)));
    }
    output.push_str(&format!(
        "\n{} {}: {} item(s), ~{} reclaimed estimate\n",
        "✓".green().bold(),
        summary.op,
        summary.items_touched,
        gb(summary.bytes_freed_estimate)
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

pub fn print_error_json(message: &str) -> std::io::Result<()> {
    print_json("unknown", false, &[], None, &[message.to_string()])
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

        assert_eq!(finding.label, "cache\u{1b}[2Jé");
        assert_eq!(finding.path.as_deref(), Some("/tmp/line\nnext\u{202e}"));
        assert_eq!(finding.note, "note\rhidden");
        let serialized = serde_json::to_value(&finding).unwrap();
        assert_eq!(serialized["label"], "cache\u{1b}[2Jé");
        assert_eq!(serialized["path"], "/tmp/line\nnext\u{202e}");
        assert!(serialized.get("identity").is_none());
        assert_eq!(terminal_safe(&finding.label), "cache\\u{1b}[2Jé");
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
