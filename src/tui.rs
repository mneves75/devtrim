//! Interactive Ratatui adapter over the existing scan, preview, and apply core.

use std::io::{self, IsTerminal};
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::cli::Target;
use crate::ops::{self, Action, ApplyOutcome, Finding, Op};
use crate::process::{self, Interrupts};
use crate::report::{self, Summary};
use crate::safety::{self, ConfirmationRequirement, Ctx};
use crate::theme::{Theme, Token, danger_token};
use crate::volume::FreeSpace;

const MIN_WIDTH: u16 = 64;
const MIN_HEIGHT: u16 = 18;
/// Rows PgUp and PgDn move, in every list and scrolling view.
pub(crate) const PAGE_ROWS: u16 = 8;
/// Longer than every typed acknowledgment (`PURGE ` and a `u64` of
/// gigabytes), so a held key or a stuck repeat cannot grow the answer
/// without bound.
const MAX_CONFIRMATION_INPUT: usize = 32;
/// The Error screen's status after a scan or a refused approval, neither of
/// which changes anything.
const NOTHING_CHANGED: &str = "Nothing was changed. r rescan · b back";
/// The Error screen's status after an apply that stopped with an error: some
/// targets may already be gone, and the journal is the record of which.
const APPLY_STOPPED: &str = "The apply did not finish; devtrim history shows what changed.";
/// The menu's status, before anything has been scanned.
const MENU_STATUS: &str = "Preview first. Nothing changes until you explicitly approve.";

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

    /// What a scan of this operation looks at, as the working screen says it.
    fn subject(self) -> &'static str {
        match self {
            Self::ScanAll => "everything",
            Self::Purge => "projects to purge",
            Self::Icloud => "iCloud files",
            Self::TrashEmpty => "the Trash",
            Self::Clean(target) => match target {
                Target::Caches => "caches",
                Target::NodeModules => "node_modules",
                Target::Artifacts => "build artifacts",
                Target::Simulators => "simulators",
                Target::Xcode => "Xcode data",
                Target::Docker => "Docker",
                Target::Toolchains => "Swift toolchains",
                Target::Installers => "installers",
                Target::Agents => "agent caches and history",
                Target::Leftovers => "agent leftovers",
            },
        }
    }

    /// The operation's menu label, which the header names as the current view.
    fn label(self) -> &'static str {
        MENU.iter()
            .find(|item| item.operation == self)
            .map_or_else(|| self.name(), |item| item.label)
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
    /// How much the status matters, set with it by [`App::say`].
    status_token: Token,
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
    /// How far the apply on the worker thread has gone, read from the journal
    /// as each attempt begins and finishes; it also carries the stop request.
    apply_monitor: Option<std::sync::Arc<crate::journal::ApplyMonitor>>,
    /// Each category of a Scan everything in progress, as it finishes.
    scan_board: Option<std::sync::Arc<ScanBoard>>,
    /// The home folder, shown as `~` in the path column; `None` shows paths
    /// as they are.
    home: Option<std::path::PathBuf>,
    /// The home volume's free space as last read for the menu, or why it
    /// could not be; `None` before any read, as in a test.
    disk: Option<Result<FreeSpace, String>>,
    /// The home volume's local snapshots, answered on a worker thread.
    snapshots: Snapshots,
    /// What the last Scan everything found per category, for the menu.
    overview: Option<Overview>,
}

/// The local-snapshot count as the menu shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Snapshots {
    /// Never asked, as in a test: the menu says nothing about snapshots.
    NotAsked,
    Checking,
    Counted(usize),
    Unavailable(String),
}

/// The last Scan everything of this session, as the menu rows show it. A
/// category applied since then no longer has a size worth showing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Overview {
    totals: Vec<report::CategoryTotal>,
    failed: Vec<&'static str>,
    applied: std::collections::BTreeSet<&'static str>,
}

/// What a menu row says about the last Scan everything.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RowSize {
    /// Not a scanned category, or nothing scanned yet: the row is as before.
    Blank,
    Failed,
    /// Applied after the scan, so its size is stale.
    Rescan,
    /// Scanned and offering nothing.
    Nothing,
    /// Scanned, reporting findings no apply acts on.
    Report {
        findings: usize,
    },
    /// What it offers: the size, or the range from the lower bound, and how
    /// many findings.
    Offers {
        size: String,
        findings: usize,
        at_least: Option<u64>,
        bytes: u64,
    },
}

impl Overview {
    fn of(result: &ops::ScanResult) -> Self {
        Self {
            totals: report::category_totals(&result.findings, &result.sections),
            failed: ops::failed_categories(&result.sections),
            applied: std::collections::BTreeSet::new(),
        }
    }

    /// Marks the categories an apply of `operation` may have changed.
    fn mark_applied(&mut self, operation: Operation) {
        let names = match operation {
            Operation::Clean(target) => vec![target.as_str()],
            Operation::Purge => vec![Target::NodeModules.as_str(), Target::Artifacts.as_str()],
            Operation::ScanAll | Operation::Icloud | Operation::TrashEmpty => Vec::new(),
        };
        for name in &names {
            if let Some(total) = self.totals.iter().find(|total| total.category == *name) {
                self.applied.insert(total.category);
            }
        }
    }

    fn offers(at_least: u64, bytes: u64, findings: usize) -> RowSize {
        let lower = (at_least < bytes).then_some(at_least);
        RowSize::Offers {
            size: lower.map_or_else(
                || report::gb(bytes),
                |at_least| report::size_range(at_least, bytes),
            ),
            findings,
            at_least: lower,
            bytes,
        }
    }

    fn row(&self, operation: Operation) -> RowSize {
        if operation == Operation::ScanAll {
            if !self.applied.is_empty() {
                return RowSize::Rescan;
            }
            let (bytes, at_least, findings) = self.totals.iter().fold(
                (0u64, 0u64, 0usize),
                |(bytes, at_least, findings), total| {
                    (
                        bytes.saturating_add(total.actionable_bytes),
                        at_least.saturating_add(total.frees_at_least_bytes),
                        findings + total.findings,
                    )
                },
            );
            return Self::offers(at_least, bytes, findings);
        }
        let Operation::Clean(target) = operation else {
            return RowSize::Blank;
        };
        let name = target.as_str();
        if self.failed.contains(&name) {
            return RowSize::Failed;
        }
        if self.applied.contains(name) {
            return RowSize::Rescan;
        }
        match self.totals.iter().find(|total| total.category == name) {
            None => RowSize::Blank,
            Some(total) if !total.offers && total.errors > 0 => RowSize::Failed,
            Some(total) if !total.offers && total.findings > 0 => RowSize::Report {
                findings: total.findings,
            },
            Some(total) if !total.offers => RowSize::Nothing,
            Some(total) => Self::offers(
                total.frees_at_least_bytes,
                total.actionable_bytes,
                total.findings,
            ),
        }
    }

    /// Fresh reclaimable total across categories that offer savings, formatted
    /// as a range when APFS clones share part of it. Absent once any category
    /// has been applied since the scan, so stale totals are never advertised.
    fn reclaimable_summary(&self) -> Option<(String, usize)> {
        if !self.applied.is_empty() {
            return None;
        }
        let (bytes, at_least, categories) = self.totals.iter().fold(
            (0u64, 0u64, 0usize),
            |(bytes, at_least, categories), total| {
                let counted = usize::from(total.offers && total.actionable_bytes > 0);
                (
                    bytes.saturating_add(total.actionable_bytes),
                    at_least.saturating_add(total.frees_at_least_bytes),
                    categories + counted,
                )
            },
        );
        if bytes == 0 || categories == 0 {
            return None;
        }
        let size = if at_least < bytes {
            report::size_range(at_least, bytes)
        } else {
            report::gb(bytes)
        };
        Some((size, categories))
    }
}

impl RowSize {
    /// The row's text in at most `width` columns: the size and count, then
    /// the size alone, then a range's lower bound, then nothing. A range is
    /// never cut to its upper end.
    fn text(&self, width: usize) -> Option<String> {
        let candidates: Vec<String> = match self {
            Self::Blank => Vec::new(),
            Self::Failed => vec!["failed".into()],
            Self::Rescan => vec!["rescan".into()],
            Self::Nothing => vec!["none".into()],
            Self::Report { findings } => vec![format!("report · {findings}"), "report".into()],
            Self::Offers {
                size,
                findings,
                at_least,
                ..
            } => {
                let mut candidates = vec![format!("{size} · {findings}"), size.clone()];
                // Too narrow for the range: its lower bound, never its upper.
                if let Some(at_least) = at_least {
                    candidates.push(format!("≥{}", report::gb(*at_least)));
                }
                candidates
            }
        };
        candidates
            .into_iter()
            .find(|candidate| candidate.chars().count() <= width)
    }

    /// What the Selected pane says about the last scan of this row; the
    /// size line only when the row itself could not show it (`shown`).
    fn detail(&self, shown: bool) -> Vec<String> {
        match self {
            Self::Blank => Vec::new(),
            Self::Failed => vec!["Last scan: failed; open it to see why and retry.".into()],
            Self::Rescan => {
                vec!["Changed since the last scan; rescan for its size.".into()]
            }
            Self::Nothing => vec!["Last scan: nothing to give back.".into()],
            Self::Report { findings } => {
                vec![format!("Last scan: {findings} finding(s), report only.")]
            }
            Self::Offers {
                size,
                findings,
                at_least,
                ..
            } => {
                let mut lines = Vec::new();
                if !shown {
                    lines.push(format!("Last scan: {size} in {findings} finding(s)."));
                }
                if let Some(at_least) = at_least {
                    lines.push(format!(
                        "at least {} comes back; APFS clones share the rest",
                        report::gb(*at_least)
                    ));
                }
                lines
            }
        }
    }
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
            status: MENU_STATUS.into(),
            status_token: Token::Muted,
            failed: false,
            theme: Theme::from_env(),
            help: false,
            scroll_limit: std::cell::Cell::new(0),
            working: None,
            apply_monitor: None,
            scan_board: None,
            home: None,
            disk: None,
            snapshots: Snapshots::NotAsked,
            overview: None,
        }
    }
}

impl App {
    /// The findings still in the plan, in display order, with the actions the
    /// current mode gives them.
    fn effective_findings(&self) -> Vec<Finding> {
        let mut findings: Vec<Finding> = self.in_plan().cloned().collect();
        report::effective_actions(&mut findings, self.shred);
        findings
    }

    /// The previewed findings still in the plan, borrowed.
    fn in_plan(&self) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .enumerate()
            .filter(|(index, _)| !self.excluded.contains(index))
            .map(|(_, finding)| finding)
    }

    fn has_actionable_findings(&self) -> bool {
        self.in_plan().any(|finding| finding.action.is_actionable())
    }

    /// Whether `s` acts. Turning permanent mode on needs a selected item that
    /// moves to Trash; turning it off always works, so leaving every such item
    /// out never strands the operator in permanent mode.
    fn can_toggle_shred(&self) -> bool {
        self.shred
            || matches!(self.operation, Some(Operation::Clean(_) | Operation::Purge))
                && self
                    .in_plan()
                    .any(|finding| finding.action == Action::Trash)
    }

    /// Sets the status line and how much it matters: `Muted` for neutral
    /// information, `Warning` for a refused key or something to read first,
    /// `Critical` for a failure.
    fn say(&mut self, token: Token, text: impl Into<String>) {
        self.status = text.into();
        self.status_token = token;
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
        let status = format!(
            "{} of {} items selected.",
            self.choice_count() - self.excluded.len(),
            self.choice_count()
        );
        self.say(Token::Muted, status);
    }

    /// Everything back in when anything is out; otherwise every choice out.
    fn toggle_all(&mut self) {
        if self.excluded.is_empty() {
            self.excluded = (0..self.findings.len())
                .filter(|index| self.is_selectable(*index))
                .collect();
            self.say(
                Token::Muted,
                "Every item is left out; A selects them all again.",
            );
        } else {
            self.excluded.clear();
            self.say(
                Token::Muted,
                "Every item is selected; A leaves them all out.",
            );
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
        self.say(Token::Muted, format!("Scanning {}…", operation.subject()));
        self.working = Some((Work::Scan, Instant::now()));
        self.scan_board =
            (operation == Operation::ScanAll).then(|| std::sync::Arc::new(ScanBoard::new()));
    }

    fn begin_apply(&mut self, monitor: std::sync::Arc<crate::journal::ApplyMonitor>) {
        // Whatever the apply ends with, the last scan's size for what it
        // touched is no longer what is on disk.
        if let (Some(overview), Some(operation)) = (self.overview.as_mut(), self.operation) {
            overview.mark_applied(operation);
        }
        self.screen = Screen::Loading;
        self.say(Token::Muted, "Applying only the exact previewed findings…");
        self.working = Some((Work::Apply, Instant::now()));
        self.apply_monitor = Some(monitor);
    }

    /// `x` during an apply: the journal refuses every attempt after the one
    /// in progress, which always runs to its end. Asked once; a second press
    /// changes nothing.
    fn request_stop(&mut self) {
        let Some(monitor) = self.apply_monitor.as_ref().filter(|_| applying(self)) else {
            return;
        };
        if monitor.stop_requested() {
            return;
        }
        monitor.request_stop();
        let subject = if monitor.current().is_some_and(|item| item.command) {
            "command"
        } else {
            "item"
        };
        self.say(
            Token::Warning,
            format!("Stopping after the current {subject}…"),
        );
    }

    /// What `x` would wait for, as the footer names it.
    fn stop_key_label(&self) -> Option<&'static str> {
        let monitor = self.apply_monitor.as_ref().filter(|_| applying(self))?;
        if monitor.stop_requested() {
            return None;
        }
        Some(if monitor.current().is_some_and(|item| item.command) {
            "stops after this command"
        } else {
            "stop after this item"
        })
    }

    /// Changes when the working screen has something new to show.
    fn progress_version(&self) -> u64 {
        let applied = self
            .apply_monitor
            .as_ref()
            .map_or(0, |monitor| monitor.version());
        let scanned = self.scan_board.as_ref().map_or(0, |board| board.version());
        applied.wrapping_add(scanned)
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
                if outcome.stopped > 0 {
                    let status = format!(
                        "Stopped by you: {} done, {} not attempted.",
                        outcome.summary.items_touched, outcome.stopped
                    );
                    self.say(Token::Warning, status);
                } else {
                    let (token, status) =
                        outcome_status(!outcome.errors.is_empty(), outcome.summary.items_touched);
                    self.say(token, status);
                }
                self.summary = Some(outcome.summary);
                self.errors = outcome.errors;
                self.scroll = 0;
                self.screen = Screen::Outcome;
            }
            Err(error) => {
                self.fail(error);
                self.say(Token::Critical, APPLY_STOPPED);
            }
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
        self.overview = Some(Overview::of(&result));
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
        if let Some(failure) = self.failure_status() {
            self.say(Token::Critical, failure);
        } else if self.findings.is_empty() {
            // The list says nothing was found; saying it here too would only
            // repeat it.
            self.say(Token::Muted, "");
        } else if operation.read_only() {
            self.say(
                Token::Muted,
                "Read-only result. No apply action is available.",
            );
        } else {
            self.say(
                Token::Muted,
                "Review every finding. Space leaves one out; a applies the rest",
            );
        }
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
            let missing = ops::failed_categories(&self.sections);
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

    /// The menu entry for the Scan everything section holding the highlighted
    /// finding, or `None` for diagnostic rows and unsectioned results.
    fn scan_section_entry(&self) -> Option<&'static MenuItem> {
        if self.cursor >= self.findings.len() {
            return None;
        }
        self.sections
            .iter()
            .find(|section| section.range.contains(&self.cursor))
            .and_then(|section| {
                MENU.iter()
                    .find(|item| item.operation.name() == section.category)
            })
    }

    /// The actionable category operation for the highlighted Scan everything
    /// row; read-only categories (`leftovers`), informational findings inside
    /// actionable categories, and diagnostic rows have none.
    fn scan_jump_operation(&self) -> Option<Operation> {
        if self.operation != Some(Operation::ScanAll) {
            return None;
        }
        let finding = self.findings.get(self.cursor)?;
        if !finding.action.is_actionable() {
            return None;
        }
        let entry = self.scan_section_entry()?;
        (!entry.operation.read_only()).then_some(entry.operation)
    }

    /// Where the highlighted Scan everything row can be acted on: the menu
    /// entry of its own category, named only when that view can apply it.
    fn scan_route(&self) -> String {
        let Some(finding) = self.findings.get(self.cursor) else {
            return "Read-only scan. Open a category from the menu (b) to act.".into();
        };
        match self.scan_section_entry() {
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

    /// Shows a failure that changed nothing: a scan, or an approval that no
    /// longer matched the preview. An apply that failed says so itself.
    fn fail(&mut self, error: anyhow::Error) {
        self.failed = true;
        self.errors = vec![format!("{error:#}")];
        self.summary = None;
        self.scroll = 0;
        self.screen = Screen::Error;
        self.say(Token::Critical, NOTHING_CHANGED);
    }

    fn begin_confirmation(&mut self) {
        let Some(operation) = self.operation else {
            return;
        };
        if operation.read_only() {
            self.say(Token::Warning, "This result has no actionable findings.");
            return;
        }
        if !self.has_actionable_findings() {
            let reason = if self
                .findings
                .iter()
                .any(|finding| finding.action.is_actionable())
            {
                "Nothing is selected: Space adds an item, A selects all."
            } else {
                "This result has no actionable findings."
            };
            self.say(Token::Warning, reason);
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
        self.say(Token::Muted, "");
        self.screen = Screen::Confirm;
    }

    /// Reads the home volume's free space again; `statvfs` is instant.
    fn refresh_disk(&mut self) {
        if let Some(home) = &self.home {
            self.disk = Some(
                crate::volume::free_space(home)
                    .map_err(|error| report::terminal_safe(&format!("{error:#}"))),
            );
        }
    }

    fn back_to_menu(&mut self) {
        self.refresh_disk();
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
        self.say(Token::Muted, MENU_STATUS);
    }

    fn handle_key(&mut self, key: KeyEvent) -> Intent {
        if key.kind != KeyEventKind::Press {
            return Intent::None;
        }
        if is_interrupt(key) {
            return Intent::Quit;
        }
        // A modified key is not the letter it carries: Alt+y arrives as `ESC y`,
        // so a stray Escape before a `y` would approve a plan. Shift is part of
        // the typed character (`Y`, a typed `PURGE`).
        let modified = key.modifiers.difference(event::KeyModifiers::SHIFT);
        if !modified.is_empty() {
            return Intent::None;
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
                // load, and an apply leaves something to look at again; either
                // way scanning again is a fresh preview, never a retried apply.
                KeyCode::Char('r') => self.operation.map_or(Intent::None, Intent::Load),
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
            self.say(Token::Warning, reason);
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
                if self.shred {
                    self.say(
                        Token::Warning,
                        "Permanent mode: every action is SHRED; danger is critical.",
                    );
                } else {
                    self.say(Token::Muted, "Trash-first mode restored.");
                }
                Intent::None
            }
            KeyCode::Char('a') => {
                self.begin_confirmation();
                Intent::None
            }
            KeyCode::Char('o') if self.operation == Some(Operation::ScanAll) => {
                if let Some(target_operation) = self.scan_jump_operation() {
                    return Intent::Load(target_operation);
                }
                let reason = match self.scan_section_entry() {
                    Some(entry) if entry.operation.read_only() => {
                        "Read-only category: no actionable category view to open."
                    }
                    Some(_) => "Informational row: no actionable category view to open.",
                    None => "This row has no actionable category view to open.",
                };
                self.say(Token::Muted, reason);
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
            self.say(
                Token::Muted,
                "Apply canceled; the preview remains unchanged.",
            );
            return Intent::None;
        }
        match confirmation {
            ConfirmationKind::YesNo { .. } => match key {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.approve(Approval::Yes),
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.screen = Screen::Results;
                    self.say(
                        Token::Muted,
                        "Apply canceled; the preview remains unchanged.",
                    );
                    Intent::None
                }
                _ => Intent::None,
            },
            ConfirmationKind::Critical { expected_gb, .. } => match key {
                KeyCode::Char(value) if value.is_ascii_digit() => self.type_answer(value),
                KeyCode::Backspace => self.erase_answer(),
                KeyCode::Enter if self.input == expected_gb.to_string() => {
                    self.approve(Approval::CriticalGigabytes(expected_gb))
                }
                KeyCode::Enter => {
                    self.input.clear();
                    self.say(
                        Token::Warning,
                        "Confirmation mismatch. The plan was not applied.",
                    );
                    Intent::None
                }
                _ => Intent::None,
            },
            ConfirmationKind::TrashPurge { expected_gb } => match key {
                KeyCode::Char(value)
                    if value.is_ascii_alphanumeric() || value == ' ' || value == '-' =>
                {
                    self.type_answer(value)
                }
                KeyCode::Backspace => self.erase_answer(),
                KeyCode::Enter if self.input == format!("PURGE {expected_gb}") => {
                    self.approve(Approval::TrashPurgeGigabytes(expected_gb))
                }
                KeyCode::Enter => {
                    self.input.clear();
                    self.say(
                        Token::Warning,
                        "Confirmation mismatch. Trash was not purged.",
                    );
                    Intent::None
                }
                _ => Intent::None,
            },
        }
    }

    fn type_answer(&mut self, value: char) -> Intent {
        if self.input.len() < MAX_CONFIRMATION_INPUT {
            self.input.push(value);
        } else {
            self.say(
                Token::Warning,
                "Longer than any acknowledgment; Backspace erases.",
            );
        }
        Intent::None
    }

    fn erase_answer(&mut self) -> Intent {
        self.input.pop();
        self.say(Token::Muted, "");
        Intent::None
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
    // A paste arrives as one `Event::Paste`, which no screen acts on, instead
    // of a burst of keys that could type an approval.
    let result = crossterm::execute!(io::stdout(), event::EnableBracketedPaste)
        .context("cannot enable bracketed paste")
        .and_then(|()| run_loop(&mut terminal, ctx, &interrupts));
    crossterm::execute!(io::stdout(), event::DisableBracketedPaste).ok();
    let restore = ratatui::try_restore().context("cannot restore terminal after TUI exit");
    match (result, restore) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(code), Ok(())) => Ok(code),
    }
}

/// Whether `key` is Ctrl+C, which quits every full-screen view, as SIGINT
/// does, whatever screen is showing.
pub(crate) fn is_interrupt(key: KeyEvent) -> bool {
    key.modifiers.contains(event::KeyModifiers::CONTROL) && key.code == KeyCode::Char('c')
}

/// What every full-screen view does with a SIGINT, SIGTERM or SIGHUP that
/// [`process::forward_interrupts`] recorded: leave bracketed paste, restore
/// the terminal and show the cursor, then end the run as an interrupt does.
/// Only the thread that owns the terminal calls it.
pub(crate) fn restore_and_exit(signal: i32) -> ! {
    crossterm::execute!(
        io::stdout(),
        event::DisableBracketedPaste,
        crossterm::cursor::Show
    )
    .ok();
    ratatui::try_restore().ok();
    process::kill_and_exit(signal)
}

fn run_loop(
    terminal: &mut DefaultTerminal,
    ctx: &Ctx,
    interrupts: &Interrupts,
) -> Result<ExitCode> {
    let mut app = App {
        home: Some(ctx.home.clone()),
        snapshots: Snapshots::Checking,
        ..App::default()
    };
    app.refresh_disk();
    // `diskutil` can take a while; the menu draws without waiting for it.
    let (answer, snapshots) = std::sync::mpsc::channel();
    let home = ctx.home.clone();
    thread::spawn(move || {
        let counted = safety::local_snapshot_count(&home)
            .map_err(|error| report::terminal_safe(&format!("{error:#}")));
        answer.send(counted).ok();
    });
    let mut redraw = true;
    loop {
        if let Some(signal) = interrupts.received() {
            restore_and_exit(signal);
        }
        if let Ok(counted) = snapshots.try_recv() {
            app.snapshots = counted.map_or_else(Snapshots::Unavailable, Snapshots::Counted);
            redraw = true;
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
                let board = app.scan_board.clone();
                let loaded = in_background(terminal, &mut app, interrupts, || {
                    run_scan(ctx, operation, board.as_deref())
                })?;
                app.finish_load(operation, loaded);
                settle(terminal, &app)?;
            }
            Intent::Apply(plan) => {
                if approved_plan_matches(&app, &plan) {
                    let monitor = std::sync::Arc::new(crate::journal::ApplyMonitor::new(
                        plan.findings
                            .iter()
                            .filter(|finding| finding.action.is_actionable())
                            .count(),
                        report::actionable_bytes(&plan.findings),
                    ));
                    app.begin_apply(monitor.clone());
                    let result = in_background(terminal, &mut app, interrupts, || {
                        run_apply(ctx, plan, &monitor)
                    })?;
                    app.finish_apply(result);
                    // What the apply reported on the way, shown with its
                    // outcome rather than left for the next scan to carry.
                    app.warnings = ctx.take_diagnostics();
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
/// screen that does not exist yet, except a quit and, during an apply, `x`,
/// which asks the journal to refuse every attempt after the current one. During a scan, `q` or
/// Ctrl+C ends the run the way SIGINT does: the terminal is restored, every
/// command the scan started is killed and devtrim exits 130. A scan changes
/// nothing, so nothing is left half done.
///
/// During an apply, `q` and Ctrl+C are refused until it returns, and the
/// working screen says so: quitting could abandon a target half moved or half
/// deleted, and the operator can wait to see what changed. A SIGINT, SIGTERM or
/// SIGHUP still stops it at once, as it stops the CLI, because whoever sent it wants
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
    let applying = matches!(app.working, Some((Work::Apply, _)));
    let finished = thread::scope(|scope| -> Result<thread::Result<T>> {
        let worker = scope.spawn(job);
        let mut shown = None;
        while !worker.is_finished() {
            if let Some(signal) = interrupts.received() {
                restore_and_exit(signal);
            }
            let now = (app.elapsed_seconds(), app.progress_version());
            if shown != Some(now) {
                terminal.draw(|frame| render(frame, app))?;
                shown = Some(now);
            }
            if !event::poll(TICK).context("cannot poll terminal input")? {
                continue;
            }
            match event::read().context("cannot read terminal input")? {
                Event::Key(key) if scanning && is_quit_key(key) => {
                    restore_and_exit(signal_hook::consts::SIGINT)
                }
                Event::Key(key) if applying && is_stop_key(key) => {
                    app.request_stop();
                    shown = None;
                }
                Event::Resize(..) => shown = None,
                _ => {}
            }
        }
        Ok(worker.join())
    })?;
    app.working = None;
    app.apply_monitor = None;
    app.scan_board = None;
    match finished {
        Ok(value) => Ok(value),
        // The panic already restored the terminal through Ratatui's hook; end
        // the run as a panic on this thread always did.
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// `x` alone, which asks an apply to stop between attempts; Alt+x and the
/// like are not it.
fn is_stop_key(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press
        && key.code == KeyCode::Char('x')
        && key
            .modifiers
            .difference(event::KeyModifiers::SHIFT)
            .is_empty()
}

fn is_quit_key(key: KeyEvent) -> bool {
    key.kind == KeyEventKind::Press && (key.code == KeyCode::Char('q') || is_interrupt(key))
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

/// One category and how its scan ended, once it has.
type BoardRow = (&'static str, Option<Result<ops::CategoryTally, String>>);

/// Every category of a Scan everything, in registry order, with how it
/// ended once its scan thread returns; filled from those threads while the
/// working screen reads it.
struct ScanBoard {
    rows: std::sync::Mutex<Vec<BoardRow>>,
    version: std::sync::atomic::AtomicU64,
}

impl ScanBoard {
    fn new() -> Self {
        Self {
            rows: std::sync::Mutex::new(
                ops::all()
                    .iter()
                    .map(|operation| (operation.name(), None))
                    .collect(),
            ),
            version: std::sync::atomic::AtomicU64::new(0),
        }
    }

    fn record(&self, done: ops::CategoryDone) {
        let mut rows = self
            .rows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(row) = rows.iter_mut().find(|(name, _)| *name == done.category) {
            row.1 = Some(done.tally);
        }
        self.version
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }

    fn rows(&self) -> Vec<BoardRow> {
        self.rows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn version(&self) -> u64 {
        self.version.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// What a scan produced, carried from the worker thread to the screen.
enum Loaded {
    Scan(ops::ScanResult, Vec<String>),
    Findings(Vec<Finding>, Vec<String>),
    Failed(anyhow::Error, Vec<String>),
}

/// Scans for `operation` on the worker thread, touching no screen state; the
/// diagnostics it captured travel with the result, together with any still
/// queued from before it, such as the root, protect and journal warnings
/// `Ctx::from_cli` raises before the first scan.
fn run_scan(ctx: &Ctx, operation: Operation, board: Option<&ScanBoard>) -> Loaded {
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
            let record = |done: ops::CategoryDone| {
                if let Some(board) = board {
                    board.record(done);
                }
            };
            let result = ops::scan_all_observed(ctx, Some(&record));
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
            ops::filter_protected_findings(&mut findings, ctx);
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
    if is_interrupt(key) {
        return Intent::Quit;
    }
    match key.code {
        // The screen offers only q; Esc, which means back on every other
        // screen, never ends the session.
        KeyCode::Char('q') => Intent::Quit,
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
/// the displayed preview before it started. `monitor` watches it from the
/// journal, which also honours its stop request between attempts.
fn run_apply(
    ctx: &Ctx,
    plan: ApprovedPlan,
    monitor: &std::sync::Arc<crate::journal::ApplyMonitor>,
) -> Result<ApplyOutcome> {
    let ApprovedPlan {
        operation,
        findings,
        approval,
    } = plan;
    let read_only = || anyhow::anyhow!("refusing to apply a read-only operation");
    // Refused before the journal preflight, which may rotate the journal.
    if matches!(operation, Operation::ScanAll | Operation::Icloud) {
        return Err(read_only());
    }
    crate::journal::check_writable(ctx)?;
    let mut outcome = ctx.with_apply_monitor(monitor.clone(), || {
        crate::volume::measure_apply(&ctx.home, || match operation {
            Operation::Clean(target) => ops::for_target(target).apply(&findings, ctx),
            Operation::Purge => ops::purge::Purge.apply(&findings, ctx),
            Operation::TrashEmpty => apply_trash(ctx, &findings, approval),
            Operation::ScanAll | Operation::Icloud => Err(read_only()),
        })
    })?;
    // Before the journal's own errors join, which belong to finished items.
    outcome.settle_stop(monitor);
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
        render_too_small(frame, area, app.theme);
        return;
    }

    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(8),
        Constraint::Length(2),
    ])
    .areas(area);
    render_header(
        frame,
        header,
        app.theme,
        app.operation.map_or("Menu", Operation::label),
    );
    match app.screen {
        Screen::Menu => render_menu(frame, body, app),
        Screen::Loading => render_loading(frame, body, app),
        Screen::Results | Screen::Confirm => {
            let shown = Shown::of(app);
            render_results(frame, body, app, &shown);
            if app.screen == Screen::Confirm {
                render_confirmation(frame, body, app, &shown);
            }
        }
        Screen::Detail => render_scrolled(frame, body, app, "Details", detail_entries(app)),
        Screen::Outcome => render_scrolled(frame, body, app, "Apply outcome", outcome_entries(app)),
        Screen::Error => render_scrolled(frame, body, app, "Error", error_entries(app)),
    }
    render_footer(frame, footer, app);
    if app.help {
        render_help(frame, area, app);
    }
}

/// What a terminal below the supported size shows instead of any screen:
/// the size it needs and the one key it takes.
pub(crate) fn render_too_small(frame: &mut Frame, area: Rect, theme: Theme) {
    let message = Paragraph::new(Text::from(vec![
        Line::styled("devtrim — terminal too small", theme.bold(Token::Warning)),
        Line::raw(format!(
            "Need at least {MIN_WIDTH}×{MIN_HEIGHT}; current {}×{}.",
            area.width, area.height
        )),
        Line::raw("Resize the terminal, or press q to quit."),
    ]))
    .alignment(Alignment::Center)
    .wrap(Wrap { trim: true });
    frame.render_widget(message, area);
}

pub(crate) fn terminal_too_small(area: Rect) -> bool {
    area.width < MIN_WIDTH || area.height < MIN_HEIGHT
}

/// The one header row every full-screen view shares: the product and the
/// current view on the theme's bar, a step in tone rather than a box. A view
/// too long for the row keeps its end, where a path's leaf is. A cell of
/// margin on each side keeps the text off the screen's edge.
pub(crate) fn render_header(frame: &mut Frame, area: Rect, theme: Theme, view: &str) {
    let product = " devtrim ";
    let version = format!("v{} · ", env!("CARGO_PKG_VERSION"));
    let room = usize::from(area.width)
        .saturating_sub(Span::raw(product).width() + Span::raw(version.as_str()).width() + 1);
    let line = Line::from(vec![
        Span::styled(product, theme.style(Token::Strong)),
        Span::raw(version),
        Span::raw(truncate_left(&report::terminal_safe(view), room)),
    ]);
    frame.render_widget(Paragraph::new(line).style(theme.style(Token::Bar)), area);
}

/// The two footer rows every full-screen view shares: the keys this screen
/// takes, then the status in its own weight. Neither row wraps.
pub(crate) fn render_footer_rows(
    frame: &mut Frame,
    area: Rect,
    theme: Theme,
    keys: &[(&str, &str)],
    status: (&str, Token),
) {
    let width = usize::from(area.width.saturating_sub(2));
    let (text, token) = status;
    frame.render_widget(
        Paragraph::new(vec![
            key_line(theme, keys, width),
            Line::from(vec![
                Span::raw(" "),
                Span::styled(report::terminal_safe(text), theme.style(token)),
            ]),
        ]),
        area,
    );
}

/// The keys a narrow footer may leave to the `?` reference, first to go
/// first. Every other key stays: how to act, go back, quit and find the rest.
const DROPPABLE_KEYS: [&str; 5] = ["Enter", "r", "s", "↑↓", "o"];

/// Keys in the one footer form, `key verb`, separated by ` · `, within
/// `width` columns after a cell of margin. A line too wide drops keys in the
/// order of [`DROPPABLE_KEYS`], never any other.
pub(crate) fn key_line(theme: Theme, keys: &[(&str, &str)], width: usize) -> Line<'static> {
    const SEPARATOR: &str = " · ";
    let line_width = |keys: &[(&str, &str)]| {
        keys.iter()
            .map(|(key, verb)| {
                Span::raw(*key).width()
                    + if verb.is_empty() {
                        0
                    } else {
                        1 + Span::raw(*verb).width()
                    }
            })
            .sum::<usize>()
            + Span::raw(SEPARATOR).width() * keys.len().saturating_sub(1)
    };
    let mut kept = keys.to_vec();
    for droppable in DROPPABLE_KEYS {
        if line_width(&kept) <= width {
            break;
        }
        kept.retain(|(key, _)| *key != droppable);
    }
    let mut spans = vec![Span::raw(" ")];
    for (index, (key, verb)) in kept.iter().enumerate() {
        if index > 0 {
            spans.push(Span::styled(SEPARATOR, theme.style(Token::Muted)));
        }
        spans.push(Span::styled(
            key.to_string(),
            theme.style(Token::AccentSecondary),
        ));
        if !verb.is_empty() {
            spans.push(Span::raw(format!(" {verb}")));
        }
    }
    Line::from(spans)
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
            ("Enter, o", "open operation/item; o jumps to category"),
            ("b, Esc, ?", "back; Esc quits the menu; ? this reference"),
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
            ("x", "during an apply, stop after the current item"),
        ],
    ),
];

/// The status an apply leaves, and how much it matters: whether it failed,
/// and how many items it changed before it did.
fn outcome_status(failed: bool, touched: usize) -> (Token, &'static str) {
    if !failed {
        (
            Token::Muted,
            "Apply finished. The summary below lists what changed.",
        )
    } else if touched == 0 {
        (Token::Critical, "Apply failed before any item was changed.")
    } else {
        // Some categories continue past a refused item, so "stopped" would be
        // false; the summary is the record either way.
        (
            Token::Critical,
            "Apply finished with errors: see what changed and what failed.",
        )
    }
}

/// The badge beside a menu operation, its token, and what it means, which the
/// Selected pane spells out so the badge is never left to guess. The words
/// are the site's: READ-ONLY, TRASH, PERMANENT.
fn badge(operation: Operation) -> (&'static str, Token, &'static str) {
    match operation {
        _ if operation.read_only() => (
            "READ-ONLY",
            Token::Info,
            "READ-ONLY: it reports and never changes anything.",
        ),
        Operation::TrashEmpty => (
            "PERMANENT",
            Token::Critical,
            "PERMANENT: it deletes for good, after a typed size acknowledgment.",
        ),
        Operation::Clean(Target::Docker | Target::Simulators) => (
            "PERMANENT",
            Token::Critical,
            "PERMANENT: what you approve runs a command that cannot be undone.",
        ),
        _ => (
            "TRASH",
            Token::Success,
            "TRASH: what you approve moves to the Trash; nothing changes until you press a and confirm.",
        ),
    }
}

/// Width of the menu column: the selection mark, the key, the widest badge
/// and the longest label, so no badge is ever clipped.
const MENU_WIDTH: u16 = 2 + 2 + 10 + 22;

/// Columns the menu grows by once a scan has sizes to show, and the width the
/// Selected pane keeps however narrow the terminal.
const MENU_SIZE_COLUMNS: u16 = 18;
const SELECTED_MIN_WIDTH: u16 = 21;

fn render_menu(frame: &mut Frame, area: Rect, app: &App) {
    let grown = if app.overview.is_some() {
        MENU_SIZE_COLUMNS.min(
            area.width
                .saturating_sub(MENU_WIDTH + 1 + SELECTED_MIN_WIDTH),
        )
    } else {
        0
    };
    let [menu_area, _, detail_area] = Layout::horizontal([
        Constraint::Length(MENU_WIDTH + grown),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    let [title_area, list_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(menu_area);
    // Every row fits at the minimum size; a taller menu says it scrolls.
    let title = if usize::from(list_area.height) < MENU.len() {
        " Operations · ↑↓ scrolls"
    } else {
        " Operations"
    };
    frame.render_widget(
        Paragraph::new(Line::styled(title, app.theme.style(Token::Muted))),
        title_area,
    );
    // The label and the size share what is left after the highlight mark,
    // the key and the badge.
    let room = usize::from(menu_area.width.saturating_sub(2 + 2 + 10));
    let row_room = |item: &MenuItem| room.saturating_sub(item.label.chars().count() + 1);
    let items = MENU.iter().map(|item| {
        let (marker, token, _) = badge(item.operation);
        let mut spans = vec![
            Span::styled(format!("{} ", item.key), app.theme.style(Token::Muted)),
            Span::styled(format!("{marker:<9} "), app.theme.style(token)),
            Span::raw(item.label),
        ];
        let label = item.label.chars().count();
        let size = app.overview.as_ref().and_then(|overview| {
            let row = overview.row(item.operation);
            row.text(row_room(item)).map(|text| (text, row))
        });
        if let Some((text, row)) = size {
            let token = match row {
                RowSize::Failed => Token::Critical,
                RowSize::Rescan | RowSize::Nothing | RowSize::Report { .. } => Token::Muted,
                _ => Token::Accent,
            };
            let gap = room.saturating_sub(label + text.chars().count());
            spans.push(Span::raw(" ".repeat(gap)));
            spans.push(Span::styled(text, app.theme.style(token)));
        }
        ListItem::new(Line::from(spans))
    });
    // The mark and weight show the selection; a color over the whole row
    // would hide the badge's own.
    let list = List::new(items)
        .highlight_symbol("▶ ")
        .highlight_style(app.theme.style(Token::Strong));
    let mut state = ListState::default();
    state.select(Some(app.selected));
    frame.render_stateful_widget(list, list_area, &mut state);

    let block = Block::bordered().title(" Selected ");
    let inner = block.inner(detail_area);
    let width = usize::from(inner.width);
    let height = usize::from(inner.height);
    // The badge's meaning is a safety signal and the last scan's size is what
    // the row abbreviates, so both come before the description, which is the
    // first thing a short pane cuts.
    let selected_lines = |item: &MenuItem, spaced: bool| -> Vec<Line<'static>> {
        let (_, _, meaning) = badge(item.operation);
        let mut lines = wrapped(item.label, width, app.theme.bold(Token::Accent));
        if let Some(overview) = &app.overview {
            let row = overview.row(item.operation);
            // A size the row already shows is not repeated; what it cannot
            // show is said here.
            let shown = row.text(row_room(item)).is_some();
            for text in row.detail(shown) {
                lines.extend(wrapped(&text, width, Style::default()));
            }
        }
        if spaced {
            lines.push(Line::raw(""));
        }
        lines.extend(wrapped(meaning, width, app.theme.style(Token::Warning)));
        if spaced {
            lines.push(Line::raw(""));
        }
        lines.extend(wrapped(item.description, width, Style::default()));
        lines
    };
    // The form is chosen for the tallest entry, so the pane does not reflow
    // as the highlight moves: the fullest disk block that fits, with blank
    // rows between the parts and then without them, before a sparer block.
    let tallest = |spaced: bool| {
        MENU.iter()
            .map(|item| selected_lines(item, spaced).len())
            .max()
            .unwrap_or(0)
    };
    let forms = disk_forms(app, width);
    let summary = overview_summary_lines(app, width);
    let fits = |disk: &[Line<'static>], spaced: bool, with_summary: bool| {
        let summary_rows = if with_summary { summary.len() } else { 0 };
        let disk_gap = usize::from(spaced && !disk.is_empty());
        let summary_gap = usize::from(spaced && summary_rows > 0);
        disk.len() + disk_gap + summary_rows + summary_gap + tallest(spaced) <= height
    };
    let sparest = forms.len() - 1;
    let (chosen, spaced, with_summary) = (0..forms.len())
        .flat_map(|index| {
            if summary.is_empty() {
                vec![(index, true, false), (index, false, false)]
            } else {
                vec![
                    (index, true, true),
                    (index, false, true),
                    (index, true, false),
                    (index, false, false),
                ]
            }
        })
        .find(|(index, spaced, with_summary)| fits(&forms[*index], *spaced, *with_summary))
        .unwrap_or((sparest, false, false));
    let mut lines = forms[chosen].clone();
    if spaced && !lines.is_empty() {
        lines.push(Line::raw(""));
    }
    if with_summary {
        lines.extend(summary);
        if spaced {
            lines.push(Line::raw(""));
        }
    }
    lines.extend(selected_lines(&MENU[app.selected], spaced));
    frame.render_widget(Paragraph::new(Text::from(lines)).block(block), detail_area);
}

fn overview_summary_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let Some((size, categories)) = app
        .overview
        .as_ref()
        .and_then(Overview::reclaimable_summary)
    else {
        return Vec::new();
    };
    let noun = if categories == 1 {
        "category"
    } else {
        "categories"
    };
    let mut lines = wrapped(
        &format!("Reclaimable: {size} across {categories} {noun}"),
        width,
        app.theme.bold(Token::Accent),
    );
    lines.extend(wrapped(
        "Press p for Project purge, or pick a category",
        width,
        app.theme.style(Token::Muted),
    ));
    lines
}

/// The disk block from fullest to sparest: free space, bar and snapshot
/// line; without the bar; free space alone, each with the free-space line
/// whole and then in its one-row form. Always at least one form, empty when
/// nothing was read.
fn disk_forms(app: &App, width: usize) -> Vec<Vec<Line<'static>>> {
    let (first, short, bar) = match &app.disk {
        None => return vec![Vec::new()],
        Some(Ok(space)) => {
            let used = space.total.saturating_sub(space.available);
            let percent = u128::from(used) * 100 / u128::from(space.total.max(1));
            let free = report::gb(space.available);
            let first = wrapped(
                &format!(
                    "{free} free of {} · {percent}% used",
                    report::gb(space.total)
                ),
                width,
                Style::default(),
            );
            // The one-row form, as short as the pane needs.
            let short = [
                format!("{free} free · {percent}% used"),
                format!("{free} free · {percent}%"),
            ]
            .iter()
            .map(|text| wrapped(text, width, Style::default()))
            .min_by_key(|rows| (rows.len() > 1, rows.len()))
            .unwrap_or_default();
            let filled = crate::analyze::bar(used, space.total, width);
            let rest = width.saturating_sub(filled.chars().count());
            let bar = Line::from(vec![
                Span::styled(filled, app.theme.style(Token::Accent)),
                Span::styled("·".repeat(rest), app.theme.style(Token::Muted)),
            ]);
            (first, short, vec![bar])
        }
        Some(Err(reason)) => {
            let line = wrapped(
                &format!("disk: unavailable ({reason})"),
                width,
                app.theme.style(Token::Warning),
            );
            (line.clone(), line, Vec::new())
        }
    };
    let snapshots = match &app.snapshots {
        Snapshots::NotAsked => None,
        Snapshots::Checking => Some(("checking snapshots…".to_string(), Token::Muted)),
        Snapshots::Counted(0) => Some(("no local snapshots".to_string(), Token::Muted)),
        Snapshots::Counted(1) => Some((
            "1 local snapshot holds space no scan can see".to_string(),
            Token::Warning,
        )),
        Snapshots::Counted(count) => Some((
            format!("{count} local snapshots hold space no scan can see"),
            Token::Warning,
        )),
        Snapshots::Unavailable(reason) => {
            Some((format!("snapshots: unavailable ({reason})"), Token::Muted))
        }
    };
    let snapshots: Vec<Line<'static>> = snapshots
        .map(|(text, token)| wrapped(&text, width, app.theme.style(token)))
        .unwrap_or_default();
    let shortest = if short.len() < first.len() {
        short
    } else {
        first.clone()
    };
    vec![
        [first.clone(), bar.clone(), snapshots.clone()].concat(),
        [shortest.clone(), bar, snapshots.clone()].concat(),
        [first.clone(), snapshots.clone()].concat(),
        [shortest.clone(), snapshots].concat(),
        first,
        shortest,
    ]
}

/// Prose rows for the Selected pane, through [`wrap_words`] so the pane
/// knows how tall it is before it draws.
fn wrapped(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    wrap_words(text, width.max(1))
        .into_iter()
        .map(|row| Line::styled(row, style))
        .collect()
}

const PROGRESS_BAR_WIDTH: usize = 24;

fn progress_bar_line(
    theme: Theme,
    done: usize,
    total: usize,
    inner_width: usize,
) -> Option<Line<'static>> {
    if total == 0 {
        return None;
    }
    let bar_width = PROGRESS_BAR_WIDTH.min(inner_width.saturating_sub(8));
    if bar_width == 0 {
        return None;
    }
    let clamped = done.min(total);
    let done_count = u64::try_from(clamped).unwrap_or(u64::MAX);
    let total_count = u64::try_from(total).unwrap_or(u64::MAX);
    let filled = crate::analyze::bar(done_count, total_count, bar_width);
    let rest = bar_width.saturating_sub(filled.chars().count());
    let percent = u128::from(done_count) * 100 / u128::from(total_count);
    Some(Line::from(vec![
        Span::styled(filled, theme.style(Token::Accent)),
        Span::styled("·".repeat(rest), theme.style(Token::Muted)),
        Span::styled(format!(" {percent}%"), theme.style(Token::Muted)),
    ]))
}

fn render_loading(frame: &mut Frame, area: Rect, app: &App) {
    let inner = usize::from(area.width.saturating_sub(2));
    let mut lines = vec![Line::raw(app.status.as_str())];
    if let Some(monitor) = app.apply_monitor.as_ref().filter(|_| applying(app)) {
        if let Some(bar) =
            progress_bar_line(app.theme, monitor.finished(), monitor.total_findings, inner)
        {
            lines.push(bar);
        }
        lines.push(Line::raw(apply_progress_line(app, monitor, inner)));
    }
    // The clock moving is what says the scan is still alive; a probe can take
    // minutes on a loaded machine.
    let board = app.scan_board.as_ref().map(|board| board.rows());
    if let Some(rows) = &board {
        let done = rows.iter().filter(|(_, state)| state.is_some()).count();
        if let Some(bar) = progress_bar_line(app.theme, done, rows.len(), inner) {
            lines.push(bar);
        }
    }
    if let Some(elapsed) = app.elapsed_seconds() {
        let clock = match &board {
            Some(rows) => format!(
                "{elapsed} s elapsed · {} of {} categories done",
                rows.iter().filter(|(_, state)| state.is_some()).count(),
                rows.len()
            ),
            None => format!("{elapsed} s elapsed"),
        };
        lines.push(Line::styled(clock, app.theme.style(Token::Muted)));
    }
    if let Some(rows) = board {
        lines.extend(scan_board_lines(app, &rows, inner));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .alignment(Alignment::Center)
            .block(Block::bordered().title(" Working ")),
        area,
    );
}

/// One row per category in registry order, each saying in words whether it
/// is done (what it offers), failed (why) or still waiting, so the state reads
/// without color. Padded to one width so the centered rows line up.
fn scan_board_lines(app: &App, rows: &[BoardRow], width: usize) -> Vec<Line<'static>> {
    let texts: Vec<(String, Token)> = rows
        .iter()
        .map(|(name, state)| {
            let (state, token) = match state {
                None => ("waiting".to_string(), Token::Muted),
                Some(Ok(tally)) => {
                    let mut text = format!(
                        "{} · {} finding{}",
                        report::gb(tally.actionable_bytes),
                        tally.findings,
                        if tally.findings == 1 { "" } else { "s" }
                    );
                    if tally.unjudged > 0 {
                        text.push_str(&format!(" · {} not judged", tally.unjudged));
                    }
                    (text, Token::Success)
                }
                Some(Err(reason)) => {
                    let reason = reason.lines().next().unwrap_or_default();
                    (
                        format!("failed: {}", report::terminal_safe(reason)),
                        Token::Critical,
                    )
                }
            };
            (truncate_right(&format!("{name:<12} {state}"), width), token)
        })
        .collect();
    let widest = texts
        .iter()
        .map(|(text, _)| Span::raw(text.as_str()).width())
        .max()
        .unwrap_or(0);
    texts
        .into_iter()
        .map(|(text, token)| {
            let pad = widest.saturating_sub(Span::raw(text.as_str()).width());
            Line::styled(format!("{text}{}", " ".repeat(pad)), app.theme.style(token))
        })
        .collect()
}

/// `n of N · X of Y · <category> <item>` within `width` columns: the counts
/// whole, then the item in progress keeping its end, where a path names it.
fn apply_progress_line(app: &App, monitor: &crate::journal::ApplyMonitor, width: usize) -> String {
    let counts = format!(
        "{} of {} · {} of {}",
        monitor.finished(),
        monitor.total_findings,
        report::gb(monitor.bytes_finished()),
        report::gb(monitor.total_bytes)
    );
    let Some(item) = monitor.current() else {
        return truncate_right(&counts, width);
    };
    let subject = match (&app.home, item.command) {
        (Some(home), false) => match std::path::Path::new(&item.subject).strip_prefix(home) {
            Ok(below) => format!("~/{}", below.display()),
            Err(_) => item.subject.clone(),
        },
        _ => item.subject.clone(),
    };
    let head = format!("{counts} · {} ", report::terminal_safe(&item.op));
    let room = width.saturating_sub(Span::raw(head.as_str()).width());
    if room < 8 {
        return truncate_right(&counts, width);
    }
    format!(
        "{head}{}",
        truncate_left(&report::terminal_safe(&subject), room)
    )
}

/// The plan as the current mode would apply it, computed once per frame: the
/// title, the confirmation and the mode all read from it, so they can never
/// describe different plans.
struct Shown {
    /// The selected findings, with the action and danger the mode gives them:
    /// exactly what an approval covers.
    plan: Vec<Finding>,
    bytes: u64,
    danger: u8,
    /// What `a` would do here, as the title names it.
    mode: &'static str,
}

impl Shown {
    fn of(app: &App) -> Self {
        let plan = app.effective_findings();
        let permanent = |findings: &[Finding]| {
            findings
                .iter()
                .any(|finding| matches!(finding.action, Action::Shred | Action::Command { .. }))
        };
        let mode = if app.operation.is_some_and(Operation::read_only) {
            "READ-ONLY"
        } else if plan.iter().any(|finding| finding.action.is_actionable()) {
            if permanent(&plan) {
                "PERMANENT"
            } else {
                "TRASH-FIRST"
            }
        } else {
            // With nothing selected the view still names what it applies.
            let mut displayed = app.findings.clone();
            report::effective_actions(&mut displayed, app.shred);
            if permanent(&displayed) {
                "PERMANENT"
            } else {
                "TRASH-FIRST"
            }
        };
        Self {
            bytes: report::actionable_bytes(&plan),
            danger: safety::plan_danger(&plan),
            plan,
            mode,
        }
    }
}

/// The results list's title, within `width` columns. The mode is a safety
/// signal, so it leads: counts and sizes grow with the plan and would
/// otherwise push it past the minimum width. A title too wide for the
/// terminal drops its words before any number, so what failed and the danger
/// stay whole.
fn results_title(app: &App, shown: &Shown, width: usize) -> String {
    let size = report::gb(shown.bytes);
    let danger = shown.danger;
    let mode = shown.mode;
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

fn render_results(frame: &mut Frame, area: Rect, app: &App, shown: &Shown) {
    let rows = app.row_count();
    if rows == 0 {
        let title = results_title(app, shown, usize::from(area.width.saturating_sub(2)));
        frame.render_widget(
            Paragraph::new(Line::styled(
                "Nothing found. There is nothing to apply.",
                app.theme.style(Token::Success),
            ))
            .block(Block::bordered().title(title)),
            area,
        );
        return;
    }
    // The detail pane is the only place a long path appears whole, so it grows
    // past its usual share to hold the highlighted row, leaving the list three
    // rows (all of them when there are fewer), and says how many lines a
    // terminal too short for it still hides and that Enter shows them.
    let mut details = detail_lines(app, usize::from(area.width.saturating_sub(2)));
    let list_floor = u16::try_from(rows.min(3)).unwrap_or(3) + 2;
    let pane_floor = (area.height / 3).clamp(5, 10);
    let pane_ceiling = area.height.saturating_sub(list_floor).max(pane_floor);
    let pane_needed = u16::try_from(details.len()).map_or(u16::MAX, |rows| rows.saturating_add(2));
    let detail_height = pane_needed.clamp(pane_floor, pane_ceiling);
    let detail_rows = usize::from(detail_height.saturating_sub(2));
    if details.len() > detail_rows {
        let shown_rows = detail_rows.saturating_sub(1);
        let hidden = details.len() - shown_rows;
        details.truncate(shown_rows);
        details.push(Line::styled(
            format!("… {hidden} more line(s); Enter shows all"),
            app.theme.style(Token::Warning),
        ));
    }
    let [list_area, detail_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(detail_height)]).areas(area);
    let title = results_title(app, shown, usize::from(list_area.width.saturating_sub(2)));
    let visible = usize::from(list_area.height.saturating_sub(2)).max(1);
    let mut offset = app.list_offset.get();
    if app.cursor < offset {
        offset = app.cursor;
    } else if app.cursor >= offset.saturating_add(visible) {
        offset = app.cursor + 1 - visible;
    }
    offset = offset.min(rows.saturating_sub(visible));
    app.list_offset.set(offset);

    let width = usize::from(list_area.width.saturating_sub(2));
    let largest_bytes = max_finding_bytes(&app.findings);
    let lines: Vec<_> = (offset..rows.min(offset.saturating_add(visible)))
        .map(|row| result_row_with_max(app, row, width, largest_bytes))
        .collect();
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

/// Narrower views (including 64×18 and 84-column captures) omit the inline
/// share bar so label and path truncation stay unchanged.
const SHARE_BAR_MIN_WIDTH: usize = 96;
const SHARE_BAR_WIDTH: usize = 6;

fn max_finding_bytes(findings: &[Finding]) -> u64 {
    findings
        .iter()
        .map(|finding| finding.size_bytes)
        .max()
        .unwrap_or(0)
}

/// One results row: a finding with its selection mark, or a scan error or
/// warning. Rows are clipped to one line, and a row cut short ends in `…`;
/// the detail pane shows the highlighted row whole, or says how much of it a
/// short terminal hides. No row leaves a space in its last cell.
#[cfg(test)]
fn result_row(app: &App, row: usize, width: usize) -> Line<'static> {
    result_row_with_max(app, row, width, max_finding_bytes(&app.findings))
}

fn result_row_with_max(app: &App, row: usize, width: usize, largest_bytes: u64) -> Line<'static> {
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
            Span::styled(
                truncate_right(&report::terminal_safe(&text), width.saturating_sub(2)),
                app.theme.style(token),
            ),
        ]);
    };
    let check = match (app.is_selectable(row), app.excluded.contains(&row)) {
        (false, _) => "    ",
        (true, true) => "[ ] ",
        (true, false) => "[x] ",
    };
    // Wide enough for the last row's number, so every column stays aligned.
    let number = app.findings.len().to_string().len().max(3);
    let danger = format!("{:>number$}. danger-{} ", row + 1, finding.danger);
    let danger_width = danger.len();
    let size_column = format!("{:>8}", report::gb(finding.size_bytes));
    let size_width = size_column.len();
    let action_column = format!("  {:<8} ", action_label(&finding.action));
    let action_width = action_column.len();
    let label = report::terminal_safe(&finding.label);
    let label_width = Span::raw(label.as_str()).width();
    let path = report::terminal_safe(&short_place(app, &finding));
    let mut spans = vec![
        marker,
        Span::raw(check),
        // `style`, not `bold`: adding BOLD to every level collapses the monochrome
        // ladder, because moderate carries no modifier and high carries BOLD. The
        // theme's test pins the ladder `style()` draws; BOLD added here would
        // collapse it where that test cannot see.
        Span::styled(danger, app.theme.style(danger_token(finding.danger))),
    ];
    let show_share_bar = width >= SHARE_BAR_MIN_WIDTH && largest_bytes > 0;
    let fixed = if show_share_bar {
        let filled = crate::analyze::bar(finding.size_bytes, largest_bytes, SHARE_BAR_WIDTH);
        let rest = SHARE_BAR_WIDTH.saturating_sub(filled.chars().count());
        spans.push(Span::styled(
            format!("{size_column} "),
            app.theme.style(Token::AccentSecondary),
        ));
        spans.push(Span::styled(filled, app.theme.style(Token::Accent)));
        spans.push(Span::styled(
            "·".repeat(rest),
            app.theme.style(Token::Muted),
        ));
        spans.push(Span::styled(
            action_column,
            app.theme.style(Token::AccentSecondary),
        ));
        2 + check.len() + danger_width + size_width + 1 + SHARE_BAR_WIDTH + action_width
    } else {
        spans.push(Span::styled(
            format!("{size_column}{action_column}"),
            app.theme.style(Token::AccentSecondary),
        ));
        2 + check.len() + danger_width + size_width + action_width
    };
    let room = width.saturating_sub(fixed);
    if label_width > room {
        spans.push(Span::styled(
            truncate_right(&label, room),
            app.theme.style(Token::Strong),
        ));
        return Line::from(spans);
    }
    spans.push(Span::styled(label, app.theme.style(Token::Strong)));
    // Two columns of gap, then at least the ellipsis and one character.
    let path_room = room - label_width;
    if !path.is_empty() && path_room >= 4 {
        spans.push(Span::styled(
            format!("  {}", truncate_left(&path, path_room - 2)),
            app.theme.style(Token::Muted),
        ));
    }
    Line::from(spans)
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

/// The place a results row names, short enough to tell rows apart at the
/// minimum width: inside a project, the project folder's name and the path
/// below it; inside the home folder, `~` and the path below it. Prefixes are
/// matched by whole components. The detail pane keeps the complete path.
fn short_place(app: &App, finding: &Finding) -> String {
    let Some(full) = finding.path.as_deref() else {
        return finding_place(finding);
    };
    let path = std::path::Path::new(full);
    if let Some(project) = finding.project.as_deref().map(std::path::Path::new)
        && let Some(name) = project.file_name()
        && let Ok(below) = path.strip_prefix(project)
    {
        return std::path::Path::new(name).join(below).display().to_string();
    }
    if let Some(home) = &app.home
        && let Ok(below) = path.strip_prefix(home)
    {
        return if below.as_os_str().is_empty() {
            "~".into()
        } else {
            format!("~/{}", below.display())
        };
    }
    full.to_string()
}

/// A finding as the current Trash/permanent mode would apply it — its action
/// and its danger both — through the one policy the plan itself uses.
fn effective_finding(app: &App, finding: &Finding) -> Finding {
    let mut effective = [finding.clone()];
    report::effective_actions(&mut effective, app.shred);
    let [finding] = effective;
    finding
}

/// The scan error or warning shown on a row past the findings. A captured
/// diagnostic carries its level: `info:` is muted, anything else a warning.
fn diagnostic_at(app: &App, row: usize) -> Option<(String, Token)> {
    let index = row.checked_sub(app.findings.len())?;
    match app.errors.get(index) {
        Some(error) => Some((format!("error: {error}"), Token::Critical)),
        None => app.warnings.get(index - app.errors.len()).map(|warning| {
            let token = if warning.starts_with("info:") {
                Token::Muted
            } else {
                Token::Warning
            };
            (warning.clone(), token)
        }),
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
        if let Some(text) = finding.frees_at_least_text() {
            lines.push((text, Style::default()));
        }
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

/// Prose wrapped at spaces into rows of at most `max` columns, so its row
/// count is known before it is drawn; a word wider than a row is split by
/// columns. Runs of spaces collapse, which is right for prose and wrong for a
/// path: a path goes through `wrap_columns`.
pub(crate) fn wrap_words(text: &str, max: usize) -> Vec<String> {
    if max == 0 {
        return Vec::new();
    }
    let columns = |text: &str| Span::raw(text).width();
    let mut rows = Vec::new();
    let mut row = String::new();
    for word in text.split(' ').filter(|word| !word.is_empty()) {
        let needed = if row.is_empty() {
            columns(word)
        } else {
            columns(&row) + 1 + columns(word)
        };
        if needed <= max {
            if !row.is_empty() {
                row.push(' ');
            }
            row.push_str(word);
            continue;
        }
        if !row.is_empty() {
            rows.push(std::mem::take(&mut row));
        }
        if columns(word) <= max {
            row.push_str(word);
        } else {
            let mut pieces = wrap_columns(word, max);
            row = pieces.pop().unwrap_or_default();
            rows.extend(pieces);
        }
    }
    if !row.is_empty() || rows.is_empty() {
        rows.push(row);
    }
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

/// Keeps the start of `text` within `max` terminal columns, ending in `…`
/// when anything was cut, and never in a space, which would read as padding.
pub(crate) fn truncate_right(text: &str, max: usize) -> String {
    let span = Span::raw(text);
    if span.width() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut kept = 0;
    let mut end = 0;
    for grapheme in span.styled_graphemes(Style::default()) {
        let columns = Span::raw(grapheme.symbol).width();
        if kept + columns + 1 > max {
            break;
        }
        kept += columns;
        // Each grapheme borrows `text`, so its address is its byte offset.
        end = grapheme.symbol.as_ptr() as usize - text.as_ptr() as usize + grapheme.symbol.len();
    }
    format!("{}…", text[..end].trim_end())
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
        if let Some(line) = report::home_volume_line(summary) {
            lines.push((report::terminal_safe(&line), Style::default()));
        }
        if !app.excluded.is_empty() {
            lines.push((
                format!(
                    "{} left out of this plan and not touched.",
                    app.excluded.len()
                ),
                app.theme.style(Token::Muted),
            ));
        }
        if summary.bytes_trashed_estimate > 0 {
            lines.push((
                "Next: run `devtrim trash-empty --only-devtrim` to preview purging only what devtrim moved (b, then 0 previews the whole Trash)."
                    .to_string(),
                app.theme.style(Token::Accent),
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
    if !app.warnings.is_empty() {
        lines.push((String::new(), Style::default()));
        lines.push(("Warnings".into(), app.theme.style(Token::Warning)));
        for warning in &app.warnings {
            lines.push((
                format!("• {}", report::terminal_safe(warning)),
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

/// The keys the current screen takes, in priority order for the footer.
fn footer_keys(app: &App) -> Vec<(&'static str, &'static str)> {
    let mut keys = Vec::new();
    match app.screen {
        Screen::Menu => keys.extend([("↑↓", "move"), ("Enter", "open")]),
        Screen::Results => {
            if app.choice_count() > 0 {
                // `A` toggles, so its verb names what the next press does.
                let toggle = if app.excluded.is_empty() {
                    "none"
                } else {
                    "all"
                };
                keys.extend([("Space", "pick"), ("A", toggle), ("a", "apply")]);
            } else if app.row_count() > 1 {
                keys.push(("↑↓", "move"));
            }
            if app.scan_jump_operation().is_some() {
                keys.push(("o", "open category"));
            }
            if app.row_count() > 0 {
                keys.push(("Enter", "details"));
            }
            if app.can_toggle_shred() {
                keys.push(("s", if app.shred { "trash" } else { "permanent" }));
            }
            keys.extend([("r", "rescan"), ("b", "back")]);
        }
        Screen::Detail => keys.extend([("↑↓", "move"), ("Esc/b", "back")]),
        Screen::Outcome | Screen::Error => {
            keys.extend([("↑↓", "move"), ("r", "rescan"), ("b", "back")]);
        }
        // Their keys depend on the plan; `render_footer` writes them.
        Screen::Confirm | Screen::Loading => return keys,
    }
    keys.extend([("q", "quit"), ("?", "keys")]);
    keys
}

fn render_footer(frame: &mut Frame, area: Rect, app: &App) {
    // Progressive disclosure: the footer carries only the few keys that matter
    // on this screen, and `?` opens the complete reference. Listing everything
    // here would make the one key the operator needs harder to find.
    let typed;
    let keys: Vec<(&str, &str)> = match (app.screen, app.confirmation) {
        (Screen::Confirm, Some(ConfirmationKind::YesNo { .. })) => {
            vec![("y", "apply"), ("n/Esc", "cancel")]
        }
        (Screen::Confirm, Some(ConfirmationKind::Critical { expected_gb, .. })) => {
            typed = format!("Type {expected_gb}, then Enter");
            vec![(typed.as_str(), ""), ("Esc", "cancel")]
        }
        (Screen::Confirm, Some(ConfirmationKind::TrashPurge { expected_gb })) => {
            typed = format!("Type PURGE {expected_gb}, then Enter");
            vec![(typed.as_str(), ""), ("Esc", "cancel")]
        }
        (Screen::Loading, _) if applying(app) => app
            .stop_key_label()
            .map(|label| vec![("x", label)])
            .unwrap_or_default(),
        (Screen::Loading, _) => vec![("q", "quit")],
        _ => footer_keys(app),
    };
    // The results status names keys that act on the plan; none of them works
    // in the detail view or while working, so the footer says what does. The
    // confirmation reports inside itself, under the answer.
    let status = match app.screen {
        Screen::Detail => ("Reading only: nothing here changes the plan.", Token::Muted),
        Screen::Loading if applying(app) => (
            "Quitting is refused until this apply returns.",
            Token::Warning,
        ),
        Screen::Loading => (
            "q stops the scan and quits; other keys wait for the results.",
            Token::Muted,
        ),
        Screen::Confirm => ("", Token::Muted),
        _ => (app.status.as_str(), app.status_token),
    };
    render_footer_rows(frame, area, app.theme, &keys, status);
}

fn applying(app: &App) -> bool {
    matches!(app.working, Some((Work::Apply, _)))
}

/// What applying `plan` does, in one sentence: how many items each kind of
/// action takes and how much space they hold.
fn plan_effect(plan: &[Finding]) -> String {
    // Count and bytes for each kind of action, in the order the sentence
    // names them.
    let mut tally = [(0usize, 0u64); 3];
    for finding in plan {
        let slot = match finding.action {
            Action::Trash => 0,
            Action::Shred => 1,
            Action::Command { .. } => 2,
            Action::Info | Action::None => continue,
        };
        tally[slot].0 += 1;
        tally[slot].1 = tally[slot].1.saturating_add(finding.size_bytes);
    }
    let wording = [
        ("moves", "item", " to the Trash"),
        ("permanently deletes", "item", ""),
        ("runs", "command", ""),
    ];
    let mut parts = Vec::new();
    for ((count, bytes), (verb, noun, tail)) in tally.into_iter().zip(wording) {
        if count == 0 {
            continue;
        }
        let plural = if count == 1 { "" } else { "s" };
        parts.push(format!(
            "{verb} {count} {noun}{plural} ({}){tail}",
            report::gb(bytes)
        ));
    }
    let sentence = parts.join("; ");
    let mut characters = sentence.chars();
    characters.next().map_or_else(String::new, |first| {
        format!("{}{}.", first.to_uppercase(), characters.as_str())
    })
}

/// The confirmation, inside the body so the footer below it, which names the
/// keys it takes, always shows. It is as tall as what it says, and the rows it
/// covers are cleared edge to edge so nothing of the list shows beside it.
fn render_confirmation(frame: &mut Frame, area: Rect, app: &App, shown: &Shown) {
    let Some(confirmation) = app.confirmation else {
        return;
    };
    let width = area.width.saturating_sub(4).min(100);
    let inner = usize::from(width.saturating_sub(2));
    let mut lines = vec![Line::styled(
        "DATA-LOSS WARNING",
        app.theme.bold(Token::Critical),
    )];
    for row in wrap_words(&plan_effect(&shown.plan), inner) {
        lines.push(Line::styled(row, app.theme.style(Token::Strong)));
    }
    // Consent is given here, so this screen — not only the title behind it —
    // says how much of the preview the approval covers.
    let choices = app.choice_count();
    if choices > 0 {
        lines.push(Line::raw(format!(
            "This plan: {} of {choices} selected · {} left out",
            choices.saturating_sub(app.excluded.len()),
            app.excluded.len()
        )));
    }
    for row in wrap_words(safety::DATA_LOSS_NOTICE, inner) {
        lines.push(Line::raw(row));
    }
    // The answer and, under it, the feedback on it: a mismatch or a refused
    // key. The footer blanks its status on this screen, so the popup must say
    // this itself.
    // Every status set here is devtrim's own text.
    let answer = Text::from(vec![
        Line::from(vec![
            Span::raw("> "),
            Span::styled(app.input.clone(), app.theme.bold(Token::Accent)),
        ]),
        Line::styled(app.status.as_str(), app.theme.style(Token::Warning)),
    ]);
    match confirmation {
        ConfirmationKind::YesNo { .. } => lines.extend(
            answer
                .lines
                .into_iter()
                .skip(1)
                .filter(|_| !app.status.is_empty()),
        ),
        ConfirmationKind::Critical { .. } | ConfirmationKind::TrashPurge { .. } => {
            lines.push(Line::raw(""));
            // The feedback row is kept while empty so the popup does not jump.
            lines.extend(answer.lines);
        }
    }
    let danger = match confirmation {
        ConfirmationKind::YesNo { danger } | ConfirmationKind::Critical { danger, .. } => danger,
        ConfirmationKind::TrashPurge { .. } => shown.danger,
    };
    let popup = centered_rect(area, 100, lines.len());
    // Only the results title, which names the mode, stays behind the popup:
    // a half-hidden list or path beside it would read as part of the plan.
    frame.render_widget(
        Clear,
        Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        ),
    );
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::bordered().title(format!(" Confirm exact plan · danger-{danger} "))),
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

    /// A TUI apply starts as a CLI apply does, through the journal preflight
    /// that rotates an oversized journal; the CLI side is
    /// `eval_only_an_apply_rotates_the_journal`.
    #[test]
    fn an_apply_rotates_an_oversized_journal() {
        let fixture = ops::TestFixture::new("tui-apply-rotation");
        let ctx = Ctx::for_test(fixture.path().to_path_buf());
        std::fs::create_dir_all(ctx.journal_path.parent().unwrap()).unwrap();
        std::fs::write(&ctx.journal_path, "evidence\n").unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(&ctx.journal_path)
            .unwrap()
            .set_len(10 * 1024 * 1024 + 1)
            .unwrap();
        let plan = ApprovedPlan {
            operation: Operation::Clean(Target::Installers),
            findings: Vec::new(),
            approval: Approval::Yes,
        };

        run_apply(&ctx, plan, &Default::default()).unwrap();

        assert!(ctx.journal_path.with_extension("jsonl.1").exists());
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
            apply_results: Default::default(),
            apply_monitor: Default::default(),
        };

        app.finish_apply(run_apply(&ctx, plan, &Default::default()));

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

    fn critical_confirmation(app: &mut App) {
        app.finish_results(
            Operation::Clean(Target::Caches),
            vec![Finding::new(
                "cache",
                Some(std::path::PathBuf::from(
                    "/Users/example/Library/Caches/pip",
                )),
                3 * 1024 * 1024 * 1024,
                "test",
                9,
                Action::Shred,
            )],
            Vec::new(),
            Vec::new(),
        );
        app.begin_confirmation();
        assert!(matches!(
            app.confirmation,
            Some(ConfirmationKind::Critical { .. })
        ));
    }

    #[test]
    fn a_mismatch_is_reported_on_the_confirmation_at_the_minimum_size() {
        let mut app = App::default();
        critical_confirmation(&mut app);
        app.handle_key(key(KeyCode::Char('9')));
        app.handle_key(key(KeyCode::Enter));

        let output = rendered(&app, MIN_WIDTH, MIN_HEIGHT);

        // The footer blanks its status on the confirmation, so the feedback
        // lives inside the popup.
        assert!(
            output.contains(&app.status) && app.status.contains("Confirmation mismatch"),
            "PV tui/confirmation-feedback: {output}"
        );
        assert!(output.contains("understand the request."), "{output}");
        assert!(output.contains("> "));
    }

    #[test]
    fn a_typed_answer_stops_at_its_length_limit() {
        let mut app = App::default();
        critical_confirmation(&mut app);
        for _ in 0..1000 {
            app.handle_key(key(KeyCode::Char('7')));
        }

        assert_eq!(
            app.input.len(),
            MAX_CONFIRMATION_INPUT,
            "PV tui/input-limit"
        );
        assert!(app.status.contains("Longer than"), "{}", app.status);
        // Whole on one line in the popup, at the minimum size and on a larger
        // terminal; the footer blanks its status on the confirmation.
        assert!(rendered(&app, MIN_WIDTH, MIN_HEIGHT).contains(&app.status));
        assert!(rendered(&app, 100, 30).contains(&app.status));
        app.handle_key(key(KeyCode::Backspace));
        assert!(app.status.is_empty());
    }

    #[test]
    fn r_on_the_outcome_scans_again() {
        let operation = Operation::Clean(Target::Caches);
        let mut app = App {
            screen: Screen::Outcome,
            operation: Some(operation),
            ..App::default()
        };

        // The help lists r as rescan; after an apply that is what is left.
        assert_eq!(
            app.handle_key(key(KeyCode::Char('r'))),
            Intent::Load(operation),
            "PV tui/outcome-rescan"
        );
        assert!(rendered(&app, MIN_WIDTH, MIN_HEIGHT).contains("r rescan"));
    }

    /// The measured line wraps inside the outcome at the minimum size, and the
    /// footer keeps its keys.
    #[test]
    fn the_outcome_shows_measured_free_space_at_the_minimum_size() {
        let mut app = App {
            operation: Some(Operation::Clean(Target::Caches)),
            ..App::default()
        };
        let mut outcome = ApplyOutcome::new("caches");
        outcome.summary.items_touched = 1;
        outcome.summary.home_volume = Some(report::HomeVolume::Measured {
            free_before: 3 << 30,
            free_after: 5 << 30,
            gained_bytes: 2 << 30,
        });
        app.finish_apply(Ok(outcome));
        assert_eq!(app.screen, Screen::Outcome);

        let rows = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT);
        let text = rows.join("\n");
        assert!(
            rows.iter()
                .any(|row| row.contains("free space on the home volume: 3.0 GB")),
            "{text}"
        );
        assert!(text.contains("(+2.0 GB)"), "{text}");
        assert!(text.contains("other processes also write"), "{text}");
        assert!(!text.contains("freed"), "{text}");
        for key in ["q quit", "? keys", "b back"] {
            assert!(
                rows[16].contains(key),
                "{key:?} missing from {:?}",
                rows[16]
            );
        }
    }

    #[test]
    fn row_numbers_keep_every_column_aligned_past_999_findings() {
        let mut app = App::default();
        let findings = (0..1200)
            .map(|index| {
                Finding::new(
                    format!("item{index}"),
                    Some(std::path::PathBuf::from(format!("/Users/example/x{index}"))),
                    1,
                    "test",
                    3,
                    Action::Trash,
                )
            })
            .collect();
        app.finish_results(
            Operation::Clean(Target::Caches),
            findings,
            Vec::new(),
            Vec::new(),
        );
        let text = |row| {
            result_row(&app, row, 120)
                .spans
                .iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        };

        let (first, last) = (text(0), text(1199));
        let column = |line: &str| line.find("danger-").map(|end| line[..end].chars().count());
        assert_eq!(
            column(&first),
            column(&last),
            "PV tui/row-number-width: {first:?} vs {last:?}"
        );
        assert!(last.contains("1200. danger-3"), "{last:?}");
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
        // Esc means back everywhere else; here it must not end the session.
        assert_eq!(
            handle_visible_key(&mut app, key(KeyCode::Esc), too_small),
            Intent::None,
            "PV tui/small-esc"
        );
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

    /// A modified key is not the letter it carries: a terminal sends Alt+y as
    /// `ESC y`, and a stray Escape before a `y` must not approve a plan. Shift
    /// still counts, since `Y` and a typed `PURGE` arrive with it.
    #[test]
    fn modified_keys_never_approve_a_plan() {
        for modifiers in [
            KeyModifiers::ALT,
            KeyModifiers::CONTROL,
            KeyModifiers::SUPER,
            KeyModifiers::META,
        ] {
            let mut app = cleanup(vec![trash("first", 1, 2)]);
            app.begin_confirmation();
            assert_eq!(
                app.handle_key(KeyEvent::new(KeyCode::Char('y'), modifiers)),
                Intent::None,
                "PV tui/modified-key-approval: {modifiers:?}"
            );
            assert_eq!(app.screen, Screen::Confirm);
        }
        let mut app = cleanup(vec![trash("first", 1, 2)]);
        app.begin_confirmation();
        assert!(matches!(
            app.handle_key(KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::SHIFT)),
            Intent::Apply(_)
        ));
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

    const GIB: u64 = 1 << 30;

    fn sized(label: &str, bytes: u64, at_least: Option<u64>) -> Finding {
        let mut finding = Finding::new(
            label,
            Some(std::path::PathBuf::from(format!("/tmp/{label}"))),
            bytes,
            "stale",
            3,
            Action::Trash,
        );
        finding.frees_at_least_bytes = at_least;
        finding
    }

    /// The menu after a Scan everything like the owner's: node_modules held
    /// mostly in clones, caches without, docker failed, toolchains empty.
    fn overview_menu() -> App {
        let findings = vec![
            sized("caches", 12 * GIB, None),
            sized("nm-a", 20 * GIB, Some(GIB)),
            sized("nm-b", 1_820 * (1 << 20), Some(900 << 20)),
            sized("target", 5 * GIB, None),
        ];
        let sections = vec![
            section("caches", 0..1),
            section("node-modules", 1..3),
            section("artifacts", 3..4),
            section("simulators", 4..4),
            section("xcode", 4..4),
            section("toolchains", 4..4),
            section("installers", 4..4),
            section("agents", 4..4),
            section("leftovers", 4..4),
        ];
        let mut app = scan_everything(findings, sections);
        app.back_to_menu();
        app.disk = Some(Ok(FreeSpace {
            available: 123 * GIB,
            total: 994 * GIB,
        }));
        app.snapshots = Snapshots::Counted(2);
        app
    }

    fn disk_menu() -> App {
        App {
            disk: Some(Ok(FreeSpace {
                available: 123 * GIB,
                total: 994 * GIB,
            })),
            snapshots: Snapshots::Counted(2),
            ..App::default()
        }
    }

    fn menu_with(app: &App, selected: usize) -> App {
        App {
            selected,
            disk: app.disk.clone(),
            snapshots: app.snapshots.clone(),
            overview: app.overview.clone(),
            theme: app.theme,
            ..App::default()
        }
    }

    /// The Selected pane's rows, each trimmed, joined by spaces.
    fn selected_pane_text(rows: &[String]) -> String {
        rows.iter()
            .filter_map(|row| {
                let (_, pane) = row.split_once('│')?;
                Some(pane.trim_end_matches(['│', ' ']).trim().to_string())
            })
            .filter(|row| !row.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The menu row of `label`, trimmed, from a rendered screen.
    fn menu_row<'a>(rows: &'a [String], label: &str) -> &'a str {
        rows.iter()
            .map(|row| row.split(['│', '└', '┌']).next().unwrap_or("").trim_end())
            .find(|row| row.ends_with(label) || row.contains(&format!(" {label} ")))
            .unwrap_or("")
    }

    #[test]
    fn the_menu_opens_on_the_disk_and_keeps_every_entry_and_meaning_whole() {
        let app = disk_menu();
        for (selected, entry) in MENU.iter().enumerate() {
            let app = menu_with(&app, selected);
            let rows = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT);
            let pane = selected_pane_text(&rows);
            assert!(pane.contains("123.0 GB free · 87% used"), "{rows:#?}");
            let (_, _, meaning) = badge(entry.operation);
            // The pane rewraps words, so compare without the breaks.
            let squeeze = |text: &str| text.split_whitespace().collect::<String>();
            assert!(squeeze(&pane).contains(&squeeze(meaning)), "{rows:#?}");
            assert!(
                squeeze(&pane).contains(&squeeze(entry.description)),
                "{rows:#?}"
            );
            for item in MENU {
                assert_eq!(
                    menu_row(&rows, item.label).trim_start_matches('▶').trim(),
                    format!("{} {:<9} {}", item.key, badge(item.operation).0, item.label),
                    "before any scan a row is as it always was"
                );
            }
            assert!(
                rows.concat()
                    .contains("↑↓ move · Enter open · q quit · ? keys")
            );
        }
        let rows = screen_rows(&menu_with(&app, 0), 120, 40).concat();
        assert!(
            rows.contains("123.0 GB free of 994.0 GB · 87% used"),
            "{rows}"
        );
        assert!(rows.contains('█') && rows.contains('·'), "{rows}");
        assert!(
            rows.contains("2 local snapshots hold space no scan can see"),
            "{rows}"
        );
    }

    #[test]
    fn the_disk_block_says_what_it_could_not_read() {
        let mut app = App {
            disk: Some(Err("cannot read free space: /x".into())),
            snapshots: Snapshots::Checking,
            ..App::default()
        };
        let rows = screen_rows(&app, 120, 40).concat();
        assert!(
            rows.contains("disk: unavailable (cannot read free space: /x)"),
            "{rows}"
        );
        assert!(rows.contains("checking snapshots…"), "{rows}");
        assert!(!rows.contains('█'), "no bar without a measurement: {rows}");
        assert!(!app.failed, "an unreadable volume never fails the run");
        app.snapshots = Snapshots::Unavailable("not an APFS volume".into());
        let rows = screen_rows(&app, 120, 40).concat();
        assert!(
            rows.contains("snapshots: unavailable (not an APFS volume)"),
            "{rows}"
        );
        app.snapshots = Snapshots::Counted(0);
        assert!(
            screen_rows(&app, 120, 40)
                .concat()
                .contains("no local snapshots")
        );
        let rows = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT);
        assert!(
            selected_pane_text(&rows).contains("disk: unavailable"),
            "{rows:#?}"
        );
    }

    #[test]
    fn the_menu_reads_the_volume_again_when_it_is_shown() {
        let home = std::env::temp_dir();
        let mut app = App {
            home: Some(home),
            ..App::default()
        };
        assert_eq!(app.disk, None);
        app.back_to_menu();
        let Some(Ok(space)) = app.disk else {
            panic!("statvfs of the temporary folder: {:?}", app.disk);
        };
        assert!(space.total > 0 && space.available <= space.total);
    }

    #[test]
    fn after_scan_everything_each_row_carries_its_size_or_range() {
        let app = overview_menu();
        let narrow = screen_rows(&menu_with(&app, 0), MIN_WIDTH, MIN_HEIGHT);
        let wide = screen_rows(&menu_with(&app, 0), 120, 40);
        for rows in [&narrow, &wide] {
            assert!(
                menu_row(rows, "Caches").ends_with("12.0 GB · 1"),
                "{rows:#?}"
            );
            assert!(
                menu_row(rows, "node_modules").ends_with("1.9–21.8 GB · 2"),
                "{rows:#?}"
            );
            assert!(menu_row(rows, "Docker").ends_with("failed"), "{rows:#?}");
            assert!(menu_row(rows, "Xcode").ends_with("none"), "{rows:#?}");
            assert!(menu_row(rows, "Project purge").ends_with("Project purge"));
            assert!(menu_row(rows, "iCloud status").ends_with("iCloud status"));
            assert!(
                rows.concat()
                    .contains("↑↓ move · Enter open · q quit · ? keys")
            );
            for item in MENU {
                assert!(menu_row(rows, item.label).contains(badge(item.operation).0));
            }
        }
        // At 64 columns the total's range fits only without its count.
        assert!(
            menu_row(&narrow, "Scan everything").ends_with("Scan everything 18.9–38.8 GB"),
            "{narrow:#?}"
        );
        assert!(
            menu_row(&wide, "Scan everything").ends_with("18.9–38.8 GB · 4"),
            "{wide:#?}"
        );
        let selected = screen_rows(&menu_with(&app, 2), 120, 40);
        let pane = selected_pane_text(&selected);
        assert!(
            pane.contains("at least 1.9 GB comes back; APFS clones share the rest"),
            "{pane}"
        );
        assert!(
            !pane.contains("Last scan:"),
            "the row already shows it: {pane}"
        );
        let caches = selected_pane_text(&screen_rows(&menu_with(&app, 1), 120, 40));
        assert!(!caches.contains("at least"), "{caches}");
    }

    /// A row whose label leaves no room at 64 columns says in the pane what
    /// the row cannot; every other row shows its state at 64 columns, and
    /// every entry's badge meaning stays whole.
    #[test]
    fn at_the_minimum_size_every_row_states_its_scan_and_every_meaning_is_whole() {
        let findings = vec![
            sized("caches", 12 * GIB, None),
            sized("nm", 100 * GIB, Some(54 * GIB)),
            sized("target", 5 * GIB, Some(GIB)),
            sized("agents", 3 * GIB, None),
            sized("toolchain", 2 * GIB, None),
        ];
        let sections = vec![
            section("caches", 0..1),
            section("node-modules", 1..2),
            section("artifacts", 2..3),
            section("simulators", 3..3),
            section("xcode", 3..3),
            section("toolchains", 4..5),
            section("installers", 5..5),
            section("agents", 3..4),
            section("leftovers", 5..5),
        ];
        let mut app = scan_everything(findings, sections);
        app.back_to_menu();
        app.disk = disk_menu().disk;
        app.snapshots = Snapshots::Counted(2);
        let squeeze = |text: &str| text.split_whitespace().collect::<String>();
        let rows = screen_rows(&menu_with(&app, 0), MIN_WIDTH, MIN_HEIGHT);
        // 72.0–122.0 GB is too wide beside its label: its lower bound.
        assert!(
            menu_row(&rows, "Scan everything").ends_with("everything     ≥72.0 GB"),
            "{rows:#?}"
        );
        assert!(
            menu_row(&rows, "node_modules").ends_with("54.0–100.0 GB"),
            "{rows:#?}"
        );
        assert!(
            menu_row(&rows, "Build artifacts").ends_with("1.0–5.0 GB"),
            "{rows:#?}"
        );
        assert!(
            menu_row(&rows, "Swift toolchains").ends_with("2.0 GB · 1"),
            "{rows:#?}"
        );
        assert!(menu_row(&rows, "Docker").ends_with("failed"), "{rows:#?}");
        // The one label too long for any size at 64 columns.
        assert!(
            menu_row(&rows, "Agent caches & history").ends_with("history"),
            "{rows:#?}"
        );
        let agents = selected_pane_text(&screen_rows(&menu_with(&app, 10), MIN_WIDTH, MIN_HEIGHT));
        assert!(
            squeeze(&agents).contains(&squeeze("Last scan: 3.0 GB in 1 finding(s).")),
            "{agents}"
        );
        let overview = app.overview.as_mut().unwrap();
        overview.applied.insert("agents");
        overview.failed.push("caches");
        for (selected, entry) in MENU.iter().enumerate() {
            let rows = screen_rows(&menu_with(&app, selected), MIN_WIDTH, MIN_HEIGHT);
            let pane = selected_pane_text(&rows);
            let (_, _, meaning) = badge(entry.operation);
            assert!(squeeze(&pane).contains(&squeeze(meaning)), "{rows:#?}");
            assert!(pane.contains("GB free"), "the disk line stays: {rows:#?}");
            assert!(
                rows.concat()
                    .contains("↑↓ move · Enter open · q quit · ? keys")
            );
        }
        let rows = screen_rows(&menu_with(&app, 0), MIN_WIDTH, MIN_HEIGHT);
        assert!(menu_row(&rows, "Caches").ends_with("failed"), "{rows:#?}");
        let agents = selected_pane_text(&screen_rows(&menu_with(&app, 10), MIN_WIDTH, MIN_HEIGHT));
        assert!(
            squeeze(&agents).contains(&squeeze("Changed since the last scan")),
            "{agents}"
        );
    }

    #[test]
    fn an_applied_category_asks_for_a_rescan_instead_of_a_stale_size() {
        let mut app = overview_menu();
        app.operation = Some(Operation::Purge);
        app.begin_apply(std::sync::Arc::new(crate::journal::ApplyMonitor::new(0, 0)));
        app.back_to_menu();
        for (width, height) in [(MIN_WIDTH, MIN_HEIGHT), (120, 40)] {
            let rows = screen_rows(&menu_with(&app, 0), width, height);
            for label in ["node_modules", "Build artifacts", "Scan everything"] {
                assert!(menu_row(&rows, label).ends_with("rescan"), "{rows:#?}");
            }
            assert!(
                menu_row(&rows, "Caches").ends_with("12.0 GB · 1"),
                "{rows:#?}"
            );
        }
        let pane = selected_pane_text(&screen_rows(&menu_with(&app, 2), 120, 40));
        assert!(pane.contains("Changed since the last scan"), "{pane}");
        // A new Scan everything replaces the overview, and with it every rescan.
        app.finish_scan(
            ops::ScanResult {
                findings: vec![sized("nm", GIB, None)],
                sections: vec![section("node-modules", 0..1)],
                errors: Vec::new(),
            },
            Vec::new(),
        );
        app.back_to_menu();
        let rows = screen_rows(&menu_with(&app, 0), 120, 40);
        assert!(
            menu_row(&rows, "node_modules").ends_with("1.0 GB · 1"),
            "{rows:#?}"
        );
        assert!(menu_row(&rows, "Caches").ends_with("failed"), "{rows:#?}");
    }

    #[test]
    fn a_row_drops_its_count_before_its_size_and_never_cuts_a_range() {
        let row = Overview::offers(GIB, 20 * GIB, 7);
        assert_eq!(row.text(20).as_deref(), Some("1.0–20.0 GB · 7"));
        assert_eq!(row.text(12).as_deref(), Some("1.0–20.0 GB"));
        assert_eq!(
            row.text(10).as_deref(),
            Some("≥1.0 GB"),
            "the lower bound, never the upper"
        );
        assert_eq!(row.text(6), None);
        let single = Overview::offers(5 * GIB, 5 * GIB, 1);
        assert_eq!(single.text(20).as_deref(), Some("5.0 GB · 1"));
        assert_eq!(RowSize::Blank.text(40), None);
    }

    #[test]
    fn the_disk_block_and_the_overview_paint_no_color_under_no_color() {
        use crate::theme::ColorSupport;
        use ratatui::style::Color;

        for colors in [ColorSupport::None, ColorSupport::Named] {
            let mut app = overview_menu();
            app.theme = Theme::new(colors);
            let backend = TestBackend::new(120, 40);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| render(frame, &app)).unwrap();
            let buffer = terminal.backend().buffer();
            let painted = buffer
                .content()
                .iter()
                .filter(|cell| cell.symbol() == "█")
                .any(|cell| cell.fg != Color::Reset);
            let any = buffer
                .content()
                .iter()
                .any(|cell| cell.fg != Color::Reset || cell.bg != Color::Reset);
            if colors == ColorSupport::None {
                assert!(!any, "the menu painted a color under NO_COLOR");
            } else {
                assert!(painted, "control: the colored theme paints the bar");
            }
        }
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
            let (_, status) = outcome_status(failed, touched);
            assert!(status.chars().count() <= 62, "{status}");
        }
        for status in [NOTHING_CHANGED, APPLY_STOPPED] {
            assert!(status.chars().count() <= 62, "{status}");
        }
    }

    #[test]
    fn every_footer_keeps_how_to_leave_at_the_minimum_size() {
        let footer = |app: &App| screen_rows(app, MIN_WIDTH, MIN_HEIGHT)[16].clone();
        let with_choices = cleanup(vec![trash("first", 1, 2), trash("second", 1, 2)]);
        assert_eq!(
            footer(&with_choices).trim_end(),
            " Space pick · A none · a apply · b back · q quit · ? keys",
            "keys that drop at 64 columns go in their fixed order"
        );
        let mut detail = cleanup(vec![trash("first", 1, 2)]);
        detail.handle_key(key(KeyCode::Enter));
        assert_eq!(detail.screen, Screen::Detail);
        let mut outcome = cleanup(vec![trash("first", 1, 2)]);
        outcome.screen = Screen::Outcome;
        let mut error = App::default();
        error.fail(anyhow::anyhow!("synthetic"));
        let mut read_only = App::default();
        read_only.finish_results(
            Operation::ScanAll,
            vec![trash("first", 1, 2), trash("second", 1, 2)],
            Vec::new(),
            Vec::new(),
        );
        let mut permanent = cleanup(vec![trash("first", 1, 2), trash("second", 1, 2)]);
        permanent.handle_key(key(KeyCode::Char('s')));
        for (name, app) in [
            ("menu", App::default()),
            ("results", with_choices),
            ("permanent results", permanent),
            ("read-only results", read_only),
            ("empty results", cleanup(Vec::new())),
            ("detail", detail),
            ("outcome", outcome),
            ("error", error),
        ] {
            let row = footer(&app);
            for key in ["q quit", "? keys"] {
                assert!(row.contains(key), "{name}: {key:?} missing from {row:?}");
            }
            if matches!(
                app.screen,
                Screen::Results | Screen::Detail | Screen::Outcome | Screen::Error
            ) {
                assert!(
                    row.contains("b back"),
                    "{name}: b back missing from {row:?}"
                );
            }
        }
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

    /// A lower bound beside the size shows in the detail pane only: the row
    /// keeps the allocated size every total and confirmation uses, and stays
    /// one line at the minimum width.
    #[test]
    fn the_detail_pane_shows_what_a_cloned_target_frees_at_least() {
        let finding = Finding::sized(
            "node_modules",
            Some(std::path::PathBuf::from(
                "/Users/someone/dev/web/node_modules",
            )),
            crate::safety::Size {
                allocated: 1_470_000_000,
                frees_at_least: Some(78 << 20),
            },
            "repo last active 2020-01-01 UTC",
            5,
            Action::Trash,
        );
        let app = cleanup(vec![finding]);

        let rows = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT);
        let text = rows.join("\n");
        assert!(
            text.contains("frees at least 78 MB: APFS clones share the rest"),
            "{text}"
        );
        let row = rows
            .iter()
            .find(|row| row.contains("danger-5") && row.contains("node_modules"))
            .unwrap();
        assert!(row.contains("1.4 GB"), "{row:?}");
        assert!(!row.contains("frees at least"), "{row:?}");
    }

    /// At the minimum size a long path cannot fit, so the pane says how many
    /// lines it hides and which key shows them, instead of cutting them
    /// silently, and the selection state comes before the path so it is never
    /// among them.
    #[test]
    fn a_terminal_too_short_for_the_details_says_how_many_lines_it_hides() {
        // Label, action, selection state, fifteen path rows at 62 columns, and
        // the note: nineteen rows for a pane with room for ten, the last of
        // which says how many of the others it hides.
        let path = format!("/{}", "x".repeat(900));
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
            pane.contains("… 10 more line(s); Enter shows all"),
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
            .find(|row| {
                ["┌ TRASH-FIRST", "┌ PERMANENT", "┌ READ-ONLY"]
                    .iter()
                    .any(|mode| row.starts_with(mode))
            })
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

    /// The results title row, whatever its mode.
    fn title_row(app: &App, width: u16, height: u16) -> String {
        screen_rows(app, width, height)
            .into_iter()
            .find(|row| row.starts_with('┌') && row.contains("danger-"))
            .expect("the results list has a title")
    }

    fn shredded(label: &str, size: u64) -> Finding {
        let mut finding = trash(label, size, 9);
        finding.action = Action::Shred;
        finding
    }

    /// The title names what `a` would do here: Empty Trash deletes for good,
    /// a typed command cannot be undone, and Scan everything applies nothing.
    #[test]
    fn the_title_names_the_mode_the_screen_applies() {
        let mut empty_trash = App::default();
        empty_trash.finish_results(
            Operation::TrashEmpty,
            vec![shredded("Trash item", 1)],
            Vec::new(),
            Vec::new(),
        );
        let mut docker = App::default();
        docker.finish_results(
            Operation::Clean(Target::Docker),
            vec![Finding::new(
                "build cache",
                None,
                1,
                "test",
                6,
                Action::command("docker", &["builder", "prune"]),
            )],
            Vec::new(),
            Vec::new(),
        );
        let scan = scan_everything(vec![trash("x", 1, 2)], Vec::new());
        let caches = cleanup(vec![trash("cache", 1, 2)]);

        for (app, mode) in [
            (&empty_trash, "┌ PERMANENT"),
            (&docker, "┌ PERMANENT"),
            (&scan, "┌ READ-ONLY"),
            (&caches, "┌ TRASH-FIRST"),
        ] {
            let title = title_row(app, MIN_WIDTH, MIN_HEIGHT);
            assert!(title.starts_with(mode), "expected {mode}: {title}");
        }
        // Leaving everything out of Empty Trash changes nothing it would do.
        empty_trash.handle_key(key(KeyCode::Char('A')));
        assert!(title_row(&empty_trash, MIN_WIDTH, MIN_HEIGHT).starts_with("┌ PERMANENT"));
    }

    /// The popup's rows from its top border to its bottom border, and the
    /// columns it spans.
    fn popup_rows(rows: &[String]) -> (usize, usize, usize, usize) {
        let top = rows
            .iter()
            .position(|row| row.contains("┌ Confirm"))
            .expect("the confirmation is drawn");
        let left = rows[top].chars().position(|cell| cell == '┌').unwrap();
        let right = rows[top].chars().position(|cell| cell == '┐').unwrap();
        let bottom = top
            + rows[top..]
                .iter()
                .position(|row| row.chars().nth(left) == Some('└'))
                .expect("the confirmation is closed");
        (top, bottom, left, right)
    }

    /// Consent is given on this screen, so it says what will happen in words
    /// and numbers, computed from the plan the approval covers.
    #[test]
    fn the_confirmation_says_what_will_happen() {
        let mut trash_plan = cleanup(vec![
            trash("first", 1 << 30, 2),
            trash("second", 1 << 30, 2),
            trash("third", 1 << 30, 2),
            trash("left-out", 1 << 30, 2),
        ]);
        trash_plan.handle_key(key(KeyCode::End));
        trash_plan.handle_key(key(KeyCode::Char(' ')));
        trash_plan.begin_confirmation();
        let output = rendered(&trash_plan, MIN_WIDTH, MIN_HEIGHT);
        assert!(
            output.contains("Moves 3 items (3.0 GB) to the Trash."),
            "{output}"
        );

        let mut permanent = cleanup(vec![trash("only", 2 << 30, 2)]);
        permanent.handle_key(key(KeyCode::Char('s')));
        permanent.begin_confirmation();
        let output = rendered(&permanent, MIN_WIDTH, MIN_HEIGHT);
        assert!(
            output.contains("Permanently deletes 1 item (2.0 GB)."),
            "{output}"
        );
    }

    /// A y/N plan asks for one key: no input line, and the keys say so. A
    /// typed plan names exactly what to type.
    #[test]
    fn the_confirmation_keys_match_what_the_plan_asks_for() {
        let mut yes_no = cleanup(vec![trash("first", 1, 2)]);
        yes_no.begin_confirmation();
        let rows = screen_rows(&yes_no, MIN_WIDTH, MIN_HEIGHT);
        let output = rows.concat();
        assert!(output.contains("y apply · n/Esc cancel"), "{rows:#?}");
        assert!(
            !rows.iter().any(|row| row.contains("│> ")),
            "a y/N plan draws an input line: {rows:#?}"
        );

        let mut critical = App::default();
        critical_confirmation(&mut critical);
        let output = rendered(&critical, MIN_WIDTH, MIN_HEIGHT);
        assert!(
            output.contains("Type 3, then Enter · Esc cancel"),
            "{output}"
        );
        assert!(output.contains("> "), "{output}");

        let mut purge = App::default();
        purge.finish_results(
            Operation::TrashEmpty,
            vec![shredded("Trash item", 1)],
            Vec::new(),
            Vec::new(),
        );
        purge.begin_confirmation();
        let output = rendered(&purge, MIN_WIDTH, MIN_HEIGHT);
        assert!(
            output.contains("Type PURGE 0, then Enter · Esc cancel"),
            "{output}"
        );
    }

    /// The popup is as tall as what it says, and the list behind it does not
    /// show through on either side of the rows it covers.
    #[test]
    fn the_confirmation_is_sized_to_its_content_and_clears_its_rows() {
        let findings = (0..40)
            .map(|index| trash(&format!("cache-{index:02}-with-a-long-name"), 1, 2))
            .collect();
        let mut app = cleanup(findings);
        app.begin_confirmation();
        for (width, height) in [(MIN_WIDTH, MIN_HEIGHT), (120, 40)] {
            let rows = screen_rows(&app, width, height);
            let (top, bottom, left, right) = popup_rows(&rows);
            for row in &rows[top..=bottom] {
                let cells: Vec<char> = row.chars().collect();
                assert!(
                    cells[..left]
                        .iter()
                        .chain(&cells[right + 1..])
                        .all(|cell| *cell == ' '),
                    "the list shows beside the popup at {width}x{height}: {rows:#?}"
                );
            }
            let last = rows[bottom - 1].chars().collect::<Vec<_>>();
            assert!(
                cells_text(&last[left + 1..right]).trim() != "",
                "the popup ends in a blank row at {width}x{height}: {rows:#?}"
            );
        }
    }

    #[test]
    fn a_long_path_leaves_the_list_at_least_three_rows() {
        let long = |leaf: &str| {
            std::path::PathBuf::from(format!(
                "/tmp/{}/{leaf}",
                "a-folder-whose-name-wraps-".repeat(20)
            ))
        };
        let finding = |label: &str, leaf: &str| {
            Finding::new(label, Some(long(leaf)), 4096, "test", 5, Action::Trash)
        };
        let two = cleanup(vec![
            finding("stale target artifacts", "target"),
            finding("stale node_modules", "node_modules"),
        ]);
        let rows = screen_rows(&two, MIN_WIDTH, MIN_HEIGHT);
        let screen = rows.join("\n");
        for label in ["stale target artifacts", "stale node_modules"] {
            assert!(screen.contains(label), "{label} is not listed: {rows:#?}");
        }
        assert!(
            screen.contains("more line(s); Enter shows all"),
            "{rows:#?}"
        );

        let five = cleanup(
            (0..5)
                .map(|index| finding(&format!("finding-{index}"), "target"))
                .collect(),
        );
        let screen = screen_rows(&five, MIN_WIDTH, MIN_HEIGHT).join("\n");
        let listed = (0..5)
            .filter(|index| screen.contains(&format!("finding-{index} ")))
            .count();
        assert!(listed >= 3, "{listed} rows listed: {screen}");
    }

    #[test]
    fn nothing_of_the_details_shows_around_the_confirmation() {
        let long = format!("/tmp/{}/target", "a-folder-whose-name-wraps-".repeat(12));
        let finding = Finding::new(
            "stale target artifacts",
            Some(std::path::PathBuf::from(&long)),
            16 * 1024,
            "too much bandwidth or rebuild time",
            5,
            Action::Trash,
        );
        let mut app = cleanup(vec![finding, trash("second", 4 * 1024, 5)]);
        app.begin_confirmation();
        assert_eq!(app.screen, Screen::Confirm);
        for (width, height) in [(MIN_WIDTH, MIN_HEIGHT), (100, 30)] {
            let rows = screen_rows(&app, width, height);
            let (top, bottom, left, right) = popup_rows(&rows);
            // Between the results title (row 1) and the footer, only the
            // popup is drawn.
            for (index, row) in rows
                .iter()
                .enumerate()
                .take(usize::from(height) - 2)
                .skip(2)
            {
                let cells: Vec<char> = row.chars().collect();
                let outside = if (top..=bottom).contains(&index) {
                    cells[..left]
                        .iter()
                        .chain(&cells[right + 1..])
                        .collect::<String>()
                } else {
                    cells.iter().collect::<String>()
                };
                assert!(
                    outside.trim().is_empty(),
                    "row {index} shows text outside the popup at {width}x{height}: {rows:#?}"
                );
            }
        }
    }

    fn cells_text(cells: &[char]) -> String {
        cells.iter().collect()
    }

    /// At 64 columns every menu row shows its whole badge, every operation is
    /// on screen, and the badges use the site's words.
    #[test]
    fn every_menu_row_shows_its_whole_badge_at_the_minimum_size() {
        let app = App::default();
        let rows = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT);
        for item in MENU {
            let (badge, _, _) = badge(item.operation);
            let row = rows
                .iter()
                .find(|row| row.contains(item.label))
                .unwrap_or_else(|| panic!("{} is not on screen: {rows:#?}", item.label));
            assert!(row.contains(badge), "{} lost its badge: {row}", item.label);
        }
        let output = rows.concat();
        assert!(!output.contains("PREVIEW"), "{output}");
        assert!(output.contains("TRASH"), "{output}");
        assert!(
            !output.contains("Enter opens"),
            "the pane repeats the footer: {output}"
        );
    }

    /// One row of header and two of footer, without boxes or taglines, so
    /// the minimum size keeps its rows for content.
    #[test]
    fn the_header_is_one_row_and_the_footer_two() {
        let app = cleanup(vec![trash("cache", 1, 2)]);
        let rows = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT);
        let output = rows.concat();
        assert!(rows[0].contains("devtrim"), "{rows:#?}");
        assert!(
            rows[1].starts_with('┌'),
            "the body starts on row 2: {rows:#?}"
        );
        assert!(rows[MIN_HEIGHT as usize - 3].starts_with('└'), "{rows:#?}");
        assert!(!output.contains("measure · classify"), "{output}");
        assert!(!output.contains("Safe disk hygiene"), "{output}");
    }

    /// Every status says how much it matters: neutral information is muted, a
    /// refused key is a warning, and a failure is critical.
    #[test]
    fn the_status_carries_its_own_weight() {
        use crate::theme::ColorSupport;
        let theme = Theme::new(ColorSupport::Named);
        let status_style = |app: &App| {
            let backend = TestBackend::new(MIN_WIDTH, MIN_HEIGHT);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| render(frame, app)).unwrap();
            let buffer = terminal.backend().buffer();
            let row = MIN_HEIGHT - 1;
            let column = (0..MIN_WIDTH)
                .find(|column| buffer[(*column, row)].symbol() != " ")
                .expect("the status row is drawn");
            buffer[(column, row)].fg
        };
        let mut neutral = cleanup(vec![trash("first", 1, 2), trash("second", 1, 2)]);
        neutral.theme = theme;
        neutral.handle_key(key(KeyCode::Char(' ')));
        let mut refused = scan_everything(vec![trash("x", 1, 2)], Vec::new());
        refused.theme = theme;
        refused.handle_key(key(KeyCode::Char('a')));
        let mut failed = App {
            theme,
            ..App::default()
        };
        failed.fail(anyhow::anyhow!("synthetic"));

        assert_eq!(
            status_style(&neutral),
            theme.style(Token::Muted).fg.unwrap()
        );
        assert_eq!(
            status_style(&refused),
            theme.style(Token::Warning).fg.unwrap()
        );
        assert_eq!(
            status_style(&failed),
            theme.style(Token::Critical).fg.unwrap()
        );
    }

    /// A diagnostic cut at the edge says so, and its level sets its style.
    #[test]
    fn diagnostic_rows_say_where_they_are_cut_and_how_much_they_matter() {
        use crate::theme::ColorSupport;
        let theme = Theme::new(ColorSupport::Named);
        let mut app = cleanup(Vec::new());
        app.theme = theme;
        app.warnings = vec![
            format!("info: scanned roots {}", "r".repeat(100)),
            "warn: a short warning".into(),
        ];
        let backend = TestBackend::new(MIN_WIDTH, MIN_HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, &app)).unwrap();
        let buffer = terminal.backend().buffer();
        let row_of = |text: &str| {
            (0..MIN_HEIGHT)
                .find(|row| {
                    (0..MIN_WIDTH)
                        .map(|column| buffer[(column, *row)].symbol())
                        .collect::<String>()
                        .contains(text)
                })
                .unwrap_or_else(|| panic!("{text} is not drawn"))
        };
        let info = row_of("info: scanned");
        let line: String = (0..MIN_WIDTH)
            .map(|column| buffer[(column, info)].symbol())
            .collect();
        assert!(line.trim_end_matches(['│', ' ']).ends_with('…'), "{line}");
        let style_at = |row: u16, text: &str| {
            let line: String = (0..MIN_WIDTH)
                .map(|column| buffer[(column, row)].symbol())
                .collect();
            let start = line.find(text).unwrap();
            let column = u16::try_from(line[..start].chars().count()).unwrap();
            buffer[(column, row)].fg
        };
        assert_eq!(
            style_at(info, "info:"),
            theme.style(Token::Muted).fg.unwrap()
        );
        let warn = row_of("warn: a short");
        assert_eq!(
            style_at(warn, "warn:"),
            theme.style(Token::Warning).fg.unwrap()
        );
    }

    /// Rows stay distinguishable when narrow: a project's findings name the
    /// project folder, and the home folder reads as `~`.
    #[test]
    fn the_path_column_names_the_project_or_the_home_folder() {
        let project = std::path::Path::new("/Users/example/dev/payments-api");
        let in_project = Finding::new(
            "node_modules",
            Some(project.join("web/node_modules")),
            1,
            "stale",
            5,
            Action::Trash,
        )
        .with_project(project);
        let in_home = Finding::new(
            "uv cache",
            Some(std::path::PathBuf::from("/Users/example/.cache/uv")),
            1,
            "cache",
            2,
            Action::Trash,
        );
        let mut app = cleanup(vec![in_project, in_home]);
        app.home = Some(std::path::PathBuf::from("/Users/example"));
        let rows = screen_rows(&app, 120, 30);
        assert!(
            rows.iter()
                .any(|row| row.contains("node_modules  payments-api/web/node_modules")),
            "{rows:#?}"
        );
        assert!(
            rows.iter().any(|row| row.contains("uv cache  ~/.cache/uv")),
            "{rows:#?}"
        );
        // The detail pane keeps the complete path.
        assert!(
            detail_pane_text(&rows).contains("/Users/example/dev/payments-api/web/node_modules"),
            "{rows:#?}"
        );
    }

    /// The results footer lists Enter for the details where every key fits;
    /// at the minimum width it is the first key left to the `?` reference.
    #[test]
    fn the_results_footer_lists_enter_details() {
        let with_choices = cleanup(vec![trash("first", 1, 2)]);
        let read_only = scan_everything(vec![trash("x", 1, 2)], Vec::new());
        // At 64 columns "Enter details" is the first key to drop, so the
        // footer is read where every key fits.
        for app in [&with_choices, &read_only] {
            let output = rendered(app, 100, 30);
            assert!(output.contains("Enter details"), "{output}");
            assert!(output.contains("? keys"), "{output}");
        }
        assert!(rendered(&App::default(), MIN_WIDTH, MIN_HEIGHT).contains("Enter open"));
    }

    /// Warnings queued before the first scan, such as a skipped root from
    /// `Ctx::from_cli`, reach its result.
    #[test]
    fn warnings_queued_before_a_scan_reach_its_result() {
        let fixture = ops::TestFixture::new("tui-early-warnings");
        let home = fixture.path().to_path_buf();
        std::fs::create_dir_all(home.join("Downloads")).unwrap();
        let ctx = Ctx {
            yes: false,
            yolo: false,
            json: false,
            roots: Vec::new(),
            roots_origin: crate::safety::RootsOrigin::Default,
            active_days: 30,
            retain_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home,
            interactive: true,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
            apply_results: Default::default(),
            apply_monitor: Default::default(),
        };
        ctx.diagnostic("warn", "skipping a scan root: synthetic");

        let operation = Operation::Clean(Target::Installers);
        let mut app = App::default();
        app.finish_load(operation, run_scan(&ctx, operation, None));

        assert!(
            app.warnings
                .iter()
                .any(|warning| warning == "warn: skipping a scan root: synthetic"),
            "{:?}",
            app.warnings
        );
    }

    #[test]
    fn the_working_screen_names_what_it_scans_in_words() {
        let mut app = App::default();
        app.begin_load(Operation::ScanAll);
        assert_eq!(app.status, "Scanning everything…");
        app.begin_load(Operation::Clean(Target::NodeModules));
        assert_eq!(app.status, "Scanning node_modules…");
    }

    /// A result with nothing in it says so once.
    #[test]
    fn an_empty_result_says_nothing_was_found_once() {
        let app = cleanup(Vec::new());
        let output = rendered(&app, MIN_WIDTH, MIN_HEIGHT);
        assert_eq!(output.matches("Nothing found").count(), 1, "{output}");
        assert!(!output.contains("No findings"), "{output}");
        assert!(
            !output.contains("move"),
            "the footer offers no movement over nothing: {output}"
        );
    }

    #[test]
    fn a_failure_is_titled_plainly() {
        let mut app = App::default();
        app.fail(anyhow::anyhow!("synthetic"));
        let output = rendered(&app, MIN_WIDTH, MIN_HEIGHT);
        assert!(output.contains("┌ Error"), "{output}");
        assert!(!output.contains("Failed closed"), "{output}");
        assert!(output.contains("Nothing was changed."), "{output}");
    }

    fn applying_app(command: bool) -> (App, std::sync::Arc<crate::journal::ApplyMonitor>) {
        let monitor = std::sync::Arc::new(crate::journal::ApplyMonitor::new(3, 3 << 30));
        monitor.set_current_for_test(crate::journal::CurrentItem {
            op: "node_modules".into(),
            subject: if command {
                "docker --host unix:///var/run/docker.sock image prune -a -f".into()
            } else {
                "/Users/example/dev/a-very-long-project-name/packages/web/node_modules".into()
            },
            command,
        });
        let mut app = App {
            home: Some(std::path::PathBuf::from("/Users/example")),
            ..App::default()
        };
        app.begin_apply(monitor.clone());
        (app, monitor)
    }

    /// The working screen of an apply says how far it has gone and what it is
    /// doing, whole at the minimum size, and offers the stop key; asking it
    /// changes the screen at once and the key is no longer offered.
    #[test]
    fn the_apply_screen_shows_progress_and_offers_a_stop_at_the_minimum_size() {
        let (mut app, monitor) = applying_app(false);
        let rows = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT);
        let frame = rows.concat();
        assert!(
            frame.contains("Applying only the exact previewed findings"),
            "{rows:#?}"
        );
        assert!(
            frame.contains("0 of 3 · 0 B of 3.0 GB · node_modules …"),
            "{rows:#?}"
        );
        assert!(
            frame.contains("/web/node_modules"),
            "the item keeps its end: {rows:#?}"
        );
        assert!(frame.contains("x stop after this item"), "{rows:#?}");
        assert!(frame.contains("Quitting is refused until this apply returns."));

        app.request_stop();
        assert!(monitor.stop_requested());
        let frame = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT).concat();
        assert!(
            frame.contains("Stopping after the current item…"),
            "{frame}"
        );
        assert!(!frame.contains("x stop"), "{frame}");
        assert!(frame.contains("Quitting is refused until this apply returns."));
    }

    /// A typed command can run for minutes, so the key says it waits for the
    /// command, and so does the screen once it is pressed.
    #[test]
    fn a_stop_during_a_typed_command_says_it_waits_for_the_command() {
        let (mut app, _monitor) = applying_app(true);
        let frame = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT).concat();
        assert!(frame.contains("x stops after this command"), "{frame}");
        app.request_stop();
        let frame = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT).concat();
        assert!(
            frame.contains("Stopping after the current command…"),
            "{frame}"
        );
    }

    /// Only a bare `x` asks for a stop, and never outside an apply.
    #[test]
    fn only_a_bare_x_during_an_apply_stops_it() {
        assert!(is_stop_key(key(KeyCode::Char('x'))));
        let mut alt = key(KeyCode::Char('x'));
        alt.modifiers = event::KeyModifiers::ALT;
        assert!(!is_stop_key(alt));
        assert!(!is_stop_key(key(KeyCode::Char('q'))));
        let (mut app, monitor) = applying_app(false);
        app.working = Some((Work::Scan, Instant::now()));
        app.request_stop();
        assert!(
            !monitor.stop_requested(),
            "a scan has nothing to stop between"
        );
    }

    /// The outcome of a stopped apply says what finished and what was never
    /// attempted, and the run still counts as failed.
    #[test]
    fn a_stopped_apply_reports_what_was_not_attempted() {
        let mut app = App::default();
        let mut outcome = ApplyOutcome::new("caches");
        outcome.summary.items_touched = 1;
        outcome.stopped = 2;
        outcome
            .errors
            .push("stopped by you: 2 not attempted".into());
        app.finish_apply(Ok(outcome));
        assert_eq!(app.screen, Screen::Outcome);
        assert!(app.failed);
        assert_eq!(app.status, "Stopped by you: 1 done, 2 not attempted.");
        let frame = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT).concat();
        assert!(frame.contains("stopped by you: 2 not attempted"), "{frame}");
    }

    /// The working screens' new lines paint nothing under `NO_COLOR`, and they
    /// say in words what color would add.
    #[test]
    fn progress_screens_paint_no_color_under_no_color() {
        use crate::theme::ColorSupport;
        use ratatui::style::Color;
        let (mut stopping, _monitor) = applying_app(false);
        stopping.request_stop();
        let screens = vec![applying_app(false).0, applying_app(true).0, stopping]
            .into_iter()
            .chain(progress_apps());
        for mut app in screens {
            app.theme = Theme::new(ColorSupport::None);
            let backend = TestBackend::new(MIN_WIDTH, MIN_HEIGHT);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal.draw(|frame| render(frame, &app)).unwrap();
            for cell in terminal.backend().buffer().content() {
                assert_eq!(cell.fg, Color::Reset, "a working screen painted a color");
                assert_eq!(
                    cell.bg,
                    Color::Reset,
                    "a working screen painted a background"
                );
            }
        }
    }

    /// Scan screens for the test above.
    fn progress_apps() -> Vec<App> {
        vec![scanning_app()]
    }

    /// A Scan everything in progress: one category done, one failed with a
    /// long reason, one done with an unjudged finding, the rest waiting.
    fn scanning_app() -> App {
        let mut app = App::default();
        app.begin_load(Operation::ScanAll);
        let board = app.scan_board.clone().unwrap();
        board.record(ops::CategoryDone {
            category: "caches",
            tally: Ok(ops::CategoryTally {
                actionable_bytes: 3 << 30,
                findings: 12,
                unjudged: 0,
            }),
        });
        board.record(ops::CategoryDone {
            category: "docker",
            tally: Err(format!("cannot reach the daemon {}", "x".repeat(200))),
        });
        board.record(ops::CategoryDone {
            category: "artifacts",
            tally: Ok(ops::CategoryTally {
                actionable_bytes: 4096,
                findings: 1,
                unjudged: 1,
            }),
        });
        app
    }

    /// A Scan everything lists every category with its state, whole at the
    /// minimum size, below the status and the clock; a single-category scan
    /// keeps the screen it had.
    #[test]
    fn the_scan_screen_lists_every_category_at_the_minimum_size() {
        let app = scanning_app();
        let rows = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT);
        let frame = rows.concat();
        assert!(frame.contains("Scanning everything"), "{rows:#?}");
        assert!(
            frame.contains("0 s elapsed · 3 of 10 categories done"),
            "{rows:#?}"
        );
        for operation in ops::all() {
            assert!(
                frame.contains(operation.name()),
                "{} missing: {rows:#?}",
                operation.name()
            );
        }
        assert!(
            frame.contains("caches       3.0 GB · 12 findings"),
            "{rows:#?}"
        );
        assert!(
            frame.contains("artifacts    4 KB · 1 finding · 1 not judged"),
            "{rows:#?}"
        );
        assert!(
            frame.contains("docker       failed: cannot reach the daemon"),
            "{rows:#?}"
        );
        assert!(frame.contains("toolchains   waiting"), "{rows:#?}");
        assert!(frame.contains("q quit"), "the footer survives: {rows:#?}");

        let mut single = App::default();
        single.begin_load(Operation::Clean(Target::Caches));
        assert!(single.scan_board.is_none());
        let frame = screen_rows(&single, MIN_WIDTH, MIN_HEIGHT).concat();
        assert!(frame.contains("0 s elapsed"));
        assert!(!frame.contains("waiting"));
    }

    #[test]
    fn scan_all_o_jumps_to_actionable_category_and_refuses_read_only_or_diagnostics() {
        let findings = vec![
            trash("uv cache", 2 * GIB, 2),
            Finding::new(
                "unreadable cache",
                None,
                0,
                "permission denied",
                0,
                Action::Info,
            ),
            trash("stale node_modules", 4 * GIB, 5),
            Finding::new("agent leftover", None, 1024, "hint", 0, Action::Info),
        ];
        let sections = vec![
            section("caches", 0..2),
            section("node-modules", 2..3),
            section("leftovers", 3..4),
        ];
        let mut app = scan_everything(findings, sections);

        // Footer shows `o open category` on an actionable ScanAll row while
        // keeping escape keys intact at 64×18.
        let wide_footer = screen_rows(&app, 100, 30)[28].clone();
        assert!(wide_footer.contains("o open category"), "{wide_footer:?}");
        let narrow_footer = screen_rows(&app, MIN_WIDTH, MIN_HEIGHT)[16].clone();
        for required in ["b back", "q quit", "? keys"] {
            assert!(
                narrow_footer.contains(required),
                "{required} missing from {narrow_footer:?}"
            );
        }

        // `o` jumps straight to Caches from row 0.
        assert_eq!(
            app.handle_key(key(KeyCode::Char('o'))),
            Intent::Load(Operation::Clean(Target::Caches))
        );
        // Row 1 is an informational diagnostic inside the actionable Caches
        // section: `o` must refuse and the footer must omit `o open category`.
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.handle_key(key(KeyCode::Char('o'))), Intent::None);
        assert_eq!(app.status_token, Token::Muted);
        assert!(app.status.contains("no actionable category view"));
        assert!(!screen_rows(&app, 100, 30)[28].contains("o open category"));

        // Row 2 is actionable NodeModules.
        app.handle_key(key(KeyCode::Down));
        assert_eq!(
            app.handle_key(key(KeyCode::Char('o'))),
            Intent::Load(Operation::Clean(Target::NodeModules))
        );
        // `a` still refuses in ScanAll with its existing guidance.
        assert_eq!(app.handle_key(key(KeyCode::Char('a'))), Intent::None);
        assert_eq!(app.status_token, Token::Warning);
        assert!(
            app.status
                .contains("Read-only scan. To act on it: b, then 3 (node-modules).")
        );

        // `leftovers` is read-only: `o` explains it has no actionable view.
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.handle_key(key(KeyCode::Char('o'))), Intent::None);
        assert_eq!(app.status_token, Token::Muted);
        assert!(app.status.contains("no actionable category view"));
        assert!(!screen_rows(&app, 100, 30)[28].contains("o open category"));

        // Diagnostic error row: `o` refuses with a muted status.
        app.handle_key(key(KeyCode::Down));
        assert_eq!(app.handle_key(key(KeyCode::Char('o'))), Intent::None);
        assert_eq!(app.status_token, Token::Muted);
        assert!(app.status.contains("no actionable category view"));
    }

    #[test]
    fn menu_overview_summary_shows_when_tall_enough_and_yields_at_minimum_size() {
        let mut app = overview_menu();
        let wide_pane = selected_pane_text(&screen_rows(&menu_with(&app, 0), 120, 40));
        assert!(
            wide_pane.contains("Reclaimable: 18.9–38.8 GB across 3 categories"),
            "{wide_pane}"
        );
        assert!(
            wide_pane.contains("Press p for Project purge, or pick a category"),
            "{wide_pane}"
        );

        // Single non-cloned category uses singular wording without a range.
        let mut single = scan_everything(
            vec![sized("caches", 2 * GIB, None)],
            vec![section("caches", 0..1)],
        );
        single.back_to_menu();
        let single_pane = selected_pane_text(&screen_rows(&single, 120, 40));
        assert!(
            single_pane.contains("Reclaimable: 2.0 GB across 1 category"),
            "{single_pane}"
        );

        // After an apply marks a category stale, the stale total disappears.
        app.operation = Some(Operation::Clean(Target::Caches));
        app.begin_apply(std::sync::Arc::new(crate::journal::ApplyMonitor::new(0, 0)));
        app.back_to_menu();
        let stale_pane = selected_pane_text(&screen_rows(&app, 120, 40));
        assert!(!stale_pane.contains("Reclaimable:"), "{stale_pane}");
    }

    #[test]
    fn loading_progress_bars_fit_minimum_size_and_outcome_hints_trash_empty() {
        use crate::theme::ColorSupport;

        // ScanAll progress bar + 10 categories fit cleanly at 64×18.
        let scan = scanning_app();
        let backend = TestBackend::new(MIN_WIDTH, MIN_HEIGHT - 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_loading(frame, frame.area(), &scan))
            .unwrap();
        let scan_frame: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(scan_frame.contains("30%"), "{scan_frame}");
        assert!(
            scan_frame.contains('█') && scan_frame.contains('·'),
            "{scan_frame}"
        );
        assert!(
            scan_frame.contains("0 s elapsed · 3 of 10 categories done"),
            "{scan_frame}"
        );
        for operation in ops::all() {
            assert!(scan_frame.contains(operation.name()), "{scan_frame}");
        }

        // Apply progress bar sits above apply_progress_line at 64×18.
        let (apply, _monitor) = applying_app(false);
        let backend = TestBackend::new(MIN_WIDTH, MIN_HEIGHT - 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_loading(frame, frame.area(), &apply))
            .unwrap();
        let apply_frame: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(apply_frame.contains("0%"), "{apply_frame}");
        assert!(apply_frame.contains("0 of 3"), "{apply_frame}");

        // Outcome next-step hint appears when bytes were moved to Trash, and
        // stays uncolored under NO_COLOR; omitted when nothing was trashed.
        let mut trashed = App {
            operation: Some(Operation::Clean(Target::Caches)),
            theme: Theme::new(ColorSupport::None),
            ..App::default()
        };
        let mut outcome = ApplyOutcome::new("caches");
        outcome.summary.items_touched = 2;
        outcome.summary.bytes_freed_estimate = 2 * GIB;
        outcome.summary.bytes_trashed_estimate = 2 * GIB;
        trashed.finish_apply(Ok(outcome));
        let entries = outcome_entries(&trashed);
        let hint = entries
            .iter()
            .find(|(text, _)| text.contains("devtrim trash-empty --only-devtrim"))
            .expect("missing trash-empty --only-devtrim hint");
        assert!(hint.0.contains("whole Trash"), "{}", hint.0);
        assert!(
            hint.1.fg.is_none() && hint.1.bg.is_none(),
            "outcome hint painted a color under NO_COLOR"
        );

        let mut permanent = App {
            operation: Some(Operation::TrashEmpty),
            ..App::default()
        };
        let mut purged = ApplyOutcome::new("trash-empty");
        purged.summary.items_touched = 1;
        purged.summary.bytes_freed_estimate = GIB;
        permanent.finish_apply(Ok(purged));
        assert!(
            outcome_entries(&permanent)
                .iter()
                .all(|(text, _)| !text.contains("trash-empty --only-devtrim")),
            "permanent purge must not hint trash-empty"
        );
    }
}
