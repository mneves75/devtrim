//! Coding-agent caches and stale session history. Filesystem targets remain Trash-first.
//!
//! Caches, standalone packages, and history carry different promises:
//!
//! * A *regenerable* entry is a cache the agent rebuilds on demand, so it is
//!   offered unconditionally at a low danger score. What that claims is only
//!   that the content comes back — not that a running agent will not notice,
//!   because no vendor documents these as removable mid-session. Trash-first
//!   provides *recovery*, not safety, and `--shred` removes even that. The
//!   preview says so rather than leaving the user to infer it.
//! * A *history* entry is a session transcript or a shell snapshot. Nothing
//!   regenerates it. It is therefore offered only after the
//!   configured active window has passed over the whole subtree, and its note
//!   says plainly that the content does not come back.
//! * An older Codex standalone package is reinstallable only after its
//!   installer-owned shape, lock, and current selection are verified.
//!
//! Authentication material (`auth.json`, `.credentials.json`), configuration,
//! memories, skills, agent definitions, installed plugins, and the `.claude.json`
//! backup copies are never in either list and are never traversed as candidates.
//!
//! Two whole trees under `~/.claude` are excluded, and they are the exclusions
//! to know about. `projects` holds auto memory keyed by repository root, beside
//! transcripts the vendor retains at any age when the session came from Claude
//! Desktop — age is not evidence there, and telling the retained ones apart
//! would mean reading transcript contents. `jobs` is the background-session
//! supervisor's live state, with a `pins.json` beside it naming the sessions
//! kept alive while idle. Both cases share one rule: a closed category that has
//! to consult a liveness signal to stay safe has gone one directory too far.
//!
//! Other stores are absent only because they cost more preview than they
//! return, a preview nobody can read being no preview at all. Against the
//! machine this was built on, Codex lane transcripts produced 558 findings for
//! 0.25 GB, its `.tmp` tree 324 for 0.07 GB, the Claude Code file-edit history
//! 123 for 0.10 GB, and the paste cache a comparable count for about 2 MB.

use anyhow::{Context, Result};
use rustix::fs::{FlockOperation, Mode, OFlags, flock, open};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use super::{Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, dir_size, removal_note};
use crate::safety::{Ctx, DeletionEntry, escalate};

pub struct Agents;

/// Caches an agent rebuilds on demand. Exact paths relative to `$HOME`; nothing
/// is matched by prefix, so a sibling directory can never inherit authority.
///
/// Each entry names the evidence it rests on, because this list is the only
/// thing guarding these paths: there is no age gate behind it and no
/// corroboration signal, only Trash-first.
///
/// Documented by their vendors: `.claude/cache` holds the changelog and model
/// catalogue Claude Code refreshes in the background; Pi's web-search cache
/// self-evicts on a one-hour lifetime with fixed entry and size limits; and
/// OpenCode's own troubleshooting guide prescribes removing its cache as a reset
/// step, with persistent data kept under a different tree.
///
/// Rests on direct inspection only: `.codex/cache`, which no OpenAI
/// documentation describes. Every child observed there was a hash-named JSON
/// catalogue re-fetched from the network (`codex_app_directory`,
/// `codex_apps_server_info`, `codex_apps_tools`, `remote_plugin_catalog`).
/// Codex keeps its model catalogue in `models_cache.json` and plugin bundles in
/// `plugins/cache/`, neither of which is this directory.
///
/// `.claude/downloads` was dropped for having neither: undocumented and empty
/// wherever it could be examined, so nothing could say what it holds.
const REGENERABLE: &[DeletionEntry] = &[
    DeletionEntry {
        label: "Claude Code metadata cache",
        relative: ".claude/cache",
        evidence: "Vendor-documented: the `.claude` directory reference lists \
                   `cache/changelog.md` as refreshed in the background. The \
                   observed siblings (`model-catalog/`, `my-closed-issues.json`) \
                   are re-fetched the same way.",
    },
    DeletionEntry {
        label: "Codex catalog cache",
        relative: ".codex/cache",
        evidence: "Inspection only — no OpenAI documentation describes this \
                   directory. Every child observed was a hash-named JSON \
                   catalogue re-fetched from the network \
                   (`codex_app_directory`, `codex_apps_server_info`, \
                   `codex_apps_tools`, `remote_plugin_catalog`). Codex keeps \
                   its model catalogue in `models_cache.json` and plugin \
                   bundles in `plugins/cache/`, neither of which is this path. \
                   Observation cannot exhaust future contents.",
    },
    DeletionEntry {
        label: "Pi web-search cache",
        relative: ".pi/web-search-cache",
        evidence: "Vendor-documented as a private fetched-content cache with a \
                   one-hour lifetime and fixed 128-entry / 128 MiB limits that \
                   evict oldest-first; it discards itself.",
    },
    DeletionEntry {
        label: "OpenCode cache",
        relative: ".cache/opencode",
        evidence: "OpenCode's troubleshooting guide prescribes removing this \
                   directory as a reset step. Persistent state (auth, sessions, \
                   messages, logs) lives under `~/.local/share/opencode`.",
    },
];

const _: () = assert!(
    crate::safety::evidence_is_present(REGENERABLE),
    "every regenerable agent cache needs evidence for why it may be deleted"
);

/// Every regenerable entry shares one danger score: the cost of removing any of
/// them is a re-fetch. A per-entry column would be a knob with one value.
const REGENERABLE_DANGER: u8 = 2;

// OpenAI's standalone installer at commit 0a2eb4696c26ac33204bcd255721ab30220a4774
// (`scripts/install/install.sh`, lines 30-60, 947-1035, 1200-1285) owns
// `releases/<version>-<target>`, the `current` symlink, and `install.lock`.
// A 2026-09-23 local audit found six old release directories beside the current
// one. This authority covers only verified older direct children, never the
// whole packages tree or legacy layouts. The vendor does not promise removal
// is safe mid-session.
const CODEX_STANDALONE: &str = ".codex/packages/standalone";

// Observed in the official macOS standalone bundle at 0.156.1. New vendor
// resource names need an explicit review before they can authorize deletion.
const CODEX_VOICE_FILES: &[&str] = &[
    "codex-resources/voice/bin/codex-voice-host",
    "codex-resources/voice/plugins/libgstapp.dylib",
    "codex-resources/voice/plugins/libgstaudioconvert.dylib",
    "codex-resources/voice/plugins/libgstaudioresample.dylib",
    "codex-resources/voice/plugins/libgstcoreelements.dylib",
    "codex-resources/voice/plugins/libgstopus.dylib",
    "codex-resources/voice/plugins/libgstrtp.dylib",
    "codex-resources/voice/plugins/libgstrtpmanager.dylib",
    "codex-resources/voice/lib/libffi.8.dylib",
    "codex-resources/voice/lib/libgio-2.0.0.dylib",
    "codex-resources/voice/lib/libglib-2.0.0.dylib",
    "codex-resources/voice/lib/libgmodule-2.0.0.dylib",
    "codex-resources/voice/lib/libgobject-2.0.0.dylib",
    "codex-resources/voice/lib/libgstapp-1.0.0.dylib",
    "codex-resources/voice/lib/libgstaudio-1.0.0.dylib",
    "codex-resources/voice/lib/libgstbase-1.0.0.dylib",
    "codex-resources/voice/lib/libgstnet-1.0.0.dylib",
    "codex-resources/voice/lib/libgstpbutils-1.0.0.dylib",
    "codex-resources/voice/lib/libgstreamer-1.0.0.dylib",
    "codex-resources/voice/lib/libgstrtp-1.0.0.dylib",
    "codex-resources/voice/lib/libgsttag-1.0.0.dylib",
    "codex-resources/voice/lib/libgstvideo-1.0.0.dylib",
    "codex-resources/voice/lib/libintl.8.dylib",
    "codex-resources/voice/lib/libopus.0.dylib",
    "codex-resources/voice/lib/libpcre2-8.0.dylib",
    "codex-resources/voice/lib/libz.1.dylib",
    "codex-resources/voice/runtime.json",
    "codex-resources/voice/NOTICE.md",
    "codex-resources/voice/sources.json",
    "codex-resources/voice/licenses/LGPL-2.1.txt",
    "codex-resources/voice/licenses/Opus.txt",
    "codex-resources/voice/licenses/PCRE2.md",
    "codex-resources/voice/licenses/libffi.txt",
    "codex-resources/voice/licenses/proxy-libintl.txt",
    "codex-resources/voice/licenses/sljit.txt",
    "codex-resources/voice/licenses/zlib.txt",
];

struct CodexReleases {
    root: PathBuf,
    current: PathBuf,
    current_version: [u64; 3],
    _install_lock: File,
}

impl CodexReleases {
    fn open(home: &Path) -> Result<Option<Self>> {
        let standalone = home.join(CODEX_STANDALONE);
        let root = standalone.join("releases");
        match fs::symlink_metadata(&root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("cannot inspect {}", root.display()));
            }
            Ok(metadata) if !metadata.file_type().is_dir() => {
                anyhow::bail!(
                    "Codex releases root is not a real directory: {}",
                    root.display()
                );
            }
            Ok(_) => {}
        }
        if !fs::symlink_metadata(&standalone)?.file_type().is_dir() {
            anyhow::bail!(
                "Codex standalone root is not a real directory: {}",
                standalone.display()
            );
        }

        // The installer holds this file while replacing releases and `current`.
        // Keep a compatible advisory lock through the entire scan or apply.
        let lock_path = standalone.join("install.lock");
        let lock_metadata = fs::symlink_metadata(&lock_path).with_context(|| {
            format!(
                "cannot inspect Codex installer lock {}",
                lock_path.display()
            )
        })?;
        if !lock_metadata.file_type().is_file() {
            anyhow::bail!(
                "Codex installer lock is not a real file: {}",
                lock_path.display()
            );
        }
        let install_lock = File::open(&lock_path)?;
        let opened = install_lock.metadata()?;
        if (lock_metadata.dev(), lock_metadata.ino()) != (opened.dev(), opened.ino()) {
            anyhow::bail!(
                "Codex installer lock changed while opening: {}",
                lock_path.display()
            );
        }
        flock(&install_lock, FlockOperation::NonBlockingLockShared).with_context(|| {
            format!(
                "Codex installer may be active; cannot lock {}",
                lock_path.display()
            )
        })?;
        // The installer can fall back to a directory lock when neither lockf
        // nor flock exists. Even a stale directory is ambiguous to us.
        match fs::symlink_metadata(standalone.join("install.lock.d")) {
            Ok(_) => anyhow::bail!("Codex installer directory lock exists"),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }

        let current_link = standalone.join("current");
        if !fs::symlink_metadata(&current_link)?
            .file_type()
            .is_symlink()
        {
            anyhow::bail!(
                "Codex current release is not a symlink: {}",
                current_link.display()
            );
        }
        let current = current_link.canonicalize().with_context(|| {
            format!(
                "cannot resolve Codex current release {}",
                current_link.display()
            )
        })?;
        if current.parent() != Some(root.canonicalize()?.as_path()) {
            anyhow::bail!(
                "Codex current release is outside verified standalone packages: {}",
                current_link.display()
            );
        }
        let Some(current_version) = codex_package_version(&current)? else {
            anyhow::bail!(
                "Codex current release is not a verified package: {}",
                current.display()
            );
        };
        Ok(Some(Self {
            root,
            current,
            current_version,
            _install_lock: install_lock,
        }))
    }

    fn eligible(&self, path: &Path) -> Result<bool> {
        Ok(path.parent() == Some(self.root.as_path())
            && codex_package_version(path)?.is_some_and(|version| version < self.current_version)
            && path.canonicalize()? != self.current)
    }
}

fn codex_package_version(path: &Path) -> Result<Option<[u64; 3]>> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Ok(None);
    }
    let Some(manifest) = codex_json_file(&path.join("codex-package.json"), 4096)? else {
        return Ok(None);
    };
    let Some(version) = manifest["version"].as_str() else {
        return Ok(None);
    };
    let Some(target) = manifest["target"].as_str() else {
        return Ok(None);
    };
    let version_core = codex_version_core(version);
    let expected_name = format!("{version}-{target}");
    if version_core.is_none()
        || !matches!(target, "aarch64-apple-darwin" | "x86_64-apple-darwin")
        || path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str())
        || manifest["layoutVersion"] != 1
        || manifest["variant"] != "codex"
        || manifest["entrypoint"] != "bin/codex"
        || manifest["resourcesDir"] != "codex-resources"
        || manifest["pathDir"] != "codex-path"
    {
        return Ok(None);
    }
    for directory in ["bin", "codex-resources", "codex-path"] {
        if !codex_entry_metadata(&path.join(directory))?
            .is_some_and(|metadata| metadata.file_type().is_dir())
        {
            return Ok(None);
        }
    }
    if !codex_executable(&path.join("bin/codex"))?
        || !codex_executable(&path.join("bin/codex-code-mode-host"))?
        || !codex_executable(&path.join("codex-path/rg"))?
        || !codex_dir_entries_known(&path.join("bin"), &["codex", "codex-code-mode-host"])?
        || !codex_dir_entries_known(&path.join("codex-path"), &["rg"])?
        || !codex_resources_known(path, version, target)?
        || !codex_entry_metadata(&path.join("codex"))?
            .is_some_and(|metadata| metadata.file_type().is_symlink())
        || fs::read_link(path.join("codex"))? != Path::new("bin/codex")
    {
        return Ok(None);
    }
    for entry in fs::read_dir(path)? {
        let name = entry?.file_name();
        if ![
            "bin",
            "codex",
            "codex-package.json",
            "codex-path",
            "codex-resources",
        ]
        .iter()
        .any(|expected| name == *expected)
        {
            return Ok(None);
        }
    }
    Ok(version_core)
}

fn codex_entry_metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot inspect {}", path.display())),
    }
}

fn codex_json_file(path: &Path, max_bytes: u64) -> Result<Option<serde_json::Value>> {
    let fd = match open(path, OFlags::RDONLY | OFlags::NOFOLLOW, Mode::empty()) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT | rustix::io::Errno::LOOP) => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("cannot open {}", path.display())),
    };
    let file = File::from(fd);
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() || metadata.len() > max_bytes {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(max_bytes + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&bytes).with_context(|| {
        format!("cannot parse {}", path.display())
    })?))
}

fn codex_executable(path: &Path) -> Result<bool> {
    Ok(codex_entry_metadata(path)?.is_some_and(|metadata| {
        metadata.file_type().is_file()
            && metadata.len() > 0
            && metadata.permissions().mode() & 0o111 == 0o111
    }))
}

fn codex_dir_entries_known(path: &Path, names: &[&str]) -> Result<bool> {
    for entry in fs::read_dir(path)? {
        let name = entry?.file_name();
        if !names.iter().any(|expected| name == *expected) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn codex_resources_known(release: &Path, version: &str, target: &str) -> Result<bool> {
    let resources = release.join("codex-resources");
    let voice = resources.join("voice");
    let mut expected = HashSet::new();
    let Some(metadata) = codex_entry_metadata(&voice)? else {
        return Ok(false);
    };
    if !metadata.file_type().is_dir() {
        return Ok(false);
    }
    let Some(manifest) = codex_json_file(&voice.join("manifest.json"), 64 * 1024)? else {
        return Ok(false);
    };
    let Some(files) = manifest["sha256"].as_object() else {
        return Ok(false);
    };
    if manifest["schemaVersion"] != 1
        || manifest["appVersion"] != version
        || manifest["appTarget"] != target
        || manifest["voiceTarget"] != target
        || !files.contains_key("bin/codex")
    {
        return Ok(false);
    }
    for (name, digest) in files {
        let Some(digest) = digest.as_str() else {
            return Ok(false);
        };
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(false);
        }
        if name != "bin/codex" && !CODEX_VOICE_FILES.contains(&name.as_str()) {
            return Ok(false);
        }
        let relative = Path::new(name);
        if !relative
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
            || !codex_digest_matches(&release.join(relative), digest)?
        {
            return Ok(false);
        }
        if name != "bin/codex" {
            expected.insert(relative.to_path_buf());
        }
    }
    expected.insert(PathBuf::from("codex-resources/voice/manifest.json"));
    let zsh = resources.join("zsh");
    if let Some(metadata) = codex_entry_metadata(&zsh)? {
        if !metadata.file_type().is_dir() || !codex_executable(&zsh.join("bin/zsh"))? {
            return Ok(false);
        }
        expected.insert(PathBuf::from("codex-resources/zsh/bin/zsh"));
    }
    let mut directories = vec![resources];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path.strip_prefix(release)?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() && expected.iter().any(|file| file.starts_with(relative)) {
                directories.push(path);
            } else if file_type.is_file() && expected.remove(relative) {
                continue;
            } else {
                return Ok(false);
            }
        }
    }
    Ok(expected.is_empty())
}

fn codex_digest_matches(path: &Path, expected: &str) -> Result<bool> {
    let fd = match open(path, OFlags::RDONLY | OFlags::NOFOLLOW, Mode::empty()) {
        Ok(fd) => fd,
        Err(rustix::io::Errno::NOENT | rustix::io::Errno::LOOP) => return Ok(false),
        Err(error) => return Err(error).with_context(|| format!("cannot open {}", path.display())),
    };
    let mut file = File::from(fd);
    if !file.metadata()?.file_type().is_file() {
        return Ok(false);
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok(expected.len() == 64
        && digest.iter().enumerate().all(|(index, byte)| {
            u8::from_str_radix(&expected[index * 2..index * 2 + 2], 16)
                .is_ok_and(|expected_byte| expected_byte == *byte)
        }))
}

fn codex_version_core(version: &str) -> Option<[u64; 3]> {
    let (core, prerelease) = match version.split_once('-') {
        Some((_, "")) => return None,
        Some(parts) => parts,
        None => (version, ""),
    };
    let parts: Vec<_> = core.split('.').collect();
    if parts.len() != 3
        || !parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    let core = [
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ];
    if prerelease.is_empty() {
        return Some(core);
    }
    let fields: Vec<_> = prerelease.split('.').collect();
    let valid_prerelease = match fields.as_slice() {
        ["alpha"] | ["beta"] => true,
        ["alpha", number] | ["beta", number]
            if !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            true
        }
        ["alpha", first, second]
            if [first, second].iter().all(|number| {
                !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
            }) =>
        {
            true
        }
        _ => false,
    };
    valid_prerelease.then_some(core)
}

/// A store whose children are per-session history rather than cache.
struct HistoryRoot {
    label: &'static str,
    /// Exact path relative to `$HOME`.
    relative: &'static str,
    /// Levels below `relative` at which one child becomes one finding. Codex
    /// nests sessions as `<year>/<month>/<day>`, so waiting for a whole year to
    /// go stale would never offer the current one.
    depth: usize,
    /// Why this root's children may be offered once stale, and the source that
    /// establishes it. Required for the same reason as [`DeletionEntry`]: a
    /// root added without evidence must not compile.
    evidence: &'static str,
}

const HISTORY: &[HistoryRoot] = &[
    // A shell snapshot is written once per session and sourced by every later
    // shell call in that session; nothing rewrites it if it disappears. It is
    // therefore history, not cache.
    //
    // The age gate is the right signal here, and not for the reason it was
    // wrong for `jobs`: Claude Code sweeps this directory itself once an entry
    // passes its own `cleanupPeriodDays` retention, so age is the vendor's own
    // criterion here and devtrim only reaches the same conclusion sooner when
    // the configured window is shorter, with every finding stating the age it
    // used. `jobs` had no such sweep and did have a liveness file beside it,
    // which is what made age the wrong signal there.
    HistoryRoot {
        label: "Claude Code shell snapshots",
        relative: ".claude/shell-snapshots",
        depth: 1,
        evidence: "Vendor-documented: one snapshot per session, applied by the \
                   Bash tool to each command, and swept by Claude Code's own \
                   `cleanupPeriodDays` retention since v2.1.117 — so age is the \
                   vendor's own criterion here. Not rewritten if removed \
                   mid-session.",
    },
    HistoryRoot {
        label: "Codex shell snapshots",
        relative: ".codex/shell_snapshots",
        depth: 1,
        evidence: "Per-session `<session-id>.<nanos>.sh` captures. Removing stale \
                   ones is desirable: openai/codex#30971 documents that they \
                   can retain exported secrets in plaintext. devtrim never \
                   reads their contents.",
    },
    HistoryRoot {
        label: "Codex session transcripts",
        relative: ".codex/sessions",
        depth: 3,
        evidence: "Rollout transcripts nested `<year>/<month>/<day>`, confirmed on \
                   disk and in openai/codex#24948. Conversation history, not \
                   cache: offered only past the active window, and the note \
                   says it does not come back.",
    },
    HistoryRoot {
        label: "Codex archived sessions",
        relative: ".codex/archived_sessions",
        depth: 1,
        evidence: "Rollout transcripts rolled off from `sessions/`, flat on this \
                   installation; Codex rescans them to rebuild its session \
                   index. Conversation history, not cache — same age gate and \
                   same note as `sessions/`.",
    },
];

const _: () = {
    let mut index = 0;
    while index < HISTORY.len() {
        assert!(
            crate::safety::evidence_is_meaningful(HISTORY[index].evidence),
            "every history root needs evidence for why its children may be deleted"
        );
        index += 1;
    }
};

const DAY: u64 = 60 * 60 * 24;

impl Op for Agents {
    fn name(&self) -> &'static str {
        "agents"
    }

    fn scan(
        &self,
        ctx: &Ctx,
        _observations: &super::project::ScanObservations,
    ) -> Result<Vec<Finding>> {
        let mut findings = Vec::new();
        for entry in REGENERABLE {
            let path = ctx.home.join(entry.relative);
            let size = dir_size(&path)?;
            if size > 0 {
                findings.push(Finding::new(
                    entry.label,
                    Some(path),
                    size,
                    "rebuilt on demand; cleanup may interrupt an active session, so close agents first",
                    escalate(REGENERABLE_DANGER, size),
                    Action::Trash,
                ));
            }
        }
        let release_findings = (|| -> Result<Vec<Finding>> {
            let Some(releases) = CodexReleases::open(&ctx.home)? else {
                return Ok(Vec::new());
            };
            let mut release_findings = Vec::new();
            for entry in fs::read_dir(&releases.root)? {
                let path = entry?.path();
                if !releases.eligible(&path)? {
                    continue;
                }
                let size = dir_size(&path)?;
                if size == 0 {
                    continue;
                }
                release_findings.push(Finding::new(
                    format!(
                        "Codex standalone release {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ),
                    Some(path),
                    size,
                    "reinstallable previous release; close Codex first; Trash does not free disk space until purged",
                    escalate(3, size),
                    Action::Trash,
                ));
            }
            Ok(release_findings)
        })();
        match release_findings {
            Ok(releases) => findings.extend(releases),
            Err(error) => {
                let message = format!("Codex release cleanup refused: {error:#}");
                findings.push(
                    Finding::new(
                        "Codex standalone releases unavailable",
                        None,
                        0,
                        &message,
                        5,
                        Action::Info,
                    )
                    .with_scan_error(message),
                );
            }
        }
        for root in HISTORY {
            let base = ctx.home.join(root.relative);
            let mut candidates = Vec::new();
            collect_at_depth(&base, root.depth, &mut candidates)?;
            candidates.sort();
            for path in candidates {
                let Some((age, size)) = history_details(&path, &ctx.home, ctx.active_days)? else {
                    continue;
                };
                if size == 0 {
                    continue;
                }
                findings.push(Finding::new(
                    format!("{}: {}", root.label, relative_display(&path, &base)),
                    Some(path),
                    size,
                    format!(
                        "untouched for {age} days; agent history is not regenerable and cannot be re-downloaded"
                    ),
                    escalate(6, size),
                    Action::Trash,
                ));
            }
        }
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let releases = if findings
            .iter()
            .filter_map(Finding::target)
            .any(|target| is_codex_release_child(target, &ctx.home))
        {
            Some(CodexReleases::open(&ctx.home))
        } else {
            None
        };
        let mut outcome = ApplyOutcome::new(self.name());
        for finding in findings {
            if !matches!(finding.action, Action::Trash | Action::Shred) {
                continue;
            }
            let result = (|| -> Result<()> {
                let target = finding
                    .target()
                    .ok_or_else(|| anyhow::anyhow!("agent finding missing internal target"))?;
                let release_context = match releases.as_ref() {
                    Some(Ok(Some(releases))) => Some(releases),
                    Some(Err(error)) if is_codex_release_child(target, &ctx.home) => {
                        anyhow::bail!("Codex release cannot be removed while its installer is active or unverifiable: {error:#}");
                    }
                    _ => None,
                };
                authorize(target, ctx, release_context)?;
                apply_filesystem_finding(self.name(), finding, ctx)
            })()
            .with_context(|| format!("failed to remove {}", finding.label));
            // One refused finding must not abandon the rest of the previewed
            // plan. The age gate is re-read at apply, so a session resumed
            // between preview and apply is an ordinary, expected refusal — and
            // the documented promise is that such a session falls out of the
            // plan, not that it takes every later finding with it. Each failure
            // is still recorded, so the run reports nonzero.
            if let Err(error) = result {
                outcome.fail(error);
                continue;
            }
            outcome.record(finding, removal_note(finding, &finding.label));
        }
        Ok(outcome)
    }
}

/// The scanner is never deletion authority: apply reasserts the full shape of
/// whichever tier the target claims, including the age gate, before the sink
/// sees it.
///
/// The two refusals are reported separately. A target outside both lists is a
/// forged or stale plan; a target inside a history root that no longer passes
/// the gate is the ordinary case of a session resumed between preview and
/// apply, and saying "outside its authorized namespace" would misdescribe it.
fn authorize(target: &Path, ctx: &Ctx, releases: Option<&CodexReleases>) -> Result<()> {
    if is_regenerable_target(target, &ctx.home) {
        return Ok(());
    }
    if is_codex_release_child(target, &ctx.home) {
        if releases
            .map(|releases| releases.eligible(target))
            .transpose()?
            .unwrap_or(false)
        {
            return Ok(());
        }
        anyhow::bail!(
            "Codex release is no longer a verified obsolete package: {}",
            target.display()
        );
    }
    if !is_history_child(target, &ctx.home) {
        anyhow::bail!(
            "agent target is outside its authorized namespace: {}",
            target.display()
        );
    }
    if history_details(target, &ctx.home, ctx.active_days)?.is_some() {
        return Ok(());
    }
    anyhow::bail!(
        "agent history no longer meets its preview shape — it became active or is now a symlink; refusing {}",
        target.display()
    )
}

fn is_codex_release_child(path: &Path, home: &Path) -> bool {
    path.parent() == Some(home.join(CODEX_STANDALONE).join("releases").as_path())
}

fn is_regenerable_target(path: &Path, home: &Path) -> bool {
    REGENERABLE
        .iter()
        .any(|entry| path == home.join(entry.relative))
}

/// Age in days and logical size for an eligible history child, or `None` when
/// the path is not a direct child of a configured root at its configured depth,
/// is a symlink, has the wrong file type, or is still inside the active window.
fn history_details(path: &Path, home: &Path, active_days: u32) -> Result<Option<(u64, u64)>> {
    if !is_history_child(path, home) {
        return Ok(None);
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot inspect {}", path.display()));
        }
    };
    // A symlink is refused outright: following one would delete a tree outside
    // the authorized root while every path check above still passed.
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Ok(None);
    }
    // Every root holds either one directory or one loose file per session, so
    // both are candidates; anything else — a socket, a device node — is not.
    if !file_type.is_dir() && !file_type.is_file() {
        return Ok(None);
    }
    let (size, newest) = crate::safety::dir_stats(path)?;
    // No timestamp means a file in this subtree would not say when it changed,
    // so its staleness is unknown and the gate fails closed.
    let Some(newest) = newest else {
        return Ok(None);
    };
    let Ok(elapsed) = SystemTime::now().duration_since(newest) else {
        return Ok(None);
    };
    let age = elapsed.as_secs() / DAY;
    Ok((age >= u64::from(active_days)).then_some((age, size)))
}

/// Whether this path is a child of a configured root at exactly that root's depth.
///
/// Matching is structural rather than by prefix: the path must be `root` plus
/// exactly `depth` normal components, so neither a shallower ancestor (the root
/// itself) nor a deeper descendant can borrow the root's authority.
fn is_history_child(path: &Path, home: &Path) -> bool {
    HISTORY.iter().any(|root| {
        let base = home.join(root.relative);
        let Ok(relative) = path.strip_prefix(&base) else {
            return false;
        };
        relative.components().count() == root.depth
            && relative
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
    })
}

fn relative_display(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Direct children of `base` exactly `depth` levels down.
///
/// Traversal refuses to descend through a symlink, so a link planted inside an
/// authorized root cannot widen the candidate set to a foreign tree.
fn collect_at_depth(base: &Path, depth: usize, found: &mut Vec<PathBuf>) -> Result<()> {
    let entries = match std::fs::read_dir(base) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", base.display()));
        }
    };
    for entry in entries {
        let entry = entry.with_context(|| format!("cannot enumerate {}", base.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("cannot inspect {}", path.display()))?;
        if file_type.is_symlink() {
            continue;
        }
        if depth == 1 {
            if file_type.is_dir() || file_type.is_file() {
                found.push(path);
            }
        } else if file_type.is_dir() {
            collect_at_depth(&path, depth - 1, found)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::project::ScanObservations;
    use std::os::unix::fs::symlink;
    use std::time::Duration;

    fn test_ctx(home: PathBuf) -> Ctx {
        Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            active_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home,
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        }
    }

    fn write_aged(path: &Path, contents: &str, days: u64) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
        let stale = SystemTime::now() - Duration::from_secs(DAY * days);
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(stale)
            .unwrap();
    }

    fn write_codex_release(home: &Path, version: &str) -> PathBuf {
        let release = home.join(format!(
            ".codex/packages/standalone/releases/{version}-aarch64-apple-darwin"
        ));
        std::fs::create_dir_all(release.join("bin")).unwrap();
        std::fs::create_dir_all(release.join("codex-resources")).unwrap();
        std::fs::create_dir_all(release.join("codex-path")).unwrap();
        for binary in ["bin/codex", "bin/codex-code-mode-host", "codex-path/rg"] {
            let path = release.join(binary);
            std::fs::write(&path, "binary").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        symlink("bin/codex", release.join("codex")).unwrap();
        let voice = release.join("codex-resources/voice");
        std::fs::create_dir_all(&voice).unwrap();
        std::fs::write(voice.join("runtime.json"), "{}").unwrap();
        std::fs::write(
            voice.join("manifest.json"),
            serde_json::json!({
                "schemaVersion": 1,
                "appVersion": version,
                "appTarget": "aarch64-apple-darwin",
                "voiceTarget": "aarch64-apple-darwin",
                "sha256": {
                    "bin/codex": "9a3a45d01531a20e89ac6ae10b0b0beb0492acd7216a368aa062d1a5fecaf9cd",
                    "codex-resources/voice/runtime.json": "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a"
                }
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            release.join("codex-package.json"),
            format!(
                r#"{{"layoutVersion":1,"version":"{version}","target":"aarch64-apple-darwin","variant":"codex","entrypoint":"bin/codex","resourcesDir":"codex-resources","pathDir":"codex-path"}}"#
            ),
        )
        .unwrap();
        release
    }

    #[test]
    fn codex_standalone_offers_only_an_obsolete_release() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-codex-releases")
            .tempdir()
            .unwrap();
        let home = home.path();
        let old = write_codex_release(home, "0.155.1");
        let current = write_codex_release(home, "0.156.1");
        let newer = write_codex_release(home, "0.157.0");
        let same_core_beta = write_codex_release(home, "0.156.1-beta.1");
        let standalone = home.join(".codex/packages/standalone");
        std::fs::write(standalone.join("install.lock"), "").unwrap();
        symlink(&current, standalone.join("current")).unwrap();
        let staging = standalone.join("releases/.staging.0.157.0-aarch64-apple-darwin.123");
        std::fs::create_dir_all(&staging).unwrap();

        let findings = Agents
            .scan(&test_ctx(home.to_path_buf()), &ScanObservations::default())
            .unwrap();
        let targets: Vec<_> = findings.iter().filter_map(Finding::target).collect();
        assert!(
            targets.contains(&old.as_path()),
            "old release must be offered"
        );
        assert!(
            !targets.contains(&current.as_path()),
            "current release must survive"
        );
        assert!(
            !targets.contains(&newer.as_path()),
            "a newer installed release is not superseded"
        );
        assert!(
            !targets.contains(&same_core_beta.as_path()),
            "same-core prerelease is kept conservatively"
        );
        assert!(
            !targets.contains(&staging.as_path()),
            "installer staging must survive"
        );
    }

    #[test]
    fn codex_releases_fail_closed_without_a_valid_current_link_or_install_lock() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-codex-current")
            .tempdir()
            .unwrap();
        let home = home.path();
        let old = write_codex_release(home, "0.155.1");
        let current = write_codex_release(home, "0.156.1");
        let standalone = home.join(".codex/packages/standalone");
        let cache = home.join(".codex/cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("catalog.json"), "{}").unwrap();
        let ctx = test_ctx(home.to_path_buf());

        let blocked = |ctx: &Ctx| {
            let findings = Agents.scan(ctx, &ScanObservations::default()).unwrap();
            assert!(
                findings.iter().any(|finding| {
                    finding.label == "Codex standalone releases unavailable"
                        && finding.action == Action::Info
                }),
                "PV agents/codex-current-executable: invalid current must refuse release cleanup"
            );
            assert!(
                !findings
                    .iter()
                    .any(|finding| finding.target() == Some(old.as_path()))
            );
            assert!(
                findings
                    .iter()
                    .any(|finding| finding.target() == Some(cache.as_path()))
            );
        };
        blocked(&ctx);
        std::fs::write(standalone.join("install.lock"), "").unwrap();
        blocked(&ctx);
        let outside = home.join("outside");
        std::fs::create_dir(&outside).unwrap();
        let foreign = outside.join(current.file_name().unwrap());
        std::fs::rename(&current, &foreign).unwrap();
        symlink(&foreign, standalone.join("current")).unwrap();
        blocked(&ctx);
        std::fs::remove_file(standalone.join("current")).unwrap();
        std::fs::rename(&foreign, &current).unwrap();
        symlink(&current, standalone.join("current")).unwrap();
        std::fs::remove_file(current.join("bin/codex-code-mode-host")).unwrap();
        blocked(&ctx);
        std::fs::write(current.join("bin/codex-code-mode-host"), "binary").unwrap();
        std::fs::set_permissions(
            current.join("bin/codex-code-mode-host"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::set_permissions(
            current.join("bin/codex"),
            std::fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        blocked(&ctx);
        std::fs::set_permissions(
            current.join("bin/codex"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::set_permissions(
            current.join("bin/codex"),
            std::fs::Permissions::from_mode(0o001),
        )
        .unwrap();
        blocked(&ctx);
        std::fs::set_permissions(
            current.join("bin/codex"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::fs::write(current.join("bin/codex"), "").unwrap();
        blocked(&ctx);
        std::fs::write(current.join("bin/codex"), "binary").unwrap();
        std::fs::create_dir(standalone.join("install.lock.d")).unwrap();
        blocked(&ctx);
        std::fs::remove_dir(standalone.join("install.lock.d")).unwrap();
        let manifest = old.join("codex-package.json");
        let contents = std::fs::read(&manifest).unwrap();
        std::fs::write(&manifest, "{").unwrap();
        blocked(&ctx);
        std::fs::write(&manifest, contents).unwrap();
        let malformed_current = write_codex_release(home, "0.156.1-");
        std::fs::remove_file(standalone.join("current")).unwrap();
        symlink(&malformed_current, standalone.join("current")).unwrap();
        blocked(&ctx);
        std::fs::remove_file(standalone.join("current")).unwrap();
        symlink(&current, standalone.join("current")).unwrap();
        let findings = Agents.scan(&ctx, &ScanObservations::default()).unwrap();
        assert!(
            findings
                .iter()
                .any(|finding| finding.target() == Some(old.as_path()))
        );
    }

    #[test]
    fn codex_installer_lock_blocks_release_preview() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-codex-locked")
            .tempdir()
            .unwrap();
        let home = home.path();
        let old = write_codex_release(home, "0.155.1");
        let current = write_codex_release(home, "0.156.1");
        let standalone = home.join(".codex/packages/standalone");
        let lock_path = standalone.join("install.lock");
        std::fs::write(&lock_path, "").unwrap();
        symlink(&current, standalone.join("current")).unwrap();
        let lock = File::open(&lock_path).unwrap();
        flock(&lock, FlockOperation::NonBlockingLockExclusive).unwrap();
        let ctx = test_ctx(home.to_path_buf());
        let findings = Agents.scan(&ctx, &ScanObservations::default()).unwrap();
        assert!(findings.iter().any(|finding| {
            finding.action == Action::Info && finding.note.contains("installer may be active")
        }));
        assert!(
            !findings
                .iter()
                .any(|finding| finding.target() == Some(old.as_path()))
        );
        flock(&lock, FlockOperation::Unlock).unwrap();
        drop(lock);
        let findings = Agents.scan(&ctx, &ScanObservations::default()).unwrap();
        assert!(
            findings
                .iter()
                .any(|finding| finding.target() == Some(old.as_path()))
        );
    }

    #[test]
    fn busy_codex_installer_refuses_only_release_apply() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-codex-busy-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(&home).unwrap();
        let old = write_codex_release(&home, "0.155.1");
        let current = write_codex_release(&home, "0.156.1");
        let standalone = home.join(".codex/packages/standalone");
        let lock_path = standalone.join("install.lock");
        std::fs::write(&lock_path, "").unwrap();
        symlink(&current, standalone.join("current")).unwrap();
        let cache = home.join(".codex/cache");
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("catalog.json"), "{}").unwrap();
        let ctx = test_ctx(home.clone());
        let mut findings = Agents.scan(&ctx, &ScanObservations::default()).unwrap();
        findings.retain(|finding| {
            finding.target() == Some(old.as_path()) || finding.target() == Some(cache.as_path())
        });
        assert_eq!(findings.len(), 2);
        for finding in &mut findings {
            finding.action = Action::Shred;
        }
        let lock = File::open(&lock_path).unwrap();
        flock(&lock, FlockOperation::NonBlockingLockExclusive).unwrap();

        let outcome = Agents.apply(&findings, &ctx).unwrap();
        assert_eq!(outcome.summary.items_touched, 1);
        assert_eq!(outcome.errors.len(), 1);
        assert!(!cache.exists(), "unrelated cache still applies");
        assert!(old.exists(), "busy release must survive");
        drop(lock);
        crate::ops::remove_test_path(home);
    }

    #[test]
    fn codex_release_scan_rejects_unknown_contents_and_symlinked_candidates() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-codex-shape")
            .tempdir()
            .unwrap();
        let home = home.path();
        let old = write_codex_release(home, "0.155.1");
        let unknown = write_codex_release(home, "0.154.0");
        std::fs::write(unknown.join("user-notes.txt"), "keep").unwrap();
        let incomplete = write_codex_release(home, "0.152.0");
        std::fs::remove_file(incomplete.join("bin/codex")).unwrap();
        let nested = write_codex_release(home, "0.153.0");
        std::fs::write(nested.join("bin/user-notes.txt"), "keep").unwrap();
        let resources = write_codex_release(home, "0.150.0");
        std::fs::write(resources.join("codex-resources/personal.txt"), "keep").unwrap();
        let current = write_codex_release(home, "0.156.1");
        let standalone = home.join(".codex/packages/standalone");
        std::fs::write(standalone.join("install.lock"), "").unwrap();
        symlink(&current, standalone.join("current")).unwrap();
        let linked = write_codex_release(home, "0.151.0");
        let outside = home.join("outside");
        std::fs::create_dir(&outside).unwrap();
        let linked_target = outside.join(linked.file_name().unwrap());
        std::fs::rename(&linked, &linked_target).unwrap();
        symlink(&linked_target, &linked).unwrap();

        let findings = Agents
            .scan(&test_ctx(home.to_path_buf()), &ScanObservations::default())
            .unwrap();
        let targets: Vec<_> = findings.iter().filter_map(Finding::target).collect();
        assert!(
            targets.contains(&old.as_path()),
            "eligible control must be offered"
        );
        assert!(!targets.contains(&unknown.as_path()));
        assert!(!targets.contains(&incomplete.as_path()));
        assert!(!targets.contains(&nested.as_path()));
        assert!(!targets.contains(&resources.as_path()));
        assert!(!targets.contains(&linked.as_path()));
    }

    #[test]
    fn codex_release_apply_refuses_a_version_promoted_after_preview() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-codex-promoted-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(&home).unwrap();
        let old = write_codex_release(&home, "0.154.0");
        let promoted = write_codex_release(&home, "0.155.1");
        let current = write_codex_release(&home, "0.156.1");
        let standalone = home.join(".codex/packages/standalone");
        std::fs::write(standalone.join("install.lock"), "").unwrap();
        symlink(&current, standalone.join("current")).unwrap();
        let ctx = test_ctx(home.clone());
        let mut findings = Agents.scan(&ctx, &ScanObservations::default()).unwrap();
        findings.retain(|finding| {
            finding.target() == Some(promoted.as_path()) || finding.target() == Some(old.as_path())
        });
        assert_eq!(
            findings.len(),
            2,
            "both versions must be eligible before the promotion"
        );
        for finding in &mut findings {
            finding.action = Action::Shred;
        }
        std::fs::remove_file(standalone.join("current")).unwrap();
        symlink(&promoted, standalone.join("current")).unwrap();

        let outcome = Agents.apply(&findings, &ctx).unwrap();
        assert_eq!(
            outcome.summary.items_touched, 1,
            "the still-obsolete control should be removed"
        );
        assert_eq!(outcome.errors.len(), 1);
        assert!(outcome.errors[0].contains("no longer a verified obsolete package"));
        assert!(promoted.exists(), "new current release must survive");
        assert!(!old.exists(), "still-obsolete control must be removed");
        crate::ops::remove_test_path(home);
    }

    /// The const assertion above rejects empty and ASCII-whitespace evidence
    /// while compiling, so those cases never reach a test — the crate does not
    /// build at all. What is left for a test is the gap that assertion cannot
    /// see: `is_ascii_whitespace` accepts non-ASCII blanks such as U+00A0, which
    /// `str::trim` strips. That is the only case these loops can catch. Both
    /// are proven separately — `evidence/agents-regenerable` for `REGENERABLE`
    /// and `evidence/agents-history` for `HISTORY`. Each plant is visible only
    /// to the loop over its own list, so a loop without its own case could be
    /// deleted silently; the distinct markers keep the gate's attribution
    /// honest about which one actually fired.
    #[test]
    fn every_agent_entry_carries_evidence() {
        for entry in REGENERABLE {
            assert!(
                !entry.evidence.trim().is_empty(),
                "PV evidence/agents-regenerable: missing deletion evidence: {}",
                entry.relative
            );
        }
        for root in HISTORY {
            assert!(
                !root.evidence.trim().is_empty(),
                "PV evidence/agents-history: missing deletion evidence: {}",
                root.relative
            );
        }
    }

    #[test]
    fn regenerable_caches_are_offered_and_credentials_are_not() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-agents-cache")
            .tempdir()
            .unwrap();
        let home = home.path();
        write_aged(&home.join(".claude/cache/changelog.md"), "# 1.0.0\n", 400);
        // Everything an agent needs to keep must be invisible to the scanner.
        for secret in [
            ".claude/.credentials.json",
            ".claude/settings.json",
            ".codex/auth.json",
            ".codex/config.toml",
            ".claude/backups/.claude.json.backup.1",
        ] {
            write_aged(&home.join(secret), "secret", 400);
        }

        let findings = Agents
            .scan(&test_ctx(home.to_path_buf()), &ScanObservations::default())
            .unwrap();

        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].target(),
            Some(home.join(".claude/cache").as_path())
        );
        assert_eq!(findings[0].action, Action::Trash);
        for secret in [
            ".claude/.credentials.json",
            ".claude/settings.json",
            ".codex/auth.json",
            ".codex/config.toml",
            ".claude/backups/.claude.json.backup.1",
        ] {
            assert!(home.join(secret).exists());
        }
    }

    #[test]
    fn history_is_offered_only_after_the_active_window() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-agents-history")
            .tempdir()
            .unwrap();
        let home = home.path();
        // Codex nests three levels; the day directory is the unit.
        write_aged(
            &home.join(".codex/sessions/2020/01/02/rollout.jsonl"),
            "old",
            400,
        );
        // A loose file at the configured depth is a candidate; the fresh one
        // beside it is the control proving the age gate is what excludes it.
        write_aged(&home.join(".codex/archived_sessions/old.jsonl"), "old", 400);
        write_aged(&home.join(".codex/archived_sessions/new.jsonl"), "new", 0);

        let findings = Agents
            .scan(&test_ctx(home.to_path_buf()), &ScanObservations::default())
            .unwrap();

        let targets: Vec<_> = findings.iter().filter_map(Finding::target).collect();
        assert!(targets.contains(&home.join(".codex/sessions/2020/01/02").as_path()));
        assert!(targets.contains(&home.join(".codex/archived_sessions/old.jsonl").as_path()));
        assert!(!targets.contains(&home.join(".codex/archived_sessions/new.jsonl").as_path()));
        // The root itself is never a finding, only its children at the configured depth.
        assert!(!targets.contains(&home.join(".codex/sessions").as_path()));
        assert!(!targets.contains(&home.join(".codex/sessions/2020").as_path()));
        assert!(
            findings
                .iter()
                .any(|finding| finding.note.contains("not regenerable"))
        );
    }

    /// `~/.claude/projects` is not a cleanup root at all, and this is the
    /// assertion that keeps it that way. It holds Claude Code auto memory in
    /// `<project>/memory/`, and since the transcripts beside it can originate in
    /// Claude Desktop — which the vendor retains at any age — file age is not
    /// evidence that anything there is finished with. A `.codex` transcript of
    /// identical shape and age is offered in the same run, so this proves an
    /// exclusion rather than an inert fixture.
    #[test]
    fn the_claude_projects_tree_is_never_a_candidate() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-agents-projects")
            .tempdir()
            .unwrap();
        let home = home.path();
        let session = "24bb0c93-fbb9-49b7-99b0-7a97be87baeb";
        let project = home.join(".claude/projects/-repo");
        write_aged(&project.join(format!("{session}.jsonl")), "old", 400);
        write_aged(&project.join("memory/MEMORY.md"), "durable fact", 400);
        let codex = home.join(".codex/archived_sessions");
        write_aged(&codex.join(format!("rollout-{session}.jsonl")), "old", 400);

        let ctx = test_ctx(home.to_path_buf());
        let findings = Agents.scan(&ctx, &ScanObservations::default()).unwrap();
        let targets: Vec<_> = findings.iter().filter_map(Finding::target).collect();

        assert!(
            targets.contains(&codex.join(format!("rollout-{session}.jsonl")).as_path()),
            "a transcript of the same shape and age elsewhere must still be offered"
        );
        for excluded in [
            home.join(".claude/projects"),
            project.clone(),
            project.join("memory"),
        ] {
            assert!(
                !targets.contains(&excluded.as_path()),
                "{}",
                excluded.display()
            );
            let outcome = Agents
                .apply(
                    &[Finding::new(
                        "forged",
                        Some(excluded.clone()),
                        4,
                        "test",
                        6,
                        Action::Trash,
                    )],
                    &ctx,
                )
                .unwrap();
            assert_eq!(outcome.summary.items_touched, 0, "{}", excluded.display());
            assert_eq!(outcome.errors.len(), 1, "{}", excluded.display());
        }
        assert_eq!(
            std::fs::read_to_string(project.join("memory/MEMORY.md")).unwrap(),
            "durable fact"
        );
    }

    /// SECURITY.md states as a non-negotiable boundary that `~/.claude/projects`
    /// and `~/.claude/jobs` are not cleanup roots, so nothing beneath either can
    /// become a target. Both were roots at some point during development and
    /// both were retired after review found live or unjudgeable data inside, so
    /// the boundary needs to be executable rather than prose: re-adding either
    /// path to a list must fail here. The structural half catches it at the
    /// list, the behavioural half at the scan, and the `.codex` fixture is the
    /// control proving the scan was capable of returning something.
    #[test]
    fn the_retired_claude_trees_can_never_become_roots_again() {
        for retired in [".claude/projects", ".claude/jobs"] {
            assert!(
                !REGENERABLE
                    .iter()
                    .any(|entry| entry.relative.starts_with(retired)),
                "{retired} must never be a regenerable entry"
            );
            assert!(
                !HISTORY
                    .iter()
                    .any(|root| root.relative.starts_with(retired)),
                "{retired} must never be a history root"
            );
        }

        let home = tempfile::Builder::new()
            .prefix("devtrim-agents-retired")
            .tempdir()
            .unwrap();
        let home = home.path();
        let session = "24bb0c93-fbb9-49b7-99b0-7a97be87baeb";
        write_aged(
            &home.join(format!(".claude/projects/-repo/{session}.jsonl")),
            "old",
            400,
        );
        write_aged(
            &home.join(".claude/projects/-repo/memory/MEMORY.md"),
            "durable fact",
            400,
        );
        write_aged(&home.join(".claude/jobs/pins.json"), "[]", 400);
        write_aged(&home.join(".claude/jobs/abc123/state.json"), "{}", 400);
        let control = home.join(format!(".codex/archived_sessions/rollout-{session}.jsonl"));
        write_aged(&control, "old", 400);

        let findings = Agents
            .scan(&test_ctx(home.to_path_buf()), &ScanObservations::default())
            .unwrap();
        let targets: Vec<_> = findings.iter().filter_map(Finding::target).collect();

        assert!(
            targets.contains(&control.as_path()),
            "control: a stale transcript under a live root must still be offered"
        );
        // Both directions matter. A target *beneath* a retired tree deletes part
        // of it; a target that is an *ancestor* of one — a `.claude` entry, say —
        // deletes the whole thing while never starting with the retired path.
        for retired in [home.join(".claude/projects"), home.join(".claude/jobs")] {
            for target in &targets {
                assert!(
                    !target.starts_with(&retired) && !retired.starts_with(target),
                    "{} must not be reachable through {}",
                    retired.display(),
                    target.display()
                );
            }
        }

        // Scanning cannot delete, so survival has to be proven against apply —
        // and the refusal has to name the retired tree. A memory file is a
        // regular file with an ordinary Trash action, so neither the file-type
        // gate nor the action check can stand in for the boundary; only the
        // namespace check can refuse it.
        let memory = home.join(".claude/projects/-repo/memory/MEMORY.md");
        let outcome = Agents
            .apply(
                &[Finding::new(
                    "forged",
                    Some(memory.clone()),
                    4,
                    "test",
                    6,
                    Action::Trash,
                )],
                &test_ctx(home.to_path_buf()),
            )
            .unwrap();
        // Reason first, counts after: a mutation that removes `authorize` must
        // fail HERE, not on a downstream refusal or a survival assertion that
        // happens to hold for an unrelated reason. Keyed on the reason phrase
        // rather than on `.claude/projects`, which is a substring of the forged
        // target path and would be satisfied by any refusal echoing it.
        assert!(
            outcome
                .errors
                .first()
                .is_some_and(|error| error.contains("outside its authorized namespace")),
            "PV agents/apply-namespace: expected namespace refusal, got {:?}",
            outcome.errors
        );
        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(std::fs::read_to_string(&memory).unwrap(), "durable fact");
    }

    /// A shell snapshot is sourced by every later shell call in the session that
    /// wrote it, and nothing rewrites it, so it belongs to the age-gated tier.
    #[test]
    fn shell_snapshots_are_age_gated_rather_than_offered_outright() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-agents-snapshots")
            .tempdir()
            .unwrap();
        let home = home.path();
        let snapshots = home.join(".claude/shell-snapshots");
        write_aged(
            &snapshots.join("snapshot-zsh-1-old.sh"),
            "export A=1\n",
            400,
        );
        write_aged(&snapshots.join("snapshot-zsh-2-live.sh"), "export B=2\n", 0);

        let findings = Agents
            .scan(&test_ctx(home.to_path_buf()), &ScanObservations::default())
            .unwrap();
        let targets: Vec<_> = findings.iter().filter_map(Finding::target).collect();

        assert!(targets.contains(&snapshots.join("snapshot-zsh-1-old.sh").as_path()));
        assert!(!targets.contains(&snapshots.join("snapshot-zsh-2-live.sh").as_path()));
        assert!(
            !targets.contains(&snapshots.as_path()),
            "the directory itself must never be offered wholesale"
        );
    }

    /// A session resumed between preview and apply is an expected refusal, and
    /// the documented promise is that it falls out of the plan — not that it
    /// takes every later finding with it.
    #[test]
    fn a_resumed_session_does_not_block_the_rest_of_the_plan() {
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-agents-partial-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(&home).unwrap();
        let home = home.canonicalize().unwrap();
        let session = "24bb0c93-fbb9-49b7-99b0-7a97be87baeb";
        let archived = home.join(".codex/archived_sessions");
        let resumed = archived.join(format!("rollout-resumed-{session}.jsonl"));
        let stale = archived.join(format!("rollout-stale-{session}.jsonl"));
        write_aged(&resumed, "old", 400);
        write_aged(&stale, "old", 400);
        let ctx = test_ctx(home.clone());
        let findings = vec![
            Finding::new(
                "resumed",
                Some(resumed.clone()),
                4,
                "test",
                6,
                Action::Shred,
            ),
            Finding::new("stale", Some(stale.clone()), 4, "test", 6, Action::Shred),
        ];
        // The resumed session is written after the plan was built, exactly as a
        // live agent would; its inode is unchanged, so only the age re-read sees it.
        std::fs::write(&resumed, "resumed").unwrap();

        let outcome = Agents.apply(&findings, &ctx).unwrap();

        assert_eq!(
            outcome.errors.len(),
            1,
            "the refusal must still be reported"
        );
        assert!(
            outcome.errors[0].contains("no longer meets its preview shape"),
            "an age refusal must not read as a forged target: {}",
            outcome.errors[0]
        );
        assert_eq!(
            outcome.summary.items_touched, 1,
            "the finding listed after the resumed one must still be removed"
        );
        assert!(resumed.exists(), "the resumed session must survive");
        assert!(!stale.exists());
        crate::ops::remove_test_path(home);
    }

    /// Positive control for the apply-time boundary. The forged targets are
    /// exactly the shapes a compromised or buggy preview could produce: a path
    /// outside every root, the root itself, and a descendant below the
    /// configured depth.
    #[test]
    fn apply_refuses_forged_targets_and_preserves_them() {
        // The deletion sink refuses anything under `/private/var`, so the
        // positive control needs a home the global protection list allows.
        let home = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("devtrim-agents-forged-{}", std::process::id()));
        crate::ops::remove_test_path(&home);
        std::fs::create_dir_all(&home).unwrap();
        let home = home.canonicalize().unwrap();
        write_aged(&home.join(".ssh/id_ed25519"), "PRIVATE KEY", 400);
        write_aged(
            &home.join(
                ".codex/archived_sessions/rollout-24bb0c93-fbb9-49b7-99b0-7a97be87baeb.jsonl",
            ),
            "old",
            400,
        );
        write_aged(
            &home.join(".codex/archived_sessions/nested/rollout-24bb0c93-fbb9-49b7-99b0-7a97be87baeb.jsonl"),
            "old",
            400,
        );
        let ctx = test_ctx(home.clone());

        for forged in [
            home.join(".ssh"),
            home.join(".codex"),
            home.join(".codex/archived_sessions"),
            home.join(".codex/archived_sessions/nested/rollout-24bb0c93-fbb9-49b7-99b0-7a97be87baeb.jsonl"),
        ] {
            let finding = Finding::new("forged", Some(forged.clone()), 4, "test", 9, Action::Shred);
            let outcome = Agents.apply(&[finding], &ctx).unwrap();
            assert_eq!(outcome.summary.items_touched, 0, "{}", forged.display());
            assert_eq!(outcome.errors.len(), 1, "{}", forged.display());
            assert!(forged.exists(), "{}", forged.display());
        }
        assert_eq!(
            std::fs::read_to_string(home.join(".ssh/id_ed25519")).unwrap(),
            "PRIVATE KEY"
        );

        // Positive control: the authorized shape at the same depth is accepted,
        // proving the refusals above are the boundary and not a vacuous pass.
        let authorized = home
            .join(".codex/archived_sessions/rollout-24bb0c93-fbb9-49b7-99b0-7a97be87baeb.jsonl");
        let finding = Finding::new(
            "authorized",
            Some(authorized.clone()),
            4,
            "test",
            6,
            Action::Shred,
        );
        let outcome = Agents.apply(&[finding], &ctx).unwrap();
        assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
        assert_eq!(outcome.summary.items_touched, 1);
        assert!(!authorized.exists());
        crate::ops::remove_test_path(home);
    }

    /// Apply reasserts age, not just shape. A transcript appended to after
    /// preview keeps its inode and generation, so the sink's identity check
    /// cannot see the change; only re-reading the timestamp can.
    #[test]
    fn history_that_stopped_being_stale_after_preview_is_refused() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-agents-touched")
            .tempdir()
            .unwrap();
        let home = home.path();
        let session = home.join(".codex/sessions/2020/01/02");
        write_aged(&session.join("rollout.jsonl"), "old", 400);
        assert!(history_details(&session, home, 30).unwrap().is_some());

        std::fs::write(session.join("rollout.jsonl"), "resumed").unwrap();

        assert!(
            history_details(&session, home, 30).unwrap().is_none(),
            "a resumed session must fall out of the plan"
        );
    }

    /// A symlink planted inside a live root must be refused because it is a
    /// symlink, not because it happens to sit somewhere unscanned. The link is
    /// therefore placed under `.codex/archived_sessions` — a root that is
    /// actually traversed — and an ordinary stale file beside it is the positive
    /// control: if the scan returned nothing at all, the refusal would prove
    /// nothing about symlinks.
    #[test]
    fn a_symlinked_history_child_is_refused() {
        let home = tempfile::Builder::new()
            .prefix("devtrim-agents-symlink")
            .tempdir()
            .unwrap();
        let home = home.path();
        // The link must resolve to a *regular file*. Pointed at a directory it
        // would also trip the wrong-file-type check, so removing the symlink
        // branch would leave this test green while a symlink to a stale file
        // outside the root was followed and offered.
        let outside = home.join("outside");
        let payload = outside.join("payload");
        write_aged(&payload, "keep", 400);
        let root = home.join(".codex/archived_sessions");
        let genuine = root.join("rollout-24bb0c93-fbb9-49b7-99b0-7a97be87baeb.jsonl");
        write_aged(&genuine, "old", 400);
        let link = root.join("rollout-linked.jsonl");
        std::os::unix::fs::symlink(&payload, &link).unwrap();
        // Age the link itself, not just its target: a fresh link inode would let
        // the age gate refuse it, so deleting the symlink check could leave this
        // test green while a stale symlink out of the root was followed.
        let stale = rustix::fs::Timespec {
            tv_sec: i64::try_from(
                (SystemTime::now() - Duration::from_secs(DAY * 400))
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_secs(),
            )
            .unwrap(),
            tv_nsec: 0,
        };
        rustix::fs::utimensat(
            rustix::fs::CWD,
            &link,
            &rustix::fs::Timestamps {
                last_access: stale,
                last_modification: stale,
            },
            rustix::fs::AtFlags::SYMLINK_NOFOLLOW,
        )
        .unwrap();

        assert!(is_history_child(&link, home), "the fixture must be scanned");
        assert!(
            history_details(&link, home, 30).unwrap().is_none(),
            "PV agents/history-symlink: a symlink must not be eligible"
        );

        let ctx = test_ctx(home.to_path_buf());
        let findings = Agents.scan(&ctx, &ScanObservations::default()).unwrap();
        let targets: Vec<_> = findings.iter().filter_map(Finding::target).collect();
        assert!(
            targets.contains(&genuine.as_path()),
            "positive control: the real stale transcript must be offered"
        );
        assert!(!targets.contains(&link.as_path()));

        // Apply is the boundary that matters: a forged finding naming the link
        // must be refused rather than followed out of the root.
        let outcome = Agents
            .apply(
                &[Finding::new(
                    "forged",
                    Some(link.clone()),
                    4,
                    "test",
                    6,
                    Action::Shred,
                )],
                &ctx,
            )
            .unwrap();
        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
        assert!(
            outcome.errors[0].contains("symlink"),
            "the refusal must name the symlink, not some incidental check: {}",
            outcome.errors[0]
        );
        assert_eq!(std::fs::read_to_string(&payload).unwrap(), "keep");
        assert!(std::fs::symlink_metadata(&link).unwrap().is_symlink());
    }
}
