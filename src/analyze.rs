//! Read-only interactive disk explorer.
//!
//! Navigation only. This module never creates deletion authority, and that is a
//! deliberate boundary rather than an unfinished feature: every cleanup surface
//! in devtrim binds deletion to a closed, corroborated category — a `target`
//! beside `Cargo.toml`, a `.venv` containing `pyvenv.cfg`. An explorer that
//! deleted whatever the cursor happened to be on would replace that structural
//! evidence with the operator's aim, which is a different capability wearing the
//! same interface.
//!
//! Measurement runs on a worker thread and streams results back, because sizing
//! a real home directory takes minutes and a frozen interface is not an
//! acceptable way to spend them. Leaving a directory cancels its in-flight walk.

use std::io::{self, IsTerminal};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};

use crate::report::{self, Action, Finding};
use crate::safety::Ctx;
use crate::theme::{Theme, Token};

/// How many progress messages one frame may absorb. A directory with tens of
/// thousands of children would otherwise let the producer starve key handling.
const DRAIN_BUDGET: usize = 512;

/// Idle redraw cadence. Roughly 12 frames per second is enough for a progress
/// readout and leaves the CPU alone; a keystroke redraws immediately anyway.
const POLL_INTERVAL: Duration = Duration::from_millis(80);

/// Sub-cell bar resolution. A terminal cell is the smallest unit devtrim can
/// paint, so eighths are the only way to show a proportion honestly at this size.
const BLOCKS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) path: PathBuf,
    pub(crate) size: u64,
    pub(crate) is_dir: bool,
    /// True when traversal could not read everything below this entry, so the
    /// size is a lower bound rather than a measurement.
    pub(crate) partial: bool,
}

impl Entry {
    fn name(&self) -> String {
        self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    }
}

#[derive(Debug)]
enum Progress {
    Measured(Entry),
    Done,
    Failed(String),
}

/// Recursively sums regular-file sizes below `path`.
///
/// Symbolic links are never followed and a different device is never entered:
/// an explorer that wandered onto a network mount would appear to hang, and one
/// that followed a link would attribute another tree's bytes to this one.
/// Returns the total and whether anything was unreadable.
fn measure_tree(path: &Path, device: u64, cancel: &AtomicBool) -> (u64, bool) {
    let mut total = 0u64;
    let mut partial = false;
    // `same_file_system` stops the walk at the mount point instead of filtering
    // afterwards. Filtering after the fact still descends: a stalled network
    // mount blocks inside `WalkDir::next()`, where the cancellation flag is
    // never read, and the thread cannot be released by quitting.
    for result in walkdir::WalkDir::new(path)
        .follow_links(false)
        .follow_root_links(false)
        .same_file_system(true)
    {
        if cancel.load(Ordering::Relaxed) {
            return (total, true);
        }
        let Ok(entry) = result else {
            partial = true;
            continue;
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            partial = true;
            continue;
        };
        if metadata.dev() != device {
            // Not reachable while `same_file_system` holds, but a subtree left
            // unmeasured is a lower bound and must say so rather than vanish
            // into a confident total.
            partial = true;
            continue;
        }
        // Measured bytes never saturate silently: an overflowed total that
        // still presents itself as a measurement is the shape S8 forbids. It
        // cannot happen with real file sizes, so it becomes a lower bound
        // rather than an error the explorer has no way to show.
        match total.checked_add(metadata.len()) {
            Some(sum) => total = sum,
            None => {
                partial = true;
                break;
            }
        }
    }
    (total, partial)
}

/// Immediate children of `directory`, without following symlinks.
fn child_paths(directory: &Path) -> Result<Vec<PathBuf>> {
    let entries = std::fs::read_dir(directory)
        .with_context(|| format!("cannot read {}", directory.display()))?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot enumerate {}", directory.display()))?;
        paths.push(entry.path());
    }
    Ok(paths)
}

/// Measures every immediate child of `directory`, reporting each as it finishes.
pub(crate) fn measure_children(
    directory: &Path,
    cancel: &AtomicBool,
    mut report_entry: impl FnMut(Entry),
) -> Result<()> {
    let device = std::fs::symlink_metadata(directory)
        .with_context(|| format!("cannot inspect {}", directory.display()))?
        .dev();
    for path in child_paths(directory)? {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            // Only a genuine disappearance between listing and stat is a race
            // worth ignoring. Any other error would otherwise drop a child from
            // the view with no `(partial)` marker and no error, which
            // contradicts this command's lower-bound disclosure.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("cannot inspect {}", path.display()));
            }
        };
        // A symlink is listed at its own size, never resolved: its target may
        // live outside this tree entirely.
        if metadata.file_type().is_dir() {
            // A foreign mount must be refused BEFORE the walk starts.
            // `measure_tree` roots a new `WalkDir` at this child, and
            // `same_file_system` takes its boundary from the walk's own root —
            // so entering a mount would authorize that mount's device and
            // traverse all of it, which is exactly the network-mount hang the
            // boundary exists to prevent.
            //
            // What this does NOT catch, and cannot: the macOS system/data
            // firmlink. `/`, `/System/Volumes/Data` and `/Users` all report the
            // same `st_dev` even though `df` shows separate APFS volumes, so
            // only `statfs` distinguishes them. That is correct to allow —
            // walking `/` into the user's own data is what analyzing `/` means
            // — but it is why measuring `/` is slow, not a broken boundary.
            if metadata.dev() != device {
                report_entry(Entry {
                    path,
                    size: 0,
                    is_dir: true,
                    partial: true,
                });
                continue;
            }
            let (size, partial) = measure_tree(&path, device, cancel);
            report_entry(Entry {
                path,
                size,
                is_dir: true,
                partial,
            });
        } else {
            report_entry(Entry {
                path,
                size: metadata.len(),
                is_dir: false,
                partial: false,
            });
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Intent {
    None,
    Quit,
}

pub(crate) struct Explorer {
    /// Drill-down stack; the last element is the directory on screen.
    stack: Vec<PathBuf>,
    pub(crate) entries: Vec<Entry>,
    selected: usize,
    scanning: bool,
    errors: Vec<String>,
    help: bool,
    theme: Theme,
    cancel: Arc<AtomicBool>,
    progress: Option<Receiver<Progress>>,
}

impl Explorer {
    pub(crate) fn new(root: PathBuf, theme: Theme) -> Self {
        Self {
            stack: vec![root],
            entries: Vec::new(),
            selected: 0,
            scanning: false,
            errors: Vec::new(),
            help: false,
            theme,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: None,
        }
    }

    fn current(&self) -> &Path {
        self.stack.last().map_or(Path::new("/"), PathBuf::as_path)
    }

    fn total(&self) -> u64 {
        self.entries
            .iter()
            .fold(0u64, |sum, entry| sum.saturating_add(entry.size))
    }

    /// Abandons any in-flight walk and starts measuring the current directory.
    fn begin_scan(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.entries.clear();
        self.errors.clear();
        self.selected = 0;
        self.scanning = true;

        let (sender, receiver) = channel();
        self.progress = Some(receiver);
        let directory = self.current().to_path_buf();
        let cancel = Arc::clone(&self.cancel);
        // A detached worker is correct here: the only thing it can do after
        // cancellation is observe the flag and exit, and its channel is dropped.
        std::thread::spawn(move || {
            let outcome = measure_children(&directory, &cancel, |entry| {
                let _ = sender.send(Progress::Measured(entry));
            });
            let _ = match outcome {
                Ok(()) => sender.send(Progress::Done),
                Err(error) => sender.send(Progress::Failed(format!("{error:#}"))),
            };
        });
    }

    /// Absorbs a bounded batch of worker messages without blocking.
    fn drain_progress(&mut self) {
        let mut changed = false;
        for _ in 0..DRAIN_BUDGET {
            let Some(receiver) = self.progress.as_ref() else {
                break;
            };
            match receiver.try_recv() {
                Ok(Progress::Measured(entry)) => {
                    self.entries.push(entry);
                    changed = true;
                }
                Ok(Progress::Done) => {
                    self.scanning = false;
                    self.progress = None;
                    changed = true;
                }
                Ok(Progress::Failed(error)) => {
                    self.errors.push(error);
                    self.scanning = false;
                    self.progress = None;
                    changed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.scanning = false;
                    self.progress = None;
                    break;
                }
            }
        }
        if changed {
            self.sort_entries();
        }
    }

    /// Largest first. The cursor follows its entry rather than its index, so a
    /// late arrival reordering the list cannot move the selection off the row
    /// the operator was reading.
    fn sort_entries(&mut self) {
        let anchor = self
            .entries
            .get(self.selected)
            .map(|entry| entry.path.clone());
        self.entries.sort_by(|left, right| {
            right
                .size
                .cmp(&left.size)
                .then_with(|| left.path.cmp(&right.path))
        });
        if let Some(anchor) = anchor
            && let Some(index) = self.entries.iter().position(|entry| entry.path == anchor)
        {
            self.selected = index;
        }
    }

    fn descend(&mut self) {
        let Some(entry) = self.entries.get(self.selected) else {
            return;
        };
        if !entry.is_dir {
            return;
        }
        let path = entry.path.clone();
        // `is_dir` was decided when the entry was measured. Re-check it here,
        // immediately before entering, because measurement calls `read_dir`,
        // which follows symlinks: a path swapped for a link after the listing
        // would otherwise be walked outside this tree.
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                self.errors.push(format!(
                    "refusing to enter {}: it is no longer a directory",
                    path.display()
                ));
                return;
            }
            Err(error) => {
                self.errors
                    .push(format!("cannot enter {}: {error}", path.display()));
                return;
            }
        }
        self.stack.push(path);
        self.begin_scan();
    }

    fn ascend(&mut self) {
        if self.stack.len() <= 1 {
            return;
        }
        self.stack.pop();
        self.begin_scan();
    }

    fn handle_key(&mut self, key: KeyEvent) -> Intent {
        if key.kind != KeyEventKind::Press {
            return Intent::None;
        }
        if key.modifiers.contains(event::KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Intent::Quit;
        }
        if self.help {
            if matches!(
                key.code,
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q')
            ) {
                self.help = false;
            }
            return Intent::None;
        }
        match key.code {
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('q') => return Intent::Quit,
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let last = self.entries.len().saturating_sub(1);
                self.selected = self.selected.saturating_add(1).min(last);
            }
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.descend(),
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('b') => self.ascend(),
            KeyCode::Char('r') => self.begin_scan(),
            _ => {}
        }
        Intent::None
    }
}

/// Proportional bar in eighth-cell resolution.
fn bar(size: u64, largest: u64, width: usize) -> String {
    if largest == 0 || width == 0 {
        return String::new();
    }
    let eighths = (size as u128)
        .saturating_mul(width as u128)
        .saturating_mul(8)
        / u128::from(largest).max(1);
    let full = (eighths / 8) as usize;
    let remainder = (eighths % 8) as usize;
    let mut rendered = String::new();
    for _ in 0..full.min(width) {
        rendered.push(BLOCKS[7]);
    }
    if full < width && remainder > 0 {
        rendered.push(BLOCKS[remainder.saturating_sub(1)]);
    }
    rendered
}

fn render(frame: &mut Frame, explorer: &Explorer) {
    let area = frame.area();
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(3),
    ])
    .areas(area);

    let total = explorer.total();
    let partial = explorer.entries.iter().any(|entry| entry.partial);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" analyze ", explorer.theme.bold(Token::Accent)),
            Span::raw(report::terminal_safe(
                &explorer.current().display().to_string(),
            )),
            Span::styled(
                format!("  {}{}", report::gb(total), if partial { "+" } else { "" }),
                explorer.theme.style(Token::AccentSecondary),
            ),
        ]))
        .block(Block::bordered().title(" measure · classify · trim ")),
        header,
    );

    let largest = explorer.entries.first().map_or(0, |entry| entry.size);
    let bar_width = usize::from(body.width).saturating_sub(40).clamp(0, 24);
    let items = explorer.entries.iter().map(|entry| {
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("{:>9} ", report::gb(entry.size)),
                explorer.theme.style(Token::AccentSecondary),
            ),
            Span::styled(
                format!(
                    "{:<width$} ",
                    bar(entry.size, largest, bar_width),
                    width = bar_width
                ),
                explorer.theme.style(Token::Muted),
            ),
            Span::raw(if entry.is_dir { "/" } else { " " }),
            Span::raw(report::terminal_safe(&entry.name())),
            Span::styled(
                if entry.partial { "  (partial)" } else { "" },
                explorer.theme.style(Token::Warning),
            ),
        ]))
    });
    let mut state = ListState::default();
    state.select((!explorer.entries.is_empty()).then_some(explorer.selected));
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(if explorer.scanning {
                " Measuring… "
            } else {
                " Contents "
            }))
            .highlight_symbol("▶ ")
            .highlight_style(explorer.theme.bold(Token::Accent)),
        body,
        &mut state,
    );

    let status = explorer.errors.first().map_or_else(
        || {
            if partial {
                "Sizes marked (partial) are lower bounds: some entries were unreadable.".to_string()
            } else {
                "Read-only. This screen never deletes anything.".to_string()
            }
        },
        |error| report::terminal_safe(error),
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                "↑/↓ move · Enter open · Esc up · r rescan · ? keys · q quit",
                explorer.theme.style(Token::AccentSecondary),
            ),
            Line::styled(
                status,
                explorer.theme.style(if explorer.errors.is_empty() {
                    Token::Muted
                } else {
                    Token::Critical
                }),
            ),
        ])
        .block(Block::bordered()),
        footer,
    );

    if explorer.help {
        render_help(frame, area, explorer);
    }
}

fn render_help(frame: &mut Frame, area: Rect, explorer: &Explorer) {
    let width = area.width.saturating_sub(4).min(72);
    let height = area.height.saturating_sub(2).min(16);
    let popup = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, popup);
    let mut lines = vec![
        Line::styled("Keys", explorer.theme.bold(Token::Accent)),
        Line::raw(""),
    ];
    for (key, description) in HELP_KEYS {
        lines.push(Line::from(vec![
            Span::styled(format!("  {key:<14}"), explorer.theme.style(Token::Muted)),
            Span::raw(*description),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Read-only: analyze never deletes. Use `devtrim clean <category>` for that.",
        explorer.theme.style(Token::Warning),
    ));
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Help · ? or Esc closes ")),
        popup,
    );
}

const HELP_KEYS: &[(&str, &str)] = &[
    ("↑/↓, k/j", "move the cursor"),
    ("Enter, →, l", "open the selected directory"),
    ("Esc, ←, h, b", "go up one level"),
    ("r", "measure this directory again"),
    ("?", "open or close this reference"),
    ("q, Ctrl+C", "quit"),
];

/// Validates the requested starting point before any terminal is taken over.
fn resolve_root(ctx: &Ctx, requested: Option<&str>) -> Result<PathBuf> {
    let path = requested.map_or_else(|| ctx.home.clone(), PathBuf::from);
    let metadata = std::fs::symlink_metadata(&path)
        .with_context(|| format!("cannot inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("refusing to explore through a symlink: {}", path.display());
    }
    if !metadata.file_type().is_dir() {
        bail!("not a directory: {}", path.display());
    }
    Ok(path)
}

/// One-shot machine-readable breakdown of the requested directory.
fn run_json(ctx: &Ctx, requested: Option<&str>) -> Result<ExitCode> {
    let root = resolve_root(ctx, requested)?;
    let cancel = AtomicBool::new(false);
    let mut entries = Vec::new();
    let result = measure_children(&root, &cancel, |entry| entries.push(entry));
    let errors = match result {
        Ok(()) => Vec::new(),
        Err(error) => vec![format!("{error:#}")],
    };
    entries.sort_by(|left, right| {
        right
            .size
            .cmp(&left.size)
            .then_with(|| left.path.cmp(&right.path))
    });
    let findings = entries
        .iter()
        .map(|entry| {
            Finding::new(
                if entry.is_dir { "directory" } else { "file" },
                Some(entry.path.clone()),
                entry.size,
                if entry.partial {
                    "report-only; lower bound, some entries below this path were unreadable"
                } else {
                    "report-only; estimated logical bytes"
                },
                1,
                Action::Info,
            )
        })
        .collect::<Vec<_>>();
    report::print_json("analyze", false, &findings, None, &errors)?;
    Ok(if errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

pub fn run(ctx: &Ctx, requested: Option<&str>) -> Result<ExitCode> {
    if ctx.json {
        return run_json(ctx, requested);
    }
    let root = resolve_root(ctx, requested)?;
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("analyze requires an interactive terminal, or --json for automation");
    }
    let mut terminal = ratatui::try_init().context("cannot initialize analyze terminal")?;
    let result = run_loop(&mut terminal, root, ctx);
    let restore = ratatui::try_restore().context("cannot restore terminal after analyze exit");
    match (result, restore) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok(code), Ok(())) => Ok(code),
    }
}

fn run_loop(terminal: &mut DefaultTerminal, root: PathBuf, ctx: &Ctx) -> Result<ExitCode> {
    let mut explorer = Explorer::new(root, Theme::from_env());
    explorer.begin_scan();
    let outcome = loop {
        explorer.drain_progress();
        terminal.draw(|frame| render(frame, &explorer))?;
        if event::poll(POLL_INTERVAL).context("cannot poll terminal input")?
            && let Event::Key(key) = event::read().context("cannot read terminal input")?
            && explorer.handle_key(key) == Intent::Quit
        {
            break if explorer.errors.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            };
        }
    };
    // Leave no worker walking a tree after the interface is gone.
    explorer.cancel.store(true, Ordering::Relaxed);
    for error in &explorer.errors {
        ctx.diagnostic("warn", error.clone());
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, bytes: usize) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    fn theme() -> Theme {
        Theme::new(crate::theme::ColorSupport::Named)
    }

    #[test]
    fn measures_each_child_and_ranks_largest_first() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-analyze")
            .tempdir()
            .unwrap();
        write(&root.path().join("small/one.bin"), 1024);
        write(&root.path().join("big/two.bin"), 8192);
        write(&root.path().join("big/nested/three.bin"), 4096);
        write(&root.path().join("loose.bin"), 512);

        let cancel = AtomicBool::new(false);
        let mut entries = Vec::new();
        measure_children(root.path(), &cancel, |entry| entries.push(entry)).unwrap();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.size));

        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].size, 8192 + 4096);
        assert!(entries[0].is_dir);
        assert_eq!(entries[1].size, 1024);
        assert_eq!(entries[2].size, 512);
        assert!(!entries[2].is_dir);
    }

    /// A symlinked directory must be reported at its own size, never resolved:
    /// following it would attribute another tree's bytes to this one.
    #[test]
    fn a_symlinked_child_is_not_followed() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-analyze-symlink")
            .tempdir()
            .unwrap();
        let outside = tempfile::Builder::new()
            .prefix("devtrim-analyze-outside")
            .tempdir()
            .unwrap();
        write(&outside.path().join("huge.bin"), 65536);
        std::fs::create_dir_all(root.path().join("inside")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();

        let cancel = AtomicBool::new(false);
        let mut entries = Vec::new();
        measure_children(root.path(), &cancel, |entry| entries.push(entry)).unwrap();

        let link = entries
            .iter()
            .find(|entry| entry.path.ends_with("link"))
            .unwrap();
        assert!(
            link.size < 65536,
            "the symlink must not carry the target tree's size"
        );
    }

    #[test]
    fn cancellation_stops_measurement() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-analyze-cancel")
            .tempdir()
            .unwrap();
        write(&root.path().join("dir/file.bin"), 4096);

        let cancel = AtomicBool::new(true);
        let mut entries = Vec::new();
        measure_children(root.path(), &cancel, |entry| entries.push(entry)).unwrap();
        assert!(entries.is_empty(), "a cancelled walk must report nothing");
    }

    #[test]
    fn bar_is_proportional_and_bounded() {
        assert_eq!(bar(0, 100, 8), "");
        assert_eq!(bar(100, 100, 8).chars().count(), 8);
        assert_eq!(bar(50, 100, 8).chars().count(), 4);
        // Never wider than the space it was given, whatever the inputs.
        assert!(bar(u64::MAX, 1, 10).chars().count() <= 10);
        assert_eq!(bar(1, 0, 8), "");
        assert_eq!(bar(1, 100, 0), "");
    }

    #[test]
    fn the_cursor_follows_its_entry_when_late_results_reorder_the_list() {
        let mut explorer = Explorer::new(PathBuf::from("/tmp"), theme());
        explorer.entries = vec![
            Entry {
                path: PathBuf::from("/tmp/a"),
                size: 10,
                is_dir: true,
                partial: false,
            },
            Entry {
                path: PathBuf::from("/tmp/b"),
                size: 5,
                is_dir: true,
                partial: false,
            },
        ];
        explorer.selected = 1;
        // `b` grows past `a` and moves to the top; the cursor must stay on `b`.
        explorer.entries[1].size = 100;
        explorer.sort_entries();
        assert_eq!(explorer.entries[0].path, PathBuf::from("/tmp/b"));
        assert_eq!(explorer.selected, 0);
    }

    #[test]
    fn navigation_never_leaves_the_starting_root() {
        let mut explorer = Explorer::new(PathBuf::from("/tmp"), theme());
        assert_eq!(explorer.current(), Path::new("/tmp"));
        explorer.ascend();
        assert_eq!(
            explorer.current(),
            Path::new("/tmp"),
            "ascending past the starting root must be refused"
        );
    }

    #[test]
    fn a_file_is_never_descended_into() {
        let mut explorer = Explorer::new(PathBuf::from("/tmp"), theme());
        explorer.entries = vec![Entry {
            path: PathBuf::from("/tmp/file.bin"),
            size: 10,
            is_dir: false,
            partial: false,
        }];
        explorer.descend();
        assert_eq!(explorer.current(), Path::new("/tmp"));
    }

    /// `is_dir` is decided at measurement time; measurement itself uses
    /// `read_dir`, which follows links. A path swapped for a symlink between
    /// the listing and the keypress must be refused, not walked.
    #[test]
    fn descend_refuses_an_entry_swapped_for_a_symlink_after_listing() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-analyze-swap")
            .tempdir()
            .unwrap();
        let outside = tempfile::Builder::new()
            .prefix("devtrim-analyze-swap-target")
            .tempdir()
            .unwrap();
        let real = root.path().join("child");
        std::fs::create_dir_all(&real).unwrap();

        let mut explorer = Explorer::new(root.path().to_path_buf(), theme());
        explorer.entries = vec![Entry {
            path: real.clone(),
            size: 0,
            is_dir: true,
            partial: false,
        }];

        // The listing said "directory"; the filesystem now says "symlink".
        std::fs::remove_dir(&real).unwrap();
        std::os::unix::fs::symlink(outside.path(), &real).unwrap();

        explorer.descend();
        assert_eq!(
            explorer.current(),
            root.path(),
            "a swapped symlink must not be entered"
        );
        assert!(
            explorer
                .errors
                .iter()
                .any(|error| error.contains("refusing to enter")),
            "the refusal must be visible: {:?}",
            explorer.errors
        );
    }

    #[test]
    fn resolve_root_refuses_a_symlink_and_a_file() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-analyze-root")
            .tempdir()
            .unwrap();
        write(&root.path().join("file.bin"), 16);
        std::os::unix::fs::symlink(root.path(), root.path().join("link")).unwrap();

        let ctx = crate::safety::Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: root.path().join("journal.jsonl"),
            home: root.path().to_path_buf(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Stderr,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };

        let file = root.path().join("file.bin");
        assert!(resolve_root(&ctx, Some(&file.display().to_string())).is_err());
        let link = root.path().join("link");
        assert!(resolve_root(&ctx, Some(&link.display().to_string())).is_err());
        assert!(resolve_root(&ctx, None).is_ok());
    }
}
