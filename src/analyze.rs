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

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::io::{self, IsTerminal};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::explain::{Relation, Subject, Target};
use crate::report::{self, Action, Finding};
use crate::safety::Ctx;
use crate::theme::{Theme, Token};

/// How many progress messages one frame may absorb. A directory with tens of
/// thousands of children would otherwise let the producer starve key handling.
const DRAIN_BUDGET: usize = 512;

/// Poll for worker progress without delaying keyboard events.
const POLL_INTERVAL: Duration = Duration::from_millis(80);

/// Sub-cell bar resolution. A terminal cell is the smallest unit devtrim can
/// paint, so eighths are the only way to show a proportion honestly at this size.
const BLOCKS: [char; 8] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉', '█'];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    path: PathBuf,
    size: u64,
    is_dir: bool,
    /// True when traversal could not read everything below this entry, so the
    /// size is a lower bound rather than a measurement.
    partial: bool,
    /// Where most of a directory's space is, when one child chain holds it.
    culprit: Option<Culprit>,
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

/// Where most of a directory's space is: the chain of children, each holding
/// at least half of the folder above it, that a walk found below an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Culprit {
    /// Names below the entry, outermost first; never empty.
    chain: Vec<OsString>,
    /// Allocated bytes of the chain's last element.
    bytes: u64,
    /// Whether the last element is a directory (else a regular file).
    last_is_dir: bool,
    /// The walk was partial, or met a file with further hard links, which is
    /// counted in the first folder that reaches it: the chain's sizes are then
    /// lower bounds rather than measurements.
    lower_bound: bool,
}

/// A chain stops at a folder that is one thing to the operator: an install,
/// a bundle, or a Git repository (judged by a `.git` child, see [`Frame`]).
fn is_whole_unit(name: &OsStr) -> bool {
    let bytes = name.as_encoded_bytes();
    bytes.eq_ignore_ascii_case(b"node_modules")
        || [
            &b".app"[..],
            b".framework",
            b".bundle",
            b".photoslibrary",
            b".xcarchive",
        ]
        .iter()
        .any(|suffix| {
            bytes.len() > suffix.len()
                && bytes[bytes.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
        })
}

/// The largest child a folder has shown so far, with its own chain.
#[derive(Debug, Default)]
struct Largest {
    bytes: u64,
    name: OsString,
    is_dir: bool,
    /// The child's own chain, `None` for a file or a whole unit.
    below: Option<(Vec<OsString>, u64, bool)>,
}

/// One open directory of the post-order walk: what its children add up to
/// and which of them is largest. One frame per level, so memory follows the
/// walk's depth, never the number of directories.
#[derive(Debug, Default)]
struct Level {
    total: u64,
    largest: Option<Largest>,
    /// A child named `.git` (any ASCII case): this folder is a repository.
    repository: bool,
}

impl Level {
    /// Larger first; between equal sizes the name that sorts first, as the
    /// listing orders them.
    fn offer(
        &mut self,
        bytes: u64,
        name: &OsStr,
        is_dir: bool,
        below: Option<(Vec<OsString>, u64, bool)>,
    ) {
        self.total = self.total.saturating_add(bytes);
        let better = self.largest.as_ref().is_none_or(|largest| {
            bytes > largest.bytes || (bytes == largest.bytes && name < largest.name.as_os_str())
        });
        if better {
            self.largest = Some(Largest {
                bytes,
                name: name.to_os_string(),
                is_dir,
                below,
            });
        }
    }

    /// This folder's chain: its largest child when that child holds at least
    /// half of it, then that child's own chain.
    fn chain(self) -> Option<(Vec<OsString>, u64, bool)> {
        let largest = self.largest?;
        if largest.bytes == 0 || u128::from(largest.bytes) * 2 < u128::from(self.total) {
            return None;
        }
        Some(match largest.below {
            Some((mut chain, bytes, last_is_dir)) => {
                chain.insert(0, largest.name);
                (chain, bytes, last_is_dir)
            }
            None => (vec![largest.name], largest.bytes, largest.is_dir),
        })
    }
}

struct BulkUnsupported;

enum BulkTreeStop {
    Overflow,
}

struct PendingDir {
    path: PathBuf,
    name: OsString,
    subdirs: Vec<(PathBuf, OsString)>,
    level: Level,
    opened: bool,
}

struct BulkTreeState<'a> {
    device: u64,
    total: u64,
    partial: bool,
    linked: bool,
    hard_links: HashSet<(u64, u64)>,
    cancel: &'a AtomicBool,
}

impl BulkTreeState<'_> {
    fn open_frame(
        &mut self,
        frame: &mut PendingDir,
        buffer: &mut apfs_size::BulkBuffer,
    ) -> std::result::Result<bool, BulkUnsupported> {
        let dir_path = &frame.path;
        let level = &mut frame.level;
        let mut subdirs = Vec::new();
        let visit =
            apfs_size::visit_dir_bulk(dir_path, apfs_size::BulkMode::SizeOnly, buffer, |entry| {
                if crate::safety::is_git_metadata_name(entry.name) {
                    level.repository = true;
                }
                match entry.kind {
                    apfs_size::EntryKind::Directory => {
                        if entry.dev != self.device {
                            self.partial = true;
                            level.offer(0, entry.name, true, None);
                        } else {
                            subdirs.push((dir_path.join(entry.name), entry.name.to_os_string()));
                        }
                    }
                    apfs_size::EntryKind::File => self.visit_file(level, entry)?,
                    apfs_size::EntryKind::Other => {}
                }
                Ok(())
            });
        match visit {
            Ok(()) => {}
            Err(apfs_size::BulkVisitError::Bulk(apfs_size::BulkError::Unsupported)) => {
                return Err(BulkUnsupported);
            }
            Err(apfs_size::BulkVisitError::Bulk(apfs_size::BulkError::Io(_))) => {
                self.partial = true;
            }
            Err(apfs_size::BulkVisitError::Visitor(BulkTreeStop::Overflow)) => {
                return Ok(true);
            }
        }
        subdirs.reverse();
        frame.subdirs = subdirs;
        Ok(false)
    }

    fn visit_file(
        &mut self,
        level: &mut Level,
        entry: apfs_size::BulkEntryRef<'_>,
    ) -> std::result::Result<(), BulkTreeStop> {
        if entry.dev != self.device {
            self.partial = true;
            return Ok(());
        }
        let bytes = if entry.nlink > 1 {
            self.linked = true;
            if self.hard_links.insert((entry.dev, entry.ino)) {
                entry.allocated
            } else {
                0
            }
        } else {
            entry.allocated
        };
        level.offer(bytes, entry.name, false, None);
        let Some(sum) = self.total.checked_add(bytes) else {
            self.partial = true;
            return Err(BulkTreeStop::Overflow);
        };
        self.total = sum;
        Ok(())
    }
}

/// Recursively sums regular-file sizes below `path`, and finds in the same
/// walk where most of them are ([`Culprit`]).
///
/// Symbolic links are never followed and a different device is never entered:
/// an explorer that wandered onto a network mount would appear to hang, and one
/// that followed a link would attribute another tree's bytes to this one.
/// Returns the total, whether anything was unreadable, and the culprit.
fn measure_tree(path: &Path, device: u64, cancel: &AtomicBool) -> (u64, bool, Option<Culprit>) {
    if !std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir()) {
        return measure_tree_walkdir(path, device, cancel);
    }
    match measure_tree_bulk(path, device, cancel) {
        Ok(result) => result,
        Err(BulkUnsupported) => measure_tree_walkdir(path, device, cancel),
    }
}

fn measure_tree_bulk(
    path: &Path,
    device: u64,
    cancel: &AtomicBool,
) -> std::result::Result<(u64, bool, Option<Culprit>), BulkUnsupported> {
    let mut state = BulkTreeState {
        device,
        total: 0,
        partial: false,
        linked: false,
        hard_links: HashSet::new(),
        cancel,
    };
    let mut buffer = apfs_size::BulkBuffer::new();
    let mut root_chain = None;
    let mut stack = vec![PendingDir {
        path: path.to_path_buf(),
        name: OsString::new(),
        subdirs: Vec::new(),
        level: Level::default(),
        opened: false,
    }];

    while let Some(frame) = stack.last_mut() {
        if state.cancel.load(Ordering::Relaxed) {
            return Ok((state.total, true, None));
        }
        if !frame.opened {
            frame.opened = true;
            if state.open_frame(frame, &mut buffer)? {
                return Ok((state.total, true, None));
            }
        }
        if let Some((child_path, child_name)) = frame.subdirs.pop() {
            stack.push(PendingDir {
                path: child_path,
                name: child_name,
                subdirs: Vec::new(),
                level: Level::default(),
                opened: false,
            });
            continue;
        }
        let Some(finished) = stack.pop() else {
            break;
        };
        let own_total = finished.level.total;
        let unit = finished.level.repository || is_whole_unit(&finished.name);
        let chain = finished.level.chain();
        if let Some(parent) = stack.last_mut() {
            parent.level.offer(
                own_total,
                &finished.name,
                true,
                if unit { None } else { chain },
            );
        } else {
            root_chain = chain;
        }
    }

    let culprit = root_chain.map(|(chain, bytes, last_is_dir)| Culprit {
        chain,
        bytes,
        last_is_dir,
        lower_bound: state.partial || state.linked,
    });
    Ok((state.total, state.partial, culprit))
}

fn measure_tree_walkdir(
    path: &Path,
    device: u64,
    cancel: &AtomicBool,
) -> (u64, bool, Option<Culprit>) {
    let mut total = 0u64;
    let mut usage = crate::safety::DiskUsage::default();
    let mut partial = false;
    let mut linked = false;
    // `frames[depth]` gathers the children of the open directory at `depth`.
    // The walk yields contents first, so a directory arrives after all of
    // its children and closes its frame.
    let mut frames: Vec<Level> = Vec::new();
    let mut root_chain = None;
    // `same_file_system` stops the walk at the mount point instead of filtering
    // afterwards. Filtering after the fact still descends: a stalled network
    // mount blocks inside `WalkDir::next()`, where the cancellation flag is
    // never read, and the thread cannot be released by quitting.
    for result in walkdir::WalkDir::new(path)
        .follow_links(false)
        .follow_root_links(false)
        .same_file_system(true)
        .contents_first(true)
    {
        if cancel.load(Ordering::Relaxed) {
            return (total, true, None);
        }
        let Ok(entry) = result else {
            partial = true;
            continue;
        };
        let depth = entry.depth();
        while frames.len() < depth {
            frames.push(Level::default());
        }
        if depth > 0 && crate::safety::is_git_metadata_name(entry.file_name()) {
            frames[depth - 1].repository = true;
        }
        if entry.file_type().is_dir() {
            // A directory walkdir could not open, or one on another device,
            // arrives without children and so without a frame.
            let own = if frames.len() > depth {
                // Deeper frames left open would be a walk that skipped a
                // directory's own entry; their bytes still belong here.
                while frames.len() > depth + 1 {
                    let orphan = frames.pop().unwrap_or_default();
                    if let Some(parent) = frames.last_mut() {
                        parent.total = parent.total.saturating_add(orphan.total);
                    }
                    partial = true;
                }
                frames.pop().unwrap_or_default()
            } else {
                Level::default()
            };
            let (bytes, unit) = (
                own.total,
                own.repository || is_whole_unit(entry.file_name()),
            );
            let chain = own.chain();
            if depth == 0 {
                root_chain = chain;
            } else {
                frames[depth - 1].offer(
                    bytes,
                    entry.file_name(),
                    true,
                    if unit { None } else { chain },
                );
            }
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            partial = true;
            continue;
        };
        if metadata.dev() != device {
            // `same_file_system` ties the walk to the device walkdir reads for
            // its own root, not to `device`, which the caller read earlier: a
            // mount made at this child in between would pass walkdir. A
            // subtree left unmeasured is a lower bound and must say so rather
            // than vanish into a confident total.
            partial = true;
            continue;
        }
        linked |= metadata.nlink() > 1;
        // Measured bytes never saturate silently: an overflowed total that
        // still presents itself as a measurement is the shape S8 forbids. It
        // cannot happen with real file sizes, so it becomes a lower bound
        // rather than an error the explorer has no way to show.
        let Ok(bytes) = usage.add(&metadata, entry.path()) else {
            partial = true;
            continue;
        };
        if depth > 0 {
            frames[depth - 1].offer(bytes, entry.file_name(), false, None);
        }
        match total.checked_add(bytes) {
            Some(sum) => total = sum,
            None => {
                partial = true;
                break;
            }
        }
    }
    let culprit = root_chain.map(|(chain, bytes, last_is_dir)| Culprit {
        chain,
        bytes,
        last_is_dir,
        lower_bound: partial || linked,
    });
    (total, partial, culprit)
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

fn measure_child(path: PathBuf, device: u64, cancel: &AtomicBool) -> Result<Option<Entry>> {
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        // Only a genuine disappearance between listing and stat is a race
        // worth ignoring. Any other error would otherwise drop a child from
        // the view with no `(partial)` marker and no error, which
        // contradicts this command's lower-bound disclosure.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", path.display()));
        }
    };
    // A symlink is listed at its own size, never resolved: its target may
    // live outside this tree entirely.
    if metadata.file_type().is_dir() {
        // A foreign mount must be refused BEFORE the walk starts.
        // Entering a mount would authorize that mount's device and traverse
        // all of it, which is exactly the network-mount hang the boundary
        // exists to prevent.
        //
        // What this does NOT catch, and cannot: the macOS system/data
        // firmlink. `/`, `/System/Volumes/Data` and `/Users` all report the
        // same `st_dev` even though `df` shows separate APFS volumes, so
        // only `statfs` distinguishes them. That is correct to allow —
        // walking `/` into the user's own data is what analyzing `/` means
        // — but it is why measuring `/` is slow, not a broken boundary.
        if metadata.dev() != device {
            return Ok(Some(Entry {
                path,
                size: 0,
                is_dir: true,
                partial: true,
                culprit: None,
            }));
        }
        let (size, partial, culprit) = measure_tree(&path, device, cancel);
        Ok(Some(Entry {
            path,
            size,
            is_dir: true,
            partial,
            culprit,
        }))
    } else {
        // A file that cannot be sized is a lower bound, never a guess.
        let (size, partial) = match crate::safety::allocated_bytes(&metadata, &path) {
            Ok(size) => (size, false),
            Err(_) => (0, true),
        };
        Ok(Some(Entry {
            path,
            size,
            is_dir: false,
            partial,
            culprit: None,
        }))
    }
}

/// Measures every immediate child of `directory`, reporting each as it finishes.
fn measure_children(
    directory: &Path,
    cancel: &AtomicBool,
    mut report_entry: impl FnMut(Entry),
) -> Result<()> {
    let device = std::fs::symlink_metadata(directory)
        .with_context(|| format!("cannot inspect {}", directory.display()))?
        .dev();
    let paths = child_paths(directory)?;
    let max_workers = std::thread::available_parallelism()
        .map_or(4, std::num::NonZeroUsize::get)
        .min(8);
    let workers = paths.len().min(max_workers);
    if workers <= 1 {
        for path in paths {
            if cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            if let Some(entry) = measure_child(path, device, cancel)? {
                report_entry(entry);
            }
        }
        return Ok(());
    }
    let next_index = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let (sender, receiver) = channel::<Result<Option<Entry>>>();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let worker_sender = sender.clone();
            let paths = &paths;
            let next_index = &next_index;
            let stop = &stop;
            scope.spawn(move || {
                while !cancel.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                    let index = next_index.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index) else {
                        break;
                    };
                    let result = measure_child(path.clone(), device, cancel);
                    let failed = result.is_err();
                    if worker_sender.send(result).is_err() || failed {
                        stop.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            });
        }
        drop(sender);
        for message in receiver {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            match message {
                Ok(Some(entry)) => report_entry(entry),
                Ok(None) => {}
                Err(error) => {
                    stop.store(true, Ordering::Relaxed);
                    return Err(error);
                }
            }
        }
        Ok(())
    })
}

/// What devtrim's own categories say about one entry, from one fresh scan.
/// Display only: nothing here is a finding, a target or an action, so no
/// verdict can reach an apply.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// The entry is an actionable finding's exact target.
    Offered {
        action: &'static str,
        command: String,
    },
    /// Actionable findings lie strictly inside the entry.
    Holds {
        count: usize,
        bytes: u64,
        command: String,
    },
    /// The entry lies inside an offered target, so it would go with it.
    InOffer { command: String },
    /// A built-in rule or a configured `protect` entry refuses it.
    Protected(String),
    /// Outside every scan root and every folder a finding came from. No
    /// interface says where each category looks, so this is what the scan
    /// can show, not proof that no category looked here.
    NotOffered,
}

impl Verdict {
    fn tag(&self) -> String {
        match self {
            Self::Offered { .. } => "OFFERED".into(),
            Self::Holds { count, .. } => format!("HOLDS {count}"),
            Self::InOffer { .. } => "IN OFFER".into(),
            Self::Protected(_) => "PROTECTED".into(),
            Self::NotOffered => "NOT OFFERED".into(),
        }
    }

    fn token(&self) -> Token {
        match self {
            Self::Offered { .. } | Self::InOffer { .. } => Token::Warning,
            Self::Holds { .. } => Token::AccentSecondary,
            Self::Protected(_) => Token::Info,
            Self::NotOffered => Token::Muted,
        }
    }

    /// The status line for the highlighted entry. Raw: the footer escapes.
    fn detail(&self) -> String {
        match self {
            Self::Offered { action, command } => format!("OFFERED · {action} · {command}"),
            Self::Holds {
                count,
                bytes,
                command,
            } => format!(
                "HOLDS {count} offer{} · {} · {command}",
                if *count == 1 { "" } else { "s" },
                report::gb(*bytes)
            ),
            Self::InOffer { command } => format!("IN OFFER · inside a target of {command}"),
            Self::Protected(reason) => format!("PROTECTED · {reason}"),
            Self::NotOffered => {
                "NOT OFFERED · outside the scan roots; nothing reported here".into()
            }
        }
    }

    /// What the filter keeps: anything an offer reaches, inside one too.
    fn holds_offers(&self) -> bool {
        matches!(
            self,
            Self::Offered { .. } | Self::Holds { .. } | Self::InOffer { .. }
        )
    }
}

/// The word the TUI's plans use for what an actionable finding does.
fn action_word(action: &Action) -> Option<&'static str> {
    match action {
        Action::Trash => Some("TRASH"),
        Action::Shred => Some("PERMANENT"),
        Action::Command { .. } => Some("COMMAND"),
        Action::Info | Action::None => None,
    }
}

/// The command that previews, and with `--apply` acts on, a category.
fn category_command(category: &str) -> String {
    format!("devtrim clean {category}")
}

/// An actionable finding's target, kept only to compare paths with.
struct Offer {
    target: Target,
    category: &'static str,
    action: &'static str,
    bytes: u64,
}

/// One scan's findings, reduced to what the explorer compares entries with,
/// and each entry's verdict once judged.
struct Verdicts {
    offers: Vec<Offer>,
    /// The scan roots and the folder of every finding's target.
    covered: Vec<Target>,
    home: PathBuf,
    protect: Vec<PathBuf>,
    judged: HashMap<PathBuf, Option<Verdict>>,
}

impl Verdicts {
    fn from_scan(scan: &crate::ops::ScanResult, ctx: &Ctx) -> Self {
        let mut offers = Vec::new();
        let mut folders = ctx.roots.iter().cloned().collect::<BTreeSet<_>>();
        for section in &scan.sections {
            for finding in &scan.findings[section.range.clone()] {
                let Some(target) = finding.target() else {
                    continue;
                };
                if let Some(parent) = target.parent() {
                    folders.insert(parent.to_path_buf());
                }
                if let Some(action) = action_word(&finding.action) {
                    offers.push(Offer {
                        target: Target::of(target),
                        category: section.category,
                        action,
                        bytes: finding.size_bytes,
                    });
                }
            }
        }
        Self {
            offers,
            covered: folders.iter().map(|folder| Target::of(folder)).collect(),
            home: ctx.home.clone(),
            protect: ctx.protect.clone(),
            judged: HashMap::new(),
        }
    }

    /// `explain`'s matching: the path resolved as `explain` resolves it,
    /// then compared by whole components and by device and inode.
    fn judge(&self, path: &Path) -> Option<Verdict> {
        let resolved = crate::explain::resolve(path).unwrap_or_else(|_| path.to_path_buf());
        let subject = Subject::of(&resolved);
        let mut inside: Vec<&Offer> = Vec::new();
        let mut holder = None;
        for offer in &self.offers {
            match subject.relation(&offer.target) {
                Some(Relation::Exact) => {
                    return Some(Verdict::Offered {
                        action: offer.action,
                        command: category_command(offer.category),
                    });
                }
                Some(Relation::Inside) => inside.push(offer),
                Some(Relation::Holds) => holder = holder.or(Some(offer)),
                None => {}
            }
        }
        // What an entry holds outranks its own protection: `~/Library` is
        // protected and still holds the caches `clean caches` offers.
        if let Some(first) = inside.first() {
            let command = if inside.iter().all(|offer| offer.category == first.category) {
                category_command(first.category)
            } else {
                "devtrim scan".to_string()
            };
            return Some(Verdict::Holds {
                count: inside.len(),
                bytes: inside
                    .iter()
                    .fold(0u64, |sum, offer| sum.saturating_add(offer.bytes)),
                command,
            });
        }
        if let Some(offer) = holder {
            return Some(Verdict::InOffer {
                command: category_command(offer.category),
            });
        }
        if let Some(reason) = crate::safety::protection_reason(&resolved, &self.home) {
            return Some(Verdict::Protected(reason));
        }
        if let Some(entry) = crate::safety::config_protect_entry(&resolved, &self.protect) {
            return Some(Verdict::Protected(format!(
                "configured protect entry {}",
                entry.display()
            )));
        }
        if !self
            .covered
            .iter()
            .any(|folder| subject.relation(folder).is_some())
        {
            return Some(Verdict::NotOffered);
        }
        None
    }

    fn remember(&mut self, path: &Path) {
        if !self.judged.contains_key(path) {
            let verdict = self.judge(path);
            self.judged.insert(path.to_path_buf(), verdict);
        }
    }
}

/// Whether devtrim's verdicts are absent, on their way, or here.
enum VerdictState {
    Unloaded,
    Loading {
        started: Instant,
        receiver: Receiver<(Verdicts, Vec<String>)>,
    },
    Loaded(Verdicts),
}

/// The context the verdict scan runs with: `ctx`'s roots, windows and
/// protection, its diagnostics captured, since anything written to the
/// terminal would land on the explorer's screen. Owned, so the scan can run
/// on a detached thread that a quit never waits for.
fn scan_context(ctx: &Ctx) -> Ctx {
    Ctx {
        yes: false,
        yolo: false,
        json: false,
        roots: ctx.roots.clone(),
        roots_origin: ctx.roots_origin,
        active_days: ctx.active_days,
        retain_days: ctx.retain_days,
        home: ctx.home.clone(),
        protect: ctx.protect.clone(),
        journal_path: ctx.journal_path.clone(),
        interactive: false,
        diagnostic_output: crate::safety::DiagnosticOutput::Capture,
        diagnostics: Default::default(),
        journal_errors: Default::default(),
        apply_results: Default::default(),
        // The verdict scan never applies.
        apply_monitor: Default::default(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Intent {
    None,
    Redraw,
    Quit,
}

struct Explorer {
    /// Drill-down stack; the last element is the directory on screen.
    stack: Vec<PathBuf>,
    entries: Vec<Entry>,
    selected: usize,
    scanning: bool,
    /// Errors about the directory on screen, for the footer.
    errors: Vec<String>,
    /// Every error and lower bound of the session, each once: the folders
    /// left behind count toward the exit status as much as the one shown.
    session: Vec<String>,
    help: bool,
    theme: Theme,
    cancel: Arc<AtomicBool>,
    progress: Option<Receiver<Progress>>,
    /// The folder just left by going up, selected again once its new
    /// measurement arrives, unless the cursor moves first.
    reselect: Option<PathBuf>,
    /// A one-key answer, such as why a key did nothing; never an error.
    notice: Option<String>,
    /// The context a verdict scan runs with; absent where none can run.
    scan_context: Option<Arc<Ctx>>,
    verdicts: VerdictState,
    /// The second of the verdict scan last drawn, so its clock moves.
    shown_second: Option<u64>,
    /// Errors of the verdict scans, counted at exit like any other.
    verdict_errors: Vec<String>,
    /// Show only entries that are or hold an offer.
    filter: bool,
}

impl Explorer {
    fn new(root: PathBuf, theme: Theme) -> Self {
        Self {
            stack: vec![root],
            entries: Vec::new(),
            selected: 0,
            scanning: false,
            errors: Vec::new(),
            session: Vec::new(),
            help: false,
            theme,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: None,
            reselect: None,
            notice: None,
            scan_context: None,
            verdicts: VerdictState::Unloaded,
            shown_second: None,
            verdict_errors: Vec::new(),
            filter: false,
        }
    }

    /// Indexes into `entries` of the rows on screen: every entry, or with
    /// the filter on and verdicts loaded, those that are or hold an offer.
    fn shown(&self) -> Vec<usize> {
        (0..self.entries.len())
            .filter(|index| {
                !self.filtering()
                    || self
                        .verdict(&self.entries[*index])
                        .is_some_and(Verdict::holds_offers)
            })
            .collect()
    }

    fn filtering(&self) -> bool {
        self.filter && matches!(self.verdicts, VerdictState::Loaded(_))
    }

    fn highlighted(&self) -> Option<&Entry> {
        let shown = self.shown();
        shown.get(self.selected).map(|index| &self.entries[*index])
    }

    fn verdict(&self, entry: &Entry) -> Option<&Verdict> {
        match &self.verdicts {
            VerdictState::Loaded(verdicts) => verdicts.judged.get(&entry.path)?.as_ref(),
            _ => None,
        }
    }

    fn verdicts_loading(&self) -> bool {
        matches!(self.verdicts, VerdictState::Loading { .. })
    }

    /// Starts one scan of every category on a detached thread. It never
    /// applies: its findings only become [`Verdicts`].
    fn load_verdicts(&mut self) {
        if self.verdicts_loading() {
            self.notice = Some("devtrim's verdicts are still loading.".into());
            return;
        }
        let Some(context) = self.scan_context.clone() else {
            self.notice = Some("verdicts are not available in this view.".into());
            return;
        };
        let (sender, receiver) = channel();
        std::thread::spawn(move || {
            let scan = crate::ops::scan_all(&context);
            // Captured so they never reach the screen; the scan's errors
            // travel with its result.
            context.take_diagnostics();
            let _ = sender.send((Verdicts::from_scan(&scan, &context), scan.errors));
        });
        self.verdicts = VerdictState::Loading {
            started: Instant::now(),
            receiver,
        };
        self.shown_second = Some(0);
    }

    /// Takes a finished verdict scan, and says whether its clock moved.
    fn poll_verdicts(&mut self) -> bool {
        let VerdictState::Loading { started, receiver } = &self.verdicts else {
            return false;
        };
        let second = started.elapsed().as_secs();
        match receiver.try_recv() {
            Ok((mut verdicts, errors)) => {
                for entry in &self.entries {
                    verdicts.remember(&entry.path);
                }
                self.notice = (!errors.is_empty()).then(|| {
                    format!(
                        "verdicts are partial: {} scan error(s), listed at exit.",
                        errors.len()
                    )
                });
                for error in errors {
                    let error = format!("verdicts: {error}");
                    if !self.verdict_errors.contains(&error) {
                        self.verdict_errors.push(error);
                    }
                }
                // Read before the swap: with the filter on, the rows on
                // screen change with the verdicts.
                let anchor = self.highlighted().map(|entry| entry.path.clone());
                self.verdicts = VerdictState::Loaded(verdicts);
                self.shown_second = None;
                self.reposition(anchor);
                true
            }
            Err(TryRecvError::Disconnected) => {
                self.verdict_errors
                    .push("verdicts: the scan stopped before it finished".into());
                self.verdicts = VerdictState::Unloaded;
                self.shown_second = None;
                true
            }
            Err(TryRecvError::Empty) => {
                let moved = self.shown_second != Some(second);
                self.shown_second = Some(second);
                moved
            }
        }
    }

    /// Keeps the cursor on `anchor`, or on the row it was on, after the rows
    /// on screen changed; the first row when that row is gone.
    fn reposition(&mut self, anchor: Option<PathBuf>) {
        let anchor = anchor.or_else(|| self.highlighted().map(|entry| entry.path.clone()));
        let shown = self.shown();
        self.selected = anchor
            .and_then(|anchor| {
                shown
                    .iter()
                    .position(|index| self.entries[*index].path == anchor)
            })
            .unwrap_or(0);
    }

    fn toggle_filter(&mut self) {
        if !matches!(self.verdicts, VerdictState::Loaded(_)) {
            self.notice = Some("press v first to load devtrim's verdicts.".into());
            return;
        }
        let anchor = self.highlighted().map(|entry| entry.path.clone());
        self.filter = !self.filter;
        self.reposition(anchor);
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
        self.session = self.exit_report().1;
        self.entries.clear();
        self.errors.clear();
        self.selected = 0;
        self.reselect = None;
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
    fn drain_progress(&mut self) -> bool {
        let mut changed = false;
        for _ in 0..DRAIN_BUDGET {
            let Some(receiver) = self.progress.as_ref() else {
                break;
            };
            match receiver.try_recv() {
                Ok(Progress::Measured(entry)) => {
                    if let VerdictState::Loaded(verdicts) = &mut self.verdicts {
                        verdicts.remember(&entry.path);
                    }
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
                    changed = true;
                    self.scanning = false;
                    self.progress = None;
                    break;
                }
            }
        }
        if changed {
            self.sort_entries();
        }
        changed
    }

    /// Largest first. The cursor follows its entry rather than its index, so a
    /// late arrival reordering the list cannot move the selection off the row
    /// the operator was reading.
    fn sort_entries(&mut self) {
        let arrived = self
            .reselect
            .as_ref()
            .filter(|path| self.entries.iter().any(|entry| &entry.path == *path))
            .cloned();
        if arrived.is_some() {
            self.reselect = None;
        }
        let anchor = arrived.or_else(|| self.highlighted().map(|entry| entry.path.clone()));
        self.entries.sort_by(|left, right| {
            right
                .size
                .cmp(&left.size)
                .then_with(|| left.path.cmp(&right.path))
        });
        let shown = self.shown();
        if let Some(anchor) = anchor
            && let Some(index) = shown
                .iter()
                .position(|index| self.entries[*index].path == anchor)
        {
            self.selected = index;
        }
    }

    /// Whether `path` may be entered, checked immediately before entering.
    ///
    /// `is_dir` was decided when the entry was measured. Re-check it here
    /// because measurement calls `read_dir`, which follows symlinks: a path
    /// swapped for a link after the listing would otherwise be walked outside
    /// this tree. Measurement refuses to walk another device, but entering
    /// one would root the next walk there and authorize that device
    /// wholesale.
    fn enterable(&self, path: &Path) -> Result<(), String> {
        let root_device = self
            .stack
            .first()
            .and_then(|root| std::fs::symlink_metadata(root).ok())
            .map(|metadata| metadata.dev());
        match std::fs::symlink_metadata(path) {
            Ok(metadata)
                if metadata.file_type().is_dir() && Some(metadata.dev()) == root_device =>
            {
                Ok(())
            }
            Ok(metadata) if metadata.file_type().is_dir() => Err(format!(
                "refusing to enter {}: it is on a different device",
                path.display()
            )),
            Ok(_) => Err(format!(
                "refusing to enter {}: it is no longer a directory",
                path.display()
            )),
            Err(error) => Err(format!("cannot enter {}: {error}", path.display())),
        }
    }

    fn descend(&mut self) {
        let Some(entry) = self.highlighted() else {
            return;
        };
        if !entry.is_dir {
            return;
        }
        let path = entry.path.clone();
        if let Err(error) = self.enterable(&path) {
            self.errors.push(error);
            return;
        }
        self.stack.push(path);
        self.begin_scan();
    }

    /// Descends straight to the end of the highlighted folder's culprit
    /// chain: into its last directory, or to the dominant file's folder with
    /// that file selected. Every level is pushed and checked as `descend`
    /// checks one, so going back climbs one level at a time; a level that
    /// fails the check stops the descent there.
    fn follow_culprit(&mut self) {
        let Some(entry) = self.highlighted() else {
            return;
        };
        let Some(culprit) = entry.culprit.as_ref().filter(|_| entry.is_dir) else {
            self.notice = Some(format!("no single folder holds most of {}.", entry.name()));
            return;
        };
        let mut levels = vec![entry.path.clone()];
        let mut file = None;
        for (index, name) in culprit.chain.iter().enumerate() {
            let next = levels[levels.len() - 1].join(name);
            if index + 1 == culprit.chain.len() && !culprit.last_is_dir {
                file = Some(next);
            } else {
                levels.push(next);
            }
        }
        let mut refusal = None;
        let mut entered = 0;
        for level in &levels {
            if let Err(error) = self.enterable(level) {
                refusal = Some(error);
                break;
            }
            entered += 1;
        }
        if entered == 0 {
            self.errors.extend(refusal);
            return;
        }
        let complete = entered == levels.len();
        self.stack.extend(levels.into_iter().take(entered));
        self.begin_scan();
        self.errors.extend(refusal);
        if complete {
            self.reselect = file;
        }
    }

    fn ascend(&mut self) {
        if self.stack.len() <= 1 {
            return;
        }
        let left = self.stack.pop();
        self.begin_scan();
        self.reselect = left;
    }

    /// Whether the session failed, and the warnings printed when it ends:
    /// what earlier folders left in the ledger, then the folder on screen.
    fn exit_report(&self) -> (bool, Vec<String>) {
        let mut warnings = self.session.clone();
        let current = lower_bound_errors(&self.entries);
        for warning in self
            .errors
            .iter()
            .chain(&current)
            .chain(&self.verdict_errors)
        {
            if !warnings.contains(warning) {
                warnings.push(warning.clone());
            }
        }
        (!warnings.is_empty(), warnings)
    }

    /// Below the supported size nothing is drawn but the resize screen, so
    /// only a quit reaches the explorer; Esc, which goes up a level here,
    /// must not act on a listing nobody can see.
    fn handle_visible_key(&mut self, key: KeyEvent, area: Rect) -> Intent {
        if !crate::tui::terminal_too_small(area) {
            return self.handle_key(key);
        }
        if key.kind == KeyEventKind::Press
            && (key.code == KeyCode::Char('q') || crate::tui::is_interrupt(key))
        {
            Intent::Quit
        } else {
            Intent::None
        }
    }

    /// Moves the cursor to `index`, clamped to the listing; the operator has
    /// chosen a row, so a pending reselection no longer applies.
    fn select(&mut self, index: usize) {
        self.selected = index.min(self.shown().len().saturating_sub(1));
        self.reselect = None;
    }

    fn handle_key(&mut self, key: KeyEvent) -> Intent {
        if key.kind != KeyEventKind::Press {
            return Intent::None;
        }
        if crate::tui::is_interrupt(key) {
            return Intent::Quit;
        }
        if self.help {
            if matches!(
                key.code,
                KeyCode::Char('?') | KeyCode::Esc | KeyCode::Char('q')
            ) {
                self.help = false;
                return Intent::Redraw;
            }
            return Intent::None;
        }
        self.notice = None;
        match key.code {
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('m') => self.follow_culprit(),
            KeyCode::Char('v') => self.load_verdicts(),
            KeyCode::Char('f') => self.toggle_filter(),
            KeyCode::Char('q') => return Intent::Quit,
            KeyCode::Up | KeyCode::Char('k') => self.select(self.selected.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.select(self.selected.saturating_add(1)),
            KeyCode::PageUp => self.select(
                self.selected
                    .saturating_sub(usize::from(crate::tui::PAGE_ROWS)),
            ),
            KeyCode::PageDown => self.select(
                self.selected
                    .saturating_add(usize::from(crate::tui::PAGE_ROWS)),
            ),
            KeyCode::Home | KeyCode::Char('g') => self.select(0),
            KeyCode::End | KeyCode::Char('G') => self.select(usize::MAX),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => self.descend(),
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('b') => self.ascend(),
            KeyCode::Char('r') => self.begin_scan(),
            _ => return Intent::None,
        }
        Intent::Redraw
    }
}

/// Proportional bar in eighth-cell resolution.
pub(crate) fn bar(size: u64, largest: u64, width: usize) -> String {
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

/// Keeps the end of `text` within `max` columns, where a path's leaf is,
/// starting with `…` when anything was cut.
fn keep_end(text: &str, max: usize) -> String {
    if Span::raw(text).width() <= max {
        return text.to_string();
    }
    let mut kept: Vec<char> = Vec::new();
    let mut width = 1;
    for character in text.chars().rev() {
        let columns = Span::raw(character.to_string()).width();
        if width + columns > max {
            break;
        }
        width += columns;
        kept.push(character);
    }
    std::iter::once('…').chain(kept.into_iter().rev()).collect()
}

/// `mostly in <path> · <size> · <pct>%` for a folder whose culprit chain
/// exists, within `width` columns. The percentage is of the whole folder,
/// so a chain of halves reads as the share it really is.
fn culprit_line(entry: &Entry, width: usize) -> Option<String> {
    let culprit = entry.culprit.as_ref()?;
    let share = if entry.size == 0 {
        0
    } else {
        (u128::from(culprit.bytes) * 100 / u128::from(entry.size)).min(100)
    };
    let suffix = format!(
        " · {} · {share}%{}",
        report::gb(culprit.bytes),
        if culprit.lower_bound {
            " (lower bound)"
        } else {
            ""
        }
    );
    let mut relative = culprit
        .chain
        .iter()
        .map(|name| name.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if culprit.last_is_dir {
        relative.push('/');
    }
    let prefix = " mostly in ";
    let room =
        width.saturating_sub(Span::raw(prefix).width() + Span::raw(suffix.as_str()).width() + 1);
    Some(format!(
        "{prefix}{}{suffix} ",
        keep_end(&report::terminal_safe(&relative), room)
    ))
}

/// The keys the footer offers on this screen: the culprit key only under a
/// folder that has a chain, the filter only once verdicts are loaded.
fn footer_keys(explorer: &Explorer) -> Vec<(&'static str, &'static str)> {
    let mut keys = vec![("↑↓", "move"), ("Enter", "open"), ("Esc", "back")];
    if explorer
        .highlighted()
        .is_some_and(|entry| entry.is_dir && entry.culprit.is_some())
    {
        keys.push(("m", "culprit"));
    }
    if !explorer.verdicts_loading() {
        keys.push(("v", "verdicts"));
    }
    if matches!(explorer.verdicts, VerdictState::Loaded(_)) {
        keys.push(("f", if explorer.filter { "all" } else { "filter" }));
    }
    keys.extend([("r", "rescan"), ("q", "quit"), ("?", "keys")]);
    keys
}

/// The status row: this folder's errors first, then the answer to the last
/// key, the verdict scan's clock, the highlighted entry's verdict, and the
/// lower-bound note. Raw: the footer escapes at its sink.
fn status_line(explorer: &Explorer, partial: bool, shown_empty: bool) -> (String, Token) {
    match explorer.errors.as_slice() {
        [] => {}
        [only] => return (only.clone(), Token::Critical),
        [first, ..] => {
            return (
                format!("1 of {} errors: {first}", explorer.errors.len()),
                Token::Critical,
            );
        }
    }
    if let Some(notice) = &explorer.notice {
        return (notice.clone(), Token::Warning);
    }
    if let VerdictState::Loading { started, .. } = &explorer.verdicts {
        return (
            format!(
                "loading devtrim's verdicts… {}s",
                started.elapsed().as_secs()
            ),
            Token::Muted,
        );
    }
    if explorer.filtering() && shown_empty {
        return (
            "Filtered: nothing here is offered, holds an offer or lies inside one.".into(),
            Token::Muted,
        );
    }
    if let Some(verdict) = explorer
        .highlighted()
        .and_then(|entry| explorer.verdict(entry))
    {
        return (verdict.detail(), verdict.token());
    }
    if partial {
        return (
            "(partial) sizes are lower bounds; part was unreadable.".into(),
            Token::Muted,
        );
    }
    (
        "Read-only. This screen never deletes anything.".into(),
        Token::Muted,
    )
}

/// The verdict column, as wide as its longest tag plus a space.
const TAG_WIDTH: usize = 12;

fn render(frame: &mut Frame, explorer: &Explorer) {
    let area = frame.area();
    if crate::tui::terminal_too_small(area) {
        crate::tui::render_too_small(frame, area, explorer.theme);
        return;
    }
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .areas(area);

    let total = explorer.total();
    let partial = explorer.entries.iter().any(|entry| entry.partial);
    crate::tui::render_header(
        frame,
        header,
        explorer.theme,
        &format!(
            "analyze · {} · {}{}",
            explorer.current().display(),
            report::gb(total),
            if partial { "+" } else { "" }
        ),
    );

    let shown = explorer.shown();
    let tagged = matches!(explorer.verdicts, VerdictState::Loaded(_));
    let largest = explorer.entries.first().map_or(0, |entry| entry.size);
    let bar_width = usize::from(body.width)
        .saturating_sub(40 + if tagged { TAG_WIDTH } else { 0 })
        .clamp(0, 24);
    let visible_rows = usize::from(body.height.saturating_sub(2));
    let offset = explorer
        .selected
        .saturating_sub(visible_rows.saturating_sub(1));
    let items = shown.iter().skip(offset).take(visible_rows).map(|index| {
        let entry = &explorer.entries[*index];
        let mut spans = vec![
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
        ];
        if tagged {
            let verdict = explorer.verdict(entry);
            spans.push(Span::styled(
                format!(
                    "{:<width$}",
                    verdict.map(Verdict::tag).unwrap_or_default(),
                    width = TAG_WIDTH
                ),
                explorer
                    .theme
                    .bold(verdict.map_or(Token::Muted, Verdict::token)),
            ));
        }
        spans.extend([
            Span::raw(report::terminal_safe(&entry.name())),
            Span::raw(if entry.is_dir { "/" } else { "" }),
            Span::styled(
                if entry.partial { "  (partial)" } else { "" },
                explorer.theme.style(Token::Warning),
            ),
        ]);
        ListItem::new(Line::from(spans))
    });
    let mut state = ListState::default();
    state.select((!shown.is_empty()).then_some(explorer.selected - offset));
    let title = format!(
        " {}{} ",
        if explorer.scanning {
            "Measuring…"
        } else {
            "Contents"
        },
        if explorer.filtering() {
            " · filtered"
        } else {
            ""
        }
    );
    let mut block = Block::bordered().title(title);
    if let Some(line) = explorer
        .highlighted()
        .and_then(|entry| culprit_line(entry, usize::from(body.width.saturating_sub(2))))
    {
        block = block.title_bottom(Line::styled(line, explorer.theme.style(Token::Info)));
    }
    frame.render_stateful_widget(
        List::new(items)
            .block(block)
            .highlight_symbol("▶ ")
            .highlight_style(explorer.theme.bold(Token::Accent)),
        body,
        &mut state,
    );

    // The footer shows this folder's errors, the count saying the first is
    // not the only one; every error and lower bound of the session is
    // printed when analyze exits.
    let (status, token) = status_line(explorer, partial, shown.is_empty());
    crate::tui::render_footer_rows(
        frame,
        footer,
        explorer.theme,
        &footer_keys(explorer),
        (&status, token),
    );

    if explorer.help {
        render_help(frame, area, explorer);
    }
}

/// The key reference, as tall as its lines once wrapped to the popup's width,
/// so it neither leaves blank rows nor hides its last line.
fn render_help(frame: &mut Frame, area: Rect, explorer: &Explorer) {
    const WIDTH: u16 = 72;
    let inner = usize::from(area.width.saturating_sub(4).min(WIDTH).saturating_sub(2));
    // No heading: the border names the popup, and at 64×18 its fourteen
    // rows hold every key, a blank and the two-line warning exactly.
    let mut lines = Vec::new();
    for (key, description) in HELP_KEYS {
        lines.push(Line::from(vec![
            Span::styled(format!("  {key:<14}"), explorer.theme.style(Token::Muted)),
            Span::raw(*description),
        ]));
    }
    lines.push(Line::raw(""));
    for row in crate::tui::wrap_words(
        "Read-only: analyze never deletes. Use `devtrim clean <category>` for that.",
        inner,
    ) {
        lines.push(Line::styled(row, explorer.theme.style(Token::Warning)));
    }
    let popup = crate::tui::centered_rect(area, WIDTH, lines.len());
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(Block::bordered().title(" Help · ? or Esc closes ")),
        popup,
    );
}

const HELP_KEYS: &[(&str, &str)] = &[
    ("↑/↓, k/j", "move the cursor"),
    ("PgUp/PgDn", "move a page"),
    ("g/G, Home/End", "first or last entry"),
    ("Enter, →, l", "open the selected directory"),
    ("Esc, ←, h, b", "go up one level"),
    ("m", "open where most of the space is"),
    ("v", "load devtrim's verdicts (one scan)"),
    ("f", "only entries that are or hold offers"),
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

/// A lower bound is a partial measurement, and a partial operation is visible
/// in its status as well as in its entries (`CLAUDE.md § Conventions`).
fn lower_bound_errors(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| entry.partial)
        .map(|entry| {
            format!(
                "size of {} is a lower bound: part of it was unreadable or on another device",
                entry.path.display()
            )
        })
        .collect()
}

/// One-shot machine-readable breakdown of the requested directory.
fn run_json(ctx: &Ctx, requested: Option<&str>) -> Result<ExitCode> {
    let root = resolve_root(ctx, requested)?;
    let cancel = AtomicBool::new(false);
    let mut entries = Vec::new();
    let result = measure_children(&root, &cancel, |entry| entries.push(entry));
    let mut errors = match result {
        Ok(()) => Vec::new(),
        Err(error) => vec![format!("{error:#}")],
    };
    errors.extend(lower_bound_errors(&entries));
    entries.sort_by(|left, right| {
        right
            .size
            .cmp(&left.size)
            .then_with(|| left.path.cmp(&right.path))
    });
    let findings = entries
        .into_iter()
        .map(|entry| {
            Finding::new(
                if entry.is_dir { "directory" } else { "file" },
                Some(entry.path),
                entry.size,
                if entry.partial {
                    "report-only; lower bound, some entries below this path were unreadable"
                } else {
                    "report-only; estimated allocated bytes"
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
    // Before the terminal changes, so no signal can leave it in raw mode.
    let interrupts =
        crate::process::forward_interrupts().context("cannot install the interrupt handler")?;
    let mut terminal = ratatui::try_init().context("cannot initialize analyze terminal")?;
    let result = run_loop(&mut terminal, root, scan_context(ctx), &interrupts);
    let restore = ratatui::try_restore().context("cannot restore terminal after analyze exit");
    match (result, restore) {
        (Err(error), _) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Ok((code, warnings, loading)), Ok(())) => {
            // After the restore: written on the alternate screen, they would
            // vanish with it.
            for warning in warnings {
                ctx.diagnostic("warn", warning);
            }
            if loading {
                // A verdict scan still running started commands of its own
                // process groups; end them as an interrupted scan does, and
                // say so in the exit status. The exit skips the terminal's
                // drop, which is what shows the cursor again.
                crossterm::execute!(io::stdout(), crossterm::cursor::Show).ok();
                crate::process::kill_and_exit(signal_hook::consts::SIGINT);
            }
            Ok(code)
        }
    }
}

/// The exit status, the warnings to print once the terminal is restored, and
/// whether a verdict scan was still running.
fn run_loop(
    terminal: &mut DefaultTerminal,
    root: PathBuf,
    scan_context: Ctx,
    interrupts: &crate::process::Interrupts,
) -> Result<(ExitCode, Vec<String>, bool)> {
    let mut explorer = Explorer::new(root, Theme::from_env());
    explorer.scan_context = Some(Arc::new(scan_context));
    explorer.begin_scan();
    let mut redraw = true;
    let outcome = loop {
        if let Some(signal) = interrupts.received() {
            explorer.cancel.store(true, Ordering::Relaxed);
            crate::tui::restore_and_exit(signal);
        }
        redraw |= explorer.drain_progress();
        redraw |= explorer.poll_verdicts();
        if redraw {
            terminal.draw(|frame| render(frame, &explorer))?;
            redraw = false;
        }
        if event::poll(POLL_INTERVAL).context("cannot poll terminal input")? {
            match event::read().context("cannot read terminal input")? {
                Event::Key(key) => match explorer.handle_visible_key(
                    key,
                    terminal
                        .size()
                        .context("cannot inspect terminal size")?
                        .into(),
                ) {
                    Intent::Quit => break explorer.exit_report(),
                    Intent::Redraw => redraw = true,
                    Intent::None => {}
                },
                Event::Resize(_, _) => redraw = true,
                _ => {}
            }
        }
    };
    // Leave no worker walking a tree after the interface is gone.
    explorer.cancel.store(true, Ordering::Relaxed);
    let (failed, warnings) = outcome;
    let code = if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    };
    Ok((code, warnings, explorer.verdicts_loading()))
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

    /// The culprit by plain recursion, independent of the post-order walk:
    /// each folder's size, then its chain under the same rules.
    fn reference(path: &Path) -> (u64, Option<(Vec<OsString>, u64, bool)>) {
        let Ok(listing) = std::fs::read_dir(path) else {
            return (0, None);
        };
        let mut names: Vec<OsString> = listing.map(|child| child.unwrap().file_name()).collect();
        names.sort();
        let mut children = Vec::new();
        for name in names {
            let child = path.join(&name);
            let metadata = std::fs::symlink_metadata(&child).unwrap();
            if metadata.is_dir() {
                let (bytes, chain) = reference(&child);
                let repository = std::fs::read_dir(&child).is_ok_and(|mut listing| {
                    listing.any(|grandchild| {
                        crate::safety::is_git_metadata_name(&grandchild.unwrap().file_name())
                    })
                });
                let below = if repository || is_whole_unit(&name) {
                    None
                } else {
                    chain
                };
                children.push((bytes, name, true, below));
            } else if metadata.is_file() {
                let bytes = crate::safety::allocated_bytes(&metadata, &child).unwrap();
                children.push((bytes, name, false, None));
            }
        }
        let total: u64 = children.iter().map(|child| child.0).sum();
        // Larger first; names arrive sorted, so the first of equals stays.
        let largest = children
            .into_iter()
            .reduce(|best, child| if child.0 > best.0 { child } else { best });
        let chain = largest.and_then(|(bytes, name, is_dir, below)| {
            if bytes == 0 || bytes * 2 < total {
                return None;
            }
            Some(match below {
                Some((rest, last, last_is_dir)) => {
                    let mut chain = vec![name];
                    chain.extend(rest);
                    (chain, last, last_is_dir)
                }
                None => (vec![name], bytes, is_dir),
            })
        });
        (total, chain)
    }

    #[derive(Debug, Clone)]
    enum Spec {
        File(usize),
        Dir(Vec<(usize, Spec)>, bool),
    }

    /// Case-distinct names, so an APFS volume keeps them apart; `.git`
    /// makes its folder a repository and the next two are whole units.
    const NAMES: [&str; 7] = ["a", "b", "c", "node_modules", "Tool.app", ".git", "d"];
    const SIZES: [usize; 6] = [0, 1, 4096, 5000, 9000, 40_000];

    /// Writes `children` below `path`, recording the folders to lock.
    fn plant(path: &Path, children: &[(usize, Spec)], locked: &mut Vec<PathBuf>) {
        std::fs::create_dir_all(path).unwrap();
        let mut seen = std::collections::HashSet::new();
        for (name, spec) in children {
            if !seen.insert(*name) {
                continue;
            }
            let child = path.join(NAMES[*name]);
            match spec {
                Spec::File(size) => std::fs::write(&child, vec![b'x'; SIZES[*size]]).unwrap(),
                Spec::Dir(grandchildren, lock) => {
                    plant(&child, grandchildren, locked);
                    if *lock {
                        locked.push(child);
                    }
                }
            }
        }
    }

    /// Puts back the mode of every folder a case locked, even when an
    /// assertion fails, so the temporary tree can be removed.
    struct Unlock(Vec<PathBuf>);

    impl Drop for Unlock {
        fn drop(&mut self) {
            use std::os::unix::fs::PermissionsExt;
            for path in self.0.iter().rev() {
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
            }
        }
    }

    fn spec() -> impl proptest::strategy::Strategy<Value = Vec<(usize, Spec)>> {
        use proptest::prelude::*;
        let leaf = (0..SIZES.len()).prop_map(Spec::File);
        let tree = leaf.prop_recursive(4, 48, 5, |inner| {
            (
                proptest::collection::vec((0..NAMES.len(), inner), 0..5),
                proptest::bool::weighted(0.08),
            )
                .prop_map(|(children, lock)| Spec::Dir(children, lock))
        });
        proptest::collection::vec((0..NAMES.len(), tree), 0..6)
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(64))]
        /// The walk's chain equals the reference's on generated trees: ties,
        /// a dominant file, whole units, repositories and unreadable folders.
        #[test]
        fn the_culprit_chain_matches_a_brute_force_reference(children in spec()) {
            use std::os::unix::fs::PermissionsExt;
            let root = tempfile::Builder::new().prefix("devtrim-culprit").tempdir().unwrap();
            let mut locked = Vec::new();
            plant(root.path(), &children, &mut locked);
            for path in &locked {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
            }
            let _unlock = Unlock(locked.clone());
            let device = std::fs::symlink_metadata(root.path()).unwrap().dev();

            let (total, partial, culprit) =
                measure_tree(root.path(), device, &AtomicBool::new(false));
            let (expected_total, expected_chain) = reference(root.path());

            proptest::prop_assert_eq!(total, expected_total);
            proptest::prop_assert_eq!(partial, !locked.is_empty());
            let found = culprit
                .clone()
                .map(|culprit| (culprit.chain, culprit.bytes, culprit.last_is_dir));
            proptest::prop_assert_eq!(found, expected_chain);
            if let Some(culprit) = culprit {
                proptest::prop_assert_eq!(culprit.lower_bound, !locked.is_empty());
            }
        }
    }

    fn culprit_of(path: &Path) -> Option<Culprit> {
        let device = std::fs::symlink_metadata(path).unwrap().dev();
        measure_tree(path, device, &AtomicBool::new(false)).2
    }

    fn names(culprit: &Culprit) -> Vec<&str> {
        culprit
            .chain
            .iter()
            .map(|name| name.to_str().unwrap())
            .collect()
    }

    /// The chain follows the folders that hold most of the space, stops at
    /// an install, and may end at one dominant file.
    #[test]
    fn the_chain_stops_at_a_whole_unit_or_a_file() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-culprit-stop")
            .tempdir()
            .unwrap();
        write(&root.path().join("app/node_modules/pkg/big"), 200_000);
        write(&root.path().join("app/src/main.rs"), 4096);
        write(&root.path().join("media/clips/movie.mov"), 300_000);
        write(&root.path().join("media/clips/note.txt"), 10);
        let install = culprit_of(&root.path().join("app")).unwrap();
        assert_eq!(names(&install), ["node_modules"]);
        assert!(install.last_is_dir && !install.lower_bound);
        let movie = culprit_of(&root.path().join("media")).unwrap();
        assert_eq!(names(&movie), ["clips", "movie.mov"]);
        assert!(!movie.last_is_dir);
        // No child holds half: no chain at all.
        write(&root.path().join("even/one"), 40_000);
        write(&root.path().join("even/two"), 40_000);
        write(&root.path().join("even/three"), 40_000);
        assert_eq!(culprit_of(&root.path().join("even")), None);
    }

    /// A Git repository is one project: the chain names it and stops.
    #[test]
    fn the_chain_stops_at_a_repository() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-culprit-repo")
            .tempdir()
            .unwrap();
        write(&root.path().join("web/.git/HEAD"), 10);
        write(&root.path().join("web/node_modules/pkg/big"), 200_000);
        assert_eq!(names(&culprit_of(root.path()).unwrap()), ["web"]);
    }

    /// A hard-linked file counts once, in the first folder that reaches it,
    /// so the chain's sizes are lower bounds and say so.
    #[test]
    fn a_hard_link_makes_the_chain_a_lower_bound() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-culprit-link")
            .tempdir()
            .unwrap();
        write(&root.path().join("a/blob"), 200_000);
        std::fs::create_dir_all(root.path().join("b")).unwrap();
        std::fs::hard_link(root.path().join("a/blob"), root.path().join("b/blob")).unwrap();
        let device = std::fs::symlink_metadata(root.path()).unwrap().dev();
        let (total, partial, culprit) = measure_tree(root.path(), device, &AtomicBool::new(false));
        assert!(!partial);
        let culprit = culprit.unwrap();
        assert_eq!(culprit.bytes, total, "the blob counts once, in one folder");
        assert!(culprit.lower_bound);
        // Positive control: the same tree without the link is a measurement.
        std::fs::remove_file(root.path().join("b/blob")).unwrap();
        assert!(!culprit_of(root.path()).unwrap().lower_bound);
    }

    /// A cancelled walk claims no chain.
    #[test]
    fn a_cancelled_walk_has_no_culprit() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-culprit-cancel")
            .tempdir()
            .unwrap();
        write(&root.path().join("a/blob"), 200_000);
        let device = std::fs::symlink_metadata(root.path()).unwrap().dev();
        let (_, partial, culprit) = measure_tree(root.path(), device, &AtomicBool::new(true));
        assert!(partial);
        assert_eq!(culprit, None);
    }

    #[test]
    fn measure_children_handles_single_child_git_file_and_cancellation() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-analyze-single")
            .tempdir()
            .unwrap();
        write(&root.path().join("worktree/.git"), 16);
        write(&root.path().join("worktree/deep/blob"), 200_000);

        let mut entries = Vec::new();
        measure_children(root.path(), &AtomicBool::new(false), |entry| {
            entries.push(entry)
        })
        .unwrap();
        assert_eq!(entries.len(), 1);
        let worktree = &entries[0];
        assert_eq!(worktree.path, root.path().join("worktree"));
        let device = std::fs::symlink_metadata(root.path()).unwrap().dev();
        let (_, _, culprit) = measure_tree(root.path(), device, &AtomicBool::new(false));
        assert_eq!(names(&culprit.unwrap()), ["worktree"]);

        let mut cancelled_entries = Vec::new();
        measure_children(root.path(), &AtomicBool::new(true), |entry| {
            cancelled_entries.push(entry)
        })
        .unwrap();
        assert!(cancelled_entries.is_empty());
    }

    /// What would give analyze a way to remove something: an `Op`'s apply,
    /// the TUI's apply, the deletion sink and its validation, a typed
    /// command, raw removal, the Trash, and the registries that hand out
    /// `Op`s. Each is split so this file's own text never matches it.
    const APPLY_PATHS: [&str; 10] = [
        concat!(".app", "ly("),
        concat!("run_", "apply"),
        concat!("validate_path", "_for_deletion"),
        concat!("run_command", "_authority"),
        concat!("remove", "_dir"),
        concat!("remove", "_file"),
        concat!("Verified", "Target"),
        concat!("for_", "target("),
        concat!("ops::", "all("),
        concat!("trash", "::"),
    ];

    /// Verdicts never act: analyze's code names no path to an apply, the
    /// sink or a removal. The owners of those paths are the positive
    /// control: each needle is found where apply really lives.
    #[test]
    fn analyze_has_no_path_to_an_apply() {
        let source = include_str!("analyze.rs");
        let code = &source[..source.find("#[cfg(test)]\nmod tests").unwrap()];
        let owners = [include_str!("ops/mod.rs"), include_str!("tui.rs")].concat();
        for needle in APPLY_PATHS {
            assert!(!code.contains(needle), "analyze.rs names {needle:?}");
            assert!(owners.contains(needle), "control: {needle:?} found nowhere");
        }
    }

    /// A planted home outside the protected system temporary folder: a
    /// stale install, another category's permanent target, a protected
    /// folder, a configured protect entry, and a folder no root reaches.
    struct Planted {
        _fixture: crate::ops::TestFixture,
        home: PathBuf,
        verdicts: Verdicts,
    }

    fn planted(name: &str) -> Planted {
        let fixture = crate::ops::TestFixture::new(name);
        let root = fixture.path().to_path_buf();
        write(&root.join("dev/web/package.json"), 10);
        write(&root.join("dev/web/node_modules/pkg/index.js"), 4096);
        write(&root.join("dev/web/src/main.js"), 10);
        write(&root.join("dev/api/target/debug/out"), 8192);
        write(&root.join("dev/kept/notes"), 10);
        write(&root.join("Library/Keychains/login.keychain-db"), 10);
        write(&root.join("elsewhere/file"), 10);
        let home = root.canonicalize().unwrap();
        let dev = home.join("dev");
        let findings = vec![
            Finding::new(
                "node_modules",
                Some(dev.join("web/node_modules")),
                4096,
                "stale",
                3,
                Action::Trash,
            ),
            Finding::new(
                "target",
                Some(dev.join("api/target")),
                8192,
                "stale",
                9,
                Action::Shred,
            ),
            // Report-only: a place a category looked, never an offer.
            Finding::new(
                "note",
                Some(dev.join("api/README")),
                0,
                "report-only",
                1,
                Action::Info,
            ),
        ];
        let scan = crate::ops::ScanResult {
            findings,
            sections: vec![
                report::ScanSection {
                    category: "node-modules",
                    range: 0..1,
                },
                report::ScanSection {
                    category: "artifacts",
                    range: 1..3,
                },
            ],
            errors: Vec::new(),
        };
        let ctx = Ctx {
            roots: vec![dev.clone()],
            protect: vec![dev.join("kept")],
            ..Ctx::for_test(home.clone())
        };
        Planted {
            _fixture: fixture,
            home,
            verdicts: Verdicts::from_scan(&scan, &ctx),
        }
    }

    /// Each verdict from the planted scan, by explain's matching.
    #[test]
    fn each_entry_gets_the_verdict_its_findings_give_it() {
        let planted = planted("analyze-verdicts-1");
        let judge = |relative: &str| planted.verdicts.judge(&planted.home.join(relative));
        assert_eq!(
            judge("dev/web/node_modules"),
            Some(Verdict::Offered {
                action: "TRASH",
                command: "devtrim clean node-modules".into()
            })
        );
        assert_eq!(
            judge("dev/api/target"),
            Some(Verdict::Offered {
                action: "PERMANENT",
                command: "devtrim clean artifacts".into()
            })
        );
        assert_eq!(
            judge("dev/web"),
            Some(Verdict::Holds {
                count: 1,
                bytes: 4096,
                command: "devtrim clean node-modules".into()
            })
        );
        // Offers of two categories name the command that lists both.
        assert_eq!(
            judge("dev"),
            Some(Verdict::Holds {
                count: 2,
                bytes: 4096 + 8192,
                command: "devtrim scan".into()
            })
        );
        assert_eq!(
            judge("dev/web/node_modules/pkg"),
            Some(Verdict::InOffer {
                command: "devtrim clean node-modules".into()
            })
        );
        assert!(matches!(
            judge("Library/Keychains"),
            Some(Verdict::Protected(reason)) if reason.contains("~/Library")
        ));
        assert!(matches!(
            judge("dev/kept"),
            Some(Verdict::Protected(reason)) if reason.contains("configured protect entry")
        ));
        assert_eq!(judge("elsewhere"), Some(Verdict::NotOffered));
        // Scanned, not offered: inside a root, and beside a report-only
        // finding's folder, nothing is said.
        assert_eq!(judge("dev/web/src"), None);
        // On a case-insensitive volume another spelling is the same folder.
        if std::fs::symlink_metadata(planted.home.join("DEV/WEB/NODE_MODULES")).is_ok() {
            assert!(matches!(
                judge("dev/WEB/NODE_MODULES"),
                Some(Verdict::Offered { .. })
            ));
        }
    }

    /// Without an offer inside it, a protected folder reads as protected;
    /// holding an offer outranks that, as `~/Library` holds managed caches.
    #[test]
    fn holding_an_offer_outranks_protection() {
        let planted = planted("analyze-verdicts-2");
        let mut verdicts = planted.verdicts;
        let keychains = planted.home.join("Library/Keychains");
        verdicts.offers.push(Offer {
            target: Target::of(&keychains.join("login.keychain-db")),
            category: "caches",
            action: "TRASH",
            bytes: 10,
        });
        assert!(matches!(
            verdicts.judge(&keychains),
            Some(Verdict::Holds { count: 1, .. })
        ));
    }

    /// An explorer on the planted home with its verdicts loaded.
    fn loaded(planted: Planted, theme: Theme) -> (Explorer, crate::ops::TestFixture) {
        let Planted {
            _fixture,
            home,
            mut verdicts,
        } = planted;
        let mut explorer = Explorer::new(home.clone(), theme);
        explorer.entries = ["dev", "elsewhere", "Library"]
            .iter()
            .enumerate()
            .map(|(index, name)| Entry {
                path: home.join(name),
                size: 30_000 - index as u64 * 1000,
                is_dir: true,
                partial: false,
                culprit: None,
            })
            .collect();
        for entry in &explorer.entries {
            verdicts.remember(&entry.path);
        }
        explorer.verdicts = VerdictState::Loaded(verdicts);
        (explorer, _fixture)
    }

    /// Every tag reads as text, at the minimum and a wide size, and with
    /// `NO_COLOR` too, where no colour can tell them apart.
    #[test]
    fn each_tag_shows_in_text_at_both_sizes() {
        for theme in [
            Theme::new(crate::theme::ColorSupport::None),
            Theme::new(crate::theme::ColorSupport::Named),
        ] {
            let (mut explorer, _fixture) = loaded(planted("analyze-verdicts-3"), theme);
            for (width, height) in [(64, 18), (120, 40)] {
                let rows = screen(&explorer, width, height);
                let output = rows.concat();
                for tag in ["HOLDS 2", "NOT OFFERED", "PROTECTED"] {
                    assert!(output.contains(tag), "{tag} at {width}x{height}: {rows:#?}");
                }
                assert!(
                    rows[usize::from(height) - 1].contains("HOLDS 2 offers · ")
                        && rows[usize::from(height) - 1].contains("devtrim scan"),
                    "{rows:#?}"
                );
                assert!(
                    rows[usize::from(height) - 2].contains("f filter"),
                    "{rows:#?}"
                );
            }
            // The other states, on rows of their own.
            let home = explorer.stack[0].clone();
            explorer.entries = ["dev/web/node_modules", "dev/web/node_modules/pkg"]
                .iter()
                .map(|relative| Entry {
                    path: home.join(relative),
                    size: 4096,
                    is_dir: true,
                    partial: false,
                    culprit: None,
                })
                .collect();
            if let VerdictState::Loaded(verdicts) = &mut explorer.verdicts {
                for entry in &explorer.entries {
                    verdicts.remember(&entry.path);
                }
            }
            for (width, height) in [(64, 18), (120, 40)] {
                let rows = screen(&explorer, width, height);
                let output = rows.concat();
                assert!(output.contains("OFFERED"), "{rows:#?}");
                assert!(output.contains("IN OFFER"), "{rows:#?}");
                assert!(
                    rows[usize::from(height) - 1]
                        .contains("OFFERED · TRASH · devtrim clean node-modules"),
                    "{rows:#?}"
                );
            }
        }
    }

    /// The filter shows only entries that are or hold offers, says so in
    /// the title, and needs verdicts first; asking early is no error.
    #[test]
    fn the_filter_keeps_only_offers_and_needs_verdicts() {
        let mut early = sized(3);
        early.handle_key(key(KeyCode::Char('f')));
        assert!(early.errors.is_empty() && !early.filter);
        assert!(screen(&early, 64, 18).concat().contains("press v first"));
        assert!(!early.exit_report().0);

        let (mut explorer, _fixture) = loaded(planted("analyze-verdicts-4"), Theme::from_env());
        explorer.select(2);
        explorer.handle_key(key(KeyCode::Char('f')));
        assert_eq!(explorer.shown().len(), 1);
        assert!(explorer.highlighted().unwrap().path.ends_with("dev"));
        let output = screen(&explorer, 64, 18).concat();
        assert!(output.contains("Contents · filtered"), "{output}");
        assert!(!output.contains("NOT OFFERED"), "{output}");
        assert!(output.contains("f all"), "{output}");
        explorer.handle_key(key(KeyCode::Char('f')));
        assert_eq!(explorer.shown().len(), 3);
    }

    /// While the verdict scan runs, the status counts its seconds and the
    /// listing still moves.
    #[test]
    fn the_verdict_scan_shows_its_clock_and_navigation_keeps_working() {
        let mut explorer = sized(3);
        let (_sender, receiver) = channel::<(Verdicts, Vec<String>)>();
        explorer.verdicts = VerdictState::Loading {
            started: Instant::now() - Duration::from_secs(3),
            receiver,
        };
        for (width, height) in [(64, 18), (120, 40)] {
            let rows = screen(&explorer, width, height);
            // At least the three seconds already spent; a loaded machine may
            // add one between the two renders.
            let status = &rows[usize::from(height) - 1];
            let seconds = status
                .split("loading devtrim's verdicts… ")
                .nth(1)
                .and_then(|rest| rest.trim_end().strip_suffix('s'))
                .and_then(|seconds| seconds.parse::<u64>().ok());
            assert!(seconds.is_some_and(|seconds| seconds >= 3), "{rows:#?}");
            assert!(
                !rows[usize::from(height) - 2].contains("v verdicts"),
                "{rows:#?}"
            );
        }
        explorer.handle_key(key(KeyCode::Down));
        assert_eq!(explorer.selected, 1);
        explorer.handle_key(key(KeyCode::Char('v')));
        assert!(explorer.verdicts_loading());
        assert!(explorer.errors.is_empty());
    }

    /// A finished verdict scan judges the entries on screen, and its errors
    /// count toward the exit status as any partial operation's do.
    #[test]
    fn verdict_scan_errors_count_at_exit() {
        let planted = planted("analyze-verdicts-5");
        let (sender, receiver) = channel();
        let mut explorer = Explorer::new(planted.home.clone(), Theme::from_env());
        explorer.entries = vec![Entry {
            path: planted.home.join("elsewhere"),
            size: 1,
            is_dir: true,
            partial: false,
            culprit: None,
        }];
        explorer.verdicts = VerdictState::Loading {
            started: Instant::now(),
            receiver,
        };
        sender
            .send((planted.verdicts, vec!["docker: unreachable".into()]))
            .unwrap();
        assert!(explorer.poll_verdicts());
        assert_eq!(
            explorer.verdict(&explorer.entries[0].clone()),
            Some(&Verdict::NotOffered)
        );
        let (failed, warnings) = explorer.exit_report();
        assert!(failed);
        assert_eq!(warnings, ["verdicts: docker: unreachable"]);
    }

    /// The culprit line sits in the listing's bottom border, which the
    /// chrome test pins, and the footer offers the key, whole at 64 columns.
    #[test]
    fn the_culprit_line_shows_under_the_highlighted_folder() {
        let mut explorer = sized(2);
        explorer.entries[0].size = 100_000;
        explorer.entries[0].culprit = Some(Culprit {
            chain: vec!["app".into(), "node_modules".into()],
            bytes: 77_000,
            last_is_dir: true,
            lower_bound: false,
        });
        for (width, height) in [(64, 18), (120, 40)] {
            let rows = screen(&explorer, width, height);
            let border = &rows[usize::from(height) - 3];
            assert!(border.starts_with('└'), "{rows:#?}");
            assert!(
                border.contains("mostly in app/node_modules/ · ") && border.contains(" · 77%"),
                "{rows:#?}"
            );
            // At 64 columns the line keeps every key it cannot drop, whole;
            // wider, it keeps them all.
            let expected = if width == 64 {
                "Esc back · m culprit · v verdicts · q quit · ? keys"
            } else {
                "↑↓ move · Enter open · Esc back · m culprit · v verdicts · r rescan · q quit · ? keys"
            };
            assert!(
                rows[usize::from(height) - 2].contains(expected),
                "{rows:#?}"
            );
        }
        // A lower bound says so; a long chain keeps its leaf.
        explorer.entries[0].culprit = Some(Culprit {
            chain: vec!["a-very-long-directory-name".into(); 4],
            bytes: 77_000,
            last_is_dir: false,
            lower_bound: true,
        });
        let rows = screen(&explorer, 64, 18);
        assert!(
            rows[15].contains("…")
                && rows[15].contains("name · ")
                && rows[15].contains("(lower bound)"),
            "{rows:#?}"
        );
        // No chain, no line and no key.
        explorer.select(1);
        let rows = screen(&explorer, 64, 18);
        assert!(!rows[15].contains("mostly in"), "{rows:#?}");
        assert!(!rows[16].contains("m culprit"), "{rows:#?}");
    }

    /// `m` descends straight to the end of the chain, pushing every level,
    /// so going back climbs one level at a time and reselects as `Esc` does.
    #[test]
    fn the_culprit_key_descends_level_by_level() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-analyze-culprit-key")
            .tempdir()
            .unwrap();
        write(&root.path().join("big/a/b/movie.mov"), 300_000);
        write(&root.path().join("big/a/b/note"), 10);
        write(&root.path().join("small/data"), 4096);
        let mut explorer = Explorer::new(root.path().to_path_buf(), Theme::from_env());
        explorer.begin_scan();
        finish(&mut explorer);
        // The cursor stays on whichever entry arrived first; start at the top.
        explorer.handle_key(key(KeyCode::Home));
        assert!(explorer.highlighted().unwrap().path.ends_with("big"));

        explorer.handle_key(key(KeyCode::Char('m')));
        finish(&mut explorer);

        assert_eq!(
            explorer.stack,
            [
                root.path().to_path_buf(),
                root.path().join("big"),
                root.path().join("big/a"),
                root.path().join("big/a/b"),
            ]
        );
        assert!(explorer.highlighted().unwrap().path.ends_with("movie.mov"));
        explorer.handle_key(key(KeyCode::Esc));
        finish(&mut explorer);
        assert_eq!(explorer.current(), root.path().join("big/a"));
        assert!(explorer.highlighted().unwrap().path.ends_with("b"));

        // A file has no chain to follow: the answer is a notice, not an error.
        explorer.stack.truncate(1);
        explorer.begin_scan();
        finish(&mut explorer);
        let small = explorer
            .shown()
            .iter()
            .position(|index| explorer.entries[*index].path.ends_with("small"))
            .unwrap();
        explorer.select(small);
        explorer.entries.iter_mut().for_each(|entry| {
            if entry.path.ends_with("small") {
                entry.culprit = None;
            }
        });
        explorer.handle_key(key(KeyCode::Char('m')));
        assert_eq!(explorer.stack.len(), 1);
        assert!(explorer.errors.is_empty());
        assert!(
            explorer
                .notice
                .as_deref()
                .unwrap()
                .contains("no single folder")
        );
    }

    /// A level that is no longer a directory stops the descent there and is
    /// reported, as `descend` reports it.
    #[test]
    fn the_culprit_key_rechecks_every_level() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-analyze-culprit-recheck")
            .tempdir()
            .unwrap();
        write(&root.path().join("big/a/b/movie.mov"), 300_000);
        let mut explorer = Explorer::new(root.path().to_path_buf(), Theme::from_env());
        explorer.begin_scan();
        finish(&mut explorer);
        std::fs::rename(root.path().join("big/a"), root.path().join("moved")).unwrap();
        write(&root.path().join("big/a"), 10);

        explorer.handle_key(key(KeyCode::Char('m')));
        finish(&mut explorer);

        assert_eq!(explorer.current(), root.path().join("big"));
        assert!(
            explorer.errors[0].contains("no longer a directory"),
            "{:?}",
            explorer.errors
        );
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    fn finish(explorer: &mut Explorer) {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while explorer.scanning {
            assert!(
                std::time::Instant::now() < deadline,
                "the walk never finished"
            );
            explorer.drain_progress();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn screen(explorer: &Explorer, width: u16, height: u16) -> Vec<String> {
        let backend = ratatui::backend::TestBackend::new(width, height);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, explorer)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .chunks(usize::from(width))
            .map(|row| row.iter().map(|cell| cell.symbol()).collect())
            .collect()
    }

    fn sized(count: usize) -> Explorer {
        let mut explorer = Explorer::new(PathBuf::from("/fixture"), Theme::from_env());
        explorer.entries = (0..count)
            .map(|index| Entry {
                path: PathBuf::from(format!("/fixture/entry-{index:02}")),
                size: 100 - index as u64,
                is_dir: index % 2 == 0,
                partial: false,
                culprit: None,
            })
            .collect();
        explorer
    }

    /// Going up a level lands on the folder just left, so drilling into one
    /// subtree after another does not start from the top each time.
    #[test]
    fn going_up_reselects_the_folder_it_came_from() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-analyze-ascend")
            .tempdir()
            .unwrap();
        for (name, bytes) in [("large", 65536), ("medium", 16384), ("small", 4096)] {
            write(&root.path().join(name).join("data"), bytes);
        }
        let mut explorer = Explorer::new(root.path().to_path_buf(), Theme::from_env());
        explorer.begin_scan();
        finish(&mut explorer);
        explorer.handle_key(key(KeyCode::End));
        assert!(explorer.entries[explorer.selected].path.ends_with("small"));
        explorer.handle_key(key(KeyCode::Enter));
        finish(&mut explorer);

        explorer.handle_key(key(KeyCode::Esc));
        finish(&mut explorer);

        assert!(
            explorer.entries[explorer.selected].path.ends_with("small"),
            "selected {:?}",
            explorer.entries[explorer.selected].path
        );
    }

    #[test]
    fn paging_keys_move_as_they_do_in_the_tui() {
        let mut explorer = sized(30);
        explorer.handle_key(key(KeyCode::PageDown));
        assert_eq!(explorer.selected, 8);
        explorer.handle_key(key(KeyCode::Char('G')));
        assert_eq!(explorer.selected, 29);
        explorer.handle_key(key(KeyCode::PageUp));
        assert_eq!(explorer.selected, 21);
        explorer.handle_key(key(KeyCode::Char('g')));
        assert_eq!(explorer.selected, 0);
    }

    /// A folder reads as a folder: its name ends in a slash.
    #[test]
    fn a_folder_ends_in_a_slash() {
        let explorer = sized(2);
        let output = screen(&explorer, 64, 18).concat();
        assert!(output.contains("entry-00/"), "{output}");
        assert!(!output.contains("/entry-00"), "{output}");
        assert!(!output.contains("entry-01/"), "{output}");
    }

    /// More than one error says how many there are, not only the first.
    #[test]
    fn the_status_counts_every_error() {
        let mut explorer = sized(1);
        explorer.errors = vec!["first".into(), "second".into(), "third".into()];
        let output = screen(&explorer, 64, 18).concat();
        assert!(output.contains("1 of 3 errors: first"), "{output}");
    }

    /// Below the supported size analyze shows what the TUI shows, and takes
    /// only q, so a key cannot act on a screen that is not drawn.
    #[test]
    fn a_terminal_too_small_shows_the_resize_screen_and_takes_only_q() {
        let mut explorer = sized(3);
        explorer.stack.push(PathBuf::from("/fixture/entry-00"));
        let output = screen(&explorer, 50, 12).concat();
        assert!(output.contains("terminal too small"), "{output}");
        let small = Rect::new(0, 0, 50, 12);
        assert_eq!(
            explorer.handle_visible_key(key(KeyCode::Esc), small),
            Intent::None
        );
        assert_eq!(explorer.stack.len(), 2, "Esc went up a level unseen");
        assert_eq!(
            explorer.handle_visible_key(key(KeyCode::Char('q')), small),
            Intent::Quit
        );
    }

    /// A folder measured partially stays a failure after the operator moves
    /// on: the exit status and the warnings printed at exit cover the whole
    /// session, each folder once however often it was shown.
    #[test]
    fn every_partial_folder_of_the_session_is_reported_at_exit() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::Builder::new()
            .prefix("devtrim-analyze-session")
            .tempdir()
            .unwrap();
        write(&root.path().join("bad/data"), 4096);
        let locked = root.path().join("bad/locked");
        std::fs::create_dir_all(&locked).unwrap();
        write(&root.path().join("good/data"), 4096);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let mut explorer = Explorer::new(root.path().to_path_buf(), Theme::from_env());
        explorer.begin_scan();
        finish(&mut explorer);
        // Shown twice while still unreadable: it must be reported once.
        explorer.handle_key(key(KeyCode::Char('r')));
        finish(&mut explorer);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Positive control: the walk did reach the unreadable folder.
        assert!(
            explorer
                .entries
                .iter()
                .any(|entry| entry.path.ends_with("bad") && entry.partial),
            "{:?}",
            explorer.entries
        );
        let good = explorer
            .entries
            .iter()
            .position(|entry| entry.path.ends_with("good"))
            .unwrap();
        explorer.select(good);
        explorer.handle_key(key(KeyCode::Enter));
        finish(&mut explorer);
        assert!(explorer.entries.iter().all(|entry| !entry.partial));

        let (failed, warnings) = explorer.exit_report();

        assert!(failed, "{warnings:?}");
        let bad = root.path().join("bad").display().to_string();
        assert_eq!(
            warnings
                .iter()
                .filter(|warning| warning.contains(&format!("{bad} is a lower bound")))
                .count(),
            1,
            "{warnings:?}"
        );
    }

    /// The reference lists every key the explorer takes and still fits whole
    /// at the minimum size.
    #[test]
    fn the_help_lists_every_key_and_fits_at_the_minimum_size() {
        let mut explorer = sized(1);
        explorer.handle_key(key(KeyCode::Char('?')));
        let output = screen(&explorer, 64, 18).concat();
        for binding in [
            "PgUp/PgDn",
            "g/G",
            "Home/End",
            "Ctrl+C",
            "open where most of the space is",
            "load devtrim's verdicts (one scan)",
            "only entries that are or hold offers",
        ] {
            assert!(output.contains(binding), "{binding}: {output}");
        }
        assert!(output.contains("for that."), "{output}");
    }

    /// The header and footer match the TUI's: one row, then two.
    #[test]
    fn the_chrome_matches_the_tui() {
        let rows = screen(&sized(1), 64, 18);
        assert!(rows[0].contains("devtrim") && rows[0].contains("analyze"));
        assert!(rows[1].starts_with('┌'), "{rows:#?}");
        assert!(rows[15].starts_with('└'), "{rows:#?}");
        assert!(rows[16].contains("? keys"), "{rows:#?}");
        assert!(rows[16].contains("q quit"), "{rows:#?}");
    }

    /// REVIEW: docs say `f` "hides what nothing offers". Inside an offered
    /// target every entry is IN OFFER (it goes with the offer), yet the filter
    /// hides them all and the status says nothing here is offered.
    #[test]
    fn the_filter_keeps_entries_inside_an_offer() {
        let Planted {
            _fixture,
            home,
            mut verdicts,
        } = planted("analyze-review-inoffer");
        let install = home.join("dev/web/node_modules");
        let mut explorer = Explorer::new(install.clone(), Theme::from_env());
        explorer.entries = vec![Entry {
            path: install.join("pkg"),
            size: 4096,
            is_dir: true,
            partial: false,
            culprit: None,
        }];
        verdicts.remember(&install.join("pkg"));
        explorer.verdicts = VerdictState::Loaded(verdicts);
        assert!(matches!(
            explorer.verdict(&explorer.entries[0].clone()),
            Some(Verdict::InOffer { .. })
        ));
        explorer.handle_key(key(KeyCode::Char('f')));
        let output = screen(&explorer, 64, 18).concat();
        assert_eq!(
            explorer.shown().len(),
            1,
            "REVIEW: filter hid an IN OFFER entry: {output}"
        );
    }
}
