//! Write-ahead apply journal and read-only history rendering.

use anyhow::{Context, Result};
use std::fs::{OpenOptions, Permissions};
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

use crate::ops::project::iso_from_epoch_days;
use crate::safety::Ctx;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
pub(crate) struct JournalRecord {
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

pub(crate) fn append(ctx: &Ctx, record: &JournalRecord) -> Result<()> {
    append_to_path(&ctx.journal_path, record)
        .with_context(|| format!("cannot write apply journal: {}", ctx.journal_path.display()))
}

pub(crate) fn finish<T>(ctx: &Ctx, attempt: &JournalRecord, result: Result<T>) -> Result<T> {
    let record = JournalRecord::result(attempt, &result);
    if let Err(journal_error) = append(ctx, &record) {
        // The mutation already happened, so the summary must stay truthful even
        // when the result record cannot be written; the attempt line remains
        // and history reports the operation as interrupted.
        return match result {
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
        };
    }
    result
}

fn append_to_path(path: &Path, record: &JournalRecord) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("journal path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    std::fs::set_permissions(parent, Permissions::from_mode(0o700))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(Permissions::from_mode(0o600))?;
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    file.write_all(&line)?;
    file.flush()?;
    Ok(())
}

pub(crate) fn read_history(path: &Path, limit: usize) -> Result<History> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(History {
                entries: Vec::new(),
                errors: Vec::new(),
            });
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("cannot read apply journal: {}", path.display()));
        }
    };
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)
        .with_context(|| format!("cannot read apply journal: {}", path.display()))?;

    let mut malformed = 0usize;
    let mut records = Vec::new();
    let lines = contents.split(|byte| *byte == b'\n').collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if line.is_empty() && index == lines.len().saturating_sub(1) {
            continue;
        }
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        match serde_json::from_slice::<JournalRecord>(line) {
            Ok(record) if valid_record(&record) => records.push(record),
            Ok(_) | Err(_) => malformed = malformed.saturating_add(1),
        }
    }

    let mut paired_attempts = vec![false; records.len()];
    let mut entries = Vec::new();
    for (index, record) in records.iter().enumerate() {
        if record.phase != "result" {
            continue;
        }
        if let Some(attempt_index) = (0..index).rev().find(|candidate| {
            !paired_attempts[*candidate]
                && records[*candidate].phase == "attempt"
                && same_attempt(&records[*candidate], record)
        }) {
            paired_attempts[attempt_index] = true;
        }
        entries.push((index, record.clone()));
    }
    for (index, record) in records.into_iter().enumerate() {
        if record.phase == "attempt" && !paired_attempts[index] {
            let mut interrupted = record;
            interrupted.status = Some("interrupted".into());
            entries.push((index, interrupted));
        }
    }
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    let entries = entries
        .into_iter()
        .take(limit.clamp(1, 1000))
        .map(|(_, record)| record)
        .collect();
    let errors = if malformed == 0 {
        Vec::new()
    } else {
        vec![format!(
            "skipped {malformed} malformed apply journal line(s)"
        )]
    };
    Ok(History { entries, errors })
}

fn valid_record(record: &JournalRecord) -> bool {
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

fn same_attempt(attempt: &JournalRecord, result: &JournalRecord) -> bool {
    attempt.op == result.op
        && attempt.action == result.action
        && attempt.target == result.target
        && attempt.target_lossy == result.target_lossy
        && attempt.size_bytes == result.size_bytes
        && attempt.argv == result.argv
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

fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("devtrim-journal-{name}-{}", std::process::id()));
        crate::ops::remove_test_path(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn formats_epoch_seconds_as_utc() {
        assert_eq!(format_utc_timestamp(0), "1970-01-01 00:00:00");
        assert_eq!(format_utc_timestamp(1_704_114_309), "2024-01-01 13:05:09");
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
        let preserved = finish(&ctx, &attempt, Ok("trashed".to_string()));
        assert_eq!(preserved.unwrap(), "trashed");
        assert!(
            ctx.take_journal_errors()
                .iter()
                .any(|message| message.contains("journal result could not be written"))
        );

        // A failed operation keeps its own error, with the journal failure attached.
        let failed = finish::<()>(&ctx, &attempt, Err(anyhow::anyhow!("removal failed")));
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
}
