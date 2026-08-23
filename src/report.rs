//! Findings model + human/JSON rendering.

use colored::Colorize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Action {
    Trash,
    Shred,
    Command { program: String, args: Vec<String> },
    Info,
    None,
}

impl Action {
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

#[derive(Debug, Clone, serde::Serialize)]
pub struct Finding {
    pub label: String,
    pub path: Option<String>,
    /// Estimated logical bytes. APFS clones and sparse files can differ on disk.
    pub size_bytes: u64,
    pub note: String,
    /// 1-10
    pub danger: u8,
    pub action: Action,
    #[serde(skip)]
    target: Option<PathBuf>,
    #[serde(skip)]
    authority: TargetAuthority,
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
        Self {
            label: label.into(),
            path: display_path,
            size_bytes,
            note: note.into(),
            danger,
            action,
            target: path,
            authority: TargetAuthority::Standard,
        }
    }

    pub(crate) fn target(&self) -> Option<&Path> {
        self.target.as_deref()
    }

    pub(crate) fn with_authority(mut self, authority: TargetAuthority) -> Self {
        self.authority = authority;
        self
    }

    pub(crate) fn authority(&self) -> TargetAuthority {
        self.authority
    }
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
        .map(|finding| finding.size_bytes)
        .sum()
}

pub fn print_human(findings: &[Finding]) {
    let total = actionable_bytes(findings);
    for finding in findings {
        let path = finding.path.as_deref().unwrap_or("-");
        println!(
            "{:>9}  {}  {}  {}\n           └─ {}; action: {}",
            gb(finding.size_bytes),
            danger_tag(finding.danger),
            finding.label.bold(),
            path.dimmed(),
            finding.note,
            finding.action.display()
        );
    }
    println!(
        "\n{} actionable across {} finding(s)",
        gb(total).bold(),
        findings.len()
    );
}

pub fn print_summary(summary: &Summary) {
    for note in &summary.notes {
        println!("  {note}");
    }
    println!(
        "\n{} {}: {} item(s), ~{} reclaimed estimate",
        "✓".green().bold(),
        summary.op,
        summary.items_touched,
        gb(summary.bytes_freed_estimate)
    );
}

pub fn print_json(
    operation: &str,
    applied: bool,
    findings: &[Finding],
    summary: Option<&Summary>,
    errors: &[String],
) {
    let response = Response {
        operation,
        applied,
        findings,
        summary,
        errors,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&response)
            .unwrap_or_else(|_| r#"{"operation":"unknown","applied":false,"findings":[],"errors":["serialization failed"]}"#.into())
    );
}

pub fn print_error_json(message: &str) {
    print_json("unknown", false, &[], None, &[message.to_string()]);
}
