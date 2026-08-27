//! CLI application flow.

use crate::{cli, journal, largest, ops, report, safety, tui};
use anyhow::Result;
use clap::{CommandFactory, Parser};
use colored::Colorize;
use std::io::IsTerminal;
use std::process::ExitCode;

pub fn main_impl() -> ExitCode {
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
        let _ = report::write_stdout(b"\n");
        return ExitCode::from(2);
    }
    let json = cli.json;
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            if json {
                if let Err(output_error) = report::print_error_json(&format!("{error:#}")) {
                    eprintln!(
                        "{} {}",
                        "error:".red().bold(),
                        report::terminal_safe(&output_error.to_string())
                    );
                }
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

fn run_history(limit: Option<usize>, json: bool) -> Result<ExitCode> {
    let limit = limit.unwrap_or(20).clamp(1, 1000);
    let journal_path = safety::default_journal_path()?;
    let history = match journal::read_history(&journal_path, limit) {
        Ok(history) => history,
        Err(error) if json => {
            let history = journal::History {
                entries: Vec::new(),
                errors: vec![format!("{error:#}")],
            };
            journal::print_json(&history)?;
            return Ok(ExitCode::from(1));
        }
        Err(error) => return Err(error),
    };
    if json {
        journal::print_json(&history)?;
    } else {
        journal::print_human(&history)?;
        for error in &history.errors {
            eprintln!("{} {}", "warn".yellow(), report::terminal_safe(error));
        }
    }
    // An incomplete audit is a partial operation: visible in errors AND status.
    if history.errors.is_empty() {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(1))
    }
}

fn generated_doc_json_error(operation: &str) -> Result<ExitCode> {
    report::print_json(
        operation,
        false,
        &[],
        None,
        &[format!("{operation} has no JSON form")],
    )?;
    Ok(ExitCode::from(1))
}

fn run_completions(shell: clap_complete::Shell, json: bool) -> Result<ExitCode> {
    if json {
        return generated_doc_json_error("completions");
    }
    let mut command = cli::Cli::command();
    let mut output = Vec::new();
    clap_complete::generate(shell, &mut command, "devtrim", &mut output);
    report::write_stdout(&output)?;
    Ok(ExitCode::SUCCESS)
}

fn run_manpage(json: bool) -> Result<ExitCode> {
    if json {
        return generated_doc_json_error("manpage");
    }
    let mut output = Vec::new();
    clap_mangen::Man::new(cli::Cli::command()).render(&mut output)?;
    report::write_stdout(&output)?;
    Ok(ExitCode::SUCCESS)
}

fn run(mut cli: cli::Cli) -> Result<ExitCode> {
    // Recovery and shell-integration commands must not depend on cleanup
    // configuration: a malformed devtrim.toml cannot be allowed to block
    // journal inspection or completion/man-page generation. Borrow only —
    // `Ctx::from_cli` still needs `cli.command` to route diagnostics.
    match cli.command {
        Some(cli::Command::History { limit }) => return run_history(limit, cli.json),
        Some(cli::Command::Completions { shell }) => return run_completions(shell, cli.json),
        Some(cli::Command::Manpage) => return run_manpage(cli.json),
        _ => {}
    }
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
                report::print_json("scan", false, &scan.findings, None, &scan.errors)?;
            } else {
                report::print_human(&scan.findings)?;
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
        cli::Command::Largest { top } => {
            let result = largest::scan(&ctx, top);
            if ctx.json {
                report::print_json("largest", false, &result.findings, None, &result.errors)?;
            } else {
                report::print_human(&result.findings)?;
                for error in &result.errors {
                    ctx.diagnostic("warn", error.clone());
                }
            }
            // Partial visibility follows the same contract as scan: disclosed
            // in errors AND in the exit status.
            if result.errors.is_empty() {
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::from(1))
            }
        }
        cli::Command::Clean { target } => clean(target, &cli, &ctx),
        cli::Command::History { .. } | cli::Command::Completions { .. } | cli::Command::Manpage => {
            unreachable!("context-free commands are dispatched before configuration loads")
        }
        cli::Command::Icloud => {
            let findings = match ops::icloud_status(&ctx) {
                Ok(findings) => findings,
                Err(error) => return command_error("icloud", false, &[], &ctx, error),
            };
            if ctx.json {
                report::print_json("icloud", false, &findings, None, &[])?;
            } else {
                report::print_human(&findings)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::TrashEmpty { confirm_gb } => {
            let mut findings = match ops::trash_findings(&ctx) {
                Ok(findings) => findings,
                Err(error) => return command_error("trash-empty", false, &[], &ctx, error),
            };
            ops::filter_protected_findings(&mut findings, &ctx);
            let size = report::actionable_bytes(&findings);
            if !cli.apply {
                if ctx.json {
                    report::print_json("trash-empty", false, &findings, None, &[])?;
                } else {
                    report::print_human(&findings)?;
                    report::print_line(&format!(
                        "\n{} no changes made. Re-run with {} and {} to act.",
                        "dry-run".yellow().bold(),
                        "--apply".cyan(),
                        format!("--confirm={}", size / (1024 * 1024 * 1024)).cyan()
                    ))?;
                }
                return Ok(ExitCode::SUCCESS);
            }
            if !ctx.json {
                report::print_human(&findings)?;
            }
            safety::warn_data_loss(&ctx);
            if let Err(error) = safety::trash_gate(&ctx.home, confirm_gb) {
                return command_error("trash-empty", false, &findings, &ctx, error);
            }
            match ops::purge_trash(&findings, &ctx) {
                Ok(outcome) => print_outcome("trash-empty", &findings, &outcome, &ctx),
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
    ops::filter_protected_findings(&mut findings, ctx);
    report::effective_actions(&mut findings, cli.shred);

    if !cli.apply {
        if ctx.json {
            report::print_json(operation.name(), false, &findings, None, &[])?;
        } else if findings.is_empty() {
            report::print_line(&format!(
                "{} nothing to clean in '{}'",
                "✓".green(),
                operation.name()
            ))?;
        } else {
            report::print_human(&findings)?;
            report::print_line(&format!(
                "\n{} no changes made. Re-run with {} to act.",
                "dry-run".yellow().bold(),
                "--apply".cyan()
            ))?;
        }
        return Ok(ExitCode::SUCCESS);
    }

    if findings.is_empty() {
        if ctx.json {
            report::print_json(operation.name(), true, &findings, None, &[])?;
        } else {
            report::print_line(&format!(
                "{} nothing to clean in '{}'",
                "✓".green(),
                operation.name()
            ))?;
        }
        return Ok(ExitCode::SUCCESS);
    }

    if !ctx.json {
        report::print_human(&findings)?;
    }
    let actionable = findings
        .iter()
        .any(|finding| finding.action.is_actionable());
    if !actionable {
        return match operation.apply(&findings, ctx) {
            Ok(outcome) => print_outcome(operation.name(), &findings, &outcome, ctx),
            Err(error) => command_error(operation.name(), false, &findings, ctx, error),
        };
    }

    let danger = safety::plan_danger(&findings);
    if let Err(error) = safety::gate(danger, ctx, &findings) {
        return command_error(operation.name(), false, &findings, ctx, error);
    }
    match operation.apply(&findings, ctx) {
        Ok(outcome) => print_outcome(operation.name(), &findings, &outcome, ctx),
        Err(error) => command_error(operation.name(), true, &findings, ctx, error),
    }
}

fn print_outcome(
    operation: &str,
    findings: &[report::Finding],
    outcome: &ops::ApplyOutcome,
    ctx: &safety::Ctx,
) -> Result<ExitCode> {
    // A journal failure after a successful mutation keeps the touched summary
    // truthful but must still surface as an error with a nonzero status.
    let mut errors = outcome.errors.clone();
    errors.extend(ctx.take_journal_errors());
    if ctx.json {
        report::print_json(operation, true, findings, Some(&outcome.summary), &errors)?;
    } else {
        report::print_summary(&outcome.summary)?;
        for error in &errors {
            eprintln!("{} {}", "error:".red().bold(), report::terminal_safe(error));
        }
    }
    if errors.is_empty() {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::from(1))
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
        report::print_json(operation, applied, findings, None, &[format!("{error:#}")])?;
        Ok(ExitCode::from(1))
    } else {
        Err(error)
    }
}
