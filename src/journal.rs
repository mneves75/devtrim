//! Write-ahead apply journal and read-only history rendering.

use anyhow::{Context, Result};
use rustix::fs::{FlockOperation, Mode, OFlags, fchmod, flock, mkdirat, open, openat, renameat};
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs::{File, Permissions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::ops::project::iso_from_epoch_days;
use crate::safety::Ctx;

const MAX_JOURNAL_BYTES: u64 = 10 * 1024 * 1024;
const MAX_JOURNAL_RECORD_BYTES: usize = 64 * 1024;
const HISTORY_READ_CHUNK_BYTES: usize = 8 * 1024;
const KEEP_ROTATED: usize = 3;
const HISTORY_LOCK_RACE_RETRIES: usize = 3;
static JOURNAL_ID_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub(crate) struct JournalRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub ts: u64,
    pub phase: String,
    pub op: String,
    pub action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_lossy: Option<bool>,
    pub size_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl JournalRecord {
    pub(crate) fn filesystem_attempt(
        op: &str,
        action: &str,
        target: &Path,
        size_bytes: u64,
    ) -> Self {
        Self {
            id: Some(next_record_id()),
            ts: unix_secs(),
            phase: "attempt".into(),
            op: op.into(),
            action: action.into(),
            target: Some(target.to_string_lossy().into_owned()),
            target_lossy: target.to_str().is_none().then_some(true),
            size_bytes,
            argv: None,
            status: None,
            error: None,
        }
    }

    pub(crate) fn command_attempt(op: &str, program: &str, args: &[&str], size_bytes: u64) -> Self {
        let mut argv = Vec::with_capacity(args.len().saturating_add(1));
        argv.push(program.to_string());
        argv.extend(args.iter().map(|arg| (*arg).to_string()));
        Self {
            id: Some(next_record_id()),
            ts: unix_secs(),
            phase: "attempt".into(),
            op: op.into(),
            action: "command".into(),
            target: None,
            target_lossy: None,
            size_bytes,
            argv: Some(argv),
            status: None,
            error: None,
        }
    }

    fn result<T>(attempt: &Self, result: &Result<T>) -> Self {
        let mut record = attempt.clone();
        record.ts = unix_secs();
        record.phase = "result".into();
        match result {
            Ok(_) => record.status = Some("ok".into()),
            Err(error) => {
                record.status = Some("error".into());
                record.error = Some(format!("{error:#}"));
            }
        }
        record
    }
}

#[derive(Debug)]
pub(crate) struct History {
    pub entries: Vec<JournalRecord>,
    pub errors: Vec<String>,
}

#[must_use = "dropping a journal attempt leaves it recorded as interrupted"]
pub(crate) struct JournalAttempt {
    record: JournalRecord,
    location: JournalLocation,
    rotation_lock: File,
}

pub(crate) fn begin(ctx: &Ctx, record: JournalRecord) -> Result<JournalAttempt> {
    if record.phase != "attempt" || !valid_record(&record) {
        anyhow::bail!("refusing invalid apply journal attempt");
    }
    let location = JournalLocation::open(&ctx.journal_path, ParentMode::Create)?
        .ok_or_else(|| anyhow::anyhow!("journal parent unexpectedly disappeared"))?;
    let rotation_lock = acquire_apply_lock(&location)?;
    append_at(&location, &record)
        .with_context(|| format!("cannot write apply journal: {}", ctx.journal_path.display()))?;
    Ok(JournalAttempt {
        record,
        location,
        rotation_lock,
    })
}

impl JournalAttempt {
    pub(crate) fn finish<T>(self, ctx: &Ctx, result: Result<T>) -> Result<T> {
        let finished = finish_record_at(ctx, &self.location, &self.record, result);
        drop(self.rotation_lock);
        finished
    }
}

#[cfg(test)]
fn finish_record<T>(ctx: &Ctx, attempt: &JournalRecord, result: Result<T>) -> Result<T> {
    let location = match JournalLocation::open(&ctx.journal_path, ParentMode::Create) {
        Ok(Some(location)) => location,
        Ok(None) => {
            return preserve_operation_result(
                ctx,
                result,
                anyhow::anyhow!("journal parent unexpectedly disappeared"),
            );
        }
        Err(error) => return preserve_operation_result(ctx, result, error.into()),
    };
    finish_record_at(ctx, &location, attempt, result)
}

fn finish_record_at<T>(
    ctx: &Ctx,
    location: &JournalLocation,
    attempt: &JournalRecord,
    result: Result<T>,
) -> Result<T> {
    let record = JournalRecord::result(attempt, &result);
    if let Err(journal_error) = append_at(location, &record) {
        return preserve_operation_result(ctx, result, journal_error);
    }
    result
}

fn preserve_operation_result<T>(
    ctx: &Ctx,
    result: Result<T>,
    journal_error: anyhow::Error,
) -> Result<T> {
    // The mutation already happened, so the summary must stay truthful even
    // when the result record cannot be written; the attempt line remains and
    // history reports the operation as interrupted.
    match result {
        Ok(value) => {
            // Carried into the apply outcome's errors: the mutation stays
            // counted, the JSON errors array is non-empty, and the process
            // exits nonzero.
            ctx.record_journal_error(format!(
                "apply succeeded but the journal result could not be written: {journal_error:#}"
            ));
            Ok(value)
        }
        Err(operation_error) => Err(operation_error.context(format!(
            "journal result could not be written: {journal_error:#}"
        ))),
    }
}

#[cfg(test)]
fn append_to_path(path: &Path, record: &JournalRecord) -> Result<()> {
    let location = JournalLocation::open(path, ParentMode::Create)?
        .ok_or_else(|| anyhow::anyhow!("journal parent unexpectedly disappeared"))?;
    append_at(&location, record)
}

fn append_at(location: &JournalLocation, record: &JournalRecord) -> Result<()> {
    let mut line = serde_json::to_vec(record)?;
    if line.len() > MAX_JOURNAL_RECORD_BYTES {
        anyhow::bail!("apply journal record exceeds {MAX_JOURNAL_RECORD_BYTES} byte limit");
    }
    line.push(b'\n');
    let mut file = open_regular_at(
        &location.parent,
        &location.leaf,
        OFlags::WRONLY | OFlags::APPEND | OFlags::CREATE,
        Mode::from(0o600),
    )?;
    file.set_permissions(Permissions::from_mode(0o600))?;
    flock(&file, FlockOperation::LockExclusive).map_err(io::Error::from)?;
    file.write_all(&line)?;
    file.flush()?;
    file.sync_data()?;
    Ok(())
}

#[derive(Clone, Copy)]
enum ParentMode {
    Create,
    Existing,
}

struct JournalLocation {
    parent: File,
    leaf: OsString,
    path: PathBuf,
}

impl JournalLocation {
    fn open(path: &Path, mode: ParentMode) -> io::Result<Option<Self>> {
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "journal path must be absolute",
            ));
        }
        let leaf = match path.components().next_back() {
            Some(Component::Normal(leaf)) => leaf.to_os_string(),
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "journal path must end in a file name",
                ));
            }
        };
        let parent_path = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "journal path has no parent")
        })?;
        let mut parent =
            File::from(open("/", directory_open_flags(), Mode::empty()).map_err(io::Error::from)?);
        let mut opened_component = false;
        for component in parent_path.components() {
            match component {
                Component::RootDir => continue,
                Component::Normal(name) => {
                    let next = match open_directory_at(&parent, name) {
                        Ok(next) => next,
                        Err(error)
                            if error.kind() == io::ErrorKind::NotFound
                                && matches!(mode, ParentMode::Create) =>
                        {
                            match mkdirat(&parent, name, Mode::from(0o700)) {
                                Ok(()) => {}
                                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                                Err(error) => return Err(error.into()),
                            }
                            open_directory_at(&parent, name)?
                        }
                        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                        Err(error) => return Err(error),
                    };
                    parent = next;
                    opened_component = true;
                }
                Component::CurDir | Component::ParentDir | Component::Prefix(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "journal path contains an unsupported component",
                    ));
                }
            }
        }
        if matches!(mode, ParentMode::Create) && opened_component {
            fchmod(&parent, Mode::from(0o700)).map_err(io::Error::from)?;
        }
        Ok(Some(Self {
            parent,
            leaf,
            path: path.to_path_buf(),
        }))
    }

    fn rotated_leaf(&self, index: usize) -> OsString {
        let mut leaf = self.leaf.clone();
        leaf.push(format!(".{index}"));
        leaf
    }
}

fn directory_open_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC
}

fn open_directory_at(parent: &File, name: &OsStr) -> io::Result<File> {
    openat(parent, name, directory_open_flags(), Mode::empty())
        .map(File::from)
        .map_err(io::Error::from)
}

pub(crate) fn rotate_if_needed(path: &Path) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    let location = match JournalLocation::open(path, ParentMode::Existing)? {
        Some(location) => location,
        None => return Ok(warnings),
    };
    if journal_is_oversized(&location, &mut warnings) != Some(true) {
        return Ok(warnings);
    }

    // The lock file intentionally persists; flock ownership follows this fd.
    let _lock = match try_acquire_rotation_lock(&location) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            warnings.push("journal rotation skipped: journal.lock is already held".into());
            return Ok(warnings);
        }
        Err(error) => {
            warnings.push(format!(
                "journal rotation skipped: cannot acquire {}: {error}",
                location.path.with_file_name("journal.lock").display()
            ));
            return Ok(warnings);
        }
    };
    if journal_is_oversized(&location, &mut warnings) != Some(true) {
        return Ok(warnings);
    }

    for index in (1..KEEP_ROTATED).rev() {
        let source = location.rotated_leaf(index);
        let destination = location.rotated_leaf(index + 1);
        if !rename_rotation_source(&location, &source, &destination, &mut warnings) {
            return Ok(warnings);
        }
    }
    let destination = location.rotated_leaf(1);
    let source = location.leaf.clone();
    rename_rotation_source(&location, &source, &destination, &mut warnings);
    Ok(warnings)
}

fn journal_is_oversized(location: &JournalLocation, warnings: &mut Vec<String>) -> Option<bool> {
    match open_regular_at(
        &location.parent,
        &location.leaf,
        OFlags::RDONLY,
        Mode::empty(),
    ) {
        Ok(file) => match file.metadata() {
            Ok(metadata) => Some(metadata.len() > MAX_JOURNAL_BYTES),
            Err(error) => {
                warnings.push(format!(
                    "cannot inspect apply journal for rotation at {}: {error}",
                    location.path.display()
                ));
                None
            }
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => Some(false),
        Err(error) => {
            warnings.push(format!(
                "cannot inspect apply journal for rotation at {}: {error}",
                location.path.display()
            ));
            None
        }
    }
}

fn rename_rotation_source(
    location: &JournalLocation,
    source: &OsStr,
    destination: &OsStr,
    warnings: &mut Vec<String>,
) -> bool {
    let source_path = location.path.with_file_name(source);
    let destination_path = location.path.with_file_name(destination);
    match open_regular_at(&location.parent, source, OFlags::RDONLY, Mode::empty()) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return true,
        Err(error) => {
            warnings.push(format!(
                "journal rotation stopped while inspecting {}: {error}",
                source_path.display()
            ));
            return false;
        }
    }
    match open_regular_at(&location.parent, destination, OFlags::RDONLY, Mode::empty()) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            warnings.push(format!(
                "journal rotation stopped while inspecting {}: {error}",
                destination_path.display()
            ));
            return false;
        }
    }
    if let Err(error) = renameat(&location.parent, source, &location.parent, destination) {
        warnings.push(format!(
            "journal rotation stopped while renaming {} to {}: {error}",
            source_path.display(),
            destination_path.display()
        ));
        return false;
    }
    true
}

#[cfg(test)]
fn rotated_path(path: &Path, index: usize) -> PathBuf {
    let mut rotated = path.as_os_str().to_os_string();
    rotated.push(format!(".{index}"));
    PathBuf::from(rotated)
}

fn open_journal_lock(location: &JournalLocation, create: bool) -> io::Result<File> {
    let flags = if create {
        OFlags::RDWR | OFlags::CREATE
    } else {
        OFlags::RDONLY
    };
    open_regular_at(
        &location.parent,
        OsStr::new("journal.lock"),
        flags,
        Mode::from(0o600),
    )
}

fn open_history_generation(location: &JournalLocation, leaf: &OsStr) -> io::Result<File> {
    open_regular_at(&location.parent, leaf, OFlags::RDONLY, Mode::empty())
}

fn open_regular_at(parent: &File, leaf: &OsStr, flags: OFlags, mode: Mode) -> io::Result<File> {
    let open_file = || {
        openat(
            parent,
            leaf,
            flags | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            mode,
        )
        .map_err(io::Error::from)
    };
    let fd = match open_file() {
        Ok(fd) => fd,
        Err(error) if flags.contains(OFlags::CREATE) && error.kind() == io::ErrorKind::NotFound => {
            // Darwin can transiently report ENOENT when processes race to
            // create the same leaf. Retry only on the anchored parent fd;
            // a removed parent remains unavailable and still fails closed.
            std::thread::yield_now();
            open_file()?
        }
        Err(error) => return Err(error),
    };
    let file = File::from(fd);
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "journal path is not a regular file",
        ));
    }
    Ok(file)
}

fn try_acquire_rotation_lock(location: &JournalLocation) -> io::Result<Option<File>> {
    let file = open_journal_lock(location, true)?;
    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn acquire_apply_lock(location: &JournalLocation) -> Result<File> {
    let lock_path = location.path.with_file_name("journal.lock");
    let file = open_journal_lock(location, true)
        .with_context(|| format!("cannot open apply journal lock: {}", lock_path.display()))?;
    file.set_permissions(Permissions::from_mode(0o600))
        .with_context(|| format!("cannot secure apply journal lock: {}", lock_path.display()))?;
    flock(&file, FlockOperation::LockShared)
        .map_err(io::Error::from)
        .with_context(|| format!("cannot lock apply journal: {}", lock_path.display()))?;
    Ok(file)
}

fn acquire_history_lock(location: &JournalLocation) -> Result<Option<File>> {
    let lock_path = location.path.with_file_name("journal.lock");
    let file = match open_journal_lock(location, false) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "cannot open apply journal history lock: {}",
                    lock_path.display()
                )
            });
        }
    };
    flock(&file, FlockOperation::LockExclusive)
        .map_err(io::Error::from)
        .with_context(|| format!("cannot lock apply journal history: {}", lock_path.display()))?;
    Ok(Some(file))
}

pub(crate) fn read_history(path: &Path, limit: usize) -> Result<History> {
    read_history_with_lock_observer(path, limit, || {})
}

fn read_history_with_lock_observer(
    path: &Path,
    limit: usize,
    mut lock_acquired: impl FnMut(),
) -> Result<History> {
    let Some(location) = JournalLocation::open(path, ParentMode::Existing)? else {
        return Ok(History {
            entries: Vec::new(),
            errors: Vec::new(),
        });
    };

    let (history_lock, files) = {
        let mut snapshot = None;
        for attempt in 0..=HISTORY_LOCK_RACE_RETRIES {
            if let Some(lock) = acquire_history_lock(&location)? {
                lock_acquired();
                snapshot = Some((Some(lock), open_history_snapshot(&location)?));
                break;
            }

            let files = open_history_snapshot(&location)?;
            match open_journal_lock(&location, false) {
                Ok(lock) => {
                    drop(lock);
                    if attempt == HISTORY_LOCK_RACE_RETRIES {
                        anyhow::bail!(
                            "apply journal changed while history opened a read-only snapshot"
                        );
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    snapshot = Some((None, files));
                    break;
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "cannot inspect apply journal history lock: {}",
                            location.path.with_file_name("journal.lock").display()
                        )
                    });
                }
            }
        }
        snapshot.ok_or_else(|| anyhow::anyhow!("cannot stabilize apply journal history"))?
    };
    let history = parse_history_snapshot(files, limit);
    drop(history_lock);
    history
}

struct HistoryFile {
    display_path: PathBuf,
    file: File,
    snapshot_len: u64,
}

fn open_history_snapshot(location: &JournalLocation) -> Result<Vec<HistoryFile>> {
    let mut files = Vec::new();
    for index in 0..=KEEP_ROTATED {
        let leaf = if index == 0 {
            location.leaf.clone()
        } else {
            location.rotated_leaf(index)
        };
        let history_path = location.path.with_file_name(&leaf);
        match open_history_generation(location, &leaf) {
            Ok(file) => {
                flock(&file, FlockOperation::LockShared)
                    .map_err(io::Error::from)
                    .with_context(|| {
                        format!("cannot lock apply journal: {}", history_path.display())
                    })?;
                let metadata = file.metadata().with_context(|| {
                    format!("cannot inspect apply journal: {}", history_path.display())
                })?;
                let snapshot_len = metadata.len();
                files.push(HistoryFile {
                    display_path: history_path,
                    file,
                    snapshot_len,
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("cannot read apply journal: {}", history_path.display())
                });
            }
        }
    }
    Ok(files)
}

fn parse_history_snapshot(files: Vec<HistoryFile>, limit: usize) -> Result<History> {
    let limit = limit.clamp(1, 1000);
    let max_scanned_bytes = limit
        .saturating_mul(2)
        .saturating_add(1)
        .saturating_mul(MAX_JOURNAL_RECORD_BYTES.saturating_add(1));
    let mut scan_budget = max_scanned_bytes;
    let mut malformed = 0usize;
    let mut entries = Vec::with_capacity(limit);
    let mut fully_scanned = true;
    let mut pending_results = HashMap::new();
    for history_file in files {
        if entries.len() == limit {
            break;
        }
        if scan_budget == 0 && history_file.snapshot_len != 0 {
            fully_scanned = false;
            break;
        }
        if !scan_history_generation(
            history_file,
            limit,
            &mut scan_budget,
            &mut entries,
            &mut malformed,
            &mut pending_results,
        )? {
            fully_scanned = false;
            break;
        }
    }
    if entries.len() < limit && !fully_scanned {
        anyhow::bail!(
            "apply journal history exceeds the bounded {max_scanned_bytes} byte tail scan after {malformed} malformed line(s)"
        );
    }
    let errors = if malformed == 0 {
        Vec::new()
    } else {
        vec![format!(
            "skipped {malformed} malformed apply journal line(s)"
        )]
    };
    Ok(History { entries, errors })
}

fn scan_history_generation(
    mut history_file: HistoryFile,
    limit: usize,
    scan_budget: &mut usize,
    entries: &mut Vec<JournalRecord>,
    malformed: &mut usize,
    pending_results: &mut HashMap<AttemptKey, usize>,
) -> Result<bool> {
    let mut position = history_file.snapshot_len;
    let mut block = vec![0_u8; HISTORY_READ_CHUNK_BYTES];
    let mut reversed_line = Vec::with_capacity(1024);
    let mut at_file_end = true;
    while position != 0 && entries.len() < limit {
        if *scan_budget == 0 {
            return Ok(false);
        }
        let read_len = usize::try_from(position.min(HISTORY_READ_CHUNK_BYTES as u64))
            .unwrap_or(HISTORY_READ_CHUNK_BYTES)
            .min(*scan_budget);
        let start = position.saturating_sub(read_len as u64);
        history_file
            .file
            .seek(SeekFrom::Start(start))
            .with_context(|| {
                format!(
                    "cannot seek apply journal: {}",
                    history_file.display_path.display()
                )
            })?;
        history_file
            .file
            .read_exact(&mut block[..read_len])
            .with_context(|| {
                format!(
                    "cannot read apply journal: {}",
                    history_file.display_path.display()
                )
            })?;
        position = start;
        *scan_budget = scan_budget.saturating_sub(read_len);

        for &byte in block[..read_len].iter().rev() {
            if at_file_end && byte == b'\n' {
                at_file_end = false;
                continue;
            }
            at_file_end = false;
            if byte == b'\n' {
                process_reversed_history_line(
                    &mut reversed_line,
                    &history_file.display_path,
                    pending_results,
                    entries,
                    malformed,
                )?;
                if entries.len() == limit {
                    return Ok(true);
                }
            } else {
                reversed_line.push(byte);
                if reversed_line.len() > MAX_JOURNAL_RECORD_BYTES.saturating_add(1) {
                    anyhow::bail!(
                        "journal line exceeds {MAX_JOURNAL_RECORD_BYTES} byte limit at {}",
                        history_file.display_path.display()
                    );
                }
            }
        }
    }

    if position == 0 && history_file.snapshot_len != 0 && entries.len() < limit {
        process_reversed_history_line(
            &mut reversed_line,
            &history_file.display_path,
            pending_results,
            entries,
            malformed,
        )?;
    }
    Ok(position == 0)
}

fn process_reversed_history_line(
    reversed_line: &mut Vec<u8>,
    display_path: &Path,
    pending_results: &mut HashMap<AttemptKey, usize>,
    entries: &mut Vec<JournalRecord>,
    malformed: &mut usize,
) -> Result<()> {
    reversed_line.reverse();
    if reversed_line.last() == Some(&b'\r') {
        reversed_line.pop();
    }
    if reversed_line.len() > MAX_JOURNAL_RECORD_BYTES {
        anyhow::bail!(
            "journal line exceeds {MAX_JOURNAL_RECORD_BYTES} byte limit at {}",
            display_path.display()
        );
    }
    match serde_json::from_slice::<JournalRecord>(reversed_line) {
        Ok(mut record) if valid_record(&record) => {
            let key = AttemptKey::from(&record);
            if record.phase == "result" {
                let count = pending_results.entry(key).or_default();
                *count = count.saturating_add(1);
                entries.push(record);
            } else if let Some(count) = pending_results.get_mut(&key) {
                *count -= 1;
                if *count == 0 {
                    pending_results.remove(&key);
                }
            } else {
                record.status = Some("interrupted".into());
                entries.push(record);
            }
        }
        Ok(_) | Err(_) => *malformed = malformed.saturating_add(1),
    }
    reversed_line.clear();
    Ok(())
}

fn valid_record(record: &JournalRecord) -> bool {
    let id_valid = record.id.as_deref().is_none_or(|id| {
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    });
    if !id_valid {
        return false;
    }
    let action_valid = match record.action.as_str() {
        "trash" | "shred" => record.target.is_some() && record.argv.is_none(),
        "command" => record.target.is_none() && record.argv.as_ref().is_some_and(|v| !v.is_empty()),
        _ => false,
    };
    if !action_valid {
        return false;
    }
    match record.phase.as_str() {
        "attempt" => record.status.is_none() && record.error.is_none(),
        "result" => match record.status.as_deref() {
            Some("ok") => record.error.is_none(),
            Some("error") => record.error.is_some(),
            _ => false,
        },
        _ => false,
    }
}

#[derive(Debug, Hash, PartialEq, Eq)]
enum AttemptKey {
    Id(String),
    Legacy {
        op: String,
        action: String,
        target: Option<String>,
        target_lossy: Option<bool>,
        size_bytes: u64,
        argv: Option<Vec<String>>,
    },
}

impl From<&JournalRecord> for AttemptKey {
    fn from(record: &JournalRecord) -> Self {
        if let Some(id) = &record.id {
            return Self::Id(id.clone());
        }
        Self::Legacy {
            op: record.op.clone(),
            action: record.action.clone(),
            target: record.target.clone(),
            target_lossy: record.target_lossy,
            size_bytes: record.size_bytes,
            argv: record.argv.clone(),
        }
    }
}

pub(crate) fn print_human(history: &History) -> std::io::Result<()> {
    let mut output = String::new();
    if history.entries.is_empty() {
        output.push_str("no apply history\n");
        return crate::report::write_stdout(output.as_bytes());
    }
    for entry in &history.entries {
        let timestamp = format_utc_timestamp(entry.ts);
        let subject = entry.target.clone().unwrap_or_else(|| {
            entry
                .argv
                .as_ref()
                .map(|argv| argv.join(" "))
                .unwrap_or_else(|| "-".into())
        });
        let status = entry.status.as_deref().unwrap_or("interrupted");
        output.push_str(&format!(
            "{}  {}  {}  {}  {}\n",
            timestamp,
            crate::report::terminal_safe(&entry.op),
            crate::report::terminal_safe(&entry.action),
            crate::report::terminal_safe(&subject),
            crate::report::terminal_safe(status)
        ));
        if let Some(error) = &entry.error {
            output.push_str(&format!("    {}\n", crate::report::terminal_safe(error)));
        }
    }
    crate::report::write_stdout(output.as_bytes())
}

fn format_utc_timestamp(ts: u64) -> String {
    let seconds_per_day = 86_400;
    let day_seconds = ts % seconds_per_day;
    let hours = day_seconds / 3_600;
    let minutes = day_seconds % 3_600 / 60;
    let seconds = day_seconds % 60;
    format!(
        "{} {hours:02}:{minutes:02}:{seconds:02}",
        iso_from_epoch_days(ts / seconds_per_day)
    )
}

#[derive(serde::Serialize)]
struct HistoryResponse<'a> {
    operation: &'static str,
    entries: &'a [JournalRecord],
    errors: &'a [String],
}

pub(crate) fn print_json(history: &History) -> std::io::Result<()> {
    let response = HistoryResponse {
        operation: "history",
        entries: &history.entries,
        errors: &history.errors,
    };
    let mut output = serde_json::to_string_pretty(&response)
        .unwrap_or_else(|_| {
            r#"{"operation":"history","entries":[],"errors":["serialization failed"]}"#.into()
        })
        .into_bytes();
    output.push(b'\n');
    crate::report::write_stdout(&output)
}

fn next_record_id() -> String {
    use std::hash::{BuildHasher, Hasher};

    let sequence = JOURNAL_ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let random = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "{nanos:032x}-{:08x}-{random:016x}-{sequence:016x}",
        std::process::id()
    )
}

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom};
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("devtrim-journal-{name}-{}", std::process::id()));
        crate::ops::remove_test_path(&path);
        std::fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }

    fn write_oversized(path: &Path) {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(path)
            .unwrap();
        file.write_all(b"oversized current journal").unwrap();
        file.set_len(MAX_JOURNAL_BYTES + 1).unwrap();
    }

    #[test]
    fn formats_epoch_seconds_as_utc() {
        assert_eq!(format_utc_timestamp(0), "1970-01-01 00:00:00");
        assert_eq!(format_utc_timestamp(1_704_114_309), "2024-01-01 13:05:09");
    }

    #[test]
    fn new_attempts_have_unique_ids_and_results_keep_them() {
        let first =
            JournalRecord::filesystem_attempt("caches", "trash", Path::new("/tmp/cache"), 4);
        let second =
            JournalRecord::filesystem_attempt("caches", "trash", Path::new("/tmp/cache"), 4);
        let result = JournalRecord::result(&first, &Ok(()));

        assert!(first.id.as_ref().is_some_and(|id| !id.is_empty()));
        assert_ne!(first.id, second.id);
        assert_eq!(result.id, first.id);
    }

    #[test]
    fn legacy_duplicate_results_pair_with_the_nearest_attempt() {
        let root = temp("legacy-nearest");
        let path = root.join("journal.jsonl");
        let mut oldest =
            JournalRecord::filesystem_attempt("caches", "trash", Path::new("/tmp/cache"), 4);
        oldest.id = None;
        oldest.ts = 1;
        let mut nearest = oldest.clone();
        nearest.ts = 2;
        let mut result = JournalRecord::result(&nearest, &Ok(()));
        result.ts = 3;
        append_to_path(&path, &oldest).unwrap();
        append_to_path(&path, &nearest).unwrap();
        append_to_path(&path, &result).unwrap();

        let history = read_history(&path, 20).unwrap();

        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].ts, 3);
        assert_eq!(history.entries[0].status.as_deref(), Some("ok"));
        assert_eq!(history.entries[1].ts, 1);
        assert_eq!(history.entries[1].status.as_deref(), Some("interrupted"));
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn history_reads_the_newest_entry_from_each_oversized_sparse_generation() {
        for generation in 0..=KEEP_ROTATED {
            let root = temp(&format!("history-generation-{generation}"));
            let path = root.join("journal.jsonl");
            let generation_path = if generation == 0 {
                path.clone()
            } else {
                rotated_path(&path, generation)
            };
            let mut file = File::create(&generation_path).unwrap();
            file.set_len(20 * 1024 * 1024).unwrap();
            file.seek(SeekFrom::End(0)).unwrap();
            file.write_all(b"\n").unwrap();
            drop(file);
            let attempt =
                JournalRecord::filesystem_attempt("caches", "trash", Path::new("/tmp/newest"), 4);
            append_to_path(&generation_path, &attempt).unwrap();
            append_to_path(&generation_path, &JournalRecord::result(&attempt, &Ok(()))).unwrap();

            let history = read_history(&path, 1).unwrap();

            assert!(history.errors.is_empty());
            assert_eq!(history.entries.len(), 1);
            assert_eq!(history.entries[0].target.as_deref(), Some("/tmp/newest"));
            assert_eq!(history.entries[0].status.as_deref(), Some("ok"));
            crate::ops::remove_test_path(root);
        }
    }

    #[test]
    fn history_rejects_an_oversized_line() {
        let root = temp("history-line-limit");
        let path = root.join("journal.jsonl");
        let mut file = File::create(&path).unwrap();
        file.write_all(&vec![b'x'; MAX_JOURNAL_RECORD_BYTES + 1])
            .unwrap();
        file.write_all(b"\n").unwrap();

        let error = read_history(&path, 20).unwrap_err();

        assert!(error.to_string().contains("journal line exceeds"));
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn history_ignores_more_than_100k_old_lines_when_the_tail_satisfies_the_limit() {
        let root = temp("history-record-limit");
        let path = root.join("journal.jsonl");
        std::fs::write(&path, vec![b'\n'; 100_001]).unwrap();
        let attempt =
            JournalRecord::filesystem_attempt("caches", "trash", Path::new("/tmp/newest"), 4);
        append_to_path(&path, &attempt).unwrap();
        append_to_path(&path, &JournalRecord::result(&attempt, &Ok(()))).unwrap();

        let history = read_history(&path, 1).unwrap();

        assert!(history.errors.is_empty());
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].target.as_deref(), Some("/tmp/newest"));
        assert_eq!(history.entries[0].status.as_deref(), Some("ok"));
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn malformed_tiny_lines_cannot_force_an_unbounded_tail_scan() {
        let root = temp("history-byte-limit");
        let path = root.join("journal.jsonl");
        let max_scanned_bytes = 3 * (MAX_JOURNAL_RECORD_BYTES + 1);
        std::fs::write(&path, vec![b'\n'; max_scanned_bytes + 1]).unwrap();

        let error = read_history(&path, 1).unwrap_err();

        assert!(error.to_string().contains("bounded"));
        assert!(error.to_string().contains("malformed line"));
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn journal_paths_never_follow_symlinks() {
        let root = temp("nofollow");
        let path = root.join("journal.jsonl");
        let lock_path = root.join("journal.lock");
        let target = root.join("unrelated-file");
        std::fs::write(&target, "unchanged").unwrap();
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: path.clone(),
            home: root.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };

        symlink(&target, &path).unwrap();
        let record =
            JournalRecord::filesystem_attempt("caches", "trash", Path::new("/tmp/cache"), 4);
        assert!(begin(&ctx, record).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "unchanged");
        crate::ops::remove_test_path(&path);

        crate::ops::remove_test_path(&lock_path);
        symlink(&target, &lock_path).unwrap();
        let record =
            JournalRecord::filesystem_attempt("caches", "trash", Path::new("/tmp/cache"), 4);
        assert!(begin(&ctx, record).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "unchanged");
        crate::ops::remove_test_path(&lock_path);

        symlink(&target, &path).unwrap();
        assert!(read_history(&path, 20).is_err());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "unchanged");
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn journal_parent_components_never_follow_symlinks() {
        let root = temp("nofollow-parent");
        let actual_state = root.join("actual/devtrim");
        std::fs::create_dir_all(&actual_state).unwrap();
        std::fs::set_permissions(&actual_state, std::fs::Permissions::from_mode(0o755)).unwrap();
        let alias = root.join("state-alias");
        symlink(root.join("actual"), &alias).unwrap();
        let path = alias.join("devtrim/journal.jsonl");
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: path.clone(),
            home: root.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };
        let record =
            JournalRecord::filesystem_attempt("caches", "trash", Path::new("/tmp/cache"), 4);

        assert!(begin(&ctx, record).is_err());
        assert!(read_history(&path, 20).is_err());
        assert!(!actual_state.join("journal.jsonl").exists());
        assert!(!actual_state.join("journal.lock").exists());
        assert_eq!(
            std::fs::metadata(&actual_state)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn history_does_not_create_a_lock_file() {
        let root = temp("history-read-only");
        let path = root.join("journal.jsonl");
        let record =
            JournalRecord::filesystem_attempt("caches", "trash", Path::new("/tmp/cache"), 4);
        append_to_path(&path, &record).unwrap();
        let lock_path = root.join("journal.lock");
        assert!(!lock_path.exists());

        let history = read_history(&path, 20).unwrap();

        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].status.as_deref(), Some("interrupted"));
        assert!(!lock_path.exists());
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn history_waits_for_an_active_attempt_before_snapshotting() {
        let root = temp("history-active-attempt");
        let path = root.join("journal.jsonl");
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: path.clone(),
            home: root.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };
        let record =
            JournalRecord::filesystem_attempt("caches", "trash", Path::new("/tmp/cache"), 4);
        let attempt = begin(&ctx, record).unwrap();
        let reader_path = path.clone();
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
        let (locked_tx, locked_rx) = std::sync::mpsc::sync_channel(0);
        let reader = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            read_history_with_lock_observer(&reader_path, 20, || {
                locked_tx.send(()).unwrap();
            })
        });

        started_rx.recv().unwrap();
        assert!(matches!(
            locked_rx.recv_timeout(std::time::Duration::from_secs(1)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout)
        ));

        attempt.finish(&ctx, Ok(())).unwrap();
        locked_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        let history = reader.join().unwrap().unwrap();

        assert!(history.errors.is_empty());
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].phase, "result");
        assert_eq!(history.entries[0].status.as_deref(), Some("ok"));
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn orphan_attempt_is_interrupted_and_malformed_lines_are_aggregated() {
        let root = temp("history");
        let path = root.join("journal.jsonl");
        let attempt =
            JournalRecord::filesystem_attempt("caches", "trash", Path::new("/tmp/cache"), 4);
        append_to_path(&path, &attempt).unwrap();
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"not-json\n{\"phase\":\"result\"}\n")
            .unwrap();

        let history = read_history(&path, 20).unwrap();

        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].status.as_deref(), Some("interrupted"));
        assert_eq!(history.errors.len(), 1);
        assert!(history.errors[0].contains("2 malformed"));
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn finish_keeps_successful_result_when_journal_write_fails() {
        use std::os::unix::fs::PermissionsExt;
        let root = temp("finish-unwritable");
        let path = root.join("journal.jsonl");
        let attempt =
            JournalRecord::filesystem_attempt("caches", "trash", Path::new("/tmp/cache"), 4);
        append_to_path(&path, &attempt).unwrap();
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: path.clone(),
            home: root.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400)).unwrap();

        // The deletion already happened; the summary must stay truthful.
        let preserved = finish_record(&ctx, &attempt, Ok("trashed".to_string()));
        assert_eq!(preserved.unwrap(), "trashed");
        assert!(
            ctx.take_journal_errors()
                .iter()
                .any(|message| message.contains("journal result could not be written"))
        );

        // A failed operation keeps its own error, with the journal failure attached.
        let failed = finish_record::<()>(&ctx, &attempt, Err(anyhow::anyhow!("removal failed")));
        let message = format!("{:#}", failed.unwrap_err());
        assert!(message.contains("removal failed"));
        assert!(message.contains("journal result could not be written"));

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn history_returns_newest_results_and_orphans_first_with_limit() {
        let root = temp("order");
        let path = root.join("journal.jsonl");
        let completed =
            JournalRecord::filesystem_attempt("caches", "trash", Path::new("/tmp/first"), 1);
        append_to_path(&path, &completed).unwrap();
        append_to_path(&path, &JournalRecord::result(&completed, &Ok(()))).unwrap();
        let orphan =
            JournalRecord::filesystem_attempt("xcode", "shred", Path::new("/tmp/second"), 2);
        append_to_path(&path, &orphan).unwrap();

        let history = read_history(&path, 20).unwrap();
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].target.as_deref(), Some("/tmp/second"));
        assert_eq!(history.entries[0].status.as_deref(), Some("interrupted"));
        assert_eq!(history.entries[1].target.as_deref(), Some("/tmp/first"));
        assert_eq!(history.entries[1].status.as_deref(), Some("ok"));

        let limited = read_history(&path, 1).unwrap();
        assert_eq!(limited.entries.len(), 1);
        assert_eq!(limited.entries[0].target.as_deref(), Some("/tmp/second"));
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn rotation_shifts_files_and_clobbers_the_oldest_generation() {
        let root = temp("rotation-shift");
        let path = root.join("journal.jsonl");
        write_oversized(&path);
        std::fs::write(rotated_path(&path, 1), "generation one").unwrap();
        std::fs::write(rotated_path(&path, 2), "generation two").unwrap();
        std::fs::write(rotated_path(&path, 3), "old generation three").unwrap();

        let warnings = rotate_if_needed(&path).unwrap();

        assert!(warnings.is_empty());
        assert!(!path.exists());
        assert_eq!(
            std::fs::metadata(rotated_path(&path, 1)).unwrap().len(),
            MAX_JOURNAL_BYTES + 1
        );
        assert_eq!(
            std::fs::read_to_string(rotated_path(&path, 2)).unwrap(),
            "generation one"
        );
        assert_eq!(
            std::fs::read_to_string(rotated_path(&path, 3)).unwrap(),
            "generation two"
        );
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn held_lock_skips_rotation_with_a_warning() {
        let root = temp("rotation-held-lock");
        let path = root.join("journal.jsonl");
        write_oversized(&path);
        let location = JournalLocation::open(&path, ParentMode::Existing)
            .unwrap()
            .unwrap();
        let held_lock = open_journal_lock(&location, true).unwrap();
        flock(&held_lock, FlockOperation::LockExclusive).unwrap();

        let warnings = rotate_if_needed(&path).unwrap();

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("already held"));
        assert!(path.exists());
        assert!(!rotated_path(&path, 1).exists());
        drop(held_lock);
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn persistent_unlocked_lock_file_does_not_block_rotation() {
        let root = temp("rotation-persistent-lock-file");
        let path = root.join("journal.jsonl");
        let lock_path = root.join("journal.lock");
        write_oversized(&path);
        let location = JournalLocation::open(&path, ParentMode::Existing)
            .unwrap()
            .unwrap();
        drop(open_journal_lock(&location, true).unwrap());

        let warnings = rotate_if_needed(&path).unwrap();

        assert!(warnings.is_empty());
        assert!(rotated_path(&path, 1).exists());
        assert!(lock_path.exists());
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn history_pairs_records_across_rotation_generations() {
        let root = temp("history-rotation-pair");
        let path = root.join("journal.jsonl");
        let attempt =
            JournalRecord::filesystem_attempt("caches", "trash", Path::new("/tmp/cache"), 4);
        append_to_path(&rotated_path(&path, 1), &attempt).unwrap();
        append_to_path(&path, &JournalRecord::result(&attempt, &Ok(()))).unwrap();

        let history = read_history(&path, 20).unwrap();

        assert!(history.errors.is_empty());
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].phase, "result");
        assert_eq!(history.entries[0].status.as_deref(), Some("ok"));
        crate::ops::remove_test_path(root);
    }

    #[test]
    fn attempt_guard_keeps_a_pair_in_one_rotation_generation() {
        let root = temp("guarded-rotation");
        let path = root.join("journal.jsonl");
        let mut oversized = File::create(&path).unwrap();
        oversized.set_len(MAX_JOURNAL_BYTES + 1).unwrap();
        oversized.seek(SeekFrom::End(0)).unwrap();
        oversized.write_all(b"\n").unwrap();
        drop(oversized);
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: path.clone(),
            home: root.clone(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };
        let record =
            JournalRecord::filesystem_attempt("caches", "trash", Path::new("/tmp/cache"), 4);
        let attempt = begin(&ctx, record).unwrap();
        let expected_id = attempt.record.id.clone();

        let warnings = rotate_if_needed(&path).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("already held"));
        assert!(path.exists());
        assert!(!rotated_path(&path, 1).exists());

        attempt.finish(&ctx, Ok(())).unwrap();
        assert!(rotate_if_needed(&path).unwrap().is_empty());
        assert!(!path.exists());

        let mut rotated = File::open(rotated_path(&path, 1)).unwrap();
        let len = rotated.metadata().unwrap().len();
        rotated
            .seek(SeekFrom::Start(len.saturating_sub(4096)))
            .unwrap();
        let mut tail = String::new();
        rotated.read_to_string(&mut tail).unwrap();
        let records = tail
            .lines()
            .filter_map(|line| serde_json::from_str::<JournalRecord>(line).ok())
            .collect::<Vec<_>>();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].phase, "attempt");
        assert_eq!(records[1].phase, "result");
        assert_eq!(records[0].id, expected_id);
        assert_eq!(records[1].id, expected_id);
        crate::ops::remove_test_path(root);
    }

    fn append_writer_records(path: &Path, writer: &str) {
        let ctx = Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: path.to_path_buf(),
            home: path.parent().unwrap().to_path_buf(),
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        };
        for sequence in 0..50 {
            let target = PathBuf::from(format!("/tmp/writer-{writer}-{sequence}"));
            let record = JournalRecord::filesystem_attempt("caches", "trash", &target, 4);
            let attempt = begin(&ctx, record).unwrap_or_else(|error| {
                panic!(
                    "writer {writer} pid {} attempt {sequence} begin at {} failed: {error:#}; parent_exists={}; journal_exists={}",
                    std::process::id(),
                    path.display(),
                    path.parent().is_some_and(Path::exists),
                    path.exists()
                )
            });
            std::thread::yield_now();
            attempt.finish(&ctx, Ok(())).unwrap_or_else(|error| {
                panic!(
                    "writer {writer} pid {} result {sequence} finish at {} failed: {error:#}; parent_exists={}; journal_exists={}",
                    std::process::id(),
                    path.display(),
                    path.parent().is_some_and(Path::exists),
                    path.exists()
                )
            });
        }
    }

    #[test]
    #[ignore = "helper process for concurrent_writers_preserve_every_record"]
    fn concurrent_writer_child() {
        let Some(path) = std::env::var_os("DEVTRIM_TEST_JOURNAL_PATH") else {
            return;
        };
        append_writer_records(&PathBuf::from(path), "child");
    }

    #[test]
    fn concurrent_writers_preserve_every_record() {
        let root = tempfile::Builder::new()
            .prefix("devtrim-journal-concurrent-writers-")
            .tempdir()
            .unwrap();
        let path = root.path().canonicalize().unwrap().join("journal.jsonl");
        let executable = std::env::current_exe().unwrap();
        let child = std::process::Command::new(&executable)
            .arg("--exact")
            .arg("journal::tests::concurrent_writer_child")
            .arg("--ignored")
            .arg("--nocapture")
            .env("DEVTRIM_TEST_JOURNAL_PATH", &path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        append_writer_records(&path, "parent");
        let child_output = child.wait_with_output().unwrap();
        assert!(
            child_output.status.success(),
            "child writer failed with {}\nstdout:\n{}\nstderr:\n{}",
            child_output.status,
            String::from_utf8_lossy(&child_output.stdout),
            String::from_utf8_lossy(&child_output.stderr)
        );

        let contents = std::fs::read_to_string(&path).unwrap();
        let lines = contents.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 200);
        assert!(
            lines
                .iter()
                .all(|line| serde_json::from_str::<JournalRecord>(line).is_ok())
        );
        let history = read_history(&path, 1000).unwrap();
        assert!(history.errors.is_empty());
        assert_eq!(history.entries.len(), 100);
        assert!(
            history
                .entries
                .iter()
                .all(|record| record.status.as_deref() == Some("ok"))
        );
    }
}
