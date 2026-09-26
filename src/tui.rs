//! Interactive Ratatui adapter over the existing scan, preview, and apply core.

use std::io::{self, IsTerminal};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::cli::Target;
use crate::ops::{self, Action, ApplyOutcome, Finding, Op};
use crate::report::{self, Summary};
use crate::safety::{self, ConfirmationRequirement, Ctx};
use crate::theme::{Theme, Token, danger_token};

const MIN_WIDTH: u16 = 64;
const MIN_HEIGHT: u16 = 18;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    ScanAll,
    Clean(Target),
    Purge,
    Icloud,
    TrashEmpty,
}

impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::ScanAll => "scan",
            Self::Clean(target) => target.as_str(),
            Self::Purge => "purge",
            Self::Icloud => "icloud",
            Self::TrashEmpty => "trash-empty",
        }
    }

    fn read_only(self) -> bool {
        matches!(
            self,
            Self::ScanAll | Self::Icloud | Self::Clean(Target::Leftovers)
        )
    }
}

struct MenuItem {
    key: &'static str,
    label: &'static str,
    description: &'static str,
    operation: Operation,
}

const MENU: &[MenuItem] = &[
    MenuItem {
        key: "1",
        label: "Scan everything",
        description: "Read-only report across every cleanup category.",
        operation: Operation::ScanAll,
    },
    MenuItem {
        key: "2",
        label: "Caches",
        description: "Regenerable package and model download caches.",
        operation: Operation::Clean(Target::Caches),
    },
    MenuItem {
        key: "3",
        label: "node_modules",
        description: "Exact paths in conclusively stale Git repositories.",
        operation: Operation::Clean(Target::NodeModules),
    },
    MenuItem {
        key: "4",
        label: "Build artifacts",
        description: "Regenerable outputs in conclusively stale Git repositories.",
        operation: Operation::Clean(Target::Artifacts),
    },
    MenuItem {
        key: "p",
        label: "Project purge",
        description: "node_modules and build artifacts together, grouped by project, largest first.",
        operation: Operation::Purge,
    },
    MenuItem {
        key: "5",
        label: "Simulators",
        description: "Unavailable Apple simulator devices only.",
        operation: Operation::Clean(Target::Simulators),
    },
    MenuItem {
        key: "6",
        label: "Xcode",
        description: "DeviceSupport and DerivedData; Archives stay excluded.",
        operation: Operation::Clean(Target::Xcode),
    },
    MenuItem {
        key: "7",
        label: "Docker",
        description: "Unused images and build cache; volumes are never touched.",
        operation: Operation::Clean(Target::Docker),
    },
    MenuItem {
        key: "8",
        label: "Swift toolchains",
        description: "Unreferenced swift.org toolchains only.",
        operation: Operation::Clean(Target::Toolchains),
    },
    MenuItem {
        key: "d",
        label: "Installers",
        description: "Stale .dmg/.pkg archives in Downloads and Desktop.",
        operation: Operation::Clean(Target::Installers),
    },
    MenuItem {
        key: "a",
        label: "Agent caches & history",
        description: "Agent caches, plus session history past the active window.",
        operation: Operation::Clean(Target::Agents),
    },
    MenuItem {
        key: "9",
        label: "Agent leftovers",
        description: "Read-only hints; whole worktrees are never deleted.",
        operation: Operation::Clean(Target::Leftovers),
    },
    MenuItem {
        key: "i",
        label: "iCloud status",
        description: "Read-only local-materialization status for large uploads.",
        operation: Operation::Icloud,
    },
    MenuItem {
        key: "0",
        label: "Empty Trash",
        description: "Permanent purge with a typed size acknowledgment.",
        operation: Operation::TrashEmpty,
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Menu,
    Loading,
    Results,
    Confirm,
    Outcome,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfirmationKind {
    YesNo { danger: u8 },
    Critical { danger: u8, expected_gb: u64 },
    TrashPurge { expected_gb: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Approval {
    Yes,
    CriticalGigabytes(u64),
    TrashPurgeGigabytes(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ApprovedPlan {
    operation: Operation,
    findings: Vec<Finding>,
    approval: Approval,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Intent {
    None,
    Load(Operation),
    Apply(ApprovedPlan),
    Quit,
}

struct App {
    screen: Screen,
    selected: usize,
    operation: Option<Operation>,
    findings: Vec<Finding>,
    /// Highlighted row of the results list: a finding, then scan errors, then
    /// warnings, in that order.
    cursor: usize,
    /// Findings left out of the plan. Selection can only narrow a preview: the
    /// approved plan is always a subset of what was displayed.
    excluded: std::collections::BTreeSet<usize>,
    /// First visible results row, kept across frames so the list scrolls only
    /// as far as the cursor needs.
    list_offset: std::cell::Cell<usize>,
    errors: Vec<String>,
    warnings: Vec<String>,
    summary: Option<Summary>,
    shred: bool,
    scroll: u16,
    confirmation: Option<ConfirmationKind>,
    input: String,
    status: String,
    failed: bool,
    /// Resolved once at construction so every frame renders from the same
    /// decision rather than re-reading the environment per widget.
    theme: Theme,
    /// Whether the full keybinding reference is open over the current screen.
    help: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            screen: Screen::Menu,
            selected: 0,
            operation: None,
            findings: Vec::new(),
            cursor: 0,
            excluded: std::collections::BTreeSet::new(),
            list_offset: std::cell::Cell::new(0),
            errors: Vec::new(),
            warnings: Vec::new(),
            summary: None,
            shred: false,
            scroll: 0,
            confirmation: None,
            input: String::new(),
            status: "Preview first. Nothing changes until you explicitly approve.".into(),
            failed: false,
            theme: Theme::from_env(),
            help: false,
        }
    }
}

impl App {
    /// The previewed findings still in the plan, in display order.
    fn selected_findings(&self) -> Vec<Finding> {
        self.findings
            .iter()
            .enumerate()
            .filter(|(index, _)| !self.excluded.contains(index))
            .map(|(_, finding)| finding.clone())
            .collect()
    }

    fn effective_findings(&self) -> Vec<Finding> {
        let mut findings = self.selected_findings();
        report::effective_actions(&mut findings, self.shred);
        findings
    }

    fn has_actionable_findings(&self) -> bool {
        self.selected_findings()
            .iter()
            .any(|finding| finding.action.is_actionable())
    }

    fn can_toggle_shred(&self) -> bool {
        matches!(self.operation, Some(Operation::Clean(_) | Operation::Purge))
            && self
                .selected_findings()
                .iter()
                .any(|finding| finding.action == Action::Trash)
    }

    /// Whether the finding at `index` is a choice the operator can make: only
    /// an actionable finding of an operation that can apply.
    fn is_selectable(&self, index: usize) -> bool {
        self.operation
            .is_some_and(|operation| !operation.read_only())
            && self
                .findings
                .get(index)
                .is_some_and(|finding| finding.action.is_actionable())
    }

    fn row_count(&self) -> usize {
        self.findings.len() + self.errors.len() + self.warnings.len()
    }

    /// How many previewed findings the operator can choose between.
    fn choice_count(&self) -> usize {
        (0..self.findings.len())
            .filter(|index| self.is_selectable(*index))
            .count()
    }

    fn move_cursor(&mut self, to: usize) {
        self.cursor = to.min(self.row_count().saturating_sub(1));
    }

    fn toggle_selected(&mut self) {
        if !self.is_selectable(self.cursor) {
            return;
        }
        if !self.excluded.remove(&self.cursor) {
            self.excluded.insert(self.cursor);
        }
    }

    /// Everything back in when anything is out; otherwise every choice out.
    fn toggle_all(&mut self) {
        if self.excluded.is_empty() {
            self.excluded = (0..self.findings.len())
                .filter(|index| self.is_selectable(*index))
                .collect();
        } else {
            self.excluded.clear();
        }
    }

    fn reset_selection(&mut self) {
        self.cursor = 0;
        self.excluded.clear();
        self.list_offset.set(0);
    }

    fn begin_load(&mut self, operation: Operation) {
        self.screen = Screen::Loading;
        self.operation = Some(operation);
        self.reset_selection();
        self.findings.clear();
        self.errors.clear();
        self.warnings.clear();
        self.summary = None;
        self.shred = false;
        self.scroll = 0;
        self.confirmation = None;
        self.input.clear();
        self.status = format!("Scanning {}…", operation.name());
    }

    fn finish_results(
        &mut self,
        operation: Operation,
        findings: Vec<Finding>,
        errors: Vec<String>,
        warnings: Vec<String>,
    ) {
        self.operation = Some(operation);
        self.reset_selection();
        self.findings = findings;
        self.errors = errors;
        self.warnings = warnings;
        if !self.errors.is_empty() {
            self.failed = true;
        }
        self.summary = None;
        self.screen = Screen::Results;
        self.status = if self.findings.is_empty() {
            "No findings. Nothing can be applied.".into()
        } else if operation.read_only() {
            "Read-only result. No apply action is available.".into()
        } else {
            "Review every finding. Space leaves one out; a applies the rest.".into()
        };
    }

    fn fail(&mut self, error: anyhow::Error) {
        self.failed = true;
        self.errors = vec![format!("{error:#}")];
        self.summary = None;
        self.screen = Screen::Error;
        self.status = "The operation failed closed; no new action was authorized.".into();
    }

    fn begin_confirmation(&mut self) {
        let Some(operation) = self.operation else {
            return;
        };
        if operation.read_only() {
            self.status = "This result has no actionable findings.".into();
            return;
        }
        if !self.has_actionable_findings() {
            self.status = if self
                .findings
                .iter()
                .any(|finding| finding.action.is_actionable())
            {
                "Nothing is selected. Space adds the highlighted item; A selects every item."
            } else {
                "This result has no actionable findings."
            }
            .into();
            return;
        }
        let findings = self.effective_findings();
        self.confirmation = Some(if operation == Operation::TrashEmpty {
            ConfirmationKind::TrashPurge {
                expected_gb: report::actionable_bytes(&findings) / (1024 * 1024 * 1024),
            }
        } else {
            match safety::confirmation_requirement(safety::plan_danger(&findings), &findings) {
                ConfirmationRequirement::YesNo { danger } => ConfirmationKind::YesNo { danger },
                ConfirmationRequirement::TypedGigabytes { danger, expected } => {
                    ConfirmationKind::Critical {
                        danger,
                        expected_gb: expected,
                    }
                }
            }
        });
        self.input.clear();
        self.status.clear();
        self.screen = Screen::Confirm;
    }

    fn back_to_menu(&mut self) {
        self.screen = Screen::Menu;
        self.operation = None;
        self.reset_selection();
        self.findings.clear();
        self.errors.clear();
        self.warnings.clear();
        self.summary = None;
        self.shred = false;
        self.scroll = 0;
        self.confirmation = None;
        self.input.clear();
        self.status = "Preview first. Nothing changes until you explicitly approve.".into();
    }

    fn handle_key(&mut self, key: KeyEvent) -> Intent {
        if key.kind != KeyEventKind::Press {
            return Intent::None;
        }
        if key.modifiers.contains(event::KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Intent::Quit;
        }
        // The help overlay is deliberately unavailable on the confirmation
        // screen: that screen demands an exact typed acknowledgment, and
        // stacking a second overlay over it would obscure the plan being
        // approved. Everywhere else `?` opens it and any of `?`/Esc/q closes it
        // without reaching the screen underneath.
        if self.screen != Screen::Confirm {
            if self.help {
                if matches!(
                    key.code,
                    KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q')
                ) {
                    self.help = false;
                }
                return Intent::None;
            }
            if key.code == KeyCode::Char('?') {
                self.help = true;
                return Intent::None;
            }
        }
        match self.screen {
            Screen::Menu => self.handle_menu_key(key.code),
            Screen::Results => self.handle_results_key(key.code),
            Screen::Confirm => self.handle_confirm_key(key.code),
            Screen::Outcome | Screen::Error => match key.code {
                KeyCode::Char('q') => Intent::Quit,
                KeyCode::Esc | KeyCode::Char('b') => {
                    self.back_to_menu();
                    Intent::None
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.scroll = self.scroll.saturating_sub(1);
                    Intent::None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.scroll = self.scroll.saturating_add(1);
                    Intent::None
                }
                KeyCode::PageUp => {
                    self.scroll = self.scroll.saturating_sub(8);
                    Intent::None
                }
                KeyCode::PageDown => {
                    self.scroll = self.scroll.saturating_add(8);
                    Intent::None
                }
                _ => Intent::None,
            },
            Screen::Loading => Intent::None,
        }
    }

    fn handle_menu_key(&mut self, key: KeyCode) -> Intent {
        match key {
            KeyCode::Char('q') | KeyCode::Esc => Intent::Quit,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                Intent::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = (self.selected + 1).min(MENU.len() - 1);
                Intent::None
            }
            KeyCode::Home => {
                self.selected = 0;
                Intent::None
            }
            KeyCode::End => {
                self.selected = MENU.len() - 1;
                Intent::None
            }
            KeyCode::Enter => Intent::Load(MENU[self.selected].operation),
            KeyCode::Char(value) => MENU
                .iter()
                .position(|item| item.key.chars().eq(std::iter::once(value)))
                .map_or(Intent::None, |index| {
                    self.selected = index;
                    Intent::Load(MENU[index].operation)
                }),
            _ => Intent::None,
        }
    }

    fn handle_results_key(&mut self, key: KeyCode) -> Intent {
        match key {
            KeyCode::Char('q') => Intent::Quit,
            KeyCode::Esc | KeyCode::Char('b') => {
                self.back_to_menu();
                Intent::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_cursor(self.cursor.saturating_sub(1));
                Intent::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_cursor(self.cursor.saturating_add(1));
                Intent::None
            }
            KeyCode::PageUp => {
                self.move_cursor(self.cursor.saturating_sub(8));
                Intent::None
            }
            KeyCode::PageDown => {
                self.move_cursor(self.cursor.saturating_add(8));
                Intent::None
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.move_cursor(0);
                Intent::None
            }
            KeyCode::End | KeyCode::Char('G') => {
                self.move_cursor(usize::MAX);
                Intent::None
            }
            KeyCode::Char(' ') => {
                self.toggle_selected();
                Intent::None
            }
            KeyCode::Char('A') if self.choice_count() > 0 => {
                self.toggle_all();
                Intent::None
            }
            KeyCode::Char('s') if self.can_toggle_shred() => {
                self.shred = !self.shred;
                self.scroll = 0;
                self.status = if self.shred {
                    "Permanent mode: preview actions changed to SHRED and danger is critical."
                        .into()
                } else {
                    "Trash-first mode restored.".into()
                };
                Intent::None
            }
            KeyCode::Char('a') => {
                self.begin_confirmation();
                Intent::None
            }
            KeyCode::Char('r') => self.operation.map_or(Intent::None, Intent::Load),
            _ => Intent::None,
        }
    }

    fn handle_confirm_key(&mut self, key: KeyCode) -> Intent {
        let Some(confirmation) = self.confirmation else {
            self.screen = Screen::Results;
            return Intent::None;
        };
        if key == KeyCode::Esc {
            self.screen = Screen::Results;
            self.input.clear();
            self.status = "Apply canceled; the preview remains unchanged.".into();
            return Intent::None;
        }
        match confirmation {
            ConfirmationKind::YesNo { .. } => match key {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.approve(Approval::Yes),
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.screen = Screen::Results;
                    self.status = "Apply canceled; the preview remains unchanged.".into();
                    Intent::None
                }
                _ => Intent::None,
            },
            ConfirmationKind::Critical { expected_gb, .. } => match key {
                KeyCode::Char(value) if value.is_ascii_digit() => {
                    self.input.push(value);
                    Intent::None
                }
                KeyCode::Backspace => {
                    self.input.pop();
                    Intent::None
                }
                KeyCode::Enter if self.input == expected_gb.to_string() => {
                    self.approve(Approval::CriticalGigabytes(expected_gb))
                }
                KeyCode::Enter => {
                    self.input.clear();
                    self.status = "Confirmation mismatch. The plan was not applied.".into();
                    Intent::None
                }
                _ => Intent::None,
            },
            ConfirmationKind::TrashPurge { expected_gb } => match key {
                KeyCode::Char(value)
                    if value.is_ascii_alphanumeric() || value == ' ' || value == '-' =>
                {
                    self.input.push(value);
                    Intent::None
                }
                KeyCode::Backspace => {
                    self.input.pop();
                    Intent::None
                }
                KeyCode::Enter if self.input == format!("PURGE {expected_gb}") => {
                    self.approve(Approval::TrashPurgeGigabytes(expected_gb))
                }
                KeyCode::Enter => {
                    self.input.clear();
                    self.status = "Confirmation mismatch. Trash was not purged.".into();
                    Intent::None
                }
                _ => Intent::None,
            },
        }
    }

    fn approve(&self, approval: Approval) -> Intent {
        self.operation.map_or(Intent::None, |operation| {
            Intent::Apply(ApprovedPlan {
                operation,
                findings: self.effective_findings(),
                approval,
            })
        })
    }
}

pub fn run(ctx: &Ctx) -> Result<ExitCode> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("the TUI requires an interactive stdin and stdout terminal");
    }
    let mut terminal = ratatui::try_init().context("cannot initialize TUI terminal")?;
    let result = run_loop(&mut terminal, ctx);
    let restore = ratatui::try_restore().context("cannot restore terminal after TUI exit");
    match (result, restore) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(code), Ok(())) => Ok(code),
    }
}

/// Keys typed while a scan or apply blocked the loop were aimed at a screen
/// that no longer exists. Delivering them afterwards would let type-ahead
/// toggle permanent mode and approve a plan before it was ever displayed.
fn discard_pending_input() -> Result<()> {
    while event::poll(std::time::Duration::ZERO).context("cannot poll terminal input")? {
        event::read().context("cannot read terminal input")?;
    }
    Ok(())
}

fn run_loop(terminal: &mut DefaultTerminal, ctx: &Ctx) -> Result<ExitCode> {
    let mut app = App::default();
    loop {
        terminal.draw(|frame| render(frame, &app))?;
        let Event::Key(key) = event::read().context("cannot read terminal input")? else {
            continue;
        };
        let area = terminal.size().context("cannot inspect terminal size")?;
        match handle_visible_key(&mut app, key, area.into()) {
            Intent::None => {}
            Intent::Quit => {
                return Ok(if app.failed {
                    ExitCode::from(1)
                } else {
                    ExitCode::SUCCESS
                });
            }
            Intent::Load(operation) => {
                app.begin_load(operation);
                terminal.draw(|frame| render(frame, &app))?;
                load_operation(&mut app, operation, ctx);
                discard_pending_input()?;
            }
            Intent::Apply(plan) => {
                app.screen = Screen::Loading;
                app.status = "Applying only the exact previewed findings…".into();
                terminal.draw(|frame| render(frame, &app))?;
                apply_operation(&mut app, ctx, plan);
                discard_pending_input()?;
            }
        }
    }
}

fn load_operation(app: &mut App, operation: Operation, ctx: &Ctx) {
    ctx.take_diagnostics();
    match operation {
        Operation::ScanAll => {
            let result = ops::scan_all(ctx);
            let warnings = ctx.take_diagnostics();
            app.finish_results(operation, result.findings, result.errors, warnings);
        }
        Operation::Clean(target) => {
            let cleanup = ops::for_target(target);
            match cleanup.scan(ctx, &ops::project::ScanObservations::default()) {
                Ok(mut findings) => {
                    ops::filter_protected_findings(&mut findings, ctx);
                    let warnings = ctx.take_diagnostics();
                    app.finish_results(operation, findings, Vec::new(), warnings);
                }
                Err(error) => {
                    app.fail(error);
                    app.warnings = ctx.take_diagnostics();
                }
            }
        }
        Operation::Purge => {
            match ops::purge::Purge.scan(ctx, &ops::project::ScanObservations::default()) {
                Ok(mut findings) => {
                    ops::filter_protected_findings(&mut findings, ctx);
                    let warnings = ctx.take_diagnostics();
                    app.finish_results(operation, findings, Vec::new(), warnings);
                }
                Err(error) => {
                    app.fail(error);
                    app.warnings = ctx.take_diagnostics();
                }
            }
        }
        Operation::Icloud => match ops::icloud_status(ctx) {
            Ok(findings) => {
                let warnings = ctx.take_diagnostics();
                app.finish_results(operation, findings, Vec::new(), warnings);
            }
            Err(error) => {
                app.fail(error);
                app.warnings = ctx.take_diagnostics();
            }
        },
        Operation::TrashEmpty => match ops::trash_findings(ctx) {
            Ok(mut findings) => {
                ops::filter_protected_findings(&mut findings, ctx);
                let warnings = ctx.take_diagnostics();
                app.finish_results(operation, findings, Vec::new(), warnings);
            }
            Err(error) => {
                app.fail(error);
                app.warnings = ctx.take_diagnostics();
            }
        },
    }
}

fn handle_visible_key(app: &mut App, key: KeyEvent, area: Rect) -> Intent {
    if !terminal_too_small(area) {
        return app.handle_key(key);
    }
    if key.kind != KeyEventKind::Press {
        return Intent::None;
    }
    if key.modifiers.contains(event::KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Intent::Quit;
    }
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => Intent::Quit,
        _ => Intent::None,
    }
}

fn approval_matches(operation: Operation, findings: &[Finding], approval: Approval) -> bool {
    if operation == Operation::TrashEmpty {
        let expected = report::actionable_bytes(findings) / (1024 * 1024 * 1024);
        return approval == Approval::TrashPurgeGigabytes(expected);
    }
    match (
        safety::confirmation_requirement(safety::plan_danger(findings), findings),
        approval,
    ) {
        (ConfirmationRequirement::YesNo { .. }, Approval::Yes) => true,
        (
            ConfirmationRequirement::TypedGigabytes { expected, .. },
            Approval::CriticalGigabytes(actual),
        ) => actual == expected,
        _ => false,
    }
}

fn approved_plan_matches(app: &App, plan: &ApprovedPlan) -> bool {
    app.operation == Some(plan.operation)
        && app.effective_findings() == plan.findings
        && approval_matches(plan.operation, &plan.findings, plan.approval)
}

fn apply_operation(app: &mut App, ctx: &Ctx, plan: ApprovedPlan) {
    if !approved_plan_matches(app, &plan) {
        app.fail(anyhow::anyhow!(
            "confirmation does not authorize the current preview"
        ));
        return;
    }
    let ApprovedPlan {
        operation,
        findings,
        approval,
    } = plan;

    let result = match operation {
        Operation::Clean(target) => ops::for_target(target).apply(&findings, ctx),
        Operation::Purge => ops::purge::Purge.apply(&findings, ctx),
        Operation::TrashEmpty => apply_trash(ctx, &findings, approval),
        Operation::ScanAll | Operation::Icloud => {
            Err(anyhow::anyhow!("refusing to apply a read-only operation"))
        }
    };
    match result {
        Ok(mut outcome) => {
            outcome.errors.extend(ctx.take_journal_errors());
            if !outcome.errors.is_empty() {
                app.failed = true;
            }
            app.summary = Some(outcome.summary);
            app.errors = outcome.errors;
            app.screen = Screen::Outcome;
            app.status = if app.errors.is_empty() {
                "Apply completed. Review the truthful summary below.".into()
            } else if app
                .summary
                .as_ref()
                .is_some_and(|summary| summary.items_touched == 0)
            {
                "Apply failed before any item was changed.".into()
            } else {
                "Apply stopped after an error; earlier successes remain reported.".into()
            };
        }
        Err(error) => app.fail(error),
    }
}

fn apply_trash(ctx: &Ctx, findings: &[Finding], approval: Approval) -> Result<ApplyOutcome> {
    let Approval::TrashPurgeGigabytes(confirm_gb) = approval else {
        bail!("Trash purge requires its typed size acknowledgment");
    };
    safety::trash_gate(findings, Some(confirm_gb))?;
    ops::purge_trash(findings, ctx)
}

fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if terminal_too_small(area) {
        let message = Paragraph::new(Text::from(vec![
            Line::styled(
                "devtrim — terminal too small",
                app.theme.bold(Token::Warning),
            ),
            Line::raw(format!(
                "Need at least {MIN_WIDTH}×{MIN_HEIGHT}; current {}×{}.",
                area.width, area.height
            )),
            Line::raw("Resize the terminal, or press q to quit."),
        ]))
        .alignment(Alignment::Center)
        .block(Block::bordered().title(" Safe disk hygiene "));
        frame.render_widget(message, area);
        return;
    }

    let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(8),
        Constraint::Length(4),
    ])
    .areas(area);
    render_header(frame, header, app);
    match app.screen {
        Screen::Menu => render_menu(frame, body, app),
        Screen::Loading => render_loading(frame, body, app),
        Screen::Results | Screen::Confirm => render_results(frame, body, app),
        Screen::Outcome => render_outcome(frame, body, app),
        Screen::Error => render_error(frame, body, app),
    }
    render_footer(frame, footer, app);
    if app.screen == Screen::Confirm {
        render_confirmation(frame, area, app);
    }
    if app.help {
        render_help(frame, area, app);
    }
}

/// Complete keybinding reference, the second tier of progressive disclosure
/// behind the footer's few contextual keys.
fn render_help(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = vec![
        Line::styled("Keys", app.theme.bold(Token::Accent)),
        Line::raw(""),
    ];
    for (group, keys) in HELP_KEYS {
        lines.push(Line::styled(
            *group,
            app.theme.style(Token::AccentSecondary),
        ));
        for (key, description) in *keys {
            lines.push(Line::from(vec![
                Span::styled(format!("  {key:<12}"), app.theme.style(Token::Muted)),
                Span::raw(*description),
            ]));
        }
        lines.push(Line::raw(""));
    }
    lines.push(Line::styled(
        "Nothing is applied without an explicit, separate approval.",
        app.theme.style(Token::Warning),
    ));
    // Sized to its own content, not to the confirmation popup's fixed 16 rows:
    // the reference is 17 logical lines and was being clipped from the last
    // binding onward. A reference that hides the keys it advertises is worse
    // than none, because the reader has no way to know it was truncated.
    let popup = centered_rect(area, 72, lines.len());
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Help · ? or Esc closes ")),
        popup,
    );
}

/// Centers a popup wide enough for `max_width` and tall enough for `content`
/// lines plus its border, never exceeding the area it sits in.
pub(crate) fn centered_rect(area: Rect, max_width: u16, content: usize) -> Rect {
    let width = area.width.saturating_sub(4).min(max_width);
    let wanted = u16::try_from(content).unwrap_or(u16::MAX).saturating_add(2);
    let height = wanted.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

/// Grouped by the layer an operator reaches for: universal movement first,
/// then the actions that change what happens.
type HelpGroup = (&'static str, &'static [(&'static str, &'static str)]);
const HELP_KEYS: &[HelpGroup] = &[
    (
        "Move",
        &[
            ("↑/↓, k/j", "navigate lists and scroll output"),
            (
                "PgUp/PgDn",
                "move a page; g/G jump to the first or last row",
            ),
            ("Enter", "open the selected operation"),
            ("b, Esc", "back to the menu"),
        ],
    ),
    (
        "Act",
        &[
            ("Space", "leave the highlighted item out, or add it back"),
            ("A", "select every item, or none"),
            ("a", "apply the selected items of the previewed plan"),
            ("s", "toggle Trash-first and permanent deletion"),
            ("r", "rescan the current operation"),
        ],
    ),
    (
        "Session",
        &[("?", "open or close this reference"), ("q, Ctrl+C", "quit")],
    ),
];

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let operation = app.operation.map_or("choose an operation", Operation::name);
    let line = Line::from(vec![
        Span::styled(" devtrim ", app.theme.bold(Token::Accent)),
        Span::styled(
            format!("v{}  ", env!("CARGO_PKG_VERSION")),
            app.theme.style(Token::Muted),
        ),
        Span::raw(operation),
    ]);
    frame.render_widget(
        Paragraph::new(line).block(Block::bordered().title(" measure · classify · trim ")),
        area,
    );
}

fn render_menu(frame: &mut Frame, area: Rect, app: &App) {
    let [menu_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(46), Constraint::Percentage(54)]).areas(area);
    let items = MENU.iter().map(|item| {
        let marker = if item.operation.read_only() {
            "READ-ONLY"
        } else if item.operation == Operation::TrashEmpty {
            "PERMANENT"
        } else {
            "PREVIEW"
        };
        ListItem::new(Line::from(vec![
            Span::styled(format!(" {} ", item.key), app.theme.style(Token::Muted)),
            Span::raw(item.label),
            Span::styled(
                format!("  {marker}"),
                app.theme.style(if marker == "PERMANENT" {
                    Token::Critical
                } else if marker == "READ-ONLY" {
                    Token::Info
                } else {
                    Token::Success
                }),
            ),
        ]))
    });
    let list = List::new(items)
        .block(Block::bordered().title(" Operations "))
        .highlight_symbol("▶ ")
        .highlight_style(app.theme.bold(Token::Accent));
    let mut state = ListState::default();
    state.select(Some(app.selected));
    frame.render_stateful_widget(list, menu_area, &mut state);

    let selected = &MENU[app.selected];
    let detail = Text::from(vec![
        Line::styled(selected.label, app.theme.bold(Token::Accent)),
        Line::raw(""),
        Line::raw(selected.description),
        Line::raw(""),
        Line::styled(
            if selected.operation.read_only() {
                "No mutation is available from this screen."
            } else {
                "Selecting this operation scans first. Apply is a separate, explicit step."
            },
            app.theme.style(Token::Warning),
        ),
        Line::raw(""),
        Line::raw("↑/↓ or j/k navigate · Enter opens · menu key opens directly · ? all keys"),
    ]);
    frame.render_widget(
        Paragraph::new(detail)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Selected ")),
        detail_area,
    );
}

fn render_loading(frame: &mut Frame, area: Rect, app: &App) {
    frame.render_widget(
        Paragraph::new(app.status.as_str())
            .alignment(Alignment::Center)
            .block(Block::bordered().title(" Working ")),
        area,
    );
}

fn render_results(frame: &mut Frame, area: Rect, app: &App) {
    let plan = app.effective_findings();
    let total = report::actionable_bytes(&plan);
    let danger = safety::plan_danger(&plan);
    let mode = if app.shred {
        "PERMANENT"
    } else {
        "TRASH-FIRST"
    };
    let choices = app.choice_count();
    // The mode is a safety signal, so it leads: counts and sizes grow with the
    // plan and would otherwise push it past the minimum width.
    let title = if choices > 0 {
        format!(
            " {mode} · {}/{choices} selected · {} · danger-{danger} ",
            choices.saturating_sub(app.excluded.len()),
            report::gb(total),
        )
    } else {
        format!(
            " {mode} · {} finding(s) · {} actionable · danger-{danger} ",
            app.findings.len(),
            report::gb(total),
        )
    };

    // The detail pane is the only place a long path appears whole, so it grows
    // past its usual share to hold the highlighted row, leaving the list one
    // row, and says how many lines a terminal too short for it still hides.
    let mut details = detail_lines(app, usize::from(area.width.saturating_sub(2)));
    let usual = (area.height / 3).clamp(5, 10);
    let most = area.height.saturating_sub(3).max(usual);
    let wanted = u16::try_from(details.len()).map_or(u16::MAX, |rows| rows.saturating_add(2));
    let detail_height = wanted.clamp(usual, most);
    let room = usize::from(detail_height.saturating_sub(2));
    if details.len() > room {
        let shown = room.saturating_sub(1);
        let hidden = details.len() - shown;
        details.truncate(shown);
        details.push(Line::styled(
            format!("… {hidden} more line(s); enlarge the terminal"),
            app.theme.style(Token::Warning),
        ));
    }
    let [list_area, detail_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(detail_height)]).areas(area);
    let visible = usize::from(list_area.height.saturating_sub(2)).max(1);
    let rows = app.row_count();
    let mut offset = app.list_offset.get();
    if app.cursor < offset {
        offset = app.cursor;
    } else if app.cursor >= offset.saturating_add(visible) {
        offset = app.cursor + 1 - visible;
    }
    offset = offset.min(rows.saturating_sub(visible));
    app.list_offset.set(offset);

    let width = usize::from(list_area.width.saturating_sub(2));
    let mut lines = Vec::with_capacity(visible);
    if rows == 0 {
        lines.push(Line::styled(
            "No findings.",
            app.theme.style(Token::Success),
        ));
    }
    for row in offset..rows.min(offset.saturating_add(visible)) {
        lines.push(result_row(app, row, width));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(Block::bordered().title(title)),
        list_area,
    );
    frame.render_widget(
        Paragraph::new(Text::from(details)).block(Block::bordered().title(" Details ")),
        detail_area,
    );
}

/// One results row: a finding with its selection mark, or a scan error or
/// warning. Rows are clipped to one line; the detail pane shows the
/// highlighted row whole, or says how much of it a short terminal hides.
fn result_row(app: &App, row: usize, width: usize) -> Line<'static> {
    let marker = if row == app.cursor {
        Span::styled("› ", app.theme.bold(Token::Accent))
    } else {
        Span::raw("  ")
    };
    let Some(finding) = app
        .findings
        .get(row)
        .map(|finding| effective_finding(app, finding))
    else {
        let (text, token) =
            diagnostic_at(app, row).unwrap_or_else(|| (String::new(), Token::Warning));
        return Line::from(vec![
            marker,
            Span::styled(report::terminal_safe(&text), app.theme.style(token)),
        ]);
    };
    let check = match (app.is_selectable(row), app.excluded.contains(&row)) {
        (false, _) => "    ",
        (true, true) => "[ ] ",
        (true, false) => "[x] ",
    };
    // `style`, not `bold`: adding BOLD to every level collapses the monochrome
    // ladder, because moderate carries no modifier and high carries BOLD. The
    // theme's own test cannot see this — it exercises `style()` — so the
    // distinction has to be preserved at the call site.
    let danger = format!("{:>3}. danger-{} ", row + 1, finding.danger);
    let size = format!(
        "{:>8}  {:<8} ",
        report::gb(finding.size_bytes),
        action_label(&finding.action)
    );
    let label = report::terminal_safe(&finding.label);
    let used = 2 + check.len() + danger.len() + size.len() + Span::raw(label.as_str()).width() + 2;
    let path = report::terminal_safe(finding.path.as_deref().unwrap_or("command action"));
    Line::from(vec![
        marker,
        Span::raw(check),
        Span::styled(danger, app.theme.style(danger_token(finding.danger))),
        Span::styled(size, app.theme.style(Token::AccentSecondary)),
        Span::styled(label, Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("  {}", truncate_left(&path, width.saturating_sub(used))),
            app.theme.style(Token::Muted),
        ),
    ])
}

/// A finding as the current Trash/permanent mode would apply it — its action
/// and its danger both — through the one policy the plan itself uses.
fn effective_finding(app: &App, finding: &Finding) -> Finding {
    let mut effective = [finding.clone()];
    report::effective_actions(&mut effective, app.shred);
    let [finding] = effective;
    finding
}

/// The scan error or warning shown on a row past the findings.
fn diagnostic_at(app: &App, row: usize) -> Option<(String, Token)> {
    let index = row.checked_sub(app.findings.len())?;
    match app.errors.get(index) {
        Some(error) => Some((format!("error: {error}"), Token::Critical)),
        None => app
            .warnings
            .get(index - app.errors.len())
            .map(|warning| (warning.clone(), Token::Warning)),
    }
}

/// Everything about the highlighted row, wrapped to `width` columns. The
/// selection state precedes the path, the part most likely to run long.
fn detail_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(finding) = app
        .findings
        .get(app.cursor)
        .map(|finding| effective_finding(app, finding))
    {
        lines.push((
            report::terminal_safe(&finding.label),
            app.theme.bold(Token::Accent),
        ));
        lines.push((
            format!(
                "{} · danger-{} · {}",
                report::gb(finding.size_bytes),
                finding.danger,
                report::human_action_display(&finding.action)
            ),
            Style::default(),
        ));
        if app.excluded.contains(&app.cursor) {
            lines.push((
                "Left out of this plan; Space adds it back.".to_string(),
                app.theme.style(Token::Warning),
            ));
        }
        lines.push((
            report::terminal_safe(finding.path.as_deref().unwrap_or("command action")),
            Style::default(),
        ));
        lines.push((
            report::terminal_safe(&finding.note),
            app.theme.style(Token::Muted),
        ));
        if let Some(project) = &finding.project {
            lines.push((
                format!("project: {}", report::terminal_safe(project)),
                Style::default(),
            ));
        }
    } else if let Some((text, _)) = diagnostic_at(app, app.cursor) {
        lines.push((report::terminal_safe(&text), Style::default()));
    } else {
        lines.push(("No findings.".to_string(), app.theme.style(Token::Success)));
    }
    lines
        .into_iter()
        .flat_map(|(text, style)| {
            wrap_columns(&text, width)
                .into_iter()
                .map(move |row| Line::styled(row, style))
        })
        .collect()
}

/// Splits `text` into rows of at most `max` terminal columns, breaking where
/// the next grapheme would pass the edge, or before a space that would take a
/// row's last cell. It walks graphemes as the renderer draws and measures
/// them, so a character with its presentation selector or joined sequence is
/// never split or undercounted, and every grapheme, a space included, keeps a
/// cell of its own: nothing is left past the edge for the renderer to clip,
/// and the rows concatenate back to `text` exactly, so a wrapped path reads as
/// it is. Prose may break mid-word; this pane is where a path has to be exact.
/// Only a single grapheme wider than `max` overflows its row.
fn wrap_columns(text: &str, max: usize) -> Vec<String> {
    if max == 0 {
        return Vec::new();
    }
    let span = Span::raw(text);
    let mut rows = Vec::new();
    let mut start = 0;
    let mut used = 0;
    let mut graphemes = span.styled_graphemes(Style::default()).peekable();
    while let Some(grapheme) = graphemes.next() {
        // Each grapheme borrows `text`, so its address is its byte offset; the
        // iterator skips control characters, so a running total could drift.
        let index = grapheme.symbol.as_ptr() as usize - text.as_ptr() as usize;
        let columns = Span::raw(grapheme.symbol).width();
        // A space in a row's last cell reads as padding and joins the names on
        // either side, so when something follows it opens the next row, where
        // its indent shows.
        let edge_space =
            grapheme.symbol == " " && used + columns == max && graphemes.peek().is_some();
        if (used + columns > max || edge_space) && index > start {
            rows.push(text[start..index].to_string());
            start = index;
            used = 0;
        }
        used += columns;
    }
    rows.push(text[start..].to_string());
    rows
}

/// Keeps the end of `text`, where a path's distinguishing leaf is, within
/// `max` terminal columns.
fn truncate_left(text: &str, max: usize) -> String {
    let width = |value: &str| Span::raw(value).width();
    if width(text) <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut kept = 0;
    let mut start = text.len();
    for (index, character) in text.char_indices().rev() {
        let character_width = width(&text[index..index + character.len_utf8()]);
        if kept + character_width + 1 > max {
            break;
        }
        kept += character_width;
        start = index;
    }
    format!("…{}", &text[start..])
}

fn render_outcome(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = Vec::new();
    if let Some(summary) = &app.summary {
        lines.push(Line::styled(
            report::terminal_safe(&report::summary_headline(summary)),
            app.theme
                .bold(match (app.errors.is_empty(), summary.items_touched) {
                    (true, _) => Token::Success,
                    (false, 0) => Token::Critical,
                    (false, _) => Token::Warning,
                }),
        ));
        if !app.excluded.is_empty() {
            lines.push(Line::styled(
                format!(
                    "{} left out of this plan and not touched.",
                    app.excluded.len()
                ),
                app.theme.style(Token::Muted),
            ));
        }
        lines.push(Line::raw(""));
        for note in &summary.notes {
            lines.push(Line::raw(format!("• {}", report::terminal_safe(note))));
        }
    }
    if !app.errors.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled("Errors", app.theme.bold(Token::Critical)));
        for error in &app.errors {
            lines.push(Line::raw(format!("• {}", report::terminal_safe(error))));
        }
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .scroll((app.scroll, 0))
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Apply outcome ")),
        area,
    );
}

fn render_error(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = vec![Line::styled(
        "Operation refused or failed",
        app.theme.bold(Token::Critical),
    )];
    for error in &app.errors {
        lines.push(Line::raw(report::terminal_safe(error)));
    }
    if !app.warnings.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Warnings captured before the failure",
            app.theme.style(Token::Warning),
        ));
        for warning in &app.warnings {
            lines.push(Line::raw(report::terminal_safe(warning)));
        }
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .scroll((app.scroll, 0))
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Failed closed ")),
        area,
    );
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    // Progressive disclosure: the footer carries only the few keys that matter
    // on this screen, and `?` opens the complete reference. Listing everything
    // here would make the one key the operator needs harder to find.
    let keys = match app.screen {
        Screen::Menu => "↑/↓ navigate · Enter select · ? keys · q quit",
        Screen::Results => {
            if app.choice_count() == 0 {
                "↑/↓ move · r rescan · b back · ? keys"
            } else if app.can_toggle_shred() {
                "Space select · A all · a apply · s permanent · b back · ? keys"
            } else {
                "Space select · A all · a apply · r rescan · b back · ? keys"
            }
        }
        Screen::Confirm => "Esc cancel · type the exact requested acknowledgment",
        Screen::Outcome | Screen::Error => "↑/↓ or j/k scroll · b back to menu · ? keys",
        Screen::Loading => "Scanning and apply are synchronous; please wait",
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(keys, app.theme.style(Token::AccentSecondary)),
            Line::styled(app.status.as_str(), app.theme.style(Token::Warning)),
        ])
        .block(Block::bordered()),
        area,
    );
}

fn render_confirmation(frame: &mut Frame, area: Rect, app: &App) {
    let Some(confirmation) = app.confirmation else {
        return;
    };
    let popup = centered_rect(area, 100, 14);
    frame.render_widget(Clear, popup);
    let prompt = match confirmation {
        ConfirmationKind::YesNo { danger } => {
            format!("Danger-{danger}. Press y to apply this exact plan, or n/Esc to cancel.")
        }
        ConfirmationKind::Critical {
            danger,
            expected_gb,
        } => format!(
            "Danger-{danger} permanent action. Type {expected_gb} and press Enter. Esc cancels."
        ),
        ConfirmationKind::TrashPurge { expected_gb } => format!(
            "Trash purge is permanent. Type PURGE {expected_gb} and press Enter. Esc cancels."
        ),
    };
    // Consent is given here, so this screen — not only the title behind it —
    // says how much of the preview the approval covers.
    let choices = app.choice_count();
    let coverage = if choices > 0 {
        Line::raw(format!(
            "This plan: {} of {choices} selected · {} left out",
            choices.saturating_sub(app.excluded.len()),
            app.excluded.len()
        ))
    } else {
        Line::raw("")
    };
    let text = Text::from(vec![
        Line::styled("DATA-LOSS WARNING", app.theme.bold(Token::Critical)),
        coverage,
        Line::raw(safety::DATA_LOSS_NOTICE),
        Line::raw(""),
        Line::styled(prompt, app.theme.style(Token::Warning)),
        Line::raw(""),
        Line::from(vec![
            Span::raw("> "),
            Span::styled(app.input.as_str(), app.theme.bold(Token::Accent)),
        ]),
    ]);
    frame.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Confirm exact plan ")),
        popup,
    );
}

fn action_label(action: &Action) -> &'static str {
    match action {
        Action::Trash => "TRASH",
        Action::Shred => "SHRED",
        Action::Command { .. } => "COMMAND",
        Action::Info => "INFO",
        Action::None => "EXCLUDED",
    }
}

fn terminal_too_small(area: Rect) -> bool {
    area.width < MIN_WIDTH || area.height < MIN_HEIGHT
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn rendered(app: &App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .join("")
    }

    /// A cleanup category registered in `ops::all()` but absent from `MENU` is
    /// reachable from the CLI and invisible in the interface. Nothing else
    /// catches that: `MenuItem` is a hand-written list with no exhaustiveness
    /// check, which is exactly how `installers` shipped unreachable.
    #[test]
    fn every_cleanup_target_is_reachable_from_the_menu() {
        use clap::ValueEnum;

        for target in Target::value_variants() {
            assert!(
                MENU.iter()
                    .any(|item| item.operation == Operation::Clean(*target)),
                "`{}` has no menu entry, so the TUI cannot reach it",
                target.as_str()
            );
        }
    }

    #[test]
    fn menu_keys_are_unique() {
        let mut keys: Vec<_> = MENU.iter().map(|item| item.key).collect();
        keys.sort_unstable();
        let count = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), count, "two menu entries share a key");
    }

    /// Structural guard for the monochrome baseline. Every screen, including
    /// both overlays, must render with no cell carrying a color once the theme
    /// says color is unavailable. A single inline `Color::` literal
    /// reintroduced anywhere in the render path fails here, which the theme's
    /// own unit tests cannot see.
    #[test]
    fn monochrome_theme_leaves_no_colored_cell_on_any_screen() {
        use crate::theme::ColorSupport;
        use ratatui::style::Color;

        let screens = [
            Screen::Menu,
            Screen::Loading,
            Screen::Results,
            Screen::Confirm,
            Screen::Outcome,
            Screen::Error,
        ];
        for screen in screens {
            for help in [false, true] {
                let app = App {
                    screen,
                    help,
                    theme: Theme::new(ColorSupport::None),
                    errors: vec!["a scan error".into()],
                    warnings: vec!["a warning".into()],
                    confirmation: Some(ConfirmationKind::Critical {
                        danger: 9,
                        expected_gb: 12,
                    }),
                    ..App::default()
                };
                let backend = TestBackend::new(100, 30);
                let mut terminal = Terminal::new(backend).unwrap();
                terminal.draw(|frame| render(frame, &app)).unwrap();
                for cell in terminal.backend().buffer().content() {
                    assert_eq!(
                        cell.fg,
                        Color::Reset,
                        "{screen:?} (help={help}) painted a color under NO_COLOR"
                    );
                    assert_eq!(
                        cell.bg,
                        Color::Reset,
                        "{screen:?} (help={help}) painted a background under NO_COLOR"
                    );
                }
            }
        }
    }

    /// The theme's ladder test proves `style()` keeps danger levels distinct;
    /// it cannot see a render site that adds the same modifier to all of them.
    /// This asserts the property where it actually has to hold — on the painted
    /// screen — which is where it was in fact broken.
    #[test]
    fn monochrome_results_screen_keeps_danger_levels_distinguishable() {
        use crate::theme::ColorSupport;

        let levels = [1u8, 4, 7, 10];
        let findings: Vec<Finding> = levels
            .iter()
            .map(|danger| {
                Finding::new(
                    format!("finding {danger}"),
                    None,
                    1024,
                    "note",
                    *danger,
                    Action::Trash,
                )
            })
            .collect();
        let app = App {
            screen: Screen::Results,
            findings,
            theme: Theme::new(ColorSupport::None),
            ..App::default()
        };
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let width = usize::from(buffer.area.width);
        let height = usize::from(buffer.area.height);
        let content = buffer.content();

        let modifiers: Vec<_> = levels
            .iter()
            .enumerate()
            .map(|(position, danger)| {
                // Located by CELL COLUMN, never by byte offset: rows begin with
                // a multi-byte border glyph, so `str::find` points at the wrong
                // cell. The row number anchors the match to a finding row —
                // the panel title also contains `danger-N`, rendered with no
                // modifier, and matched first without it.
                let needle = format!("{}. danger-{danger} ", position + 1);
                (0..height)
                    .find_map(|y| {
                        let symbols: Vec<&str> = (0..width)
                            .map(|x| content[y * width + x].symbol())
                            .collect();
                        (0..width).find_map(|start| {
                            symbols[start..]
                                .concat()
                                .starts_with(&needle)
                                .then(|| content[y * width + start].modifier)
                        })
                    })
                    .unwrap_or_else(|| panic!("{needle} was never rendered"))
            })
            .collect();

        for (index, modifier) in modifiers.iter().enumerate() {
            for other in modifiers.iter().skip(index + 1) {
                assert_ne!(
                    modifier, other,
                    "danger levels must stay distinguishable under NO_COLOR: {modifiers:?}"
                );
            }
        }
    }

    /// Positive control for the guard above: with color available the same
    /// screens must actually paint something, otherwise the monochrome
    /// assertion would pass over an interface that never colors anything.
    #[test]
    fn colored_theme_still_paints_the_results_screen() {
        use crate::theme::ColorSupport;
        use ratatui::style::Color;

        let app = App {
            screen: Screen::Results,
            errors: vec!["a scan error".into()],
            // Pinned, not `from_env`: this is the control that proves the
            // monochrome assertion is testing something, and it would pass
            // vacuously — or fail — in a shell that exports NO_COLOR.
            theme: Theme::new(ColorSupport::Named),
            ..App::default()
        };
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        assert!(
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .any(|cell| cell.fg != Color::Reset),
            "the colored theme must paint at least one cell"
        );
    }

    #[test]
    fn help_overlay_opens_and_closes_without_reaching_the_screen_beneath() {
        for closing in [KeyCode::Char('?'), KeyCode::Esc, KeyCode::Char('q')] {
            let mut app = App::default();
            assert_eq!(app.handle_key(key(KeyCode::Char('?'))), Intent::None);
            assert!(app.help);
            // While open, a key that would otherwise launch an operation must be
            // swallowed rather than acted on behind the overlay.
            assert_eq!(app.handle_key(key(KeyCode::Char('4'))), Intent::None);
            assert!(app.help, "an unrelated key must not close the overlay");
            assert_eq!(app.selected, 0, "the menu must not move behind the overlay");

            assert_eq!(app.handle_key(key(closing)), Intent::None);
            assert!(!app.help, "{closing:?} must close the overlay");
        }
    }

    #[test]
    fn help_overlay_renders_the_full_key_reference() {
        let mut app = App::default();
        app.handle_key(key(KeyCode::Char('?')));
        let frame = rendered(&app, 100, 30);
        assert!(frame.contains("Help"));
        for (_, keys) in HELP_KEYS {
            for (_, description) in *keys {
                assert!(
                    frame.contains(description),
                    "help overlay must document: {description}"
                );
            }
        }
        // The clipping canary, and the reason the loop above is not enough:
        // the footer also contains the word "quit", so a truncated overlay
        // satisfied every description assertion while hiding its last binding.
        // This line is rendered last, so if it is visible nothing above it was
        // cut.
        assert!(
            frame.contains("Nothing is applied without an explicit"),
            "the overlay's final line must be visible, or bindings above it are hidden too"
        );
    }

    /// The overlay must fit its own content at the supported minimum size, not
    /// merely on a roomy terminal.
    #[test]
    fn help_overlay_is_sized_to_its_content() {
        let area = Rect::new(0, 0, 100, 40);
        let popup = centered_rect(area, 72, 17);
        assert!(popup.width <= area.width && popup.height <= area.height);

        // Never larger than the space available.
        let cramped = centered_rect(Rect::new(0, 0, 40, 10), 72, 17);
        assert!(cramped.height <= 8);
        assert!(cramped.width <= 36);
    }

    /// The confirmation screen demands an exact typed acknowledgment, so a
    /// second overlay must never cover the plan being approved.
    #[test]
    fn help_overlay_never_opens_over_a_confirmation() {
        let mut app = App {
            screen: Screen::Confirm,
            confirmation: Some(ConfirmationKind::YesNo { danger: 5 }),
            ..App::default()
        };
        app.handle_key(key(KeyCode::Char('?')));
        assert!(!app.help);
    }

    #[test]
    fn menu_supports_vim_navigation_and_direct_numbers() {
        let mut app = App::default();
        assert_eq!(app.handle_key(key(KeyCode::Char('j'))), Intent::None);
        assert_eq!(app.selected, 1);
        assert_eq!(
            app.handle_key(key(KeyCode::Char('4'))),
            Intent::Load(Operation::Clean(Target::Artifacts))
        );
        assert!(!Operation::Clean(Target::Artifacts).read_only());
        assert_eq!(
            app.handle_key(key(KeyCode::Char('0'))),
            Intent::Load(Operation::TrashEmpty)
        );
        assert_eq!(app.selected, MENU.len() - 1);
    }

    #[test]
    fn low_danger_apply_requires_explicit_yes() {
        let mut app = App::default();
        app.finish_results(
            Operation::Clean(Target::Caches),
            vec![Finding::new(
                "cache",
                None,
                1,
                "test",
                2,
                Action::command("test", &[]),
            )],
            Vec::new(),
            Vec::new(),
        );
        app.begin_confirmation();
        assert_eq!(
            app.confirmation,
            Some(ConfirmationKind::YesNo { danger: 2 })
        );
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Intent::None);
        assert!(matches!(
            app.handle_key(key(KeyCode::Char('y'))),
            Intent::Apply(ApprovedPlan {
                approval: Approval::Yes,
                ..
            })
        ));
    }

    #[test]
    fn permanent_apply_rejects_mismatched_typed_size() {
        let mut app = App::default();
        app.finish_results(
            Operation::Clean(Target::Caches),
            vec![Finding::new("cache", None, 1024, "test", 2, Action::Trash)],
            Vec::new(),
            Vec::new(),
        );
        app.shred = true;
        app.begin_confirmation();
        assert!(matches!(
            app.confirmation,
            Some(ConfirmationKind::Critical { expected_gb: 0, .. })
        ));
        app.input.push('1');
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Intent::None);
        assert!(app.input.is_empty());
        assert!(app.status.contains("mismatch"));
    }

    #[test]
    fn trash_purge_requires_exact_phrase() {
        let mut app = App::default();
        app.finish_results(
            Operation::TrashEmpty,
            vec![Finding::new(
                "Trash item",
                Some(std::path::PathBuf::from("/Users/example/.Trash/item")),
                1,
                "test",
                9,
                Action::Shred,
            )],
            Vec::new(),
            Vec::new(),
        );
        app.begin_confirmation();
        assert_eq!(
            app.confirmation,
            Some(ConfirmationKind::TrashPurge { expected_gb: 0 })
        );

        for ch in "PURGE 1".chars() {
            assert_eq!(app.handle_key(key(KeyCode::Char(ch))), Intent::None);
        }
        assert_eq!(app.handle_key(key(KeyCode::Enter)), Intent::None);
        assert!(app.input.is_empty());
        assert!(app.status.contains("mismatch"));

        for ch in "PURGE 0".chars() {
            assert_eq!(app.handle_key(key(KeyCode::Char(ch))), Intent::None);
        }
        assert!(matches!(
            app.handle_key(key(KeyCode::Enter)),
            Intent::Apply(ApprovedPlan {
                approval: Approval::TrashPurgeGigabytes(0),
                ..
            })
        ));
    }

    #[test]
    fn trash_preview_filters_protected_items_before_approval() {
        let root = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-tui-trash-protect-{}", std::process::id()));
        crate::ops::remove_test_path(&root);
        let trash = root.join(".Trash");
        std::fs::create_dir_all(&trash).unwrap();
        let ordinary = trash.join("ordinary");
        let protected = trash.join("protected");
        std::fs::write(&ordinary, "visible positive control").unwrap();
        std::fs::write(&protected, "keep").unwrap();
        let home = root.canonicalize().unwrap();
        let mut app = App::default();
        let ctx = Ctx {
            yes: false,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: vec![protected.clone()],
            journal_path: home.join("journal.jsonl"),
            home: home.clone(),
            interactive: true,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };

        load_operation(&mut app, Operation::TrashEmpty, &ctx);

        assert_eq!(
            app.findings.len(),
            1,
            "the ordinary item proves the scan ran"
        );
        assert_eq!(app.findings[0].target(), Some(ordinary.as_path()));
        assert!(app.warnings.iter().any(|warning| {
            warning.contains("skipping protected path") && warning.contains("protected")
        }));
        app.begin_confirmation();
        assert_eq!(
            app.confirmation,
            Some(ConfirmationKind::TrashPurge { expected_gb: 0 })
        );
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn approval_must_match_current_plan_and_operation() {
        let low = vec![Finding::new(
            "cache",
            None,
            1,
            "test",
            2,
            Action::command("test", &[]),
        )];
        assert!(approval_matches(
            Operation::Clean(Target::Caches),
            &low,
            Approval::Yes
        ));
        assert!(!approval_matches(
            Operation::Clean(Target::Caches),
            &low,
            Approval::CriticalGigabytes(0)
        ));
        assert!(!approval_matches(
            Operation::TrashEmpty,
            &low,
            Approval::Yes
        ));

        let mut app = App::default();
        app.finish_results(
            Operation::Clean(Target::Caches),
            low.clone(),
            Vec::new(),
            Vec::new(),
        );
        let Intent::Apply(plan) = app.approve(Approval::Yes) else {
            panic!("expected an approved plan");
        };
        assert!(approved_plan_matches(&app, &plan));
        app.operation = Some(Operation::Clean(Target::Xcode));
        assert!(!approved_plan_matches(&app, &plan));
        app.operation = Some(Operation::Clean(Target::Caches));
        app.findings.push(Finding::new(
            "new target",
            None,
            1,
            "not previewed",
            2,
            Action::command("test", &[]),
        ));
        assert!(!approved_plan_matches(&app, &plan));
    }

    #[test]
    fn approval_is_invalidated_when_shred_mode_changes() {
        let mut app = App::default();
        app.finish_results(
            Operation::Clean(Target::Caches),
            vec![Finding::new("cache", None, 1, "test", 2, Action::Trash)],
            Vec::new(),
            Vec::new(),
        );
        let Intent::Apply(plan) = app.approve(Approval::Yes) else {
            panic!("expected an approved plan");
        };
        assert!(approved_plan_matches(&app, &plan));

        app.shred = true;

        assert!(!approved_plan_matches(&app, &plan));
    }

    #[test]
    fn forged_read_only_plan_fails_closed() {
        let mut app = App::default();
        app.finish_results(
            Operation::ScanAll,
            vec![Finding::new(
                "forged action",
                None,
                1,
                "test",
                2,
                Action::command("test", &[]),
            )],
            Vec::new(),
            Vec::new(),
        );
        let Intent::Apply(plan) = app.approve(Approval::Yes) else {
            panic!("expected a forged approved plan");
        };
        let ctx = Ctx {
            yes: false,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: std::path::PathBuf::from("/tmp/devtrim-tui-test-journal.jsonl"),
            home: std::path::PathBuf::from("/Users/example"),
            interactive: true,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };

        apply_operation(&mut app, &ctx, plan);

        assert_eq!(app.screen, Screen::Error);
        assert!(app.summary.is_none());
        assert!(app.errors[0].contains("refusing to apply a read-only operation"));
    }

    #[test]
    fn control_c_quits_from_every_screen() {
        let mut app = App::default();
        let interrupt = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.handle_key(interrupt), Intent::Quit);
        app.screen = Screen::Confirm;
        assert_eq!(app.handle_key(interrupt), Intent::Quit);
    }

    #[test]
    fn rendered_menu_and_warning_have_non_color_labels() {
        let app = App::default();
        let menu = rendered(&app, 100, 28);
        assert!(menu.contains("READ-ONLY"));
        assert!(menu.contains("PERMANENT"));

        let mut confirm = App::default();
        confirm.finish_results(
            Operation::Clean(Target::Caches),
            vec![Finding::new(
                "cache",
                None,
                1,
                "test",
                2,
                Action::command("test", &[]),
            )],
            Vec::new(),
            Vec::new(),
        );
        confirm.begin_confirmation();
        let warning = rendered(&confirm, 100, 28);
        assert!(warning.contains("DATA-LOSS WARNING"));
        assert!(warning.contains("provided AS IS"));
        assert!(warning.contains("Press y"));
    }

    #[test]
    fn rendered_errors_escape_terminal_controls() {
        let mut app = App::default();
        app.fail(anyhow::anyhow!("bad\u{1b}[2J\nline\u{202e}"));

        let output = rendered(&app, 100, 28);

        assert!(output.contains("bad\\u{1b}[2J\\nline\\u{202e}"));
        assert!(!output.contains('\u{1b}'));
    }

    #[test]
    fn rendered_findings_and_scan_warnings_escape_terminal_controls() {
        let mut app = App::default();
        app.finish_results(
            Operation::ScanAll,
            vec![Finding::new(
                "bad\u{1b}[2J",
                Some(std::path::PathBuf::from("/tmp/line\nnext")),
                0,
                "note\u{202e}",
                1,
                Action::Info,
            )],
            Vec::new(),
            vec!["warn\u{1b}]8;;https://example.com\u{7}".into()],
        );

        let output = rendered(&app, 100, 28);

        assert!(output.contains("bad\\u{1b}[2J"));
        assert!(output.contains("/tmp/line\\nnext"));
        assert!(output.contains("note\\u{202e}"));
        assert!(output.contains("warn\\u{1b}]8;;https://example.com\\u{7}"));
        assert!(!output.contains('\u{1b}'));
    }

    #[test]
    fn confirmation_is_complete_at_the_minimum_terminal_size() {
        let mut app = App::default();
        app.finish_results(
            Operation::TrashEmpty,
            vec![Finding::new(
                "Trash item",
                Some(std::path::PathBuf::from("/Users/example/.Trash/item")),
                1,
                "test",
                9,
                Action::Shred,
            )],
            Vec::new(),
            Vec::new(),
        );
        app.begin_confirmation();

        let output = rendered(&app, MIN_WIDTH, MIN_HEIGHT);

        assert!(output.contains("DATA-LOSS WARNING"));
        assert!(output.contains("Type PURGE 0"));
        assert!(output.contains("> "));
        assert!(!output.contains("terminal too small"));
    }

    #[test]
    fn small_terminal_blocks_hidden_confirmation_input() {
        let mut app = App::default();
        app.finish_results(
            Operation::Clean(Target::Caches),
            vec![Finding::new(
                "cache",
                None,
                1,
                "test",
                2,
                Action::command("test", &[]),
            )],
            Vec::new(),
            Vec::new(),
        );
        app.begin_confirmation();
        let too_small = Rect::new(0, 0, MIN_WIDTH - 1, MIN_HEIGHT);

        assert_eq!(
            handle_visible_key(&mut app, key(KeyCode::Char('y')), too_small),
            Intent::None
        );
        assert_eq!(app.screen, Screen::Confirm);
        assert!(app.input.is_empty());
        assert_eq!(
            handle_visible_key(&mut app, key(KeyCode::Char('q')), too_small),
            Intent::Quit
        );
    }

    #[test]
    fn captured_scanner_diagnostics_are_visible_in_results() {
        let ctx = Ctx {
            yes: false,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: std::path::PathBuf::from("/tmp/devtrim-tui-test-journal.jsonl"),
            home: std::path::PathBuf::from("/Users/example"),
            interactive: true,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };
        ctx.diagnostic("warn", "skipped path\u{1b}[2J");
        let mut app = App::default();
        app.finish_results(
            Operation::ScanAll,
            Vec::new(),
            Vec::new(),
            ctx.take_diagnostics(),
        );

        let output = rendered(&app, 100, 28);

        assert!(output.contains("warn: skipped path\\u{1b}[2J"));
        assert!(!output.contains('\u{1b}'));
    }

    #[test]
    fn partial_apply_error_is_reachable_by_scrolling() {
        let mut app = App {
            screen: Screen::Outcome,
            summary: Some(Summary {
                op: "test".into(),
                items_touched: 25,
                bytes_freed_estimate: 25,
                bytes_trashed_estimate: 0,
                notes: (0..25).map(|index| format!("completed {index}")).collect(),
            }),
            errors: vec!["partial apply failure".into()],
            ..App::default()
        };

        for _ in 0..3 {
            assert_eq!(app.handle_key(key(KeyCode::PageDown)), Intent::None);
        }
        let output = rendered(&app, MIN_WIDTH, MIN_HEIGHT);

        assert_eq!(app.scroll, 24);
        assert!(output.contains("partial apply failure"));
    }

    fn trash(label: &str, size: u64, danger: u8) -> Finding {
        Finding::new(
            label,
            Some(std::path::PathBuf::from(format!("/tmp/{label}"))),
            size,
            "test",
            danger,
            Action::Trash,
        )
    }

    fn cleanup(findings: Vec<Finding>) -> App {
        let mut app = App::default();
        app.finish_results(
            Operation::Clean(Target::Caches),
            findings,
            Vec::new(),
            Vec::new(),
        );
        app
    }

    #[test]
    fn project_purge_is_reachable_from_the_menu() {
        assert!(
            MENU.iter()
                .any(|item| item.operation == Operation::Purge && item.key == "p")
        );
        assert!(!Operation::Purge.read_only());
    }

    /// Deselecting only narrows: a finding left out must never reach the plan
    /// the confirmation approves.
    #[test]
    fn deselected_findings_never_reach_the_approved_plan() {
        let mut app = cleanup(vec![trash("first", 1, 2), trash("second", 1, 2)]);

        assert_eq!(app.handle_key(key(KeyCode::Char(' '))), Intent::None);
        app.begin_confirmation();
        let Intent::Apply(plan) = app.handle_key(key(KeyCode::Char('y'))) else {
            panic!("expected an approved plan");
        };

        let labels: Vec<_> = plan
            .findings
            .iter()
            .map(|finding| finding.label.as_str())
            .collect();
        assert_eq!(
            labels,
            ["second"],
            "PV tui/selection-plan: a deselected finding reached the approved plan"
        );
    }

    #[test]
    fn changing_the_selection_invalidates_an_earlier_approval() {
        let mut app = cleanup(vec![trash("first", 1, 2), trash("second", 1, 2)]);
        let Intent::Apply(plan) = app.approve(Approval::Yes) else {
            panic!("expected an approved plan");
        };
        assert!(approved_plan_matches(&app, &plan));

        app.handle_key(key(KeyCode::Char(' ')));

        assert!(!approved_plan_matches(&app, &plan));
    }

    #[test]
    fn confirmation_strength_is_recomputed_for_the_selected_subset() {
        let mut app = cleanup(vec![trash("critical", 1, 9), trash("routine", 1, 2)]);
        app.begin_confirmation();
        assert!(matches!(
            app.confirmation,
            Some(ConfirmationKind::Critical { .. })
        ));
        app.handle_key(key(KeyCode::Esc));

        app.handle_key(key(KeyCode::Char(' ')));
        app.begin_confirmation();

        assert_eq!(
            app.confirmation,
            Some(ConfirmationKind::YesNo { danger: 2 })
        );
    }

    #[test]
    fn read_only_and_non_actionable_rows_cannot_be_deselected() {
        let mut scan = App::default();
        scan.finish_results(
            Operation::ScanAll,
            vec![trash("reported", 1, 2)],
            Vec::new(),
            Vec::new(),
        );
        scan.handle_key(key(KeyCode::Char(' ')));
        assert!(scan.excluded.is_empty());

        let mut app = cleanup(vec![
            Finding::new("disclosure", None, 1, "test", 0, Action::None),
            trash("deletable", 1, 2),
        ]);
        app.handle_key(key(KeyCode::Char(' ')));
        assert!(
            app.excluded.is_empty(),
            "an excluded disclosure is not a choice"
        );
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Char(' ')));
        assert_eq!(app.excluded, std::collections::BTreeSet::from([1]));
    }

    #[test]
    fn select_all_toggles_between_every_actionable_row_and_none() {
        let mut app = cleanup(vec![trash("a", 1, 2), trash("b", 1, 2), trash("c", 1, 2)]);

        app.handle_key(key(KeyCode::Char('A')));
        assert_eq!(app.excluded.len(), 3);
        app.begin_confirmation();
        assert_eq!(
            app.screen,
            Screen::Results,
            "an empty plan cannot be confirmed"
        );
        assert!(app.status.contains("Nothing is selected"), "{}", app.status);

        app.handle_key(key(KeyCode::Char('A')));
        assert!(app.excluded.is_empty());
    }

    #[test]
    fn results_show_selection_marks_and_the_highlighted_details() {
        let mut app = cleanup(vec![trash("first", 1, 2), trash("second", 1, 2)]);
        app.handle_key(key(KeyCode::Char(' ')));

        let output = rendered(&app, 100, 30);

        assert!(output.contains("[ ]"), "{output}");
        assert!(output.contains("[x]"), "{output}");
        assert!(output.contains("1/2 selected"), "{output}");
        assert!(output.contains("Left out of this plan"), "{output}");
        assert!(output.contains("/tmp/first"), "{output}");
    }

    /// The rendered screen, one string per terminal row.
    fn screen_rows(app: &App, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(usize::from(width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect()
    }

    /// The detail pane's rows without borders or padding, joined, so a path
    /// wrapped across rows reads back whole.
    fn detail_pane_text(rows: &[String]) -> String {
        let top = rows
            .iter()
            .position(|row| row.contains(" Details "))
            .expect("the results screen has a detail pane");
        rows[top + 1..]
            .iter()
            .take_while(|row| !row.starts_with('└'))
            .map(|row| {
                row.chars()
                    .skip(1)
                    .collect::<String>()
                    .trim_end_matches(['│', ' '])
                    .to_string()
            })
            .collect()
    }

    /// The detail pane's cells between its borders, untrimmed and joined, so
    /// a space at a row's edge counts like any other character.
    fn detail_pane_cells(rows: &[String]) -> String {
        let top = rows
            .iter()
            .position(|row| row.contains(" Details "))
            .expect("the results screen has a detail pane");
        rows[top + 1..]
            .iter()
            .take_while(|row| !row.starts_with('└'))
            .map(|row| {
                let cells: Vec<char> = row.chars().collect();
                cells[1..cells.len() - 1].iter().collect::<String>()
            })
            .collect()
    }

    /// The detail pane is the one place a long path is shown whole, so it
    /// grows past its usual share to hold the highlighted finding.
    #[test]
    fn the_detail_pane_grows_to_show_the_highlighted_finding_whole() {
        let project = format!(
            "/Users/someone/dev/{}",
            "a-directory-name-long-enough-to-wrap".repeat(4)
        );
        let path = format!("{project}/node_modules");
        let note = "repo last active 2020-01-01 UTC; no running build uses it";
        let finding = Finding::new(
            "node_modules",
            Some(std::path::PathBuf::from(&path)),
            1,
            note,
            5,
            Action::Trash,
        )
        .with_project(std::path::Path::new(&project));
        let mut app = cleanup(vec![finding, trash("other", 1, 2)]);
        app.handle_key(key(KeyCode::Char(' ')));

        let rows = screen_rows(&app, 100, 30);
        // A row's trailing space is indistinguishable from padding, so compare
        // without spaces; the lines broken after one still read back whole.
        let pane = detail_pane_text(&rows).replace(' ', "");

        let project_line = format!("project: {project}");
        for expected in [
            path.as_str(),
            note,
            "Left out of this plan",
            project_line.as_str(),
        ] {
            assert!(
                pane.contains(&expected.replace(' ', "")),
                "missing {expected:?}: {rows:#?}"
            );
        }
        assert!(!pane.contains("moreline(s)"), "{rows:#?}");
    }

    /// At the minimum size a long path cannot fit, so the pane says how many
    /// lines it hides instead of cutting them silently, and the selection
    /// state comes before the path so it is never among them.
    #[test]
    fn a_terminal_too_short_for_the_details_says_how_many_lines_it_hides() {
        // Label, action, selection state, six path rows at 62 columns, and the
        // note: ten rows for a pane with room for six.
        let path = format!("/{}", "x".repeat(371));
        let finding = Finding::new(
            "node_modules",
            Some(std::path::PathBuf::from(&path)),
            1,
            "stale",
            5,
            Action::Trash,
        );
        let mut app = cleanup(vec![finding]);
        app.handle_key(key(KeyCode::Char(' ')));

        let rows = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT);
        let pane = detail_pane_text(&rows);

        assert!(pane.contains("Left out of this plan"), "{rows:#?}");
        assert!(
            pane.contains("… 5 more line(s); enlarge the terminal"),
            "{rows:#?}"
        );
        assert!(
            rows.iter().any(|row| row.contains("› [ ]")),
            "the highlighted row stays listed: {rows:#?}"
        );
    }

    /// The graphemes the renderer draws for `text`, and the columns it gives
    /// them.
    fn grapheme_columns(text: &str) -> (usize, usize) {
        Span::raw(text)
            .styled_graphemes(Style::default())
            .fold((0, 0), |(count, columns), grapheme| {
                (count + 1, columns + Span::raw(grapheme.symbol).width())
            })
    }

    /// A wrapped path must read back exactly, and no row may hold more than
    /// the pane shows, or the renderer clips it — a space included, since a
    /// clipped space joins two names: multi-byte, double-width and zero-width
    /// characters included, measured per grapheme as the renderer measures
    /// them. A grapheme — a character with its presentation selector or joined
    /// sequence — is never divided between rows. No row may be blank, or it
    /// spends a pane row on nothing.
    #[test]
    fn wrapping_is_lossless_and_keeps_every_row_within_the_width() {
        for (text, max) in [
            ("/Users/someone/dev/a-very-long-project/node_modules", 10),
            (
                "repo last active 2020-01-01 UTC; no running build uses it",
                12,
            ),
            ("naïve/café/日本語/パス/node_modules", 7),
            ("e\u{301}e\u{301}e\u{301}", 1),
            ("☺\u{FE0F}☺\u{FE0F}☺\u{FE0F}", 3),
            ("👨\u{200D}👩\u{200D}👧/family", 4),
            ("/a🇧🇷/b👍🏽", 3),
            // The renderer skips control characters; offsets must not drift.
            ("ab\u{1b}cdé\u{7}fg", 2),
            ("exactly-eleven", 14),
            ("word  spaced   apart", 4),
            ("", 5),
        ] {
            let rows = wrap_columns(text, max);
            assert_eq!(rows.concat(), text, "{rows:?}");
            assert!(!rows.is_empty(), "every line keeps a row");
            let graphemes: usize = rows.iter().map(|row| grapheme_columns(row).0).sum();
            assert_eq!(
                graphemes,
                grapheme_columns(text).0,
                "a grapheme split: {rows:?}"
            );
            for row in &rows {
                assert!(
                    grapheme_columns(row).1 <= max,
                    "{row:?} is wider than {max}: {rows:?}"
                );
                assert!(
                    text.is_empty() || !row.trim().is_empty(),
                    "blank row: {rows:?}"
                );
            }
        }
    }

    /// Selectors and joiners are escaped before display, but a flag or a
    /// skin-tone emoji reaches the pane as one grapheme of two characters. At
    /// the edge it moves to the next row whole rather than splitting there.
    #[test]
    fn a_grapheme_at_the_edge_moves_to_the_next_row_whole() {
        // 61 columns, then a flag at an inner width of 62: one regional
        // indicator fits, the pair does not.
        let path = format!("/{}🇧🇷{}", "a".repeat(60), "b".repeat(10));
        let finding = Finding::new(
            "node_modules",
            Some(std::path::PathBuf::from(&path)),
            1,
            "stale",
            5,
            Action::Trash,
        );
        let app = cleanup(vec![finding]);

        let rows = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT);

        assert!(rows.iter().any(|row| row.contains("🇧🇷")), "{rows:#?}");
        assert!(
            detail_pane_text(&rows).replace(' ', "").contains(&path),
            "{rows:#?}"
        );
    }

    /// A path is shown cell for cell: a space that falls just past the edge
    /// opens the next row, visibly, rather than hanging where the renderer
    /// clips it and joining two names into one.
    #[test]
    fn a_space_at_the_edge_of_a_path_stays_visible() {
        // 62 columns fill the first row at the minimum width; the space is
        // the 63rd.
        let path = format!("/{} {}", "a".repeat(61), "b".repeat(10));
        let finding = Finding::new(
            "node_modules",
            Some(std::path::PathBuf::from(&path)),
            1,
            "stale",
            5,
            Action::Trash,
        );
        let app = cleanup(vec![finding]);

        let rows = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT);

        assert!(detail_pane_cells(&rows).contains(&path), "{rows:#?}");
    }

    /// A space in a row's last cell reads as padding and joins the names on
    /// either side, so it opens the next row, where its indent shows.
    #[test]
    fn a_space_in_a_rows_last_column_moves_to_the_next_row() {
        // 61 columns, then a space that would take column 62 of 62.
        let path = format!("/{} {}", "a".repeat(60), "b".repeat(10));
        let finding = Finding::new(
            "node_modules",
            Some(std::path::PathBuf::from(&path)),
            1,
            "stale",
            5,
            Action::Trash,
        );
        let app = cleanup(vec![finding]);

        let rows = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT);

        // The list row keeps the path's tail too, so read the pane alone.
        let pane = rows
            .iter()
            .position(|row| row.contains(" Details "))
            .expect("the results screen has a detail pane");
        let continued = rows[pane + 1..]
            .iter()
            .map(|row| row.chars().skip(1).collect::<String>())
            .find(|row| row.contains("bbbbbbbbbb"))
            .expect("the path's second row is in the pane");
        assert!(continued.starts_with(" bbbbbbbbbb"), "{rows:#?}");
        // The first row now ends a cell early, so its padding is not part of
        // the path: trimming each row's right edge reads the path back exactly.
        assert!(detail_pane_text(&rows).contains(&path), "{rows:#?}");
    }

    #[test]
    fn wrapping_breaks_only_where_the_next_grapheme_would_pass_the_edge() {
        assert_eq!(wrap_columns("abcd efgh", 4), ["abcd", " efg", "h"]);
        assert_eq!(wrap_columns("abc defg", 4), ["abc", " def", "g"]);
        assert_eq!(wrap_columns("one two three", 8), ["one two", " three"]);
        // Nothing follows a trailing space, so it stays rather than opening a
        // blank row.
        assert_eq!(wrap_columns("abc ", 4), ["abc "]);
        assert_eq!(wrap_columns("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap_columns("日本語", 3), ["日", "本", "語"]);
        assert!(wrap_columns("no room", 0).is_empty());
    }

    /// Positive control for the growth: a finding that fits keeps the pane at
    /// its usual share, so the list keeps its rows.
    #[test]
    fn a_finding_that_fits_leaves_the_list_its_rows() {
        let app = cleanup(
            (0..30)
                .map(|index| trash(&format!("item-{index}"), 1, 2))
                .collect(),
        );

        let rows = screen_rows(&app, 100, 30);

        assert!(rows.iter().any(|row| row.contains("item-13")), "{rows:#?}");
        assert!(
            !detail_pane_text(&rows).contains("more line(s)"),
            "{rows:#?}"
        );
    }

    #[test]
    fn the_list_follows_the_cursor_past_the_first_page() {
        let mut app = cleanup(
            (0..60)
                .map(|index| trash(&format!("item-{index}"), 1, 2))
                .collect(),
        );
        app.handle_key(key(KeyCode::End));
        assert_eq!(app.cursor, 59);

        let output = rendered(&app, 100, 30);

        assert!(output.contains("60. danger-2"), "{output}");
        assert!(!output.contains("  1. danger-2"), "{output}");
    }

    /// Permanent mode raises every Trash finding to critical; a row that kept
    /// its Trash-mode danger beside SHRED understated what `a` would do.
    #[test]
    fn permanent_mode_rows_show_the_danger_the_plan_carries() {
        let mut app = cleanup(vec![trash("cache", 1, 3)]);
        app.handle_key(key(KeyCode::Char('s')));

        let output = rendered(&app, 100, 30);

        assert!(output.contains("1. danger-9"), "{output}");
        assert!(!output.contains("1. danger-3"), "{output}");
        assert!(output.contains("danger-9 · permanently delete"), "{output}");
    }

    /// The mode is a safety signal, so it leads the title: a title that grew
    /// with the plan's counts and size clipped TRASH-FIRST at 64 columns.
    #[test]
    fn the_mode_survives_the_minimum_width_on_a_large_plan() {
        let mut app = cleanup(
            (0..120)
                .map(|index| trash(&format!("cache-{index}"), 1 << 30, 5))
                .collect(),
        );

        let trash_first = rendered(&app, MIN_WIDTH, MIN_HEIGHT);
        app.handle_key(key(KeyCode::Char('s')));
        let permanent = rendered(&app, MIN_WIDTH, MIN_HEIGHT);

        assert!(trash_first.contains("TRASH-FIRST"), "{trash_first}");
        assert!(permanent.contains("PERMANENT"), "{permanent}");
    }

    #[test]
    fn the_footer_offers_selection_only_when_something_can_be_selected() {
        let mut disclosure = App::default();
        disclosure.finish_results(
            Operation::Clean(Target::Simulators),
            vec![Finding::new("disclosure", None, 1, "test", 0, Action::None)],
            Vec::new(),
            Vec::new(),
        );
        let report_only = rendered(&disclosure, 100, 30);
        assert!(!report_only.contains("Space select"), "{report_only}");
        assert!(report_only.contains("r rescan"), "{report_only}");

        let choosable = rendered(&cleanup(vec![trash("cache", 1, 2)]), 100, 30);
        assert!(choosable.contains("Space select"), "{choosable}");
    }

    /// The confirmation is where consent is given, so it says how much of the
    /// preview the approval covers, not only the results title behind it.
    #[test]
    fn the_confirmation_names_how_much_of_the_preview_it_covers() {
        let mut app = cleanup(vec![trash("first", 1, 2), trash("second", 1, 2)]);
        app.handle_key(key(KeyCode::Char(' ')));
        app.begin_confirmation();

        let output = rendered(&app, MIN_WIDTH, MIN_HEIGHT);

        assert!(output.contains("1 of 2 selected"), "{output}");
        assert!(output.contains("1 left out"), "{output}");
    }

    #[test]
    fn the_outcome_says_what_was_left_out() {
        let mut app = cleanup(vec![trash("first", 1, 2), trash("second", 1, 2)]);
        app.handle_key(key(KeyCode::Char(' ')));
        app.screen = Screen::Outcome;
        app.summary = Some(Summary {
            op: "caches".into(),
            items_touched: 1,
            bytes_freed_estimate: 1,
            bytes_trashed_estimate: 1,
            notes: vec!["trashed second".into()],
        });

        let output = rendered(&app, 100, 30);

        assert!(output.contains("1 left out of this plan"), "{output}");
    }

    #[test]
    fn small_terminal_fails_visibly_without_rendering_the_menu() {
        let app = App::default();
        let output = rendered(&app, 50, 12);
        assert!(output.contains("terminal too small"));
        assert!(output.contains("64×18"));
    }
}
