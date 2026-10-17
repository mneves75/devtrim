//! Offline inventory only. Category owners supply previews; this view cannot apply them.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::hash_map::RandomState;
use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::process::{BoundedCommand as _, QUERY_TIMEOUT};
use crate::report::{self, Action};
use crate::safety::{Ctx, RootsOrigin};

const MAX_DEPTH: usize = 6;
const MAX_NODES: usize = 10_000;
// A large early subtree must not consume every slot before its root siblings
// are encountered. Those siblings can then be opened or mapped as a new root.
const ROOT_RESERVE: usize = 1_024;
const MAX_ERRORS: usize = 256;
const CSS: &str = include_str!("dashboard/style.css");
const JS: &str = include_str!("dashboard/app.js");
const HTML: &str = include_str!("dashboard/page.html");

#[derive(Serialize)]
struct Node {
    name: String,
    path: String,
    kind: &'static str,
    bytes: u64,
    partial: bool,
    summarized_bytes: u64,
    summarized_entries: u64,
    children: Vec<Node>,
}

impl Node {
    fn new(path: &Path, kind: &'static str) -> Self {
        Self {
            name: path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned(),
            path: path.display().to_string(),
            kind,
            bytes: 0,
            partial: false,
            summarized_bytes: 0,
            summarized_entries: 0,
            children: Vec::new(),
        }
    }
}

#[derive(Serialize, Default)]
struct Coverage {
    partial: bool,
    issue_count: u64,
    errors: Vec<String>,
    summarized_entries: u64,
    retained_nodes: usize,
    max_depth: usize,
    max_nodes: usize,
}

impl Coverage {
    fn issue(&mut self, message: String) {
        self.partial = true;
        self.issue_count = self.issue_count.saturating_add(1);
        if self.errors.len() < MAX_ERRORS {
            self.errors.push(message);
        }
    }
}

#[derive(Serialize)]
struct Volume {
    capacity_bytes: Option<u64>,
    available_bytes: Option<u64>,
    error: Option<String>,
}

fn volume(root: &Path) -> Volume {
    let metrics = (|| -> Result<(u64, u64)> {
        let stat =
            rustix::fs::statvfs(root).context("cannot read the selected filesystem's capacity")?;
        if stat.f_frsize == 0 || stat.f_blocks == 0 || stat.f_bavail > stat.f_blocks {
            bail!("invalid filesystem capacity metrics");
        }
        let capacity = stat
            .f_blocks
            .checked_mul(stat.f_frsize)
            .context("volume capacity overflow")?;
        let available = stat
            .f_bavail
            .checked_mul(stat.f_frsize)
            .context("available-space overflow")?;
        Ok((capacity, available))
    })();
    match metrics {
        Ok((capacity, available)) => Volume {
            capacity_bytes: Some(capacity),
            available_bytes: Some(available),
            error: None,
        },
        Err(error) => Volume {
            capacity_bytes: None,
            available_bytes: None,
            error: Some(format!("{error:#}")),
        },
    }
}

/// One preorder walk; each measured byte is assigned once, then folded into
/// retained ancestors in reverse order. Omitting visual detail never omits bytes.
fn inventory(root: &Path) -> Result<(Node, Coverage)> {
    let device = fs::metadata(root)?.dev();
    let mut nodes = vec![Node::new(root, "directory")];
    let mut parents = vec![0];
    let mut ancestors = vec![0usize];
    let mut coverage = Coverage {
        max_depth: MAX_DEPTH,
        max_nodes: MAX_NODES,
        ..Coverage::default()
    };
    for result in walkdir::WalkDir::new(root)
        .follow_links(false)
        .follow_root_links(false)
        .same_file_system(true)
        .sort_by_file_name()
        .max_open(32)
        .min_depth(1)
    {
        let entry = match result {
            Ok(entry) => entry,
            Err(error) => {
                let parent = ancestors.last().copied().unwrap_or(0);
                nodes[parent].partial = true;
                coverage.issue(error.to_string());
                continue;
            }
        };
        ancestors.truncate(entry.depth());
        let parent = ancestors.last().copied().unwrap_or(0);
        let metadata = match fs::symlink_metadata(entry.path()) {
            Ok(metadata) => metadata,
            Err(error) => {
                nodes[parent].partial = true;
                coverage.issue(format!(
                    "cannot inspect {}: {error}",
                    entry.path().display()
                ));
                if entry.file_type().is_dir() {
                    ancestors.push(parent);
                }
                continue;
            }
        };
        let kind = if metadata.is_symlink() {
            "symlink"
        } else if metadata.is_dir() {
            "directory"
        } else if metadata.is_file() {
            "file"
        } else {
            "special"
        };
        let retained = entry.depth() <= MAX_DEPTH
            && nodes.len() < MAX_NODES
            && (entry.depth() == 1 || nodes.len() < MAX_NODES - ROOT_RESERVE);
        let current = if retained {
            nodes.push(Node::new(entry.path(), kind));
            parents.push(parent);
            nodes.len() - 1
        } else {
            nodes[parent].summarized_entries += 1;
            coverage.summarized_entries += 1;
            parent
        };
        if metadata.dev() != device {
            nodes[current].partial = true;
            coverage.issue(format!(
                "foreign filesystem not traversed: {}",
                entry.path().display()
            ));
        } else if metadata.is_file() || metadata.is_symlink() {
            match nodes[current].bytes.checked_add(metadata.len()) {
                Some(bytes) => {
                    nodes[current].bytes = bytes;
                    if !retained {
                        nodes[current].summarized_bytes = bytes;
                    }
                }
                None => {
                    nodes[current].partial = true;
                    coverage.issue(format!(
                        "logical size overflow at {}",
                        entry.path().display()
                    ));
                }
            }
        }
        if entry.file_type().is_dir() {
            ancestors.push(current);
        }
    }
    coverage.retained_nodes = nodes.len();
    // `pop` keeps memory bounded by the overview budget, regardless of how much
    // omitted detail the walk measured.
    while nodes.len() > 1 {
        if let (Some(mut child), Some(parent)) = (nodes.pop(), parents.pop()) {
            child.children.sort_by(|left, right| {
                right
                    .bytes
                    .cmp(&left.bytes)
                    .then_with(|| left.path.cmp(&right.path))
            });
            let owner = &mut nodes[parent];
            owner.partial |= child.partial;
            if let Some(bytes) = owner.bytes.checked_add(child.bytes) {
                owner.bytes = bytes;
            } else {
                owner.partial = true;
                coverage.issue(format!("logical size overflow at {}", owner.path));
            }
            owner.children.push(child);
        }
    }
    let mut root = nodes.pop().context("inventory root missing")?;
    root.children.sort_by(|left, right| {
        right
            .bytes
            .cmp(&left.bytes)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok((root, coverage))
}

#[derive(Serialize)]
struct Recommendation {
    label: String,
    path: String,
    bytes: u64,
    kind: &'static str,
    note: String,
}

#[derive(Serialize)]
struct Category {
    category: &'static str,
    preview_command: Option<String>,
    preview_unavailable: Option<&'static str>,
    findings: Vec<Recommendation>,
}

// Display only: POSIX single quotes, or POSIX.1-2024/zsh/Bash dollar-single
// quotes when controls would otherwise be stripped by the single-line input.
fn shell_argument(value: &str) -> String {
    if !value.chars().any(char::is_control) {
        return format!("'{}'", value.replace('\'', "'\\''"));
    }
    let mut quoted = String::from("$'");
    for character in value.chars() {
        match character {
            '\'' => quoted.push_str("\\'"),
            '\\' => quoted.push_str("\\\\"),
            control if control.is_control() => {
                for byte in control.encode_utf8(&mut [0; 4]).bytes() {
                    quoted.push_str(&format!("\\{byte:03o}"));
                }
            }
            other => quoted.push(other),
        }
    }
    quoted.push('\'');
    quoted
}

fn preview_command(
    operation: &dyn crate::ops::Op,
    ctx: &Ctx,
) -> std::result::Result<String, &'static str> {
    let mut command = format!("devtrim clean {}", operation.name());
    if operation.scans_roots() && ctx.roots_origin == RootsOrigin::Flag {
        for root in &ctx.roots {
            // Missing explicit roots can still be relative in Ctx. Anchor them
            // now, rather than changing scope in the operator's later terminal.
            let absolute = std::path::absolute(root).map_err(
                |_| "Preview command unavailable: cannot resolve an absolute scan root.",
            )?;
            let value = absolute.to_str().ok_or(
                "Preview command unavailable: an explicit scan root is not valid UTF-8. Re-run a category preview with the original roots.",
            )?;
            command.push_str(" --root ");
            command.push_str(&shell_argument(value));
        }
    }
    Ok(command)
}

#[derive(Serialize)]
struct Recommendations {
    categories: Vec<Category>,
    errors: Vec<String>,
    diagnostics: Vec<String>,
    roots_note: String,
}

fn recommendations(ctx: &Ctx, root: &Path) -> Recommendations {
    let scan = crate::ops::scan_all(ctx);
    let operations = crate::ops::all();
    let mut categories = Vec::new();
    for section in &scan.sections {
        let findings = scan.findings[section.range.clone()]
            .iter()
            .filter_map(|finding| {
                let target = finding.target()?;
                if !target.starts_with(root)
                    || finding.scan_error().is_some()
                    || matches!(finding.action, Action::Command { .. })
                {
                    return None;
                }
                Some(Recommendation {
                    label: finding.label.clone(),
                    path: target.display().to_string(),
                    bytes: finding.size_bytes,
                    kind: if matches!(finding.action, Action::Trash | Action::Shred)
                        && !matches!(section.category, "agents" | "installers")
                    {
                        "preview"
                    } else {
                        "review"
                    },
                    note: finding.note.clone(),
                })
            })
            .collect::<Vec<_>>();
        if !findings.is_empty() {
            let command = operations
                .iter()
                .find(|operation| operation.name() == section.category)
                .ok_or("Preview command unavailable: category owner is unavailable.")
                .and_then(|operation| preview_command(operation.as_ref(), ctx));
            let (preview_command, preview_unavailable) = match command {
                Ok(command) => (Some(command), None),
                Err(reason) => (None, Some(reason)),
            };
            categories.push(Category {
                category: section.category,
                preview_command,
                preview_unavailable,
                findings,
            });
        }
    }
    Recommendations {
        categories,
        errors: scan.errors,
        diagnostics: ctx.take_diagnostics(),
        roots_note: ctx.roots_note(),
    }
}

#[derive(Serialize)]
struct Snapshot {
    schema_version: u8,
    operation: &'static str,
    root: String,
    output_file: String,
    generated_unix_seconds: u64,
    inventory: Node,
    coverage: Coverage,
    volume: Volume,
    recommendations: Recommendations,
    errors: Vec<String>,
}

fn output_path(requested: &Path) -> Result<PathBuf> {
    let name = requested
        .file_name()
        .context("output must name a new HTML file")?;
    let parent = requested
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let path = parent
        .canonicalize()
        .with_context(|| format!("cannot resolve output parent {}", parent.display()))?
        .join(name);
    match fs::symlink_metadata(&path) {
        Ok(_) => bail!(
            "output already exists (never overwritten): {}",
            path.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(path),
        Err(error) => {
            Err(error).with_context(|| format!("cannot inspect output {}", path.display()))
        }
    }
}

fn create_output(explicit: Option<&Path>) -> Result<(File, PathBuf)> {
    if let Some(path) = explicit {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("cannot create new output {}", path.display()))?;
        return Ok((file, path.to_path_buf()));
    }
    let directory = std::env::temp_dir()
        .canonicalize()
        .context("cannot resolve temporary directory")?;
    for _ in 0..16 {
        let random = RandomState::new().build_hasher().finish();
        let path = directory.join(format!("devtrim-dashboard-{random:016x}.html"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("cannot create output {}", path.display()));
            }
        }
    }
    bail!("cannot reserve a unique temporary dashboard file")
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::new();
    for chunk in bytes.chunks(3) {
        let bits = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for shift in [18, 12, 6, 0] {
            encoded.push(
                if (shift == 6 && chunk.len() < 2) || (shift == 0 && chunk.len() < 3) {
                    '='
                } else {
                    char::from(ALPHABET[((bits >> shift) & 63) as usize])
                },
            );
        }
    }
    encoded
}

fn document(snapshot: &Snapshot) -> Result<String> {
    // Script data is inert, but its HTML raw-text terminator still needs escaping.
    let data = serde_json::to_string(snapshot)?
        .replace('&', "\\u0026")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    let policy = format!(
        "default-src 'none'; script-src 'sha256-{}'; style-src 'sha256-{}'; img-src 'none'; connect-src 'none'; font-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'",
        base64(&Sha256::digest(JS.as_bytes())),
        base64(&Sha256::digest(CSS.as_bytes()))
    );
    // Insert data last: filenames containing template markers must stay labels.
    Ok(HTML
        .replace("__CSP__", &policy)
        .replace("__CSS__", CSS)
        .replace("__JS__", JS)
        .replace("__DATA__", &data))
}

pub(crate) fn run(
    ctx: &Ctx,
    requested: Option<&Path>,
    output: Option<&Path>,
    no_open: bool,
) -> Result<ExitCode> {
    // Refuse an existing output before either inventory or category probes.
    let output = output.map(output_path).transpose()?;
    let root = requested
        .unwrap_or(&ctx.home)
        .canonicalize()
        .context("cannot resolve dashboard directory")?;
    if !fs::metadata(&root)?.is_dir() {
        bail!("dashboard root is not a directory: {}", root.display());
    }
    let (inventory, coverage) = inventory(&root)?;
    let volume = volume(&root);
    let recommendations = recommendations(ctx, &root);
    let mut errors = Vec::new();
    if coverage.partial {
        errors.push(format!(
            "inventory is a partial lower bound ({} issues)",
            coverage.issue_count
        ));
    }
    if let Some(error) = &volume.error {
        errors.push(error.clone());
    }
    errors.extend(recommendations.errors.iter().cloned());
    let (mut file, output_file) = create_output(output.as_deref())?;
    let mut snapshot = Snapshot {
        schema_version: 1,
        operation: "dashboard",
        root: root.display().to_string(),
        output_file: output_file.display().to_string(),
        generated_unix_seconds: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock precedes Unix epoch")?
            .as_secs(),
        inventory,
        coverage,
        volume,
        recommendations,
        errors,
    };
    file.write_all(document(&snapshot)?.as_bytes())
        .and_then(|()| file.sync_all())
        .with_context(|| format!("cannot write dashboard {}", output_file.display()))?;
    if !no_open {
        let opened = crate::ops::command_stdout(
            Command::new("open")
                .arg(&output_file)
                .output_within(QUERY_TIMEOUT),
            "open dashboard",
        );
        if let Err(error) = opened {
            snapshot.errors.push(format!(
                "cannot open dashboard; report preserved at {}: {error:#}",
                output_file.display()
            ));
        }
    }
    if ctx.json {
        let mut data = serde_json::to_vec(&snapshot)?;
        data.push(b'\n');
        report::write_stdout(&data)?;
    } else {
        report::write_stdout(
            format!(
                "Dashboard: {}\nSnapshot only; rerun devtrim dashboard to refresh.\n",
                report::terminal_safe(&snapshot.output_file)
            )
            .as_bytes(),
        )?;
        for error in &snapshot.errors {
            ctx.diagnostic("warn", error.clone());
        }
        // Dashboard diagnostics normally stay in the report, but open/write
        // failures must also be useful to someone at the terminal.
        for message in ctx.take_diagnostics() {
            eprintln!("{}", report::terminal_safe(&message));
        }
    }
    Ok(if snapshot.errors.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}
