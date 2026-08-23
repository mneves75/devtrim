//! devtrim: developer-machine disk hygiene.
//!
//! Measure first, classify by risk, and apply only the reviewed plan.

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
    let json = cli.json;
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            if json {
                report::print_error_json(&format!("{error:#}"));
            } else {
                eprintln!("{} {error:#}", "error:".red().bold());
            }
            ExitCode::from(1)
        }
    }
}

fn run(cli: cli::Cli) -> Result<ExitCode> {
    let ctx = safety::Ctx::from_cli(&cli)?;

    match cli.command {
        cli::Command::Scan => {
            let mut scan = ops::scan_all(&ctx);
            report::effective_actions(&mut scan.findings, cli.shred);
            if ctx.json {
                report::print_json("scan", false, &scan.findings, None, &scan.errors);
            } else {
                report::print_human(&scan.findings);
                for error in &scan.errors {
                    eprintln!("{} {error}", "warn".yellow());
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
            let size = match safety::dir_size(&ctx.home.join(".Trash")) {
                Ok(size) => size,
                Err(error) => return command_error("trash-empty", false, &[], &ctx, error),
            };
            let findings = vec![report::Finding::new(
                "macOS Trash contents",
                Some(ctx.home.join(".Trash")),
                size,
                "permanent purge; Finder recovery is no longer available afterward",
                9,
                report::Action::Shred,
            )];
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
            if let Err(error) = safety::validate_trash_root(&ctx.home) {
                return command_error("trash-empty", false, &findings, &ctx, error);
            }
            if let Err(error) = safety::trash_gate(&ctx.home, confirm_gb) {
                return command_error("trash-empty", false, &findings, &ctx, error);
            }
            match ops::purge_trash(&ctx) {
                Ok(items) => {
                    let summary = report::Summary {
                        op: "trash-empty".into(),
                        items_touched: items,
                        bytes_freed_estimate: size,
                        notes: vec!["Trash permanently purged".into()],
                    };
                    print_result("trash-empty", true, &findings, &summary, &ctx);
                    Ok(ExitCode::SUCCESS)
                }
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

fn print_result(
    operation: &str,
    applied: bool,
    findings: &[report::Finding],
    summary: &report::Summary,
    ctx: &safety::Ctx,
) {
    if ctx.json {
        report::print_json(operation, applied, findings, Some(summary), &[]);
    } else {
        report::print_summary(summary);
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
            eprintln!("{} {error}", "error:".red().bold());
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
