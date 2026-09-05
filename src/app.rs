//! CLI application flow.

use crate::{analyze, cli, journal, largest, ops, report, safety, status, tui, uninstall};
use anyhow::Result;
use clap::{CommandFactory, Parser, error::ErrorKind};
use colored::Colorize;
use std::ffi::{OsStr, OsString};
use std::io::IsTerminal;
use std::process::ExitCode;

pub fn main_impl() -> ExitCode {
    let args = std::env::args_os().collect::<Vec<_>>();
    let json_requested = exact_json_flag(&args);
    let operation = operation_from_args(&args);
    let cli = match cli::Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(error) => return clap_error(error, json_requested, operation),
    };
    if let Some(message) = incompatible_flags(&cli) {
        if cli.json {
            if let Err(error) = report::print_json(operation, false, &[], None, &[message]) {
                eprintln!(
                    "{} {}",
                    "error:".red().bold(),
                    report::terminal_safe(&error.to_string())
                );
            }
        } else {
            eprintln!("{} {}", "error:".red().bold(), message);
        }
        return ExitCode::from(2);
    }
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

fn incompatible_flags(cli: &cli::Cli) -> Option<String> {
    let (operation, allow_apply, allow_yes, allow_yolo, allow_shred) = match cli.command.as_ref() {
        None | Some(cli::Command::Tui) => ("tui", false, false, false, false),
        Some(cli::Command::Scan) => ("scan", false, false, false, true),
        Some(cli::Command::Clean { target }) if *target == cli::Target::Leftovers => {
            ("leftovers", false, false, false, false)
        }
        Some(cli::Command::Clean { target })
            if matches!(*target, cli::Target::Docker | cli::Target::Simulators) =>
        {
            (target.as_str(), true, true, true, false)
        }
        Some(cli::Command::Clean { target }) => (target.as_str(), true, true, true, true),
        Some(cli::Command::Optimize { .. }) => ("optimize", true, true, true, false),
        Some(cli::Command::TrashEmpty { .. }) => ("trash-empty", true, true, true, false),
        Some(cli::Command::Status { .. }) => ("status", false, false, false, false),
        Some(cli::Command::Uninstall { .. }) => ("uninstall", false, false, false, false),
        Some(cli::Command::Analyze { .. }) => ("analyze", false, false, false, false),
        Some(cli::Command::Largest { .. }) => ("largest", false, false, false, false),
        Some(cli::Command::History { .. }) => ("history", false, false, false, false),
        Some(cli::Command::Completions { .. }) => ("completions", false, false, false, false),
        Some(cli::Command::Manpage) => ("manpage", false, false, false, false),
        Some(cli::Command::Icloud) => ("icloud", false, false, false, false),
    };
    let mut rejected = Vec::new();
    if cli.apply && !allow_apply {
        rejected.push("--apply");
    }
    if cli.yes && !allow_yes {
        rejected.push("-y/--yes");
    }
    if cli.yolo && !allow_yolo {
        rejected.push("--yolo");
    }
    if cli.shred && !allow_shred {
        rejected.push("--shred");
    }
    if matches!(cli.command, None | Some(cli::Command::Tui)) && cli.json {
        rejected.push("--json");
    }
    (!rejected.is_empty()).then(|| {
        format!(
            "{operation} does not accept flag(s): {}",
            rejected.join(", ")
        )
    })
}

fn exact_json_flag(args: &[OsString]) -> bool {
    args.iter()
        .skip(1)
        .take_while(|arg| arg.as_os_str() != OsStr::new("--"))
        .any(|arg| arg.as_os_str() == OsStr::new("--json"))
}

fn first_positional(args: &[OsString]) -> Option<(&OsStr, &[OsString])> {
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_os_str();
        if argument == OsStr::new("--") {
            break;
        }
        if argument == OsStr::new("--root") {
            index = index.saturating_add(2);
            continue;
        }
        if argument
            .to_str()
            .is_some_and(|value| value.starts_with("--root="))
        {
            index = index.saturating_add(1);
            continue;
        }
        if argument.to_string_lossy().starts_with('-') {
            index = index.saturating_add(1);
            continue;
        }
        return Some((argument, &args[index + 1..]));
    }
    None
}

fn operation_from_args(args: &[OsString]) -> &'static str {
    let Some((argument, remaining)) = first_positional(args.get(1..).unwrap_or_default()) else {
        return "unknown";
    };
    match argument.to_str() {
        Some("clean") => clean_operation_from_args(remaining),
        Some("tui") => "tui",
        Some("scan") => "scan",
        Some("analyze") => "analyze",
        Some("status") => "status",
        Some("optimize") => "optimize",
        Some("uninstall") => "uninstall",
        Some("largest") => "largest",
        Some("history") => "history",
        Some("completions") => "completions",
        Some("manpage") => "manpage",
        Some("icloud") => "icloud",
        Some("trash-empty") => "trash-empty",
        _ => "unknown",
    }
}

fn clean_operation_from_args(args: &[OsString]) -> &'static str {
    match first_positional(args).and_then(|(argument, _)| argument.to_str()) {
        Some("caches") => "caches",
        Some("node-modules") => "node-modules",
        Some("artifacts") => "artifacts",
        Some("installers") => "installers",
        Some("simulators") => "simulators",
        Some("xcode") => "xcode",
        Some("docker") => "docker",
        Some("toolchains") => "toolchains",
        Some("leftovers") => "leftovers",
        _ => "clean",
    }
}

fn clap_error(error: clap::Error, json: bool, operation: &str) -> ExitCode {
    let kind = error.kind();
    if json {
        let (message, code) = match kind {
            ErrorKind::DisplayHelp => ("help has no JSON form".to_string(), 1),
            ErrorKind::DisplayVersion => ("version has no JSON form".to_string(), 1),
            _ => (error.to_string(), error.exit_code()),
        };
        if let Err(output_error) =
            report::print_json(operation, false, &[], None, std::slice::from_ref(&message))
        {
            eprintln!(
                "{} {}",
                "error:".red().bold(),
                report::terminal_safe(&output_error.to_string())
            );
            return ExitCode::from(1);
        }
        return exit_code(code);
    }

    let code = error.exit_code();
    if let Err(print_error) = error.print() {
        eprintln!(
            "{} {}",
            "error:".red().bold(),
            report::terminal_safe(&print_error.to_string())
        );
        return ExitCode::from(1);
    }
    exit_code(code)
}

fn exit_code(code: i32) -> ExitCode {
    u8::try_from(code)
        .map(ExitCode::from)
        .unwrap_or_else(|_| ExitCode::from(1))
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
        cli::Command::Tui => tui::run(&ctx),
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
        cli::Command::Status { watch } => status::run(&ctx, watch),
        cli::Command::Uninstall { ref app } => uninstall::run(&ctx, app),
        cli::Command::Analyze { ref path } => analyze::run(&ctx, path.as_deref()),
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
        cli::Command::Optimize { ref tasks } => {
            let operation = ops::optimize::Optimize::new(tasks, cli.apply)?;
            run_op(&operation, &cli, &ctx)
        }
        cli::Command::Clean { target } => clean(target, &cli, &ctx),
        #[allow(
            clippy::unreachable,
            reason = "context-free commands return earlier in run(), so no configuration failure can reach this arm"
        )]
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
    run_op(ops::for_target(target).as_ref(), cli, ctx)
}

/// Shared preview/confirm/apply flow for every `Op`, whether it is reached
/// through `clean <target>` or through a command of its own.
fn run_op(operation: &dyn ops::Op, cli: &cli::Cli, ctx: &safety::Ctx) -> Result<ExitCode> {
    if !ctx.json {
        eprintln!("{} scanning '{}'…", "devtrim".bold(), operation.name());
    }
    let mut findings = match operation.scan(ctx, &ops::project::ScanObservations::default()) {
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
            let outcome = ops::ApplyOutcome::new(operation.name());
            return print_outcome(operation.name(), &findings, &outcome, ctx);
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
        if errors.is_empty() {
            report::print_summary(&outcome.summary)?;
        } else {
            print_non_success_summary(&outcome.summary)?;
        }
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

fn print_non_success_summary(summary: &report::Summary) -> std::io::Result<()> {
    for note in &summary.notes {
        report::print_line(&format!("  {}", report::terminal_safe(note)))?;
    }
    let (marker, status) = if summary.items_touched == 0 {
        ("✗".red().bold(), "failed".red().bold())
    } else {
        ("!".yellow().bold(), "partial".yellow().bold())
    };
    report::print_line(&format!(
        "\n{marker} {} {status}: {} item(s), ~{} reclaimed estimate",
        report::terminal_safe(&summary.op),
        summary.items_touched,
        report::gb(summary.bytes_freed_estimate)
    ))
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
