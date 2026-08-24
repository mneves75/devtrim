//! devtrim: developer-machine disk hygiene.
//!
//! Measure first, classify by risk, and apply only the reviewed plan.

mod cli;
mod ops;
mod report;
mod safety;
mod tui;

use anyhow::Result;
use clap::{CommandFactory, Parser};
use colored::Colorize;
use std::io::IsTerminal;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    if cli.command.is_none()
        && !cli.json
        && (!std::io::stdin().is_terminal() || !std::io::stdout().is_terminal())
    {
        if let Err(error) = cli::Cli::command().print_help() {
            eprintln!(
                "{} {}",
                "error:".red().bold(),
                report::terminal_safe(&error.to_string())
            );
            return ExitCode::from(1);
        }
        println!();
        return ExitCode::from(2);
    }
    let json = cli.json;
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            if json {
                report::print_error_json(&format!("{error:#}"));
            } else {
                eprintln!(
                    "{} {}",
                    "error:".red().bold(),
                    report::terminal_safe(&format!("{error:#}"))
                );
            }
            ExitCode::from(1)
        }
    }
}

fn run(mut cli: cli::Cli) -> Result<ExitCode> {
    let ctx = safety::Ctx::from_cli(&cli)?;
    let command = cli.command.take().unwrap_or(cli::Command::Tui);

    match command {
        cli::Command::Tui => {
            if cli.apply || cli.yes || cli.yolo || cli.shred || cli.json {
                anyhow::bail!(
                    "the TUI owns preview and confirmation; do not pass --apply, -y, --yolo, --shred, or --json"
                );
            }
            tui::run(&ctx)
        }
        cli::Command::Scan => {
            let mut scan = ops::scan_all(&ctx);
            report::effective_actions(&mut scan.findings, cli.shred);
            if ctx.json {
                report::print_json("scan", false, &scan.findings, None, &scan.errors);
            } else {
                report::print_human(&scan.findings);
                for error in &scan.errors {
                    eprintln!("{} {}", "warn".yellow(), report::terminal_safe(error));
                }
            }
            if scan.errors.is_empty() {
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::from(1))
            }
        }
        cli::Command::Clean { target } => clean(target, &cli, &ctx),
        cli::Command::Icloud => {
            let findings = match ops::icloud_status(&ctx) {
                Ok(findings) => findings,
                Err(error) => return command_error("icloud", false, &[], &ctx, error),
            };
            if ctx.json {
                report::print_json("icloud", false, &findings, None, &[]);
            } else {
                report::print_human(&findings);
            }
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::TrashEmpty { confirm_gb } => {
            let findings = match ops::trash_findings(&ctx) {
                Ok(findings) => findings,
                Err(error) => return command_error("trash-empty", false, &[], &ctx, error),
            };
            let size = report::actionable_bytes(&findings);
            if !cli.apply {
                if ctx.json {
                    report::print_json("trash-empty", false, &findings, None, &[]);
                } else {
                    report::print_human(&findings);
                    println!(
                        "\n{} no changes made. Re-run with {} and {} to act.",
                        "dry-run".yellow().bold(),
                        "--apply".cyan(),
                        format!("--confirm={}", size / (1024 * 1024 * 1024)).cyan()
                    );
                }
                return Ok(ExitCode::SUCCESS);
            }
            if !ctx.json {
                report::print_human(&findings);
            }
            safety::warn_data_loss(&ctx);
            if let Err(error) = safety::trash_gate(&ctx.home, confirm_gb) {
                return command_error("trash-empty", false, &findings, &ctx, error);
            }
            match ops::purge_trash(&findings, &ctx) {
                Ok(outcome) => Ok(print_outcome("trash-empty", &findings, &outcome, &ctx)),
                Err(error) => command_error("trash-empty", true, &findings, &ctx, error),
            }
        }
    }
}

fn clean(target: cli::Target, cli: &cli::Cli, ctx: &safety::Ctx) -> Result<ExitCode> {
    let operation = ops::by_name(target.as_str()).ok_or_else(|| {
        anyhow::anyhow!(
            "unknown target '{}'. valid: {}",
            target.as_str(),
            ops::names().join(", ")
        )
    })?;
    if !ctx.json {
        eprintln!("{} scanning '{}'…", "devtrim".bold(), operation.name());
    }
    let mut findings = match operation.scan(ctx) {
        Ok(findings) => findings,
        Err(error) => return command_error(operation.name(), false, &[], ctx, error),
    };
    report::effective_actions(&mut findings, cli.shred);

    if !cli.apply {
        if ctx.json {
            report::print_json(operation.name(), false, &findings, None, &[]);
        } else if findings.is_empty() {
            println!("{} nothing to clean in '{}'", "✓".green(), operation.name());
        } else {
            report::print_human(&findings);
            println!(
                "\n{} no changes made. Re-run with {} to act.",
                "dry-run".yellow().bold(),
                "--apply".cyan()
            );
        }
        return Ok(ExitCode::SUCCESS);
    }

    if findings.is_empty() {
        if ctx.json {
            report::print_json(operation.name(), true, &findings, None, &[]);
        } else {
            println!("{} nothing to clean in '{}'", "✓".green(), operation.name());
        }
        return Ok(ExitCode::SUCCESS);
    }

    if !ctx.json {
        report::print_human(&findings);
    }
    let actionable = findings
        .iter()
        .any(|finding| finding.action.is_actionable());
    if !actionable {
        return match operation.apply(&findings, ctx) {
            Ok(outcome) => Ok(print_outcome(operation.name(), &findings, &outcome, ctx)),
            Err(error) => command_error(operation.name(), false, &findings, ctx, error),
        };
    }

    let danger = safety::plan_danger(&findings);
    if let Err(error) = safety::gate(danger, ctx, &findings) {
        return command_error(operation.name(), false, &findings, ctx, error);
    }
    match operation.apply(&findings, ctx) {
        Ok(outcome) => Ok(print_outcome(operation.name(), &findings, &outcome, ctx)),
        Err(error) => command_error(operation.name(), true, &findings, ctx, error),
    }
}

fn print_outcome(
    operation: &str,
    findings: &[report::Finding],
    outcome: &ops::ApplyOutcome,
    ctx: &safety::Ctx,
) -> ExitCode {
    if ctx.json {
        report::print_json(
            operation,
            true,
            findings,
            Some(&outcome.summary),
            &outcome.errors,
        );
    } else {
        report::print_summary(&outcome.summary);
        for error in &outcome.errors {
            eprintln!("{} {}", "error:".red().bold(), report::terminal_safe(error));
        }
    }
    if outcome.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn command_error(
    operation: &str,
    applied: bool,
    findings: &[report::Finding],
    ctx: &safety::Ctx,
    error: anyhow::Error,
) -> Result<ExitCode> {
    if ctx.json {
        report::print_json(operation, applied, findings, None, &[format!("{error:#}")]);
        Ok(ExitCode::from(1))
    } else {
        Err(error)
    }
}
