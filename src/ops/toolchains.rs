//! swift.org toolchains under ~/Library/Developer/Toolchains.
//! Cleanup proceeds only when a valid swift-latest target, every symlink
//! reference, and every toolchain swiftly records are known; a toolchain
//! swiftly installed is never offered.

use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::project::{
    UnreadFolder, is_directory_if_present, unjudged_finding, unread_folders_finding,
};
use super::{Action, ApplyOutcome, Finding, Op, apply_filesystem_finding, dir_size, removal_note};
use crate::safety::{Ctx, escalate};

pub struct Toolchains;

impl Op for Toolchains {
    fn name(&self) -> &'static str {
        "toolchains"
    }

    fn scan(
        &self,
        ctx: &Ctx,
        _observations: &super::project::ScanObservations,
    ) -> Result<Vec<Finding>> {
        let directory = ctx.home.join("Library/Developer/Toolchains");
        if !is_directory_if_present(&directory)? {
            return Ok(Vec::new());
        }
        // A reference that cannot be proven withholds the whole category: the
        // toolchain it could not name may be any of them.
        let references = match References::read(&ctx.home, &directory) {
            Ok(references) => references,
            Err(error) => {
                return Ok(vec![unjudged_finding(
                    "Swift toolchains",
                    &directory,
                    &error,
                )]);
            }
        };
        let mut findings = Vec::new();
        // A toolchain that cannot be inspected or measured offers nothing and
        // is named in one error finding (S8); the others are still judged.
        let mut unread = Vec::new();
        for entry in std::fs::read_dir(&directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if !file_type.is_dir()
                || path
                    .extension()
                    .is_none_or(|extension| extension != "xctoolchain")
            {
                continue;
            }
            match toolchain_finding(path.clone(), &references) {
                Ok(Some(finding)) => findings.push(finding),
                Ok(None) => {}
                Err(error) => unread.push(UnreadFolder::new(&path, &error)),
            }
        }
        if !unread.is_empty() {
            findings.push(unread_folders_finding("Swift toolchains", &unread));
        }
        Ok(findings)
    }

    fn apply(&self, findings: &[Finding], ctx: &Ctx) -> Result<ApplyOutcome> {
        let directory = ctx.home.join("Library/Developer/Toolchains");
        let references = References::read(&ctx.home, &directory)?;
        let mut outcome = ApplyOutcome::new(self.name());
        for finding in findings {
            if !matches!(finding.action, Action::Trash | Action::Shred) {
                continue;
            }
            let result = (|| -> Result<String> {
                let path = finding
                    .target()
                    .ok_or_else(|| anyhow::anyhow!("toolchain finding missing internal target"))?;
                authorize_toolchain_target(path, &directory, &references)?;
                apply_filesystem_finding(self.name(), finding, ctx)?;
                Ok(removal_note(finding, path.display()))
            })();
            match result {
                Ok(note) => outcome.record(finding, note),
                Err(error) => {
                    outcome.fail(error);
                    break;
                }
            }
        }
        Ok(outcome)
    }
}

/// Everything that says a toolchain is in use, read afresh by each scan and
/// each apply.
/// The finding for one toolchain folder: report-only when swiftly installed
/// it, none when a verified symlink references it, otherwise offered.
fn toolchain_finding(path: PathBuf, references: &References) -> Result<Option<Finding>> {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let metadata = std::fs::symlink_metadata(&path)
        .with_context(|| format!("cannot verify toolchain {}", path.display()))?;
    if let Some(owner) = references.swiftly.get(&(metadata.dev(), metadata.ino())) {
        let size = dir_size(&path)?;
        let state = if owner.in_use { " and in use" } else { "" };
        return Ok(Some(Finding::new(
            format!("Swift toolchain {name} (swiftly)"),
            Some(path),
            size,
            format!(
                "EXCLUDED: installed by swiftly{state}; `swiftly uninstall {}` removes it",
                owner.version
            ),
            0,
            Action::None,
        )));
    }
    let canonical = path
        .canonicalize()
        .with_context(|| format!("cannot verify toolchain {}", path.display()))?;
    if references.linked.contains(&canonical) {
        return Ok(None);
    }
    let size = dir_size(&path)?;
    Ok(Some(Finding::new(
        format!("Swift toolchain {name}"),
        Some(path),
        size,
        "not referenced by any verified Toolchains symlink or by swiftly; reinstallable from swift.org",
        escalate(6, size),
        Action::Trash,
    )))
}

struct References {
    /// Canonical targets of the verified symlinks in the Toolchains folder.
    linked: BTreeSet<PathBuf>,
    /// Toolchains swiftly records, by the device and inode of their folder, so
    /// a spelling the volume folds still names the same folder.
    swiftly: BTreeMap<(u64, u64), SwiftlyToolchain>,
}

impl References {
    fn read(home: &Path, directory: &Path) -> Result<Self> {
        Ok(Self {
            linked: linked_targets(directory)?,
            swiftly: swiftly_toolchains(home, directory)?,
        })
    }
}

fn linked_targets(directory: &Path) -> Result<BTreeSet<PathBuf>> {
    let canonical_directory = directory
        .canonicalize()
        .with_context(|| format!("cannot verify {}", directory.display()))?;
    let latest = directory.join("swift-latest.xctoolchain");
    let latest_type = std::fs::symlink_metadata(&latest)
        .with_context(|| {
            format!(
                "cannot prove required Swift toolchain link {}",
                latest.display()
            )
        })?
        .file_type();
    if !latest_type.is_symlink() {
        anyhow::bail!(
            "required Swift toolchain reference is not a symlink: {}",
            latest.display()
        );
    }
    let mut preserved = BTreeSet::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_symlink() {
            continue;
        }
        let link = entry.path();
        let target = std::fs::read_link(&link)
            .with_context(|| format!("cannot read symlink {}", link.display()))?;
        let target = if target.is_absolute() {
            target
        } else {
            directory.join(target)
        };
        let canonical = target.canonicalize().with_context(|| {
            format!(
                "cannot resolve Swift toolchain reference {}",
                link.display()
            )
        })?;
        if canonical.parent() != Some(canonical_directory.as_path())
            || !canonical.is_dir()
            || canonical
                .extension()
                .is_none_or(|extension| extension != "xctoolchain")
        {
            anyhow::bail!(
                "Swift toolchain reference escapes the verified Toolchains directory: {}",
                link.display()
            );
        }
        preserved.insert(canonical);
    }
    let latest_target = latest.canonicalize().with_context(|| {
        format!(
            "cannot resolve required Swift toolchain link {}",
            latest.display()
        )
    })?;
    if !preserved.contains(&latest_target) {
        anyhow::bail!(
            "required Swift toolchain reference is not in the verified target set: {}",
            latest.display()
        );
    }
    Ok(preserved)
}

/// A toolchain swiftly installed, as its configuration names it.
struct SwiftlyToolchain {
    /// The version swiftly's own commands take: `6.3.3`, `main-snapshot-…`.
    version: String,
    in_use: bool,
}

/// The two fields of swiftly's `config.json` that say which toolchains it owns
/// (swiftlang/swiftly 1.1.4 `Sources/Swiftly/Config.swift`: `inUse:
/// ToolchainVersion?` and `installedToolchains: Set<ToolchainVersion>`, each
/// encoded as its `name`). swiftly's other fields are ignored.
#[derive(serde::Deserialize)]
struct SwiftlyConfig {
    #[serde(rename = "inUse")]
    in_use: Option<String>,
    #[serde(rename = "installedToolchains")]
    installed_toolchains: Vec<String>,
}

/// Every toolchain folder in `directory` that swiftly records.
///
/// swiftly, the installer swift.org recommends on macOS, selects the toolchain
/// in use through its `config.json` and its `~/.swiftly/bin` proxies rather
/// than a symlink here, so the symlinks alone cannot say which toolchain is in
/// use. swiftly reads `$SWIFTLY_HOME_DIR/config.json`, by default
/// `~/.swiftly/config.json` (swiftly 1.1.4 `Sources/SwiftlyCore/Platform.swift`
/// `swiftlyHomeDir`, `swiftlyConfigFile`; `SWIFTLY_HOME_DIR` is documented in
/// `Documentation/SwiftlyDocs.docc/automated-install.md`). No configuration
/// means swiftly is not set up and records nothing. One that cannot be read,
/// parsed, or mapped to folder names refuses: the toolchain it could not name
/// may be the one in use.
fn swiftly_toolchains(
    home: &Path,
    directory: &Path,
) -> Result<BTreeMap<(u64, u64), SwiftlyToolchain>> {
    let config = match std::env::var_os("SWIFTLY_HOME_DIR") {
        Some(swiftly_home) => {
            let swiftly_home = PathBuf::from(swiftly_home);
            if !swiftly_home.is_absolute() {
                anyhow::bail!(
                    "refusing Swift toolchains: SWIFTLY_HOME_DIR is not an absolute path, so swiftly's configuration cannot be located: {}",
                    swiftly_home.display()
                );
            }
            swiftly_home.join("config.json")
        }
        None => home.join(".swiftly/config.json"),
    };
    let contents = match std::fs::read(&config) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BTreeMap::new());
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!("cannot read swiftly's configuration {}", config.display())
            });
        }
    };
    let parsed: SwiftlyConfig = serde_json::from_slice(&contents)
        .with_context(|| format!("cannot parse swiftly's configuration {}", config.display()))?;
    let mut versions = parsed.installed_toolchains;
    // The toolchain in use is kept even when the installed list omits it.
    versions.extend(parsed.in_use.clone());
    let mut owned = BTreeMap::new();
    for version in versions {
        let names = swiftly_folder_names(&version).with_context(|| {
            format!(
                "refusing Swift toolchains: swiftly's configuration {} names a toolchain devtrim cannot map to a folder",
                config.display()
            )
        })?;
        for name in names {
            let folder = directory.join(format!("{name}.xctoolchain"));
            let metadata = match std::fs::metadata(&folder) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("cannot inspect swiftly toolchain {}", folder.display())
                    });
                }
            };
            owned.insert(
                (metadata.dev(), metadata.ino()),
                SwiftlyToolchain {
                    in_use: parsed.in_use.as_deref() == Some(version.as_str()),
                    version: version.clone(),
                },
            );
        }
    }
    Ok(owned)
}

/// The `.xctoolchain` folder names, without the extension, swiftly has given
/// `version` on macOS. swiftly installs `<identifier>.xctoolchain` into the
/// Toolchains folder and uninstalls by that name (swiftly 1.1.4
/// `Sources/MacOSPlatform/MacOS.swift` `install`, `uninstall`); the identifier
/// is `identifier` in `Sources/SwiftlyCore/ToolchainVersion.swift`. It changed
/// in 1.1: a stable `X.Y.0` from 6.4.0 on is `swift-X.Y.0-RELEASE`, where 1.0.0
/// named every `X.Y.0` `swift-X.Y-RELEASE`, so both are kept for a `.0`
/// release, and a folder that does not exist costs nothing. `xcode` is Xcode's
/// own toolchain, which swiftly finds through `xcrun` (`findToolchainLocation`),
/// never a folder here. Any spelling swiftly's `init(parsing:)` would reject is
/// an error.
fn swiftly_folder_names(version: &str) -> Result<Vec<String>> {
    fn number(part: &str) -> Option<u64> {
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    }
    fn is_date(part: &str) -> bool {
        part.len() == 10
            && part.bytes().enumerate().all(|(index, byte)| match index {
                4 | 7 => byte == b'-',
                _ => byte.is_ascii_digit(),
            })
    }
    if version == "xcode" {
        return Ok(Vec::new());
    }
    if let Some(day) = version.strip_prefix("main-snapshot-")
        && is_date(day)
    {
        return Ok(vec![format!("swift-DEVELOPMENT-SNAPSHOT-{day}-a")]);
    }
    let release = version.strip_prefix("Swift ").unwrap_or(version);
    if let Some((branch, day)) = release.split_once("-snapshot-")
        && is_date(day)
    {
        let parts = branch.split('.').collect::<Vec<_>>();
        let patch = match parts.as_slice() {
            [_, _] => Some(String::new()),
            [_, _, patch]
                if !patch.is_empty() && patch.bytes().all(|byte| byte.is_ascii_alphanumeric()) =>
            {
                Some(format!(".{patch}"))
            }
            _ => None,
        };
        if let (Some(major), Some(minor), Some(patch)) = (
            parts.first().copied().and_then(number),
            parts.get(1).copied().and_then(number),
            patch,
        ) {
            return Ok(vec![format!(
                "swift-{major}.{minor}{patch}-DEVELOPMENT-SNAPSHOT-{day}-a"
            )]);
        }
    }
    if let [major, minor, patch] = release.split('.').collect::<Vec<_>>().as_slice()
        && let (Some(major), Some(minor), Some(patch)) =
            (number(major), number(minor), number(patch))
    {
        let mut names = vec![format!("swift-{major}.{minor}.{patch}-RELEASE")];
        if patch == 0 {
            names.push(format!("swift-{major}.{minor}-RELEASE"));
        }
        return Ok(names);
    }
    anyhow::bail!("unrecognized swiftly toolchain version `{version}`")
}

fn authorize_toolchain_target(
    path: &Path,
    directory: &Path,
    references: &References,
) -> Result<PathBuf> {
    if path.parent() != Some(directory) {
        anyhow::bail!(
            "refusing toolchain target outside the direct Toolchains children: {}",
            path.display()
        );
    }
    let metadata = std::fs::symlink_metadata(path)
        .with_context(|| format!("cannot re-verify toolchain {}", path.display()))?;
    if !metadata.file_type().is_dir()
        || path
            .extension()
            .is_none_or(|extension| extension != "xctoolchain")
    {
        anyhow::bail!(
            "refusing non-directory or non-.xctoolchain target: {}",
            path.display()
        );
    }
    if let Some(owner) = references.swiftly.get(&(metadata.dev(), metadata.ino())) {
        anyhow::bail!(
            "refusing a toolchain swiftly installed: {}; `swiftly uninstall {}` removes it",
            path.display(),
            owner.version
        );
    }
    let canonical_directory = directory
        .canonicalize()
        .with_context(|| format!("cannot re-verify {}", directory.display()))?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("cannot re-verify toolchain {}", path.display()))?;
    if canonical.parent() != Some(canonical_directory.as_path()) {
        anyhow::bail!(
            "refusing toolchain outside the verified Toolchains directory: {}",
            path.display()
        );
    }
    if references.linked.contains(&canonical) {
        anyhow::bail!(
            "toolchain became referenced after preview: {}",
            path.display()
        );
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn target_temp(name: &str) -> (crate::ops::TestFixture, PathBuf) {
        let fixture = crate::ops::TestFixture::new(&format!("devtrim-toolchains-{name}"));
        let path = fixture.path().to_path_buf();
        std::fs::create_dir_all(&path).unwrap();
        (fixture, path)
    }

    fn test_context(home: PathBuf) -> Ctx {
        Ctx {
            yes: true,
            yolo: false,
            json: false,
            roots: Vec::new(),
            roots_origin: crate::safety::RootsOrigin::Default,
            active_days: 30,
            retain_days: 30,
            protect: Vec::new(),
            journal_path: home.join("journal.jsonl"),
            home,
            interactive: false,
            diagnostic_output: crate::safety::DiagnosticOutput::Capture,
            diagnostics: Default::default(),
            journal_errors: Default::default(),
        }
    }

    /// A toolchain that cannot be measured offers nothing and is named in one
    /// error finding; the toolchains beside it are still judged.
    #[test]
    fn an_unmeasurable_toolchain_withholds_only_itself() {
        let (_fixture, root) = target_temp("unread");
        let home = root.canonicalize().unwrap();
        let directory = home.join("Library/Developer/Toolchains");
        let readable = directory.join("swift-1.xctoolchain");
        std::fs::create_dir_all(readable.join("usr/bin")).unwrap();
        std::fs::write(readable.join("usr/bin/swift"), "binary").unwrap();
        let sealed = crate::ops::Unreadable::new(&directory.join("swift-2.xctoolchain/usr"), 0o000);
        std::fs::create_dir_all(directory.join("swift-3.xctoolchain")).unwrap();
        symlink(
            "swift-3.xctoolchain",
            directory.join("swift-latest.xctoolchain"),
        )
        .unwrap();

        let findings = Toolchains
            .scan(&test_context(home.clone()), &Default::default())
            .unwrap();

        drop(sealed);
        let offered = findings
            .iter()
            .filter_map(Finding::target)
            .collect::<Vec<_>>();
        assert_eq!(offered, vec![readable.as_path()]);
        let errors = findings
            .iter()
            .filter_map(Finding::scan_error)
            .collect::<Vec<_>>();
        assert_eq!(errors.len(), 1, "{findings:?}");
        assert!(errors[0].contains("swift-2.xctoolchain"), "{errors:?}");
    }

    #[test]
    fn apply_rejects_forged_nested_toolchain_target() {
        let (_fixture, root) = target_temp("apply-nested");
        let home = root.canonicalize().unwrap();
        let directory = home.join("Library/Developer/Toolchains");
        let preserved = directory.join("swift-2.xctoolchain");
        let nested = directory.join("container/swift-1.xctoolchain");
        std::fs::create_dir_all(&preserved).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        symlink(
            "swift-2.xctoolchain",
            directory.join("swift-latest.xctoolchain"),
        )
        .unwrap();
        let finding = Finding::new(
            "forged nested Swift toolchain",
            Some(nested.clone()),
            0,
            "test",
            9,
            Action::Shred,
        );
        let outcome = Toolchains
            .apply(&[finding], &test_context(home.clone()))
            .unwrap();

        assert_eq!(outcome.summary.items_touched, 0);
        assert_eq!(outcome.errors.len(), 1);
        assert!(outcome.errors[0].contains("direct Toolchains children"));
        assert!(nested.exists());
        crate::ops::remove_test_path(root);
    }
}
