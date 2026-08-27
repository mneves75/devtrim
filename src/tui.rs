//! Interactive Ratatui adapter over the existing scan, preview, and apply core.

use std::io::{self, IsTerminal};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::cli::Target;
use crate::ops::{self, Action, ApplyOutcome, Finding};
use crate::report::{self, Summary};
use crate::safety::{self, ConfirmationRequirement, Ctx};

const MIN_WIDTH: u16 = 64;
const MIN_HEIGHT: u16 = 18;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operation {
    ScanAll,
    Clean(Target),
    Icloud,
    TrashEmpty,
}

impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::ScanAll => "scan",
            Self::Clean(target) => target.as_str(),
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
    errors: Vec<String>,
    warnings: Vec<String>,
    summary: Option<Summary>,
    shred: bool,
    scroll: u16,
    confirmation: Option<ConfirmationKind>,
    input: String,
    status: String,
    failed: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            screen: Screen::Menu,
            selected: 0,
            operation: None,
            findings: Vec::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            summary: None,
            shred: false,
            scroll: 0,
            confirmation: None,
            input: String::new(),
            status: "Preview first. Nothing changes until you explicitly approve.".into(),
            failed: false,
        }
    }
}

impl App {
    fn effective_findings(&self) -> Vec<Finding> {
        let mut findings = self.findings.clone();
        report::effective_actions(&mut findings, self.shred);
        findings
    }

    fn has_actionable_findings(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.action.is_actionable())
    }

    fn can_toggle_shred(&self) -> bool {
        matches!(self.operation, Some(Operation::Clean(_)))
            && self
                .findings
                .iter()
                .any(|finding| finding.action == Action::Trash)
    }

    fn begin_load(&mut self, operation: Operation) {
        self.screen = Screen::Loading;
        self.operation = Some(operation);
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
            "Review every finding. Press a only when the exact plan is acceptable.".into()
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
        if operation.read_only() || !self.has_actionable_findings() {
            self.status = "This result has no actionable findings.".into();
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
                self.scroll = self.scroll.saturating_sub(1);
                Intent::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
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
            }
            Intent::Apply(plan) => {
                app.screen = Screen::Loading;
                app.status = "Applying only the exact previewed findings…".into();
                terminal.draw(|frame| render(frame, &app))?;
                apply_operation(&mut app, ctx, plan);
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
            let Some(cleanup) = ops::by_name(target.as_str()) else {
                app.fail(anyhow::anyhow!(
                    "unknown cleanup target {}",
                    target.as_str()
                ));
                return;
            };
            match cleanup.scan(ctx) {
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
            Ok(findings) => {
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
        Operation::Clean(target) => ops::by_name(target.as_str())
            .ok_or_else(|| anyhow::anyhow!("unknown cleanup target {}", target.as_str()))
            .and_then(|cleanup| cleanup.apply(&findings, ctx)),
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
    safety::trash_gate(&ctx.home, Some(confirm_gb))?;
    ops::purge_trash(findings, ctx)
}

fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if terminal_too_small(area) {
        let message = Paragraph::new(Text::from(vec![
            Line::styled(
                "devtrim — terminal too small",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
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
}

fn render_header(frame: &mut Frame, area: Rect, app: &App) {
    let operation = app.operation.map_or("choose an operation", Operation::name);
    let line = Line::from(vec![
        Span::styled(
            " devtrim ",
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("v{}  ", env!("CARGO_PKG_VERSION")),
            Style::default().fg(Color::DarkGray),
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
            Span::styled(
                format!(" {} ", item.key),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(item.label),
            Span::styled(
                format!("  {marker}"),
                Style::default().fg(if marker == "PERMANENT" {
                    Color::Red
                } else if marker == "READ-ONLY" {
                    Color::Blue
                } else {
                    Color::Green
                }),
            ),
        ]))
    });
    let list = List::new(items)
        .block(Block::bordered().title(" Operations "))
        .highlight_symbol("▶ ")
        .highlight_style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default();
    state.select(Some(app.selected));
    frame.render_stateful_widget(list, menu_area, &mut state);

    let selected = &MENU[app.selected];
    let detail = Text::from(vec![
        Line::styled(
            selected.label,
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw(selected.description),
        Line::raw(""),
        Line::styled(
            if selected.operation.read_only() {
                "No mutation is available from this screen."
            } else {
                "Selecting this operation scans first. Apply is a separate, explicit step."
            },
            Style::default().fg(Color::Yellow),
        ),
        Line::raw(""),
        Line::raw("↑/↓ or j/k navigate · Enter opens · menu key opens directly"),
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
    let findings = app.effective_findings();
    let total = report::actionable_bytes(&findings);
    let danger = safety::plan_danger(&findings);
    let mode = if app.shred {
        "PERMANENT"
    } else {
        "TRASH-FIRST"
    };
    let title = format!(
        " Preview · {} finding(s) · {} actionable · danger-{danger} · {mode} ",
        findings.len(),
        report::gb(total),
    );
    let mut lines = Vec::new();
    if findings.is_empty() {
        lines.push(Line::styled(
            "No findings.",
            Style::default().fg(Color::Green),
        ));
    }
    for (index, finding) in findings.iter().enumerate() {
        let action = action_label(&finding.action);
        let color = danger_color(finding.danger);
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:>2}. danger-{}  ", index + 1, finding.danger),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                report::terminal_safe(&finding.label),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {}  {action}", report::gb(finding.size_bytes)),
                Style::default().fg(Color::Cyan),
            ),
        ]));
        lines.push(Line::raw(report::terminal_safe(
            finding.path.as_deref().unwrap_or("command action"),
        )));
        lines.push(Line::styled(
            report::terminal_safe(&finding.note),
            Style::default().fg(Color::DarkGray),
        ));
        lines.push(Line::raw(""));
    }
    if !app.errors.is_empty() {
        lines.push(Line::styled(
            "Scan errors",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        for error in &app.errors {
            lines.push(Line::raw(format!("• {}", report::terminal_safe(error))));
        }
    }
    if !app.warnings.is_empty() {
        lines.push(Line::styled(
            "Warnings",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        for warning in &app.warnings {
            lines.push(Line::raw(format!("• {}", report::terminal_safe(warning))));
        }
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .scroll((app.scroll, 0))
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(title)),
        area,
    );
}

fn render_outcome(frame: &mut Frame, area: Rect, app: &App) {
    let mut lines = Vec::new();
    if let Some(summary) = &app.summary {
        lines.push(Line::styled(
            format!(
                "{} · {} item(s) · ~{} reclaimed estimate",
                summary.op,
                summary.items_touched,
                report::gb(summary.bytes_freed_estimate)
            ),
            Style::default()
                .fg(match (app.errors.is_empty(), summary.items_touched) {
                    (true, _) => Color::Green,
                    (false, 0) => Color::Red,
                    (false, _) => Color::Yellow,
                })
                .add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::raw(""));
        for note in &summary.notes {
            lines.push(Line::raw(format!("• {}", report::terminal_safe(note))));
        }
    }
    if !app.errors.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Errors",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
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
        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
    )];
    for error in &app.errors {
        lines.push(Line::raw(report::terminal_safe(error)));
    }
    if !app.warnings.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            "Warnings captured before the failure",
            Style::default().fg(Color::Yellow),
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
    let keys = match app.screen {
        Screen::Menu => "↑/↓ navigate · Enter select · q quit",
        Screen::Results => {
            if app.can_toggle_shred() {
                "a apply · s Trash/permanent · r rescan · b back · q quit"
            } else {
                "a apply when available · r rescan · b back · q quit"
            }
        }
        Screen::Confirm => "Esc cancel · type the exact requested acknowledgment",
        Screen::Outcome | Screen::Error => "↑/↓ or j/k scroll · b back to menu · q quit",
        Screen::Loading => "Scanning and apply are synchronous; please wait",
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(keys, Style::default().fg(Color::Cyan)),
            Line::styled(app.status.as_str(), Style::default().fg(Color::Yellow)),
        ])
        .block(Block::bordered()),
        area,
    );
}

fn render_confirmation(frame: &mut Frame, area: Rect, app: &App) {
    let Some(confirmation) = app.confirmation else {
        return;
    };
    let popup = confirmation_rect(area);
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
    let text = Text::from(vec![
        Line::styled(
            "DATA-LOSS WARNING",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::raw(safety::DATA_LOSS_NOTICE),
        Line::raw(""),
        Line::styled(prompt, Style::default().fg(Color::Yellow)),
        Line::raw(""),
        Line::from(vec![
            Span::raw("> "),
            Span::styled(
                app.input.as_str(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
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

fn danger_color(danger: u8) -> Color {
    match danger {
        0..=2 => Color::Green,
        3..=5 => Color::Yellow,
        6..=8 => Color::LightRed,
        _ => Color::Red,
    }
}

fn confirmation_rect(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).min(100);
    let height = area.height.saturating_sub(2).min(16);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
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

    #[test]
    fn small_terminal_fails_visibly_without_rendering_the_menu() {
        let app = App::default();
        let output = rendered(&app, 50, 12);
        assert!(output.contains("terminal too small"));
        assert!(output.contains("64×18"));
    }
}
