//! devtrim: developer-machine disk hygiene.
//!
//! Measure first, classify by risk, trim with a safety net (Trash-first).
//! Every mutating operation is previewable, scored 1-10 for danger, and
//! guarded by protected-path and size checks.

mod cli;
mod ops;
mod report;
mod safety;

use anyhow::Result;
use clap::Parser;
use colored::Colorize;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("{} {e:#}", "error:".red().bold());
            ExitCode::from(1)
        }
    }
}

fn run(cli: cli::Cli) -> Result<ExitCode> {
    let ctx = safety::Ctx::from_cli(&cli)?;

    match cli.command {
        cli::Command::Scan => {
            let findings = ops::scan_all(&ctx)?;
            report::print(&findings, &cli);
        }
        cli::Command::Clean { target } => {
            let op = ops::by_name(target.as_str()).ok_or_else(|| {
                anyhow::anyhow!(
                    "unknown target '{}'. valid: {}", target.as_str(),
                    ops::names().join(", ")
                )
            })?;
            if !ctx.json {
                eprintln!("{} scanning '{}'…", "devtrim".bold(), op.name());
            }
            let findings = op.scan(&ctx)?;
            if findings.is_empty() {
                if !ctx.json {
                    println!("{} nothing to clean in '{}'", "✓".green(), op.name());
                }
                return Ok(ExitCode::SUCCESS);
            }
            report::print(&findings, &cli);

            if !cli.apply {
                if !ctx.json {
                    println!("\n{} no changes made. Re-run with {} to act.", "dry-run".yellow().bold(), "--apply".cyan());
                }
                return Ok(ExitCode::SUCCESS);
            }

            // Safety gate: interactive confirm scaled by danger; refused in non-TTY
            // unless -y/--yolo was explicit.
            let max_danger = findings.iter().map(|f| f.danger).max().unwrap_or(1);
            safety::gate(max_danger, &ctx, &findings)?;

            let summary = op.apply(&findings, &ctx)?;
            report::print_summary(&summary, &cli);
        }
        cli::Command::Icloud => {
            let findings = ops::icloud_status(&ctx)?;
            report::print(&findings, &cli);
        }
        cli::Command::TrashEmpty { confirm_gb } => {
            safety::trash_gate(confirm_gb)?;
            let n = safety::purge_trash(&ctx.home)?;
            if !ctx.json {
                println!("{} purged {n} items from Trash", "✓".green());
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}


