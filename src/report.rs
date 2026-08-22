//! Findings model + rendering.

use crate::cli::Cli;
use colored::Colorize;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Finding {
    /// Human label, e.g. "huggingface model cache"
    pub label: String,
    pub path: Option<String>,
    pub size_bytes: u64,
    /// Why it is safe (or not) to remove; shown in preview
    pub note: String,
    /// 1-10
    pub danger: u8,
    /// What apply will do: "trash" | "command: <cmd>"
    pub action: String,
}

#[derive(Debug, serde::Serialize)]
pub struct Summary {
    pub op: String,
    pub items_touched: usize,
    pub bytes_freed_estimate: u64,
    pub notes: Vec<String>,
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

pub fn print(findings: &[Finding], cli: &Cli) {
    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(findings).unwrap_or_else(|_| "[]".into())
        );
        return;
    }
    let total: u64 = findings.iter().map(|f| f.size_bytes).sum();
    for f in findings {
        let path = f.path.as_deref().unwrap_or("-");
        println!(
            "{:>9}  {}  {}  {}\n           └─ {}",
            gb(f.size_bytes),
            danger_tag(f.danger),
            f.label.bold(),
            path.dimmed(),
            f.note
        );
    }
    println!("\n{} across {} item(s)", gb(total).bold(), findings.len());
}

pub fn print_summary(s: &Summary, cli: &Cli) {
    if cli.json {
        println!("{}", serde_json::to_string_pretty(s).unwrap_or_default());
        return;
    }
    for n in &s.notes {
        println!("  {n}");
    }
    println!(
        "\n{} {}: {} item(s), ~{} reclaimed estimate",
        "✓".green().bold(),
        s.op,
        s.items_touched,
        gb(s.bytes_freed_estimate)
    );
}
