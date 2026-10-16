//! Interactive Ratatui adapter over the existing scan, preview, and apply core.

use std::io::{self, IsTerminal};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::cli::Target;
use crate::ops::{self, Action, ApplyOutcome, Finding, Op};
use crate::process::{self, Interrupts};
use crate::report::{self, Summary};
use crate::safety::{self, ConfirmationRequirement, Ctx};
use crate::theme::{Theme, Token, danger_token};

const MIN_WIDTH: u16 = 64;
/// The Error screen's status: nothing was authorized, and `r` scans again.
const FAILED_CLOSED: &str = "Failed closed; no new action was authorized. r rescans.";
const MIN_HEIGHT: u16 = 18;
/// Rows PgUp and PgDn move, in every list and scrolling view.
const PAGE_ROWS: u16 = 8;

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
        description: "Agent caches, plus session history past the retention window.",
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
    /// The highlighted results row whole, for when the pane cannot hold it.
    Detail,
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

/// What runs on the worker thread while the working screen shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Work {
    Scan,
    Apply,
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
    /// Each category's slice of a Scan everything result, so a key that cannot
    /// act there names the view that can; empty for every other operation.
    sections: Vec<report::ScanSection>,
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
    /// The furthest the detail, outcome or error view can scroll, measured
    /// when it was last drawn, so a key never scrolls past its content.
    scroll_limit: std::cell::Cell<u16>,
    /// The scan or apply running on the worker thread, and when it started,
    /// for the working screen's clock.
    working: Option<(Work, Instant)>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            screen: Screen::Menu,
            selected: 0,
            operation: None,
            findings: Vec::new(),
            sections: Vec::new(),
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
            scroll_limit: std::cell::Cell::new(0),
            working: None,
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

    /// Whether `s` acts. Turning permanent mode on needs a selected item that
    /// moves to Trash; turning it off always works, so leaving every such item
    /// out never strands the operator in permanent mode.
    fn can_toggle_shred(&self) -> bool {
        self.shred
            || matches!(self.operation, Some(Operation::Clean(_) | Operation::Purge))
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
        // Replaces whatever an earlier `A` said, which this press made false.
        self.status = format!(
            "{} of {} items selected.",
            self.choice_count() - self.excluded.len(),
            self.choice_count()
        );
    }

    /// Everything back in when anything is out; otherwise every choice out.
    fn toggle_all(&mut self) {
        if self.excluded.is_empty() {
            self.excluded = (0..self.findings.len())
                .filter(|index| self.is_selectable(*index))
                .collect();
            self.status = "Every item is left out; A selects them all again.".into();
        } else {
            self.excluded.clear();
            self.status = "Every item is selected; A leaves them all out.".into();
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
        self.working = Some((Work::Scan, Instant::now()));
    }

    fn begin_apply(&mut self) {
        self.screen = Screen::Loading;
        self.status = "Applying only the exact previewed findings…".into();
        self.working = Some((Work::Apply, Instant::now()));
    }

    /// Whole seconds the scan or apply on the worker thread has run.
    fn elapsed_seconds(&self) -> Option<u64> {
        self.working.map(|(_, started)| started.elapsed().as_secs())
    }

    fn finish_load(&mut self, operation: Operation, loaded: Loaded) {
        match loaded {
            Loaded::Scan(result, warnings) => self.finish_scan(result, warnings),
            Loaded::Findings(findings, warnings) => {
                self.finish_results(operation, findings, Vec::new(), warnings);
            }
            Loaded::Failed(error, warnings) => {
                self.fail(error);
                self.warnings = warnings;
            }
        }
    }

    fn finish_apply(&mut self, result: Result<ApplyOutcome>) {
        match result {
            Ok(outcome) => {
                if !outcome.errors.is_empty() {
                    self.failed = true;
                }
                self.status =
                    outcome_status(!outcome.errors.is_empty(), outcome.summary.items_touched)
                        .into();
                self.summary = Some(outcome.summary);
                self.errors = outcome.errors;
                self.scroll = 0;
                self.screen = Screen::Outcome;
            }
            Err(error) => self.fail(error),
        }
    }

    fn finish_results(
        &mut self,
        operation: Operation,
        findings: Vec<Finding>,
        errors: Vec<String>,
        warnings: Vec<String>,
    ) {
        self.show_results(operation, findings, Vec::new(), errors, warnings);
    }

    /// A Scan everything result, keeping each category's slice for routing.
    fn finish_scan(&mut self, result: ops::ScanResult, warnings: Vec<String>) {
        self.show_results(
            Operation::ScanAll,
            result.findings,
            result.sections,
            result.errors,
            warnings,
        );
    }

    fn show_results(
        &mut self,
        operation: Operation,
        findings: Vec<Finding>,
        sections: Vec<report::ScanSection>,
        errors: Vec<String>,
        warnings: Vec<String>,
    ) {
        self.operation = Some(operation);
        self.reset_selection();
        self.findings = findings;
        self.sections = sections;
        self.errors = errors;
        self.warnings = warnings;
        if !self.errors.is_empty()
            || self
                .findings
                .iter()
                .any(|finding| finding.scan_error().is_some())
        {
            self.failed = true;
        }
        self.summary = None;
        self.screen = Screen::Results;
        self.status = if let Some(failure) = self.failure_status() {
            failure
        } else if self.findings.is_empty() {
            "No findings. Nothing can be applied.".into()
        } else if operation.read_only() {
            "Read-only result. No apply action is available.".into()
        } else {
            "Review every finding. Space leaves one out; a applies the rest".into()
        };
    }

    /// How many findings stand for something the scan could not judge.
    fn unjudged_count(&self) -> usize {
        report::finding_and_error_counts(&self.findings).1
    }

    /// How many things this result failed at. A Scan everything result lists
    /// each category that failed outright among its errors, beside a copy of
    /// every finding that could not be judged; other results carry only those
    /// findings.
    fn failure_count(&self) -> usize {
        if self.operation == Some(Operation::ScanAll) {
            self.errors.len().max(self.unjudged_count())
        } else {
            self.unjudged_count()
        }
    }

    /// What this result failed at and what the operator can do, short enough
    /// for the footer at the minimum width, or `None` when nothing failed. A
    /// category whose probe failed offers nothing at all, which must never
    /// read as a category with nothing to offer.
    fn failure_status(&self) -> Option<String> {
        let unjudged = self.unjudged_count();
        let outright = if self.operation == Some(Operation::ScanAll) {
            self.errors.len().saturating_sub(unjudged)
        } else {
            0
        };
        if outright > 0 {
            // A category that scanned has a section, even an empty one, so the
            // one without is the one that failed.
            let missing: Vec<&str> = ops::all()
                .iter()
                .map(|op| op.name())
                .filter(|name| {
                    !self
                        .sections
                        .iter()
                        .any(|section| section.category == *name)
                })
                .collect();
            let what = match (outright, missing.as_slice()) {
                (1, [name]) => (*name).to_string(),
                (1, _) => "1 category".into(),
                _ => format!("{outright} categories"),
            };
            return Some(format!(
                "{what} failed to scan: error rows say why; r retries."
            ));
        }
        (unjudged > 0)
            .then(|| format!("{unjudged} item(s) not judged: their rows say why; r retries."))
    }

    /// Why an action key would change nothing on the results screen, so no
    /// key press passes in silence; `None` when the key acts.
    fn inert_reason(&self, key: KeyCode) -> Option<String> {
        let operation = self.operation?;
        if operation == Operation::ScanAll {
            return Some(self.scan_route());
        }
        if operation.read_only() {
            return Some("Read-only report: nothing here can be applied.".into());
        }
        let reason = match key {
            KeyCode::Char(' ' | 'A') if self.choice_count() == 0 => {
                "Nothing here can be applied, so nothing can be selected."
            }
            KeyCode::Char(' ') if self.cursor >= self.findings.len() => {
                "Space leaves out findings only, not errors or notes."
            }
            KeyCode::Char(' ') if !self.is_selectable(self.cursor) => {
                "This row is a report; Space leaves out only items that apply."
            }
            KeyCode::Char('s') if self.can_toggle_shred() => return None,
            KeyCode::Char('s') if operation == Operation::TrashEmpty => {
                "Emptying the Trash is already permanent."
            }
            KeyCode::Char('s')
                if !self
                    .findings
                    .iter()
                    .any(|finding| finding.action == Action::Trash) =>
            {
                "Permanent mode does not apply: nothing here moves to Trash."
            }
            KeyCode::Char('s') => "Permanent mode needs a selected item that moves to Trash.",
            _ => return None,
        };
        Some(reason.into())
    }

    /// Where the highlighted Scan everything row can be acted on: the menu
    /// entry of its own category, named only when that view can apply it.
    fn scan_route(&self) -> String {
        let Some(finding) = self.findings.get(self.cursor) else {
            return "Read-only scan. Open a category from the menu (b) to act.".into();
        };
        let entry = self
            .sections
            .iter()
            .find(|section| section.range.contains(&self.cursor))
            .and_then(|section| {
                MENU.iter()
                    .find(|item| item.operation.name() == section.category)
            });
        match entry {
            Some(entry) if finding.action.is_actionable() && !entry.operation.read_only() => {
                format!(
                    "Read-only scan. To act on it: b, then {} ({}).",
                    entry.key,
                    entry.operation.name()
                )
            }
            Some(_) => "Read-only scan: this item is a report, with nothing to apply.".into(),
            None => "Read-only scan. Open a category from the menu (b) to act.".into(),
        }
    }

    fn fail(&mut self, error: anyhow::Error) {
        self.failed = true;
        self.errors = vec![format!("{error:#}")];
        self.summary = None;
        self.scroll = 0;
        self.screen = Screen::Error;
        self.status = FAILED_CLOSED.into();
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
                "Nothing is selected: Space adds an item, A selects all."
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
            Screen::Detail => self.handle_detail_key(key.code),
            Screen::Confirm => self.handle_confirm_key(key.code),
            Screen::Outcome | Screen::Error => match key.code {
                KeyCode::Char('q') => Intent::Quit,
                KeyCode::Esc | KeyCode::Char('b') => {
                    self.back_to_menu();
                    Intent::None
                }
                // A failure may be transient, a probe that did not answer under
                // load; scanning again is a fresh preview, never a retried apply.
                KeyCode::Char('r') if self.screen == Screen::Error => {
                    self.operation.map_or(Intent::None, Intent::Load)
                }
                code => {
                    self.scroll_by(code);
                    Intent::None
                }
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
        if matches!(key, KeyCode::Char(' ' | 'A' | 'a' | 's'))
            && let Some(reason) = self.inert_reason(key)
        {
            self.status = reason;
            return Intent::None;
        }
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
                self.move_cursor(self.cursor.saturating_sub(usize::from(PAGE_ROWS)));
                Intent::None
            }
            KeyCode::PageDown => {
                self.move_cursor(self.cursor.saturating_add(usize::from(PAGE_ROWS)));
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
                    "Permanent mode: every action is SHRED; danger is critical.".into()
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
            KeyCode::Enter if self.row_count() > 0 => {
                self.screen = Screen::Detail;
                self.scroll = 0;
                self.scroll_limit.set(0);
                Intent::None
            }
            _ => Intent::None,
        }
    }

    /// The detail view only reads: it scrolls within its content and closes,
    /// and no key here reaches the plan behind it.
    fn handle_detail_key(&mut self, key: KeyCode) -> Intent {
        match key {
            KeyCode::Char('q') => return Intent::Quit,
            KeyCode::Esc | KeyCode::Enter | KeyCode::Char('b') => {
                self.screen = Screen::Results;
                self.scroll = 0;
            }
            code => self.scroll_by(code),
        }
        Intent::None
    }

    /// Scrolls a detail, outcome or error view within its content, never past
    /// the last line it drew.
    fn scroll_by(&mut self, key: KeyCode) {
        let limit = self.scroll_limit.get();
        let scroll = self.scroll.min(limit);
        self.scroll = match key {
            KeyCode::Up | KeyCode::Char('k') => scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => scroll.saturating_add(1).min(limit),
            KeyCode::PageUp => scroll.saturating_sub(PAGE_ROWS),
            KeyCode::PageDown => scroll.saturating_add(PAGE_ROWS).min(limit),
            KeyCode::Home | KeyCode::Char('g') => 0,
            KeyCode::End | KeyCode::Char('G') => limit,
            _ => scroll,
        };
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

/// How long the loop waits for input before it looks again: often enough that
/// the working screen's clock moves and a signal is acted on at once, rarely
/// enough to cost nothing while idle.
const TICK: Duration = Duration::from_millis(100);

pub fn run(ctx: &Ctx) -> Result<ExitCode> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("the TUI requires an interactive stdin and stdout terminal");
    }
    // Before the terminal changes, so no signal can leave it in raw mode with
    // nothing to restore it.
    let interrupts =
        process::forward_interrupts().context("cannot install the interrupt handler")?;
    let mut terminal = ratatui::try_init().context("cannot initialize TUI terminal")?;
    let result = run_loop(&mut terminal, ctx, &interrupts);
    let restore = ratatui::try_restore().context("cannot restore terminal after TUI exit");
    match (result, restore) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(code), Ok(())) => Ok(code),
    }
}

/// Restores the terminal, then ends the run as an interrupt does: every
/// command devtrim started is killed, and it exits 128 plus `signal`. Only the
/// thread that owns the terminal calls it.
fn stop(terminal: &mut DefaultTerminal, signal: i32) -> ! {
    ratatui::try_restore().ok();
    terminal.show_cursor().ok();
    process::kill_and_exit(signal)
}

fn run_loop(
    terminal: &mut DefaultTerminal,
    ctx: &Ctx,
    interrupts: &Interrupts,
) -> Result<ExitCode> {
    let mut app = App::default();
    let mut redraw = true;
    loop {
        if let Some(signal) = interrupts.received() {
            stop(terminal, signal);
        }
        if redraw {
            terminal.draw(|frame| render(frame, &app))?;
        }
        // Waking only to look for a signal changes nothing on screen.
        redraw = event::poll(TICK).context("cannot poll terminal input")?;
        if !redraw {
            continue;
        }
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
                let loaded =
                    in_background(terminal, &mut app, interrupts, || run_scan(ctx, operation))?;
                app.finish_load(operation, loaded);
                settle(terminal, &app)?;
            }
            Intent::Apply(plan) => {
                if approved_plan_matches(&app, &plan) {
                    app.begin_apply();
                    let result =
                        in_background(terminal, &mut app, interrupts, || run_apply(ctx, plan))?;
                    app.finish_apply(result);
                } else {
                    app.fail(anyhow::anyhow!(
                        "confirmation does not authorize the current preview"
                    ));
                }
                settle(terminal, &app)?;
            }
        }
    }
}

/// Runs `job`, a scan or an apply, on a worker thread while this thread keeps
/// the terminal: the working screen redraws as its clock moves, and a signal
/// is acted on at once.
///
/// Every key typed meanwhile is read and dropped, since it was aimed at a
/// screen that does not exist yet, except a quit. During a scan, `q` or
/// Ctrl+C ends the run the way SIGINT does: the terminal is restored, every
/// command the scan started is killed and devtrim exits 130. A scan changes
/// nothing, so nothing is left half done.
///
/// During an apply, `q` and Ctrl+C are refused until it returns, and the
/// working screen says so: quitting could abandon a target half moved or half
/// deleted, and the operator can wait to see what changed. A SIGINT or SIGTERM
/// still stops it at once, as it stops the CLI, because whoever sent it wants
/// devtrim gone and refusing would only invite SIGKILL, which restores and
/// kills nothing. The target in progress then keeps a journal attempt without
/// a result, which `history` reports as interrupted.
///
/// A terminal that fails while the worker runs ends the run once the worker
/// returns.
fn in_background<T: Send>(
    terminal: &mut DefaultTerminal,
    app: &mut App,
    interrupts: &Interrupts,
    job: impl FnOnce() -> T + Send,
) -> Result<T> {
    let scanning = matches!(app.working, Some((Work::Scan, _)));
    let finished = thread::scope(|scope| -> Result<thread::Result<T>> {
        let worker = scope.spawn(job);
        let mut shown = None;
        while !worker.is_finished() {
            if let Some(signal) = interrupts.received() {
                stop(terminal, signal);
            }
            let elapsed = app.elapsed_seconds();
            if shown != Some(elapsed) {
                terminal.draw(|frame| render(frame, app))?;
                shown = Some(elapsed);
            }
            if !event::poll(TICK).context("cannot poll terminal input")? {
                continue;
            }
            match event::read().context("cannot read terminal input")? {
                Event::Key(key) if scanning && is_quit_key(key) => {
                    stop(terminal, signal_hook::consts::SIGINT)
                }
                Event::Resize(..) => shown = None,
                _ => {}
            }
        }
        Ok(worker.join())
    })?;
    app.working = None;
    match finished {
        Ok(value) => Ok(value),
        // The panic already restored the terminal through Ratatui's hook; end
        // the run as a panic on this thread always did.
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

fn is_quit_key(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && (key.code == KeyCode::Char('q')
            || key.modifiers.contains(event::KeyModifiers::CONTROL)
                && key.code == KeyCode::Char('c'))
}

/// Shows a finished scan or apply, then drops every key still queued. Those
/// keys were typed before the result was on screen, at a screen that no
/// longer exists; delivering them would let type-ahead toggle permanent mode
/// and approve a plan before it was ever displayed.
fn settle(terminal: &mut DefaultTerminal, app: &App) -> Result<()> {
    terminal.draw(|frame| render(frame, app))?;
    while event::poll(Duration::ZERO).context("cannot poll terminal input")? {
        event::read().context("cannot read terminal input")?;
    }
    Ok(())
}

/// What a scan produced, carried from the worker thread to the screen.
enum Loaded {
    Scan(ops::ScanResult, Vec<String>),
    Findings(Vec<Finding>, Vec<String>),
    Failed(anyhow::Error, Vec<String>),
}

/// Scans for `operation` on the worker thread, touching no screen state; the
/// diagnostics it captured travel with the result.
fn run_scan(ctx: &Ctx, operation: Operation) -> Loaded {
    ctx.take_diagnostics();
    let walks_roots = match operation {
        Operation::ScanAll | Operation::Purge => true,
        Operation::Clean(target) => ops::for_target(target).scans_roots(),
        Operation::Icloud | Operation::TrashEmpty => false,
    };
    if walks_roots {
        ctx.diagnostic("info", ctx.roots_note());
    }
    let scanned = match operation {
        Operation::ScanAll => {
            let result = ops::scan_all(ctx);
            return Loaded::Scan(result, ctx.take_diagnostics());
        }
        Operation::Clean(target) => {
            ops::for_target(target).scan(ctx, &ops::project::ScanObservations::default())
        }
        Operation::Purge => ops::purge::Purge.scan(ctx, &ops::project::ScanObservations::default()),
        Operation::Icloud => ops::icloud_status(ctx),
        Operation::TrashEmpty => ops::trash_findings(ctx),
    };
    match scanned {
        Ok(mut findings) => {
            // iCloud status reports only; it offers nothing to protect.
            if operation != Operation::Icloud {
                ops::filter_protected_findings(&mut findings, ctx);
            }
            Loaded::Findings(findings, ctx.take_diagnostics())
        }
        Err(error) => Loaded::Failed(error, ctx.take_diagnostics()),
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

/// Applies a plan on the worker thread. The loop matched its approval against
/// the displayed preview before it started.
fn run_apply(ctx: &Ctx, plan: ApprovedPlan) -> Result<ApplyOutcome> {
    let ApprovedPlan {
        operation,
        findings,
        approval,
    } = plan;
    let mut outcome = match operation {
        Operation::Clean(target) => ops::for_target(target).apply(&findings, ctx),
        Operation::Purge => ops::purge::Purge.apply(&findings, ctx),
        Operation::TrashEmpty => apply_trash(ctx, &findings, approval),
        Operation::ScanAll | Operation::Icloud => {
            Err(anyhow::anyhow!("refusing to apply a read-only operation"))
        }
    }?;
    outcome.errors.extend(ctx.take_journal_errors());
    Ok(outcome)
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
        Screen::Detail => render_scrolled(frame, body, app, "Details", detail_entries(app)),
        Screen::Outcome => render_scrolled(frame, body, app, "Apply outcome", outcome_entries(app)),
        Screen::Error => render_scrolled(frame, body, app, "Failed closed", error_entries(app)),
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
    let mut lines = Vec::new();
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
    }
    lines.push(Line::styled(
        "Nothing is applied without an explicit, separate approval.",
        app.theme.style(Token::Warning),
    ));
    // Sized to its own content, and compact enough to fit whole at the
    // minimum 64×18, where it has 14 rows of 44-column descriptions. A
    // reference that hides the keys it advertises is worse than none, because
    // the reader has no way to know it was truncated.
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

/// Grouped by the layer an operator reaches for: getting around first, then
/// the actions that change what happens.
type HelpGroup = (&'static str, &'static [(&'static str, &'static str)]);
const HELP_KEYS: &[HelpGroup] = &[
    (
        "Navigate",
        &[
            ("↑/↓, k/j", "navigate lists and scroll output"),
            ("PgUp/PgDn", "move a page; g/G the first or last row"),
            ("Enter", "open the operation, or the item in full"),
            ("b, Esc", "back a screen; on the menu, Esc quits"),
            ("?", "open or close this reference"),
            ("q, Ctrl+C", "quit, stopping a scan; not during an apply"),
        ],
    ),
    (
        "Act",
        &[
            ("Space", "leave the highlighted item out, or back in"),
            ("A", "select every item, or none"),
            ("a", "apply the selected items of the plan"),
            ("s", "toggle Trash-first and permanent deletion"),
            ("r", "rescan, or retry after a failure"),
        ],
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

/// The status an apply leaves: whether it failed, and how many items it
/// changed before it did.
fn outcome_status(failed: bool, touched: usize) -> &'static str {
    if !failed {
        "Apply completed. Review the truthful summary below."
    } else if touched == 0 {
        "Apply failed before any item was changed."
    } else {
        // Some categories continue past a refused item, so "stopped" would be
        // false; the summary is the record either way.
        "Apply finished with errors: see what changed and what failed."
    }
}

/// The badge beside a menu operation, its token, and what it means, which the
/// Selected pane spells out so the badge is never left to guess.
fn badge(operation: Operation) -> (&'static str, Token, &'static str) {
    if operation.read_only() {
        (
            "READ-ONLY",
            Token::Info,
            "READ-ONLY: it reports and never changes anything.",
        )
    } else if operation == Operation::TrashEmpty {
        (
            "PERMANENT",
            Token::Critical,
            "PERMANENT: it deletes for good, after a typed size acknowledgment.",
        )
    } else {
        (
            "PREVIEW",
            Token::Success,
            "PREVIEW: it scans first; nothing changes until you select items, press a and approve.",
        )
    }
}

fn render_menu(frame: &mut Frame, area: Rect, app: &App) {
    let [menu_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(46), Constraint::Percentage(54)]).areas(area);
    let items = MENU.iter().map(|item| {
        let (marker, token, _) = badge(item.operation);
        ListItem::new(Line::from(vec![
            Span::styled(format!(" {} ", item.key), app.theme.style(Token::Muted)),
            Span::raw(item.label),
            Span::styled(format!("  {marker}"), app.theme.style(token)),
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
    let (_, _, meaning) = badge(selected.operation);
    let detail = Text::from(vec![
        Line::styled(selected.label, app.theme.bold(Token::Accent)),
        Line::raw(""),
        Line::raw(selected.description),
        Line::raw(""),
        Line::styled(meaning, app.theme.style(Token::Warning)),
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
    let mut lines = vec![Line::raw(app.status.as_str())];
    // The clock moving is what says the scan is still alive; a probe can take
    // minutes on a loaded machine.
    if let Some(elapsed) = app.elapsed_seconds() {
        lines.push(Line::styled(
            format!("{elapsed} s elapsed"),
            app.theme.style(Token::Muted),
        ));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .alignment(Alignment::Center)
            .block(Block::bordered().title(" Working ")),
        area,
    );
}

/// The results list's title, within `width` columns. The mode is a safety
/// signal, so it leads: counts and sizes grow with the plan and would
/// otherwise push it past the minimum width. A title too wide for the
/// terminal drops its words before any number, so what failed and the danger
/// stay whole.
fn results_title(app: &App, width: usize) -> String {
    let plan = app.effective_findings();
    let size = report::gb(report::actionable_bytes(&plan));
    let danger = safety::plan_danger(&plan);
    let mode = if app.shred {
        "PERMANENT"
    } else {
        "TRASH-FIRST"
    };
    let failed = match app.failure_count() {
        0 => String::new(),
        failures => format!(" · {failures} failed"),
    };
    let choices = app.choice_count();
    let (full, compact) = if choices > 0 {
        let selected = choices.saturating_sub(app.excluded.len());
        (
            format!(" {mode} · {selected}/{choices} selected · {size} · danger-{danger}{failed} "),
            format!(" {mode} · {selected}/{choices} · {size} · danger-{danger}{failed} "),
        )
    } else {
        let (counted, _) = report::finding_and_error_counts(&app.findings);
        (
            format!(
                " {mode} · {counted} finding(s) · {size} actionable · danger-{danger}{failed} "
            ),
            format!(" {mode} · {counted} found · {size} · danger-{danger}{failed} "),
        )
    };
    if Span::raw(full.as_str()).width() <= width {
        full
    } else {
        compact
    }
}

fn render_results(frame: &mut Frame, area: Rect, app: &App) {
    // The detail pane is the only place a long path appears whole, so it grows
    // past its usual share to hold the highlighted row, leaving the list one
    // row, and says how many lines a terminal too short for it still hides and
    // that Enter shows them.
    let mut details = detail_lines(app, usize::from(area.width.saturating_sub(2)));
    let pane_floor = (area.height / 3).clamp(5, 10);
    let pane_ceiling = area.height.saturating_sub(3).max(pane_floor);
    let pane_needed = u16::try_from(details.len()).map_or(u16::MAX, |rows| rows.saturating_add(2));
    let detail_height = pane_needed.clamp(pane_floor, pane_ceiling);
    let detail_rows = usize::from(detail_height.saturating_sub(2));
    if details.len() > detail_rows {
        let shown = detail_rows.saturating_sub(1);
        let hidden = details.len() - shown;
        details.truncate(shown);
        details.push(Line::styled(
            format!("… {hidden} more line(s); Enter shows all"),
            app.theme.style(Token::Warning),
        ));
    }
    let [list_area, detail_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(detail_height)]).areas(area);
    let title = results_title(app, usize::from(list_area.width.saturating_sub(2)));
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

/// A view that scrolls within its content: the highlighted results row whole,
/// an apply outcome, or a failure. Lines are wrapped here rather than by the
/// renderer, so their count is known and no key scrolls past the last one.
fn render_scrolled(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    name: &str,
    entries: Vec<(String, Style)>,
) {
    let lines = wrap_lines(entries, usize::from(area.width.saturating_sub(2)));
    let visible = area.height.saturating_sub(2);
    let total = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    let limit = total.saturating_sub(visible);
    app.scroll_limit.set(limit);
    let scroll = app.scroll.min(limit);
    let title = if limit == 0 {
        format!(" {name} ")
    } else {
        format!(
            " {name} · lines {}-{} of {total} ",
            scroll + 1,
            scroll.saturating_add(visible).min(total)
        )
    };
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .scroll((scroll, 0))
            .block(Block::bordered().title(title)),
        area,
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
    let path = report::terminal_safe(&finding_place(&finding));
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

/// Where a finding acts: its path, or for a typed command that has none, that
/// it is a command. Anything else without a path — a report row, or an error
/// entry for something that could not be judged — names no place at all.
fn finding_place(finding: &Finding) -> String {
    match (&finding.path, &finding.action) {
        (Some(path), _) => path.clone(),
        (None, Action::Command { .. }) => "command action".into(),
        (None, _) => String::new(),
    }
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

/// Everything about the highlighted row, wrapped to `width` columns.
fn detail_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    wrap_lines(detail_entries(app), width)
}

/// Everything about the highlighted row, one entry per line before wrapping.
/// The selection state precedes the path, the part most likely to run long.
fn detail_entries(app: &App) -> Vec<(String, Style)> {
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
        let place = finding_place(&finding);
        if !place.is_empty() {
            lines.push((report::terminal_safe(&place), Style::default()));
        }
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
}

/// Styled entries wrapped to `width` columns, one terminal row per line.
fn wrap_lines(entries: Vec<(String, Style)>, width: usize) -> Vec<Line<'static>> {
    entries
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
/// `max` terminal columns. It walks graphemes, as `wrap_columns` does, so a
/// flag or a decomposed accent is kept whole or not at all.
fn truncate_left(text: &str, max: usize) -> String {
    let span = Span::raw(text);
    if span.width() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let graphemes: Vec<_> = span.styled_graphemes(Style::default()).collect();
    let mut kept = 0;
    let mut start = text.len();
    for grapheme in graphemes.iter().rev() {
        let columns = Span::raw(grapheme.symbol).width();
        if kept + columns + 1 > max {
            break;
        }
        kept += columns;
        // Each grapheme borrows `text`, so its address is its byte offset.
        start = grapheme.symbol.as_ptr() as usize - text.as_ptr() as usize;
    }
    format!("…{}", &text[start..])
}

fn outcome_entries(app: &App) -> Vec<(String, Style)> {
    let mut lines = Vec::new();
    if let Some(summary) = &app.summary {
        lines.push((
            report::terminal_safe(&report::summary_headline(summary)),
            app.theme
                .bold(match (app.errors.is_empty(), summary.items_touched) {
                    (true, _) => Token::Success,
                    (false, 0) => Token::Critical,
                    (false, _) => Token::Warning,
                }),
        ));
        if !app.excluded.is_empty() {
            lines.push((
                format!(
                    "{} left out of this plan and not touched.",
                    app.excluded.len()
                ),
                app.theme.style(Token::Muted),
            ));
        }
        lines.push((String::new(), Style::default()));
        for note in &summary.notes {
            lines.push((
                format!("• {}", report::terminal_safe(note)),
                Style::default(),
            ));
        }
    }
    if !app.errors.is_empty() {
        lines.push((String::new(), Style::default()));
        lines.push(("Errors".into(), app.theme.bold(Token::Critical)));
        for error in &app.errors {
            lines.push((
                format!("• {}", report::terminal_safe(error)),
                Style::default(),
            ));
        }
    }
    lines
}

fn error_entries(app: &App) -> Vec<(String, Style)> {
    let mut lines = vec![(
        "Operation refused or failed".to_string(),
        app.theme.bold(Token::Critical),
    )];
    for error in &app.errors {
        lines.push((report::terminal_safe(error), Style::default()));
    }
    if !app.warnings.is_empty() {
        lines.push((String::new(), Style::default()));
        lines.push((
            "Warnings captured before the failure".into(),
            app.theme.style(Token::Warning),
        ));
        for warning in &app.warnings {
            lines.push((report::terminal_safe(warning), Style::default()));
        }
    }
    lines
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    // Progressive disclosure: the footer carries only the few keys that matter
    // on this screen, and `?` opens the complete reference. Listing everything
    // here would make the one key the operator needs harder to find.
    let keys: String = match app.screen {
        Screen::Menu => "↑/↓ navigate · Enter select · ? keys · q quit".into(),
        Screen::Results if app.choice_count() == 0 => {
            "↑/↓ move · r rescan · b back · ? keys".into()
        }
        Screen::Results => {
            // `A` toggles, so the label names what the next press does.
            let toggle = if app.excluded.is_empty() {
                "none"
            } else {
                "all"
            };
            let mode = if app.can_toggle_shred() {
                "s permanent"
            } else {
                "r rescan"
            };
            format!("Space pick · A {toggle} · a apply · {mode} · b back · ? keys")
        }
        Screen::Detail => "↑/↓ or j/k scroll · Esc back to the results · ? keys".into(),
        Screen::Confirm => "Esc cancel · type the exact requested acknowledgment".into(),
        Screen::Outcome => "↑/↓ or j/k scroll · b back to menu · ? keys".into(),
        Screen::Error => "↑/↓ or j/k scroll · r rescan · b back to menu · ? keys".into(),
        Screen::Loading if applying(app) => {
            "Applying the approved plan · q works once it returns".into()
        }
        Screen::Loading => "q or Ctrl+C stops the scan and quits".into(),
    };
    // The results status names keys that act on the plan; none of them works
    // in the detail view or while working, so the footer says what does.
    let status = match app.screen {
        Screen::Detail => "Reading only: nothing here changes the plan.",
        Screen::Loading if applying(app) => "Quitting is refused until this apply returns.",
        Screen::Loading => "Other keys are ignored until the results appear.",
        _ => app.status.as_str(),
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(keys, app.theme.style(Token::AccentSecondary)),
            Line::styled(status, app.theme.style(Token::Warning)),
        ])
        .block(Block::bordered()),
        area,
    );
}

fn applying(app: &App) -> bool {
    matches!(app.working, Some((Work::Apply, _)))
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

    #[test]
    fn scan_error_results_preserve_failure_across_every_route() {
        for operation in [
            Operation::ScanAll,
            Operation::Purge,
            Operation::Clean(Target::NodeModules),
            Operation::Clean(Target::Artifacts),
        ] {
            let mut app = App::default();
            app.finish_results(
                operation,
                vec![
                    Finding::new("not judged", None, 0, "failed", 0, Action::Info)
                        .with_scan_error("synthetic failure".into()),
                ],
                Vec::new(),
                Vec::new(),
            );
            assert!(app.failed, "PV tui/scan-error-results");
            assert!(!app.has_actionable_findings());
            app.finish_results(operation, Vec::new(), Vec::new(), Vec::new());
            assert!(app.failed, "PV tui/scan-error-results");
        }
        let mut control = App::default();
        control.finish_results(Operation::Purge, Vec::new(), Vec::new(), Vec::new());
        assert!(!control.failed);
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
            Screen::Detail,
            Screen::Confirm,
            Screen::Outcome,
            Screen::Error,
        ];
        for screen in screens {
            for help in [false, true] {
                let app = App {
                    screen,
                    help,
                    working: Some((Work::Scan, Instant::now())),
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

    /// At the supported minimum size the reference must still show its last
    /// line, or the bindings above it may be hidden with no sign they are.
    #[test]
    fn help_overlay_shows_every_binding_at_the_minimum_size() {
        for screen in [Screen::Menu, Screen::Results, Screen::Detail] {
            let mut app = cleanup(vec![Finding::new(
                "node_modules",
                Some(std::path::PathBuf::from("/tmp/x")),
                1,
                "stale",
                5,
                Action::Trash,
            )]);
            app.screen = screen;
            app.handle_key(key(KeyCode::Char('?')));
            let frame = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT).concat();
            for (_, keys) in HELP_KEYS {
                for (binding, _) in *keys {
                    assert!(
                        frame.contains(binding),
                        "{screen:?}: the reference hides {binding} at the minimum size"
                    );
                }
            }
            assert!(
                frame.contains("Nothing is applied without an explicit"),
                "{screen:?}: the reference's last line is cut at the minimum size"
            );
        }
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
        assert!(approved_plan_matches(&app, &plan));
        let ctx = Ctx {
            yes: false,
            yolo: false,
            json: false,
            roots: Vec::new(),
            roots_origin: crate::safety::RootsOrigin::Default,
            active_days: 30,
            retain_days: 30,
            protect: Vec::new(),
            journal_path: std::path::PathBuf::from("/tmp/devtrim-tui-test-journal.jsonl"),
            home: std::path::PathBuf::from("/Users/example"),
            interactive: true,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };

        app.finish_apply(run_apply(&ctx, plan));

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

    fn scan_everything(findings: Vec<Finding>, sections: Vec<report::ScanSection>) -> App {
        let mut app = App::default();
        app.finish_scan(
            ops::ScanResult {
                findings,
                sections,
                errors: vec!["docker: failed".into()],
            },
            Vec::new(),
        );
        app
    }

    fn section(category: &'static str, range: std::ops::Range<usize>) -> report::ScanSection {
        report::ScanSection { category, range }
    }

    /// Every category a scan reports routes to its own menu entry, and the
    /// whole explanation fits the footer at the minimum terminal size, which
    /// does not wrap and would cut a longer line off silently.
    #[test]
    fn every_scan_category_routes_to_its_menu_entry_within_the_minimum_width() {
        for op in ops::all() {
            let category = op.name();
            let mut app = scan_everything(vec![trash("x", 1, 2)], vec![section(category, 0..1)]);
            app.handle_key(key(KeyCode::Char('A')));
            let entry = MENU
                .iter()
                .find(|item| item.operation.name() == category)
                .unwrap_or_else(|| panic!("{category} has no menu entry"));
            if entry.operation.read_only() {
                assert!(app.status.contains("nothing to apply"), "{category}");
            } else {
                assert_eq!(
                    app.status,
                    format!(
                        "Read-only scan. To act on it: b, then {} ({category}).",
                        entry.key
                    )
                );
            }
            let output = rendered(&app, 64, 18);
            assert!(output.contains(&app.status), "{category}: {output}");
        }
    }

    /// The footer does not wrap, so at the minimum width each key line and the
    /// results status must fit whole or lose their last words silently.
    #[test]
    fn every_results_footer_fits_the_minimum_width() {
        let shred = |label: &str| {
            let mut finding = trash(label, 1, 2);
            finding.action = Action::Shred;
            finding
        };
        let trash_plan = || cleanup(vec![trash("first", 1, 2), trash("second", 1, 2)]);
        let shred_plan = || cleanup(vec![shred("first"), shred("second")]);
        let (mut trash_left_out, mut shred_left_out) = (trash_plan(), shred_plan());
        trash_left_out.handle_key(key(KeyCode::Char(' ')));
        shred_left_out.handle_key(key(KeyCode::Char(' ')));
        let mut nothing_selected = trash_plan();
        nothing_selected.handle_key(key(KeyCode::Char('A')));
        // On a warning row, where Space cannot add anything.
        nothing_selected.warnings = vec!["caches: a warning".into()];
        nothing_selected.handle_key(key(KeyCode::End));
        nothing_selected.handle_key(key(KeyCode::Char('a')));
        assert_eq!(
            nothing_selected.status,
            "Nothing is selected: Space adds an item, A selects all."
        );
        let mut permanent = trash_plan();
        permanent.handle_key(key(KeyCode::Char('s')));
        assert!(permanent.shred);
        let mut restored = trash_plan();
        restored.handle_key(key(KeyCode::Char('s')));
        restored.handle_key(key(KeyCode::Char('s')));
        assert!(!restored.shred);
        // Leaving every Trash item out must not strand the view in permanent
        // mode, where adding an item back would plan a permanent deletion.
        let mut restored_with_nothing_selected = trash_plan();
        restored_with_nothing_selected.handle_key(key(KeyCode::Char('s')));
        restored_with_nothing_selected.handle_key(key(KeyCode::Char('A')));
        restored_with_nothing_selected.handle_key(key(KeyCode::Char('s')));
        assert!(
            !restored_with_nothing_selected.shred,
            "PV tui/shred-off: permanent mode cannot be turned off with nothing selected"
        );
        // The statuses `A` and a Space on a report row leave behind.
        let mut all_left_out = trash_plan();
        all_left_out.handle_key(key(KeyCode::Char('A')));
        let mut report_row = cleanup(vec![
            Finding::new("disclosure", None, 1, "test", 0, Action::None),
            trash("deletable", 1, 2),
        ]);
        report_row.handle_key(key(KeyCode::Char(' ')));
        assert!(!all_left_out.status.is_empty() && !report_row.status.is_empty());
        // A result that failed says what failed and that r retries, so a
        // category withheld by a failed probe is not mistaken for an empty one.
        let failed_scan = |failed: &[&str]| {
            let mut app = App::default();
            app.finish_scan(
                ops::ScanResult {
                    findings: vec![trash("cache", 1, 2)],
                    sections: ops::all()
                        .iter()
                        .map(|op| op.name())
                        .filter(|name| !failed.contains(name))
                        .map(|name| section(name, 0..0))
                        .collect(),
                    errors: failed
                        .iter()
                        .map(|name| format!("{name}: probe failed"))
                        .collect(),
                },
                Vec::new(),
            );
            app
        };
        let one_category = failed_scan(&["node-modules"]);
        let every_category: Vec<_> = ops::all().iter().map(|op| op.name()).collect();
        let every_category = failed_scan(&every_category);
        let not_judged = cleanup(
            (0..12)
                .map(|index| {
                    Finding::new(
                        format!("repo-{index} not judged"),
                        None,
                        0,
                        "x",
                        5,
                        Action::Info,
                    )
                    .with_scan_error("synthetic failure".into())
                })
                .collect(),
        );
        for (app, what) in [
            (&one_category, "node-modules failed"),
            (&every_category, "10 categories failed"),
            (&not_judged, "12 item(s) not judged"),
        ] {
            assert!(
                app.status.starts_with(what) && app.status.contains("r retries"),
                "the status does not say what failed: {}",
                app.status
            );
        }
        for app in [
            trash_plan(),
            trash_left_out,
            shred_plan(),
            shred_left_out,
            nothing_selected,
            permanent,
            restored,
            restored_with_nothing_selected,
            all_left_out,
            report_row,
            one_category,
            every_category,
            not_judged,
        ] {
            let output = rendered(&app, 64, 18);
            assert!(output.contains("? keys"), "{output}");
            assert!(output.contains(&app.status), "{output}");
        }
    }

    /// The outcome screen's status shares the footer, which does not wrap.
    #[test]
    fn every_outcome_status_fits_the_minimum_footer() {
        for (failed, touched) in [(false, 1), (true, 0), (true, 1)] {
            let status = outcome_status(failed, touched);
            assert!(status.chars().count() <= 62, "{status}");
        }
        assert!(FAILED_CLOSED.chars().count() <= 62, "{FAILED_CLOSED}");
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
    /// lines it hides and which key shows them, instead of cutting them
    /// silently, and the selection state comes before the path so it is never
    /// among them.
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
            pane.contains("… 5 more line(s); Enter shows all"),
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

    /// The results list's title row at the minimum size.
    fn results_title(app: &App) -> String {
        screen_rows(app, MIN_WIDTH, MIN_HEIGHT)
            .into_iter()
            .find(|row| row.starts_with("┌ TRASH-FIRST") || row.starts_with("┌ PERMANENT"))
            .expect("the results list has a title")
    }

    /// The mode is a safety signal, so it leads the title: a title that grew
    /// with the plan's counts and size clipped TRASH-FIRST at 64 columns. The
    /// count of what failed must fit beside it whole, as must the danger; the
    /// title's closing space shows nothing after it was cut.
    #[test]
    fn the_title_fits_the_minimum_width_on_a_large_plan_with_failures() {
        let mut findings: Vec<Finding> = (0..120)
            .map(|index| trash(&format!("cache-{index}"), 1 << 30, 5))
            .collect();
        findings.extend((0..12).map(|index| {
            Finding::new(
                format!("repo-{index} not judged"),
                None,
                0,
                "failed",
                5,
                Action::Info,
            )
            .with_scan_error("synthetic failure".into())
        }));
        let mut app = cleanup(findings);

        let trash_first = results_title(&app);
        app.handle_key(key(KeyCode::Char('s')));
        let permanent = results_title(&app);
        let mut scan = App::default();
        scan.finish_scan(
            ops::ScanResult {
                findings: (0..999)
                    .map(|index| trash(&format!("cache-{index}"), 1 << 30, 5))
                    .collect(),
                sections: Vec::new(),
                errors: (0..10)
                    .map(|index| format!("category-{index}: failed"))
                    .collect(),
            },
            Vec::new(),
        );
        let read_only = results_title(&scan);

        for (title, failed) in [
            (&trash_first, "12 failed "),
            (&permanent, "12 failed "),
            (&read_only, "10 failed "),
        ] {
            assert!(title.contains(" · danger-"), "{title}");
            assert!(
                title.contains(failed),
                "the failure count is cut or missing: {title}"
            );
        }
        assert!(trash_first.contains("TRASH-FIRST"), "{trash_first}");
        assert!(permanent.contains("PERMANENT"), "{permanent}");
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
}
